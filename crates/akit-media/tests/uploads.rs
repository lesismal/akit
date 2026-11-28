//! A media library's uploads (I149), fit's `admin/mediafiles_test.go` (`TestMediaUploads`,
//! `TestMediaUploadLarge`) and `medialib/upload_test.go` (`TestUploadRestart`) ported, through
//! the Admin API's endpoints on a server that streams their bodies in, as the Admin API does: an
//! upload begun, cut short and resumed, put in place and probed; the bounds; given up; a large
//! one written in pieces with its reads held; one a restart left read back. Every wait is on the
//! library's status or its uploads', with a deadline.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing,
    clippy::arithmetic_side_effects
)]

use std::io::{Read, Write};
use std::net::{Shutdown, SocketAddr, TcpStream};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use akio::{Engine, Listener};
use akio_http::{BodyMode, Context, Method, Request};
use akit_config::medialib::MediaLibrary;
use akit_config::value::{Value, parse};
use akit_media::library::{self, Library};

const T: Duration = Duration::from_secs(30);

struct Svc(usize);

impl akio_http::Service for Svc {
    fn body_mode(&self, req: &Request) -> BodyMode {
        if matches!(req.method, Method::Get | Method::Head) {
            BodyMode::Buffered
        } else {
            BodyMode::Stream
        }
    }

    fn call(&self, ctx: &mut Context) {
        if !library::admin::serve(self.0, ctx) {
            ctx.respond(akio_http::Response::text(404, "no such endpoint"));
        }
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

    fn json(&self) -> Value {
        parse(&String::from_utf8_lossy(&self.body)).unwrap_or(Value::Null)
    }
}

fn read_resp(mut s: TcpStream, head_only: bool) -> Resp {
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
        .unwrap_or(0);
    let headers = lines
        .filter_map(|l| l.split_once(':'))
        .map(|(n, v)| (n.trim().to_owned(), v.trim().to_owned()))
        .collect();
    Resp {
        status,
        headers,
        body: if head_only {
            Vec::new()
        } else {
            out.get(split + 4..).unwrap_or_default().to_vec()
        },
    }
}

fn http(addr: SocketAddr, method: &str, path: &str, fields: &[(&str, &str)], body: &[u8]) -> Resp {
    let mut s = TcpStream::connect(addr).unwrap();
    s.set_read_timeout(Some(T)).unwrap();
    let mut req = format!("{method} {path} HTTP/1.1\r\nHost: x\r\nConnection: close\r\n");
    for (n, v) in fields {
        req.push_str(&format!("{n}: {v}\r\n"));
    }
    req.push_str(&format!("Content-Length: {}\r\n\r\n", body.len()));
    s.write_all(req.as_bytes()).unwrap();
    s.write_all(body).unwrap();
    read_resp(s, method == "HEAD")
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

struct TempDir(PathBuf);

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

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
    static N: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
    let p = std::env::temp_dir().join(format!(
        "akit-uploads-{}-{}",
        std::process::id(),
        N.fetch_add(1, std::sync::atomic::Ordering::SeqCst)
    ));
    let _ = std::fs::remove_dir_all(&p);
    copy_dir(
        &Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/data/medialib/lib"),
        &p,
    );
    TempDir(std::fs::canonicalize(p).unwrap())
}

static KEYS: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(5 << 40);

struct Rig {
    key: usize,
    lib: Arc<Library>,
    root: TempDir,
    api: SocketAddr,
    engine: Engine,
    conf: MediaLibrary,
}

impl Drop for Rig {
    fn drop(&mut self) {
        self.engine.shutdown();
        library::remove(self.key);
        self.lib.join(T);
    }
}

fn scanned(l: &Library, after: u64) {
    l.wait_status(T, |st| st.scans > after && !st.scanning)
        .expect("a scan");
}

fn rig(max_upload: u64) -> Rig {
    let root = copy_lib();
    let key = KEYS.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
    let conf = MediaLibrary {
        name: "films".into(),
        roots: vec![root.0.display().to_string()],
        upload_dir: root.0.join("uploads").display().to_string(),
        max_upload_bytes: max_upload,
        ..MediaLibrary::default()
    };
    library::apply(key, std::slice::from_ref(&conf)).unwrap();
    let lib = library::lookup(key, "films").unwrap();
    scanned(&lib, 0);
    let engine = Engine::builder()
        .listen(Listener::tcp("127.0.0.1:0".parse().unwrap()).serve(akio_http::serve(Svc(key))))
        .build()
        .unwrap();
    engine.start().unwrap();
    let api = engine.local_addrs()[0];
    Rig {
        key,
        lib,
        root,
        api,
        engine,
        conf,
    }
}

impl Rig {
    fn create(&self, path: &str, size: usize) -> Resp {
        http(
            self.api,
            "POST",
            "/media/library/films/uploads",
            &[
                ("Akit-Console-User", "ada"),
                ("Content-Type", "application/json"),
            ],
            format!(r#"{{"path": "{path}", "size": {size}}}"#).as_bytes(),
        )
    }

    fn patch(&self, up: &str, off: usize, body: &[u8]) -> Resp {
        let off = off.to_string();
        http(
            self.api,
            "PATCH",
            up,
            &[
                ("Upload-Offset", &off),
                ("Content-Type", "application/offset+octet-stream"),
            ],
            body,
        )
    }
}

/// fit's `TestMediaUploads`.
#[test]
fn uploads() {
    let r = rig(1 << 20);
    let data = std::fs::read(r.root.0.join("films/bbb.mp4")).unwrap();
    let c = r.create("new/film.mp4", data.len());
    let v = c.json();
    assert_eq!(c.status, 201, "{v:?}");
    assert_eq!(
        (s(&v, "path"), s(&v, "user")),
        ("uploads/new/film.mp4".into(), "ada".into())
    );
    assert_eq!(c.header("upload-offset"), Some("0"));
    assert_eq!(
        c.header("upload-length"),
        Some(data.len().to_string().as_str())
    );
    let id = s(&v, "id");
    let up = format!("/media/library/films/uploads/{id}");
    assert_eq!(c.header("location"), Some(up.as_str()));

    // Cut short: what arrived is written.
    let half = data.len() / 2;
    {
        let mut s = TcpStream::connect(r.api).unwrap();
        let head = format!(
            "PATCH {up} HTTP/1.1\r\nHost: x\r\nUpload-Offset: 0\r\nContent-Type: application/offset+octet-stream\r\nContent-Length: {}\r\n\r\n",
            data.len()
        );
        s.write_all(head.as_bytes()).unwrap();
        s.write_all(&data[..half]).unwrap();
        s.shutdown(Shutdown::Write).unwrap();
        let _ = read_resp(s, false);
    }
    let info = r
        .lib
        .wait_upload(&id, T, |i| i.offset == half as u64)
        .expect("the half written");
    assert!(!info.busy);
    let h = http(r.api, "HEAD", &up, &[], b"");
    assert_eq!(
        (h.status, h.header("upload-offset")),
        (200, Some(half.to_string().as_str()))
    );
    let p = r.patch(&up, 0, &data[half..]);
    assert_eq!(p.status, 409);
    assert_eq!(p.header("upload-offset"), Some(half.to_string().as_str()));
    let p = http(r.api, "PATCH", &up, &[], b"");
    assert_eq!(p.status, 400);
    let scans = r.lib.status().scans;
    let p = r.patch(&up, half, &data[half..]);
    assert_eq!(p.status, 200, "{:?}", p.json());
    assert_eq!(p.json().get("complete"), Some(&Value::Bool(true)));
    assert_eq!(
        std::fs::read(r.root.0.join("uploads/new/film.mp4")).unwrap(),
        data
    );
    scanned(&r.lib, scans);
    let it = r.lib.item("uploads/new/film").expect("the upload probed");
    assert!(it.error.is_empty() && it.renditions.len() == 1, "{it:?}");
    assert_eq!(http(r.api, "GET", &up, &[], b"").status, 404);

    // Past max_upload_bytes, refused with nothing written; a body longer than the upload,
    // refused and the upload given up.
    assert_eq!(r.create("big.mp4", 1_048_577).status, 413);
    let v = r.create("b.mp4", 10).json();
    let up = format!("/media/library/films/uploads/{}", s(&v, "id"));
    assert_eq!(r.patch(&up, 0, &[0; 11]).status, 413);
    assert_eq!(http(r.api, "GET", &up, &[], b"").status, 404);
    let left: Vec<_> = std::fs::read_dir(r.root.0.join("uploads"))
        .unwrap()
        .map(|e| e.unwrap().file_name())
        .collect();
    assert_eq!(left.len(), 1, "left in upload_dir: {left:?}");

    // Given up.
    let v = r.create("c.mp4", 10).json();
    let up = format!("/media/library/films/uploads/{}", s(&v, "id"));
    let list = http(r.api, "GET", "/media/library/films/uploads", &[], b"").json();
    assert!(matches!(list.get("uploads"), Some(Value::Array(a)) if a.len() == 1));
    assert_eq!(http(r.api, "DELETE", &up, &[], b"").status, 200);
    assert_eq!(http(r.api, "DELETE", &up, &[], b"").status, 404);
    assert_eq!(r.patch(&up, 0, b"").status, 404);
    assert_eq!(r.create("x.txt", 1).status, 400);
    assert_eq!(r.create("../x.mp4", 1).status, 400);
    assert_eq!(r.create("a.mp4", 0).status, 400);
    // The library lists its uploads' settings.
    let libs = http(r.api, "GET", "/media/library", &[], b"").json();
    let Some(Value::Array(libs)) = libs.get("libraries") else {
        panic!("{libs:?}")
    };
    let upv = libs[0].get("upload").unwrap();
    assert_eq!(
        (s(upv, "dir"), n(upv, "max_bytes")),
        ("uploads".into(), 1 << 20)
    );
}

/// fit's `TestMediaUploadLarge`: a body of many pieces, written as it arrives, each piece on the
/// pool while the body's reads are held, whole and in place at the end.
#[test]
fn large_upload() {
    let r = rig(64 << 20);
    let data: Vec<u8> = (0..(9u32 << 20)).map(|i| (i % 251) as u8).collect();
    let v = r.create("big.mp4", data.len()).json();
    let up = format!("/media/library/films/uploads/{}", s(&v, "id"));
    let p = r.patch(&up, 0, &data);
    assert_eq!(p.status, 200, "{:?}", p.json());
    assert_eq!(n(&p.json(), "offset"), data.len() as i64);
    assert_eq!(
        std::fs::read(r.root.0.join("uploads/big.mp4")).unwrap(),
        data
    );
}

/// fit's `TestUploadRestart`: an upload a restart left is read back by the next scan, and goes
/// on from what was written.
#[test]
fn upload_restart() {
    let r = rig(1 << 20);
    let v = r.create("later.mp4", 100).json();
    let id = s(&v, "id");
    let up = format!("/media/library/films/uploads/{id}");
    assert_eq!(r.patch(&up, 0, &[7; 40]).status, 200);
    // The library goes, and comes back: the upload is read back from its .json.
    library::remove(r.key);
    r.lib.join(T);
    library::apply(r.key, std::slice::from_ref(&r.conf)).unwrap();
    let l = library::lookup(r.key, "films").unwrap();
    scanned(&l, 0);
    let info = l.upload(&id).expect("the upload read back");
    assert_eq!(
        (info.offset, info.size, info.path.as_str()),
        (40, 100, "uploads/later.mp4")
    );
    let w = l.write_upload(&id, 40).unwrap();
    let (tx, rx) = std::sync::mpsc::channel();
    w.set_hooks(library::ChunkHooks {
        hold: Box::new(|_| {}),
        done: Box::new(move |info, r| tx.send((info, r)).unwrap()),
    });
    w.write(&[8; 60], true, None);
    let (info, res) = rx.recv_timeout(T).unwrap();
    assert!(res.is_ok() && info.complete, "{res:?}");
    let got = std::fs::read(r.root.0.join("uploads/later.mp4")).unwrap();
    assert_eq!((got.len(), got[39], got[40]), (100, 7, 8));
    library::remove(r.key);
    l.join(T);
}
