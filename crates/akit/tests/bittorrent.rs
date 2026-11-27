//! The BitTorrent tracker through `akit run` (I151): a route whose handler is `bt_tracker` serves
//! the `bittorrent.tracker` role's HTTP announces and scrapes on the listener's engine, and the
//! role answers UDP on its own `listen`; a peer announced over HTTP is handed to one announcing
//! over UDP. Bytes written by hand, as a client would send them.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing,
    clippy::arithmetic_side_effects
)]

use std::io::{BufRead, BufReader, Read, Write};
use std::net::{SocketAddr, TcpListener, TcpStream, UdpSocket};
use std::path::Path;
use std::process::{Child, Command, Stdio};
use std::time::Duration;

fn free() -> SocketAddr {
    TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
}

struct Akit(Child);

impl Drop for Akit {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

/// `akit run -c path`, once it prints `ready`.
fn run(path: &Path, ready: &str) -> Akit {
    let mut child = Command::new(env!("CARGO_BIN_EXE_akit"))
        .args(["run", "-shutdown-timeout", "2s", "-c"])
        .arg(path)
        .stdout(Stdio::piped())
        .stderr(Stdio::inherit())
        .spawn()
        .unwrap();
    let out = BufReader::new(child.stdout.take().unwrap());
    let ready = ready.to_owned();
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        for line in out.lines() {
            let Ok(line) = line else { break };
            if line.starts_with(&ready) {
                let _ = tx.send(());
            }
        }
    });
    rx.recv_timeout(Duration::from_secs(20))
        .expect("akit run did not start");
    Akit(child)
}

fn pct(b: &[u8]) -> String {
    b.iter().map(|x| format!("%{x:02X}")).collect()
}

fn get(addr: SocketAddr, target: &str) -> Vec<u8> {
    let mut s = TcpStream::connect(addr).unwrap();
    s.set_read_timeout(Some(Duration::from_secs(10))).unwrap();
    write!(
        s,
        "GET {target} HTTP/1.1\r\nHost: t\r\nConnection: close\r\n\r\n"
    )
    .unwrap();
    let mut out = Vec::new();
    s.read_to_end(&mut out).unwrap();
    assert!(
        out.starts_with(b"HTTP/1.1 200"),
        "{}",
        String::from_utf8_lossy(&out)
    );
    let at = out.windows(4).position(|w| w == b"\r\n\r\n").unwrap() + 4;
    out[at..].to_vec()
}

#[test]
fn tracker_under_akit_run() {
    let dir = std::path::PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("bittorrent-tracker");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let web = free();
    let udp = UdpSocket::bind("127.0.0.1:0").unwrap();
    let udp_addr = udp.local_addr().unwrap();
    drop(udp);
    let path = dir.join("akit.json");
    std::fs::write(
        &path,
        format!(
            r#"{{"listeners": [{{"name": "web", "address": "{web}"}}],
                "routes": [{{"listeners": ["web"], "match": {{"path": ["/tracker/*"]}}, "handler": {{"module": "bt_tracker"}}}}],
                "bittorrent": {{"tracker": {{"listen": "{udp_addr}", "interval": "10m"}}}}}}"#
        ),
    )
    .unwrap();
    let _akit = run(&path, "akit: running bittorrent.tracker");
    let ih = [0x42u8; 20];
    // A seeder over HTTP.
    let body = get(
        web,
        &format!(
            "/tracker/announce?info_hash={}&peer_id={}&port=6881&left=0&event=started",
            pct(&ih),
            pct(&[1; 20])
        ),
    );
    assert!(
        body.starts_with(b"d8:complete"),
        "{}",
        String::from_utf8_lossy(&body)
    );
    assert!(
        body.windows(13).any(|w| w == b"8:intervali60"),
        "{}",
        String::from_utf8_lossy(&body)
    );
    // A leecher over UDP: connect, then announce; it gets the seeder.
    let s = UdpSocket::bind("127.0.0.1:0").unwrap();
    s.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
    let mut connect = 0x0417_2710_1980u64.to_be_bytes().to_vec();
    connect.extend_from_slice(&0u32.to_be_bytes());
    connect.extend_from_slice(&7u32.to_be_bytes());
    s.send_to(&connect, udp_addr).unwrap();
    let mut buf = [0u8; 1500];
    let (n, _) = s.recv_from(&mut buf).unwrap();
    assert_eq!(n, 16);
    let conn = &buf[8..16].to_vec();
    let mut ann = conn.clone();
    ann.extend_from_slice(&1u32.to_be_bytes()); // announce
    ann.extend_from_slice(&8u32.to_be_bytes()); // transaction
    ann.extend_from_slice(&ih);
    ann.extend_from_slice(&[2u8; 20]);
    ann.extend_from_slice(&0u64.to_be_bytes()); // downloaded
    ann.extend_from_slice(&100u64.to_be_bytes()); // left
    ann.extend_from_slice(&0u64.to_be_bytes()); // uploaded
    ann.extend_from_slice(&2u32.to_be_bytes()); // started
    ann.extend_from_slice(&0u32.to_be_bytes()); // ip
    ann.extend_from_slice(&0u32.to_be_bytes()); // key
    ann.extend_from_slice(&(-1i32).to_be_bytes()); // numwant
    ann.extend_from_slice(&6882u16.to_be_bytes());
    s.send_to(&ann, udp_addr).unwrap();
    let (n, _) = s.recv_from(&mut buf).unwrap();
    assert_eq!(n, 20 + 6, "{:?}", &buf[..n]);
    assert_eq!(&buf[0..8], &[0, 0, 0, 1, 0, 0, 0, 8]);
    assert_eq!(u32::from_be_bytes(buf[8..12].try_into().unwrap()), 600);
    assert_eq!(&buf[20..26], &[127, 0, 0, 1, 0x1a, 0xe1]);
}

/// An HTTP/1.1 request to `addr`: the status and the body.
fn http(addr: SocketAddr, method: &str, target: &str, ctype: &str, body: &[u8]) -> (u16, Vec<u8>) {
    let mut s = TcpStream::connect(addr).unwrap();
    s.set_read_timeout(Some(Duration::from_secs(10))).unwrap();
    write!(
        s,
        "{method} {target} HTTP/1.1\r\nHost: {addr}\r\nConnection: close\r\nContent-Type: {ctype}\r\nContent-Length: {}\r\n\r\n",
        body.len()
    )
    .unwrap();
    s.write_all(body).unwrap();
    let mut out = Vec::new();
    s.read_to_end(&mut out).unwrap();
    let status = String::from_utf8_lossy(&out[9..12]).parse().unwrap();
    let at = out.windows(4).position(|w| w == b"\r\n\r\n").unwrap() + 4;
    (status, out[at..].to_vec())
}

fn data(n: usize, seed: u32) -> Vec<u8> {
    let mut x = seed | 1;
    (0..n)
        .map(|_| {
            x ^= x << 13;
            x ^= x >> 17;
            x ^= x << 5;
            x.to_le_bytes()[0]
        })
        .collect()
}

/// fit's binary, `fit run -c path`, stopped when dropped.
struct Fit(Child);

impl Drop for Fit {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

/// Waits up to `secs` for `cond`, looking every 100 ms.
fn wait_for(what: &str, secs: u64, mut cond: impl FnMut() -> bool) {
    let deadline = std::time::Instant::now() + Duration::from_secs(secs);
    while !cond() {
        assert!(
            std::time::Instant::now() < deadline,
            "timed out waiting for {what}"
        );
        std::thread::sleep(Duration::from_millis(100));
    }
}

/// fit's client role, its Admin API at `admin`, downloading into `dl`.
fn fit_client(fit: &std::ffi::OsStr, dir: &Path, name: &str, dl: &Path) -> (Fit, SocketAddr) {
    let admin = free();
    let listen = free();
    let path = dir.join(format!("{name}.json"));
    std::fs::write(
        &path,
        format!(
            r#"{{"admin": {{"address": "{admin}"}},
                "bittorrent": {{"client": {{"listen": "{listen}", "data_dir": {:?}, "download_dir": {:?}, "dht": false, "limits": {{"min_free_bytes": 1}}}}}}}}"#,
            dir.join(format!("{name}-data")).to_str().unwrap(),
            dl.to_str().unwrap()
        ),
    )
    .unwrap();
    let child = Command::new(fit)
        .args(["run", "-c"])
        .arg(&path)
        .stdout(Stdio::null())
        .stderr(Stdio::inherit())
        .spawn()
        .unwrap();
    let fit = Fit(child);
    wait_for("fit's Admin API", 20, || {
        TcpStream::connect(admin).is_ok_and(|mut s| {
            let _ = write!(
                s,
                "GET /bittorrent/torrents HTTP/1.1\r\nHost: {admin}\r\nConnection: close\r\n\r\n"
            );
            let mut b = Vec::new();
            let _ = s.read_to_end(&mut b);
            b.starts_with(b"HTTP/1.1 200")
        })
    });
    (fit, admin)
}

/// Interop (I151, with fit's binary in `AKIT_FIT_BIN`): two of fit's clients find each other only
/// through akit's tracker under `akit run`, the seeder announcing over HTTP and the leecher over
/// UDP, and the leecher downloads the file whole.
#[test]
fn fit_clients_through_akits_tracker() {
    let Some(fit) = std::env::var_os("AKIT_FIT_BIN") else {
        eprintln!("skipped: AKIT_FIT_BIN names no fit binary");
        return;
    };
    use akio_bittorrent::metainfo::Builder;
    let dir = std::path::PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("bittorrent-fit");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let web = free();
    let udp = UdpSocket::bind("127.0.0.1:0").unwrap();
    let udp_addr = udp.local_addr().unwrap();
    drop(udp);
    let path = dir.join("akit.json");
    std::fs::write(
        &path,
        format!(
            r#"{{"listeners": [{{"name": "web", "address": "{web}"}}],
                "routes": [{{"listeners": ["web"], "match": {{"path": ["/tracker/*"]}}, "handler": {{"module": "bt_tracker"}}}}],
                "bittorrent": {{"tracker": {{"listen": "{udp_addr}"}}}}}}"#
        ),
    )
    .unwrap();
    let _akit = run(&path, "akit: running bittorrent.tracker");
    let payload = data(3 << 20, 11);
    let seed_dl = dir.join("seed-dl");
    std::fs::create_dir_all(&seed_dl).unwrap();
    std::fs::write(seed_dl.join("interop.bin"), &payload).unwrap();
    let torrent = |tracker: String| {
        Builder::new("interop.bin")
            .piece_length(64 * 1024)
            .tracker(tracker)
            .build_bytes(&payload)
            .unwrap()
    };
    let over_http = torrent(format!("http://{web}/tracker/announce"));
    let over_udp = torrent(format!("udp://{udp_addr}/announce"));
    assert_eq!(over_http.info_hash, over_udp.info_hash);
    let id: String = over_http
        .info_hash
        .0
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect();
    let (_seeder, seed_admin) = fit_client(&fit, &dir, "seeder", &seed_dl);
    let (code, body) = http(
        seed_admin,
        "POST",
        "/bittorrent/torrents",
        "application/x-bittorrent",
        &over_http.to_bytes(),
    );
    assert_eq!(code, 201, "{}", String::from_utf8_lossy(&body));
    let complete = |admin: SocketAddr| {
        let (code, body) = http(
            admin,
            "GET",
            &format!("/bittorrent/torrents/{id}"),
            "text/plain",
            b"",
        );
        code == 200 && String::from_utf8_lossy(&body).contains("\"complete\":true")
    };
    wait_for("fit's seeder to check its data", 30, || {
        complete(seed_admin)
    });
    let leech_dl = dir.join("leech-dl");
    let (_leecher, leech_admin) = fit_client(&fit, &dir, "leecher", &leech_dl);
    let (code, body) = http(
        leech_admin,
        "POST",
        "/bittorrent/torrents",
        "application/x-bittorrent",
        &over_udp.to_bytes(),
    );
    assert_eq!(code, 201, "{}", String::from_utf8_lossy(&body));
    wait_for(
        "fit's leecher to download through akit's tracker",
        90,
        || complete(leech_admin),
    );
    assert!(std::fs::read(leech_dl.join("interop.bin")).unwrap() == payload);
    // akit's tracker counts both: a seeder and a completed download.
    let body = get(
        web,
        &format!("/tracker/scrape?info_hash={}", pct(&over_http.info_hash.0)),
    );
    let text = String::from_utf8_lossy(&body);
    assert!(
        text.contains("8:completei2e") || text.contains("8:completei1e"),
        "{text}"
    );
}

/// `akit bt <args>` against the config at `cfg`: its exit code, standard output and error.
fn akit_bt(cfg: &Path, args: &[&str]) -> (i32, String, String) {
    let out = Command::new(env!("CARGO_BIN_EXE_akit"))
        .arg("bt")
        .args(args)
        .arg("-c")
        .arg(cfg)
        .output()
        .unwrap();
    (
        out.status.code().unwrap_or(-1),
        String::from_utf8_lossy(&out.stdout).into_owned(),
        String::from_utf8_lossy(&out.stderr).into_owned(),
    )
}

/// fit's `cli/bt_test.go` (I157): `akit bt` adds a .torrent file through the Admin API of an
/// `akit run` whose client role runs, lists it, shows it by its name, pauses it by the start of
/// its ID, sets its limits, shows the alternative rates and removes it with its files; a torrent
/// it does not know and a bad command line are refused.
#[test]
fn akit_bt_through_the_admin_api() {
    let dir = std::path::PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("bittorrent-akit-bt");
    let _ = std::fs::remove_dir_all(&dir);
    let downloads = dir.join("data").join("downloads");
    std::fs::create_dir_all(&downloads).unwrap();
    let payload = data(200_000, 91);
    std::fs::write(downloads.join("t.bin"), &payload).unwrap();
    let mi = akio_bittorrent::metainfo::Builder::new("t.bin")
        .piece_length(64 * 1024)
        .build_bytes(&payload)
        .unwrap();
    let id: String = mi.info_hash.0.iter().map(|b| format!("{b:02x}")).collect();
    let torrent = dir.join("t.torrent");
    std::fs::write(&torrent, mi.to_bytes()).unwrap();
    let (web, admin, peer) = (free(), free(), free());
    let path = dir.join("akit.json");
    std::fs::write(
        &path,
        format!(
            r#"{{"listeners": [{{"name": "web", "address": "{web}"}}],
                "admin": {{"address": "{admin}"}},
                "bittorrent": {{"client": {{"listen": "{peer}", "data_dir": {:?}, "dht": false}}}}}}"#,
            dir.join("data").to_str().unwrap()
        ),
    )
    .unwrap();
    let _akit = run(&path, "akit: admin API on");

    let (code, out, err) = akit_bt(&path, &["add", torrent.to_str().unwrap()]);
    assert_eq!(code, 0, "{err}");
    assert!(out.starts_with(&format!("{id} t.bin: added, ")), "{out}");
    wait_for("seeding", 20, || {
        akit_bt(&path, &["ls", "-state", "seeding"])
            .1
            .contains("t.bin")
    });
    let (code, out, _) = akit_bt(&path, &["ls"]);
    assert_eq!(code, 0);
    assert!(out.starts_with("ID  "), "{out}");
    assert!(out.contains(&id[..12]) && out.contains("100.0%"), "{out}");
    let (code, out, _) = akit_bt(&path, &["ls", "-json"]);
    assert!(
        code == 0 && out.starts_with(r#"{"torrents":[{"id":"#),
        "{out}"
    );

    let (code, out, err) = akit_bt(&path, &["info", "t.bin"]);
    assert_eq!(code, 0, "{err}");
    assert!(
        out.starts_with(&format!(
            "id         {id}\nname       t.bin\nstate      seeding\n"
        )),
        "{out}"
    );
    assert!(out.contains("FILE  SIZE") && out.contains("t.bin"), "{out}");

    let (code, out, err) = akit_bt(&path, &["pause", &id[..8]]);
    assert_eq!((code, out), (0, format!("{id} t.bin: paused\n")), "{err}");
    let (code, out, err) = akit_bt(
        &path,
        &["limit", "-up", "512K", "-seed-time", "2h", "t.bin"],
    );
    assert_eq!(code, 0, "{err}");
    assert!(out.starts_with(&id), "{out}");
    let (_, out, _) = akit_bt(&path, &["info", "t.bin"]);
    assert!(
        out.contains("limits     up 512.0 KiB/s, down unlimited, ratio 0, seed time 2h0m0s"),
        "{out}"
    );
    let (code, out, _) = akit_bt(&path, &["alt"]);
    assert_eq!(code, 0);
    assert!(
        out.starts_with("mode    auto\n") && out.contains("never hold"),
        "{out}"
    );

    let (code, out, err) = akit_bt(&path, &["rm", "-delete-data", "t.bin"]);
    assert_eq!((code, out), (0, format!("{id} removed\n")), "{err}");
    assert!(!downloads.join("t.bin").exists());

    let (code, _, err) = akit_bt(&path, &["info", "nothing"]);
    assert_eq!(code, 1);
    assert!(err.contains("no torrent is \"nothing\""), "{err}");
    let (code, _, err) = akit_bt(&path, &["ls", "extra"]);
    assert_eq!(code, 2);
    assert!(err.contains("takes no arguments"), "{err}");
    let (code, _, err) = akit_bt(&path, &["alt", "fast"]);
    assert_eq!(code, 2, "{err}");
    let (code, _, err) = akit_bt(&path, &["add", "magnet:?xt=urn:btih:zz"]);
    assert_eq!(code, 1);
    assert!(err.contains("the Admin API answered 400"), "{err}");
}

/// A private tracker through `akit run` (I159): `media_auth`'s user's passkey in the announce's
/// path lets the announce in, none is refused, and the access log writes the passkey as
/// `REDACTED`, as fit's handler does.
#[test]
fn a_private_tracker_under_akit_run() {
    let dir = std::path::PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("bittorrent-private");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let web = free();
    let log = dir.join("access.log");
    let path = dir.join("akit.json");
    std::fs::write(
        &path,
        format!(
            r#"{{"listeners": [{{"name": "web", "address": "{web}"}}],
                "access_log": {{"output": {:?}, "flush_interval": "10ms"}},
                "media_auth": [{{"name": "bt", "secret": "0123456789abcdef0123456789abcdef",
                    "users": [{{"name": "alice", "password": "alice-password", "play": ["bittorrent/*"]}}]}}],
                "routes": [{{"listeners": ["web"], "match": {{"path": ["/tracker/*"]}}, "handler": {{"module": "bt_tracker"}}}}],
                "bittorrent": {{"tracker": {{"auth": "bt"}}}}}}"#,
            log.to_str().unwrap()
        ),
    )
    .unwrap();
    let _akit = run(&path, "akit: running bittorrent.tracker");
    // fit's passkey of alice under this secret.
    let alice = "c3751c196e55193e0c3be763bcc1b8ab";
    let target = |prefix: &str| {
        format!(
            "{prefix}/announce?info_hash={}&peer_id={}&port=6881&left=0",
            pct(&[0x42; 20]),
            pct(&[1; 20])
        )
    };
    let body = get(web, &target(&format!("/tracker/{alice}")));
    assert!(
        body.starts_with(b"d8:complete"),
        "{}",
        String::from_utf8_lossy(&body)
    );
    let body = get(web, &target("/tracker/nope"));
    assert!(
        String::from_utf8_lossy(&body).contains("unknown passkey"),
        "{}",
        String::from_utf8_lossy(&body)
    );
    wait_for("the access log", 10, || {
        std::fs::read_to_string(&log).is_ok_and(|l| l.lines().count() >= 2)
    });
    let lines = std::fs::read_to_string(&log).unwrap();
    assert!(!lines.contains(alice), "{lines}");
    assert!(lines.contains("/tracker/REDACTED/announce"), "{lines}");
}
