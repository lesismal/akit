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
        // Client threads for .torrent URLs.
        let engine = akio::Engine::builder().client_threads(1).build().unwrap();
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

fn named(name: &str, payload: &[u8]) -> MetaInfo {
    let mi = Builder::new(name)
        .piece_length(64 * 1024)
        .build_bytes(payload)
        .unwrap();
    MetaInfo::parse(&mi.to_bytes()).unwrap()
}

/// fit's `TestCategories` (I153): a torrent added in a category downloads into its directory,
/// a magnet link's too once its metadata came (the stopgap for akio's A286); an unknown one is
/// refused; the category is kept across a restart.
#[test]
fn categories_and_a_magnet_links_directory() {
    let payload = data(1 << 20, 41);
    let (seed, mi, id) = seeder("cat-seed", &payload);
    let peer = seed.api().listen_addr().unwrap();
    let dir = tmp("cat-leech");
    let port = free_port();
    let cats = r#", "categories": [{"name": "films", "dir": "f"}, {"name": "iso"}]"#;
    let leech = Env::start(&dir, port, cats);
    let api = leech.api();
    assert!(matches!(
        api.add(
            Source::Torrent(mi.to_bytes()),
            AddRequest {
                category: "nope".into(),
                ..AddRequest::default()
            }
        ),
        Err(ApiError::BadRequest(_))
    ));
    let s = api
        .add(
            Source::Magnet(mi.magnet().to_string()),
            AddRequest {
                peers: vec![peer],
                category: "films".into(),
                ..AddRequest::default()
            },
        )
        .unwrap();
    assert_eq!(s.category, "films");
    assert!(s.dir.ends_with("downloads/f"), "{}", s.dir);
    wait_for("the magnet's download", 60, || {
        api.get(&id).is_ok_and(|s| s.complete)
    });
    let f = leech.downloads().join("f/t.bin");
    assert!(std::fs::read(&f).unwrap() == payload);
    assert!(
        !leech.downloads().join("t.bin").exists(),
        "left in download_dir"
    );
    // A .torrent in the other category.
    let p2 = data(256 << 10, 42);
    let mi2 = named("two.bin", &p2);
    std::fs::create_dir_all(leech.downloads().join("iso")).unwrap();
    std::fs::write(leech.downloads().join("iso/two.bin"), &p2).unwrap();
    let s2 = api
        .add(
            Source::Torrent(mi2.to_bytes()),
            AddRequest {
                category: "iso".into(),
                ..AddRequest::default()
            },
        )
        .unwrap();
    wait_for("the second's check", 20, || {
        api.get(&s2.id).is_ok_and(|s| s.state == "seeding")
    });
    drop(leech);
    let leech = Env::start(&dir, port, cats);
    assert_eq!(leech.api().get(&id).unwrap().category, "films");
    assert_eq!(leech.api().get(&s2.id).unwrap().category, "iso");
}

/// fit's `TestQueue` (I153): past max_active_downloads a torrent is queued, and starts when
/// the one ahead is removed; a queue position moves one ahead; past max_active_seeds a seed is
/// queued, and starts when the one ahead is paused.
#[test]
fn the_queue() {
    let dir = tmp("queue");
    let env = Env::start(
        &dir,
        free_port(),
        r#", "limits": {"max_active_downloads": 1, "max_active_seeds": 1}"#,
    );
    let api = env.api();
    // Two downloads with no peer: they download for ever.
    let a = api
        .add(
            Source::Torrent(named("a.bin", &data(100_000, 1)).to_bytes()),
            AddRequest::default(),
        )
        .unwrap();
    let b = api
        .add(
            Source::Torrent(named("b.bin", &data(100_000, 2)).to_bytes()),
            AddRequest::default(),
        )
        .unwrap();
    wait_for("a downloading", 10, || {
        api.get(&a.id).is_ok_and(|s| s.state == "downloading")
    });
    let st = api.get(&b.id).unwrap();
    assert_eq!(
        (st.state.as_str(), st.held.as_str(), st.queue_position),
        ("queued", "queued", 1)
    );
    // b moved ahead: a is queued.
    api.set_queue_position(&b.id, 0).unwrap();
    wait_for("b downloading", 10, || {
        api.get(&b.id).is_ok_and(|s| s.state == "downloading")
    });
    assert_eq!(api.get(&a.id).unwrap().held, "queued");
    // b removed: a starts.
    api.remove(&b.id, false).unwrap();
    wait_for("a downloading again", 10, || {
        api.get(&a.id).is_ok_and(|s| s.state == "downloading")
    });
    // Seeds: two complete torrents, one seeding at once.
    for (n, seed) in [("s1.bin", 3), ("s2.bin", 4)] {
        std::fs::write(env.downloads().join(n), data(80_000, seed)).unwrap();
    }
    let s1 = api
        .add(
            Source::Torrent(named("s1.bin", &data(80_000, 3)).to_bytes()),
            AddRequest::default(),
        )
        .unwrap();
    let s2 = api
        .add(
            Source::Torrent(named("s2.bin", &data(80_000, 4)).to_bytes()),
            AddRequest::default(),
        )
        .unwrap();
    // A seed first checks its data, paused or not: then the queue holds the second.
    wait_for("s1 seeding", 20, || {
        api.get(&s1.id).is_ok_and(|s| s.state == "seeding")
    });
    wait_for("s2 queued", 20, || {
        api.get(&s2.id)
            .is_ok_and(|s| s.state == "queued" && s.complete)
    });
    api.set_paused(&s1.id, true).unwrap();
    wait_for("s2 seeding", 20, || {
        api.get(&s2.id).is_ok_and(|s| s.state == "seeding")
    });
}

/// fit's `TestDiskFree` (I153): a download is not started short of min_free_bytes, and starts
/// once a reload asks for less; one running is held when a reload asks for more than is free.
#[test]
fn the_disk_check() {
    let dir = tmp("disk");
    let port = free_port();
    let huge = r#", "limits": {"min_free_bytes": 9000000000000000000}"#;
    let mut env = Env::start(&dir, port, huge);
    let api = env.api();
    let s = api
        .add(
            Source::Torrent(named("d.bin", &data(100_000, 5)).to_bytes()),
            AddRequest::default(),
        )
        .unwrap();
    let st = api.get(&s.id).unwrap();
    assert_eq!(st.held, "disk", "{st:?}");
    assert!(st.error.contains("min_free_bytes"), "{}", st.error);
    env.apply(port, r#", "limits": {"min_free_bytes": 1}"#)
        .unwrap();
    wait_for("started", 10, || {
        api.get(&s.id).is_ok_and(|s| s.state == "downloading")
    });
    env.apply(port, huge).unwrap();
    wait_for("held for the disk", 10, || {
        api.get(&s.id)
            .is_ok_and(|s| s.held == "disk" && s.state == "paused")
    });
}

/// fit's `TestWatchDir` (I153): .torrent and .magnet files taken once settled, a category's
/// subdirectory, bad and empty files set aside with why, hidden and other files left alone, one
/// already held counted as done.
#[test]
fn watch_dir() {
    let dir = tmp("watch");
    let w = dir.join("w");
    std::fs::create_dir_all(w.join("films")).unwrap();
    let env = Env::start(
        &dir,
        free_port(),
        &format!(
            r#", "watch_dir": {:?}, "watch_interval": "100ms", "categories": [{{"name": "films"}}]"#,
            w.to_str().unwrap()
        ),
    );
    let api = env.api();
    let one = named("one.bin", &data(50_000, 6));
    let two = named("two.bin", &data(50_000, 7));
    std::fs::write(w.join("one.torrent"), one.to_bytes()).unwrap();
    std::fs::write(
        w.join("films/two.magnet"),
        format!("\n  {}\n", two.magnet()),
    )
    .unwrap();
    std::fs::write(w.join("bad.torrent"), b"nope").unwrap();
    std::fs::write(w.join("empty.magnet"), b"\n\n").unwrap();
    std::fs::write(w.join(".hidden.torrent"), one.to_bytes()).unwrap();
    std::fs::write(w.join("other.txt"), b"x").unwrap();
    wait_for("both added", 20, || api.list().is_ok_and(|l| l.len() == 2));
    let l = api.list().unwrap();
    assert!(
        l.iter()
            .any(|s| s.name == "one.bin" && s.category.is_empty())
    );
    assert!(
        l.iter()
            .any(|s| s.id == two.info_hash.to_hex() && s.category == "films")
    );
    wait_for("set aside", 10, || {
        w.join(".failed/bad.torrent").exists() && w.join(".failed/empty.magnet").exists()
    });
    assert!(w.join(".done/one.torrent").exists() && w.join("films/.done/two.magnet").exists());
    assert!(
        std::fs::read_to_string(w.join(".failed/bad.torrent.error"))
            .unwrap()
            .contains("bad request")
            || std::fs::read_to_string(w.join(".failed/bad.torrent.error"))
                .unwrap()
                .len()
                > 1
    );
    assert!(w.join(".hidden.torrent").exists() && w.join("other.txt").exists());
    // One already held: done.
    std::fs::write(w.join("again.torrent"), one.to_bytes()).unwrap();
    wait_for("the duplicate done", 10, || {
        w.join(".done/again.torrent").exists()
    });
    assert_eq!(api.list().unwrap().len(), 2);
}

/// A server of the test's own: each request's path answered by `routes`, the connection closed.
fn http_server(routes: Vec<(&'static str, Vec<u8>)>) -> std::net::SocketAddr {
    use std::io::{Read, Write};
    let l = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = l.local_addr().unwrap();
    std::thread::spawn(move || {
        for s in l.incoming() {
            let Ok(mut s) = s else { break };
            let routes = routes.clone();
            std::thread::spawn(move || {
                let mut head = Vec::new();
                let mut b = [0u8; 1024];
                while !head.windows(4).any(|w| w == b"\r\n\r\n") {
                    match s.read(&mut b) {
                        Ok(0) | Err(_) => return,
                        Ok(n) => head.extend_from_slice(&b[..n]),
                    }
                }
                let text = String::from_utf8_lossy(&head);
                let path = text.split(' ').nth(1).unwrap_or("").to_owned();
                if let Some((_, resp)) = routes.iter().find(|(p, _)| *p == path) {
                    if resp.is_empty() {
                        // Never answers.
                        std::thread::sleep(Duration::from_secs(5));
                        return;
                    }
                    let _ = s.write_all(resp);
                } else {
                    let _ = s.write_all(
                        b"HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\nConnection: close\r\n\r\n",
                    );
                }
            });
        }
    });
    addr
}

/// fit's `TestAddURL` (I153): a .torrent by URL, through a redirect, a redirect to a magnet
/// link; a bad scheme, a 404, a redirect loop, a server that never answers within url_timeout,
/// a body past the bound.
#[test]
fn add_by_url() {
    let dir = tmp("url");
    let env = Env::start(&dir, free_port(), r#", "url_timeout": "500ms""#);
    let api = env.api();
    let a = named("a.bin", &data(50_000, 8));
    let b = named("b.bin", &data(50_000, 9));
    let ok = |body: &[u8]| {
        let mut r = format!(
            "HTTP/1.1 200 OK\r\nContent-Type: application/x-bittorrent\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
            body.len()
        )
        .into_bytes();
        r.extend_from_slice(body);
        r
    };
    let redirect = |to: &str| {
        format!(
            "HTTP/1.1 302 Found\r\nLocation: {to}\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"
        )
        .into_bytes()
    };
    let big =
        b"HTTP/1.1 200 OK\r\nContent-Length: 40000000\r\nConnection: close\r\n\r\nxxxx".to_vec();
    let addr = http_server(vec![
        ("/a.torrent", ok(&a.to_bytes())),
        ("/go", redirect("/a.torrent")),
        ("/mag", redirect(&b.magnet().to_string())),
        ("/loop", redirect("/loop")),
        ("/slow", Vec::new()),
        ("/big", big),
    ]);
    let url = |p: &str| format!("http://{addr}{p}");
    let s = api
        .add_url(&url("/a.torrent"), AddRequest::default())
        .unwrap();
    assert_eq!(s.name, "a.bin");
    api.remove(&s.id, false).unwrap();
    assert_eq!(
        api.add_url(&url("/go"), AddRequest::default())
            .unwrap()
            .name,
        "a.bin"
    );
    assert_eq!(
        api.add_url(&url("/mag"), AddRequest::default()).unwrap().id,
        b.info_hash.to_hex()
    );
    assert!(matches!(
        api.add_url("ftp://x/y", AddRequest::default()),
        Err(ApiError::BadRequest(_))
    ));
    for (p, want) in [
        ("/nope", "HTTP 404"),
        ("/loop", "redirects"),
        ("/slow", "timed out"),
        ("/big", ""),
    ] {
        match api.add_url(&url(p), AddRequest::default()) {
            Err(ApiError::Fetch(why)) => assert!(why.contains(want), "{p}: {why}"),
            r => panic!("{p}: {r:?}"),
        }
    }
}

/// fit's `TestTorrentLimits` and `TestAltSpeed` (I154): a category's limits on its torrents, a
/// torrent's own over them, kept across a restart; the alternative rates turned on and off by
/// the API, following a schedule that holds all day, the choice kept across a restart, and a
/// reload without `alt_speed` going back to the client's rates.
#[test]
fn limits_and_alt_speed() {
    use akit_config::bittorrent::BitTorrentTorrentLimits;
    let dir = tmp("limits");
    let port = free_port();
    let cfg = r#", "limits": {"upload_rate": 1000, "download_rate": 2000}, "categories": [{"name": "c", "limits": {"upload_rate": 300, "download_rate": -1}}], "alt_speed": {"upload_rate": 10, "download_rate": 20, "schedules": [{"from": "00:00", "to": "24:00"}], "time_zone": "utc"}"#;
    let mut env = Env::start(&dir, port, cfg);
    let api = env.api();
    // The schedule holds all day: the alternative rates hold.
    assert_eq!(api.alt_speed().unwrap(), ("auto".to_owned(), true));
    assert_eq!(api.client_rates(), Some((Some(10), Some(20))));
    assert_eq!(api.set_alt_speed("off").unwrap(), ("off".to_owned(), false));
    assert_eq!(api.client_rates(), Some((Some(1000), Some(2000))));
    assert!(matches!(
        api.set_alt_speed("nope"),
        Err(ApiError::BadRequest(_))
    ));
    let s = api
        .add(
            Source::Torrent(named("l.bin", &data(50_000, 11)).to_bytes()),
            AddRequest {
                category: "c".into(),
                ..AddRequest::default()
            },
        )
        .unwrap();
    assert_eq!(api.torrent_rates(&s.id), Some((Some(300), None)));
    let own = BitTorrentTorrentLimits {
        download_rate: 77,
        ..BitTorrentTorrentLimits::default()
    };
    api.set_torrent_limits(&s.id, own).unwrap();
    assert_eq!(api.torrent_rates(&s.id), Some((Some(300), Some(77))));
    drop(env);
    env = Env::start(&dir, port, cfg);
    let api = env.api();
    assert_eq!(
        api.alt_speed().unwrap(),
        ("off".to_owned(), false),
        "the choice kept"
    );
    assert_eq!(
        api.torrent_rates(&s.id),
        Some((Some(300), Some(77))),
        "the limits kept"
    );
    assert_eq!(api.get(&s.id).unwrap().category, "c");
    api.set_alt_speed("on").unwrap();
    assert_eq!(api.client_rates(), Some((Some(10), Some(20))));
    // A reload without alt_speed: the client's rates, whatever the choice.
    env.apply(port, r#", "limits": {"upload_rate": 1000, "download_rate": 2000}, "categories": [{"name": "c"}]"#)
        .unwrap();
    assert_eq!(api.client_rates(), Some((Some(1000), Some(2000))));
    assert_eq!(api.torrent_rates(&s.id), Some((None, Some(77))));
    // A zone that does not load refuses the reload.
    let e = env
        .apply(port, r#", "alt_speed": {"time_zone": "Nowhere/Nothing"}"#)
        .unwrap_err();
    assert!(e.contains("time_zone"), "{e}");
}

/// What follows an event does not wait for the manager's tick (the no-timer rule): with a
/// manager that never ticks, past max_active_downloads the second torrent is queued, and starts
/// and completes once the first completes, on the completion's event alone.
#[test]
fn the_queue_moves_on_the_completion_not_the_tick() {
    let p1 = data(512 << 10, 51);
    let p2 = data(512 << 10, 52);
    let (seed, mi1, id1) = seeder("evq-seed", &p1);
    std::fs::write(seed.downloads().join("two.bin"), &p2).unwrap();
    let mi2 = named("two.bin", &p2);
    let id2 = seed
        .api()
        .add(Source::Torrent(mi2.to_bytes()), AddRequest::default())
        .unwrap()
        .id;
    let sapi = seed.api();
    wait_for("the second seed", 20, || {
        sapi.get(&id2).is_ok_and(|s| s.state == "seeding")
    });
    let peer = sapi.listen_addr().unwrap();
    let dir = tmp("evq-leech");
    let engine = akio::Engine::builder().client_threads(1).build().unwrap();
    engine.start().unwrap();
    let engine = Arc::new(engine);
    let mut roles = Manager::new(&[super::CLIENT_NO_TICK], Arc::clone(&engine), None);
    let doc = member(
        &dir,
        free_port(),
        r#", "limits": {"max_active_downloads": 1, "min_free_bytes": 1}"#,
    );
    assert!(
        roles
            .apply(&akit_config::from_json(&doc, None).unwrap())
            .is_empty()
    );
    let api = ClientApi::of(&roles.api(super::ID).unwrap()).unwrap();
    let req = || AddRequest {
        peers: vec![peer],
        ..AddRequest::default()
    };
    // The first paced (128 KiB a second for 512 KiB), so that it is still downloading when the
    // second is added and queued behind it.
    let paced = AddRequest {
        limits: akit_config::bittorrent::BitTorrentTorrentLimits {
            download_rate: 128 << 10,
            ..Default::default()
        },
        ..req()
    };
    api.add(Source::Torrent(mi1.to_bytes()), paced).unwrap();
    api.add(Source::Torrent(mi2.to_bytes()), req()).unwrap();
    let held = api.get(&id2).unwrap();
    assert!(
        !api.get(&id1).unwrap().complete,
        "the first completed before the second came"
    );
    assert_eq!(held.held, "queued", "{held:?}");
    wait_for("the first, then the second on its completion", 60, || {
        api.get(&id1).is_ok_and(|s| s.complete) && api.get(&id2).is_ok_and(|s| s.complete)
    });
    roles.stop(Duration::ZERO);
    engine.shutdown();
    engine.wait();
}

/// A GET (or `method`) of `target` at `addr`: the status and body.
fn http_get(addr: std::net::SocketAddr, method: &str, target: &str) -> (u16, Vec<u8>) {
    use std::io::{Read, Write};
    let mut s = std::net::TcpStream::connect(addr).unwrap();
    s.set_read_timeout(Some(Duration::from_secs(10))).unwrap();
    write!(
        s,
        "{method} {target} HTTP/1.1\r\nHost: t\r\nConnection: close\r\n\r\n"
    )
    .unwrap();
    let mut out = Vec::new();
    s.read_to_end(&mut out).unwrap();
    let status = String::from_utf8_lossy(&out[9..12]).parse().unwrap();
    let at = out.windows(4).position(|w| w == b"\r\n\r\n").unwrap() + 4;
    (status, out[at..].to_vec())
}

/// fit's `TestPublish` (I156): each file of a directory published as a torrent once settled,
/// dotfiles and symbolic links left out, downloaded by another role from the publisher alone;
/// `bt_publish`'s index, .torrent and magnet link; a changed file getting a new torrent and the
/// old one stopping; a restart building nothing; whole_dir; an entry removed taking its state.
#[test]
fn publishing() {
    let dir = tmp("pub");
    let src = dir.join("src");
    std::fs::create_dir_all(src.join("sub")).unwrap();
    let (a, b) = (data(300_000, 61), data(200_000, 62));
    std::fs::write(src.join("a.bin"), &a).unwrap();
    std::fs::write(src.join("sub/b.bin"), &b).unwrap();
    std::fs::write(src.join(".hidden"), b"x").unwrap();
    #[cfg(unix)]
    std::os::unix::fs::symlink(src.join("a.bin"), src.join("link.bin")).unwrap();
    let entry = |extra: &str| {
        let ws = if extra.contains("web_seed") {
            ""
        } else {
            r#", "web_seed": "http://127.0.0.1:1/rel/""#
        };
        format!(
            r#", "publish": [{{"name": "rel", "dir": {:?}, "check_interval": "100ms"{ws}{extra}}}]"#,
            src.to_str().unwrap()
        )
    };
    let port = free_port();
    let mut env = Env::start(&dir, port, &entry(""));
    let http: std::net::SocketAddr = format!("127.0.0.1:{}", free_port()).parse().unwrap();
    env.engine
        .add_listener(akio::Listener::tcp(http).serve(akio_http::serve(
            super::publish_handler::service(
                akit_runtime::upstream::EngineSlot::with(Arc::clone(&env.engine)),
                Vec::new(),
            ),
        )))
        .unwrap();
    let index = || {
        let (code, body) = http_get(http, "GET", "/rel/");
        assert_eq!(code, 200);
        String::from_utf8(body).unwrap()
    };
    wait_for("both published", 20, || {
        index().matches("\"state\":\"seeding\"").count() == 2
    });
    let idx = index();
    assert!(
        idx.contains("\"path\":\"a.bin\"") && idx.contains("\"path\":\"sub/b.bin\""),
        "{idx}"
    );
    assert!(
        !idx.contains("hidden") && !idx.contains("link.bin"),
        "{idx}"
    );
    assert!(
        idx.contains("\"web_seed\":\"http://127.0.0.1:1/rel/\""),
        "{idx}"
    );
    let id_of = |idx: &str, path: &str| {
        let at = idx.find(&format!("\"path\":\"{path}\"")).unwrap();
        let before = &idx[..at];
        let k = before.rfind("\"id\":\"").unwrap() + 6;
        before[k..k + 40].to_owned()
    };
    let ida = id_of(&idx, "a.bin");
    let (code, tb) = http_get(http, "GET", &format!("/rel/{ida}.torrent"));
    assert_eq!(code, 200);
    let mi = MetaInfo::parse(&tb).unwrap();
    assert_eq!(mi.info.name, "a.bin");
    assert!(mi.info_hash_v2.is_some(), "hybrid");
    assert_eq!(mi.url_list, vec!["http://127.0.0.1:1/rel/a.bin".to_owned()]);
    let (code, mag) = http_get(http, "GET", &format!("/rel/{ida}.magnet"));
    assert!(code == 200 && String::from_utf8_lossy(&mag).starts_with("magnet:?"));
    assert_eq!(http_get(http, "GET", "/rel/nope.torrent").0, 404);
    assert_eq!(http_get(http, "POST", "/rel/").0, 405);
    // Another role downloads a.bin from the publisher alone.
    let leech = Env::start(&tmp("pub-leech"), free_port(), "");
    let lapi = leech.api();
    let req = AddRequest {
        peers: vec![env.api().listen_addr().unwrap()],
        ..AddRequest::default()
    };
    lapi.add(Source::Torrent(tb.clone()), req).unwrap();
    wait_for("the download from the publisher", 60, || {
        lapi.get(&ida).is_ok_and(|s| s.complete)
    });
    assert!(std::fs::read(leech.downloads().join("a.bin")).unwrap() == a);
    // The published torrents are not the role's list.
    assert!(env.api().list().unwrap().is_empty());
    // b changed: a new torrent, the old one gone.
    let idb = id_of(&idx, "sub/b.bin");
    // Another seed: data() takes `seed | 1`, so 62 and 63 would make the same bytes.
    std::fs::write(src.join("sub/b.bin"), data(200_000, 65)).unwrap();
    wait_for("b replaced", 20, || {
        let i = index();
        i.contains("\"path\":\"sub/b.bin\"")
            && !i.contains(&idb)
            && i.matches("\"state\":\"seeding\"").count() == 2
    });
    assert_eq!(http_get(http, "GET", &format!("/rel/{idb}.torrent")).0, 404);
    // A restart builds nothing: a.bin's torrent kept, its build time the same.
    let built = |i: &str| {
        let at = i.find(&format!("\"id\":\"{ida}\"")).unwrap();
        let k = i[at..].find("\"built\":\"").unwrap() + at + 9;
        i[k..k + 20].to_owned()
    };
    let before = built(&index());
    env.roles.stop(Duration::ZERO);
    assert_eq!(http_get(http, "GET", "/rel/").0, 503);
    env.roles = Manager::new(&[CLIENT], Arc::clone(&env.engine), None);
    env.apply(port, &entry("")).unwrap();
    wait_for("restored", 20, || {
        let i = index();
        i.contains(&ida) && i.matches("\"state\":\"seeding\"").count() == 2
    });
    assert_eq!(built(&index()), before, "a restart built a.bin again");
    // whole_dir: a directory with files left out is reported, not published.
    env.apply(
        port,
        &entry(r#", "mode": "whole_dir", "web_seed": "http://127.0.0.1:1/x/src""#),
    )
    .unwrap();
    wait_for("reported", 20, || index().contains("files left out"));
    std::fs::remove_file(src.join(".hidden")).unwrap();
    #[cfg(unix)]
    std::fs::remove_file(src.join("link.bin")).unwrap();
    wait_for("whole_dir published", 20, || {
        let i = index();
        i.contains("\"path\":\"\"")
            && i.contains("\"files\":2")
            && i.contains("\"state\":\"seeding\"")
    });
    // The entry removed: its state gone, its files kept.
    env.apply(port, "").unwrap();
    assert!(!dir.join("data/publish/rel").exists());
    assert!(src.join("a.bin").exists());
}
