//! The Admin API's endpoints behind the console's media pages (I149), fit's `admin/media_test.go`
//! ported: a preview signed for the console's user, by URL, by token or public, which the `vod`
//! and `hls` routes play; the policies listed with their keys but never their secrets; the
//! sessions paged and one ended; a secret rotated through its file; a test URL; the counts of
//! `/media/stats`. Every wait is on the library's status, with a deadline.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing,
    clippy::arithmetic_side_effects
)]

use std::io::{Read, Write};
use std::net::{SocketAddr, TcpStream};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use akio::{Engine, Listener};
use akio_http::Context;
use akit_config::value::{Value, parse};
use akit_media::workbench::{View, ViewFn};
use akit_roles::host::Manager;
use akit_runtime::upstream::EngineSlot;
use akit_runtime::{Env, GraphCell, Registry, compile};

const T: Duration = Duration::from_secs(30);
const SECRET: &str = "the first secret of the site's policy, long enough";

struct Resp {
    status: u16,
    headers: Vec<(String, String)>,
    body: Vec<u8>,
}

impl Resp {
    fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(n, _)| n.eq_ignore_ascii_case(name))
            .map(|(_, v)| v.as_str())
    }

    fn text(&self) -> String {
        String::from_utf8_lossy(&self.body).into_owned()
    }

    fn json(&self) -> Value {
        parse(&self.text()).unwrap_or(Value::Null)
    }
}

fn http(addr: SocketAddr, method: &str, target: &str, fields: &[(&str, &str)], body: &str) -> Resp {
    let mut s = TcpStream::connect(addr).unwrap();
    s.set_read_timeout(Some(T)).unwrap();
    let mut req = format!("{method} {target} HTTP/1.1\r\nHost: x\r\nConnection: close\r\n");
    for (n, v) in fields {
        req.push_str(&format!("{n}: {v}\r\n"));
    }
    req.push_str(&format!("Content-Length: {}\r\n\r\n{body}", body.len()));
    s.write_all(req.as_bytes()).unwrap();
    let mut out = Vec::new();
    let _ = s.read_to_end(&mut out);
    let split = out
        .windows(4)
        .position(|w| w == b"\r\n\r\n")
        .unwrap_or(out.len());
    let head = String::from_utf8_lossy(&out[..split]).into_owned();
    let mut lines = head.lines();
    let status = lines
        .next()
        .and_then(|l| l.split(' ').nth(1))
        .and_then(|c| c.parse().ok())
        .unwrap();
    let headers = lines
        .filter_map(|l| l.split_once(':'))
        .map(|(n, v)| (n.trim().to_owned(), v.trim().to_owned()))
        .collect();
    Resp {
        status,
        headers,
        body: out.get(split + 4..).unwrap_or_default().to_vec(),
    }
}

fn s(v: &Value, k: &str) -> String {
    match v.get(k) {
        Some(Value::String(s)) => s.clone(),
        _ => String::new(),
    }
}

fn n(v: &Value, k: &str) -> i64 {
    match v.get(k) {
        Some(Value::Number(n)) => n.parse().unwrap_or(-1),
        _ => -1,
    }
}

fn arr(v: &Value, k: &str) -> Vec<Value> {
    match v.get(k) {
        Some(Value::Array(a)) => a.clone(),
        _ => Vec::new(),
    }
}

/// The path and query of an absolute URL.
fn target_of(url: &str) -> String {
    let rest = url.split_once("://").map_or(url, |(_, r)| r);
    rest.find('/').map_or("/".into(), |i| rest[i..].to_owned())
}

struct TempDir(PathBuf);

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn temp() -> TempDir {
    static N: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
    let p = std::env::temp_dir().join(format!(
        "akit-workbench-{}-{}",
        std::process::id(),
        N.fetch_add(1, std::sync::atomic::Ordering::SeqCst)
    ));
    let _ = std::fs::remove_dir_all(&p);
    std::fs::create_dir_all(&p).unwrap();
    TempDir(std::fs::canonicalize(p).unwrap())
}

struct Akit {
    engine: Arc<Engine>,
    admin: Engine,
    roles: Manager,
    web: SocketAddr,
    api: SocketAddr,
    view: ViewFn,
    _cell: GraphCell,
}

impl Drop for Akit {
    fn drop(&mut self) {
        self.roles.stop(Duration::ZERO);
        self.admin.shutdown();
        self.engine.shutdown();
    }
}

fn doc(lib: &Path, secret_file: &Path) -> String {
    let hash = akio_crypto::bcrypt::hash(b"pw", akio_crypto::bcrypt::MIN_COST).unwrap();
    format!(
        r#"{{"listeners": [{{"name": "web", "address": "127.0.0.1:0"}}],
          "media_library": [{{"name": "lib", "roots": ["{l}"]}}, {{"name": "lib2", "roots": ["{l}"]}}, {{"name": "lib3", "roots": ["{l}"]}}],
          "media_auth": [
            {{"name": "site", "secret_file": "{f}", "url_signing": {{"ttl": "1m"}}}},
            {{"name": "tok", "secret": "a-secret-of-some-length-0123456789"}},
            {{"name": "pub", "public": ["play"]}}],
          "streaming": {{"channels": [{{"name": "tv/a", "library": "lib", "items": ["films/bbb"]}}]}},
          "routes": [
            {{"name": "films", "match": {{"path": ["/vod/lib/*"]}}, "handler": {{"module": "vod", "params": {{"library": "lib", "auth": "site", "cors": "*"}}}}}},
            {{"match": {{"path": ["/tok/*"]}}, "handler": {{"module": "vod", "params": {{"library": "lib2", "prefix": "/tok/", "auth": "tok"}}}}}},
            {{"match": {{"path": ["/pub/*"]}}, "handler": {{"module": "vod", "params": {{"library": "lib3", "prefix": "/pub/", "auth": "pub"}}}}}},
            {{"match": {{"path": ["/hls/*"]}}, "handler": {{"module": "hls", "params": {{"prefix": "/hls/", "auth": "site", "play_wait": "10s"}}}}}},
            {{"name": "tokens", "match": {{"path": ["/media-tokens", "/media-tokens/*"]}},
              "middleware": [{{"module": "basic_auth", "params": {{"users": [{{"username": "backend", "password_hash": "{hash}"}}]}}}}],
              "handler": {{"module": "media_tokens", "params": {{"auth": "site", "url_prefixes": ["/vod/lib/"], "max_ttl": "1h"}}}}}}]}}"#,
        l = lib.display(),
        f = secret_file.display(),
    )
}

fn start(json: &str) -> Akit {
    let config = akit_config::from_json(json, None).unwrap_or_else(|e| panic!("{json}: {e}"));
    let slot = EngineSlot::new();
    let registry = Registry::new(&[akit_media::MODULES]).unwrap();
    // The media_tokens route's basic_auth is not akit-media's: the graph serves the rest, the
    // view reads it all.
    let mut served = config.clone();
    served
        .routes
        .retain(|r| !matches!(r.handler, akit_config::Handler::MediaTokens(_)));
    let cell = GraphCell::new(compile(&served, &registry, &Env::new(slot.clone())).unwrap());
    let engine =
        Arc::new(akit_listener::engine(&cell, &akit_listener::Options::default()).unwrap());
    engine.start().unwrap();
    assert!(slot.set(Arc::clone(&engine)));
    let web = engine.local_addrs()[0];
    let mut roles = Manager::new(
        &[
            akit_media::role::MEDIA_AUTH,
            akit_media::library::MEDIA_LIBRARY,
            akit_media::streaming::STREAMING,
        ],
        Arc::clone(&engine),
        None,
    );
    let errs = roles.apply(&config);
    assert!(errs.is_empty(), "{errs:?}");
    let key = akit_media::library::key(&engine);
    for name in ["lib", "lib2", "lib3"] {
        akit_media::library::lookup(key, name)
            .unwrap()
            .wait_status(T, |st| st.scans >= 1 && !st.scanning)
            .expect("the first scan");
    }
    let config = Arc::new(config);
    let port = web.port();
    let view: ViewFn =
        Arc::new(move || View::new(Some(Arc::clone(&config)), &[("web".into(), port)]));
    let v2 = Arc::clone(&view);
    let service = akio_http::serve(move |ctx: &mut Context| {
        if !akit_media::workbench::serve(key, &v2, ctx) {
            ctx.respond(akio_http::Response::text(404, "no such endpoint"));
        }
    });
    let admin = Engine::builder()
        .listen(Listener::tcp("127.0.0.1:0".parse().unwrap()).serve(service))
        .build()
        .unwrap();
    admin.start().unwrap();
    let api = admin.local_addrs()[0];
    Akit {
        engine,
        admin,
        roles,
        web,
        api,
        view,
        _cell: cell,
    }
}

impl Akit {
    fn api(&self, method: &str, path: &str, body: &str) -> Resp {
        http(
            self.api,
            method,
            path,
            &[
                ("Akit-Console-User", "alice"),
                ("Akit-Console-Host", "console.example:9400"),
            ],
            body,
        )
    }

    fn web(&self, target: &str) -> Resp {
        http(self.web, "GET", target, &[], "")
    }
}

fn lib_root() -> PathBuf {
    std::fs::canonicalize(Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/data/medialib/lib"))
        .unwrap()
}

/// fit's `TestMediaPreview`.
#[test]
fn preview() {
    let dir = temp();
    let file = dir.0.join("site-secret");
    std::fs::write(&file, SECRET).unwrap();
    let a = start(&doc(&lib_root(), &file));
    let origin = format!("http://127.0.0.1:{}", a.web.port());
    // Signed by URL, for the console's user, with its poster.
    let r = a.api(
        "POST",
        "/media/preview",
        r#"{"library": "lib", "item": "films/bbb"}"#,
    );
    assert_eq!(r.status, 200, "{}", r.text());
    assert_eq!(r.header("cache-control"), Some("no-store"));
    let v = r.json();
    assert_eq!(s(&v, "signed"), "url");
    assert_eq!(s(&v, "subject"), "workbench:alice");
    assert_eq!(s(&v, "route"), "films");
    assert_eq!(s(&v, "cors"), "*");
    assert_eq!(s(&v, "session").len(), 16);
    let url = s(&v, "url");
    assert!(
        url.starts_with(&format!("{origin}/vod/lib/films/bbb/index.m3u8?exp=")),
        "{url}"
    );
    assert!(url.contains("&sub=workbench%3Aalice&"), "{url}");
    assert!(s(&v, "poster").starts_with(&format!("{origin}/vod/lib/films/bbb/poster.jpg?exp=")));
    let exp = n(&v, "expires");
    let now = i64::try_from(
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs(),
    )
    .unwrap();
    assert!((now + 100..=now + 121).contains(&exp), "{exp} {now}");
    let r = a.web(&target_of(&url));
    assert_eq!(r.status, 200, "{}", r.text());
    assert!(r.text().contains("#EXTM3U"));
    // By a token, where the policy signs no URL; public, where playing is.
    let v = a
        .api(
            "POST",
            "/media/preview",
            r#"{"library": "lib2", "item": "films/bbb"}"#,
        )
        .json();
    assert_eq!(s(&v, "signed"), "token");
    assert!(s(&v, "url").contains("/tok/films/bbb/index.m3u8?token="));
    assert_eq!(a.web(&target_of(&s(&v, "url"))).status, 200);
    let v = a
        .api(
            "POST",
            "/media/preview",
            r#"{"library": "lib3", "item": "sintel"}"#,
        )
        .json();
    assert_eq!(s(&v, "signed"), "public");
    assert_eq!(s(&v, "url"), format!("{origin}/pub/sintel/index.m3u8"));
    // A channel, from the hls route.
    let v = a
        .api("POST", "/media/preview", r#"{"channel": "tv/a"}"#)
        .json();
    assert!(
        s(&v, "url").starts_with(&format!("{origin}/hls/tv/a/index.m3u8?exp=")),
        "{v:?}"
    );
    assert_eq!(s(&v, "signed"), "url");
    // Refusals.
    for (body, want) in [
        ("nonsense", 400),
        (r#"{"library": "lib"}"#, 400),
        (r#"{"library": "lib", "item": "x", "channel": "tv/a"}"#, 400),
        (r#"{"library": "none", "item": "films/bbb"}"#, 404),
        (r#"{"library": "lib", "item": "nothing"}"#, 404),
        (r#"{"library": "lib", "item": "broken"}"#, 404),
        (r#"{"channel": "tv/none"}"#, 404),
    ] {
        let r = a.api("POST", "/media/preview", body);
        assert_eq!(r.status, want, "{body}: {}", r.text());
    }
    assert_eq!(a.api("GET", "/media/preview", "").status, 405);
    // The console may load media from the listener.
    assert_eq!((a.view)().origins("console.example:9400"), vec![origin]);
}

/// fit's `TestMediaAuth`: the policies, the sessions, a session ended, a secret rotated through
/// its file, a test URL.
#[test]
fn policies_sessions_rotate_and_urls() {
    let dir = temp();
    let file = dir.0.join("site-secret");
    std::fs::write(&file, SECRET).unwrap();
    let a = start(&doc(&lib_root(), &file));
    let r = a.api("GET", "/media/auth", "");
    assert_eq!(r.status, 200);
    let text = r.text();
    assert!(
        !text.contains(SECRET) && !text.contains("site-secret"),
        "{text}"
    );
    let pols = arr(&r.json(), "policies");
    assert_eq!(pols.len(), 3);
    let site = &pols[0];
    assert_eq!(s(site, "name"), "site");
    assert_eq!(site.get("url_signing"), Some(&Value::Bool(true)));
    let set = site.get("settings").unwrap();
    assert_eq!(
        (
            s(set, "ttl"),
            s(set, "play_grace"),
            s(set, "on_limit"),
            s(set, "bind_address")
        ),
        ("1m0s".into(), "30m0s".into(), "refuse".into(), "ip".into())
    );
    let keys = arr(site, "keys");
    assert_eq!(keys.len(), 1);
    assert_eq!(s(&keys[0], "name"), "");
    assert_eq!(keys[0].get("file"), Some(&Value::Bool(true)));
    assert!(keys[0].get("modified").is_some());
    let routes = arr(site, "token_routes");
    assert_eq!(s(&routes[0], "route"), "tokens");
    assert_eq!(s(&routes[0], "max_ttl"), "1h0m0s");
    assert_eq!(
        arr(&pols[1], "keys")[0].get("file"),
        Some(&Value::Bool(false))
    );

    // A session, played from a preview, listed and ended.
    let v = a
        .api(
            "POST",
            "/media/preview",
            r#"{"library": "lib", "item": "films/bbb"}"#,
        )
        .json();
    let url = target_of(&s(&v, "url"));
    assert_eq!(a.web(&url).status, 200);
    let page = a.api("GET", "/media/auth/site/sessions", "").json();
    assert_eq!(n(&page, "total"), 1, "{page:?}");
    let ss = &arr(&page, "page")[0];
    assert_eq!(s(ss, "subject"), "workbench:alice");
    assert_eq!(s(ss, "resource"), "/vod/lib/films/bbb/");
    assert_eq!(n(&arr(&page, "subjects")[0], "sessions"), 1);
    assert_eq!(
        n(
            &a.api("GET", "/media/auth/site/sessions?subject=nobody", "")
                .json(),
            "matched"
        ),
        0
    );
    assert_eq!(
        a.api("GET", "/media/auth/site/sessions?limit=0", "").status,
        400
    );
    let body = format!(
        r#"{{"subject": "workbench:alice", "id": "{}"}}"#,
        s(ss, "id")
    );
    let r = a.api("POST", "/media/auth/site/sessions/end", &body);
    assert_eq!(r.status, 200, "{}", r.text());
    assert_eq!(a.web(&url).status, 403);
    assert_eq!(
        a.api("POST", "/media/auth/site/sessions/end", &body).status,
        404
    );

    // A test URL: on the route's listener, or as media_tokens' route signs it.
    let r = a.api(
        "POST",
        "/media/auth/site/urls",
        r#"{"subject": "u1", "path": "/vod/lib/films/bbb/index.m3u8", "ttl": "30s"}"#,
    );
    assert_eq!(r.status, 200, "{}", r.text());
    let test = s(&r.json(), "url");
    assert!(
        test.starts_with(&format!("http://127.0.0.1:{}/vod/lib/", a.web.port())),
        "{test}"
    );
    assert_eq!(a.web(&target_of(&test)).status, 200);
    let r = a.api(
        "POST",
        "/media/auth/site/urls",
        r#"{"subject": "u1", "path": "/vod/lib/films/bbb/index.m3u8", "route": "tokens"}"#,
    );
    assert!(s(&r.json(), "url").starts_with("/vod/lib/films/bbb/index.m3u8?"));
    for (body, want) in [
        (r#"{"subject": "", "path": "/vod/lib/a/index.m3u8"}"#, 400),
        (
            r#"{"subject": "u", "path": "/vod/lib/a/index.m3u8", "ttl": "2h"}"#,
            400,
        ),
        (r#"{"subject": "u", "path": "nope"}"#, 400),
        (
            r#"{"subject": "u", "path": "/tok/a/index.m3u8", "route": "tokens"}"#,
            400,
        ),
        (
            r#"{"subject": "u", "path": "/vod/lib/a/index.m3u8", "route": "none"}"#,
            404,
        ),
        (
            r#"{"subject": "u", "path": "/vod/lib/a/index.m3u8", "address": "x"}"#,
            400,
        ),
    ] {
        let r = a.api("POST", "/media/auth/site/urls", body);
        assert_eq!(r.status, want, "{body}: {}", r.text());
    }
    assert_eq!(
        a.api("POST", "/media/auth/tok/urls", r#"{"subject": "u"}"#)
            .status,
        404
    );
    assert_eq!(a.api("GET", "/media/auth/none", "").status, 404);

    // The secret rotated through its file: the URLs it signed are refused.
    let r = a.api("POST", "/media/auth/site/rotate", "{}");
    assert_eq!(r.status, 200, "{}", r.text());
    assert_eq!(
        arr(&r.json(), "rotated"),
        vec![Value::String(String::new())]
    );
    assert_ne!(std::fs::read_to_string(&file).unwrap(), SECRET);
    assert_eq!(a.web(&target_of(&test)).status, 403);
    assert_eq!(a.api("POST", "/media/auth/tok/rotate", "{}").status, 409);
    assert_eq!(
        a.api("POST", "/media/auth/tok/rotate", r#"{"key": "k"}"#)
            .status,
        400
    );
}

/// fit's `TestMediaStats`.
#[test]
fn stats() {
    let dir = temp();
    let file = dir.0.join("site-secret");
    std::fs::write(&file, SECRET).unwrap();
    let a = start(&doc(&lib_root(), &file));
    let v = a
        .api(
            "POST",
            "/media/preview",
            r#"{"library": "lib", "item": "films/bbb"}"#,
        )
        .json();
    assert_eq!(a.web(&target_of(&s(&v, "url"))).status, 200);
    let r = a.api("GET", "/media/stats", "");
    assert_eq!(r.status, 200);
    let v = r.json();
    let libs = arr(&v, "libraries");
    assert_eq!(libs.len(), 3);
    assert_eq!(s(&libs[0], "name"), "lib");
    assert!(n(&libs[0], "items") >= 4);
    assert!(libs[0].get("pool").is_some() && libs[0].get("tables").is_some());
    let vod = arr(&v, "vod");
    let lib = vod.iter().find(|x| s(x, "library") == "lib").unwrap();
    assert!(n(lib, "requests") >= 1 && n(lib, "bytes") > 0, "{lib:?}");
    let pols = arr(&v, "policies");
    assert_eq!(s(&pols[0], "name"), "site");
    assert_eq!(n(&pols[0], "sessions"), 1);
    let chs = arr(&v, "channels");
    assert_eq!(s(&chs[0], "name"), "tv/a");
    assert!(n(&v, "time") > 0);
    assert_eq!(a.api("POST", "/media/stats", "").status, 405);
}
