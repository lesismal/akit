//! The `static` handler through akit's listener on akio, with fit's `handler/static` tests
//! (`static_test.go`) ported: serving, traversal attempts, conditional and range requests, files
//! sent from disk, a changed file, the config's errors and what is not supported yet. Also: a
//! disk that hangs does not hold an I/O thread, a large file to a slow reader, and a client that
//! leaves midway. On the backend `AKIO_TEST_BACKEND` names (epoll by default, `uring`).
//!
//! Where akio-http's `Files` answers otherwise than fit's handler, a test checks akit's answer
//! and says which akio API request (docs/akio-api-requests.md) brings fit's.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing,
    clippy::arithmetic_side_effects,
    clippy::type_complexity
)]

use std::fs;
use std::io::{Read, Write};
use std::net::{SocketAddr, TcpStream};
use std::path::{Path, PathBuf};
use std::sync::OnceLock;
use std::time::{Duration, SystemTime};

use akio::{Backend, Engine, UringConfig};
use akit_runtime::{Env, GraphCell, HandlerParams, Registry, compile};

fn options() -> akit_listener::Options {
    let mut o = akit_listener::Options::default();
    o.backend = match std::env::var("AKIO_TEST_BACKEND").as_deref() {
        Ok("uring") => Backend::IoUring({
            let mut v = UringConfig::default();
            v.strict = true;
            v
        }),
        _ => Backend::Epoll,
    };
    o.shutdown_timeout = Duration::from_secs(5);
    o
}

fn registry() -> Registry<HandlerParams> {
    Registry::new(&[akit_handler::MODULES]).unwrap()
}

/// A server for `routes` (a JSON array) on one HTTP/1.1 listener; stops when dropped.
struct Server {
    engine: Engine,
    addr: SocketAddr,
}

impl Drop for Server {
    fn drop(&mut self) {
        self.engine.shutdown();
        self.engine.wait();
    }
}

fn serve_routes(routes: &str) -> Server {
    let json = format!(
        r#"{{"listeners": [{{"name": "web", "address": "127.0.0.1:0"}}], "routes": {routes}}}"#
    );
    let config = akit_config::from_json(&json, None).unwrap_or_else(|e| panic!("{json}: {e}"));
    let cell = GraphCell::new(compile(&config, &registry(), &Env::default()).unwrap());
    let engine = akit_listener::engine(&cell, &options()).unwrap();
    engine.start().unwrap();
    let addr = engine.local_addrs()[0];
    Server { engine, addr }
}

/// A server whose every path goes to a `static` handler for `root` with `extra` params.
fn serve(root: &Path, extra: &str) -> Server {
    serve_routes(&format!(
        r#"[{{"handler": {{"module": "static", "params": {{"root": {}{extra}}}}}}}]"#,
        quote(root)
    ))
}

fn quote(p: &Path) -> String {
    format!("{:?}", p.to_str().unwrap())
}

/// A response as the client saw it.
#[derive(Debug)]
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

    fn text(&self) -> &str {
        std::str::from_utf8(&self.body).unwrap()
    }
}

/// One request on a connection of its own, the response read to the connection's close.
fn request(addr: SocketAddr, method: &str, target: &str, headers: &[(&str, &str)]) -> Resp {
    let mut s = TcpStream::connect(addr).unwrap();
    s.set_read_timeout(Some(Duration::from_secs(10))).unwrap();
    let mut req = format!("{method} {target} HTTP/1.1\r\nHost: x\r\nConnection: close\r\n");
    for (n, v) in headers {
        req.push_str(&format!("{n}: {v}\r\n"));
    }
    req.push_str("\r\n");
    s.write_all(req.as_bytes()).unwrap();
    let mut raw = Vec::new();
    s.read_to_end(&mut raw).unwrap();
    parse(&raw)
}

fn parse(raw: &[u8]) -> Resp {
    let end = raw
        .windows(4)
        .position(|w| w == b"\r\n\r\n")
        .unwrap_or_else(|| panic!("no response head in {:?}", String::from_utf8_lossy(raw)));
    let head = std::str::from_utf8(&raw[..end]).unwrap();
    let mut lines = head.split("\r\n");
    let status = lines
        .next()
        .unwrap()
        .split(' ')
        .nth(1)
        .unwrap()
        .parse()
        .unwrap();
    let headers = lines
        .map(|l| {
            let (n, v) = l.split_once(':').unwrap();
            (n.to_owned(), v.trim().to_owned())
        })
        .collect();
    Resp {
        status,
        headers,
        body: raw[end + 4..].to_vec(),
    }
}

fn get(addr: SocketAddr, target: &str) -> Resp {
    request(addr, "GET", target, &[])
}

/// A fresh directory under the target's test directory.
fn dir(name: &str) -> PathBuf {
    let d = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join(format!("static-{name}"));
    let _ = fs::remove_dir_all(&d);
    fs::create_dir_all(&d).unwrap();
    d
}

/// Writes `files`, by slash-separated name, under a fresh directory, and returns it.
fn tree(name: &str, files: &[(&str, &str)]) -> PathBuf {
    let root = dir(name);
    for (name, data) in files {
        let full = root.join(name);
        fs::create_dir_all(full.parent().unwrap()).unwrap();
        fs::write(full, data).unwrap();
    }
    root
}

const TEXT: &str = "text/plain; charset=utf-8";
const HTML: &str = "text/html; charset=utf-8";

/// fit's TestServe.
#[test]
fn serve_files() {
    let root = tree(
        "serve",
        &[
            ("hello.txt", "hello\n"),
            ("style.css", "body{}"),
            ("noext", "<html><body>x</body></html>"),
            ("empty.txt", ""),
            ("docs/index.html", "<p>docs</p>"),
            ("docs/a b.txt", "spaced"),
            ("nested/deep/x.js", "1"),
            ("other/default.htm", "default"),
            ("plain/readme.txt", "no index here"),
        ],
    );
    let srv = serve(&root, r#", "index": ["index.html", "default.htm"]"#);
    // (method, target, status, content type, body, Location)
    let cases: &[(&str, &str, u16, &str, &str, &str)] = &[
        ("GET", "/hello.txt", 200, TEXT, "hello\n", ""),
        ("HEAD", "/hello.txt", 200, TEXT, "", ""),
        (
            "GET",
            "/style.css",
            200,
            "text/css; charset=utf-8",
            "body{}",
            "",
        ),
        ("GET", "/empty.txt", 200, TEXT, "", ""),
        (
            "GET",
            "/nested/deep/x.js",
            200,
            "text/javascript; charset=utf-8",
            "1",
            "",
        ),
        ("GET", "/docs/a%20b.txt", 200, TEXT, "spaced", ""),
        ("GET", "/docs/", 200, HTML, "<p>docs</p>", ""),
        ("GET", "/./hello.txt", 200, TEXT, "hello\n", ""),
        ("GET", "//hello.txt", 200, TEXT, "hello\n", ""),
        ("GET", "/other/", 200, HTML, "default", ""),
        (
            "GET",
            "/docs",
            301,
            TEXT,
            "301 Moved Permanently\n",
            "/docs/",
        ),
        (
            "GET",
            "/docs?a=1",
            301,
            TEXT,
            "301 Moved Permanently\n",
            "/docs/?a=1",
        ),
        ("GET", "/plain/", 404, TEXT, "404 Not Found\n", ""),
        ("GET", "/missing.txt", 404, TEXT, "404 Not Found\n", ""),
        ("GET", "/hello.txt/", 404, TEXT, "404 Not Found\n", ""),
        (
            "POST",
            "/hello.txt",
            405,
            TEXT,
            "405 Method Not Allowed\n",
            "",
        ),
        // Sniffed from the first bytes, as fit's.
        (
            "GET",
            "/noext",
            200,
            HTML,
            "<html><body>x</body></html>",
            "",
        ),
    ];
    for &(method, target, status, ty, body, location) in cases {
        // Twice: the second time from what Files keeps.
        for _ in 0..2 {
            let r = request(srv.addr, method, target, &[]);
            assert_eq!(
                (r.status, r.header("Content-Type"), r.text()),
                (status, Some(ty), body),
                "{method} {target}: {r:?}"
            );
            assert_eq!(r.header("Location").unwrap_or(""), location, "{target}");
            if status == 200 {
                assert!(
                    r.header("ETag").is_some_and(|t| t.starts_with('"'))
                        && r.header("Last-Modified").is_some()
                        && r.header("Accept-Ranges") == Some("bytes"),
                    "{method} {target}: validators missing: {r:?}"
                );
            }
            if method == "HEAD" {
                assert_eq!(r.header("Content-Length"), Some("6"), "{r:?}");
            }
            if status == 405 {
                assert_eq!(r.header("Allow"), Some("GET, HEAD"));
            }
        }
    }
}

/// fit's TestServe's other index file, one name; and fit's TestNoIndex.
#[test]
fn index_file() {
    let root = tree(
        "index",
        &[("other/default.htm", "default"), ("index.html", "i")],
    );
    let srv = serve(&root, r#", "index": ["default.htm"]"#);
    let r = get(srv.addr, "/other/");
    assert_eq!((r.status, r.text()), (200, "default"));
    assert_eq!(r.header("Content-Type"), Some(HTML));
    assert_eq!(get(srv.addr, "/").status, 404);
    drop(srv);
    let srv = serve(&root, r#", "index": []"#);
    assert_eq!(get(srv.addr, "/").status, 404);
    assert_eq!(get(srv.addr, "/other/default.htm").text(), "default");
}

/// fit's TestPermission: a file that may not be read is 403 (not for root, who reads all).
#[test]
fn permission() {
    use std::os::unix::fs::PermissionsExt;
    let root = tree("perm", &[("locked/a.txt", "a")]);
    let locked = root.join("locked");
    fs::set_permissions(&locked, fs::Permissions::from_mode(0o000)).unwrap();
    let readable = fs::metadata(locked.join("a.txt")).is_ok();
    let srv = serve(&root, "");
    let r = get(srv.addr, "/locked/a.txt");
    fs::set_permissions(&locked, fs::Permissions::from_mode(0o755)).unwrap();
    if !readable {
        assert_eq!((r.status, r.text()), (403, "403 Forbidden\n"));
    }
}

/// fit's TestCacheLimits: files above max_file_bytes go from disk, and at most max_cache_bytes
/// are kept in memory; every answer is whole either way.
#[test]
fn cache_limits() {
    let root = tree(
        "cache-limits",
        &[
            ("a.txt", "aaaa"),
            ("b.txt", "bbbb"),
            ("big.txt", "0123456789"),
        ],
    );
    let srv = serve(&root, r#", "max_file_bytes": 8, "max_cache_bytes": 6"#);
    for name in ["a.txt", "b.txt", "big.txt"] {
        for _ in 0..2 {
            let r = get(srv.addr, &format!("/{name}"));
            assert_eq!(r.status, 200, "{name}");
            assert!(!r.body.is_empty() && r.header("ETag").is_some());
            assert_eq!(r.header("Content-Type"), Some(TEXT), "{name}");
        }
    }
}

/// fit's TestDefaultType: default_type types a file whose extension gives none, from memory, from
/// disk and for a range; without it the type is sniffed.
#[test]
fn default_type() {
    let html = "<html><body>sniffed as text/html</body></html>";
    let mid = format!("{html}{}", "m".repeat(16 << 10));
    let root = tree(
        "default-type",
        &[("small", html), ("big", &mid), ("a.css", "body{}")],
    );
    const DT: &str = "application/x-default";
    for extra in [
        format!(r#", "default_type": "{DT}""#),
        format!(r#", "default_type": "{DT}", "max_file_bytes": 4"#),
    ] {
        let srv = serve(&root, &extra);
        for target in ["/small", "/big"] {
            for range in [None, Some("bytes=0-9")] {
                let headers: Vec<(&str, &str)> = range.map(|r| ("Range", r)).into_iter().collect();
                let r = request(srv.addr, "GET", target, &headers);
                assert!(
                    r.status == 200 || r.status == 206,
                    "{extra} {target}: {r:?}"
                );
                assert_eq!(r.header("Content-Type"), Some(DT), "{extra} {target}");
            }
        }
        assert_eq!(
            get(srv.addr, "/a.css").header("Content-Type"),
            Some("text/css; charset=utf-8")
        );
    }
    let srv = serve(&root, r#", "max_file_bytes": 4"#);
    for target in ["/small", "/big"] {
        assert_eq!(
            get(srv.addr, target).header("Content-Type"),
            Some(HTML),
            "{target}"
        );
    }
}

/// fit's TestTraversal, through akit's listener: what climbs above the root is refused by the
/// listener's canonical path (I11), the rest by the handler, all with 400; a `..` that stays
/// inside is removed by the listener and the canonical path served; dotfiles are 404 unless
/// `dotfiles` is `allow`.
#[test]
fn traversal() {
    let outer = dir("traversal");
    fs::write(outer.join("secret.txt"), "secret").unwrap();
    let root = outer.join("www");
    fs::create_dir(&root).unwrap();
    fs::write(root.join("ok.txt"), "ok").unwrap();
    fs::write(root.join("..dots"), "dots").unwrap();
    fs::create_dir(root.join(".git")).unwrap();
    fs::write(root.join(".git/config"), "git").unwrap();
    fs::create_dir(root.join(".well-known")).unwrap();
    fs::write(root.join(".well-known/security.txt"), "sec").unwrap();
    let srv = serve(&root, "");
    for target in [
        "/../secret.txt",
        "/a/../../secret.txt",
        "/%2e%2e/secret.txt",
        "/..%2fsecret.txt",
        "/..%5csecret.txt",
        "/ok.txt%00",
        "/..",
        "/www/../../secret.txt",
        "/%2e%2e%2fsecret.txt",
        "/ok.txt%5c..%5c..%5csecret.txt",
    ] {
        let r = get(srv.addr, target);
        assert!(
            r.status == 400 && !r.text().contains("secret"),
            "{target}: {r:?}"
        );
    }
    assert_eq!(get(srv.addr, "/ok.txt").text(), "ok");
    // A ".." segment that stays inside the root is removed by the listener, as fit's, and the
    // handler serves the canonical path.
    for target in ["/x/../ok.txt", "/x/%2e%2e/ok.txt", "//ok.txt"] {
        let r = get(srv.addr, target);
        assert_eq!((r.status, r.text()), (200, "ok"), "{target}");
    }
    // Dots that are not a whole segment make a dotfile, which only dotfiles allow serves.
    for target in ["/..dots", "/.git/config", "/%2egit/config"] {
        let r = get(srv.addr, target);
        assert_eq!((r.status, r.text()), (404, "404 Not Found\n"), "{target}");
    }
    assert_eq!(get(srv.addr, "/.well-known/security.txt").text(), "sec");
    drop(srv);
    let srv = serve(&root, r#", "dotfiles": "allow""#);
    for (target, body) in [("/..dots", "dots"), ("/.git/config", "git")] {
        let r = get(srv.addr, target);
        assert_eq!((r.status, r.text()), (200, body), "{target}");
    }
    assert_eq!(get(srv.addr, "/x/../ok.txt").text(), "ok");
}

/// I80b (the threat model's L7): symbolic links under the root are followed wherever they lead,
/// as fit's handler follows them ("symbolic links under the root are followed, wherever they
/// lead"; fit has no option to refuse them, nginx's `disable_symlinks` is off by default): a link
/// to a file or a directory outside the root is served. A request cannot name a path outside
/// the root itself; whoever can make links under it decides what it serves (the reference's
/// `static`).
#[cfg(unix)]
#[test]
fn symlinks_out_of_the_root_followed() {
    let outer = dir("symlinks");
    fs::write(outer.join("outside.txt"), "outside").unwrap();
    fs::create_dir(outer.join("elsewhere")).unwrap();
    fs::write(outer.join("elsewhere/inner.txt"), "inner").unwrap();
    let root = outer.join("www");
    fs::create_dir(&root).unwrap();
    std::os::unix::fs::symlink(outer.join("outside.txt"), root.join("link.txt")).unwrap();
    std::os::unix::fs::symlink(outer.join("elsewhere"), root.join("dir")).unwrap();
    std::os::unix::fs::symlink("../outside.txt", root.join("relative.txt")).unwrap();
    std::os::unix::fs::symlink(outer.join("missing"), root.join("dangling")).unwrap();
    let srv = serve(&root, "");
    for (target, body) in [
        ("/link.txt", "outside"),
        ("/dir/inner.txt", "inner"),
        ("/relative.txt", "outside"),
    ] {
        let r = get(srv.addr, target);
        assert_eq!((r.status, r.text()), (200, body), "{target}");
    }
    assert_eq!(get(srv.addr, "/dangling").status, 404);
    assert_eq!(get(srv.addr, "/../outside.txt").status, 400);
}

/// fit's TestConditional: from memory, and from disk (`max_file_bytes` 4).
#[test]
fn conditional_and_ranges() {
    let root = tree(
        "conditional",
        &[("a.txt", "0123456789"), ("b.txt", &"0123456789".repeat(30))],
    );
    // 2026-01-02 03:04:05 UTC.
    let mtime = SystemTime::UNIX_EPOCH + Duration::from_secs(1_767_323_045);
    fs::File::options()
        .write(true)
        .open(root.join("a.txt"))
        .unwrap()
        .set_modified(mtime)
        .unwrap();
    for extra in ["", r#", "max_file_bytes": 4"#] {
        let srv = serve(&root, extra);
        let first = get(srv.addr, "/a.txt");
        let tag = first.header("ETag").unwrap().to_owned();
        let lm = first.header("Last-Modified").unwrap().to_owned();
        assert!(tag.starts_with('"'), "{tag}");
        assert_eq!(lm, "Fri, 02 Jan 2026 03:04:05 GMT");
        let other_then_tag = format!(r#""x", {tag}"#);
        let cases: &[(&str, &[(&str, &str)], u16, &str)] = &[
            ("if-none-match hit", &[("If-None-Match", &tag)], 304, ""),
            (
                "if-none-match list",
                &[("If-None-Match", &other_then_tag)],
                304,
                "",
            ),
            ("if-none-match star", &[("If-None-Match", "*")], 304, ""),
            (
                "if-none-match miss",
                &[("If-None-Match", r#""other""#)],
                200,
                "0123456789",
            ),
            (
                "if-modified-since same",
                &[("If-Modified-Since", &lm)],
                304,
                "",
            ),
            (
                "if-modified-since later",
                &[("If-Modified-Since", "Sat, 03 Jan 2026 00:00:00 GMT")],
                304,
                "",
            ),
            (
                "if-modified-since earlier",
                &[("If-Modified-Since", "Thu, 01 Jan 2026 00:00:00 GMT")],
                200,
                "0123456789",
            ),
            (
                "if-none-match wins",
                &[("If-None-Match", r#""other""#), ("If-Modified-Since", &lm)],
                200,
                "0123456789",
            ),
            ("if-match miss", &[("If-Match", r#""other""#)], 412, ""),
            ("if-match hit", &[("If-Match", &tag)], 200, "0123456789"),
            (
                "if-unmodified-since earlier",
                &[("If-Unmodified-Since", "Thu, 01 Jan 2026 00:00:00 GMT")],
                412,
                "",
            ),
            ("range", &[("Range", "bytes=2-4")], 206, "234"),
            ("range suffix", &[("Range", "bytes=-3")], 206, "789"),
            ("range open", &[("Range", "bytes=7-")], 206, "789"),
            ("range unsatisfiable", &[("Range", "bytes=20-30")], 416, ""),
            (
                "if-range etag hit",
                &[("Range", "bytes=0-1"), ("If-Range", &tag)],
                206,
                "01",
            ),
            (
                "if-range etag miss",
                &[("Range", "bytes=0-1"), ("If-Range", r#""other""#)],
                200,
                "0123456789",
            ),
            (
                "if-range date hit",
                &[("Range", "bytes=0-1"), ("If-Range", &lm)],
                206,
                "01",
            ),
        ];
        for (name, headers, status, body) in cases {
            let r = request(srv.addr, "GET", "/a.txt", headers);
            assert_eq!(
                (r.status, r.text()),
                (*status, *body),
                "{name}{extra}: {r:?}"
            );
            if *status == 206 {
                assert!(r.header("Content-Range").is_some(), "{name}{extra}");
            }
            if *status == 304 {
                assert_eq!(r.header("ETag"), Some(tag.as_str()), "{name}{extra}");
            }
            if *status == 416 {
                assert_eq!(
                    r.header("Content-Range"),
                    Some("bytes */10"),
                    "{name}{extra}"
                );
            }
        }
        // Several ranges: a multipart body. fit sends a file from disk whole instead; Files
        // reads the parts on its file pool. Files merges ranges less than 80 bytes apart (RFC
        // 9110 section 14.2), which fit does not: bytes=0-1,5-6 is one part, 0-6.
        let r = request(srv.addr, "GET", "/a.txt", &[("Range", "bytes=0-1,5-6")]);
        assert_eq!(
            (r.status, r.text()),
            (206, "0123456"),
            "merged ranges{extra}"
        );
        let multi = request(srv.addr, "GET", "/b.txt", &[("Range", "bytes=0-1,200-201")]);
        assert_eq!(multi.status, 206, "multiple ranges{extra}");
        assert!(
            multi
                .header("Content-Type")
                .unwrap()
                .starts_with("multipart/byteranges"),
            "{multi:?}"
        );
        let body = multi.text();
        assert!(
            body.contains("Content-Range: bytes 0-1/300\r\n\r\n01\r\n")
                && body.contains("Content-Range: bytes 200-201/300\r\n\r\n01\r\n"),
            "{body}"
        );
        // HEAD announces the length.
        let head = request(srv.addr, "HEAD", "/a.txt", &[]);
        assert_eq!(
            (head.status, head.header("Content-Length")),
            (200, Some("10"))
        );
        assert!(head.body.is_empty());
    }
}

/// fit's TestLargeFromDisk: a file larger than max_file_bytes, sent from disk, and a compressed
/// twin sent from disk with its length.
#[test]
fn large_from_disk() {
    let big: String = "0123456789".repeat(16 * 1024 / 10 + 1);
    let root = tree(
        "large",
        &[
            ("big.txt", &big),
            ("twin.txt", "plain"),
            ("twin.txt.gz", "zz"),
        ],
    );
    let srv = serve(&root, r#", "max_file_bytes": 4"#);
    let whole = get(srv.addr, "/big.txt");
    let tag = whole.header("ETag").unwrap().to_owned();
    assert_eq!((whole.status, whole.text()), (200, big.as_str()));
    let head = request(srv.addr, "HEAD", "/big.txt", &[]);
    assert!(head.status == 200 && head.body.is_empty(), "{head:?}");
    assert_eq!(
        head.header("Content-Length"),
        Some(big.len().to_string().as_str())
    );
    let r = request(srv.addr, "GET", "/big.txt", &[("If-None-Match", &tag)]);
    assert!(r.status == 304 && r.body.is_empty(), "{r:?}");
    let r = request(srv.addr, "GET", "/big.txt", &[("Range", "bytes=999999-")]);
    assert_eq!(r.status, 416);
    let r = request(srv.addr, "GET", "/big.txt", &[("Range", "bytes=10-19")]);
    assert_eq!((r.status, r.text()), (206, "0123456789"));
    // A compressed twin only for an encoding precompressed lists: none when left out.
    let r = request(srv.addr, "GET", "/twin.txt", &[("Accept-Encoding", "gzip")]);
    assert_eq!((r.text(), r.header("Content-Encoding")), ("plain", None));
    assert_eq!(get(srv.addr, "/twin.txt").text(), "plain");

    // A twin larger than max_file_bytes and 16 KiB, with precompressed: from disk, its length
    // the twin's.
    let twin = "z".repeat(16 * 1024 + 3);
    let root = tree("large-twin", &[("big.txt", &big), ("big.txt.gz", &twin)]);
    let srv = serve(&root, r#", "max_file_bytes": 4, "precompressed": ["gzip"]"#);
    let r = request(srv.addr, "GET", "/big.txt", &[("Accept-Encoding", "gzip")]);
    assert!(r.status == 200 && r.text() == twin, "{}", r.status);
    assert_eq!(r.header("Content-Encoding"), Some("gzip"));
    assert_eq!(
        r.header("Content-Length"),
        Some(twin.len().to_string().as_str())
    );
    let head = request(srv.addr, "HEAD", "/big.txt", &[("Accept-Encoding", "gzip")]);
    assert!(head.status == 200 && head.body.is_empty(), "{head:?}");
    assert_eq!(
        head.header("Content-Length"),
        Some(twin.len().to_string().as_str())
    );
    assert_eq!(get(srv.addr, "/big.txt").text(), big);
}

/// fit's TestPrecompressed: the twin a client gets for its `Accept-Encoding` (by q value, among
/// the encodings `precompressed` lists, in its order among equals), from memory and from disk,
/// its validators and ranges.
#[test]
fn precompressed() {
    let files = [
        ("app.js", "identity-js"),
        ("app.js.br", "br-js"),
        ("app.js.zst", "zstd-js"),
        ("app.js.gz", "gzip-js"),
        ("style.css", "identity-css"),
        ("style.css.gz", "gzip-css-0123456789"),
        ("plain.txt", "plain"),
    ];
    let root = tree("precompressed", &files);
    let srv = serve(&root, r#", "precompressed": ["br", "zstd", "gzip"]"#);
    // (target, Accept-Encoding, encoding and body.)
    let cases: [(&str, &str, &str, &str); 11] = [
        // Equal q: the first listed.
        ("/app.js", "gzip, br", "br", "br-js"),
        // Ranked by q.
        ("/app.js", "br;q=0.5, gzip", "gzip", "gzip-js"),
        ("/app.js", "zstd", "zstd", "zstd-js"),
        // q=0 refuses a coding.
        ("/app.js", "br;q=0, identity", "", "identity-js"),
        // "*" stands for the codings not named.
        ("/app.js", "*", "br", "br-js"),
        ("/app.js", "*, br;q=0", "zstd", "zstd-js"),
        ("/app.js", "", "", "identity-js"),
        // Codings are named in any case, with blanks around the parameters.
        ("/app.js", "GZIP", "gzip", "gzip-js"),
        ("/app.js", "gzip ; q=0.8", "gzip", "gzip-js"),
        // No br twin.
        ("/style.css", "br, gzip", "gzip", "gzip-css-0123456789"),
        ("/plain.txt", "br, gzip", "", "plain"),
    ];
    for (target, accept, enc, body) in cases {
        let headers: &[(&str, &str)] = if accept.is_empty() {
            &[]
        } else {
            &[("Accept-Encoding", accept)]
        };
        let r = request(srv.addr, "GET", target, headers);
        assert_eq!(r.status, 200, "{target} {accept:?}");
        assert_eq!(
            (r.header("Content-Encoding").unwrap_or(""), r.text()),
            (enc, body),
            "{target} {accept:?}"
        );
        assert_eq!(r.header("Vary"), Some("Accept-Encoding"), "{target}");
        if target == "/app.js" {
            assert_eq!(
                r.header("Content-Type"),
                Some("text/javascript; charset=utf-8"),
                "{accept:?}"
            );
        }
    }

    // Each representation has its own ETag, and conditions apply to it.
    let identity = get(srv.addr, "/app.js").header("ETag").unwrap().to_owned();
    let br = request(srv.addr, "GET", "/app.js", &[("Accept-Encoding", "br")]);
    let br = br.header("ETag").unwrap().to_owned();
    assert_ne!(identity, br);
    let r = request(
        srv.addr,
        "GET",
        "/app.js",
        &[("Accept-Encoding", "br"), ("If-None-Match", &br)],
    );
    assert_eq!(r.status, 304);
    // The identity's tag does not match the twin.
    let r = request(
        srv.addr,
        "GET",
        "/app.js",
        &[("Accept-Encoding", "br"), ("If-None-Match", &identity)],
    );
    assert_eq!((r.status, r.text()), (200, "br-js"));
    let r = request(
        srv.addr,
        "GET",
        "/style.css",
        &[("Accept-Encoding", "gzip"), ("Range", "bytes=0-3")],
    );
    assert_eq!(
        (r.status, r.text(), r.header("Content-Encoding")),
        (206, "gzip", Some("gzip"))
    );
    let head = request(srv.addr, "HEAD", "/app.js", &[("Accept-Encoding", "zstd")]);
    assert_eq!(
        (
            head.status,
            head.header("Content-Encoding"),
            head.header("Content-Length")
        ),
        (200, Some("zstd"), Some("7"))
    );
    drop(srv);

    // Twins too large to keep come from disk, with their length and their original's type.
    let srv = serve(&root, r#", "precompressed": ["gzip"], "max_file_bytes": 4"#);
    let r = request(
        srv.addr,
        "GET",
        "/style.css",
        &[("Accept-Encoding", "gzip")],
    );
    assert_eq!(
        (r.status, r.text(), r.header("Content-Encoding")),
        (200, "gzip-css-0123456789", Some("gzip"))
    );
    assert_eq!(r.header("Content-Type"), Some("text/css; charset=utf-8"));
    assert_eq!(r.header("Vary"), Some("Accept-Encoding"));
    assert_eq!(r.header("Content-Length"), Some("19"));
    // precompressed lists gzip alone: no br twin.
    let r = request(
        srv.addr,
        "GET",
        "/app.js",
        &[("Accept-Encoding", "br, gzip")],
    );
    assert_eq!(
        (r.header("Content-Encoding"), r.text()),
        (Some("gzip"), "gzip-js")
    );
    drop(srv);

    // Without precompressed, twins are files like any other, and there is no Vary.
    let srv = serve(&root, "");
    let r = request(srv.addr, "GET", "/app.js", &[("Accept-Encoding", "br")]);
    assert_eq!(
        (r.header("Content-Encoding"), r.text(), r.header("Vary")),
        (None, "identity-js", None)
    );
    // The twin itself, asked for by its name, is a file.
    let r = get(srv.addr, "/app.js.gz");
    assert_eq!((r.status, r.text()), (200, "gzip-js"));
}

/// fit's TestTryFiles and TestTryFilesUnsafe: candidates tried in turn, a directory served by
/// its index and not redirected, a single-page application's fallback, and a listing reached
/// through a candidate.
#[test]
fn try_files() {
    let root = tree(
        "try-files",
        &[
            ("index.html", "app"),
            ("assets/a.js", "js"),
            ("docs/index.html", "docs"),
            ("plain/x.txt", "x"),
        ],
    );
    let srv = serve(
        &root,
        r#", "try_files": ["{path}", "{path}/", "/index.html"]"#,
    );
    for (target, status, body) in [
        ("/assets/a.js", 200, "js"),
        // A directory, served by its index, not redirected.
        ("/docs", 200, "docs"),
        ("/docs/", 200, "docs"),
        ("/some/client/route", 200, "app"),
        // No index: falls through.
        ("/plain/", 200, "app"),
        ("/", 200, "app"),
        // akio's parser refuses the path before the handler sees it, with its own text.
        ("/../secret", 400, ""),
    ] {
        // Twice: the second time from what Files keeps of the resolution.
        for _ in 0..2 {
            let r = get(srv.addr, target);
            assert_eq!(r.status, status, "{target}");
            if status == 200 {
                assert_eq!(r.text(), body, "{target}");
            }
            assert_eq!(r.header("Location"), None, "{target}");
        }
    }
    drop(srv);
    let srv = serve(&root, r#", "try_files": ["{path}", "/missing.html"]"#);
    assert_eq!(get(srv.addr, "/nope").status, 404);
    drop(srv);
    // Browse applies to a directory try_files reaches.
    let srv = serve(&root, r#", "try_files": ["{path}/"], "browse": true"#);
    let r = get(srv.addr, "/plain");
    assert!(r.status == 200 && r.text().contains("x.txt"), "{r:?}");
    assert!(
        r.text().contains("<title>Index of /plain/</title>"),
        "{r:?}"
    );
    drop(srv);
    // An empty list finds nothing, as fit's.
    let srv = serve(&root, r#", "try_files": []"#);
    assert_eq!(get(srv.addr, "/assets/a.js").status, 404);
    drop(srv);
    // A candidate outside the root is passed over.
    let srv = serve(&root, r#", "try_files": ["/../index.html", "{path}"]"#);
    let r = get(srv.addr, "/assets/a.js");
    assert_eq!((r.status, r.text()), (200, "js"));
    drop(srv);
    // With dotfiles deny, a candidate a dotfile segment names is passed over: {path} stands for
    // the path asked for, which may be /.well-known/... at the root (served) but not below it.
    let root = tree(
        "try-files-dot",
        &[("a/.well-known/x", "hidden"), ("fallback.txt", "fb")],
    );
    let srv = serve(&root, r#", "try_files": ["/a{path}", "/fallback.txt"]"#);
    let r = get(srv.addr, "/.well-known/x");
    assert_eq!((r.status, r.text()), (200, "fb"));
    fs::remove_dir_all(&root).unwrap();
}

/// fit's TestBrowse, TestBrowseMaxEntries and TestBrowseOrder: the listing of a directory
/// without an index file, escaped, without dotfiles, directories first, bounded.
#[test]
fn browse() {
    let root = tree(
        "browse",
        &[
            ("files/b.txt", "bb"),
            ("files/a&b<c>.txt", "a"),
            ("files/q?x.txt", "q"),
            ("files/.hidden", "h"),
            ("files/sub/z", "z"),
        ],
    );
    let srv = serve(&root, r#", "browse": true"#);
    let r = get(srv.addr, "/files/");
    assert_eq!((r.status, r.header("Content-Type")), (200, Some(HTML)));
    let body = r.text();
    for want in [
        "<title>Index of /files/</title>",
        r#"<a href="../">../</a>"#,
        r#"<a href="./sub/">sub/</a>"#,
        r#"<a href="./a&amp;b%3Cc%3E.txt">a&amp;b&lt;c&gt;.txt</a>"#,
        r#"<a href="./q%3Fx.txt">q?x.txt</a>"#,
        r#"<a href="./b.txt">b.txt</a></td><td>2</td>"#,
    ] {
        assert!(body.contains(want), "the listing lacks {want}:\n{body}");
    }
    assert!(!body.contains("hidden"), "a dotfile is listed:\n{body}");
    assert!(
        body.find("sub/") < body.find("a&amp;b"),
        "directories do not come first:\n{body}"
    );
    drop(srv);
    let srv = serve(&root, "");
    assert_eq!(get(srv.addr, "/files/").status, 404);
    drop(srv);

    let names: Vec<String> = (0..20).map(|i| format!("d/f{i:02}.txt")).collect();
    let files: Vec<(&str, &str)> = names.iter().map(|n| (n.as_str(), "x")).collect();
    let root = tree("browse-max", &files);
    let srv = serve(&root, r#", "browse": true, "browse_max_entries": 5"#);
    let r = get(srv.addr, "/d/");
    assert_eq!(r.status, 200);
    assert_eq!(r.text().matches(r#"<tr><td><a href="./f"#).count(), 5);
    assert!(
        r.text().contains("Only the first 5 entries"),
        "{}",
        r.text()
    );
    drop(srv);
    let srv = serve(&root, r#", "browse": true"#);
    let r = get(srv.addr, "/d/");
    assert_eq!(r.text().matches(r#"<tr><td><a href="./f"#).count(), 20);
    assert!(!r.text().contains("Only the first"), "{}", r.text());
    drop(srv);

    let mut names = Vec::new();
    for i in 0..6 {
        names.push(format!("f{i}"));
        names.push(format!("d{i}/x"));
    }
    let files: Vec<(&str, &str)> = names.iter().map(|n| (n.as_str(), "x")).collect();
    let root = tree("browse-order", &files);
    let srv = serve(&root, r#", "browse": true, "check_interval": "1h""#);
    for _ in 0..2 {
        let r = get(srv.addr, "/");
        let body = r.text();
        assert_eq!(r.status, 200);
        assert!(body.rfind("d5/") < body.find(">f0<"), "{body}");
        // The root has no ../ link.
        assert!(!body.contains(r#"href="../""#), "{body}");
    }
}

/// fit's TestStripPrefix: the prefix cut before the path is looked up; a path without it is 404.
#[test]
fn strip_prefix() {
    let root = tree("strip", &[("a.txt", "a"), ("sub/index.html", "i")]);
    let srv = serve(&root, r#", "strip_prefix": "/assets""#);
    for (target, status, body) in [
        ("/assets/a.txt", 200, "a"),
        // The root has no index.html.
        ("/assets/", 404, ""),
        // Redirected to its slash, the prefix included.
        ("/assets/sub", 301, ""),
        ("/assets/sub/", 200, "i"),
        ("/a.txt", 404, ""),
        // fit's handler alone answers 400; through the listener, as through fit's, the path is
        // /a.txt, which lacks the prefix.
        ("/assets/../a.txt", 404, ""),
        ("/assets/sub/../a.txt", 200, "a"),
        // The prefix is cut as written, not at a segment's end, as fit's CutPrefix.
        ("/assetsa.txt", 200, "a"),
    ] {
        let r = get(srv.addr, target);
        assert_eq!(r.status, status, "{target}: {r:?}");
        if status == 200 {
            assert_eq!(r.text(), body, "{target}");
        }
    }
    assert_eq!(
        get(srv.addr, "/assets/sub").header("Location"),
        Some("/assets/sub/")
    );
    drop(srv);
    // A listing below a prefix: titled by the request's whole path, with a ../ link at the
    // prefix, as fit's.
    let srv = serve(
        &root,
        r#", "strip_prefix": "/assets", "browse": true, "index": ["none.html"]"#,
    );
    let r = get(srv.addr, "/assets/");
    assert!(
        r.status == 200
            && r.text().contains("<title>Index of /assets/</title>")
            && r.text().contains(r#"href="../""#),
        "{r:?}"
    );
}

/// A twin of a file that changes or goes, with a check_interval of 0: each request sees the
/// disk as it is (fit's TestWatch, for the twins).
#[test]
fn precompressed_follows_the_disk() {
    let root = tree("twin-changes", &[("a.txt", "plain"), ("a.txt.gz", "one")]);
    let srv = serve(
        &root,
        r#", "precompressed": ["gzip"], "check_interval": "0s""#,
    );
    let gz = |srv: &Server| request(srv.addr, "GET", "/a.txt", &[("Accept-Encoding", "gzip")]);
    assert_eq!(gz(&srv).text(), "one");
    fs::write(root.join("a.txt.gz"), "three").unwrap();
    assert_eq!(gz(&srv).text(), "three");
    fs::remove_file(root.join("a.txt.gz")).unwrap();
    let r = gz(&srv);
    assert_eq!((r.text(), r.header("Content-Encoding")), ("plain", None));
    fs::write(root.join("a.txt.br"), "brr").unwrap();
    let r = request(srv.addr, "GET", "/a.txt", &[("Accept-Encoding", "br")]);
    // No br twin when precompressed lists gzip alone.
    assert_eq!((r.text(), r.header("Content-Encoding")), ("plain", None));
}

/// fit's TestChangedFile: with a check_interval of 0 every request looks at the disk.
#[test]
fn changed_file() {
    let root = tree("changed", &[("a.txt", "one")]);
    let srv = serve(&root, r#", "check_interval": "0s""#);
    let first = get(srv.addr, "/a.txt");
    assert_eq!(first.text(), "one");
    let full = root.join("a.txt");
    fs::write(&full, "three").unwrap();
    fs::File::options()
        .write(true)
        .open(&full)
        .unwrap()
        .set_modified(SystemTime::now() + Duration::from_secs(3600))
        .unwrap();
    let second = get(srv.addr, "/a.txt");
    assert_eq!(second.text(), "three");
    assert_ne!(second.header("ETag"), first.header("ETag"));
    fs::remove_file(&full).unwrap();
    assert_eq!(get(srv.addr, "/a.txt").status, 404);
}

/// fit's TestErrors, and what fit takes that akit refuses as not supported yet.
#[test]
fn config_errors() {
    let root = tree("errors", &[("file.txt", "x")]);
    let compile_err = |params: &str| -> String {
        let json = format!(
            r#"{{"listeners": [{{"name": "web", "address": "127.0.0.1:0"}}], "routes": [{{"handler": {{"module": "static", "params": {params}}}}}]}}"#
        );
        match akit_config::from_json(&json, None) {
            Err(e) => e.to_string(),
            Ok(c) => match compile(&c, &registry(), &Env::default()) {
                Err(e) => e.to_string(),
                Ok(_) => panic!("{params} was taken"),
            },
        }
    };
    let r = quote(&root);
    let cases = [
        (
            "{}".to_owned(),
            "routes[0].handler.params.root: missing required field",
        ),
        (
            format!(r#"{{"root": {}}}"#, quote(&root.join("file.txt"))),
            "is not a directory",
        ),
        (
            format!(r#"{{"root": {}}}"#, quote(&root.join("none"))),
            "routes[0].handler.params.root",
        ),
        (
            format!(r#"{{"root": {r}, "index": ["a/b"]}}"#),
            r#"routes[0].handler.params.index[0]: "a/b" is not a file name"#,
        ),
        (
            format!(r#"{{"root": {r}, "index": [".."]}}"#),
            r#"".." is not a file name"#,
        ),
        (
            format!(r#"{{"root": {r}, "index": [".hidden"]}}"#),
            "starts with a dot, which dotfiles deny hides",
        ),
        (
            format!(r#"{{"root": {r}, "max_file_bytes": -1}}"#),
            "routes[0].handler.params.max_file_bytes: -1 is less than the minimum 0",
        ),
        (
            format!(r#"{{"root": {r}, "dotfiles": "maybe"}}"#),
            r#""maybe" is not one of "deny", "allow""#,
        ),
        (
            format!(r#"{{"root": {r}, "check_interval": "soon"}}"#),
            "is not a duration",
        ),
        // fit's TestFeatureErrors.
        (
            format!(r#"{{"root": {r}, "precompressed": ["deflate"]}}"#),
            r#"routes[0].handler.params.precompressed[0]: "deflate" is not one of "br", "zstd", "gzip""#,
        ),
        (
            format!(r#"{{"root": {r}, "precompressed": "gzip"}}"#),
            "routes[0].handler.params.precompressed: expected an array",
        ),
        (
            format!(r#"{{"root": {r}, "try_files": ["{{path}}", "index.html"]}}"#),
            r#"routes[0].handler.params.try_files[1]: "index.html" starts with neither / nor {path}"#,
        ),
        // fit's TestDotfilesConfig.
        (
            format!(r#"{{"root": {r}, "try_files": ["{{path}}", "/.app/index.html"]}}"#),
            r#"routes[0].handler.params.try_files[1]: "/.app/index.html" names a path"#,
        ),
        (
            format!(r#"{{"root": {r}, "try_files": ["{{path}}/.htaccess"]}}"#),
            "routes[0].handler.params.try_files[0]",
        ),
        // fit's TestStripPrefix's check.
        (
            format!(r#"{{"root": {r}, "strip_prefix": "assets"}}"#),
            r#"routes[0].handler.params.strip_prefix: "assets" does not start with /"#,
        ),
        (
            format!(r#"{{"root": {r}, "browse": "yes"}}"#),
            "routes[0].handler.params.browse: expected a boolean, got a string",
        ),
        (
            format!(r#"{{"root": {r}, "browse_max_entries": -1}}"#),
            "routes[0].handler.params.browse_max_entries: -1 is less than the minimum 0",
        ),
        (
            format!(r#"{{"root": {r}, "compress": ["gzip", "gzip"]}}"#),
            "routes[0].handler.params.compress[1]: \"gzip\" is listed twice",
        ),
        (
            format!(r#"{{"root": {r}, "compress": ["deflate"]}}"#),
            "routes[0].handler.params.compress[0]: \"deflate\" is not one of",
        ),
    ];
    for (params, want) in &cases {
        let err = compile_err(params);
        assert!(err.contains(want), "{params}: {err}, want {want}");
    }
    assert!(
        compile_err(&format!(r#"{{"root": {}}}"#, quote(&root.join("none")))).contains("os error")
    );
    // fit's TestDefaultType's refusal.
    let err = compile_err(&format!(r#"{{"root": {r}, "default_type": "not a type"}}"#));
    assert!(
        err.contains(r#"routes[0].handler.params.default_type: "not a type" is not a media type"#),
        "{err}"
    );
    let err = compile_err(&format!(r#"{{"root": {r}, "max_cache_bytes": -1}}"#));
    assert!(
        err.contains("routes[0].handler.params.max_cache_bytes: -1 is less than the minimum 0"),
        "{err}"
    );
    {
        let (params, field) = (format!(r#"{{"root": {r}, "disk_io": "auto"}}"#), "disk_io");
        let json = format!(
            r#"{{"listeners": [{{"name": "web", "address": ":0"}}], "routes": [{{"handler": {{"module": "static", "params": {params}}}}}]}}"#
        );
        let e = akit_config::from_json(&json, None).expect_err(&params);
        assert!(e.only_not_supported_yet(), "{params}: {e}");
        assert!(
            e.to_string()
                .contains(&format!("routes[0].handler.params.{field}")),
            "{params}: {e}"
        );
    }
    for params in [
        format!(r#"{{"root": {r}, "index": [".hidden"], "dotfiles": "allow"}}"#),
        format!(r#"{{"root": {r}, "index": []}}"#),
        format!(r#"{{"root": {r}, "index": ["a", "b"]}}"#),
        format!(r#"{{"root": {r}, "max_cache_bytes": 6, "default_type": "a/b; q=1"}}"#),
        format!(r#"{{"root": {r}, "max_cache_bytes": 0}}"#),
        format!(
            r#"{{"root": {r}, "disk_io": "pool", "check_interval": "1s", "max_file_bytes": 0}}"#
        ),
        format!(r#"{{"root": {r}, "precompressed": ["br", "zstd", "gzip"]}}"#),
        format!(r#"{{"root": {r}, "precompressed": []}}"#),
        format!(r#"{{"root": {r}, "browse": false, "browse_max_entries": 5}}"#),
        format!(r#"{{"root": {r}, "browse": true, "browse_max_entries": 0}}"#),
        format!(r#"{{"root": {r}, "strip_prefix": "/assets"}}"#),
        format!(r#"{{"root": {r}, "try_files": ["{{path}}", "{{path}}/", "/index.html"]}}"#),
        format!(r#"{{"root": {r}, "try_files": []}}"#),
        // A dotfile candidate is taken with dotfiles allow, and .well-known with deny (fit's
        // TestDotfilesConfig).
        format!(r#"{{"root": {r}, "try_files": ["{{path}}.html", "/.well-known/fallback"]}}"#),
        format!(
            r#"{{"root": {r}, "dotfiles": "allow", "try_files": ["{{path}}", "/.app/index.html"]}}"#
        ),
        // A candidate outside the root is passed over at request time (try_files_unsafe).
        format!(r#"{{"root": {r}, "try_files": ["/../a", "{{path}}"]}}"#),
    ] {
        let json = format!(
            r#"{{"listeners": [{{"name": "web", "address": ":0"}}], "routes": [{{"handler": {{"module": "static", "params": {params}}}}}]}}"#
        );
        let c = akit_config::from_json(&json, None).unwrap_or_else(|e| panic!("{params}: {e}"));
        compile(&c, &registry(), &Env::default()).unwrap();
    }
}

/// The two routes fit's bench config has, on one root (its `/srv/www`): a route's handler
/// looks the whole request path up under the root.
#[test]
fn routes_share_a_root() {
    let root = tree(
        "routes",
        &[("static/small.html", "<p>s</p>"), ("site/a.css", "a{}")],
    );
    let r = quote(&root);
    let srv = serve_routes(&format!(
        r#"[
          {{"name": "static", "match": {{"path": ["/static/*"]}},
            "handler": {{"module": "static", "params": {{"root": {r}}}}}}},
          {{"name": "site", "match": {{"path": ["/site/*"]}},
            "handler": {{"module": "static", "params": {{"root": {r}}}}}}}
        ]"#
    ));
    assert_eq!(get(srv.addr, "/static/small.html").text(), "<p>s</p>");
    assert_eq!(get(srv.addr, "/site/a.css").text(), "a{}");
    // No route: the listener's 404.
    assert_eq!(get(srv.addr, "/other").text(), "404 page not found\n");
}

/// What is not a regular file (a FIFO no one writes to, which would hold a thread of the file
/// pool until a writer came) is not opened: answered 404 at once, as fit's handler answers it,
/// while known files are served as ever. (A request the file pool does not answer within fit's
/// 10 s is answered 503: akio's tests of `Files::disk_timeout`, which needs a disk that hangs.)
#[test]
fn a_hung_disk_holds_no_io_thread() {
    let root = tree("hung", &[("hello.txt", "hello\n")]);
    let fifo = root.join("fifo");
    let ok = std::process::Command::new("mkfifo")
        .arg(&fifo)
        .status()
        .unwrap()
        .success();
    assert!(ok, "mkfifo");
    let srv = serve(&root, "");
    assert_eq!(get(srv.addr, "/hello.txt").text(), "hello\n");
    let started = std::time::Instant::now();
    for _ in 0..16 {
        let mut s = TcpStream::connect(srv.addr).unwrap();
        s.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
        s.write_all(b"GET /fifo HTTP/1.1\r\nHost: x\r\nConnection: close\r\n\r\n")
            .unwrap();
        let mut raw = Vec::new();
        s.read_to_end(&mut raw).expect("a FIFO is not waited on");
        let r = parse(&raw);
        assert_eq!((r.status, r.text()), (404, "404 Not Found\n"));
        assert_eq!(get(srv.addr, "/hello.txt").text(), "hello\n");
    }
    assert!(started.elapsed() < Duration::from_secs(5));
}

/// A file larger than max_file_bytes, to a client that reads slowly, arrives whole; a client
/// that leaves midway leaves the server serving.
#[test]
fn large_file_slow_reader_and_cut_connection() {
    static BIG: OnceLock<Vec<u8>> = OnceLock::new();
    let big = BIG.get_or_init(|| (0..4u32 << 20).map(|i| (i % 251) as u8).collect());
    let root = dir("slow");
    fs::write(root.join("big.bin"), big).unwrap();
    let srv = serve(&root, "");
    // Cut midway, several times.
    for _ in 0..4 {
        let mut s = TcpStream::connect(srv.addr).unwrap();
        s.write_all(b"GET /big.bin HTTP/1.1\r\nHost: x\r\n\r\n")
            .unwrap();
        let mut buf = [0u8; 4096];
        s.read_exact(&mut buf).unwrap();
        drop(s);
    }
    // A slow reader.
    let mut s = TcpStream::connect(srv.addr).unwrap();
    s.set_read_timeout(Some(Duration::from_secs(10))).unwrap();
    s.write_all(b"GET /big.bin HTTP/1.1\r\nHost: x\r\nConnection: close\r\n\r\n")
        .unwrap();
    let mut raw = Vec::new();
    let mut buf = vec![0u8; 256 << 10];
    loop {
        let n = s.read(&mut buf).unwrap();
        if n == 0 {
            break;
        }
        raw.extend_from_slice(&buf[..n]);
        if raw.len() < (1 << 20) {
            std::thread::sleep(Duration::from_millis(20));
        }
    }
    let r = parse(&raw);
    assert_eq!(r.status, 200);
    assert_eq!(r.header("Content-Type"), Some("application/octet-stream"));
    assert!(r.body == *big, "{} bytes", r.body.len());
    assert_eq!(get(srv.addr, "/missing").status, 404);
}

/// Pipelined requests on one connection, some answered from what Files knows and some from its
/// file pool, are answered in order.
#[test]
fn pipelined_in_order() {
    let root = tree(
        "pipelined",
        &[("a.txt", "a"), ("b.txt", "bb"), ("c.txt", "ccc")],
    );
    let srv = serve(&root, "");
    assert_eq!(get(srv.addr, "/a.txt").text(), "a");
    let mut s = TcpStream::connect(srv.addr).unwrap();
    s.set_read_timeout(Some(Duration::from_secs(10))).unwrap();
    s.write_all(
        b"GET /a.txt HTTP/1.1\r\nHost: x\r\n\r\nGET /b.txt HTTP/1.1\r\nHost: x\r\n\r\nGET /a.txt HTTP/1.1\r\nHost: x\r\n\r\nGET /c.txt HTTP/1.1\r\nHost: x\r\nConnection: close\r\n\r\n",
    )
    .unwrap();
    let mut raw = Vec::new();
    s.read_to_end(&mut raw).unwrap();
    let text = String::from_utf8(raw).unwrap();
    let bodies: Vec<&str> = text
        .split("HTTP/1.1 ")
        .skip(1)
        .map(|r| r.split("\r\n\r\n").nth(1).unwrap())
        .collect();
    assert_eq!(bodies, ["a", "bb", "a", "ccc"]);
}
