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
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, OnceLock};
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
            ("plain/readme.txt", "no index here"),
        ],
    );
    let srv = serve(&root, "");
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
        // fit: "404 Not Found\n" from the handler; Files' own text (A14).
        ("GET", "/plain/", 404, TEXT, "Not Found", ""),
        ("GET", "/missing.txt", 404, TEXT, "Not Found", ""),
        (
            "POST",
            "/hello.txt",
            405,
            TEXT,
            "405 Method Not Allowed\n",
            "",
        ),
        // fit sniffs the type from the first bytes, text/html here; Files' table has no type
        // for a file without an extension (A14).
        (
            "GET",
            "/noext",
            200,
            "application/octet-stream",
            "<html><body>x</body></html>",
            "",
        ),
        // fit redirects a directory asked for without its slash (301, Location /docs/), and
        // answers a file asked for with one 404; Files serves both (A14).
        ("GET", "/docs", 200, HTML, "<p>docs</p>", ""),
        ("GET", "/hello.txt/", 200, TEXT, "hello\n", ""),
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

/// fit's TestServe's other index file, one name (a list of several waits on A14).
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
}

/// fit's TestTraversal, through akit's listener: what climbs above the root is refused by the
/// listener's canonical path (I11), the rest by the handler, all with 400; dotfiles are 404
/// unless `dotfiles` is `allow`.
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
        // A ".." segment that would stay inside is refused too.
        "/x/../ok.txt",
        "/x/%2e%2e/ok.txt",
    ] {
        let r = get(srv.addr, target);
        assert!(
            r.status == 400 && !r.text().contains("secret"),
            "{target}: {r:?}"
        );
    }
    assert_eq!(get(srv.addr, "/ok.txt").text(), "ok");
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
    assert_eq!(get(srv.addr, "/x/../ok.txt").status, 400);
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
    // fit serves a compressed twin only for an encoding precompressed lists (none when left
    // out); Files serves a .br, .zst or .gz twin to every client that accepts it (A14).
    let r = request(srv.addr, "GET", "/twin.txt", &[("Accept-Encoding", "gzip")]);
    assert_eq!(
        (r.text(), r.header("Content-Encoding")),
        ("zz", Some("gzip"))
    );
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

/// fit's TestPrecompressed: the twin a client gets for its `Accept-Encoding`, from memory and
/// from disk, its validators and ranges. Where `Files` picks otherwise than fit (it takes the
/// first of `br`, `zstd`, `gzip` the field names, whatever its q value and whatever
/// `precompressed` lists), the case checks akit's answer and names akio API request A16.
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
    // (target, Accept-Encoding, fit's encoding and body, akit's where they differ.)
    let cases: [(&str, &str, &str, &str, Option<(&str, &str)>); 11] = [
        // Equal q: the first listed.
        ("/app.js", "gzip, br", "br", "br-js", None),
        // A16: Files does not rank by q.
        (
            "/app.js",
            "br;q=0.5, gzip",
            "gzip",
            "gzip-js",
            Some(("br", "br-js")),
        ),
        ("/app.js", "zstd", "zstd", "zstd-js", None),
        // A16: q=0 refuses br, which Files serves.
        (
            "/app.js",
            "br;q=0, identity",
            "",
            "identity-js",
            Some(("br", "br-js")),
        ),
        // A16: Files does not take "*".
        ("/app.js", "*", "br", "br-js", Some(("", "identity-js"))),
        (
            "/app.js",
            "*, br;q=0",
            "zstd",
            "zstd-js",
            Some(("br", "br-js")),
        ),
        ("/app.js", "", "", "identity-js", None),
        // A16: codings are named in any case.
        (
            "/app.js",
            "GZIP",
            "gzip",
            "gzip-js",
            Some(("", "identity-js")),
        ),
        // A16: Files does not trim the blank before the parameters.
        (
            "/app.js",
            "gzip ; q=0.8",
            "gzip",
            "gzip-js",
            Some(("", "identity-js")),
        ),
        // No br twin.
        (
            "/style.css",
            "br, gzip",
            "gzip",
            "gzip-css-0123456789",
            None,
        ),
        ("/plain.txt", "br, gzip", "", "plain", None),
    ];
    for (target, accept, fit_enc, fit_body, akit) in cases {
        let (enc, body) = akit.unwrap_or((fit_enc, fit_body));
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
    // A16: precompressed lists gzip alone, and fit serves no br twin; Files serves it.
    let r = request(
        srv.addr,
        "GET",
        "/app.js",
        &[("Accept-Encoding", "br, gzip")],
    );
    assert_eq!(
        (r.header("Content-Encoding"), r.text()),
        (Some("br"), "br-js")
    );
    drop(srv);

    // Without precompressed fit serves twins as files like any other, with no Vary; Files serves
    // them (A14).
    let srv = serve(&root, "");
    let r = request(srv.addr, "GET", "/app.js", &[("Accept-Encoding", "br")]);
    assert_eq!(
        (r.header("Content-Encoding"), r.text()),
        (Some("br"), "br-js")
    );
    // The twin itself, asked for by its name, is a file.
    let r = get(srv.addr, "/app.js.gz");
    assert_eq!((r.status, r.text()), (200, "gzip-js"));
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
    // A16: fit serves no br twin when precompressed lists gzip alone.
    assert_eq!(
        (r.text(), r.header("Content-Encoding")),
        ("brr", Some("br"))
    );
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
        (
            format!(r#"{{"root": {r}, "browse": "yes"}}"#),
            "routes[0].handler.params.browse: expected a boolean, got a string",
        ),
        (
            format!(r#"{{"root": {r}, "browse_max_entries": -1}}"#),
            "routes[0].handler.params.browse_max_entries: -1 is less than the minimum 0",
        ),
    ];
    for (params, want) in &cases {
        let err = compile_err(params);
        assert!(err.contains(want), "{params}: {err}, want {want}");
    }
    assert!(
        compile_err(&format!(r#"{{"root": {}}}"#, quote(&root.join("none")))).contains("os error")
    );
    // fit's index:[] is no index file; ".hidden" is taken with dotfiles allow.
    for (params, field) in [
        (
            format!(r#"{{"root": {r}, "strip_prefix": "/assets"}}"#),
            "strip_prefix",
        ),
        (
            format!(r#"{{"root": {r}, "max_cache_bytes": 6}}"#),
            "max_cache_bytes",
        ),
        (
            format!(r#"{{"root": {r}, "default_type": "a/b"}}"#),
            "default_type",
        ),
        (
            format!(r#"{{"root": {r}, "compress": ["gzip"]}}"#),
            "compress",
        ),
        (
            format!(r#"{{"root": {r}, "try_files": ["{{path}}", "{{path}}/", "/index.html"]}}"#),
            "try_files: not supported yet: paths tried in turn (akio API requests A13 and A17)",
        ),
        // A dotfile candidate is taken with dotfiles allow, and .well-known with deny (fit's
        // TestDotfilesConfig).
        (
            format!(r#"{{"root": {r}, "try_files": ["{{path}}.html", "/.well-known/fallback"]}}"#),
            "try_files",
        ),
        (
            format!(
                r#"{{"root": {r}, "dotfiles": "allow", "try_files": ["{{path}}", "/.app/index.html"]}}"#
            ),
            "try_files",
        ),
        // fit's TestTryFilesUnsafe: a candidate outside the root is passed over at request time.
        (
            format!(r#"{{"root": {r}, "try_files": ["/../a", "{{path}}"]}}"#),
            "try_files",
        ),
        (
            format!(r#"{{"root": {r}, "browse": true, "browse_max_entries": 5}}"#),
            "browse: not supported yet: directory listings (akio API request A18)",
        ),
        (format!(r#"{{"root": {r}, "index": []}}"#), "index"),
        (format!(r#"{{"root": {r}, "index": ["a", "b"]}}"#), "index"),
        (format!(r#"{{"root": {r}, "disk_io": "auto"}}"#), "disk_io"),
    ] {
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
        format!(
            r#"{{"root": {r}, "disk_io": "pool", "check_interval": "1s", "max_file_bytes": 0}}"#
        ),
        format!(r#"{{"root": {r}, "precompressed": ["br", "zstd", "gzip"]}}"#),
        format!(r#"{{"root": {r}, "precompressed": []}}"#),
        // Listings off, fit's default: nothing changes.
        format!(r#"{{"root": {r}, "browse": false, "browse_max_entries": 5}}"#),
        format!(r#"{{"root": {r}, "browse_max_entries": 0}}"#),
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

/// A file the disk takes forever to open (a FIFO no one writes to) holds a thread of Files'
/// file pool, not an I/O thread: files Files knows are still served at once, on every
/// connection; and the requests waiting on the disk are answered once it answers.
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
    // Few, as the file pool is shared by the tests of this binary.
    let hung: Vec<TcpStream> = (0..3)
        .map(|_| {
            let mut s = TcpStream::connect(srv.addr).unwrap();
            s.set_read_timeout(Some(Duration::from_secs(10))).unwrap();
            s.write_all(b"GET /fifo HTTP/1.1\r\nHost: x\r\nConnection: close\r\n\r\n")
                .unwrap();
            s
        })
        .collect();
    std::thread::sleep(Duration::from_millis(200));
    for _ in 0..16 {
        let mut s = TcpStream::connect(srv.addr).unwrap();
        s.set_read_timeout(Some(Duration::from_secs(2))).unwrap();
        s.write_all(b"GET /hello.txt HTTP/1.1\r\nHost: x\r\nConnection: close\r\n\r\n")
            .unwrap();
        let mut raw = Vec::new();
        s.read_to_end(&mut raw)
            .expect("a known file waits on no disk");
        assert_eq!(parse(&raw).text(), "hello\n");
    }
    // The disk answers: a writer opens the FIFO and closes it, which the readers see as its
    // end, until every hung request is answered.
    let done = Arc::new(AtomicBool::new(false));
    let writer = {
        let (done, fifo) = (done.clone(), fifo.clone());
        std::thread::spawn(move || {
            while !done.load(Ordering::Relaxed) {
                // Blocks until a reader opens it: left blocked when the test ends, harmless.
                drop(fs::OpenOptions::new().write(true).open(&fifo));
                std::thread::sleep(Duration::from_millis(10));
            }
        })
    };
    for mut s in hung {
        let mut raw = Vec::new();
        s.read_to_end(&mut raw).unwrap();
        assert_eq!(parse(&raw).status, 200);
    }
    done.store(true, Ordering::Relaxed);
    drop(writer);
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
