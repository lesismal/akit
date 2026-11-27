//! The dashboard (I63), with fit's `admin/dashboard_test.go` ported: its pages served with fit's
//! headers, every file the page names served, its requests from the API's own origin let read
//! but not write, its files served without the token while the API asks for it, none when
//! `disable_dashboard`, and its files read-only (no method but GET, no form that posts, nothing
//! from another origin, no HTML written from data). And the shapes of `GET /status` and
//! `GET /config/effective`, which it reads. fit's `TestStatus`, which needs a running server, is
//! ported in akit's `tests/metrics.rs` and `tests/admin.rs`. On the backend `AKIO_TEST_BACKEND`
//! names (epoll by default, `uring`).
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing,
    clippy::arithmetic_side_effects
)]

use std::io::{Read, Write};
use std::net::{SocketAddr, TcpStream};
use std::sync::Arc;
use std::time::Duration;

use akio::{Backend, UringConfig};
use akit_admin::{Configs, DASHBOARD_CSP, Options, SOURCE_FILE, Server, VERSION_HEADER};
use akit_config::value::{Value, parse};

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

const DOC: &str = r#"{
  "listeners": [{"name": "web", "address": "127.0.0.1:0"}, {"name": "other", "address": "127.0.0.1:0"}],
  "routes": [{"handler": {"module": "respond", "params": {"body": "v1"}}}],
  "upstreams": [{"name": "back", "addresses": ["127.0.0.1:1"]}],
  "metrics": {}
}"#;

/// The Admin API over `DOC`, with no control thread: these tests change nothing, and a change
/// that got past the guard would be answered 503.
struct Fixture {
    api: Server,
    configs: Configs,
}

impl Drop for Fixture {
    fn drop(&mut self) {
        self.api.shutdown();
    }
}

fn start(f: impl FnOnce(&mut akit_config::Admin), opts: Options) -> Fixture {
    let mut settings = akit_config::Admin {
        address: "127.0.0.1:0".into(),
        ..akit_config::Admin::default()
    };
    f(&mut settings);
    let configs = Configs::new(
        akit_config::value::standardize(DOC).unwrap(),
        SOURCE_FILE,
        settings.history,
    );
    let mut opts = opts;
    opts.backend = Some(backend());
    let api = Server::start(&settings, configs.clone(), Box::new(Err), &opts).unwrap();
    Fixture { api, configs }
}

#[derive(Debug)]
struct Resp {
    status: u16,
    headers: Vec<(String, String)>,
    body: String,
}

impl Resp {
    fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(n, _)| n.eq_ignore_ascii_case(name))
            .map(|(_, v)| v.as_str())
    }
}

/// One request on a connection of its own, `Connection: close`, `Host` the API's address.
fn request(addr: SocketAddr, method: &str, path: &str, headers: &[(&str, &str)]) -> Resp {
    let mut s = TcpStream::connect(addr).unwrap();
    s.set_read_timeout(Some(Duration::from_secs(10))).unwrap();
    let mut req = format!("{method} {path} HTTP/1.1\r\nConnection: close\r\nHost: {addr}\r\n");
    for (n, v) in headers {
        req.push_str(&format!("{n}: {v}\r\n"));
    }
    req.push_str("Content-Length: 0\r\n\r\n");
    s.write_all(req.as_bytes()).unwrap();
    let mut raw = Vec::new();
    let _ = s.read_to_end(&mut raw);
    let raw = String::from_utf8_lossy(&raw);
    let (head, body) = raw.split_once("\r\n\r\n").expect(&raw);
    let mut lines = head.lines();
    let status = lines.next().unwrap().split(' ').nth(1).unwrap();
    Resp {
        status: status.parse().unwrap(),
        headers: lines
            .filter_map(|l| l.split_once(':'))
            .map(|(n, v)| (n.to_owned(), v.trim().to_owned()))
            .collect(),
        body: body.to_owned(),
    }
}

impl Fixture {
    fn get(&self, path: &str) -> Resp {
        request(self.api.addr(), "GET", path, &[])
    }

    fn own(&self) -> String {
        format!("http://{}", self.api.addr())
    }
}

/// The values of `href="..."` and `src="..."` in `html` that hold no `#` or `:`: the page's own
/// files (fit's `(?:href|src)="([^"#:]+)"`).
fn named_files(html: &str) -> Vec<String> {
    let mut out = Vec::new();
    for attr in ["href=\"", "src=\""] {
        for (i, _) in html.match_indices(attr) {
            let rest = &html[i + attr.len()..];
            let Some(end) = rest.find('"') else { continue };
            let v = &rest[..end];
            if !v.is_empty() && !v.contains(['#', ':']) {
                out.push(v.to_owned());
            }
        }
    }
    out
}

#[test]
fn dashboard() {
    let f = start(|_| {}, Options::default());
    let r = f.get("/dashboard");
    assert_eq!(
        (r.status, r.header("Location")),
        (301, Some("/dashboard/")),
        "{r:?}"
    );
    let r = f.get("/dashboard/index.html");
    assert_eq!((r.status, r.header("Location")), (301, Some("/dashboard/")));

    let r = f.get("/dashboard/");
    assert_eq!(r.status, 200, "{r:?}");
    assert!(r.body.contains("<title>akit dashboard</title>"), "{r:?}");
    for (k, v) in [
        ("Content-Security-Policy", DASHBOARD_CSP),
        ("X-Content-Type-Options", "nosniff"),
        ("X-Frame-Options", "DENY"),
        ("Referrer-Policy", "no-referrer"),
        ("Cache-Control", "no-cache"),
        ("Content-Type", "text/html; charset=utf-8"),
    ] {
        assert_eq!(r.header(k), Some(v), "{k}");
    }
    assert_eq!(
        DASHBOARD_CSP,
        "default-src 'none'; script-src 'self'; style-src 'self'; img-src 'self' data:; \
         connect-src 'self'; base-uri 'none'; form-action 'none'; frame-ancestors 'none'"
    );
    // Every file the page names is served.
    let files = named_files(&r.body);
    assert_eq!(files, ["app.css", "app.js"]);
    for file in &files {
        let r = f.get(&format!("/dashboard/{file}"));
        assert_eq!(r.status, 200, "{file}");
        assert_eq!(r.header("X-Frame-Options"), Some("DENY"), "{file}");
    }
    let r = f.get("/dashboard/app.js");
    assert!(
        r.header("Content-Type")
            .unwrap()
            .starts_with("text/javascript"),
        "{r:?}"
    );
    assert!(
        f.get("/dashboard/app.css")
            .header("Content-Type")
            .unwrap()
            .starts_with("text/css")
    );
    assert_eq!(f.get("/dashboard/nothing.js").status, 404);
    let r = request(f.api.addr(), "HEAD", "/dashboard/app.js", &[]);
    assert_eq!((r.status, r.body.as_str()), (200, ""));
    let r = request(f.api.addr(), "POST", "/dashboard/", &[]);
    assert_eq!((r.status, r.header("Allow")), (405, Some("GET, HEAD")));

    // The dashboard's own requests carry its origin, which may read but not write; another
    // origin may do neither.
    let own = f.own();
    let localhost = own.replace("127.0.0.1", "localhost");
    for (method, path, origin, status) in [
        ("GET", "/status", own.as_str(), 200),
        ("GET", "/config/effective", own.as_str(), 200),
        ("GET", "/versions", own.as_str(), 200),
        ("GET", "/dashboard/", own.as_str(), 200),
        ("GET", "/versions", localhost.as_str(), 403),
        ("GET", "/status", "http://evil.example.com", 403),
        ("POST", "/load", own.as_str(), 403),
        ("DELETE", "/config/routes/0", own.as_str(), 403),
    ] {
        let r = request(
            f.api.addr(),
            method,
            path,
            &[("Origin", origin), ("Content-Type", "application/json")],
        );
        assert_eq!(r.status, status, "{method} {path} from {origin}: {r:?}");
    }
}

#[test]
fn dashboard_token() {
    let file = std::path::PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("dashboard-token");
    std::fs::write(&file, "s3cret\n").unwrap();
    let f = start(
        |a| a.token_file = Some(file.display().to_string()),
        Options::default(),
    );
    // The files hold no data, and are served without the token; the page asks for it.
    for p in ["/dashboard/", "/dashboard/app.js", "/dashboard/app.css"] {
        assert_eq!(f.get(p).status, 200, "{p} without the token");
    }
    assert_eq!(f.get("/dashboard").status, 301);
    assert_eq!(f.get("/status").status, 401);
    let bearer = [("Authorization", "Bearer s3cret")];
    assert_eq!(request(f.api.addr(), "GET", "/status", &bearer).status, 200);
    // Writes under /dashboard/ are no way around the token.
    assert_eq!(
        request(f.api.addr(), "PUT", "/dashboard/x", &[]).status,
        401
    );
    // Nor is a read past the dashboard.
    assert_ne!(f.get("/dashboard/../status").status, 200);
}

#[test]
fn dashboard_disabled() {
    let f = start(|a| a.disable_dashboard = true, Options::default());
    assert_eq!(f.get("/dashboard/").status, 404);
    assert_eq!(f.get("/dashboard").status, 404);
    assert_eq!(f.get("/status").status, 200);
}

/// Whether `text` names another origin: fit's `(?i)\b(https?:)?//[a-z0-9]`, the SVG namespace
/// aside.
fn names_another_origin(text: &str) -> bool {
    let text = text.replace("http://www.w3.org/2000/svg", "");
    let lower = text.to_ascii_lowercase();
    let b = lower.as_bytes();
    let word = |i: usize| i > 0 && (b[i - 1].is_ascii_alphanumeric() || b[i - 1] == b'_');
    lower.match_indices("//").any(|(i, _)| {
        let next = b.get(i + 2).is_some_and(u8::is_ascii_alphanumeric);
        let scheme = ["http:", "https:"]
            .iter()
            .any(|s| i >= s.len() && lower[..i].ends_with(s) && !word(i - s.len()));
        next && (scheme || word(i))
    })
}

/// fit's `\b(POST|PUT|PATCH|DELETE)\b`.
fn names_a_write(text: &str) -> Option<&'static str> {
    let b = text.as_bytes();
    let word = |c: u8| c.is_ascii_alphanumeric() || c == b'_';
    ["POST", "PUT", "PATCH", "DELETE"].into_iter().find(|m| {
        text.match_indices(m).any(|(i, _)| {
            (i == 0 || !word(b[i - 1])) && b.get(i + m.len()).is_none_or(|c| !word(*c))
        })
    })
}

/// What `method: "<m>"` sends, everywhere in `text`.
fn methods_sent(text: &str) -> Vec<String> {
    text.match_indices("method:")
        .filter_map(|(i, m)| {
            let rest = text[i + m.len()..].trim_start().strip_prefix('"')?;
            Some(rest[..rest.find('"')?].to_owned())
        })
        .collect()
}

/// The dashboard's files send nothing but GETs, have no form that posts, fetch nothing from
/// elsewhere and write no HTML from data: fit's `TestDashboardReadOnly`.
#[test]
fn dashboard_read_only() {
    let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src/dashboard");
    let mut seen = 0;
    for e in std::fs::read_dir(&dir).unwrap() {
        let path = e.unwrap().path();
        let text = std::fs::read_to_string(&path).unwrap();
        let name = path.display();
        assert_eq!(names_a_write(&text), None, "{name} names a method");
        for m in methods_sent(&text) {
            assert_eq!(m, "GET", "{name} sends {m}");
        }
        assert!(
            !text.to_ascii_lowercase().contains("method=\"post\""),
            "{name} has a form that posts"
        );
        assert!(!names_another_origin(&text), "{name} names another origin");
        if path.extension().is_some_and(|x| x == "js") {
            assert!(!text.contains("innerHTML"), "{name} writes HTML from data");
        }
        seen += 1;
    }
    assert_eq!(seen, 3);
    // The checks catch what they are for.
    assert_eq!(names_a_write("fetch(p, {method: \"POST\"})"), Some("POST"));
    assert_eq!(names_a_write("POSTED; dispatch"), None);
    assert_eq!(methods_sent("{ method:  \"PUT\" }"), ["PUT"]);
    assert!(names_another_origin("src=\"https://cdn.example/x.js\""));
    assert!(names_another_origin("a//b.example"));
    assert!(!names_another_origin(
        "// a comment\n<svg xmlns='http://www.w3.org/2000/svg'>"
    ));
}

/// `text`, a JSON value, as `Value::to_json` writes it.
fn canon(text: &str) -> String {
    parse(text).unwrap().to_json()
}

fn object(v: &Value) -> &[(String, Value)] {
    match v {
        Value::Object(m) => m,
        other => panic!("not an object: {other:?}"),
    }
}

fn keys(v: &Value) -> Vec<&str> {
    object(v).iter().map(|(k, _)| k.as_str()).collect()
}

/// `GET /status` without anything to say what akit is doing: the running config's version, when
/// and where from, and none of anything else, with fit's names.
#[test]
fn status_alone() {
    let f = start(|_| {}, Options::default());
    let r = f.get("/status");
    assert_eq!(
        (r.status, r.header(VERSION_HEADER)),
        (200, Some("1")),
        "{r:?}"
    );
    assert_eq!(r.header("Content-Type"), Some("application/json"));
    let v = parse(&r.body).unwrap();
    assert_eq!(
        keys(&v),
        [
            "version",
            "applied",
            "source",
            "reloads",
            "listeners",
            "connections",
            "upstreams",
            "certificates"
        ]
    );
    assert_eq!(v.get("version"), Some(&Value::Number("1".into())));
    assert_eq!(v.get("source"), Some(&Value::String("file".into())));
    let Some(Value::String(applied)) = v.get("applied") else {
        panic!("{v:?}")
    };
    assert!(
        applied.len() == 24 && applied.ends_with('Z') && applied.as_bytes()[10] == b'T',
        "{applied}"
    );
    assert_eq!(
        v.get("reloads").unwrap().to_json(),
        canon(r#"{"succeeded":0,"failed":0}"#)
    );
    assert_eq!(
        v.get("connections").unwrap().to_json(),
        canon(r#"{"open":0,"refused":0,"pending_bytes":0}"#)
    );
    for k in ["listeners", "upstreams", "certificates"] {
        assert_eq!(v.get(k), Some(&Value::Array(Vec::new())), "{k}");
    }
    let r = request(f.api.addr(), "POST", "/status", &[]);
    assert_eq!((r.status, r.header("Allow")), (405, Some("GET, HEAD")));
}

/// `GET /status` with what `Options::status` writes after the history's members.
#[test]
fn status_live() {
    let mut opts = Options::default();
    opts.status = Some(Arc::new(|b: &mut String| {
        b.push_str(r#""reloads":{"succeeded":2,"failed":1},"listeners":[{"name":"web"}]"#);
    }));
    let f = start(|_| {}, opts);
    let v = parse(&f.get("/status").body).unwrap();
    assert_eq!(
        keys(&v),
        ["version", "applied", "source", "reloads", "listeners"]
    );
    assert_eq!(
        v.get("listeners").unwrap().to_json(),
        canon(r#"[{"name":"web"}]"#)
    );
}

/// `GET /config/effective`: the running config, as it runs, every part from its own source (fit's
/// `Effective` of a config without providers).
#[test]
fn effective() {
    let f = start(|_| {}, Options::default());
    let r = f.get("/config/effective");
    assert_eq!(
        (r.status, r.header(VERSION_HEADER)),
        (200, Some("1")),
        "{r:?}"
    );
    let v = parse(&r.body).unwrap();
    assert_eq!(
        keys(&v),
        ["version", "etag", "precedence", "config", "sources"]
    );
    let cur = f.configs.current();
    assert_eq!(v.get("etag"), Some(&Value::String(cur.etag.clone())));
    assert_eq!(v.get("precedence").unwrap().to_json(), canon(r#"["file"]"#));
    assert_eq!(
        v.get("config").unwrap().to_json(),
        parse(DOC).unwrap().to_json()
    );
    let Some(Value::Array(sources)) = v.get("sources") else {
        panic!("{v:?}")
    };
    let paths: Vec<String> = sources
        .iter()
        .map(|s| {
            assert_eq!(s.get("source"), Some(&Value::String("file".into())));
            match s.get("path") {
                Some(Value::String(p)) => p.clone(),
                other => panic!("{other:?}"),
            }
        })
        .collect();
    assert_eq!(
        paths,
        [
            "/listeners/0",
            "/listeners/1",
            "/routes/0",
            "/upstreams/0",
            "/metrics"
        ]
    );
    // HEAD reads it too; a PUT edits the member `effective`, as fit's PUT /config/{path...}: it
    // reaches the control thread, which these tests have none of.
    assert_eq!(
        request(f.api.addr(), "HEAD", "/config/effective", &[]).status,
        200
    );
    let r = request(
        f.api.addr(),
        "PUT",
        "/config/effective",
        &[("Content-Type", "application/json")],
    );
    assert_ne!(r.status, 405, "{r:?}");
}
