//! Interop with aria2c where it is installed (I151, I152; `AKIT_ARIA2C` names it, the test is
//! skipped without): akit's tracker and client roles in this process, akit's client seeding a
//! file; aria2c, given the torrent whose tracker is akit's over UDP, finds akit's client only
//! through it and downloads the file whole. The tracker is reached over HTTP (`bt_tracker` on a
//! listener of the engine): aria2c speaks UDP to trackers only with its DHT on. Loopback only;
//! aria2c's DHT, LPD and PEX off.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing,
    clippy::arithmetic_side_effects
)]

use std::net::UdpSocket;
use std::process::{Command, Stdio};
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

#[test]
fn aria2c_downloads_from_akit_through_akits_tracker() {
    let Some(aria2c) = std::env::var_os("AKIT_ARIA2C") else {
        eprintln!("skipped: AKIT_ARIA2C names no aria2c");
        return;
    };
    let dir = std::env::temp_dir().join(format!("akit-bt-aria2c-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let udp = UdpSocket::bind("127.0.0.1:0").unwrap();
    let udp_addr = udp.local_addr().unwrap();
    drop(udp);
    // UDP threads for the tracker's binding, as the listeners' engine has under `akit run`.
    let engine = akio::Engine::builder()
        .udp_client_threads(1)
        .build()
        .unwrap();
    engine.start().unwrap();
    let engine = Arc::new(engine);
    // The tracker's HTTP listener on port 0: its address as it bound it.
    let id = engine
        .add_listener(
            akio::Listener::tcp(std::net::SocketAddr::from(([127, 0, 0, 1], 0))).serve(
                akio_http::serve(akit_bittorrent::tracker::service(
                    akit_runtime::upstream::EngineSlot::with(Arc::clone(&engine)),
                )),
            ),
        )
        .unwrap();
    let http = engine
        .listeners()
        .into_iter()
        .find(|l| l.id == id)
        .unwrap()
        .local_addr;
    let mut roles = Manager::new(
        &[akit_bittorrent::TRACKER, akit_bittorrent::CLIENT],
        Arc::clone(&engine),
        None,
    );
    let data_dir = dir.join("akit");
    let doc = format!(
        r#"{{"bittorrent": {{"tracker": {{"listen": "{udp_addr}"}}, "client": {{"listen": "127.0.0.1:0", "data_dir": {:?}, "dht": false}}}}}}"#,
        data_dir.to_str().unwrap()
    );
    let errs = roles.apply(&akit_config::from_json(&doc, None).unwrap());
    assert!(errs.is_empty(), "{errs:?}");
    let api = ClientApi::of(&roles.api(akit_bittorrent::client::ID).unwrap()).unwrap();
    let payload = data(3 << 20, 31);
    std::fs::write(data_dir.join("downloads/aria.bin"), &payload).unwrap();
    let mi = Builder::new("aria.bin")
        .piece_length(256 * 1024)
        .tracker(format!("http://{http}/announce"))
        .build_bytes(&payload)
        .unwrap();
    let mi = MetaInfo::parse(&mi.to_bytes()).unwrap();
    let s = api
        .add(Source::Torrent(mi.to_bytes()), AddRequest::default())
        .unwrap();
    let deadline = Instant::now() + Duration::from_secs(30);
    while api.get(&s.id).unwrap().state != "seeding" {
        assert!(Instant::now() < deadline, "akit's seeder");
        std::thread::sleep(Duration::from_millis(100));
    }
    let tf = dir.join("aria.torrent");
    std::fs::write(&tf, mi.to_bytes()).unwrap();
    let out = dir.join("aria2c-out");
    let status = Command::new(aria2c)
        .args([
            "--enable-dht=false",
            "--enable-dht6=false",
            "--bt-enable-lpd=false",
            "--enable-peer-exchange=false",
            "--seed-time=0",
            "--bt-stop-timeout=60",
            "--quiet=true",
        ])
        // aria2c takes no port 0: one free now.
        .arg(format!(
            "--listen-port={}",
            std::net::TcpListener::bind("127.0.0.1:0")
                .unwrap()
                .local_addr()
                .unwrap()
                .port()
        ))
        .arg(format!("--dir={}", out.to_str().unwrap()))
        .arg(format!(
            "--log={}",
            dir.join("aria2c.log").to_str().unwrap()
        ))
        .arg("--log-level=info")
        .arg(&tf)
        .stdout(Stdio::null())
        .status()
        .unwrap();
    if !status.success() {
        let log = std::fs::read_to_string(dir.join("aria2c.log")).unwrap_or_default();
        let tail: Vec<&str> = log.lines().rev().take(60).collect();
        let tail: Vec<&str> = tail.into_iter().rev().collect();
        panic!(
            "aria2c: {status}; akit's torrent: {:?}\n{}",
            api.get(&s.id),
            tail.join("\n")
        );
    }
    assert!(std::fs::read(out.join("aria.bin")).unwrap() == payload);
    roles.stop(Duration::ZERO);
    engine.shutdown();
    engine.wait();
    let _ = std::fs::remove_dir_all(&dir);
}
