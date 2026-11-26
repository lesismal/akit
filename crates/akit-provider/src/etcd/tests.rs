//! fit's `provider/etcd` tests, against a fake of etcd's JSON gateway: `TestProvider`,
//! `TestProviderMaxKeys`, `TestProviderWatchNotCreated`, `TestProviderBadPassword`,
//! `TestRangeEnd`, `TestReadWatch`, `TestReadWatchNoResult`, `TestKind`, `TestOpenFails` and
//! `TestCallFails`.
#![allow(
    clippy::unwrap_used,
    clippy::panic,
    clippy::indexing_slicing,
    clippy::arithmetic_side_effects
)]

use std::collections::BTreeMap;
use std::sync::mpsc::{self, SyncSender};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use akio_crypto::base64::STANDARD;
use akit_config::value::{self, Value};

use super::{Etcd, EtcdSource, Lines, range_end, watch_message};
use crate::api::ApiEngine;
use crate::run::Provider;
use crate::testing::{self, Parts, Req, Resp, Server, short};
use crate::watch::{Source, Stop};

#[derive(Default)]
struct State {
    kv: BTreeMap<String, String>,
    rev: u64,
    /// Each watch's channel of messages to send; "" ends the stream.
    watchers: Vec<SyncSender<String>>,
    /// The user asked for, with password; none when empty.
    user: String,
    password: String,
    tokens: usize,
    seen: Vec<String>,
    /// Watches are never created.
    no_create: bool,
    closed: bool,
}

/// Answers the parts of etcd's JSON gateway the provider asks for.
struct FakeEtcd {
    st: Arc<Mutex<State>>,
    srv: Server,
}

impl Drop for FakeEtcd {
    fn drop(&mut self) {
        self.st.lock().unwrap().closed = true;
    }
}

fn b64(v: Option<&Value>) -> String {
    match v {
        Some(Value::String(s)) => String::from_utf8(STANDARD.decode(s).unwrap()).unwrap(),
        _ => String::new(),
    }
}

impl FakeEtcd {
    fn new() -> FakeEtcd {
        let st = Arc::new(Mutex::new(State {
            rev: 1,
            ..State::default()
        }));
        let s = Arc::clone(&st);
        let srv = Server::start(move |r, w| serve(&s, r, w));
        FakeEtcd { st, srv }
    }

    /// Sets `key`, or deletes it when `value` is empty, and tells the watches.
    fn put(&self, key: &str, value: &str) {
        let mut st = self.st.lock().unwrap();
        st.rev += 1;
        if value.is_empty() {
            st.kv.remove(key);
        } else {
            st.kv.insert(key.to_owned(), value.to_owned());
        }
        let msg = format!(
            r#"{{"result":{{"header":{{"revision":"{}"}},"events":[{{"kv":{{"key":"{}"}}}}]}}}}"#,
            st.rev,
            STANDARD.encode(key)
        );
        st.watchers.retain(|w| w.try_send(msg.clone()).is_ok());
    }

    /// Sends `msg` to every watch.
    fn broadcast(&self, msg: &str) {
        self.st
            .lock()
            .unwrap()
            .watchers
            .retain(|w| w.try_send(msg.to_owned()).is_ok());
    }

    fn requests(&self) -> Vec<String> {
        self.st.lock().unwrap().seen.clone()
    }
}

fn serve(st: &Arc<Mutex<State>>, r: &Req, mut w: Resp) {
    let body = std::str::from_utf8(&r.body)
        .ok()
        .and_then(|t| value::parse(t).ok());
    let Some(body) = body.filter(|_| r.method == "POST") else {
        w.send(400, br#"{"error":"bad request"}"#);
        return;
    };
    let mut s = st.lock().unwrap();
    let auth = r.header("Authorization").unwrap_or_default().to_owned();
    s.seen.push(format!("{} auth={auth}", r.path));
    if !s.user.is_empty() && r.path != "/v3/auth/authenticate" && !auth.starts_with("token-") {
        drop(s);
        w.send(
            400,
            br#"{"error":"etcdserver: user name is empty","code":3}"#,
        );
        return;
    }
    match r.path.as_str() {
        "/v3/auth/authenticate" => {
            let field = |k: &str| match body.get(k) {
                Some(Value::String(v)) => v.clone(),
                _ => String::new(),
            };
            if field("name") != s.user || field("password") != s.password {
                drop(s);
                w.send(
                    400,
                    br#"{"error":"etcdserver: authentication failed, invalid user ID or password","code":3}"#,
                );
                return;
            }
            s.tokens += 1;
            let tok = format!("token-{}", s.tokens);
            drop(s);
            w.send(
                200,
                format!(r#"{{"header":{{}},"token":"{tok}"}}"#).as_bytes(),
            );
        }
        "/v3/kv/range" => {
            let key = b64(body.get("key"));
            let end = b64(body.get("range_end"));
            let limit: usize = match body.get("limit") {
                Some(Value::Number(n)) => n.parse().unwrap(),
                _ => 0,
            };
            let mut kvs: Vec<(String, String)> =
                s.kv.iter()
                    .filter(|(k, _)| **k >= key && (end == "\0" || **k < end))
                    .map(|(k, v)| (k.clone(), v.clone()))
                    .collect();
            let more = limit > 0 && kvs.len() > limit;
            if more {
                kvs.truncate(limit);
            }
            let list: Vec<String> = kvs
                .iter()
                .map(|(k, v)| {
                    format!(
                        r#"{{"key":"{}","value":"{}"}}"#,
                        STANDARD.encode(k),
                        STANDARD.encode(v)
                    )
                })
                .collect();
            let out = format!(
                r#"{{"header":{{"revision":"{}"}},"kvs":[{}],"more":{more},"count":"{}"}}"#,
                s.rev,
                list.join(","),
                kvs.len()
            );
            drop(s);
            w.send(200, out.as_bytes());
        }
        "/v3/watch" => {
            let (tx, rx) = mpsc::sync_channel(16);
            if !s.no_create {
                tx.try_send(r#"{"result":{"header":{"revision":"1"},"created":true}}"#.to_owned())
                    .unwrap();
            }
            s.watchers.push(tx);
            drop(s);
            if w.stream(200).is_err() {
                return;
            }
            loop {
                match rx.recv_timeout(Duration::from_millis(100)) {
                    Ok(msg) if msg.is_empty() => return, // ends the stream
                    Ok(msg) => {
                        if w.chunk(format!("{msg}\n").as_bytes()).is_err() {
                            return;
                        }
                    }
                    Err(_) if st.lock().unwrap().closed => return,
                    Err(_) => {}
                }
            }
        }
        _ => {
            drop(s);
            w.send(404, b"404 page not found\n");
        }
    }
}

fn route(name: &str) -> String {
    format!(r#"{{"routes": [{{"name": "{name}", "handler": {{"module": "respond"}}}}]}}"#)
}

fn routes_part(names: &[&str]) -> String {
    if names.is_empty() {
        return "{}".to_owned();
    }
    let rs: Vec<String> = names
        .iter()
        .map(|n| format!(r#"{{"name":"{n}","handler":{{"module":"respond"}}}}"#))
        .collect();
    format!(r#"{{"routes":[{}]}}"#, rs.join(","))
}

fn opts(endpoints: &[&str]) -> akit_config::EtcdProvider {
    akit_config::EtcdProvider {
        endpoints: endpoints.iter().map(|e| (*e).to_owned()).collect(),
        username: String::new(),
        password_file: String::new(),
        tls: akit_config::ClientTlsFiles::default(),
        prefix: akit_config::DEFAULT_KV_PREFIX.to_owned(),
        max_keys: akit_config::DEFAULT_KV_MAX_KEYS,
        refresh_interval: Duration::from_secs(3600),
        request_timeout: Duration::from_secs(10),
    }
}

fn tmp_file(name: &str, data: &str) -> (std::path::PathBuf, String) {
    let dir = std::env::temp_dir().join(format!(
        "akit-etcd-{name}-{}-{:?}",
        std::process::id(),
        std::thread::current().id()
    ));
    std::fs::create_dir_all(&dir).unwrap();
    let p = dir.join(name);
    std::fs::write(&p, data).unwrap();
    (dir, p.display().to_string())
}

fn start(p: &mut Etcd) -> Parts {
    p.set_timings(short());
    let (parts, update) = Parts::new(p.name());
    p.start(update).unwrap();
    parts
}

#[test]
fn provider() {
    let f = FakeEtcd::new();
    {
        let mut st = f.st.lock().unwrap();
        st.user = "akit".to_owned();
        st.password = "pa ss".to_owned();
    }
    f.put("akit/a", &route("a"));
    f.put("akit/bad", "nope");
    f.put("fiu", &route("outside")); // past the prefix's range
    f.put("fis", &route("before"));
    let (dir, pw) = tmp_file("password", "pa ss\n");
    let lines = testing::Lines::default();
    // The first endpoint is not there: the provider goes on to the next.
    let url = format!("{}/", f.srv.url);
    let mut o = opts(&["http://127.0.0.1:1", &url]);
    o.username = "akit".to_owned();
    o.password_file = pw;
    let mut p = Etcd::new("etcd", &o, lines.log());
    assert_eq!(p.name(), "etcd");
    let parts = start(&mut p);
    parts.want(&routes_part(&["a"]));
    f.put("akit/b", &route("b"));
    parts.want(&routes_part(&["a", "b"]));
    f.put("akit/a", "");
    parts.want(&routes_part(&["b"]));
    // Progress notifications, with no events, read nothing.
    let before = f.requests().len();
    f.broadcast(r#"{"result":{"header":{"revision":"9"}}}"#);
    std::thread::sleep(Duration::from_millis(100));
    assert_eq!(
        f.requests().len(),
        before,
        "a progress notification made requests"
    );
    // The watch is canceled, as after a compaction: the provider watches again, on the
    // endpoints in turn, and reads at once.
    f.broadcast(
        r#"{"result":{"header":{},"canceled":true,"compact_revision":"5","cancel_reason":"mvcc: required revision has been compacted"}}"#,
    );
    f.put("akit/c", &route("c"));
    parts.want(&routes_part(&["b", "c"]));
    // The stream ends.
    f.broadcast("");
    f.put("akit/b", "");
    parts.want(&routes_part(&["c"]));
    p.stop();
    let all = lines.all();
    for want in [
        "provider etcd: key akit/bad: ",
        "127.0.0.1:1",
        "etcd canceled the watch: mvcc: required revision has been compacted (compacted at revision 5)",
        "etcd ended the watch",
    ] {
        assert!(all.contains(want), "logs lack {want:?}:\n{all}");
    }
    for r in f.requests() {
        assert!(
            r.starts_with("/v3/auth/authenticate auth=") || r.contains("auth=token-"),
            "a request without a token: {r}"
        );
    }
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn provider_max_keys() {
    let f = FakeEtcd::new();
    f.put("akit/1", &route("one"));
    f.put("akit/2", &route("two"));
    f.put("akit/3", &route("three"));
    let lines = testing::Lines::default();
    let mut o = opts(&[&f.srv.url]);
    o.max_keys = 2;
    let mut p = Etcd::new("etcd", &o, lines.log());
    let parts = start(&mut p);
    parts.want(&routes_part(&["one", "two"]));
    lines.wait("provider etcd: the keys past the first 2 under akit/ are left out: max_keys");
}

#[test]
fn provider_watch_not_created() {
    let f = FakeEtcd::new();
    f.st.lock().unwrap().no_create = true;
    let lines = testing::Lines::default();
    let mut o = opts(&[&f.srv.url]);
    o.request_timeout = Duration::from_millis(50);
    let mut p = Etcd::new("etcd", &o, lines.log());
    let parts = start(&mut p);
    lines.wait("the watch was not created within 50ms");
    assert!(parts.rx.try_recv().is_err(), "a part without a watch");
}

#[test]
fn provider_bad_password() {
    let f = FakeEtcd::new();
    {
        let mut st = f.st.lock().unwrap();
        st.user = "akit".to_owned();
        st.password = "right".to_owned();
    }
    let (dir, pw) = tmp_file("password", "wrong");
    let lines = testing::Lines::default();
    let mut o = opts(&[&f.srv.url]);
    o.username = "akit".to_owned();
    o.password_file = pw;
    let mut p = Etcd::new("etcd", &o, lines.log());
    let parts = start(&mut p);
    lines.wait("authentication failed");
    assert!(
        parts.rx.try_recv().is_err(),
        "a part without authenticating"
    );
    p.stop();
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn range_ends() {
    for (input, want) in [
        (&b"akit/"[..], &b"akit0"[..]),
        (b"a\xff", b"b"),
        (b"\xff\xff", b"\x00"),
        (b"", b"\x00"),
    ] {
        assert_eq!(range_end(input), want, "{input:?}");
    }
}

/// fit's `readWatch` on a whole stream: ready at the first message, changed at each with
/// events, until the stream ends or says the watch is over.
fn read_watch(stream: &str) -> (Result<(), String>, usize, usize) {
    let (mut ready, mut changed, mut started) = (0, 0, false);
    let mut lines = Lines::new(crate::api::MAX_BODY_BYTES);
    let mut f =
        |line: &[u8]| watch_message(line, &mut started, &mut || ready += 1, &mut || changed += 1);
    let r = lines
        .push(stream.as_bytes(), &mut f)
        .and_then(|()| lines.finish(&mut f))
        .and(Err("etcd ended the watch".to_owned()));
    (r, ready, changed)
}

#[test]
fn read_watches() {
    let (r, ready, changed) = read_watch(concat!(
        r#"{"result":{"created":true}}"#,
        "\n\n",
        r#"{"result":{"events":[{}]}}"#,
        "\n",
        r#"{"error":{"message":"boom"}}"#,
        "\n"
    ));
    assert_eq!(
        (r, ready, changed),
        (Err("the watch failed: boom".to_owned()), 1, 1)
    );
    let (r, _, _) = read_watch(r#"{"result":{"events":[{}]}}"#);
    assert!(r.unwrap_err().contains("does not say it is created"));
    let (r, _, _) = read_watch("nope");
    assert!(r.unwrap_err().contains("not JSON"));
    // fit's TestReadWatchNoResult: a message with neither a result nor an error is passed
    // over.
    let (r, ready, changed) = read_watch(concat!(
        r#"{"result":{"created":true}}"#,
        "\n{}\n",
        r#"{"result":{"events":[{}]}}"#,
        "\n"
    ));
    assert_eq!(
        (r, ready, changed),
        (Err("etcd ended the watch".to_owned()), 1, 1)
    );
    // A message split across reads, and one too large.
    let mut lines = Lines::new(8);
    let mut got = Vec::new();
    let mut f = |l: &[u8]| {
        got.push(l.to_vec());
        Ok(())
    };
    lines.push(b"abc", &mut f).unwrap();
    lines.push(b"de\nfg", &mut f).unwrap();
    assert!(
        lines
            .push(b"hijklmn", &mut f)
            .unwrap_err()
            .contains("larger than 8 bytes")
    );
    assert_eq!(got, [b"abcde".to_vec()]);
}

#[test]
fn kind() {
    let kinds = crate::Kinds::new(&[crate::API_KINDS]).unwrap();
    let make = kinds.lookup("etcd").unwrap();
    let pc = akit_config::Provider {
        name: "e".to_owned(),
        kind: akit_config::ProviderKind::Etcd(opts(&["http://127.0.0.1:1", "http://127.0.0.1:2"])),
    };
    let (p, watching) = make("e", &pc).unwrap();
    assert_eq!(p.name(), "e");
    assert_eq!(
        watching,
        r#"watching etcd's keys under "akit/" at http://127.0.0.1:1, http://127.0.0.1:2"#
    );
    assert!(kinds.lookup("consul").unwrap()("e", &pc).is_err());
}

fn source(o: akit_config::EtcdProvider, max_body: usize) -> EtcdSource {
    EtcdSource {
        opts: o,
        engine: Arc::new(ApiEngine::start("test").unwrap()),
        tries: 0,
        max_body,
    }
}

#[test]
fn open_fails() {
    let missing = std::env::temp_dir()
        .join(format!("akit-etcd-missing-{}", std::process::id()))
        .display()
        .to_string();
    let mut a = opts(&["http://127.0.0.1:1"]);
    a.tls.ca_file.clone_from(&missing);
    let mut b = opts(&["http://127.0.0.1:1"]);
    b.username = "akit".to_owned();
    b.password_file = missing;
    for o in [a, b] {
        assert!(
            source(o.clone(), 1 << 20).open(&Stop::new()).is_err(),
            "{o:?}: opened"
        );
    }
}

#[test]
fn call_fails() {
    let srv = Server::start(|r, w| match r.path.as_str() {
        "/v3/kv/range" => w.send(
            200,
            format!(
                r#"{{"kvs": [], "more": false, "padding": "{}"}}"#,
                "x".repeat(100)
            )
            .as_bytes(),
        ),
        "/v3/kv/other" => w.send(200, b"nope"),
        _ => w.send(404, b"404 page not found\n"),
    });
    let stop = Stop::new();
    let mut s = source(opts(&[&srv.url]), 64);
    let c = s.open(&stop).unwrap();
    // The answer is past the limit.
    let err = crate::watch::Conn::read(&*c, &stop).unwrap_err();
    assert!(err.contains("larger than 64 bytes"), "{err}");
    let err = c.call("/v3/kv/other", "{}".to_owned(), &stop).unwrap_err();
    assert!(
        err.starts_with("POST /v3/kv/other: "),
        "an answer not JSON: {err}"
    );
    let err = c
        .call("/v3/kv/nowhere", "{}".to_owned(), &stop)
        .unwrap_err();
    assert!(err.contains("404 Not Found"), "a 404: {err}");
    let mut s = source(opts(&["::bad"]), 64);
    let c = s.open(&stop).unwrap();
    assert!(c.call("/v3/kv/range", "{}".to_owned(), &stop).is_err());
    let p = Etcd::new("e", &opts(&[]), crate::stderr_log());
    assert_eq!(p.name(), "e");
}
