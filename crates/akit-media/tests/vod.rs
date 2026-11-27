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
