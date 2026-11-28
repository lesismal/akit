//! The `vod` handler (I142) through akit's listener, with the role `media_library` scanning fit's
//! fixtures (`tests/data/medialib`): fit's `handler/vod` tests of what an item's names answer,
//! its playlists, segments, subtitles and files, its `auth`, its cache, and a file changed
//! while it is served. Every wait is on the library's status, with a deadline.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing,
    clippy::arithmetic_side_effects
)]

use std::io::{BufRead, BufReader, Read, Write};
use std::net::{SocketAddr, TcpStream};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering::SeqCst};
use std::time::Duration;

use akio::Engine;
use akit_roles::host::Manager;
use akit_runtime::upstream::EngineSlot;
use akit_runtime::{Env, GraphCell, Registry, compile};

const TOKEN: &str = "0123456789abcdef-viewer";

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
}

fn request(addr: SocketAddr, method: &str, target: &str, fields: &[(&str, &str)]) -> Resp {
    let s = TcpStream::connect(addr).unwrap();
    s.set_read_timeout(Some(Duration::from_secs(30))).unwrap();
    let mut req = format!("{method} {target} HTTP/1.1\r\nHost: x\r\nConnection: close\r\n");
    for (n, v) in fields {
        req.push_str(&format!("{n}: {v}\r\n"));
    }
    req.push_str("\r\n");
    (&s).write_all(req.as_bytes()).unwrap();
    let mut r = BufReader::new(s);
    let mut line = String::new();
    r.read_line(&mut line).unwrap();
    let status = line.split(' ').nth(1).unwrap().parse().unwrap();
    let mut headers = Vec::new();
    loop {
        let mut l = String::new();
        r.read_line(&mut l).unwrap();
        let l = l.trim_end();
        if l.is_empty() {
            break;
        }
        let (n, v) = l.split_once(':').unwrap();
        headers.push((n.to_owned(), v.trim().to_owned()));
    }
    let mut resp = Resp {
        status,
        headers,
        body: Vec::new(),
    };
    if method != "HEAD" {
        if let Some(len) = resp.header("content-length") {
            let mut b = vec![0; len.parse().unwrap()];
            r.read_exact(&mut b).unwrap();
            resp.body = b;
        }
    }
    resp
}

fn get(addr: SocketAddr, target: &str) -> Resp {
    request(addr, "GET", target, &[])
}

/// A directory of the test's own, removed when it is dropped.
struct TempDir(PathBuf);

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

static NEXT: AtomicUsize = AtomicUsize::new(0);

fn copy_dir(from: &Path, to: &Path) {
    std::fs::create_dir_all(to).unwrap();
    for e in std::fs::read_dir(from).unwrap() {
        let e = e.unwrap();
        let dst = to.join(e.file_name());
        if e.file_type().unwrap().is_dir() {
            copy_dir(&e.path(), &dst);
        } else {
            std::fs::copy(e.path(), dst).unwrap();
        }
    }
}

fn copy_lib() -> TempDir {
    let n = NEXT.fetch_add(1, SeqCst);
    let p = std::env::temp_dir().join(format!("akit-vod-{}-{n}", std::process::id()));
    let _ = std::fs::remove_dir_all(&p);
    copy_dir(
        &Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/data/medialib/lib"),
        &p,
    );
    TempDir(std::fs::canonicalize(p).unwrap())
}

struct Akit {
    engine: Arc<Engine>,
    roles: Manager,
    http: SocketAddr,
    _cell: GraphCell,
}

impl Drop for Akit {
    fn drop(&mut self) {
        self.roles.stop(Duration::ZERO);
        self.engine.shutdown();
    }
}

/// akit serving `json`'s routes, its roles applied when `roles`, and the library `lib` scanned.
fn start(json: &str, roles: bool) -> Akit {
    let config = akit_config::from_json(json, None).unwrap_or_else(|e| panic!("{json}: {e}"));
    let slot = EngineSlot::new();
    let registry = Registry::new(&[akit_media::MODULES]).unwrap();
    let cell = GraphCell::new(compile(&config, &registry, &Env::new(slot.clone())).unwrap());
    let engine =
        Arc::new(akit_listener::engine(&cell, &akit_listener::Options::default()).unwrap());
    engine.start().unwrap();
    assert!(slot.set(Arc::clone(&engine)));
    let http = engine.local_addrs()[0];
    let mut m = Manager::new(
        &[
            akit_media::role::MEDIA_AUTH,
            akit_media::library::MEDIA_LIBRARY,
        ],
        Arc::clone(&engine),
        None,
    );
    if roles {
        let errs = m.apply(&config);
        assert!(errs.is_empty(), "{errs:?}");
        let l = akit_media::library::lookup(akit_media::library::key(&engine), "lib").unwrap();
        l.wait_status(Duration::from_secs(30), |st| st.scans >= 1 && !st.scanning)
            .expect("the library's first scan");
    }
    Akit {
        engine,
        roles: m,
        http,
        _cell: cell,
    }
}

fn config(root: &Path, params: &str) -> String {
    format!(
        r#"{{"listeners": [{{"name": "web", "address": "127.0.0.1:0"}}],
          "media_auth": [{{"name": "site", "tokens": [{{"token": "{TOKEN}", "play": ["vod/lib/films/*"]}}]}}],
          "media_library": [{{"name": "lib", "roots": ["{}"]}}],
          "routes": [{{"match": {{"path": ["/vod/*"]}}, "handler": {{"module": "vod", "params": {{"library": "lib"{params}}}}}}}]}}"#,
        root.display()
    )
}

const B: &str = "/vod/lib/films/bbb/";

/// fit's `TestVOD`: what an item's names answer.
#[test]
fn an_items_names() {
    let lib = copy_lib();
    let a = start(
        &config(
            &lib.0,
            r#", "cache_control": "public, max-age=60", "cors": "*""#,
        ),
        true,
    );
    let h = a.http;
    let r = get(h, &format!("{B}index.m3u8"));
    assert_eq!(r.status, 200, "{}", r.text());
    assert_eq!(
        r.header("content-type"),
        Some("application/vnd.apple.mpegurl")
    );
    assert_eq!(r.header("cache-control"), Some("public, max-age=60"));
    assert_eq!(r.header("access-control-allow-origin"), Some("*"));
    let m = r.text();
    for want in [
        "#EXT-X-VERSION:7",
        "CODECS=\"avc1.64000a,mp4a.40.2\",RESOLUTION=128x96,AUDIO=\"audio\",SUBTITLES=\"subs\"\nv0.fmp4.m3u8",
        "NAME=\"audio-fr\",LANGUAGE=\"fr\",DEFAULT=NO,AUTOSELECT=YES,URI=\"a0.fmp4.m3u8\"",
        "TYPE=SUBTITLES,GROUP-ID=\"subs\",NAME=\"fr\",LANGUAGE=\"fr\",DEFAULT=NO,AUTOSELECT=YES,FORCED=YES,URI=\"s1.m3u8\"",
    ] {
        assert!(m.contains(want), "{want:?} not in\n{m}");
    }
    assert!(
        get(h, &format!("{B}index.ts.m3u8"))
            .text()
            .contains("v1.ts.m3u8")
    );
    // A media playlist, its initialization section and its segments.
    let p = get(h, &format!("{B}v1.fmp4.m3u8")).text();
    assert!(
        p.contains("#EXT-X-MAP:URI=\"v1.init.mp4\"") && p.contains("v1.0.m4s\n#EXT-X-ENDLIST"),
        "{p}"
    );
    let init = get(h, &format!("{B}v1.init.mp4"));
    assert_eq!(init.status, 200);
    assert_eq!(init.header("content-type"), Some("video/mp4"));
    assert_eq!(&init.body[4..8], b"ftyp");
    let seg = get(h, &format!("{B}v1.0.m4s"));
    assert_eq!(seg.status, 200, "{}", seg.text());
    assert!(seg.body.windows(4).any(|w| w == b"moof") && seg.body.windows(4).any(|w| w == b"mdat"));
    let etag = seg.header("etag").unwrap().to_owned();
    assert_eq!(
        request(
            h,
            "GET",
            &format!("{B}v1.0.m4s"),
            &[("If-None-Match", &etag)]
        )
        .status,
        304
    );
    let ts = get(h, &format!("{B}v1.0.ts"));
    assert_eq!(ts.header("content-type"), Some("video/mp2t"));
    assert!(ts.body.len() % 188 == 0 && ts.body.chunks(188).all(|p| p[0] == 0x47));
    assert_eq!(get(h, &format!("{B}v1.1.m4s")).status, 404);
    // Audio alone.
    let p = get(h, &format!("{B}a0.ts.m3u8")).text();
    assert!(p.contains("a0.0.ts"), "{p}");
    assert_eq!(
        get(h, &format!("{B}a0.init.mp4")).header("content-type"),
        Some("audio/mp4")
    );
    // Subtitles, as WebVTT on the segments' clock.
    let s = get(h, &format!("{B}s0.m3u8")).text();
    assert!(s.contains("#EXTINF:1.000,\ns0.vtt"), "{s}");
    let v = get(h, &format!("{B}s0.vtt"));
    assert_eq!(v.header("content-type"), Some("text/vtt; charset=utf-8"));
    assert!(
        v.text()
            .starts_with("WEBVTT\nX-TIMESTAMP-MAP=MPEGTS:126000,LOCAL:00:00:00.000\n")
    );
    // The files themselves, with ranges, and the poster.
    let f = request(h, "GET", &format!("{B}v1.mp4"), &[("Range", "bytes=0-99")]);
    assert_eq!((f.status, f.body.len()), (206, 100));
    assert_eq!(f.header("access-control-allow-origin"), Some("*"));
    assert_eq!(get(h, &format!("{B}poster.jpg")).status, 200);
    assert_eq!(get(h, &format!("{B}poster.png")).status, 404);
    assert_eq!(get(h, &format!("{B}v1.mov")).status, 404);
    // What is not there.
    for bad in ["v9.fmp4.m3u8", "v01.fmp4.m3u8", "s5.vtt", "aes.key", "what"] {
        assert_eq!(get(h, &format!("{B}{bad}")).status, 404, "{bad}");
    }
    assert_eq!(get(h, "/vod/lib/none/index.m3u8").status, 404);
    assert_eq!(get(h, "/vod/lib/broken/index.m3u8").status, 404);
    assert_eq!(get(h, "/vod/lib/index.m3u8").status, 404);
    // AV1 is not carried in MPEG-TS.
    let id = "clips/Av1-Opus-288ef7ab";
    assert_eq!(get(h, &format!("/vod/lib/{id}/index.ts.m3u8")).status, 404);
    assert_eq!(get(h, &format!("/vod/lib/{id}/index.m3u8")).status, 200);
    assert_eq!(
        request(h, "POST", &format!("{B}index.m3u8"), &[]).status,
        405
    );
    // Kept: the segment again is a hit.
    let before = akit_media::vod::cache_stats("lib");
    get(h, &format!("{B}v1.0.m4s"));
    assert!(akit_media::vod::cache_stats("lib").hits > before.hits);
}

/// fit's `TestAuth`: refused without credentials; a playlist fetched with `?token=` names what
/// it lists with the token, and is the viewer's own.
#[test]
fn auth() {
    let lib = copy_lib();
    let a = start(&config(&lib.0, r#", "auth": "site""#), true);
    let h = a.http;
    assert_eq!(get(h, &format!("{B}index.m3u8")).status, 401);
    let r = get(h, &format!("{B}index.m3u8?token={TOKEN}"));
    assert_eq!(r.status, 200);
    assert_eq!(r.header("cache-control"), Some("private, no-store"));
    assert!(
        r.text().contains(&format!("v0.fmp4.m3u8?token={TOKEN}")),
        "{}",
        r.text()
    );
    let p = get(h, &format!("{B}v1.fmp4.m3u8?token={TOKEN}")).text();
    assert!(
        p.contains(&format!("v1.init.mp4?token={TOKEN}"))
            && p.contains(&format!("v1.0.m4s?token={TOKEN}")),
        "{p}"
    );
    let r = request(
        h,
        "GET",
        &format!("{B}v1.0.m4s"),
        &[("Authorization", &format!("Bearer {TOKEN}"))],
    );
    assert_eq!(r.status, 200);
    // Another item the token may not play.
    assert_eq!(
        get(h, &format!("/vod/lib/sintel/index.m3u8?token={TOKEN}")).status,
        403
    );
}

/// fit's `TestChanged`: a file changed since the scan is answered 404, and what was kept of it
/// dropped; a rescan serves it again.
#[test]
fn a_file_changed() {
    let lib = copy_lib();
    let a = start(&config(&lib.0, ""), true);
    let h = a.http;
    assert_eq!(get(h, &format!("{B}v1.0.m4s")).status, 200);
    let p = lib.0.join("films/bbb.mp4");
    let later = std::fs::metadata(&p).unwrap().modified().unwrap() + Duration::from_secs(3);
    std::fs::File::options()
        .write(true)
        .open(&p)
        .unwrap()
        .set_modified(later)
        .unwrap();
    // A segment not kept yet reads the file, finds it changed, and is answered 404.
    let l = akit_media::library::lookup(akit_media::library::key(&a.engine), "lib").unwrap();
    let r = get(h, &format!("{B}v1.0.ts"));
    assert_eq!(r.status, 404, "{}", r.text());
    l.wait_status(Duration::from_secs(30), |st| !st.scanning)
        .unwrap();
    let before = l.status().scans;
    l.rescan();
    l.wait_status(Duration::from_secs(30), |st| {
        st.scans > before && !st.scanning
    })
    .unwrap();
    assert_eq!(get(h, &format!("{B}v1.0.ts")).status, 200);
}

/// Without its role, the library is not loaded: 503 with Retry-After.
#[test]
fn not_loaded() {
    let lib = copy_lib();
    let a = start(&config(&lib.0, ""), false);
    let r = get(a.http, &format!("{B}index.m3u8"));
    assert_eq!(r.status, 503);
    assert_eq!(r.header("retry-after"), Some("1"));
}

/// A policy `site` with a keyring, the viewer's token and `url_signing` as `signing` writes it.
fn signed_config(root: &Path, signing: &str) -> String {
    format!(
        r#"{{"listeners": [{{"name": "web", "address": "127.0.0.1:0"}}],
          "media_auth": [{{"name": "site", "keys": [{{"key": "k1", "secret": "the first key of the site"}}, {{"key": "k2", "secret": "the second key of the site"}}],
            "tokens": [{{"token": "{TOKEN}", "play": ["vod/lib/films/*"]}}], "url_signing": {signing}}}],
          "media_library": [{{"name": "lib", "roots": ["{}"]}}],
          "routes": [{{"match": {{"path": ["/vod/*"]}}, "handler": {{"module": "vod", "params": {{"library": "lib", "auth": "site", "cache_control": "public, max-age=60"}}}}}}]}}"#,
        root.display()
    )
}

fn site(a: &Akit) -> Arc<akit_media::auth::Policy> {
    let r = akit_media::auth::registry::key(&a.engine);
    akit_media::auth::registry::lookup(r, "site").unwrap()
}

fn unix_now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs() as i64
}

/// The URL of `name` of bbb's resource for `v`, good until `exp`.
fn sign(a: &Akit, name: &str, v: &akit_media::auth::Viewer, exp: i64, key: &str) -> String {
    format!("{B}{name}?{}", site(a).sign_url(B, v, exp, key).unwrap())
}

fn viewer(sub: &str, sid: &str) -> akit_media::auth::Viewer {
    akit_media::auth::Viewer {
        subject: sub.into(),
        address: None,
        session: sid.into(),
    }
}

/// The URIs a playlist names: its lines that are not tags, and its tags' URI attributes.
fn uris(playlist: &str) -> Vec<String> {
    let mut out = Vec::new();
    for line in playlist.lines() {
        if line.is_empty() {
            continue;
        }
        if line.starts_with('#') {
            let mut rest = line;
            while let Some(i) = rest.find("URI=\"") {
                let tail = &rest[i + 5..];
                let end = tail.find('"').unwrap();
                out.push(tail[..end].to_owned());
                rest = &tail[end..];
            }
        } else {
            out.push(line.to_owned());
        }
    }
    out
}

/// fit's `TestSignedURLs` (I144): an item played through a signed URL: its playlists hand every
/// URI a grant, which plays on once the URL has expired; the URL opens no other item, nor its
/// own path changed in any byte; its playlists are the viewer's own; its signature never
/// reaches the access log's target.
#[test]
fn signed_urls() {
    let lib = copy_lib();
    let a = start(&signed_config(&lib.0, "{}"), true);
    let h = a.http;
    let v = viewer("viewer-1", "s1");
    let u = sign(&a, "index.m3u8", &v, unix_now() + 3600, "");
    let r = get(h, &u);
    assert_eq!(r.status, 200, "{}", r.text());
    assert_eq!(r.header("cache-control"), Some("private, no-store"));
    let list = uris(&r.text());
    assert!(list.len() >= 3, "{list:?}");
    let mut grant = String::new();
    for uri in &list {
        let (name, q) = uri.split_once('?').unwrap();
        assert!(
            q.contains("&g=1&") && q.contains("sub=viewer-1") && !name.contains('/'),
            "{uri}"
        );
        if grant.is_empty() {
            grant = q.to_owned();
        } else {
            assert_eq!(q, grant);
        }
    }
    // What the playlists name plays with the grant: a media playlist, its init section and
    // segments, and subtitles.
    let media = get(h, &format!("{B}v0.fmp4.m3u8?{grant}"));
    assert_eq!(media.status, 200);
    let named = uris(&media.text());
    assert!(named.len() >= 2, "{named:?}");
    for uri in &named {
        assert!(uri.ends_with(&format!("?{grant}")), "{uri}");
        assert_eq!(get(h, &format!("{B}{uri}")).status, 200, "{uri}");
    }
    let subs = get(h, &format!("{B}s0.m3u8?{grant}")).text();
    assert_eq!(get(h, &format!("{B}{}", uris(&subs)[0])).status, 200);
    assert_eq!(get(h, &format!("{B}v0.mp4?{grant}")).status, 200);
    // A media playlist fetched without a query, by a token, is the one the cache keeps.
    let r = request(
        h,
        "GET",
        &format!("{B}v0.fmp4.m3u8"),
        &[("Authorization", &format!("Bearer {TOKEN}"))],
    );
    assert_eq!(r.status, 200);
    assert!(!r.text().contains("sig="));
    assert_eq!(r.header("cache-control"), Some("public, max-age=60"));
    // Expired, the URL is refused, while the grant its playlist handed on plays on.
    let expired = sign(&a, "index.m3u8", &v, unix_now() - 1, "");
    let r = get(h, &expired);
    assert!(
        r.status == 403 && r.text().contains("expired"),
        "{}",
        r.text()
    );
    assert_eq!(get(h, &format!("{B}v0.0.m4s?{grant}")).status, 200);
    // Another item, and the path changed in any byte.
    let q = u.split_once('?').unwrap().1;
    assert_eq!(
        get(h, &format!("/vod/lib/sintel/index.m3u8?{q}")).status,
        403
    );
    for i in "/vod/lib/".len()..B.len() - 1 {
        let mut b = format!("{B}index.m3u8").into_bytes();
        b[i] ^= 0x01;
        let p = String::from_utf8(b).unwrap();
        assert_ne!(get(h, &format!("{p}?{q}")).status, 200, "{p}");
    }
    // Bound to an address: refused from another.
    let mut other = viewer("v", "");
    other.address = Some("192.0.2.1".parse().unwrap());
    assert_eq!(
        get(h, &sign(&a, "index.m3u8", &other, unix_now() + 3600, "")).status,
        403
    );
    other.address = Some("127.0.0.1".parse().unwrap());
    assert_eq!(
        get(h, &sign(&a, "index.m3u8", &other, unix_now() + 3600, "")).status,
        200
    );
    // A signature that does not parse is refused, and counted.
    let r = get(h, &format!("{B}index.m3u8?sig=x"));
    assert!(
        r.status == 403 && r.text().contains("signature"),
        "{}",
        r.text()
    );
    let refused = site(&a).url_refusals();
    assert!(
        refused.iter().any(|(r, n)| *r == "signature" && *n >= 2),
        "{refused:?}"
    );
}

/// fit's `TestSignedURLSessions`: a third session past `max_sessions` 2 refused, and rotating
/// a key refuses the URLs it signed and only those.
#[test]
fn signed_url_sessions() {
    let lib = copy_lib();
    let a = start(&signed_config(&lib.0, r#"{"max_sessions": 2}"#), true);
    let h = a.http;
    let exp = unix_now() + 3600;
    let at = |sid: &str, key: &str| sign(&a, "v0.0.m4s", &viewer("u", sid), exp, key);
    assert_eq!(get(h, &at("a", "k1")).status, 200);
    assert_eq!(get(h, &at("b", "k2")).status, 200);
    let r = get(h, &at("c", "k1"));
    assert!(
        r.status == 403 && r.text().contains("session_limit"),
        "{}",
        r.text()
    );
    let sessions = site(&a).sessions();
    assert_eq!(sessions.len(), 2, "{sessions:?}");
    assert_eq!(sessions[0].resource, B);
    assert!(
        sessions[0].address.starts_with("127.0.0.1:"),
        "{sessions:?}"
    );
    let (ua, ub) = (at("a", "k1"), at("b", "k2"));
    site(&a).rotate_key("k1", "the first key, rotated").unwrap();
    assert_eq!(get(h, &ua).status, 403);
    assert_eq!(get(h, &ub).status, 200);
}

/// fit's `TestSignedURLCookie`: the grant handed on as a cookie; the playlists name their URIs
/// plainly, and a client that keeps the cookie plays.
#[test]
fn signed_url_cookie() {
    let lib = copy_lib();
    let a = start(&signed_config(&lib.0, r#"{"cookie": true}"#), true);
    let h = a.http;
    let u = sign(&a, "v0.fmp4.m3u8", &viewer("u", ""), unix_now() + 3600, "");
    let r = get(h, &u);
    assert_eq!(r.status, 200, "{}", r.text());
    let set = r.header("set-cookie").unwrap().to_owned();
    assert!(
        set.starts_with("akit_media_grant=exp=")
            && set.contains(&format!("; Path={B};"))
            && set.contains("; HttpOnly"),
        "{set}"
    );
    assert!(!r.text().contains('?'), "{}", r.text());
    let cookie = set.split(';').next().unwrap().to_owned();
    for uri in &uris(&r.text())[..2] {
        let r = request(h, "GET", &format!("{B}{uri}"), &[("Cookie", &cookie)]);
        assert_eq!(r.status, 200, "{uri}: {}", r.text());
    }
    // Kept by the cache, the playlist goes out again with a cookie of its own; the cookie is
    // refused for another item.
    let r = get(h, &u);
    assert!(r.header("set-cookie").is_some());
    assert_eq!(r.header("cache-control"), Some("private, no-store"));
    let r = request(
        h,
        "GET",
        "/vod/lib/sintel/index.m3u8",
        &[("Cookie", &cookie)],
    );
    assert_ne!(r.status, 200);
    assert_eq!(get(h, &format!("{B}v0.0.m4s")).status, 401);
}

const LIB_SECRET: &str = "the library's secret, 16 bytes or more";

/// A player's AES-128 of segment `n` under `key`.
fn decrypt(key: &[u8], n: u64, data: &[u8]) -> Vec<u8> {
    assert!(
        !data.is_empty() && data.len() % 16 == 0,
        "{} bytes",
        data.len()
    );
    let mut out = data.to_vec();
    let k: [u8; 16] = key.try_into().unwrap();
    akio_crypto::aes::Aes::new128(&k)
        .decrypt_cbc_padded(&u128::from(n).to_be_bytes(), &mut out)
        .unwrap();
    out
}

/// fit's `TestEncrypt` and `TestEncryptTokens` (I147): an encrypted item's media playlists
/// name the key after the init section, the key goes to whom auth lets play the item alone, and
/// its segments, fMP4 and MPEG-TS, decrypt with it into the clear handler's own; the key of an
/// item another token may not play is refused, and the files are not served.
#[test]
fn encrypt() {
    let lib = copy_lib();
    let json = format!(
        r#"{{"listeners": [{{"name": "web", "address": "127.0.0.1:0"}}],
          "media_auth": [{{"name": "site", "tokens": [{{"token": "{TOKEN}", "play": ["vod/lib/films/*"]}}]}}],
          "media_library": [{{"name": "lib", "roots": ["{}"], "secret": "{LIB_SECRET}"}}],
          "routes": [{{"match": {{"path": ["/vod/*"]}}, "handler": {{"module": "vod", "params": {{"library": "lib", "auth": "site", "encrypt": true, "cache_control": "public, max-age=60"}}}}}},
                     {{"match": {{"path": ["/clear/*"]}}, "handler": {{"module": "vod", "params": {{"library": "lib", "prefix": "/clear/lib/"}}}}}}]}}"#,
        lib.0.display()
    );
    let a = start(&json, true);
    let h = a.http;
    let clear = |name: &str| get(h, &format!("/clear/lib/films/bbb/{name}"));
    let bearer = format!("Bearer {TOKEN}");
    let with = |name: &str| {
        request(
            h,
            "GET",
            &format!("{B}{name}"),
            &[("Authorization", &bearer)],
        )
    };
    let master = with("index.m3u8");
    assert_eq!(master.status, 200);
    assert!(!master.text().contains("EXT-X-KEY"), "{}", master.text());
    let keys = akit_media::library::keys::Keys::new("lib", LIB_SECRET).unwrap();
    let want = keys.key(akit_media::library::keys::KEY_ITEM, "films/bbb", 0);
    let key = with("aes.key");
    assert_eq!(key.status, 200);
    assert_eq!(key.body, want);
    assert_eq!(key.header("cache-control"), Some("private, no-store"));
    assert_eq!(key.header("content-type"), Some("application/octet-stream"));
    for (c, ext) in [("fmp4", "m4s"), ("ts", "ts")] {
        let media = with(&format!("v0.{c}.m3u8")).text();
        let k = media
            .find(r#"#EXT-X-KEY:METHOD=AES-128,URI="aes.key""#)
            .unwrap_or_else(|| panic!("{media}"));
        assert_eq!(media.matches("#EXT-X-KEY").count(), 1);
        assert!(media.find("#EXT-X-MAP").is_none_or(|m| m < k), "{media}");
        assert!(media.find("#EXTINF").unwrap() > k, "{media}");
        let name = format!("v0.0.{ext}");
        let enc = with(&name);
        let plain = clear(&name);
        assert_eq!((enc.status, plain.status), (200, 200));
        assert_ne!(enc.body[..16], plain.body[..16]);
        assert_eq!(decrypt(&key.body, 0, &enc.body), plain.body);
        assert_eq!(enc.header("cache-control"), Some("public, max-age=60"));
        assert_ne!(enc.header("etag"), plain.header("etag"));
        assert!(enc.header("etag").is_some());
    }
    // The init section stays clear.
    assert_eq!(&with("v0.init.mp4").body[4..8], b"ftyp");
    // With a token in the query, the playlist names the key with it.
    let media = get(h, &format!("{B}v0.ts.m3u8?token={TOKEN}")).text();
    assert!(
        media.contains(&format!(r#"URI="aes.key?token={TOKEN}""#)),
        "{media}"
    );
    // The key without credentials, of an item the token may not play, and of a clear item.
    assert_eq!(get(h, &format!("{B}aes.key")).status, 401);
    assert_eq!(
        request(
            h,
            "GET",
            "/vod/lib/sintel/aes.key",
            &[("Authorization", &bearer)]
        )
        .status,
        403
    );
    assert_eq!(clear("aes.key").status, 404);
    // The files are not served; the poster is.
    assert_eq!(with("v0.mp4").status, 404);
    assert_eq!(with("poster.jpg").status, 200);
}

fn encrypted_config(root: &Path, secret_file: &Path) -> String {
    format!(
        r#"{{"listeners": [{{"name": "web", "address": "127.0.0.1:0"}}],
          "media_auth": [{{"name": "site", "tokens": [{{"token": "{TOKEN}", "play": ["vod/lib/films/*"]}}]}}],
          "media_library": [{{"name": "lib", "roots": ["{}"], "secret_file": "{}"}}],
          "routes": [{{"match": {{"path": ["/vod/*"]}}, "handler": {{"module": "vod", "params": {{"library": "lib", "auth": "site", "encrypt": true}}}}}}]}}"#,
        root.display(),
        secret_file.display()
    )
}

/// fit's `TestEncryptSecretChanged` (I147): a new secret, read from `secret_file` as the config
/// is applied again, makes new keys, new segments and new ETags of the same media.
#[test]
fn encrypt_secret_changed() {
    let lib = copy_lib();
    let file = lib.0.join("secret");
    std::fs::write(&file, "the first secret of the library\n").unwrap();
    let json = encrypted_config(&lib.0, &file);
    let mut a = start(&json, true);
    let h = a.http;
    let bearer = format!("Bearer {TOKEN}");
    let with = |name: &str| {
        request(
            h,
            "GET",
            &format!("{B}{name}"),
            &[("Authorization", &bearer)],
        )
    };
    let key1 = with("aes.key");
    let seg1 = with("v0.0.m4s");
    assert_eq!((key1.status, seg1.status), (200, 200));
    std::fs::write(&file, "the second secret of the library").unwrap();
    let config = akit_config::from_json(&json, None).unwrap();
    let errs = a.roles.apply(&config);
    assert!(errs.is_empty(), "{errs:?}");
    let key2 = with("aes.key");
    let seg2 = with("v0.0.m4s");
    assert_eq!((key2.status, seg2.status), (200, 200));
    assert_ne!(key1.body, key2.body);
    assert_ne!(seg1.body, seg2.body);
    assert_ne!(seg1.header("etag"), seg2.header("etag"));
    assert_eq!(
        decrypt(&key1.body, 0, &seg1.body),
        decrypt(&key2.body, 0, &seg2.body),
        "the segment is another"
    );
    let keys = akit_media::library::lookup(akit_media::library::key(&a.engine), "lib")
        .and_then(|l| l.keys())
        .unwrap();
    assert_eq!(
        keys.key(akit_media::library::keys::KEY_ITEM, "films/bbb", 0)[..],
        key2.body[..],
        "the library's keys are not the handler's"
    );
}

/// fit's `TestEncryptProvision` (I147), its part that reads the disk: a `secret_file` that
/// cannot be read, or holds a secret too short, fails the library's apply.
#[test]
fn encrypt_secret_file_refused() {
    let lib = copy_lib();
    let short = lib.0.join("short");
    std::fs::write(&short, "short\n").unwrap();
    for (file, want) in [
        (lib.0.join("nonexistent"), "secret_file"),
        (short, "at least 16"),
    ] {
        let config = akit_config::from_json(&encrypted_config(&lib.0, &file), None).unwrap();
        let engine = Arc::new(Engine::builder().build().unwrap());
        let mut m = Manager::new(&[akit_media::library::MEDIA_LIBRARY], engine, None);
        let errs = m.apply(&config);
        assert!(
            errs.iter().any(|e| format!("{e:?}").contains(want)),
            "{errs:?}, want {want:?}"
        );
        m.stop(Duration::ZERO);
    }
}
