//! A media library's files managed (I149), fit's `admin/mediafiles_test.go` (`TestMediaFiles`, `TestMediaUploads`,
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

struct Svc(usize, akit_media::workbench::ViewFn);

impl akio_http::Service for Svc {
    fn body_mode(&self, req: &Request) -> BodyMode {
        if matches!(req.method, Method::Get | Method::Head) {
            BodyMode::Buffered
        } else {
            BodyMode::Stream
        }
    }

    fn call(&self, ctx: &mut Context) {
        if !library::admin::serve_with(self.0, Some(&self.1), ctx) {
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
    rig_with(max_upload, 0)
}

/// [`rig`], its library's pool of `workers` (0 for the default).
fn rig_with(max_upload: u64, workers: usize) -> Rig {
    let root = copy_lib();
    let key = KEYS.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
    let conf = MediaLibrary {
        name: "films".into(),
        roots: vec![root.0.display().to_string()],
        upload_dir: root.0.join("uploads").display().to_string(),
        max_upload_bytes: max_upload,
        file_workers: workers,
        ..MediaLibrary::default()
    };
    library::apply(key, std::slice::from_ref(&conf)).unwrap();
    let lib = library::lookup(key, "films").unwrap();
    scanned(&lib, 0);
    // What uses an item: the channel the running config names, as the Admin API reads it.
    let doc = format!(
        r#"{{"listeners": [{{"name": "web", "address": "127.0.0.1:0"}}],
          "media_library": [{{"name": "films", "roots": ["{}"]}}],
          "streaming": {{"channels": [{{"name": "tv/late", "library": "films", "items": ["clips/late", "broken"], "schedule": [{{"at": "20:00", "item": "films/x"}}]}}]}},
          "routes": [{{"handler": {{"module": "respond", "params": {{"body": "x"}}}}}}]}}"#,
        root.0.display()
    );
    let cfg = Arc::new(akit_config::from_json(&doc, None).unwrap());
    let view: akit_media::workbench::ViewFn =
        Arc::new(move || akit_media::workbench::View::new(Some(Arc::clone(&cfg)), &[]));
    let engine = Engine::builder()
        .listen(
            Listener::tcp("127.0.0.1:0".parse().unwrap()).serve(akio_http::serve(Svc(key, view))),
        )
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

/// What of an upload's files is left in `upload_dir`.
fn upload_files(r: &Rig) -> Vec<String> {
    std::fs::read_dir(r.root.0.join("uploads"))
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .filter(|n| n.starts_with(".akit-upload-"))
        .collect()
}

/// A body longer than the upload, refused, is answered once the upload it gave up is gone from
/// `upload_dir`, as fit answers it: with the pool that removes its files held, no answer comes;
/// let go, the answer finds nothing of it left. Answered at once, a listing right after it found
/// the `.json` still there now and then. Both a declared length past the upload and a chunked
/// body that grows past it.
#[test]
fn refused_body_answered_after_its_files() {
    let r = rig_with(1 << 20, 1);
    for (name, framing, body) in [
        ("b.mp4", "Content-Length: 11", &[0u8; 11][..]),
        (
            "c.mp4",
            "Transfer-Encoding: chunked",
            &b"b\r\n\0\0\0\0\0\0\0\0\0\0\0\r\n0\r\n\r\n"[..],
        ),
    ] {
        let v = r.create(name, 10).json();
        let up = format!("/media/library/films/uploads/{}", s(&v, "id"));
        assert_eq!(upload_files(&r).len(), 2, "{name}: {:?}", upload_files(&r));
        // The pool's one worker held.
        let (go, gate) = std::sync::mpsc::channel::<()>();
        let (held_tx, held) = std::sync::mpsc::channel::<()>();
        assert!(r.lib.submit(
            Box::new(move || {
                held_tx.send(()).unwrap();
                let _ = gate.recv_timeout(T);
            }),
            None,
        ));
        held.recv_timeout(T).unwrap();
        let mut sock = TcpStream::connect(r.api).unwrap();
        let head = format!(
            "PATCH {up} HTTP/1.1\r\nHost: x\r\nUpload-Offset: 0\r\nContent-Type: application/offset+octet-stream\r\n{framing}\r\n\r\n"
        );
        sock.write_all(head.as_bytes()).unwrap();
        sock.write_all(body).unwrap();
        // The removal queued behind the held worker.
        let deadline = std::time::Instant::now() + T;
        while r.lib.pool().stats().queued == 0 {
            assert!(
                std::time::Instant::now() < deadline,
                "{name}: nothing queued"
            );
            std::thread::sleep(Duration::from_millis(5));
        }
        sock.set_read_timeout(Some(Duration::from_millis(300)))
            .unwrap();
        match sock.read(&mut [0u8; 1]) {
            Err(e)
                if matches!(
                    e.kind(),
                    std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut
                ) => {}
            other => panic!(
                "{name}: answered with its files still there: {other:?}, {:?}",
                upload_files(&r)
            ),
        }
        go.send(()).unwrap();
        sock.set_read_timeout(Some(T)).unwrap();
        assert_eq!(read_resp(sock, false).status, 413, "{name}");
        assert_eq!(upload_files(&r), Vec::<String>::new(), "{name}");
        assert_eq!(http(r.api, "GET", &up, &[], b"").status, 404, "{name}");
    }
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
    // akit's names (decision 037); fit's leftover upload is not taken up, nor touched.
    let dir = r.root.0.join("uploads");
    assert!(dir.join(format!(".akit-upload-{id}.part")).exists());
    assert!(dir.join(format!(".akit-upload-{id}.json")).exists());
    let fit_id = "0123456789abcdef0123456789abcdef";
    let fit_meta = dir.join(format!(".fit-upload-{fit_id}.json"));
    std::fs::write(
        &fit_meta,
        format!(
            r#"{{"id": "{fit_id}", "root": "{}", "path": "uploads/fit.mp4", "size": 10, "created": "2026-11-28T00:00:00Z"}}"#,
            r.root.0.display()
        ),
    )
    .unwrap();
    std::fs::write(dir.join(format!(".fit-upload-{fit_id}.part")), [1u8; 4]).unwrap();
    // The library goes, and comes back: the upload is read back from its .json.
    library::remove(r.key);
    r.lib.join(T);
    library::apply(r.key, std::slice::from_ref(&r.conf)).unwrap();
    let l = library::lookup(r.key, "films").unwrap();
    scanned(&l, 0);
    let info = l.upload(&id).expect("the upload read back");
    assert!(l.upload(fit_id).is_none(), "fit's upload taken up");
    assert!(fit_meta.exists(), "fit's upload touched");
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
fn post(r: &Rig, path: &str, body: &str) -> Resp {
    http(
        r.api,
        "POST",
        path,
        &[("Content-Type", "application/json")],
        body.as_bytes(),
    )
}

/// fit's `TestMediaFiles`: a root's directories, what uses an item, an item renamed, moved and
/// deleted, its sidecar read and written.
#[test]
fn files() {
    let r = rig(1 << 20);
    let get = |p: &str| http(r.api, "GET", p, &[], b"");
    let v = get("/media/library/films/dirs").json();
    let dirs = format!("{:?}", v.get("dirs"));
    assert!(dirs.contains("films/subs"), "{v:?}");
    assert_eq!(s(&v, "root"), r.root.0.display().to_string());
    assert_eq!(get("/media/library/films/dirs?root=/none").status, 404);

    // What uses an item: a channel's list, or its schedule.
    let u = get("/media/library/films/usage?item=clips/late");
    let v = u.json();
    assert_eq!(u.status, 200);
    assert_eq!(v.get("in_use"), Some(&Value::Bool(true)));
    assert_eq!(v.get("sessions_counted"), Some(&Value::Bool(false)));
    assert!(format!("{:?}", v.get("channels")).contains("tv/late"));
    let v = get("/media/library/films/usage?item=films/bbb").json();
    assert_eq!(v.get("in_use"), Some(&Value::Bool(false)));
    assert_eq!(get("/media/library/films/usage?item=none").status, 404);
    // An item in use is not deleted, moved or renamed without force.
    for op in ["delete", "move", "rename"] {
        let resp = post(
            &r,
            &format!("/media/library/films/{op}"),
            r#"{"item": "clips/late", "dir": "x", "name": "y"}"#,
        );
        assert_eq!(resp.status, 409, "{op}");
        assert!(resp.json().get("usage").is_some(), "{op}");
    }
    let scans = r.lib.status().scans;
    let resp = post(
        &r,
        "/media/library/films/move",
        r#"{"item": "clips/late", "dir": "old", "force": true}"#,
    );
    assert_eq!(resp.status, 200, "{:?}", resp.json());
    assert_eq!(s(&resp.json(), "id"), "old/late");
    // Out of the catalog at once; the scan lists it as it is.
    assert!(r.lib.item("clips/late").is_none());
    scanned(&r.lib, scans);
    assert!(r.lib.item("old/late").is_some());

    // Rename: an item without a sidecar takes the ID of its new path.
    let scans = r.lib.status().scans;
    let resp = post(
        &r,
        "/media/library/films/rename",
        r#"{"item": "films/bbb", "name": "bigbuck"}"#,
    );
    let v = resp.json();
    assert_eq!(resp.status, 200, "{v:?}");
    assert_eq!(
        (s(&v, "id"), s(&v, "op")),
        ("films/bigbuck".into(), "rename".into())
    );
    assert_eq!(get("/media/library/films/items/films/bbb").status, 404);
    scanned(&r.lib, scans);
    assert!(r.lib.item("films/bigbuck").is_some());
    for (body, want) in [
        (r#"{"item": "films/bigbuck", "name": "../x"}"#, 400),
        (r#"{"item": "films/bigbuck", "name": "bigbuck"}"#, 400),
        (r#"{"item": "films/bigbuck", "name": "big.720p"}"#, 400),
        (r#"{"item": "none", "name": "x"}"#, 404),
        ("{", 400),
    ] {
        assert_eq!(
            post(&r, "/media/library/films/rename", body).status,
            want,
            "{body}"
        );
    }
    assert_eq!(post(&r, "/media/library/none/rename", "{}").status, 404);

    // The sidecar: read, written from what was read, and refused when it changed since.
    let resp = get("/media/library/films/sidecar?item=sintel");
    let v = resp.json();
    assert_eq!(resp.status, 200);
    assert_eq!(v.get("exists"), Some(&Value::Bool(true)));
    assert_eq!(s(&v, "path"), "films/sintel.json");
    let Some(Value::Object(mut sd)) = v.get("sidecar").cloned() else {
        panic!("{v:?}")
    };
    sd.retain(|(k, _)| k != "title");
    sd.push(("title".into(), Value::String("Sintel, the film".into())));
    let body = Value::Object(vec![
        ("item".into(), Value::String("sintel".into())),
        ("sidecar".into(), Value::Object(sd)),
        ("modified".into(), v.get("modified").cloned().unwrap()),
    ])
    .to_json();
    let scans = r.lib.status().scans;
    let resp = http(
        r.api,
        "PUT",
        "/media/library/films/sidecar",
        &[],
        body.as_bytes(),
    );
    assert_eq!(resp.status, 200, "{:?}", resp.json());
    assert_eq!(s(&resp.json(), "op"), "sidecar");
    scanned(&r.lib, scans);
    assert_eq!(r.lib.item("sintel").unwrap().title, "Sintel, the film");
    let resp = http(
        r.api,
        "PUT",
        "/media/library/films/sidecar",
        &[],
        body.as_bytes(),
    );
    assert_eq!(resp.status, 409, "a stale sidecar");
    for (body, want) in [
        (r#"{"item": "sintel"}"#, 400),
        (r#"{"item": "none", "sidecar": {}}"#, 404),
        (
            r#"{"item": "sintel", "sidecar": {"poster": "../x.jpg"}}"#,
            400,
        ),
        // A new ID of an item in use.
        (r#"{"item": "broken", "sidecar": {"id": "broken2"}}"#, 409),
    ] {
        let resp = http(
            r.api,
            "PUT",
            "/media/library/films/sidecar",
            &[],
            body.as_bytes(),
        );
        assert_eq!(resp.status, want, "{body}: {:?}", resp.json());
    }
    assert_eq!(get("/media/library/films/sidecar?item=none").status, 404);
    // A draft for an item without a sidecar.
    let v = get("/media/library/films/sidecar?item=films/bigbuck").json();
    assert_eq!(v.get("exists"), Some(&Value::Bool(false)));
    assert_eq!(s(&v, "path"), "films/bigbuck.json");

    // Delete: out of the catalog at once.
    let resp = post(
        &r,
        "/media/library/films/delete",
        r#"{"item": "films/bigbuck"}"#,
    );
    let v = resp.json();
    assert_eq!(resp.status, 200, "{v:?}");
    assert!(
        matches!(v.get("files"), Some(Value::Array(f)) if f.len() == 6),
        "{v:?}"
    );
    assert_eq!(get("/media/library/films/items/films/bigbuck").status, 404);
    assert!(!r.root.0.join("films/bigbuck.mp4").exists());
}
