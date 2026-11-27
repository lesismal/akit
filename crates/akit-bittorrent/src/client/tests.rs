//! fit's `bittorrent/role_test.go`, `role_edges_test.go` and `state_test.go`, ported (I152): a
//! download between two roles on loopback, a restart resuming without a check, a magnet link's
//! .torrent kept, reloads in place and by a new client, a port taken, the state's refusals, a
//! broken torrent kept listed, the API's refusals, fit's own state file read and written back.
//! The DHT and LSD are off: nothing leaves the loopback.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing,
    clippy::arithmetic_side_effects
)]

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

use akio_bittorrent::metainfo::{Builder, MetaInfo};
use akit_roles::host::Manager;

use super::{AddRequest, ApiError, CLIENT, ClientApi, Source};

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

/// JSON without its whitespace.
fn compact(s: &str) -> String {
    s.split_whitespace().collect()
}

fn tmp(name: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("akit-bt-{}-{name}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

fn free_port() -> u16 {
    std::net::TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .port()
}

/// The role on an engine of the test's own (the client makes its own until akio's A283).
struct Env {
    engine: Arc<akio::Engine>,
    roles: Manager,
    dir: PathBuf,
}

impl Drop for Env {
    fn drop(&mut self) {
        self.roles.stop(Duration::ZERO);
        self.engine.shutdown();
        self.engine.wait();
    }
}

fn member(dir: &Path, port: u16, extra: &str) -> String {
    format!(
        r#"{{"bittorrent": {{"client": {{"listen": "127.0.0.1:{port}", "data_dir": {:?}, "dht": false, "save_interval": "100ms"{extra}}}}}}}"#,
        dir.join("data").to_str().unwrap()
    )
}

impl Env {
    fn start(dir: &Path, port: u16, extra: &str) -> Env {
        let engine = akio::Engine::builder().build().unwrap();
        engine.start().unwrap();
        let engine = Arc::new(engine);
        let mut env = Env {
            roles: Manager::new(&[CLIENT], Arc::clone(&engine), None),
            engine,
            dir: dir.to_path_buf(),
        };
        env.apply(port, extra).unwrap();
        env
    }

    fn apply(&mut self, port: u16, extra: &str) -> Result<(), String> {
        let doc = member(&self.dir, port, extra);
        let config = akit_config::from_json(&doc, None).unwrap_or_else(|e| panic!("{doc}\n{e}"));
        let errs = self.roles.apply(&config);
        if errs.is_empty() {
            Ok(())
        } else {
            Err(errs.join("; "))
        }
    }

    fn api(&self) -> ClientApi {
        ClientApi::of(&self.roles.api(super::ID).unwrap()).unwrap()
    }

    fn downloads(&self) -> PathBuf {
        self.dir.join("data").join("downloads")
    }
}

fn wait_for(what: &str, secs: u64, mut cond: impl FnMut() -> bool) {
    let deadline = Instant::now() + Duration::from_secs(secs);
    while !cond() {
        assert!(Instant::now() < deadline, "timed out waiting for {what}");
        std::thread::sleep(Duration::from_millis(50));
    }
}

fn torrent(payload: &[u8]) -> MetaInfo {
    let mi = Builder::new("t.bin")
        .piece_length(64 * 1024)
        .build_bytes(payload)
        .unwrap();
    MetaInfo::parse(&mi.to_bytes()).unwrap()
}

/// A seeder role holding `payload` as `t.bin`, its torrent added, seeding.
fn seeder(name: &str, payload: &[u8]) -> (Env, MetaInfo, String) {
    let dir = tmp(name);
    let env = Env::start(&dir, free_port(), "");
    std::fs::write(env.downloads().join("t.bin"), payload).unwrap();
    let mi = torrent(payload);
    let s = env
        .api()
        .add(Source::Torrent(mi.to_bytes()), AddRequest::default())
        .unwrap();
    let id = s.id.clone();
    let api = env.api();
    wait_for("the seeder's check", 20, || {
        api.get(&id).is_ok_and(|s| s.state == "seeding")
    });
    (env, mi, id)
}

/// fit's `TestDownload`, `TestRestartResumes`: a download between two roles, the leecher
/// restarted resuming without reading its data, the list kept with the torrent complete.
#[test]
fn download_and_restart_without_a_check() {
    let payload = data(2 << 20, 5);
    let (seed, mi, id) = seeder("dl-seed", &payload);
    let seed_addr = seed.api().listen_addr().unwrap();
    let dir = tmp("dl-leech");
    let port = free_port();
    let mut leech = Env::start(&dir, port, "");
    let req = AddRequest {
        peers: vec![seed_addr],
        ..AddRequest::default()
    };
    leech
        .api()
        .add(Source::Torrent(mi.to_bytes()), req)
        .unwrap();
    let api = leech.api();
    wait_for("the download", 60, || {
        api.get(&id).is_ok_and(|s| s.complete)
    });
    assert!(std::fs::read(leech.downloads().join("t.bin")).unwrap() == payload);
    // The saver notes the completion.
    wait_for("the completion saved", 10, || {
        std::fs::read_to_string(dir.join("data/torrents.json"))
            .is_ok_and(|s| compact(&s).contains("\"completed\":true"))
    });
    assert!(
        dir.join("data/torrents")
            .join(format!("{id}.torrent"))
            .exists()
    );
    leech.roles.stop(Duration::ZERO);
    assert!(
        dir.join("data/resume")
            .join(format!("{id}.resume"))
            .exists()
    );
    drop(leech);
    let leech = Env::start(&dir, port, "");
    let api = leech.api();
    wait_for("the restart", 20, || {
        api.get(&id).is_ok_and(|s| s.state == "seeding")
    });
    let s = api.get(&id).unwrap();
    assert_eq!(s.pieces_checked, 0, "a restart read the data again: {s:?}");
    assert!(s.complete);
    assert!(s.added.ends_with('Z'), "{}", s.added);
}

/// fit's `TestMagnetSaved`: a torrent added by magnet link gets its metadata from a peer, its
/// .torrent is saved, and a restart restores it from the file.
#[test]
fn a_magnet_links_torrent_is_kept() {
    let payload = data(1 << 20, 9);
    let (seed, mi, id) = seeder("mag-seed", &payload);
    let dir = tmp("mag-leech");
    let port = free_port();
    let leech = Env::start(&dir, port, "");
    let req = AddRequest {
        peers: vec![seed.api().listen_addr().unwrap()],
        ..AddRequest::default()
    };
    let s = leech
        .api()
        .add(Source::Magnet(mi.magnet().to_string()), req)
        .unwrap();
    assert_eq!(s.id, id);
    let api = leech.api();
    wait_for("the magnet's download", 60, || {
        api.get(&id).is_ok_and(|s| s.complete)
    });
    let path = dir.join("data/torrents").join(format!("{id}.torrent"));
    wait_for("its .torrent saved", 10, || path.exists());
    assert_eq!(
        MetaInfo::parse(&std::fs::read(&path).unwrap())
            .unwrap()
            .info_hash,
        mi.info_hash
    );
    drop(leech);
    let leech = Env::start(&dir, port, "");
    let s = leech.api().get(&id).unwrap();
    assert_eq!(s.name, "t.bin");
    assert!(s.state != "error", "{s:?}");
}

/// fit's `TestReloadLimits` and `TestListenChange`: new rates keep the client; a port taken
/// brings the old client back; a new `listen` makes a new client that keeps the torrents.
#[test]
fn reloads_in_place_and_by_a_new_client() {
    let payload = data(512 << 10, 3);
    let (mut seed, _mi, id) = seeder("reload", &payload);
    let api = seed.api();
    let addr = api.listen_addr().unwrap();
    let port = addr.port();
    seed.apply(
        port,
        r#", "limits": {"upload_rate": 1048576, "seed_ratio": 5}"#,
    )
    .unwrap();
    assert_eq!(
        api.listen_addr(),
        Some(addr),
        "a reload of limits made a new client"
    );
    assert_eq!(api.get(&id).unwrap().state, "seeding");
    // A port taken: the reload fails, the old client back.
    let taken = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let tp = taken.local_addr().unwrap().port();
    let e = seed.apply(tp, "").unwrap_err();
    assert!(e.contains("listen on"), "{e}");
    assert_eq!(api.listen_addr().unwrap().port(), port);
    wait_for("the torrent back", 20, || {
        api.get(&id).is_ok_and(|s| s.state == "seeding")
    });
    // A new port: a new client with the torrent, resumed without a check.
    let p2 = free_port();
    seed.apply(p2, "").unwrap();
    assert_eq!(api.listen_addr().unwrap().port(), p2);
    wait_for("the torrent moved", 20, || {
        api.get(&id).is_ok_and(|s| s.state == "seeding")
    });
    assert_eq!(api.get(&id).unwrap().pieces_checked, 0);
}

/// fit's `TestBadState`: a state file that does not read keeps the role from starting, rather
/// than be overwritten; temporary files a crash left are removed.
#[test]
fn a_bad_state_keeps_the_role_from_starting() {
    let h = "ab".repeat(20);
    for (doc, want) in [
        ("{nope".to_owned(), "the state cannot be read"),
        (
            r#"{"version": 2, "torrents": []}"#.to_owned(),
            "of version 2",
        ),
        (
            format!(
                r#"{{"version": 1, "torrents": [{{"id": "{h}", "dir": "/x"}}, {{"id": "{h}", "dir": "/x"}}]}}"#
            ),
            "twice",
        ),
        (
            format!(r#"{{"version": 1, "torrents": [{{"id": "{h}", "dir": "rel"}}]}}"#),
            "not absolute",
        ),
        (
            r#"{"version": 1, "torrents": [{"id": "XYZ", "dir": "/x"}]}"#.to_owned(),
            "40 lowercase hex digits",
        ),
    ] {
        let dir = tmp("bad");
        std::fs::create_dir_all(dir.join("data")).unwrap();
        std::fs::write(dir.join("data/torrents.json"), &doc).unwrap();
        std::fs::write(dir.join("data/.torrents.json.tmp-1"), "x").unwrap();
        let engine = akio::Engine::builder().build().unwrap();
        engine.start().unwrap();
        let engine = Arc::new(engine);
        let mut roles = Manager::new(&[CLIENT], Arc::clone(&engine), None);
        let config = akit_config::from_json(&member(&dir, free_port(), ""), None).unwrap();
        let errs = roles.apply(&config);
        assert!(errs.iter().any(|e| e.contains(want)), "{doc}: {errs:?}");
        assert!(roles.running().is_empty());
        assert_eq!(
            std::fs::read_to_string(dir.join("data/torrents.json")).unwrap(),
            doc
        );
        assert!(!dir.join("data/.torrents.json.tmp-1").exists());
        engine.shutdown();
        engine.wait();
    }
}

/// fit's `TestBrokenTorrentKept` and its state file read and written back: a torrent whose
/// .torrent is gone and that has no magnet link stays listed with its error; members akit does
/// not read (a category, own limits, `alt_speed`) are written back as fit wrote them.
#[test]
fn fits_state_file_and_a_broken_torrent() {
    let dir = tmp("fitstate");
    let h = "cd".repeat(20);
    let dl = dir.join("dl");
    std::fs::create_dir_all(dir.join("data")).unwrap();
    std::fs::write(
        dir.join("data/torrents.json"),
        format!(
            r#"{{"version": 1, "saved": "2026-11-09T00:00:00Z", "alt_speed": "on", "torrents": [{{"id": "{h}", "name": "gone", "dir": {:?}, "added": "2026-11-09T00:00:00Z", "category": "films", "limits": {{"upload_rate": 5}}, "priorities": [1, 0]}}]}}"#,
            dl.to_str().unwrap()
        ),
    )
    .unwrap();
    let env = Env::start(&dir, free_port(), "");
    let s = env.api().get(&h).unwrap();
    assert_eq!(s.state, "error");
    assert!(
        s.error
            .contains("neither its .torrent file nor a magnet link"),
        "{}",
        s.error
    );
    // A change of the list writes the file back with what akit does not read.
    let payload = data(64 << 10, 1);
    env.api()
        .add(
            Source::Torrent(torrent(&payload).to_bytes()),
            AddRequest {
                paused: true,
                ..AddRequest::default()
            },
        )
        .unwrap();
    let text = compact(&std::fs::read_to_string(dir.join("data/torrents.json")).unwrap());
    for want in [
        r#""alt_speed":"on""#,
        r#""category":"films""#,
        r#""limits":{"upload_rate":5}"#,
        r#""priorities":[1,0]"#,
        r#""name":"gone""#,
        r#""paused":true"#,
        r#""version":1"#,
    ] {
        assert!(text.contains(want), "{want} not in {text}");
    }
}

/// fit's `TestAPIRefusals`: bad input, max_torrents, no such torrent; pause, resume, the
/// console's routes and removal with the data.
#[test]
fn the_apis_refusals_and_removal() {
    let dir = tmp("api");
    let env = Env::start(&dir, free_port(), r#", "limits": {"max_torrents": 1}"#);
    let api = env.api();
    assert!(matches!(
        api.add(Source::Torrent(b"nope".to_vec()), AddRequest::default()),
        Err(ApiError::BadRequest(_))
    ));
    assert!(matches!(
        api.add(Source::Magnet("magnet:?nope".into()), AddRequest::default()),
        Err(ApiError::BadRequest(_))
    ));
    let payload = data(128 << 10, 2);
    std::fs::write(env.downloads().join("t.bin"), &payload).unwrap();
    let mi = torrent(&payload);
    let s = api
        .add(Source::Torrent(mi.to_bytes()), AddRequest::default())
        .unwrap();
    let other = torrent(&data(1000, 4));
    assert!(matches!(
        api.add(Source::Torrent(other.to_bytes()), AddRequest::default()),
        Err(ApiError::TooMany)
    ));
    assert_eq!(api.get(&"00".repeat(20)), Err(ApiError::NotFound));
    assert_eq!(api.remove(&"00".repeat(20), false), Err(ApiError::NotFound));
    wait_for("seeding", 20, || {
        api.get(&s.id).is_ok_and(|s| s.state == "seeding")
    });
    assert!(api.set_paused(&s.id, true).unwrap().paused);
    wait_for("paused", 10, || {
        api.get(&s.id).is_ok_and(|s| s.state == "paused")
    });
    api.set_paused(&s.id, false).unwrap();
    wait_for("resumed", 10, || {
        api.get(&s.id).is_ok_and(|s| s.state == "seeding")
    });
    // The pages.
    let pages = super::pages::Pages::new(api.clone());
    let r = pages.answer("/api/torrents");
    let body = String::from_utf8(r.body).unwrap();
    assert!(
        body.contains(&format!(r#""id":"{}""#, s.id)) && body.contains(r#""total":1"#),
        "{body}"
    );
    assert_eq!(pages.answer(&format!("/api/torrents/{}", s.id)).status, 200);
    assert_eq!(pages.answer("/api/torrents/nope").status, 404);
    // Removed with its data.
    api.remove(&s.id, true).unwrap();
    assert!(!env.downloads().join("t.bin").exists());
    assert!(api.list().unwrap().is_empty());
    assert!(
        !dir.join("data/torrents")
            .join(format!("{}.torrent", s.id))
            .exists()
    );
}
