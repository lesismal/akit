//! fit's plugin ABI (fit's `plugin/wasm/abi.go`), as host functions of akio-wasm's [`Linker`].
//!
//! A plugin is a WASI reactor that exports `fit_on_request() i32`: 0 for the request to go on,
//! 1 when the plugin answered it. The host functions it imports, from the module `fit`, take
//! strings as a pointer and a length into the plugin's memory:
//!
//! ```text
//! header_get(kind, name, name_len, buf, buf_len) i32  the field's first value written to buf,
//!     up to buf_len bytes, and its length, or -1 when there is none; kind 0 is the request's
//!     fields, 1 the response's (as the plugin set them so far)
//! header_set(kind, name, name_len, value, value_len)
//! header_add(kind, name, name_len, value, value_len)
//! header_del(kind, name, name_len)
//! request_get(field, buf, buf_len) i32  as header_get, for field 0 the method, 1 the path
//!     (decoded), 2 the query, 3 the host, 4 the client's address, 5 the plugin's config
//! respond(status, body, body_len)  answer the request with status, body and the response
//!     fields the plugin set
//! log(level, msg, msg_len)  write msg to akit's log at level, as Go's log/slog numbers them:
//!     -4 debug, 0 info, 4 warn, 8 error
//! ```
//!
//! The request's fields change at once, for the plugin's later reads; the response's are kept
//! and applied as the response's header is settled, whoever writes it.
//!
//! A host function given what the ABI does not allow fails the call, which then goes as any
//! failed call does (`on_error`), as fit's do: a string or buffer outside the plugin's memory, a
//! name or value longer than [`MAX_FIELD`], more than [`MAX_OPS`] changes to the response's
//! fields, a status outside 200 to 599, a body longer than [`MAX_BODY`]. akit also fails, where
//! fit stores what it is given, a field name that is not an HTTP token, a field value holding a
//! line break or NUL (which would split the header akio writes), a name or value that is not
//! UTF-8, and more than [`MAX_OPS`] changes to the request's fields in one call (rule 11).

use std::io::Write as _;
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Instant;

use akio_http::Headers;
use akio_wasm::{Caller, LinkError, Linker, Trap, TypedFunc};

/// The module the host functions are imported from.
pub(crate) const HOST_MODULE: &str = "fit";
/// The function a plugin of fit's ABI exports, run for each request.
pub(crate) const EXPORT_ON_REQUEST: &str = "fit_on_request";
/// What `fit_on_request` returns once the plugin answered.
pub(crate) const ACTION_STOP: i32 = 1;

/// `header_*`'s kind: the request's fields.
pub(crate) const KIND_REQUEST: u32 = 0;
/// `header_*`'s kind: the response's fields (any kind but [`KIND_REQUEST`], as in fit).
#[cfg(test)]
pub(crate) const KIND_RESPONSE: u32 = 1;

/// `request_get`'s fields.
pub(crate) const FIELD_METHOD: u32 = 0;
pub(crate) const FIELD_PATH: u32 = 1;
pub(crate) const FIELD_QUERY: u32 = 2;
pub(crate) const FIELD_HOST: u32 = 3;
pub(crate) const FIELD_REMOTE: u32 = 4;
pub(crate) const FIELD_CONFIG: u32 = 5;

/// The longest field name or value, or log line, a plugin may pass: 64 KiB.
pub(crate) const MAX_FIELD: u32 = 64 << 10;
/// The most changes to the response's fields (and, in akit, to the request's) one call makes.
pub(crate) const MAX_OPS: usize = 256;
/// The longest body a plugin may answer with: 1 MiB.
pub(crate) const MAX_BODY: u32 = 1 << 20;
/// A plugin's log lines a second, past which they are dropped.
pub(crate) const MAX_LOGS: u32 = 100;

/// A change to the response's fields, made once its header is settled.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Op {
    /// `header_set`: the field replaced by one of this value.
    Set(String, String),
    /// `header_add`: a value added after the field's others.
    Add(String, String),
    /// `header_del`: the field removed.
    Del(String),
}

impl Op {
    fn name(&self) -> &str {
        match self {
            Op::Set(n, _) | Op::Add(n, _) | Op::Del(n) => n,
        }
    }
}

/// Makes `ops`, the plugin's changes to the response's fields, to `h`.
pub(crate) fn apply(ops: &[Op], h: &mut Headers) {
    for op in ops {
        match op {
            Op::Set(n, v) => h.set(n, v),
            Op::Add(n, v) => h.append(n, v),
            Op::Del(n) => h.remove(n),
        }
    }
}

/// One request, as a plugin's call sees and changes it: copied from the request on its I/O
/// thread, and handed back with the plugin's changes once the call returns.
#[derive(Debug, Default)]
pub(crate) struct Call {
    pub(crate) method: String,
    /// Decoded, as Go's `URL.Path`.
    pub(crate) path: String,
    /// Without its `?`.
    pub(crate) query: String,
    pub(crate) host: String,
    /// The client's address and port, as Go's `RemoteAddr`.
    pub(crate) remote: String,
    /// The request's fields, changed at once by the plugin.
    pub(crate) headers: Headers,
    /// How many changes the plugin made to `headers`; 0 leaves the request's alone.
    pub(crate) request_ops: usize,
    /// The plugin's changes to the response's fields.
    pub(crate) ops: Vec<Op>,
    /// The plugin's answer (`respond`): its status, 0 for none, and body.
    pub(crate) status: u16,
    pub(crate) body: Vec<u8>,
}

impl Call {
    /// The value the response's field `name` has so far, as the plugin set it: there is no
    /// response yet.
    fn response(&self, name: &str) -> Option<&str> {
        let mut v = None;
        for op in self.ops.iter().filter(|op| op.name() == name) {
            match op {
                Op::Set(_, val) => v = Some(val.as_str()),
                Op::Add(_, val) => v = v.or(Some(val.as_str())),
                Op::Del(_) => v = None,
            }
        }
        v
    }

    fn change(&mut self, kind: u32, op: Op) -> Result<(), Trap> {
        if kind == KIND_REQUEST {
            if self.request_ops >= MAX_OPS {
                return Err(abi("more than 256 changes to the request's fields"));
            }
            self.request_ops = self.request_ops.saturating_add(1);
            apply(std::slice::from_ref(&op), &mut self.headers);
            return Ok(());
        }
        if self.ops.len() >= MAX_OPS {
            return Err(abi("more than 256 changes to the response's fields"));
        }
        self.ops.push(op);
        Ok(())
    }
}

/// What a plugin's instances share: its config, and the bound on its log lines.
#[derive(Debug)]
pub(crate) struct Shared {
    /// `path`, naming the plugin in its log lines.
    pub(crate) path: String,
    /// `config`, which `request_get` gives.
    pub(crate) config: String,
    logs: Mutex<(u64, u32)>,
    started: Instant,
}

impl Shared {
    pub(crate) fn new(path: &str, config: &str) -> Shared {
        Shared {
            path: path.to_owned(),
            config: config.to_owned(),
            logs: Mutex::new((0, 0)),
            started: Instant::now(),
        }
    }

    /// Whether another line may be written this second: at most [`MAX_LOGS`].
    pub(crate) fn may_log(&self) -> bool {
        let second = self.started.elapsed().as_secs();
        let mut l = self.logs.lock().unwrap_or_else(PoisonError::into_inner);
        if l.0 != second {
            *l = (second, 0);
        }
        l.1 = l.1.saturating_add(1);
        l.1 <= MAX_LOGS
    }
}

/// The state of an instance's store: what its instances share, its `fit_on_request`, and the
/// call it runs.
#[derive(Debug)]
pub(crate) struct Host {
    pub(crate) shared: Arc<Shared>,
    pub(crate) on_request: Option<TypedFunc<(), i32>>,
    pub(crate) call: Option<Call>,
}

impl Host {
    pub(crate) fn new(shared: Arc<Shared>) -> Host {
        Host {
            shared,
            on_request: None,
            call: None,
        }
    }
}

/// How a host function fails the call that made it.
fn abi(msg: &str) -> Trap {
    Trap::Host(format!("wasm: {msg}").into())
}

type C<'a, 'b> = &'a mut Caller<'b, Host>;

/// The call the instance running a host function serves.
fn call_of<'a>(c: &'a mut Caller<'_, Host>) -> Result<&'a mut Call, Trap> {
    c.state_mut()
        .call
        .as_mut()
        .ok_or_else(|| abi("a host function called outside a request"))
}

/// `n` bytes of the plugin's memory at `ptr`, copied.
fn bytes(c: &Caller<'_, Host>, ptr: u32, n: u32, max: u32, what: &str) -> Result<Vec<u8>, Trap> {
    if n > max {
        return Err(abi(what));
    }
    c.read(ptr, n)
        .map(<[u8]>::to_vec)
        .map_err(|_| abi("a string outside the plugin's memory"))
}

/// A field's name or value, or a log line, copied.
fn text(c: &Caller<'_, Host>, ptr: u32, n: u32) -> Result<String, Trap> {
    let b = bytes(c, ptr, n, MAX_FIELD, "a field longer than 64 KiB")?;
    String::from_utf8(b).map_err(|_| abi("a field that is not UTF-8"))
}

/// A field's name, checked to be a token and made canonical as Go's
/// `textproto.CanonicalMIMEHeaderKey` makes it (`x-plugin` is `X-Plugin`).
fn name(c: &Caller<'_, Host>, ptr: u32, n: u32) -> Result<String, Trap> {
    let s = text(c, ptr, n)?;
    if s.is_empty() || !s.bytes().all(is_tchar) {
        return Err(abi("a field name that is not a token"));
    }
    Ok(canonical(&s))
}

/// A field's value, checked to hold no line break or NUL.
fn value(c: &Caller<'_, Host>, ptr: u32, n: u32) -> Result<String, Trap> {
    let s = text(c, ptr, n)?;
    if s.bytes().any(|b| matches!(b, b'\r' | b'\n' | 0)) {
        return Err(abi("a field value with a line break or NUL"));
    }
    Ok(s)
}

/// Whether `b` may be in a token (RFC 9110, section 5.6.2).
fn is_tchar(b: u8) -> bool {
    b.is_ascii_alphanumeric() || b"!#$%&'*+-.^_`|~".contains(&b)
}

/// `name`, a token, canonical: its first letter and each after a `-` upper case, the rest
/// lower case.
pub(crate) fn canonical(name: &str) -> String {
    let mut up = true;
    name.chars()
        .map(|ch| {
            let out = if up {
                ch.to_ascii_uppercase()
            } else {
                ch.to_ascii_lowercase()
            };
            up = ch == '-';
            out
        })
        .collect()
}

/// Writes `v` at `buf`, up to `cap` bytes, and returns `v`'s length, for the plugin to call
/// again with room for it.
fn give(c: &mut Caller<'_, Host>, buf: u32, cap: u32, v: &[u8]) -> Result<i32, Trap> {
    let n = v.len().min(usize::try_from(cap).unwrap_or(usize::MAX));
    if n > 0 {
        let part = v.get(..n).unwrap_or(v);
        c.write(buf, part)
            .map_err(|_| abi("a buffer outside the plugin's memory"))?;
    }
    Ok(i32::try_from(v.len()).unwrap_or(i32::MAX))
}

/// Adds fit's host functions to `l`.
pub(crate) fn link(l: &mut Linker<Host>) -> Result<(), LinkError> {
    l.func_wrap(
        HOST_MODULE,
        "header_get",
        |c: C<'_, '_>, (kind, np, nl, buf, cap): (u32, u32, u32, u32, u32)| {
            call_of(c)?;
            let n = name(c, np, nl)?;
            let call = call_of(c)?;
            let v = if kind == KIND_REQUEST {
                call.headers.get(&n).map(|v| v.as_bytes().to_vec())
            } else {
                call.response(&n).map(|v| v.as_bytes().to_vec())
            };
            match v {
                Some(v) => give(c, buf, cap, &v),
                None => Ok(-1),
            }
        },
    )?;
    for (fname, set) in [("header_set", true), ("header_add", false)] {
        l.func_wrap(
            HOST_MODULE,
            fname,
            move |c: C<'_, '_>, (kind, np, nl, vp, vl): (u32, u32, u32, u32, u32)| {
                call_of(c)?;
                let n = name(c, np, nl)?;
                let v = value(c, vp, vl)?;
                let op = if set { Op::Set(n, v) } else { Op::Add(n, v) };
                call_of(c)?.change(kind, op)
            },
        )?;
    }
    l.func_wrap(
        HOST_MODULE,
        "header_del",
        |c: C<'_, '_>, (kind, np, nl): (u32, u32, u32)| {
            call_of(c)?;
            let n = name(c, np, nl)?;
            call_of(c)?.change(kind, Op::Del(n))
        },
    )?;
    l.func_wrap(
        HOST_MODULE,
        "request_get",
        |c: C<'_, '_>, (field, buf, cap): (u32, u32, u32)| {
            let config = Arc::clone(&c.state().shared);
            let call = call_of(c)?;
            let v: Vec<u8> = match field {
                FIELD_METHOD => call.method.as_bytes().to_vec(),
                FIELD_PATH => call.path.as_bytes().to_vec(),
                FIELD_QUERY => call.query.as_bytes().to_vec(),
                FIELD_HOST => call.host.as_bytes().to_vec(),
                FIELD_REMOTE => call.remote.as_bytes().to_vec(),
                FIELD_CONFIG => config.config.as_bytes().to_vec(),
                _ => return Ok(-1),
            };
            give(c, buf, cap, &v)
        },
    )?;
    l.func_wrap(
        HOST_MODULE,
        "respond",
        |c: C<'_, '_>, (status, bp, bl): (u32, u32, u32)| {
            call_of(c)?;
            let status = u16::try_from(status)
                .ok()
                .filter(|s| (200..=599).contains(s))
                .ok_or_else(|| abi("a status out of range"))?;
            let body = bytes(c, bp, bl, MAX_BODY, "a body longer than 1 MiB")?;
            let call = call_of(c)?;
            call.status = status;
            call.body = body;
            Ok(())
        },
    )?;
    l.func_wrap(
        HOST_MODULE,
        "log",
        |c: C<'_, '_>, (level, mp, ml): (i32, u32, u32)| {
            call_of(c)?;
            // akit has no plugin logger of its own: lines at info and above are written, as
            // fit's plugin logger at its default level writes them.
            if level < 0 {
                return Ok(());
            }
            let msg = bytes(c, mp, ml, MAX_FIELD, "a field longer than 64 KiB")?;
            let shared = Arc::clone(&c.state().shared);
            if shared.may_log() {
                write_log(&shared.path, level, &msg);
            }
            Ok(())
        },
    )?;
    Ok(())
}

/// Writes a plugin's log line to standard error, its control characters escaped. On the pool's
/// thread, within the call's time.
fn write_log(path: &str, level: i32, msg: &[u8]) {
    let level = match level {
        i32::MIN..=-1 => "DEBUG",
        0..=3 => "INFO",
        4..=7 => "WARN",
        _ => "ERROR",
    };
    let msg: String = String::from_utf8_lossy(msg)
        .chars()
        .flat_map(|ch| {
            if ch.is_control() {
                ch.escape_default().collect::<Vec<_>>()
            } else {
                vec![ch]
            }
        })
        .collect();
    let _ = writeln!(
        std::io::stderr().lock(),
        "akit: wasm plugin {path}: {level}: {msg}"
    );
}

/// fit's `TestHostABI`: the host functions, called through a guest of the tests' own that
/// forwards to each, on a request of the tests'.
#[cfg(test)]
#[allow(
    clippy::unwrap_used,
    clippy::indexing_slicing,
    clippy::arithmetic_side_effects,
    clippy::cast_possible_truncation
)]
mod tests {
    use std::sync::Arc;
    use std::time::{Duration, Instant};

    use akio_wasm::{Budget, Extern, Instance, InstanceLimits, Module, ModuleLimits, Store, Value};

    use super::*;
    use crate::encode::{self, Func, Import};

    /// The host functions, with their params and results.
    const FUNCS: [(&str, usize, usize); 7] = [
        ("header_get", 5, 1),
        ("header_set", 5, 0),
        ("header_add", 5, 0),
        ("header_del", 3, 0),
        ("request_get", 3, 1),
        ("respond", 3, 0),
        ("log", 3, 0),
    ];

    /// Where the tests write strings, and read what was given.
    const STRINGS: u32 = 16;
    const BUF: u32 = 20000;
    const BAD: u32 = 0xffff_ff00;

    fn guest() -> Arc<Module> {
        let m = encode::Module {
            imports: FUNCS
                .iter()
                .map(|(n, p, r)| Import::new(HOST_MODULE, n, *p, *r))
                .collect(),
            funcs: FUNCS
                .iter()
                .enumerate()
                .map(|(i, (n, p, r))| Func::export(n, *p, *r, encode::forward(i as u32, *p)))
                .collect(),
            pages: 1,
        };
        Arc::new(Module::new(&m.encode(), &ModuleLimits::default()).unwrap())
    }

    /// A store with an instance of the guest, serving a request of the tests'.
    struct Env {
        store: Store<Host>,
        inst: Instance,
        next: u32,
    }

    impl Env {
        fn new() -> Env {
            let mut l = Linker::new();
            link(&mut l).unwrap();
            let shared = Arc::new(Shared::new("test.wasm", "the config"));
            let mut store = Store::new(Host::new(shared), InstanceLimits::default());
            let inst = l.instantiate(&mut store, &guest(), &budget()).unwrap();
            let mut headers = Headers::new();
            headers.append("X-A", "1");
            store.state_mut().call = Some(Call {
                method: "GET".into(),
                path: "/p/q".into(),
                query: "x=1".into(),
                host: "example.com".into(),
                remote: "10.0.0.1:5".into(),
                headers,
                ..Call::default()
            });
            Env {
                store,
                inst,
                next: STRINGS,
            }
        }

        /// Writes `s` into the guest's memory; its address and length.
        fn put(&mut self, s: &str) -> (u32, u32) {
            let p = self.next;
            let m = self.memory();
            self.store.memory_data_mut(m)[p as usize..p as usize + s.len()]
                .copy_from_slice(s.as_bytes());
            self.next += s.len() as u32;
            (p, s.len() as u32)
        }

        fn memory(&self) -> akio_wasm::Memory {
            match self.store.export(self.inst, "memory") {
                Some(Extern::Memory(m)) => m,
                _ => panic!("no memory"),
            }
        }

        fn str(&self, p: u32, n: u32) -> String {
            let m = self.memory();
            String::from_utf8(self.store.memory_data(m)[p as usize..(p + n) as usize].to_vec())
                .unwrap()
        }

        /// Calls the host function `name` with `args`: its result, or its trap's message.
        fn call(&mut self, name: &str, args: &[u32]) -> Result<i32, String> {
            let f = self.store.export_func(self.inst, name).unwrap();
            let args: Vec<Value> = args.iter().map(|&a| Value::I32(a as i32)).collect();
            let results = FUNCS.iter().find(|(n, ..)| *n == name).unwrap().2;
            let mut out = vec![Value::I32(0); results];
            self.store
                .call(f, &args, &mut out, &budget())
                .map_err(|t| t.message().to_owned())?;
            Ok(match out.first() {
                Some(Value::I32(v)) => *v,
                _ => 0,
            })
        }

        fn ok(&mut self, name: &str, args: &[u32]) {
            if let Err(e) = self.call(name, args) {
                panic!("{name}: trapped with {e}");
            }
        }

        fn call_state(&mut self) -> &mut Call {
            self.store.state_mut().call.as_mut().unwrap()
        }
    }

    fn budget() -> Budget {
        Budget::deadline(Instant::now() + Duration::from_secs(10))
    }

    /// Checks that calling `name` with what `args` makes of a fresh env traps with `want`.
    fn traps(what: &str, name: &str, args: impl Fn(&mut Env) -> Vec<u32>, want: &str) {
        let mut e = Env::new();
        let a = args(&mut e);
        match e.call(name, &a) {
            Err(m) if m.contains(want) => {
                assert!(
                    m.starts_with("wasm: "),
                    "{what}: {m} is no error of the ABI"
                );
            }
            r => panic!("{what}: {r:?}, want a trap with {want:?}"),
        }
    }

    #[test]
    fn outside_a_request() {
        let mut e = Env::new();
        e.store.state_mut().call = None;
        let r = e.call("header_get", &[0, 0, 0, 0, 0]);
        assert!(
            matches!(&r, Err(m) if m.contains("outside a request")),
            "{r:?}"
        );
    }

    #[test]
    fn host_abi() {
        let mut e = Env::new();
        let (xa, xa_len) = e.put("x-a");
        assert_eq!(
            e.call("header_get", &[KIND_REQUEST, xa, xa_len, BUF, 64]),
            Ok(1)
        );
        assert_eq!(e.str(BUF, 1), "1");
        let (none, none_len) = e.put("x-none");
        assert_eq!(
            e.call("header_get", &[KIND_REQUEST, none, none_len, BUF, 64]),
            Ok(-1)
        );
        // A buffer too small gets what fits, and the length to call again with.
        let (long, long_len) = e.put("x-long");
        let (v, v_len) = e.put("0123456789");
        e.ok("header_set", &[KIND_REQUEST, long, long_len, v, v_len]);
        assert_eq!(
            e.call("header_get", &[KIND_REQUEST, long, long_len, BUF, 4]),
            Ok(10)
        );
        assert_eq!(e.str(BUF, 4), "0123");

        // The response's fields, as the plugin sets them before there is one.
        let (xr, xr_len) = e.put("x-r");
        let (v1, v1_len) = e.put("v1");
        let (v2, v2_len) = e.put("v2");
        e.ok("header_set", &[KIND_RESPONSE, xr, xr_len, v1, v1_len]);
        e.ok("header_add", &[KIND_RESPONSE, xr, xr_len, v2, v2_len]);
        assert_eq!(
            e.call("header_get", &[KIND_RESPONSE, xr, xr_len, BUF, 64]),
            Ok(2)
        );
        assert_eq!(e.str(BUF, 2), "v1");
        e.ok("header_del", &[KIND_RESPONSE, xr, xr_len]);
        assert_eq!(
            e.call("header_get", &[KIND_RESPONSE, xr, xr_len, BUF, 64]),
            Ok(-1)
        );
        let (xn, xn_len) = e.put("x-n");
        e.ok("header_add", &[KIND_RESPONSE, xn, xn_len, v1, v1_len]);
        e.ok("header_add", &[KIND_RESPONSE, xn, xn_len, v2, v2_len]);
        assert_eq!(e.call_state().response("X-N"), Some("v1"));
        let mut h = Headers::new();
        h.append("X-R", "old");
        apply(&e.call_state().ops, &mut h);
        assert_eq!(h.get("x-r"), None, "{h:?}");
        assert_eq!(h.get_all("x-n").collect::<Vec<_>>(), ["v1", "v2"]);

        // The request's fields, changed at once.
        e.ok("header_add", &[KIND_REQUEST, xa, xa_len, v2, v2_len]);
        assert_eq!(
            e.call_state().headers.get_all("x-a").collect::<Vec<_>>(),
            ["1", "v2"]
        );
        e.ok("header_del", &[KIND_REQUEST, xa, xa_len]);
        assert_eq!(e.call_state().headers.get("x-a"), None);
        assert_eq!(e.call_state().request_ops, 3);
        // Names are canonical, as Go's.
        assert!(
            e.call_state()
                .headers
                .iter()
                .any(|(n, v)| n == "X-Long" && v == "0123456789")
        );

        for (field, want) in [
            (FIELD_METHOD, "GET"),
            (FIELD_PATH, "/p/q"),
            (FIELD_QUERY, "x=1"),
            (FIELD_HOST, "example.com"),
            (FIELD_REMOTE, "10.0.0.1:5"),
            (FIELD_CONFIG, "the config"),
        ] {
            let n = e.call("request_get", &[field, BUF, 64]);
            assert_eq!(n, Ok(want.len() as i32), "request_get {field}");
            assert_eq!(e.str(BUF, want.len() as u32), want);
        }
        assert_eq!(e.call("request_get", &[99, BUF, 64]), Ok(-1));

        let (body, body_len) = e.put("the body");
        e.ok("respond", &[201, body, body_len]);
        assert_eq!(e.call_state().status, 201);
        assert_eq!(e.call_state().body, b"the body");

        let (msg, msg_len) = e.put("a line");
        e.ok("log", &[0, msg, msg_len]);
        // Below info: dropped without a read, even of a string outside the memory.
        e.ok("log", &[(-4i32) as u32, BAD, 4]);
        let shared = Arc::clone(&e.store.state().shared);
        for _ in 0..MAX_LOGS * 2 {
            e.ok("log", &[0, msg, msg_len]);
        }
        assert!(!shared.may_log(), "lines past 100 a second are dropped");
    }

    #[test]
    fn host_abi_failures() {
        traps(
            "a buffer outside memory",
            "header_get",
            |e| {
                let (p, n) = e.put("x-a");
                vec![KIND_REQUEST, p, n, BAD, 10]
            },
            "a buffer outside",
        );
        traps(
            "a long name",
            "header_get",
            |_| vec![KIND_REQUEST, 0, MAX_FIELD + 1, BUF, 64],
            "longer than 64 KiB",
        );
        traps(
            "a name outside memory",
            "header_get",
            |_| vec![KIND_REQUEST, BAD, 4, BUF, 64],
            "outside the plugin's memory",
        );
        traps(
            "a name that is no token",
            "header_set",
            |e| {
                let (p, n) = e.put("x a");
                let (v, vn) = e.put("v");
                vec![KIND_REQUEST, p, n, v, vn]
            },
            "not a token",
        );
        traps(
            "a value that splits the header",
            "header_add",
            |e| {
                let (p, n) = e.put("x-a");
                let (v, vn) = e.put("v\r\nX-Evil: 1");
                vec![KIND_RESPONSE, p, n, v, vn]
            },
            "line break",
        );
        for status in [199, 600, 70000] {
            traps(
                "respond",
                "respond",
                |e| {
                    let (p, n) = e.put("b");
                    vec![status, p, n]
                },
                "status out of range",
            );
        }
        traps(
            "a long body",
            "respond",
            |_| vec![200, 0, MAX_BODY + 1],
            "longer than 1 MiB",
        );
        for kind in [KIND_REQUEST, KIND_RESPONSE] {
            traps(
                "too many changes",
                "header_add",
                move |e| {
                    let (p, n) = e.put("x-many");
                    let (v, vn) = e.put("v");
                    for _ in 0..MAX_OPS {
                        e.ok("header_add", &[kind, p, n, v, vn]);
                    }
                    vec![kind, p, n, v, vn]
                },
                "more than 256",
            );
        }
    }

    #[test]
    fn canonical_names() {
        assert_eq!(canonical("x-plugin"), "X-Plugin");
        assert_eq!(canonical("CONTENT-type"), "Content-Type");
        assert_eq!(canonical("a--b"), "A--B");
    }
}
