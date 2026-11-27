//! The methods every LLM role's own pages call on its mesh (I109d, fit's `mesh.RegisterAdmin`,
//! `role/settings.go` and `role/tokens.go`), the caller [`ROLE_LOCAL`](crate::model::ROLE_LOCAL) alone:
//!
//! ```text
//! /mesh/peers           {self, listen, conns, targets}
//! /mesh/dial            DialReq: a target added, or changed (a token replaces the password and
//!                       the password a token; neither keeps both)
//! /mesh/undial          {addr}: a target removed
//! /mesh/kick            {id}: the peer's links closed, at once; {"closed": n}
//! /mesh/listen          ListenReq: the listener changed (an empty password keeps the current
//!                       one, unless clear_passwords), or taken out with an empty addr
//! /mesh/limits/get|set  the member's limits, their defaults filled in
//! /mesh/tokens/list|create|expiry|revoke   the listener's link tokens
//! ```
//!
//! Every change is an edit of the role's member ([`edit_member`]), answered once the config with
//! it was applied: the config is the one source of the settings, and the mesh follows it. A
//! request that is not valid changes nothing. A CA file is read on the role's pool, never on the
//! caller's thread.

use std::sync::{Arc, Mutex, OnceLock};
use std::time::Duration;

use akit_config::value::Value;
use akit_roles::host::Host;
use akit_roles::linktoken::{self as lt, ExpiryReq, IssueReq, TokenError};

use super::{EDIT_TIMEOUT, count_setting, duration_setting, edit_member, lock, number, set};
use crate::json::{self, Obj, Req};
use crate::mesh::{Answer, Mesh, Router, WeakMesh, bad_request, validate_addr};
use crate::model::{CODE_INTERNAL, CODE_NOT_FOUND, CODE_UNAVAILABLE, DialReq, ListenConfig};
use crate::wire::Error;

/// `/mesh/dial`.
pub const METHOD_DIAL: &str = "/mesh/dial";
/// `/mesh/undial`.
pub const METHOD_UNDIAL: &str = "/mesh/undial";
/// `/mesh/kick`.
pub const METHOD_KICK: &str = "/mesh/kick";
/// `/mesh/listen`.
pub const METHOD_LISTEN: &str = "/mesh/listen";
/// `/mesh/limits/get`.
pub const METHOD_LIMITS: &str = "/mesh/limits/get";
/// `/mesh/limits/set`.
pub const METHOD_LIMITS_SET: &str = "/mesh/limits/set";
/// `/mesh/tokens/list`.
pub const METHOD_TOKENS_LIST: &str = "/mesh/tokens/list";
/// `/mesh/tokens/create`.
pub const METHOD_TOKENS_CREATE: &str = "/mesh/tokens/create";
/// `/mesh/tokens/expiry`.
pub const METHOD_TOKENS_EXPIRY: &str = "/mesh/tokens/expiry";
/// `/mesh/tokens/revoke`.
pub const METHOD_TOKENS_REVOKE: &str = "/mesh/tokens/revoke";

/// The dial targets a role keeps at most (fit's `mesh.MaxTargets`).
pub const MAX_TARGETS: usize = akit_config::roles::MAX_LLM_PEERS;

/// What the methods share: the role's mesh (set once it is made), its host and its pool.
#[derive(Clone)]
pub(crate) struct Ctx {
    pub(crate) mesh: Arc<OnceLock<WeakMesh>>,
    pub(crate) host: Host,
    pub(crate) role: &'static str,
    pub(crate) work: Arc<akio::Pool>,
}

impl Ctx {
    fn mesh(&self) -> Result<Mesh, Error> {
        self.mesh
            .get()
            .and_then(WeakMesh::upgrade)
            .ok_or(Error::Closed)
    }

    /// Runs `f` on the role's pool: what reads files. 503 when the pool is full.
    fn off_thread(&self, a: &Answer, f: impl FnOnce() + Send + 'static) {
        if self.work.try_execute(Box::new(f)).is_err() {
            a.err(Error::code(
                CODE_UNAVAILABLE,
                "the role is busy; try again in a moment",
            ));
        }
    }

    fn edit(
        &self,
        change: impl FnOnce(&mut Vec<(String, Value)>) -> Result<(), Error> + Send + 'static,
        done: impl FnOnce(Result<(), Error>) + Send + 'static,
    ) {
        edit_member(&self.host, self.role, change, done);
    }
}

/// `/mesh/peers`' answer (fit's `PeersResp`): the role's identity, its listener, its links and
/// its dial targets, without passwords or tokens.
#[must_use]
pub fn peers_json(m: &Mesh) -> String {
    Obj::new()
        .raw("self", &m.identity().to_json())
        .raw("listen", &m.listen_state().to_json())
        .raw(
            "conns",
            &json::array(m.peers().iter().map(crate::model::PeerConn::to_json)),
        )
        .raw(
            "targets",
            &json::array(m.targets().iter().map(crate::model::DialTarget::to_json)),
        )
        .end()
}

// ---- The member's listener and peers, as fit's `EditMesh` reads and writes them ----

/// The mesh's settings as the member holds them (fit's `mesh.Settings`).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Settings {
    /// `listen`, `None` without one.
    pub listen: Option<ListenConfig>,
    /// `peers`.
    pub peers: Vec<DialReq>,
}

fn field_str(o: &[(String, Value)], what: &str, k: &str) -> Result<String, Error> {
    match o.iter().find(|(n, _)| n == k).map(|(_, v)| v) {
        None | Some(Value::Null) => Ok(String::new()),
        Some(Value::String(s)) => Ok(s.clone()),
        Some(_) => Err(bad_request(format!("{what}.{k} is not a string"))),
    }
}

fn field_bool(o: &[(String, Value)], what: &str, k: &str) -> Result<bool, Error> {
    match o.iter().find(|(n, _)| n == k).map(|(_, v)| v) {
        None | Some(Value::Null) => Ok(false),
        Some(Value::Bool(b)) => Ok(*b),
        Some(_) => Err(bad_request(format!("{what}.{k} is not a boolean"))),
    }
}

/// Reads the member's `listen` and `peers`.
///
/// # Errors
///
/// A member that is not what the config says it is.
pub fn read_settings(m: &[(String, Value)]) -> Result<Settings, Error> {
    let mut s = Settings::default();
    match m.iter().find(|(k, _)| k == "listen").map(|(_, v)| v) {
        None | Some(Value::Null) => {}
        Some(Value::Object(o)) => {
            s.listen = Some(ListenConfig {
                addr: field_str(o, "listen", "address")?,
                password: field_str(o, "listen", "password")?,
                node_password: field_str(o, "listen", "node_password")?,
                proxy_password: field_str(o, "listen", "proxy_password")?,
                hub_password: field_str(o, "listen", "hub_password")?,
                cert_file: field_str(o, "listen", "cert_file")?,
                key_file: field_str(o, "listen", "key_file")?,
                self_signed: field_bool(o, "listen", "self_signed")?,
                cert: field_str(o, "listen", "cert")?,
                key: field_str(o, "listen", "key")?,
            });
        }
        Some(_) => return Err(bad_request("listen is not an object")),
    }
    match m.iter().find(|(k, _)| k == "peers").map(|(_, v)| v) {
        None | Some(Value::Null) => {}
        Some(Value::Array(ps)) => {
            for p in ps {
                let Value::Object(o) = p else {
                    return Err(bad_request("peers holds what is not a peer"));
                };
                s.peers.push(DialReq {
                    addr: field_str(o, "peers", "address")?,
                    password: field_str(o, "peers", "password")?,
                    token: field_str(o, "peers", "token")?,
                    role: field_str(o, "peers", "role")?,
                    pin: field_str(o, "peers", "pin")?,
                    ca_file: field_str(o, "peers", "ca_file")?,
                    server_name: field_str(o, "peers", "server_name")?,
                });
            }
        }
        Some(_) => return Err(bad_request("peers is not a list")),
    }
    Ok(s)
}

fn put(o: &mut Vec<(String, Value)>, k: &str, v: &str) {
    if !v.is_empty() {
        o.push((k.to_owned(), Value::String(v.to_owned())));
    }
}

/// Writes `s` into the member, as fit writes a `config.LLMListen` and `config.LLMPeer`s: the
/// empty fields left out, `address` and `role` always; no peer, no `peers`.
pub fn write_settings(m: &mut Vec<(String, Value)>, s: &Settings) {
    match &s.listen {
        None => m.retain(|(k, _)| k != "listen"),
        Some(l) => {
            let mut o = vec![("address".to_owned(), Value::String(l.addr.clone()))];
            put(&mut o, "password", &l.password);
            put(&mut o, "node_password", &l.node_password);
            put(&mut o, "proxy_password", &l.proxy_password);
            put(&mut o, "hub_password", &l.hub_password);
            put(&mut o, "cert_file", &l.cert_file);
            put(&mut o, "key_file", &l.key_file);
            if l.self_signed {
                o.push(("self_signed".to_owned(), Value::Bool(true)));
            }
            put(&mut o, "cert", &l.cert);
            put(&mut o, "key", &l.key);
            match m.iter_mut().find(|(k, _)| k == "listen") {
                Some((_, v)) => *v = Value::Object(o),
                None => m.push(("listen".to_owned(), Value::Object(o))),
            }
        }
    }
    let peers: Vec<Value> = s
        .peers
        .iter()
        .map(|p| {
            let mut o = vec![("address".to_owned(), Value::String(p.addr.clone()))];
            put(&mut o, "token", &p.token);
            put(&mut o, "password", &p.password);
            o.push(("role".to_owned(), Value::String(p.role.clone())));
            put(&mut o, "pin", &p.pin);
            put(&mut o, "ca_file", &p.ca_file);
            put(&mut o, "server_name", &p.server_name);
            Value::Object(o)
        })
        .collect();
    set(m, "peers", Value::Array(peers));
}

/// Changes the member's listener and peers with `f` (fit's `Base.EditMesh`).
fn edit_mesh(
    c: &Ctx,
    f: impl FnOnce(&mut Settings) -> Result<(), Error> + Send + 'static,
    done: impl FnOnce(Result<(), Error>) + Send + 'static,
) {
    c.edit(
        move |m| {
            let mut s = read_settings(m)?;
            f(&mut s)?;
            write_settings(m, &s);
            Ok(())
        },
        done,
    );
}

fn dial_of(r: &Req) -> Result<DialReq, Error> {
    Ok(DialReq {
        addr: r.str("addr")?,
        password: r.str("password")?,
        token: r.str("token")?,
        role: r.str("role")?,
        pin: r.str("pin")?,
        ca_file: r.str("ca_file")?,
        server_name: r.str("server_name")?,
    })
}

/// Applies a dial request to the targets (fit's): the target at `addr` changed, or one added.
fn dial_into(s: &mut Settings, addr: &str, req: &DialReq) -> Result<(), Error> {
    if let Some(p) = s.peers.iter_mut().find(|p| p.addr == addr) {
        if !req.token.is_empty() {
            p.token.clone_from(&req.token);
            p.password.clear();
        } else if !req.password.is_empty() {
            p.token.clear();
            p.password.clone_from(&req.password);
        }
        p.role.clone_from(&req.role);
        p.pin.clone_from(&req.pin);
        p.ca_file.clone_from(&req.ca_file);
        p.server_name.clone_from(&req.server_name);
        return Ok(());
    }
    if req.password.is_empty() && req.token.is_empty() {
        return Err(bad_request("token or password required"));
    }
    if s.peers.len() >= MAX_TARGETS {
        return Err(bad_request(format!("at most {MAX_TARGETS} dial targets")));
    }
    s.peers.push(DialReq {
        addr: addr.to_owned(),
        password: if req.token.is_empty() {
            req.password.clone()
        } else {
            String::new()
        },
        ..req.clone()
    });
    Ok(())
}

/// Applies a listen request to the listener (fit's): `None` for an empty address.
fn listen_into(s: &mut Settings, mut cfg: ListenConfig, clear: bool) -> Result<(), Error> {
    if cfg.addr.is_empty() {
        s.listen = None;
        return Ok(());
    }
    if let Some(l) = &s.listen {
        if !clear {
            for (v, cur) in [
                (&mut cfg.password, &l.password),
                (&mut cfg.node_password, &l.node_password),
                (&mut cfg.proxy_password, &l.proxy_password),
                (&mut cfg.hub_password, &l.hub_password),
            ] {
                if v.is_empty() {
                    v.clone_from(cur);
                }
            }
        }
    }
    if clear {
        cfg.password.clear();
        cfg.node_password.clear();
        cfg.proxy_password.clear();
        cfg.hub_password.clear();
    }
    // A self-signed certificate already made is kept while the listener stays self-signed, so
    // that its peers' pins still match.
    if cfg.cert.is_empty() && cfg.self_signed && cfg.cert_file.is_empty() {
        if let Some(l) = &s.listen {
            if l.self_signed && l.cert_file.is_empty() {
                cfg.cert.clone_from(&l.cert);
                cfg.key.clone_from(&l.key);
            }
        }
    }
    let host = cfg.addr.rsplit_once(':').map_or("", |(h, _)| h);
    if !akit_config::roles::loopback(host) && !cfg.tls() {
        return Err(bad_request(format!(
            "{} is not a loopback address: set cert_file and key_file, or self_signed, so that links are taken over TLS and peers can check whose listener they reach",
            cfg.addr
        )));
    }
    s.listen = Some(cfg);
    Ok(())
}

/// The member's limits, their defaults filled in (fit's `model.Limits`).
fn limits_json(m: &Mesh) -> String {
    let l = m.limits();
    let d = akit_config::duration::format;
    Obj::new()
        .int("max_conns", i64::try_from(l.max_conns).unwrap_or(i64::MAX))
        .int(
            "max_inflight",
            i64::try_from(l.max_inflight).unwrap_or(i64::MAX),
        )
        .str("ping_interval", &d(l.ping))
        .str("slow_timeout", &d(l.slow))
        .str("retry_interval", &d(l.retry))
        .str("stream_idle", &d(l.stream_idle))
        .end()
}

/// The limits object to write, its empty and default fields left out (fit's `setLimits`).
fn limits_value(r: &Req) -> Result<Value, Error> {
    use akit_config::roles as cr;
    let n = |d: usize| i64::try_from(d).unwrap_or(i64::MAX);
    let conns = count_setting(
        "max_conns",
        r.int("max_conns")?,
        n(cr::DEFAULT_LLM_MAX_CONNS),
    )?;
    let inflight = count_setting(
        "max_inflight",
        r.int("max_inflight")?,
        n(cr::DEFAULT_LLM_MAX_INFLIGHT),
    )?;
    let ms = Duration::from_millis;
    let ping = duration_setting(
        "ping_interval",
        &r.str("ping_interval")?,
        ms(100),
        cr::DEFAULT_LLM_PING_INTERVAL,
    )?;
    let slow = duration_setting(
        "slow_timeout",
        &r.str("slow_timeout")?,
        Duration::ZERO,
        cr::DEFAULT_LLM_SLOW_TIMEOUT,
    )?;
    let retry = duration_setting(
        "retry_interval",
        &r.str("retry_interval")?,
        ms(100),
        cr::DEFAULT_LLM_RETRY_INTERVAL,
    )?;
    let idle = duration_setting(
        "stream_idle",
        &r.str("stream_idle")?,
        Duration::from_secs(1),
        cr::DEFAULT_LLM_STREAM_IDLE,
    )?;
    let mut o = Vec::new();
    for (k, v) in [("max_conns", conns), ("max_inflight", inflight)] {
        if v != 0 {
            o.push((k.to_owned(), number(v)));
        }
    }
    for (k, v) in [
        ("ping_interval", ping),
        ("slow_timeout", slow),
        ("retry_interval", retry),
        ("stream_idle", idle),
    ] {
        if !v.is_empty() {
            o.push((k.to_owned(), Value::String(v)));
        }
    }
    Ok(Value::Object(o))
}

fn token_err(e: &TokenError) -> Error {
    match e {
        TokenError::NotFound => Error::code(CODE_NOT_FOUND, e.to_string()),
        TokenError::BadRequest(m) => bad_request(m.clone()),
        e => Error::Other(e.to_string()),
    }
}

/// Changes the member's `tokens` with `f`, which answers what the method answers.
fn edit_tokens(
    c: &Ctx,
    a: Answer,
    f: impl FnOnce(&mut Vec<akit_config::roles::LinkToken>) -> Result<String, Error> + Send + 'static,
) {
    let out: Arc<Mutex<String>> = Arc::default();
    let o2 = Arc::clone(&out);
    c.edit(
        move |m| {
            let mut toks = lt::tokens_of(&Value::Object(m.clone()));
            *lock(&o2) = f(&mut toks)?;
            set(m, "tokens", lt::tokens_value(&toks));
            Ok(())
        },
        move |r| match r {
            Ok(()) => a.ok(std::mem::take(&mut *lock(&out))),
            Err(e) => a.err(e),
        },
    );
}

fn with(c: &Ctx, a: Answer, f: impl FnOnce(&Mesh, Answer)) {
    match c.mesh() {
        Ok(m) => f(&m, a),
        Err(e) => a.err(e),
    }
}

/// Parses `req`, or answers why not.
fn parsed(req: &[u8], a: &Answer) -> Option<Req> {
    match Req::parse(req) {
        Ok(r) => Some(r),
        Err(e) => {
            a.err(e);
            None
        }
    }
}

/// Registers the methods on `r`, for the role's own pages alone.
#[allow(clippy::too_many_lines)]
pub(crate) fn register(r: &mut Router, c: &Ctx) {
    let x = c.clone();
    r.handle(
        crate::model::METHOD_PEERS,
        &[],
        Duration::ZERO,
        move |_, _, a| with(&x, a, |m, a| a.ok(peers_json(m))),
    );
    let x = c.clone();
    r.handle(METHOD_DIAL, &[], EDIT_TIMEOUT, move |_, req, a| {
        let Some(rq) = parsed(req, &a) else { return };
        let checked = dial_of(&rq).and_then(|d| {
            let addr = validate_addr(&d.addr, true)?;
            x.mesh()?.validate_role(&d.role)?;
            if !d.token.is_empty() {
                lt::Secret::parse(&d.token).map_err(bad_request)?;
            }
            Ok((addr, d))
        });
        let (addr, d) = match checked {
            Ok(v) => v,
            Err(e) => return a.err(e),
        };
        // How the peer's listener is checked comes whole with the request; a CA file is read
        // on the role's pool.
        let y = x.clone();
        let a2 = a.clone();
        x.off_thread(&a, move || {
            let host = addr.rsplit_once(':').map_or("", |(h, _)| h);
            let host = host.trim_start_matches('[').trim_end_matches(']');
            if let Err(e) = crate::mesh::check_dial_tls(&d, host) {
                return a2.err(e);
            }
            let z = y.clone();
            edit_mesh(
                &y,
                move |s| dial_into(s, &addr, &d),
                move |r| match r.and_then(|()| z.mesh()) {
                    Ok(m) => a2.ok(peers_json(&m)),
                    Err(e) => a2.err(e),
                },
            );
        });
    });
    let x = c.clone();
    r.handle(METHOD_UNDIAL, &[], EDIT_TIMEOUT, move |_, req, a| {
        let Some(rq) = parsed(req, &a) else { return };
        let addr = match rq.str("addr") {
            Ok(v) => v,
            Err(e) => return a.err(e),
        };
        let y = x.clone();
        edit_mesh(
            &x,
            move |s| {
                let i =
                    s.peers.iter().position(|p| p.addr == addr).ok_or_else(|| {
                        Error::code(CODE_NOT_FOUND, format!("no dial target {addr}"))
                    })?;
                s.peers.remove(i);
                Ok(())
            },
            move |r| match r.and_then(|()| y.mesh()) {
                Ok(m) => a.ok(peers_json(&m)),
                Err(e) => a.err(e),
            },
        );
    });
    let x = c.clone();
    r.handle(METHOD_KICK, &[], Duration::ZERO, move |_, req, a| {
        let Some(rq) = parsed(req, &a) else { return };
        let id = match rq.str("id") {
            Ok(v) => v,
            Err(e) => return a.err(e),
        };
        with(&x, a, |m, a| {
            let n = m.kick(&id);
            akit_observe::info!(Llm, "mesh: peer kicked: id={id} conns={n}");
            a.ok(Obj::new()
                .int("closed", i64::try_from(n).unwrap_or(i64::MAX))
                .end());
        });
    });
    let x = c.clone();
    r.handle(METHOD_LISTEN, &[], EDIT_TIMEOUT, move |_, req, a| {
        let Some(rq) = parsed(req, &a) else { return };
        let read = || -> Result<(ListenConfig, bool), Error> {
            let mut cfg = ListenConfig {
                addr: rq.str("addr")?,
                password: rq.str("password")?,
                node_password: rq.str("node_password")?,
                proxy_password: rq.str("proxy_password")?,
                hub_password: rq.str("hub_password")?,
                cert_file: rq.str("cert_file")?,
                key_file: rq.str("key_file")?,
                self_signed: rq.bool("self_signed")?,
                cert: rq.str("cert")?,
                key: rq.str("key")?,
            };
            if !cfg.addr.is_empty() {
                cfg.addr = validate_addr(&cfg.addr, false)?;
            }
            Ok((cfg, rq.bool("clear_passwords")?))
        };
        let (cfg, clear) = match read() {
            Ok(v) => v,
            Err(e) => return a.err(e),
        };
        let y = x.clone();
        edit_mesh(
            &x,
            move |s| listen_into(s, cfg, clear),
            move |r| {
                let m = match y.mesh() {
                    Ok(m) => m,
                    Err(e) => return a.err(e),
                };
                // The setting is kept as written when it does not listen; the pages show why.
                let failed = m.listen_state().error;
                match r {
                    Ok(()) if failed.is_empty() => a.ok(peers_json(&m)),
                    Ok(()) => a.err(Error::code(CODE_INTERNAL, failed)),
                    Err(e) if !failed.is_empty() => {
                        akit_observe::warn!(Llm, "mesh: the listener did not start: {e}");
                        a.err(Error::code(CODE_INTERNAL, failed));
                    }
                    Err(e) => a.err(e),
                }
            },
        );
    });
    let x = c.clone();
    r.handle(METHOD_LIMITS, &[], Duration::ZERO, move |_, _, a| {
        with(&x, a, |m, a| a.ok(limits_json(m)));
    });
    let x = c.clone();
    r.handle(METHOD_LIMITS_SET, &[], EDIT_TIMEOUT, move |_, req, a| {
        let Some(rq) = parsed(req, &a) else { return };
        let l = match limits_value(&rq) {
            Ok(v) => v,
            Err(e) => return a.err(e),
        };
        let y = x.clone();
        x.edit(
            move |m| {
                set(m, "limits", l);
                Ok(())
            },
            move |r| match r.and_then(|()| y.mesh()) {
                Ok(m) => a.ok(limits_json(&m)),
                Err(e) => a.err(e),
            },
        );
    });
    let x = c.clone();
    r.handle(METHOD_TOKENS_LIST, &[], Duration::ZERO, move |_, _, a| {
        with(&x, a, |m, a| {
            a.ok(format!(
                "{{\"tokens\":{}}}",
                lt::list_json(&m.tokens().list())
            ));
        });
    });
    let x = c.clone();
    r.handle(METHOD_TOKENS_CREATE, &[], EDIT_TIMEOUT, move |_, req, a| {
        let Some(rq) = parsed(req, &a) else { return };
        let read = || -> Result<IssueReq, Error> {
            Ok(IssueReq {
                name: rq.str("name")?,
                note: rq.str("note")?,
                expires_at: rq.int("expires_at")?,
                ttl: rq.str("ttl")?,
            })
        };
        let ir = match read() {
            Ok(v) => v,
            Err(e) => return a.err(e),
        };
        let role = x.role;
        edit_tokens(&x, a, move |toks| {
            let now = lt::unix_now();
            let is = lt::issue(toks, &ir, now).map_err(|e| token_err(&e))?;
            akit_observe::info!(
                Llm,
                "llm.{role}: link token {} issued, named {:?}",
                is.record.id,
                is.record.name
            );
            let view = lt::record_view(&is.record, now).to_json();
            Ok(format!(
                "{{\"token\":{},{}",
                json::string(&is.secret.token()),
                view.get(1..).unwrap_or("}")
            ))
        });
    });
    let x = c.clone();
    r.handle(METHOD_TOKENS_EXPIRY, &[], EDIT_TIMEOUT, move |_, req, a| {
        let Some(rq) = parsed(req, &a) else { return };
        let read = || -> Result<(String, ExpiryReq), Error> {
            Ok((
                rq.str("id")?,
                ExpiryReq {
                    expires_at: rq.int("expires_at")?,
                    ttl: rq.str("ttl")?,
                },
            ))
        };
        let (id, er) = match read() {
            Ok(v) => v,
            Err(e) => return a.err(e),
        };
        let role = x.role;
        edit_tokens(&x, a, move |toks| {
            let now = lt::unix_now();
            lt::set_expiry(toks, &id, &er, now).map_err(|e| token_err(&e))?;
            akit_observe::info!(Llm, "llm.{role}: link token {id}'s expiry changed");
            Ok(toks
                .iter()
                .find(|t| t.id == id)
                .map_or_else(|| "null".to_owned(), |t| lt::record_view(t, now).to_json()))
        });
    });
    let x = c.clone();
    r.handle(METHOD_TOKENS_REVOKE, &[], EDIT_TIMEOUT, move |_, req, a| {
        let Some(rq) = parsed(req, &a) else { return };
        let id = match rq.str("id") {
            Ok(v) => v,
            Err(e) => return a.err(e),
        };
        let role = x.role;
        edit_tokens(&x, a, move |toks| {
            lt::revoke(toks, &id).map_err(|e| token_err(&e))?;
            akit_observe::info!(Llm, "llm.{role}: link token {id} revoked");
            Ok("{\"ok\":true}".to_owned())
        });
    });
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::indexing_slicing)]
    use super::*;

    fn member(s: &str) -> Vec<(String, Value)> {
        match akit_config::value::parse(s).unwrap() {
            Value::Object(o) => o,
            _ => panic!("not an object"),
        }
    }

    /// fit's `TestEditMesh`: the member read and written back, the roles' own passwords among it.
    #[test]
    fn settings_read_and_written() {
        let mut m = member(
            r#"{"id": "x", "listen":{"address":"127.0.0.1:1","password":"p"},"peers":[{"address":"h:1","password":"q","role":"hub"}]}"#,
        );
        let mut s = read_settings(&m).unwrap();
        assert_eq!(s.listen.as_ref().unwrap().addr, "127.0.0.1:1");
        assert_eq!(s.peers[0].password, "q");
        s.listen = None;
        s.peers.push(DialReq {
            addr: "h:2".into(),
            role: "proxy".into(),
            ..DialReq::default()
        });
        write_settings(&mut m, &s);
        let out = json::write(&Value::Object(m.clone()));
        assert_eq!(
            out,
            r#"{"id":"x","peers":[{"address":"h:1","password":"q","role":"hub"},{"address":"h:2","role":"proxy"}]}"#
        );
        s.peers.clear();
        s.listen = Some(ListenConfig {
            addr: "127.0.0.1:2".into(),
            hub_password: "h".into(),
            ..ListenConfig::default()
        });
        write_settings(&mut m, &s);
        assert_eq!(
            json::write(&Value::Object(m)),
            r#"{"id":"x","listen":{"address":"127.0.0.1:2","hub_password":"h"}}"#
        );
        for bad in [r#"{"listen":1}"#, r#"{"peers":{}}"#, r#"{"peers":[1]}"#] {
            assert!(read_settings(&member(bad)).is_err(), "{bad}");
        }
    }

    /// fit's `TestAdminTokens` and `TestAdminEdges`: a token replaces the password and the
    /// password a token; neither keeps both; at most 64 targets.
    #[test]
    fn dial_requests() {
        let mut s = Settings::default();
        let d = |pw: &str, tok: &str| DialReq {
            addr: "127.0.0.1:9".into(),
            password: pw.into(),
            token: tok.into(),
            role: "hub".into(),
            ..DialReq::default()
        };
        assert!(dial_into(&mut s, "127.0.0.1:9", &d("", "")).is_err());
        dial_into(&mut s, "127.0.0.1:9", &d("pw", "tok")).unwrap();
        assert_eq!(
            (s.peers[0].password.as_str(), s.peers[0].token.as_str()),
            ("", "tok")
        );
        dial_into(&mut s, "127.0.0.1:9", &d("pw", "")).unwrap();
        assert_eq!(
            (s.peers[0].password.as_str(), s.peers[0].token.as_str()),
            ("pw", "")
        );
        dial_into(&mut s, "127.0.0.1:9", &d("", "")).unwrap();
        assert_eq!(s.peers[0].password, "pw");
        assert_eq!(s.peers.len(), 1);
        while s.peers.len() < MAX_TARGETS {
            s.peers.push(DialReq {
                addr: format!("127.0.0.2:{}", s.peers.len() + 10),
                password: "p".into(),
                ..DialReq::default()
            });
        }
        let e = dial_into(&mut s, "127.0.0.1:10", &d("a", "")).unwrap_err();
        assert!(e.to_string().contains("at most 64"), "{e}");
    }

    /// fit's `TestRegisterAdmin` and `TestAdminTokens` of the listener: an empty password keeps
    /// the current one, each role's apart, unless they are all cleared; a self-signed certificate
    /// kept; off loopback only over TLS.
    #[test]
    fn listen_requests() {
        let mut s = Settings::default();
        let l = |addr: &str| ListenConfig {
            addr: addr.into(),
            ..ListenConfig::default()
        };
        listen_into(
            &mut s,
            ListenConfig {
                password: "lpw".into(),
                ..l("127.0.0.1:1")
            },
            false,
        )
        .unwrap();
        listen_into(
            &mut s,
            ListenConfig {
                hub_password: "h".into(),
                ..l("127.0.0.1:2")
            },
            false,
        )
        .unwrap();
        listen_into(&mut s, l("127.0.0.1:2"), false).unwrap();
        assert_eq!(
            s.listen,
            Some(ListenConfig {
                password: "lpw".into(),
                hub_password: "h".into(),
                ..l("127.0.0.1:2")
            })
        );
        listen_into(&mut s, l("127.0.0.1:2"), true).unwrap();
        assert!(!s.listen.as_ref().unwrap().has_password());
        let e = listen_into(&mut s, l("192.0.2.1:0"), false).unwrap_err();
        assert!(
            e.to_string()
                .contains("set cert_file and key_file, or self_signed"),
            "{e}"
        );
        listen_into(
            &mut s,
            ListenConfig {
                self_signed: true,
                cert: "C".into(),
                key: "K".into(),
                ..l(":9")
            },
            false,
        )
        .unwrap();
        listen_into(
            &mut s,
            ListenConfig {
                self_signed: true,
                ..l(":9")
            },
            false,
        )
        .unwrap();
        assert_eq!(s.listen.as_ref().unwrap().cert, "C");
        listen_into(&mut s, l(""), false).unwrap();
        assert!(s.listen.is_none());
    }

    /// The identity's `http_url` from the member's `console`, as fit's `Console.URL` says it.
    #[test]
    fn console_urls() {
        let c = |a: &str, cert: Option<&str>| akit_config::Console {
            address: a.into(),
            cert_file: cert.map(Into::into),
            ..akit_config::Console::default()
        };
        let url = crate::role::console_url;
        assert_eq!(url(&c("127.0.0.1:9100", None)), "http://127.0.0.1:9100/");
        assert_eq!(
            url(&c("localhost:9", Some("c.pem"))),
            "https://127.0.0.1:9/"
        );
        assert_eq!(url(&c(":9", None)), "http://[::]:9/");
        assert_eq!(url(&c("127.0.0.1:0", None)), "");
    }

    /// fit's `TestLimitsSettings`: written without the defaults.
    #[test]
    fn limits_written_without_defaults() {
        let r = Req::parse(br#"{"max_inflight": 8, "slow_timeout": "1m", "retry_interval": "5s"}"#)
            .unwrap();
        assert_eq!(
            json::write(&limits_value(&r).unwrap()),
            r#"{"max_inflight":8,"slow_timeout":"1m0s"}"#
        );
        for bad in [
            r#"{"max_conns": -1}"#,
            r#"{"ping_interval": "1ms"}"#,
            r#"{"stream_idle": "x"}"#,
        ] {
            assert!(
                limits_value(&Req::parse(bad.as_bytes()).unwrap()).is_err(),
                "{bad}"
            );
        }
    }
}
