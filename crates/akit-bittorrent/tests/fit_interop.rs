//! Interop with fit's own binary (I152; `AKIT_FIT_BIN` names it, the test is skipped without):
//! one fit runs its tracker (`bt_tracker` and `bittorrent.tracker`) and its client; akit's client
//! role downloads a torrent fit seeds, finding fit through fit's tracker, and fit's client
//! downloads one akit's role seeds, through the same tracker. Loopback only, the DHT off.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing,
    clippy::arithmetic_side_effects
)]

use std::io::{Read, Write};
use std::net::{SocketAddr, TcpStream};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};

use akio_bittorrent::metainfo::{Builder, MetaInfo};
use akit_bittorrent::client::{AddRequest, ClientApi, Source};
use akit_roles::host::Manager;

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

fn wait_for(what: &str, secs: u64, mut cond: impl FnMut() -> bool) {
    let deadline = Instant::now() + Duration::from_secs(secs);
    while !cond() {
        assert!(Instant::now() < deadline, "timed out waiting for {what}");
        std::thread::sleep(Duration::from_millis(100));
    }
}

fn http(addr: SocketAddr, method: &str, target: &str, ctype: &str, body: &[u8]) -> (u16, Vec<u8>) {
    let Ok(mut s) = TcpStream::connect(addr) else {
        return (0, Vec::new());
    };
    s.set_read_timeout(Some(Duration::from_secs(10))).unwrap();
    let _ = write!(
        s,
        "{method} {target} HTTP/1.1\r\nHost: {addr}\r\nConnection: close\r\nContent-Type: {ctype}\r\nContent-Length: {}\r\n\r\n",
        body.len()
    );
    let _ = s.write_all(body);
    let mut out = Vec::new();
    let _ = s.read_to_end(&mut out);
    if out.len() < 12 {
        return (0, out);
    }
    let status = String::from_utf8_lossy(&out[9..12]).parse().unwrap_or(0);
    let at = out
        .windows(4)
        .position(|w| w == b"\r\n\r\n")
        .map_or(out.len(), |p| p + 4);
    (status, out[at..].to_vec())
}

#[path = "../../akit-tunnel/tests/support/fit.rs"]
mod fit;

fn torrent(name: &str, payload: &[u8], tracker: &str) -> MetaInfo {
    let mi = Builder::new(name)
        .piece_length(64 * 1024)
        .tracker(tracker)
        .build_bytes(payload)
        .unwrap();
    MetaInfo::parse(&mi.to_bytes()).unwrap()
}

fn hex(b: &[u8]) -> String {
    b.iter().map(|x| format!("{x:02x}")).collect()
}

#[test]
fn akits_client_and_fits_through_fits_tracker() {
    let Some(fit) = std::env::var_os("AKIT_FIT_BIN") else {
        eprintln!("skipped: AKIT_FIT_BIN names no fit binary");
        return;
    };
    let dir: PathBuf = std::env::temp_dir().join(format!("akit-bt-fit-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let fit_dl = dir.join("fit-dl");
    std::fs::create_dir_all(&fit_dl).unwrap();
    let from_fit = data(2 << 20, 21);
    let from_akit = data(1 << 20, 22);
    std::fs::write(fit_dl.join("from-fit.bin"), &from_fit).unwrap();
    // fit's listeners on port 0, where they listen read from its log.
    let run = fit::start(
        fit.to_str().unwrap(),
        dir.join("fit"),
        &format!(
            r#"{{"listeners": [{{"name": "web", "address": "127.0.0.1:0"}}],
                "routes": [{{"listeners": ["web"], "match": {{"path": ["/announce", "/scrape"]}}, "handler": {{"module": "bt_tracker"}}}}],
                "admin": {{"address": "127.0.0.1:0"}},
                "bittorrent": {{"tracker": {{}}, "client": {{"listen": "127.0.0.1:0", "data_dir": {:?}, "download_dir": {:?}, "dht": false, "limits": {{"min_free_bytes": 1}}}}}}}}"#,
            dir.join("fit-data").to_str().unwrap(),
            fit_dl.to_str().unwrap()
        ),
        |_| {},
    );
    let web: SocketAddr = run.addr(r#"listener "web" on "#).parse().unwrap();
    let admin: SocketAddr = run.addr("admin API on ").parse().unwrap();
    let tracker = format!("http://{web}/announce");
    // The torrent fit seeds, and the one akit seeds.
    let mi_fit = torrent("from-fit.bin", &from_fit, &tracker);
    let mi_akit = torrent("from-akit.bin", &from_akit, &tracker);
    wait_for("fit's Admin API", 20, || {
        http(admin, "GET", "/bittorrent/torrents", "text/plain", b"").0 == 200
    });
    let (code, body) = http(
        admin,
        "POST",
        "/bittorrent/torrents",
        "application/x-bittorrent",
        &mi_fit.to_bytes(),
    );
    assert_eq!(code, 201, "{}", String::from_utf8_lossy(&body));
    let fit_complete = |id: &str| {
        let (code, body) = http(
            admin,
            "GET",
            &format!("/bittorrent/torrents/{id}"),
            "text/plain",
            b"",
        );
        code == 200 && String::from_utf8_lossy(&body).contains("\"complete\":true")
    };
    let id_fit = hex(&mi_fit.info_hash.0);
    wait_for("fit's seeder", 30, || fit_complete(&id_fit));
    // akit's client role, in this process.
    let engine = akio::Engine::builder().build().unwrap();
    engine.start().unwrap();
    let engine = Arc::new(engine);
    let mut roles = Manager::new(&[akit_bittorrent::CLIENT], Arc::clone(&engine), None);
    let akit_data = dir.join("akit-data");
    let doc = format!(
        r#"{{"bittorrent": {{"client": {{"listen": "127.0.0.1:0", "data_dir": {:?}, "dht": false}}}}}}"#,
        akit_data.to_str().unwrap()
    );
    let errs = roles.apply(&akit_config::from_json(&doc, None).unwrap());
    assert!(errs.is_empty(), "{errs:?}");
    let api = ClientApi::of(&roles.api(akit_bittorrent::client::ID).unwrap()).unwrap();
    // akit downloads what fit seeds, finding fit through fit's tracker.
    api.add(Source::Torrent(mi_fit.to_bytes()), AddRequest::default())
        .unwrap();
    wait_for("akit's download from fit", 90, || {
        api.get(&id_fit).is_ok_and(|s| s.complete)
    });
    let akit_dl = akit_data.join("downloads");
    assert!(std::fs::read(akit_dl.join("from-fit.bin")).unwrap() == from_fit);
    // fit downloads what akit seeds.
    std::fs::write(akit_dl.join("from-akit.bin"), &from_akit).unwrap();
    let id_akit = hex(&mi_akit.info_hash.0);
    api.add(Source::Torrent(mi_akit.to_bytes()), AddRequest::default())
        .unwrap();
    wait_for("akit's seeder", 30, || {
        api.get(&id_akit).is_ok_and(|s| s.state == "seeding")
    });
    let (code, body) = http(
        admin,
        "POST",
        "/bittorrent/torrents",
        "application/x-bittorrent",
        &mi_akit.to_bytes(),
    );
    assert_eq!(code, 201, "{}", String::from_utf8_lossy(&body));
    wait_for("fit's download from akit", 90, || fit_complete(&id_akit));
    assert!(std::fs::read(fit_dl.join("from-akit.bin")).unwrap() == from_akit);
    roles.stop(Duration::ZERO);
    engine.shutdown();
    engine.wait();
    let _ = std::fs::remove_dir_all(&dir);
}
