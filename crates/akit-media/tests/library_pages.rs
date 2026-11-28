//! `media_pages`' library and player pages (I148) through akit's listener, as `akit run` serves
//! them: fit's `handler/mediapages/library_test.go` ported: the pages and hls.js; the libraries
//! and channels the viewer may play; a page of a library's items, searched and paged; `api/play`'s
//! plain and signed URLs, which the vod handler plays; a library not scanned yet; the params'
//! checks. Every wait is on the library's status, with a deadline.
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

use akio::Engine;
use akio_crypto::base64::STANDARD as B64;
use akio_crypto::bcrypt;
use akit_config::value::{Value, parse};
use akit_roles::host::Manager;
use akit_runtime::upstream::EngineSlot;
use akit_runtime::{Env, GraphCell, Registry, compile};

const T: Duration = Duration::from_secs(30);
const TOKEN: &str = "0123456789abcdef-token";
const LIB_PARAMS: &str = r#"{"auth": "live", "login": true, "libraries": ["films", "other"]}"#;

fn lib_root() -> String {
    std::fs::canonicalize(
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/data/medialib/lib"),
    )
    .unwrap()
    .display()
    .to_string()
}

/// fit's `libDoc`: the libraries films and other, a vod route on /vod/* and media_pages on
/// /media/* of `params`, under the policy live: ann may play films/* of films and tv/*, viv (a
/// bcrypt user) everything, and the token anything. `signing` is the policy's url_signing, or
/// "". `extra` is more of the config.
fn lib_doc(params: &str, signing: &str, extra: &str) -> String {
    static HASH: std::sync::LazyLock<String> =
        std::sync::LazyLock::new(|| bcrypt::hash(b"viv-password", bcrypt::MIN_COST).unwrap());
    let signing = if signing.is_empty() {
        String::new()
    } else {
        format!(r#", "url_signing": {signing}"#)
    };
    let root = lib_root();
    format!(
        r#"{{"listeners": [{{"name": "web", "address": "127.0.0.1:0"}}],
          "media_library": [{{"name": "films", "roots": ["{root}"]}}, {{"name": "other", "roots": ["{root}/films"]}}],
          "routes": [
            {{"match": {{"path": ["/media", "/media/*"]}}, "handler": {{"module": "media_pages", "params": {params}}}}},
            {{"match": {{"path": ["/vod/*"]}}, "handler": {{"module": "vod", "params": {{"library": "films", "auth": "live"}}}}}}],
          "media_auth": [{{"name": "live", "secret": "a-secret-of-some-length-0123456789"{signing},
            "users": [
              {{"name": "ann", "password": "ann-password", "play": ["vod/films/films/*", "tv/*"]}},
              {{"name": "viv", "password_hash": "{}", "play": ["*"]}}],
            "tokens": [{{"name": "all", "token": "{TOKEN}", "publish": ["*"], "play": ["*"]}}]}}]{extra}}}"#,
        *HASH
    )
}

struct Akit {
    engine: Arc<Engine>,
    roles: Manager,
    addr: SocketAddr,
    _cell: GraphCell,
}

impl Drop for Akit {
    fn drop(&mut self) {
        self.roles.stop(Duration::ZERO);
        self.engine.shutdown();
    }
}

/// akit serving `json`, its roles applied, its libraries scanned when `scan`; without `scan`
/// the role `media_library` is not run, so that no library is there.
fn start(json: &str, scan: bool) -> Akit {
    let config = akit_config::from_json(json, None).unwrap_or_else(|e| panic!("{json}: {e}"));
    let slot = EngineSlot::new();
    let registry = Registry::new(&[akit_media::MODULES]).unwrap();
    let cell = GraphCell::new(compile(&config, &registry, &Env::new(slot.clone())).unwrap());
    let engine =
        Arc::new(akit_listener::engine(&cell, &akit_listener::Options::default()).unwrap());
    engine.start().unwrap();
    assert!(slot.set(Arc::clone(&engine)));
    let addr = engine.local_addrs()[0];
    let roles: &[akit_roles::host::Info] = if scan {
        &[
            akit_media::role::MEDIA_AUTH,
            akit_media::library::MEDIA_LIBRARY,
            akit_media::streaming::STREAMING,
        ]
    } else {
        &[akit_media::role::MEDIA_AUTH]
    };
    let mut m = Manager::new(roles, Arc::clone(&engine), None);
    let errs = m.apply(&config);
    assert!(errs.is_empty(), "{errs:?}");
    if scan {
        let key = akit_media::library::key(&engine);
        for name in ["films", "other"] {
            akit_media::library::lookup(key, name)
                .unwrap()
                .wait_status(T, |st| st.scans >= 1 && !st.scanning)
                .expect("the library's first scan");
        }
    }
    Akit {
        engine,
        roles: m,
        addr,
        _cell: cell,
    }
}

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

fn http(a: &Akit, method: &str, target: &str, fields: &[(&str, &str)], body: &str) -> Resp {
    let mut s = TcpStream::connect(a.addr).unwrap();
    s.set_read_timeout(Some(T)).unwrap();
    let mut req = format!("{method} {target} HTTP/1.1\r\nHost: akit\r\nConnection: close\r\n");
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

fn get(a: &Akit, target: &str, fields: &[(&str, &str)]) -> Resp {
    http(a, "GET", target, fields, "")
}

fn play(a: &Akit, body: &str, fields: &[(&str, &str)]) -> Resp {
    http(a, "POST", "/media/api/play", fields, body)
}

fn basic(user: &str, pw: &str) -> String {
    format!("Basic {}", B64.encode(format!("{user}:{pw}")))
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
        _ => 0,
    }
}

fn items(v: &Value) -> Vec<Value> {
    match v.get("items") {
        Some(Value::Array(a)) => a.clone(),
        _ => Vec::new(),
    }
}

fn ids(v: &Value) -> String {
    items(v)
        .iter()
        .map(|i| s(i, "id"))
        .collect::<Vec<_>>()
        .join(",")
}

/// A page of the library films, with `query`.
fn page(a: &Akit, query: &str, fields: &[(&str, &str)]) -> (u16, Value) {
    let r = get(a, &format!("/media/api/library/films{query}"), fields);
    (r.status, r.json())
}

/// The value of `key` in the query of `url`.
fn query_value(url: &str, key: &str) -> String {
    url.split_once('?')
        .map(|(_, q)| q)
        .unwrap_or("")
        .split('&')
        .find_map(|kv| kv.strip_prefix(&format!("{key}=")))
        .unwrap_or("")
        .to_owned()
}

fn unix_now() -> i64 {
    i64::try_from(
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs(),
    )
    .unwrap()
}

/// fit's `TestLibraryPages`.
#[test]
fn library_pages() {
    let a = start(&lib_doc(LIB_PARAMS, "", ""), true);
    for p in ["/media/library", "/media/watch"] {
        let r = get(&a, &format!("{p}?library=films&id=films/bbb"), &[]);
        assert_eq!(r.status, 200, "{p}");
        assert!(r.text().contains(r#"id="view-library""#), "{p}");
    }
    let r = get(&a, "/media/vendor/hls.min.js", &[]);
    assert_eq!(r.status, 200);
    assert_eq!(
        r.header("content-type"),
        Some("text/javascript; charset=utf-8")
    );
    assert!(r.text().contains("Hls"));
    let r = get(&a, "/media/", &[]);
    let csp = r.header("content-security-policy").unwrap_or_default();
    assert!(csp.contains("worker-src 'self' blob:"), "{csp}");
    let conf = get(&a, "/media/api/config", &[]).text();
    assert!(
        conf.contains(r#""library":{"channels":false,"libraries":["films","other"]}"#),
        "{conf}"
    );

    // Without credentials, nothing is listed.
    assert_eq!(get(&a, "/media/api/library", &[]).status, 401);
    let ann_b = basic("ann", "ann-password");
    let ann: &[(&str, &str)] = &[("Authorization", &ann_b)];
    let r = get(&a, "/media/api/library", ann);
    let body = r.text();
    assert_eq!(r.status, 200, "{body}");
    assert!(body.contains(r#""subject":"ann""#), "{body}");
    assert!(
        body.contains(r#"{"name":"films","ready":true,"scanned":"#),
        "{body}"
    );
    assert!(body.contains(r#""channels":[]"#), "{body}");

    // ann sees the items she may play, those under films/; the token sees every item that can
    // be served, broken not among them.
    let (code, p) = page(&a, "", ann);
    assert_eq!(
        (code, ids(&p).as_str(), n(&p, "total")),
        (200, "films/bbb", 1)
    );
    let it = &items(&p)[0];
    assert_eq!(s(it, "poster"), "/vod/films/films/bbb/poster.jpg");
    assert_eq!(s(it, "title"), "bbb");
    assert_eq!(n(it, "duration"), 1);
    assert_eq!((n(it, "width"), n(it, "height")), (128, 96));
    assert!(matches!(it.get("renditions"), Some(Value::Array(r)) if r.len() == 2));
    assert_eq!((n(it, "audio"), n(it, "subtitles")), (1, 2));
    let bearer_v = format!("Bearer {TOKEN}");
    let bearer: &[(&str, &str)] = &[("Authorization", &bearer_v)];
    let (code, p) = page(&a, "", bearer);
    let all = ids(&p);
    assert_eq!((code, n(&p, "total")), (200, 4), "{all}");
    assert!(all.starts_with("clips/Av1-Opus-"), "{all}");
    assert!(all.ends_with(",clips/late,films/bbb,sintel"), "{all}");
    let (code, p) = page(&a, "?offset=1&limit=2", bearer);
    assert_eq!(
        (code, ids(&p).as_str(), n(&p, "total"), n(&p, "offset")),
        (200, "clips/late,films/bbb", 4, 1)
    );
    let (code, p) = page(&a, "?q=SINT", bearer);
    assert_eq!((code, ids(&p).as_str()), (200, "sintel"));
    assert_eq!(s(&items(&p)[0], "poster"), "/vod/films/sintel/poster.png");
    let (code, p) = page(&a, "?offset=9", bearer);
    assert_eq!((code, n(&p, "total"), items(&p).len()), (200, 4, 0));
    let long = format!("?q={}", "a".repeat(129));
    for q in ["?offset=-1", "?limit=0", "?limit=501", "?offset=x", &long] {
        assert_eq!(page(&a, q, bearer).0, 400, "{q}");
    }
    // viv's password is a bcrypt hash, checked on the pool.
    let viv_b = basic("viv", "viv-password");
    let (code, p) = page(&a, "", &[("Authorization", &viv_b)]);
    assert_eq!((code, n(&p, "total")), (200, 4));
    let wrong = basic("viv", "wrong");
    assert_eq!(page(&a, "", &[("Authorization", &wrong)]).0, 401);
    for (method, path, want) in [
        ("POST", "/media/api/library", 405),
        ("GET", "/media/api/library/nothing", 404),
        ("GET", "/media/api/libraryx", 404),
        ("GET", "/media/api/library/", 404),
        ("GET", "/media/api/play", 405),
    ] {
        assert_eq!(
            http(&a, method, path, bearer, "").status,
            want,
            "{method} {path}"
        );
    }

    // The login's cookie plays what api/play answers: the plain URL, as the policy signs none.
    let r = http(
        &a,
        "POST",
        "/media/api/login",
        &[],
        r#"{"user": "ann", "password": "ann-password"}"#,
    );
    assert_eq!(r.status, 200, "{}", r.text());
    let cookie_v = r
        .header("set-cookie")
        .unwrap()
        .split(';')
        .next()
        .unwrap()
        .to_owned();
    let cookie: &[(&str, &str)] = &[("Cookie", &cookie_v)];
    let r = play(&a, r#"{"library": "films", "id": "films/bbb"}"#, cookie);
    let pl = r.json();
    assert_eq!(r.status, 200, "{}", r.text());
    assert_eq!(s(&pl, "url"), "/vod/films/films/bbb/index.m3u8");
    assert_eq!(pl.get("signed"), Some(&Value::Bool(false)));
    assert!(pl.get("expires").is_none());
    assert_eq!(
        (s(&pl, "title"), s(&pl, "kind")),
        ("bbb".into(), "vod".into())
    );
    let r = get(&a, &s(&pl, "url"), cookie);
    assert_eq!(r.status, 200);
    assert!(r.text().contains("#EXTM3U"));
    assert_eq!(get(&a, &s(&pl, "url"), &[]).status, 401);
    for (body, fields, want) in [
        (r#"{"library": "films", "id": "sintel"}"#, cookie, 403),
        (r#"{"library": "films", "id": "films/bbb"}"#, &[][..], 401),
        (r#"{"library": "films", "id": "nothing"}"#, cookie, 404),
        (r#"{"library": "films", "id": "broken"}"#, bearer, 404),
        (r#"{"library": "nothing", "id": "films/bbb"}"#, cookie, 404),
        (r#"{"channel": "tv/films"}"#, cookie, 404),
        (r#"{"library": "films"}"#, cookie, 400),
        (
            r#"{"library": "films", "id": "films/bbb", "channel": "tv"}"#,
            cookie,
            400,
        ),
        ("nonsense", cookie, 400),
    ] {
        let r = play(&a, body, fields);
        assert_eq!(r.status, want, "{body}: {}", r.text());
    }
    let r = play(&a, r#"{"library": "other", "id": "sintel"}"#, bearer);
    assert_eq!(r.status, 200);
    assert_eq!(s(&r.json(), "url"), "/vod/other/sintel/index.m3u8");
}

/// fit's `TestLibrarySigned`: with the policy's url_signing, api/play answers a URL signed for
/// the viewer, its address and a session of its own, which plays with no other credentials.
#[test]
fn library_signed() {
    let a = start(
        &lib_doc(LIB_PARAMS, r#"{"ttl": "1m", "play_grace": "1m"}"#, ""),
        true,
    );
    let ann_b = basic("ann", "ann-password");
    let ann: &[(&str, &str)] = &[("Authorization", &ann_b)];
    let start = unix_now();
    let r = play(&a, r#"{"library": "films", "id": "films/bbb"}"#, ann);
    assert_eq!(r.status, 200, "{}", r.text());
    let pl = r.json();
    assert_eq!(pl.get("signed"), Some(&Value::Bool(true)));
    let exp = n(&pl, "expires");
    assert!(exp >= start + 59 && exp <= unix_now() + 61, "{exp}");
    let url = s(&pl, "url");
    assert!(url.starts_with("/vod/films/films/bbb/index.m3u8?"), "{url}");
    assert_eq!(query_value(&url, "sub"), "ann");
    assert_eq!(query_value(&url, "ip"), "127.0.0.1");
    assert_eq!(query_value(&url, "sid").len(), 16, "{url}");
    assert!(!query_value(&url, "sig").is_empty());
    assert!(query_value(&url, "kid").is_empty());
    assert_eq!(query_value(&url, "exp"), exp.to_string());
    // The signed URL plays with no other credentials, and hands its grant on.
    let r = get(&a, &url, &[]);
    assert_eq!(r.status, 200, "{}", r.text());
    assert!(r.text().contains("g=1"), "{}", r.text());
    // Another session each time.
    let r = play(&a, r#"{"library": "films", "id": "films/bbb"}"#, ann);
    assert!(!s(&r.json(), "url").contains(&query_value(&url, "sid")));
}

/// fit's `TestLibraryNotScanned`.
#[test]
fn library_not_scanned() {
    let a = start(&lib_doc(LIB_PARAMS, "", ""), false);
    let bearer_v = format!("Bearer {TOKEN}");
    let bearer: &[(&str, &str)] = &[("Authorization", &bearer_v)];
    let r = get(&a, "/media/api/library", bearer);
    assert_eq!(r.status, 200);
    assert!(
        r.text().contains(r#"{"name":"films","ready":false}"#),
        "{}",
        r.text()
    );
    let r = get(&a, "/media/api/library/films", bearer);
    assert_eq!((r.status, r.header("retry-after")), (503, Some("1")));
    let r = play(&a, r#"{"library": "films", "id": "films/bbb"}"#, bearer);
    assert_eq!(r.status, 503);
}

/// fit's `TestLibraryChannels`, with the streaming role's own channels: those the viewer may play
/// are listed with what each plays, and api/play signs a channel's URL.
#[test]
fn library_channels() {
    let params = r#"{"auth": "live", "channels": true, "hls": "/hls/{stream}/index.m3u8?format=ts", "vod": "https://cdn.example.com/v/{library}/{id}/master.m3u8"}"#;
    let extra = r#", "streaming": {"channels": [{"name": "tv/films", "library": "films", "items": ["films/bbb"]}, {"name": "secret/news", "library": "films", "items": ["sintel"]}]}"#;
    let a = start(&lib_doc(params, "{}", extra), true);
    let ann_b = basic("ann", "ann-password");
    let ann: &[(&str, &str)] = &[("Authorization", &ann_b)];
    let body = get(&a, "/media/api/library", ann).text();
    assert!(
        body.contains(r#""libraries":[],"channels":[{"name":"tv/films","state":"#),
        "{body}"
    );
    assert!(!body.contains("secret/news"), "{body}");
    let bearer_v = format!("Bearer {TOKEN}");
    let body = get(&a, "/media/api/library", &[("Authorization", &bearer_v)]).text();
    assert!(body.contains(r#"{"name":"secret/news","state":"#), "{body}");
    let r = play(&a, r#"{"channel": "tv/films"}"#, ann);
    let pl = r.json();
    assert_eq!(r.status, 200, "{}", r.text());
    assert_eq!(pl.get("signed"), Some(&Value::Bool(true)));
    assert_eq!(s(&pl, "kind"), "channel");
    assert!(
        s(&pl, "url").starts_with("/hls/tv/films/index.m3u8?format=ts&exp="),
        "{}",
        s(&pl, "url")
    );
    for (body, want) in [
        (r#"{"channel": "secret/news"}"#, 403),
        (r#"{"channel": "tv/none"}"#, 404),
        (r#"{"library": "films", "id": "films/bbb"}"#, 404),
    ] {
        let r = play(&a, body, ann);
        assert_eq!(r.status, want, "{body}: {}", r.text());
    }
    assert_eq!(get(&a, "/media/api/library/films", ann).status, 404);
}

/// fit's `TestLibraryPublicAndNoPolicy`.
#[test]
fn library_public() {
    let doc = lib_doc(r#"{"libraries": ["films"]}"#, "", "").replacen(
        r#""secret""#,
        r#""public": ["play"], "secret""#,
        1,
    );
    let a = start(&doc, true);
    let (code, p) = page(&a, "", &[]);
    assert_eq!((code, n(&p, "total")), (200, 4));
    let r = play(&a, r#"{"library": "films", "id": "sintel"}"#, &[]);
    assert_eq!(r.status, 200);
    assert_eq!(r.json().get("signed"), Some(&Value::Bool(false)));
}

/// fit's `TestLibraryOff`.
#[test]
fn library_off() {
    let a = start(&lib_doc(r#"{"auth": "live"}"#, "", ""), false);
    assert!(
        get(&a, "/media/api/config", &[])
            .text()
            .contains(r#""library":null"#)
    );
    let bearer_v = format!("Bearer {TOKEN}");
    let bearer: &[(&str, &str)] = &[("Authorization", &bearer_v)];
    assert_eq!(get(&a, "/media/api/library", bearer).status, 404);
    assert_eq!(play(&a, r#"{"channel": "tv/films"}"#, bearer).status, 404);
}

/// fit's `TestLibraryParams`.
#[test]
fn library_params() {
    for (params, want) in [
        (
            r#"{"libraries": ["nothing"]}"#,
            r#"there is no media_library named "nothing""#,
        ),
        (
            r#"{"libraries": ["films", "films"]}"#,
            r#""films" is listed twice"#,
        ),
        (
            r#"{"libraries": ["a","b","c","d","e","f","g","h","i","j","k","l","m","n","o","p","q"]}"#,
            "17 libraries: at most 16",
        ),
        (r#"{"vod": "/v/{id}/index.m3u8"}"#, "name libraries too"),
        (r#"{"channels": true}"#, "set hls"),
        (
            r#"{"libraries": ["films"], "vod": "/v/{id}"}"#,
            "is not a path",
        ),
        (
            r#"{"libraries": ["films"], "vod": "/v/{library}/x.m3u8"}"#,
            "is not a path",
        ),
        (
            r#"{"libraries": ["films"], "vod": "/v/{id}/{id}/x.m3u8"}"#,
            "is not a path",
        ),
        (
            r#"{"libraries": ["films"], "vod": "/v/{id}/{library}"}"#,
            "is not a path",
        ),
        (
            r#"{"libraries": ["films"], "vod": "/v/{id}/x.m3u8?a=b"}"#,
            "is not a path",
        ),
        (
            r#"{"libraries": ["films"], "vod": "ftp://h/{id}/x.m3u8"}"#,
            "is not a path",
        ),
        (
            r#"{"libraries": ["films"], "vod": "https:///{id}/x.m3u8"}"#,
            "is not a path",
        ),
    ] {
        let doc = lib_doc(params, "", "");
        let err = match akit_config::from_json(&doc, None) {
            Err(e) => e.to_string(),
            Ok(c) => {
                let registry = Registry::new(&[akit_media::MODULES]).unwrap();
                match compile(&c, &registry, &Env::new(EngineSlot::new())) {
                    Err(e) => e.to_string(),
                    Ok(_) => String::new(),
                }
            }
        };
        assert!(err.contains(want), "{params}: {err:?}, want {want:?}");
    }
    // vod on another server joins the pages' Content-Security-Policy.
    let a = start(
        &lib_doc(
            r#"{"libraries": ["films"], "vod": "https://cdn.example.com/v/{library}/{id}/index.m3u8"}"#,
            "",
            "",
        ),
        false,
    );
    let r = get(&a, "/media/", &[]);
    let csp = r.header("content-security-policy").unwrap_or_default();
    assert!(
        csp.contains("img-src 'self' data: blob: https://cdn.example.com;"),
        "{csp}"
    );
}
