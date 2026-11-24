//! fit's `provider/consul` tests, against a fake Consul with blocking queries: `TestTranslate`,
//! `TestCatalog`, `TestCatalogMaxServices`, `TestKV`, `TestEscapePath`, `TestKinds`,
//! `TestOpenFails`, `TestCatalogReadFails`, `TestKVReadFails`, `TestGetFails`,
//! `TestPollIndexZero` and `TestTranslateNoAddress`.
#![allow(
    clippy::unwrap_used,
    clippy::panic,
    clippy::indexing_slicing,
    clippy::arithmetic_side_effects
)]

use std::collections::BTreeMap;
use std::sync::{Arc, Condvar, Mutex};
use std::time::{Duration, Instant};

use akio_crypto::base64::STANDARD;
use akit_config::value::{self, Value};

use super::{
    Blocking, CatalogSource, ConnSettings, ConsulCatalog, ConsulKv, Instance, KvSource,
    escape_path, translate,
};
use crate::api::ApiEngine;
use crate::run::Provider;
use crate::testing::{self, Parts, Req, Resp, Server, short};
use crate::watch::{Conn, Source, Stop};

/// An instance of a service, as the fake catalog keeps it.
#[derive(Clone, Default)]
struct Inst {
    svc: String,
    id: String,
    node: String,
    addr: String,
    port: u16,
    tags: Vec<String>,
    passing: bool,
}

fn inst(svc: &str, id: &str, node: &str, addr: &str, port: u16, tags: &[&str]) -> Inst {
    Inst {
        svc: svc.to_owned(),
        id: id.to_owned(),
        node: node.to_owned(),
        addr: addr.to_owned(),
        port,
        tags: tags.iter().map(|t| (*t).to_owned()).collect(),
        passing: true,
    }
}

impl Inst {
    fn entry(&self) -> Instance {
        Instance {
            node: self.node.clone(),
            node_address: "192.168.0.1".to_owned(),
            id: self.id.clone(),
            tags: self.tags.clone(),
            address: self.addr.clone(),
            port: self.port,
        }
    }

    fn json(&self) -> String {
        let tags: Vec<String> = self
            .tags
            .iter()
            .map(|t| {
                let mut b = String::new();
                value::write_string(&mut b, t);
                b
            })
            .collect();
        format!(
            r#"{{"Node":{{"Node":"{}","Address":"192.168.0.1"}},"Service":{{"ID":"{}","Service":"{}","Tags":[{}],"Address":"{}","Port":{}}}}}"#,
            self.node,
            self.id,
            self.svc,
            tags.join(","),
            self.addr,
            self.port
        )
    }
}

#[derive(Default)]
struct State {
    index: u64,
    insts: Vec<Inst>,
    kv: BTreeMap<String, String>,
    /// The token asked for; none when empty.
    token: String,
    seen: Vec<String>,
    fail: bool,
}

/// Answers the parts of Consul's API the providers ask for, with blocking queries on one index
/// for everything.
struct FakeConsul {
    st: Arc<(Mutex<State>, Condvar)>,
    srv: Server,
}

impl FakeConsul {
    fn new() -> FakeConsul {
        let st = Arc::new((
            Mutex::new(State {
                index: 10,
                ..State::default()
            }),
            Condvar::new(),
        ));
        let s = Arc::clone(&st);
        let srv = Server::start(move |r, w| serve(&s, r, w));
        FakeConsul { st, srv }
    }

    /// Changes the fake's state under its lock, and moves its index.
    fn change(&self, f: impl FnOnce(&mut State)) {
        let mut st = self.st.0.lock().unwrap();
        f(&mut st);
        st.index += 1;
        self.st.1.notify_all();
    }

    fn requests(&self) -> Vec<String> {
        self.st.0.lock().unwrap().seen.clone()
    }
}

fn serve(st: &Arc<(Mutex<State>, Condvar)>, r: &Req, mut w: Resp) {
    let mut s = st.0.lock().unwrap();
    let token = r.header("X-Consul-Token").unwrap_or_default().to_owned();
    s.seen.push(format!("{} token={token}", r.uri()));
    if s.fail {
        drop(s);
        w.send(500, b"no leader\n");
        return;
    }
    if !s.token.is_empty() && token != s.token {
        drop(s);
        w.send(403, b"ACL not found");
        return;
    }
    if let Some(idx) = r.param("index") {
        let want: u64 = idx.parse().unwrap();
        let Some(Ok(wait)) = r
            .param("wait")
            .as_deref()
            .and_then(akit_config::duration::parse)
        else {
            drop(s);
            w.send(400, b"bad wait");
            return;
        };
        let until = Instant::now() + wait;
        while s.index <= want {
            let left = until.saturating_duration_since(Instant::now());
            if left.is_zero() {
                break;
            }
            s = st.1.wait_timeout(s, left).unwrap().0;
        }
    }
    w.header("X-Consul-Index", &s.index.to_string());
    let path = r.path.as_str();
    let out = if path == "/v1/catalog/services" {
        let mut m: BTreeMap<&str, Vec<&str>> = BTreeMap::new();
        for i in &s.insts {
            let tags = m.entry(&i.svc).or_default();
            for t in &i.tags {
                if !tags.contains(&t.as_str()) {
                    tags.push(t);
                }
            }
        }
        let members: Vec<String> = m
            .iter()
            .map(|(svc, tags)| {
                let tags: Vec<String> = tags
                    .iter()
                    .map(|t| {
                        let mut b = String::new();
                        value::write_string(&mut b, t);
                        b
                    })
                    .collect();
                format!(r#""{svc}":[{}]"#, tags.join(","))
            })
            .collect();
        format!("{{{}}}", members.join(","))
    } else if path == "/v1/health/state/any" {
        "[]".to_owned()
    } else if let Some(name) = path.strip_prefix("/v1/health/service/") {
        let passing = r.param("passing").is_some();
        let es: Vec<String> = s
            .insts
            .iter()
            .filter(|i| i.svc == name && (i.passing || !passing))
            .map(Inst::json)
            .collect();
        format!("[{}]", es.join(","))
    } else if let Some(prefix) = path.strip_prefix("/v1/kv/") {
        let kvs: Vec<String> =
            s.kv.iter()
                .filter(|(k, _)| k.starts_with(prefix))
                .map(|(k, v)| format!(r#"{{"Key":"{k}","Value":"{}"}}"#, STANDARD.encode(v)))
                .collect();
        if kvs.is_empty() {
            drop(s);
            w.send(404, b"");
            return;
        }
        format!("[{}]", kvs.join(","))
    } else {
        drop(s);
        w.send(404, b"404 page not found\n");
        return;
    };
    drop(s);
    w.send(200, out.as_bytes());
}

fn blocking() -> Blocking {
    Blocking {
        wait: Duration::from_secs(2),
        min_gap: Duration::from_millis(10),
    }
}

fn conn(endpoint: &str) -> akit_config::ConsulConn {
    akit_config::ConsulConn {
        endpoint: endpoint.to_owned(),
        datacenter: String::new(),
        token_file: String::new(),
        tls: akit_config::ClientTlsFiles::default(),
        refresh_interval: Duration::from_secs(3600),
        request_timeout: Duration::from_secs(5),
    }
}

fn catalog_opts(endpoint: &str) -> akit_config::ConsulProvider {
    akit_config::ConsulProvider {
        conn: conn(endpoint),
        expose_all: false,
        tag_prefix: akit_config::DEFAULT_CONSUL_TAG_PREFIX.to_owned(),
        max_services: akit_config::DEFAULT_CONSUL_MAX_SERVICES,
    }
}

fn kv_opts(endpoint: &str) -> akit_config::ConsulKvProvider {
    akit_config::ConsulKvProvider {
        conn: conn(endpoint),
        prefix: akit_config::DEFAULT_KV_PREFIX.to_owned(),
        max_keys: akit_config::DEFAULT_KV_MAX_KEYS,
    }
}

fn decoded(part: &[u8]) -> Value {
    value::parse(std::str::from_utf8(part).unwrap()).unwrap_or_else(|e| {
        panic!(
            "the part is not JSON: {e}: {}",
            String::from_utf8_lossy(part)
        )
    })
}

/// The part's upstreams' names and addresses.
fn upstreams(part: &[u8]) -> BTreeMap<String, Vec<String>> {
    let mut m = BTreeMap::new();
    if let Some(Value::Array(ups)) = decoded(part).get("upstreams") {
        for u in ups {
            let Some(Value::String(name)) = u.get("name") else {
                panic!()
            };
            let addrs = match u.get("addresses") {
                Some(Value::Array(a)) => a
                    .iter()
                    .map(|v| match v {
                        Value::String(s) => s.clone(),
                        _ => panic!(),
                    })
                    .collect(),
                _ => Vec::new(),
            };
            m.insert(name.clone(), addrs);
        }
    }
    m
}

fn ups(pairs: &[(&str, &[&str])]) -> BTreeMap<String, Vec<String>> {
    pairs
        .iter()
        .map(|(n, a)| ((*n).to_owned(), a.iter().map(|s| (*s).to_owned()).collect()))
        .collect()
}

/// Members compared as sets, as Go compares decoded maps.
fn same(a: &Value, b: &Value) -> bool {
    match (a, b) {
        (Value::Object(x), Value::Object(y)) => {
            x.len() == y.len() && x.iter().all(|(k, v)| b.get(k).is_some_and(|w| same(v, w)))
        }
        (Value::Array(x), Value::Array(y)) => {
            x.len() == y.len() && x.iter().zip(y).all(|(v, w)| same(v, w))
        }
        (Value::Number(x), Value::Number(y)) => x.parse::<f64>().ok() == y.parse::<f64>().ok(),
        _ => a == b,
    }
}

#[test]
fn translates() {
    let opts = catalog_opts("http://127.0.0.1:1");
    let web_tags = [
        "fit.enable=true",
        "fit.routes.main.host=shop.example",
        "fit.routes.main.path=/*",
        r#"fit.upstream={"load_balancing": "least_conn"}"#,
    ];
    let mut web2_tags = vec!["primary"];
    web2_tags.extend(web_tags);
    let web = vec![
        inst("web", "web-2", "n2", "10.0.0.3", 8080, &web2_tags).entry(),
        inst("web", "web-1", "n1", "10.0.0.2", 8080, &web_tags).entry(),
        // Its node's address, as an instance registered without one.
        inst("web", "web-3", "n3", "", 8080, &["fit.enable=true"]).entry(),
        // Not taken.
        inst(
            "web",
            "web-4",
            "n4",
            "10.0.0.9",
            8080,
            &["fit.enable=false"],
        )
        .entry(),
        inst("web", "web-5", "n5", "10.0.0.9", 8080, &[]).entry(),
    ];
    let mut services = BTreeMap::new();
    services.insert("web".to_owned(), web.clone());
    // A name of its own, a port of its own, and a layer 4 service.
    services.insert(
        "db-primary".to_owned(),
        vec![
            inst(
                "db-primary",
                "db",
                "n1",
                "10.0.1.1",
                1,
                &[
                    "fit.enable=true",
                    "fit.name=db",
                    "fit.port=5432",
                    "fit.services.pg.listeners=tcp",
                    "fit.services.pg.port=5433",
                ],
            )
            .entry(),
        ],
    );
    services.insert(
        "bad".to_owned(),
        vec![
            inst("bad", "b1", "n1", "10.0.2.1", 80, &["fit.enable"]).entry(),
            inst(
                "bad",
                "b2",
                "n1",
                "10.0.2.2",
                80,
                &["fit.enable=true", "fit.routes.r.colour=red"],
            )
            .entry(),
            inst("bad", "b3", "n1", "10.0.2.3", 0, &["fit.enable=true"]).entry(),
            inst(
                "bad",
                "b4",
                "n1",
                "10.0.2.4",
                80,
                &["fit.enable=true", "fit.name=no way"],
            )
            .entry(),
            inst(
                "bad",
                "b5",
                "n1",
                "10.0.2.5",
                80,
                &["fit.enable=true", r#"fit.upstream={"addresses": []}"#],
            )
            .entry(),
        ],
    );
    let (part, problems) = translate(services, &opts);
    let want = value::parse(
        r#"{
  "upstreams": [
    {"name": "db", "addresses": ["10.0.1.1:5432"]},
    {"name": "db.pg", "addresses": ["10.0.1.1:5433"]},
    {"name": "web", "addresses": ["10.0.0.2:8080", "10.0.0.3:8080", "192.168.0.1:8080"], "load_balancing": "least_conn"}
  ],
  "routes": [
    {"name": "web.main", "match": {"host": ["shop.example"], "path": ["/*"]},
     "handler": {"module": "proxy", "params": {"upstream": "web"}}}
  ],
  "services": [
    {"name": "db.pg", "listeners": ["tcp"], "upstream": "db.pg"}
  ]
}"#,
    )
    .unwrap();
    assert!(
        same(&decoded(&part), &want),
        "part:\n{}",
        String::from_utf8_lossy(&part)
    );
    assert_eq!(
        problems,
        [
            "service bad, instance b1 on node n1: tag fit.enable: is not <key>=<value>",
            r#"service bad, instance b2 on node n1: tag fit.routes.r.colour: a route has no setting "colour""#,
            "service bad, instance b3 on node n1: it has no port: tag it fit.port=<port>",
            r#"service bad, instance b4 on node n1: "no way" is not a valid name for an upstream: tag it fit.name=<name>"#,
            "service bad, instance b5 on node n1: tag fit.upstream: addresses are the services'",
        ]
    );

    // With expose_all, an instance with no tags is taken, but one tagged enable=false still is
    // not.
    let mut o = catalog_opts("http://127.0.0.1:1");
    o.expose_all = true;
    o.tag_prefix = "app".to_owned();
    let mut services = BTreeMap::new();
    services.insert("web".to_owned(), web[3..].to_vec());
    let (part, _) = translate(services, &o);
    assert_eq!(
        upstreams(&part),
        ups(&[("web", &["10.0.0.9:8080", "10.0.0.9:8080"])])
    );
}

#[test]
fn catalog() {
    let f = FakeConsul::new();
    f.st.0.lock().unwrap().token = "s3cret".to_owned();
    let web = |id: &str, addr: &str| {
        inst(
            "web",
            id,
            &format!("n-{id}"),
            addr,
            80,
            &["fit.enable=true", "fit.routes.r.path=/"],
        )
    };
    f.change(|s| {
        s.insts = vec![
            web("w1", "10.0.0.1"),
            inst("other", "o1", "n1", "10.0.0.9", 80, &[]), // not enabled
        ];
    });
    let dir = std::env::temp_dir().join(format!("akit-consul-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let token_file = dir.join("token");
    std::fs::write(&token_file, "s3cret\n").unwrap();
    let lines = testing::Lines::default();
    let mut o = catalog_opts(&f.srv.url);
    o.conn.datacenter = "dc2".to_owned();
    o.conn.token_file = token_file.display().to_string();
    let mut p = ConsulCatalog::new("consul", &o, lines.log());
    assert_eq!(p.name(), "consul");
    p.set_timings(short(), blocking());
    let (parts, update) = Parts::new("consul");
    p.start(update).unwrap();
    let is = |want: BTreeMap<String, Vec<String>>| move |b: &[u8]| upstreams(b) == want;

    parts.next(is(ups(&[("web", &["10.0.0.1:80"])])));
    // An instance is registered.
    f.change(|s| s.insts.push(web("w2", "10.0.0.2")));
    parts.next(is(ups(&[("web", &["10.0.0.1:80", "10.0.0.2:80"])])));
    // One fails its checks.
    f.change(|s| s.insts[0].passing = false);
    parts.next(is(ups(&[("web", &["10.0.0.2:80"])])));
    // The last is deregistered: nothing.
    f.change(|s| s.insts = s.insts[1..2].to_vec());
    parts.next(|b| b == b"{}");

    // Consul fails, and comes back.
    f.st.0.lock().unwrap().fail = true;
    f.change(|_| {});
    std::thread::sleep(Duration::from_millis(100));
    f.change(|s| {
        s.fail = false;
        s.insts.push(web("w3", "10.0.0.3"));
    });
    parts.next(is(ups(&[("web", &["10.0.0.3:80"])])));

    p.stop();
    let reqs = f.requests();
    let (mut blocking, mut health) = (0, 0);
    for r in &reqs {
        assert!(
            r.contains("dc=dc2") && r.ends_with("token=s3cret"),
            "a request without the datacenter or the token: {r}"
        );
        if r.contains("index=") {
            blocking += 1;
        }
        assert!(
            !r.starts_with("/v1/health/service/other"),
            "a service not enabled was asked for: {r}"
        );
        if r.starts_with("/v1/health/service/web?") && r.contains("passing=1") {
            health += 1;
        }
    }
    assert!(
        blocking > 0 && health > 0,
        "{blocking} blocking queries, {health} of web's health:\n{}",
        reqs.join("\n")
    );
    let all = lines.all();
    assert!(
        all.contains("500 Internal Server Error: no leader"),
        "logs:\n{all}"
    );
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn catalog_max_services() {
    let f = FakeConsul::new();
    f.change(|s| {
        for name in ["c", "a", "b"] {
            s.insts
                .push(inst(name, name, "n", "10.0.0.1", 80, &["fit.enable=true"]));
        }
    });
    let lines = testing::Lines::default();
    let mut o = catalog_opts(&f.srv.url);
    o.max_services = 2;
    let mut p = ConsulCatalog::new("consul", &o, lines.log());
    p.set_timings(short(), blocking());
    let (parts, update) = Parts::new("consul");
    p.start(update).unwrap();
    let got = upstreams(&parts.next(|_| true));
    assert_eq!(
        got,
        ups(&[("a", &["10.0.0.1:80"]), ("b", &["10.0.0.1:80"])])
    );
    lines.wait("provider consul: service c: left out: max_services, 2, are taken");
}

#[test]
fn kv() {
    let f = FakeConsul::new();
    let route = |name: &str| {
        format!(r#"{{"routes": [{{"name": "{name}", "handler": {{"module": "respond"}}}}]}}"#)
    };
    f.change(|s| {
        s.kv.insert("fit/a".to_owned(), route("a"));
        s.kv.insert("fit/bad".to_owned(), r#"{"limits": {}}"#.to_owned());
        s.kv.insert("other/x".to_owned(), route("x"));
    });
    let lines = testing::Lines::default();
    let mut p = ConsulKv::new("kv", &kv_opts(&f.srv.url), lines.log());
    p.set_timings(short(), blocking());
    let (parts, update) = Parts::new("kv");
    p.start(update).unwrap();
    parts.want(r#"{"routes":[{"name":"a","handler":{"module":"respond"}}]}"#);
    f.change(|s| {
        s.kv.insert("fit/b".to_owned(), route("b"));
    });
    parts.want(r#"{"routes":[{"name":"a","handler":{"module":"respond"}},{"name":"b","handler":{"module":"respond"}}]}"#);
    // Every key gone: Consul answers 404, an empty part.
    f.change(|s| s.kv.clear());
    parts.want("{}");
    // The index goes back, as after a restore: the provider starts over.
    f.change(|s| {
        s.index = 3;
        s.kv.insert("fit/c".to_owned(), route("c"));
    });
    parts.want(r#"{"routes":[{"name":"c","handler":{"module":"respond"}}]}"#);
    p.stop();
    lines.wait("provider kv: key fit/bad: ");
    for r in f.requests() {
        assert!(
            r.starts_with("/v1/kv/fit/?") && r.contains("recurse=true"),
            "request: {r}"
        );
    }
}

#[test]
fn escapes_path() {
    assert_eq!(escape_path("fit/a b/c?"), "fit/a%20b/c%3F");
}

#[test]
fn kinds() {
    let kinds = crate::Kinds::new(&[crate::API_KINDS]).unwrap();
    let cases = [
        (
            "consul",
            akit_config::ProviderKind::Consul(catalog_opts("http://127.0.0.1:1")),
            "watching Consul's catalog at http://127.0.0.1:1",
        ),
        (
            "consul_kv",
            akit_config::ProviderKind::ConsulKv(kv_opts("http://127.0.0.1:1")),
            r#"watching Consul's keys under "fit/" at http://127.0.0.1:1"#,
        ),
    ];
    for (kind, k, want) in cases {
        let pc = akit_config::Provider {
            name: "p".to_owned(),
            kind: k,
        };
        let (p, watching) = kinds.lookup(kind).unwrap()("p", &pc).unwrap();
        assert_eq!((p.name(), watching.as_str()), ("p", want), "{kind}");
    }
}

fn settings(endpoint: &str) -> ConnSettings {
    ConnSettings {
        conn: conn(endpoint),
        blocking: blocking(),
        max_body: crate::api::MAX_BODY_BYTES,
    }
}

fn engine() -> Arc<ApiEngine> {
    Arc::new(ApiEngine::start("test").unwrap())
}

#[test]
fn open_fails() {
    let missing = std::env::temp_dir()
        .join(format!("akit-consul-missing-{}", std::process::id()))
        .display()
        .to_string();
    let mut a = settings("http://127.0.0.1:1");
    a.conn.token_file.clone_from(&missing);
    let mut b = settings("http://127.0.0.1:1");
    b.conn.tls.ca_file = missing;
    let e = engine();
    for s in [a, b] {
        let mut cat = CatalogSource {
            settings: s.clone(),
            engine: Arc::clone(&e),
            opts: catalog_opts("http://127.0.0.1:1"),
        };
        assert!(cat.open(&Stop::new()).is_err(), "catalog opened");
        let mut kv = KvSource {
            settings: s,
            engine: Arc::clone(&e),
            prefix: "fit/".to_owned(),
            max_keys: 10,
        };
        assert!(kv.open(&Stop::new()).is_err(), "kv opened");
    }
}

/// Fixed answers by path: a status and a body.
type Answers = &'static [(&'static str, u16, &'static str)];

/// Serves fixed answers by path.
fn answers(a: Answers) -> Server {
    Server::start(move |r, w| match a.iter().find(|(p, _, _)| *p == r.path) {
        Some((_, status, body)) => w.send(*status, body.as_bytes()),
        None => w.send(404, b"404 page not found\n"),
    })
}

const OK: (&str, u16, &str) = (
    "/v1/catalog/services",
    200,
    r#"{"web": ["fit.enable=true"]}"#,
);

#[test]
fn catalog_read_fails() {
    let e = engine();
    let cases: [(&str, Answers, &str); 4] = [
        (
            "services fail",
            &[("/v1/catalog/services", 500, "down")],
            "500 Internal Server Error: down",
        ),
        (
            "services not JSON",
            &[("/v1/catalog/services", 200, "nope")],
            "GET /v1/catalog/services: ",
        ),
        (
            "health fails",
            &[OK, ("/v1/health/service/web", 503, "busy")],
            "503 Service Unavailable: busy",
        ),
        (
            "health not JSON",
            &[OK, ("/v1/health/service/web", 200, "{")],
            "GET /v1/health/service/web: ",
        ),
    ];
    for (name, a, want) in cases {
        let srv = answers(a);
        let mut s = CatalogSource {
            settings: settings(&srv.url),
            engine: Arc::clone(&e),
            opts: catalog_opts(&srv.url),
        };
        let stop = Stop::new();
        let err = s.open(&stop).unwrap().read(&stop).unwrap_err();
        assert!(err.contains(want), "{name}: {err}, want {want:?}");
    }
}

#[test]
fn kv_read_fails() {
    let e = engine();
    let cases: [(&str, Answers, &str); 2] = [
        (
            "fails",
            &[("/v1/kv/fit/", 500, "down")],
            "500 Internal Server Error: down",
        ),
        (
            "not JSON",
            &[("/v1/kv/fit/", 200, "nope")],
            "GET /v1/kv/fit/: ",
        ),
    ];
    for (name, a, want) in cases {
        let srv = answers(a);
        let mut s = KvSource {
            settings: settings(&srv.url),
            engine: Arc::clone(&e),
            prefix: "fit/".to_owned(),
            max_keys: 10,
        };
        let stop = Stop::new();
        let err = s.open(&stop).unwrap().read(&stop).unwrap_err();
        assert!(err.contains(want), "{name}: {err}, want {want:?}");
    }
    let p = ConsulKv::new("kv", &kv_opts("http://127.0.0.1:1"), crate::stderr_log());
    assert_eq!(p.name(), "kv");
}

#[test]
fn get_fails() {
    let srv = Server::start(|_, mut w| {
        w.header("X-Consul-Index", "abc");
        w.send(200, b"{}");
    });
    let e = engine();
    let stop = Stop::new();
    let c = settings(&srv.url).open(&e).unwrap();
    let err = c.get("/x", &[], 0, &stop).unwrap_err();
    assert!(
        err.contains(r#"X-Consul-Index "abc" is not a number"#),
        "a bad index: {err}"
    );
    let c = settings("::bad").open(&e).unwrap();
    assert!(
        c.get("/x", &[], 0, &stop).is_err(),
        "a bad endpoint: no error"
    );
}

/// An index of 0 is taken as 1, and one gone back starts over.
#[test]
fn poll_index_zero() {
    let indexes: Arc<Mutex<Vec<String>>> = Arc::default();
    let seen = Arc::clone(&indexes);
    let srv = Server::start(move |r, mut w| {
        seen.lock()
            .unwrap()
            .push(r.param("index").unwrap_or_default());
        w.header("X-Consul-Index", "0");
        w.send(200, b"{}");
    });
    let e = engine();
    let stop = Stop::new();
    let c = settings(&srv.url).open(&e).unwrap();
    let ready = Mutex::new(false);
    let s2 = stop.clone();
    let err = c.poll(
        "/x",
        &[],
        &stop,
        &|| *ready.lock().unwrap() = true,
        &move || s2.cancel(),
    );
    assert!(err.is_err() && *ready.lock().unwrap(), "poll: {err:?}");
    // The first query, then one from index 1, which went back to 0.
    let idx = indexes.lock().unwrap();
    assert!(
        idx.len() >= 2 && idx[0].is_empty() && idx[1] == "1",
        "indexes: {idx:?}"
    );
}

#[test]
fn translate_no_address() {
    let mut e = inst("web", "w", "n", "", 80, &["fit.enable=true"]).entry();
    e.node_address = String::new();
    let mut services = BTreeMap::new();
    services.insert("web".to_owned(), vec![e]);
    let (part, problems) = translate(services, &catalog_opts("http://127.0.0.1:1"));
    assert_eq!(part, b"{}");
    assert_eq!(problems.len(), 1);
    assert!(
        problems[0].contains("it has no address, nor has its node"),
        "{problems:?}"
    );
    let p = ConsulCatalog::new(
        "c",
        &catalog_opts("http://127.0.0.1:1"),
        crate::stderr_log(),
    );
    assert_eq!(p.name(), "c");
}
