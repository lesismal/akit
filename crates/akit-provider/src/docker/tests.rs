//! fit's `provider/docker` tests, against a fake Docker daemon on a Unix socket (and on TCP), on
//! akio's HTTP client over the backend under test (`AKIO_TEST_BACKEND`).
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing,
    clippy::arithmetic_side_effects
)]

use std::collections::BTreeMap;
use std::io::{BufRead, BufReader, Write};
use std::net::TcpListener;
use std::os::unix::net::UnixListener;
use std::path::PathBuf;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, mpsc};
use std::thread;
use std::time::{Duration, Instant};

use akio::{Backend, Engine, Listener, UringConfig};
use akit_config::value::{self, Value};
use akit_config::{DockerEndpoint, DockerProvider};

use super::{
    Container, Docker, Log, MAX_EVENT_BYTES, MAX_LIST_BYTES, Port, Timings, event_changes,
    translate,
};
use crate::labels::write_compact;
use crate::run::{Kinds, Provider, Updates};

fn opts() -> DockerProvider {
    DockerProvider {
        endpoint: DockerEndpoint::Unix(PathBuf::from("/var/run/docker.sock")),
        expose_all: false,
        network: String::new(),
        use_published_ports: false,
        label_prefix: "akit".to_owned(),
        max_containers: 1000,
        refresh_interval: Duration::from_secs(30),
        request_timeout: Duration::from_secs(10),
    }
}

/// A container, written as the daemon lists it.
fn ctr(name: &str, labels: &[(&str, &str)], ip: &str, ports: &[u16]) -> Container {
    let mut c = Container {
        id: format!("id-{name}"),
        names: vec![format!("/{name}")],
        state: "running".into(),
        status: "Up 1 second".into(),
        labels: labels
            .iter()
            .map(|(k, v)| ((*k).to_owned(), (*v).to_owned()))
            .collect(),
        ..Container::default()
    };
    c.ports = ports
        .iter()
        .map(|p| Port {
            private: *p,
            ty: "tcp".into(),
            ..Port::default()
        })
        .collect();
    if !ip.is_empty() {
        c.networks
            .insert("net".into(), (ip.to_owned(), String::new()));
    }
    c
}

fn s(v: &str) -> Value {
    Value::String(v.to_owned())
}

/// `c` as the daemon writes it.
fn to_value(c: &Container) -> Value {
    let num = |n: u16| Value::Number(n.to_string());
    Value::Object(vec![
        ("Id".into(), s(&c.id)),
        (
            "Names".into(),
            Value::Array(c.names.iter().map(|n| s(n)).collect()),
        ),
        ("State".into(), s(&c.state)),
        ("Status".into(), s(&c.status)),
        (
            "Labels".into(),
            Value::Object(c.labels.iter().map(|(k, v)| (k.clone(), s(v))).collect()),
        ),
        (
            "Ports".into(),
            Value::Array(
                c.ports
                    .iter()
                    .map(|p| {
                        Value::Object(vec![
                            ("IP".into(), s(&p.ip)),
                            ("PrivatePort".into(), num(p.private)),
                            ("PublicPort".into(), num(p.public)),
                            ("Type".into(), s(&p.ty)),
                        ])
                    })
                    .collect(),
            ),
        ),
        (
            "HostConfig".into(),
            Value::Object(vec![("NetworkMode".into(), s(&c.network_mode))]),
        ),
        (
            "NetworkSettings".into(),
            Value::Object(vec![(
                "Networks".into(),
                Value::Object(
                    c.networks
                        .iter()
                        .map(|(n, (v4, v6))| {
                            (
                                n.clone(),
                                Value::Object(vec![
                                    ("IPAddress".into(), s(v4)),
                                    ("GlobalIPv6Address".into(), s(v6)),
                                ]),
                            )
                        })
                        .collect(),
                ),
            )]),
        ),
    ])
}

fn decoded(part: &[u8]) -> Value {
    value::parse(std::str::from_utf8(part).unwrap()).unwrap()
}

/// `v` with every object's members sorted, to compare JSON as fit's `reflect.DeepEqual` on maps.
fn sorted(v: Value) -> Value {
    match v {
        Value::Object(mut ms) => {
            ms.sort_by(|a, b| a.0.cmp(&b.0));
            Value::Object(ms.into_iter().map(|(k, v)| (k, sorted(v))).collect())
        }
        Value::Array(a) => Value::Array(a.into_iter().map(sorted).collect()),
        v => v,
    }
}

fn same_json(part: &[u8], want: &str) {
    let got = sorted(decoded(part));
    let want = sorted(value::parse(want).unwrap());
    assert_eq!(got, want, "part:\n{}", String::from_utf8_lossy(part));
}

/// fit's `TestTranslate`.
#[test]
fn translates() {
    let shop = |n: &str, ip: &str| {
        ctr(
            n,
            &[
                ("akit.enable", "true"),
                ("com.docker.compose.project", "shop"),
                ("com.docker.compose.service", "web"),
                ("akit.routes.main.host", "shop.example, www.shop.example"),
                ("akit.routes.main.path", "/*"),
                ("akit.routes.main.priority", "5"),
                ("akit.routes.main.listeners", "web"),
                ("akit.upstream", r#"{"load_balancing": "least_conn"}"#),
            ],
            ip,
            &[8080],
        )
    };
    let health = |n: &str, ip: &str, status: &str| {
        let mut c = ctr(n, &[("akit.enable", "true")], ip, &[80]);
        c.status = status.into();
        c
    };
    let cs = vec![
        // Two replicas of a Compose service, with a route.
        shop("shop-web-2", "10.0.0.3"),
        shop("shop-web-1", "10.0.0.2"),
        // A named container with two ports, one chosen, a handler of its own, middleware, and
        // a layer 4 service on the other port.
        ctr(
            "api",
            &[
                ("akit.enable", "true"),
                ("akit.name", "api"),
                ("akit.port", "9000"),
                ("akit.routes.r.expr", r#"PathPrefix("/api/")"#),
                (
                    "akit.routes.r.middleware",
                    r#"[{"module": "rate_limit", "params": {"rate": 10}}]"#,
                ),
                (
                    "akit.routes.r.handler",
                    r#"{"module": "respond", "params": {"body": "hi"}}"#,
                ),
                ("akit.services.db.listeners", "tcp"),
                ("akit.services.db.port", "5432"),
                ("akit.services.db.proxy_protocol", "v2"),
            ],
            "10.0.0.4",
            &[9000, 5432],
        ),
        // Left out: not enabled, enabled false, unhealthy, starting.
        ctr(
            "plain",
            &[("akit.routes.x.host", "x.example")],
            "10.0.0.5",
            &[80],
        ),
        ctr("off", &[("akit.enable", "false")], "10.0.0.6", &[80]),
        health("sick", "10.0.0.7", "Up 3 minutes (unhealthy)"),
        health("young", "10.0.0.8", "Up 1 second (health: starting)"),
        // Left out with problems.
        ctr(
            "twoports",
            &[("akit.enable", "true")],
            "10.0.0.9",
            &[80, 443],
        ),
        ctr("noports", &[("akit.enable", "true")], "10.0.0.10", &[]),
        ctr(
            "badlabel",
            &[("akit.enable", "true"), ("akit.routes.a.colour", "red")],
            "10.0.0.11",
            &[80],
        ),
        ctr(
            "badjson",
            &[("akit.enable", "true"), ("akit.routes.a.middleware", "{")],
            "10.0.0.12",
            &[80],
        ),
        ctr("nonet", &[("akit.enable", "true")], "", &[80]),
        ctr(
            "wrongnet",
            &[("akit.enable", "true"), ("akit.network", "other")],
            "10.0.0.13",
            &[80],
        ),
        ctr(
            "dotkey",
            &[("akit.enable", "true"), ("akit.routes.a.b.host", "x")],
            "10.0.0.14",
            &[80],
        ),
        ctr(
            "svc",
            &[("akit.enable", "true"), ("akit.services.s.port", "1")],
            "10.0.0.15",
            &[80],
        ),
        ctr(
            "upname",
            &[
                ("akit.enable", "true"),
                ("akit.upstream", r#"{"name": "x"}"#),
            ],
            "10.0.0.16",
            &[80],
        ),
    ];
    let (part, problems) = translate(cs.clone(), &opts());
    same_json(
        &part,
        r#"{
  "upstreams": [
    {"name": "api", "addresses": ["10.0.0.4:9000"]},
    {"name": "api.db", "addresses": ["10.0.0.4:5432"]},
    {"name": "shop-web", "addresses": ["10.0.0.2:8080", "10.0.0.3:8080"], "load_balancing": "least_conn"}
  ],
  "routes": [
    {"name": "api.r", "match": {"expr": "PathPrefix(\"/api/\")"},
     "middleware": [{"module": "rate_limit", "params": {"rate": 10}}],
     "handler": {"module": "respond", "params": {"body": "hi"}}},
    {"name": "shop-web.main", "listeners": ["web"], "priority": 5,
     "match": {"host": ["shop.example", "www.shop.example"], "path": ["/*"]},
     "handler": {"module": "proxy", "params": {"upstream": "shop-web"}}}
  ],
  "services": [
    {"name": "api.db", "listeners": ["tcp"], "upstream": "api.db", "proxy_protocol": "v2"}
  ]
}"#,
    );
    assert_eq!(
        problems,
        [
            "container badjson: label akit.routes.a.middleware: must be a JSON array of middleware, as in the config",
            r#"container badlabel: label akit.routes.a.colour: a route has no setting "colour""#,
            r#"container dotkey: label akit.routes.a.b.host: a route has no setting "b.host""#,
            "container nonet: it has no address on any network",
            "container noports: it exposes no TCP port: set akit.port",
            "container svc: service s: set akit.services.s.listeners",
            "container twoports: it exposes TCP ports [80 443]: set akit.port",
            "container upname: label akit.upstream: name is the container's: set akit.name",
            r#"container wrongnet: it is not on network "other""#,
        ]
    );
    // The same containers, in another order, give the same part.
    let rev: Vec<Container> = cs.into_iter().rev().collect();
    assert_eq!(translate(rev, &opts()).0, part);
    // Nothing taken is an empty part.
    assert_eq!(translate(Vec::new(), &opts()).0, b"{}");
}

fn addrs(part: &[u8]) -> BTreeMap<String, String> {
    let mut out = BTreeMap::new();
    if let Some(Value::Array(us)) = decoded(part).get("upstreams") {
        for u in us {
            let (Some(Value::String(n)), Some(Value::Array(a))) =
                (u.get("name"), u.get("addresses"))
            else {
                panic!("{u:?}");
            };
            let Some(Value::String(a)) = a.first() else {
                panic!("{u:?}");
            };
            out.insert(n.clone(), a.clone());
        }
    }
    out
}

fn map(kv: &[(&str, &str)]) -> BTreeMap<String, String> {
    kv.iter()
        .map(|(k, v)| ((*k).to_owned(), (*v).to_owned()))
        .collect()
}

/// fit's `TestTranslateOptions`.
#[test]
fn translate_options() {
    let mut plain = ctr(
        "plain",
        &[("app.routes.x.host", "x.example")],
        "10.0.0.5",
        &[80],
    );
    plain.ports[0].public = 32768;
    plain.ports[0].ip = "0.0.0.0".into();
    let mut host = ctr("host", &[("app.routes.h.path", "/h")], "", &[8000]);
    host.network_mode = "host".into();
    let mut two = ctr("two", &[("app.routes.t.path", "/t")], "10.0.0.20", &[80]);
    two.networks
        .insert("back".into(), (String::new(), "fd00::20".into()));
    let off = ctr("off", &[("app.enable", "false")], "10.0.0.6", &[80]);

    // expose_all takes every container but those enabled false, under another prefix; a
    // container on two networks is reached on the first by name, or the one network chooses.
    let mut o = DockerProvider {
        expose_all: true,
        label_prefix: "app".into(),
        ..opts()
    };
    let (part, problems) = translate(vec![plain.clone(), host, two.clone(), off], &o);
    assert_eq!(
        addrs(&part),
        map(&[
            ("plain", "10.0.0.5:80"),
            ("host", "127.0.0.1:8000"),
            ("two", "[fd00::20]:80")
        ])
    );
    assert!(problems.is_empty(), "{problems:?}");
    o.network = "net".into();
    assert_eq!(
        addrs(&translate(vec![two.clone()], &o).0)["two"],
        "10.0.0.20:80"
    );
    // use_published_ports sends to the host's port, and leaves out a container whose port is
    // not published.
    let o = DockerProvider {
        expose_all: true,
        label_prefix: "app".into(),
        use_published_ports: true,
        ..opts()
    };
    let (part, problems) = translate(vec![plain.clone(), two.clone()], &o);
    assert_eq!(addrs(&part), map(&[("plain", "127.0.0.1:32768")]));
    assert_eq!(
        problems,
        ["container two: port 80 is not published on the host"]
    );
    // max_containers takes the first by name.
    let o = DockerProvider {
        expose_all: true,
        label_prefix: "app".into(),
        max_containers: 1,
        ..opts()
    };
    let (part, problems) = translate(vec![plain, two], &o);
    assert_eq!(addrs(&part), map(&[("plain", "10.0.0.5:80")]));
    assert_eq!(
        problems,
        ["container two: left out: max_containers, 1, are taken"]
    );
}

/// fit's `TestTranslateEdges`.
#[test]
fn translate_edges() {
    // No name but its ID, and not running.
    let mut stopped = ctr("x", &[("akit.enable", "true")], "10.0.0.1", &[80]);
    stopped.names.clear();
    stopped.state = "exited".into();
    // A name the config does not take.
    let bad_name = ctr(
        "badname",
        &[("akit.enable", "true"), ("akit.name", "no way")],
        "10.0.0.2",
        &[80],
    );
    // On the network it names, but without an address there.
    let mut no_addr = ctr(
        "noaddr",
        &[("akit.enable", "true"), ("akit.network", "net")],
        "10.0.0.3",
        &[80],
    );
    no_addr
        .networks
        .insert("net".into(), (String::new(), String::new()));
    let (part, problems) = translate(vec![stopped, bad_name, no_addr], &opts());
    assert_eq!(part, b"{}");
    assert_eq!(
        problems,
        [
            r#"container badname: "no way" is not a valid name for an upstream: set akit.name"#,
            r#"container noaddr: it has no address on network "net""#,
        ]
    );

    // With published ports: one on "::", a service without a port of its own, and one whose
    // port is not published, which leaves the container out.
    let mut publ = ctr(
        "pub",
        &[
            ("akit.enable", "true"),
            ("akit.port", "80"),
            ("akit.services.a.listeners", "tcp"),
        ],
        "10.0.0.4",
        &[80],
    );
    publ.ports[0].public = 8080;
    publ.ports[0].ip = "::".into();
    let mut unpub = ctr(
        "unpub",
        &[
            ("akit.enable", "true"),
            ("akit.port", "80"),
            ("akit.services.b.listeners", "tcp"),
            ("akit.services.b.port", "81"),
        ],
        "10.0.0.5",
        &[80],
    );
    unpub.ports[0].public = 8081;
    let o = DockerProvider {
        use_published_ports: true,
        ..opts()
    };
    let (part, problems) = translate(vec![publ, unpub], &o);
    let text = String::from_utf8(part).unwrap();
    assert!(text.contains(r#""[::1]:8080""#), "{text}");
    assert!(!text.contains("8081") && !text.contains("unpub"), "{text}");
    assert_eq!(
        problems,
        ["container unpub: service b: port 81 is not published on the host"]
    );
}

/// fit's `TestReadEvents`, the part of it about one event.
#[test]
fn events() {
    assert_eq!(
        event_changes(br#"{"Type":"container","Action":"start"}"#),
        Ok(true)
    );
    assert_eq!(event_changes(br#"{"Action":"die"}"#), Ok(true));
    assert_eq!(
        event_changes(br#"{"Type":"network","Action":"connect"}"#),
        Ok(false)
    );
    assert_eq!(
        event_changes(br#"{"Type":"container","Action":"exec_die"}"#),
        Ok(false)
    );
    assert!(event_changes(b"nope").is_err());
}

// ---- the fake daemon ----------------------------------------------------------------------

fn backend() -> Backend {
    match std::env::var("AKIO_TEST_BACKEND").as_deref() {
        Ok("uring") => Backend::IoUring({
            let mut v = UringConfig::default();
            v.strict = true;
            v
        }),
        _ => Backend::Epoll,
    }
}

/// An engine whose dialed connections run on its listener's threads, so on the backend under
/// test.
fn engine() -> Arc<Engine> {
    let e = Engine::builder()
        .listen(
            Listener::tcp("127.0.0.1:0".parse().unwrap())
                .backend(backend())
                .threads(1)
                .serve(akio_http::serve(akio_http::Router::new())),
        )
        .build()
        .unwrap();
    e.start().unwrap();
    Arc::new(e)
}

fn stop_engine(e: &Engine) {
    e.shutdown();
    e.wait();
}

/// What a test makes the daemon answer a listing with, raw, in place of the containers.
type Answer = Box<dyn Fn(&str) -> Option<Vec<u8>> + Send>;

#[derive(Default)]
struct DaemonState {
    containers: Vec<Container>,
    lists: usize,
    /// Answer 500.
    fail: bool,
    /// The event streams open so far.
    streams: usize,
    /// Bumped to end the event streams open; the next ones last.
    epoch: usize,
    answer: Option<Answer>,
}

/// A fake Docker daemon: it lists the containers a test sets, and streams the events a test
/// sends.
struct Daemon {
    socket: PathBuf,
    state: Arc<Mutex<DaemonState>>,
    events: mpsc::Sender<String>,
    tcp: Option<String>,
}

static SOCKETS: AtomicUsize = AtomicUsize::new(0);

impl Daemon {
    fn new() -> Daemon {
        Daemon::start(false)
    }

    fn tcp() -> Daemon {
        Daemon::start(true)
    }

    fn start(tcp: bool) -> Daemon {
        let n = SOCKETS.fetch_add(1, Ordering::Relaxed);
        // Short, for sun_path.
        let socket = PathBuf::from(format!("/tmp/akdk-{}-{n}.sock", std::process::id()));
        let _ = std::fs::remove_file(&socket);
        let state = Arc::new(Mutex::new(DaemonState::default()));
        let (tx, rx) = mpsc::channel::<String>();
        let rx = Arc::new(Mutex::new(rx));
        let mut addr = None;
        if tcp {
            let l = TcpListener::bind("127.0.0.1:0").unwrap();
            addr = Some(l.local_addr().unwrap().to_string());
            let (st, rx) = (Arc::clone(&state), Arc::clone(&rx));
            thread::spawn(move || {
                for c in l.incoming() {
                    let Ok(c) = c else { return };
                    let (st, rx) = (Arc::clone(&st), Arc::clone(&rx));
                    let w = c.try_clone().unwrap();
                    thread::spawn(move || serve(BufReader::new(c), w, &st, &rx));
                }
            });
        } else {
            let l = UnixListener::bind(&socket).unwrap();
            let (st, rx) = (Arc::clone(&state), Arc::clone(&rx));
            thread::spawn(move || {
                for c in l.incoming() {
                    let Ok(c) = c else { return };
                    let (st, rx) = (Arc::clone(&st), Arc::clone(&rx));
                    let w = c.try_clone().unwrap();
                    thread::spawn(move || serve(BufReader::new(c), w, &st, &rx));
                }
            });
        }
        Daemon {
            socket,
            state,
            events: tx,
            tcp: addr,
        }
    }

    fn endpoint(&self) -> DockerEndpoint {
        match &self.tcp {
            Some(a) => DockerEndpoint::Tcp(a.clone()),
            None => DockerEndpoint::Unix(self.socket.clone()),
        }
    }

    fn set(&self, cs: Vec<Container>) {
        self.state.lock().unwrap().containers = cs;
    }

    fn fail(&self, fail: bool) {
        self.state.lock().unwrap().fail = fail;
    }

    /// Ends the event streams open, and lets the next ones last.
    fn end_streams(&self) {
        self.state.lock().unwrap().epoch += 1;
    }

    fn state(&self) -> (usize, usize) {
        let st = self.state.lock().unwrap();
        (st.lists, st.streams)
    }

    fn send(&self, e: &str) {
        self.events.send(e.to_owned()).unwrap();
    }
}

impl Drop for Daemon {
    fn drop(&mut self) {
        self.end_streams();
        let _ = std::fs::remove_file(&self.socket);
    }
}

/// Go's `url.QueryUnescape`.
fn unescape(s: &str) -> String {
    let b = s.as_bytes();
    let mut out = Vec::new();
    let mut i = 0;
    while i < b.len() {
        match b[i] {
            b'+' => out.push(b' '),
            b'%' if i + 2 < b.len() => {
                let h = std::str::from_utf8(&b[i + 1..i + 3]).unwrap();
                out.push(u8::from_str_radix(h, 16).unwrap());
                i += 2;
            }
            c => out.push(c),
        }
        i += 1;
    }
    String::from_utf8(out).unwrap()
}

/// The `filters` of a request's target, decoded.
fn filters(target: &str) -> String {
    target
        .split_once('?')
        .and_then(|(_, q)| q.split('&').find_map(|kv| kv.strip_prefix("filters=")))
        .map(unescape)
        .unwrap_or_default()
}

fn serve(
    mut r: impl BufRead,
    mut w: impl Write,
    st: &Arc<Mutex<DaemonState>>,
    events: &Arc<Mutex<mpsc::Receiver<String>>>,
) {
    loop {
        let mut line = String::new();
        if r.read_line(&mut line).unwrap_or(0) == 0 {
            return;
        }
        loop {
            let mut h = String::new();
            if r.read_line(&mut h).unwrap_or(0) == 0 || h == "\r\n" {
                break;
            }
        }
        let target = line.split(' ').nth(1).unwrap_or("").to_owned();
        let path = target.split('?').next().unwrap_or("").to_owned();
        let f = filters(&target);
        match path.as_str() {
            "/containers/json" => {
                let raw = {
                    let mut s = st.lock().unwrap();
                    s.lists += 1;
                    s.answer.as_ref().and_then(|a| a(&f))
                };
                if let Some(raw) = raw {
                    let _ = w.write_all(&raw);
                    return;
                }
                let s = st.lock().unwrap();
                if s.fail {
                    let body = "{\"message\":\"broken\"}\n";
                    let _ = write!(
                        w,
                        "HTTP/1.1 500 Internal Server Error\r\nContent-Type: text/plain\r\nContent-Length: {}\r\n\r\n{body}",
                        body.len()
                    );
                    continue;
                }
                let want: Vec<(String, String)> = match value::parse(&f) {
                    Ok(v) => match v.get("label") {
                        Some(Value::Array(ls)) => ls
                            .iter()
                            .filter_map(|l| match l {
                                Value::String(l) => {
                                    let (k, v) = l.split_once('=').unwrap_or((l, ""));
                                    Some((k.to_owned(), v.to_owned()))
                                }
                                _ => None,
                            })
                            .collect(),
                        _ => Vec::new(),
                    },
                    Err(_) => Vec::new(),
                };
                let out: Vec<Value> = s
                    .containers
                    .iter()
                    .filter(|c| want.iter().all(|(k, v)| c.labels.get(k) == Some(v)))
                    .map(to_value)
                    .collect();
                drop(s);
                let mut body = String::new();
                write_compact(&mut body, &Value::Array(out));
                let _ = write!(
                    w,
                    "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{body}",
                    body.len()
                );
            }
            "/events" => {
                assert_eq!(f, r#"{"type":["container"]}"#, "events filters");
                let epoch = {
                    let mut s = st.lock().unwrap();
                    s.streams += 1;
                    s.epoch
                };
                if w
                    .write_all(b"HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nTransfer-Encoding: chunked\r\n\r\n")
                    .is_err()
                {
                    return;
                }
                let _ = w.flush();
                loop {
                    if st.lock().unwrap().epoch != epoch {
                        let _ = w.write_all(b"0\r\n\r\n");
                        break;
                    }
                    let e = events
                        .lock()
                        .unwrap()
                        .recv_timeout(Duration::from_millis(10));
                    if let Ok(e) = e {
                        let line = format!("{e}\n");
                        if write!(w, "{:x}\r\n{line}\r\n", line.len()).is_err() {
                            return;
                        }
                        let _ = w.flush();
                    }
                }
            }
            _ => {
                let _ = w.write_all(b"HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\n\r\n");
            }
        }
    }
}

// ---- the provider ------------------------------------------------------------------------

fn short() -> Timings {
    Timings {
        debounce: Duration::from_millis(50),
        min_backoff: Duration::from_millis(20),
        max_backoff: Duration::from_millis(50),
    }
}

/// What a provider reports.
#[derive(Clone, Default)]
struct Lines(Arc<Mutex<Vec<String>>>);

impl Lines {
    fn log(&self) -> Log {
        let l = Arc::clone(&self.0);
        Arc::new(move |m: &str| l.lock().unwrap().push(m.to_owned()))
    }

    fn all(&self) -> String {
        self.0.lock().unwrap().join("\n")
    }

    fn wait_for(&self, what: &str) {
        let deadline = Instant::now() + Duration::from_secs(10);
        while !self.all().contains(what) {
            assert!(Instant::now() < deadline, "no {what:?} in:\n{}", self.all());
            thread::sleep(Duration::from_millis(10));
        }
    }
}

/// The parts a provider sends.
struct Parts {
    ups: Updates,
    woke: mpsc::Receiver<()>,
}

impl Parts {
    fn new() -> Parts {
        let (tx, rx) = mpsc::sync_channel(1);
        Parts {
            ups: Updates::new(move || {
                let _ = tx.try_send(());
            }),
            woke: rx,
        }
    }

    fn next(&self) -> Value {
        let deadline = Instant::now() + Duration::from_secs(10);
        loop {
            if let Some((_, data)) = self.ups.take() {
                return decoded(&data);
            }
            assert!(Instant::now() < deadline, "no part came");
            let _ = self.woke.recv_timeout(Duration::from_millis(20));
        }
    }

    /// No part comes for a while.
    fn none(&self) {
        thread::sleep(Duration::from_millis(150));
        if let Some((_, data)) = self.ups.take() {
            panic!("an unexpected part: {}", String::from_utf8_lossy(&data));
        }
    }
}

fn upstream_names(v: &Value) -> Vec<String> {
    match v.get("upstreams") {
        Some(Value::Array(us)) => us
            .iter()
            .filter_map(|u| match u.get("name") {
                Some(Value::String(n)) => Some(n.clone()),
                _ => None,
            })
            .collect(),
        _ => Vec::new(),
    }
}

fn provider(name: &str, o: DockerProvider, lines: &Lines, e: &Arc<Engine>) -> Docker {
    Docker::new(name, o, lines.log())
        .with_engine(Arc::clone(e))
        .with_timings(short())
}

/// fit's `TestProvider`: the containers at the start, a container started and one stopped, exec
/// events passed over, the event stream lost and found again, and the daemon failing.
#[test]
fn follows_the_daemon() {
    let d = Daemon::new();
    let web = ctr(
        "web",
        &[("akit.enable", "true"), ("akit.routes.r.path", "/")],
        "10.0.0.2",
        &[80],
    );
    let api = ctr(
        "api",
        &[("akit.enable", "true"), ("akit.routes.r.path", "/api/*")],
        "10.0.0.3",
        &[80],
    );
    let other = ctr("other", &[("akit.routes.r.path", "/o")], "10.0.0.4", &[80]);
    d.set(vec![web.clone(), other.clone()]);
    let e = engine();
    let lines = Lines::default();
    let mut p = provider(
        "docker",
        DockerProvider {
            endpoint: d.endpoint(),
            refresh_interval: Duration::from_secs(3600),
            ..opts()
        },
        &lines,
        &e,
    );
    assert_eq!(p.name(), "docker");
    let got = Parts::new();
    p.start(got.ups.update("docker")).unwrap();
    assert_eq!(upstream_names(&got.next()), ["web"], "at the start");

    // A container starts.
    d.set(vec![web.clone(), api.clone(), other.clone()]);
    d.send(r#"{"Type":"container","Action":"start","Actor":{"ID":"id-api"}}"#);
    assert_eq!(upstream_names(&got.next()), ["api", "web"], "after a start");

    // Exec events, as health checks make, change nothing and list nothing.
    let (lists, _) = d.state();
    for _ in 0..5 {
        d.send(r#"{"Type":"container","Action":"exec_start: /bin/true","Actor":{"ID":"id-api"}}"#);
    }
    thread::sleep(Duration::from_millis(150));
    assert_eq!(d.state().0, lists, "exec events listed the containers");

    // A container stops; several events make one listing.
    d.set(vec![api.clone(), other.clone()]);
    for a in ["kill", "die", "stop"] {
        d.send(&format!(
            r#"{{"Type":"container","Action":"{a}","Actor":{{"ID":"id-web"}}}}"#
        ));
    }
    assert_eq!(upstream_names(&got.next()), ["api"], "after a stop");
    assert_eq!(d.state().0, lists + 1, "three events made one listing");
    got.none();

    // The event stream ends: the provider reports it, connects again and lists at once,
    // sending even an unchanged part.
    d.end_streams();
    assert_eq!(
        upstream_names(&got.next()),
        ["api"],
        "after the stream came back"
    );
    assert!(d.state().1 >= 2, "{} event streams", d.state().1);

    // The daemon fails: the provider keeps trying, and comes back once it answers.
    d.fail(true);
    d.end_streams();
    lines.wait_for("500 Internal Server Error");
    d.set(vec![web]);
    d.fail(false);
    assert_eq!(
        upstream_names(&got.next()),
        ["web"],
        "after the daemon came back"
    );

    p.stop();
    stop_engine(&e);
    let all = lines.all();
    assert!(
        all.contains("provider docker: events: the daemon ended the event stream; trying again in"),
        "{all}"
    );
    assert!(
        all.contains(r#"GET /containers/json: 500 Internal Server Error: {"message":"broken"}"#),
        "{all}"
    );
}

/// fit's `TestProviderRefresh`: a part is sent every refresh_interval whether or not it
/// changed, and a problem is reported once.
#[test]
fn refreshes() {
    let d = Daemon::new();
    d.set(vec![
        ctr("web", &[("akit.enable", "true")], "10.0.0.2", &[80]),
        ctr("bad", &[("akit.enable", "true")], "10.0.0.3", &[80, 81]),
    ]);
    let e = engine();
    let lines = Lines::default();
    let mut p = provider(
        "dk",
        DockerProvider {
            endpoint: d.endpoint(),
            refresh_interval: Duration::from_millis(30),
            ..opts()
        },
        &lines,
        &e,
    );
    let got = Parts::new();
    p.start(got.ups.update("dk")).unwrap();
    for _ in 0..4 {
        assert_eq!(upstream_names(&got.next()), ["web"]);
    }
    p.stop();
    stop_engine(&e);
    assert_eq!(
        lines.all(),
        "provider dk: container bad: it exposes TCP ports [80 81]: set akit.port"
    );
}

/// fit's `TestProviderNoDaemon`: a daemon that is not there is reported and tried again, and
/// the provider stops meanwhile.
#[test]
fn no_daemon() {
    let e = engine();
    let lines = Lines::default();
    let mut p = provider(
        "docker",
        DockerProvider {
            endpoint: DockerEndpoint::Unix("/nonexistent/akit-test.sock".into()),
            ..opts()
        },
        &lines,
        &e,
    );
    let got = Parts::new();
    p.start(got.ups.update("docker")).unwrap();
    let deadline = Instant::now() + Duration::from_secs(10);
    while lines.0.lock().unwrap().len() < 3 {
        assert!(Instant::now() < deadline, "{}", lines.all());
        thread::sleep(Duration::from_millis(10));
    }
    let started = Instant::now();
    p.stop();
    assert!(started.elapsed() < Duration::from_secs(5), "stop waited");
    assert_eq!(got.ups.pending(), 0, "a part without a daemon");
    assert!(
        lines.all().contains("provider docker: GET /events: "),
        "{}",
        lines.all()
    );
    stop_engine(&e);
}

/// fit's `TestProviderListFails`: a listing that fails after an event, or at a refresh, ends
/// the watch, which is reported and tried again.
#[test]
fn list_fails() {
    for by_event in [true, false] {
        let d = Daemon::new();
        d.set(vec![ctr(
            "web",
            &[("akit.enable", "true")],
            "10.0.0.2",
            &[80],
        )]);
        let e = engine();
        let lines = Lines::default();
        let refresh = if by_event {
            Duration::from_secs(3600)
        } else {
            Duration::from_millis(30)
        };
        let mut p = provider(
            "dk",
            DockerProvider {
                endpoint: d.endpoint(),
                refresh_interval: refresh,
                ..opts()
            },
            &lines,
            &e,
        );
        let got = Parts::new();
        p.start(got.ups.update("dk")).unwrap();
        got.next();
        d.fail(true);
        if by_event {
            d.send(r#"{"Type":"container","Action":"start"}"#);
        }
        lines.wait_for("500 Internal Server Error");
        p.stop();
        stop_engine(&e);
    }
}

/// fit's `TestClientFails`, over TCP: a listing that is not JSON, one too large, and one cut
/// short each fail the watch, reported.
#[test]
fn client_fails() {
    let big = Arc::new(vec![b' '; MAX_LIST_BYTES + 1]);
    for (mode, want) in [
        ("json", "GET /containers/json: "),
        (
            "big",
            "GET /containers/json: the list is larger than 16777216 bytes",
        ),
        ("cut", "GET /containers/json: "),
    ] {
        let d = Daemon::tcp();
        let b = Arc::clone(&big);
        d.state.lock().unwrap().answer = Some(Box::new(move |_| {
            Some(match mode {
                "json" => b"HTTP/1.1 200 OK\r\nContent-Length: 4\r\n\r\nnope".to_vec(),
                "big" => {
                    let mut r = format!("HTTP/1.1 200 OK\r\nContent-Length: {}\r\n\r\n", b.len())
                        .into_bytes();
                    r.extend_from_slice(&b);
                    r
                }
                _ => b"HTTP/1.1 200 OK\r\nContent-Length: 100\r\n\r\n[{}".to_vec(),
            })
        }));
        let e = engine();
        let lines = Lines::default();
        let mut p = provider(
            "dk",
            DockerProvider {
                endpoint: d.endpoint(),
                ..opts()
            },
            &lines,
            &e,
        );
        let got = Parts::new();
        p.start(got.ups.update("dk")).unwrap();
        lines.wait_for("trying again in");
        p.stop();
        stop_engine(&e);
        let all = lines.all();
        assert!(all.contains(want), "{mode}: {all}");
        assert_eq!(got.ups.pending(), 0, "{mode}: a part");
    }
}

/// An event larger than the limit, and one that is not JSON, end the stream, reported; the
/// provider connects again.
#[test]
fn bad_events() {
    for (event, want) in [
        (
            format!(r#"{{"Action":"{}"}}"#, "x".repeat(MAX_EVENT_BYTES)),
            "events: an event larger than 1048576 bytes",
        ),
        ("nope".to_owned(), "events: an event that is not JSON"),
    ] {
        let d = Daemon::new();
        d.set(vec![ctr(
            "web",
            &[("akit.enable", "true")],
            "10.0.0.2",
            &[80],
        )]);
        let e = engine();
        let lines = Lines::default();
        let mut p = provider(
            "dk",
            DockerProvider {
                endpoint: d.endpoint(),
                refresh_interval: Duration::from_secs(3600),
                ..opts()
            },
            &lines,
            &e,
        );
        let got = Parts::new();
        p.start(got.ups.update("dk")).unwrap();
        got.next();
        d.send(&event);
        lines.wait_for(want);
        // Connected again: the listing at the start of the next watch sends the part again.
        assert_eq!(upstream_names(&got.next()), ["web"]);
        p.stop();
        stop_engine(&e);
    }
}

/// fit's `TestKind`: the kind is found and makes a provider saying what it watches.
#[test]
fn kind() {
    let kinds = Kinds::new(&[super::KINDS]).unwrap();
    let make = kinds.lookup("docker").unwrap();
    let pc = akit_config::Provider {
        name: "dk".into(),
        kind: akit_config::ProviderKind::Docker(DockerProvider {
            endpoint: DockerEndpoint::Tcp("127.0.0.1:2375".into()),
            ..opts()
        }),
    };
    let (p, watching) = make("dk", &pc).unwrap();
    assert_eq!(
        (p.name(), watching.as_str()),
        ("dk", "watching Docker at tcp:127.0.0.1:2375")
    );
}

/// The query of a listing is escaped as Go's `url.QueryEscape`, and read back by the daemon.
#[test]
fn query_escaping() {
    let mut b = String::new();
    super::query_escape(&mut b, r#"{"label":["akit.enable=true"]}"#);
    assert_eq!(b, "%7B%22label%22%3A%5B%22akit.enable%3Dtrue%22%5D%7D");
    assert_eq!(unescape(&b), r#"{"label":["akit.enable=true"]}"#);
}
