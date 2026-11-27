//! fit's `bittorrent/state_test.go` `TestConfigChecks` and `config/bittorrent_tracker_test.go`,
//! ported (I150), with the fields akit refuses as not supported yet.

use super::*;
use crate::from_json;

fn refused(doc: &str, want: &str) {
    match from_json(doc, None) {
        Ok(_) => panic!("{doc:.200}: loaded, want {want:?}"),
        Err(e) => {
            let s = e.to_string();
            assert!(s.contains(want), "{doc:.200}: {s}, want {want:?}");
        }
    }
}

fn loads(doc: &str) -> crate::Config {
    match from_json(doc, None) {
        Ok(c) => c,
        Err(e) => panic!("{doc:.200}: refused: {e}"),
    }
}

/// fit's `TestConfigChecks`: what loading a config refuses of the client, and its defaults.
#[test]
fn client_checks() {
    for (doc, want) in [
        (
            r#"{"bittorrent": {"client": {"listen": "nope", "data_dir": "d"}}}"#,
            "not a host and port",
        ),
        (
            r#"{"bittorrent": {"client": {"listen": ":6881", "data_dir": " "}}}"#,
            "data_dir may not be empty",
        ),
        (
            r#"{"bittorrent": {"client": {"listen": ":6881", "data_dir": "d", "encryption": "off"}}}"#,
            "encryption",
        ),
        (
            r#"{"bittorrent": {"client": {"listen": ":6881", "data_dir": "d", "limits": {"max_torrents": 100001}}}}"#,
            "max_torrents",
        ),
        (
            r#"{"listeners": [{"name": "web", "address": ":6881"}], "bittorrent": {"client": {"listen": ":6881", "data_dir": "d"}}}"#,
            "listens on TCP",
        ),
        (
            r#"{"bittorrent": {"client": {"data_dir": "d"}}}"#,
            "bittorrent.client.listen: missing required field",
        ),
        (
            r#"{"bittorrent": {"client": {"listen": ":6881"}}}"#,
            "bittorrent.client.data_dir: missing required field",
        ),
        (
            r#"{"bittorrent": {"client": {"listen": ":6881", "data_dir": "d", "save_interval": "-1s"}}}"#,
            "is negative",
        ),
        (
            r#"{"bittorrent": {"client": {"listen": ":6881", "data_dir": "d", "limits": {"seed_time": "-1s"}}}}"#,
            "is negative",
        ),
        (
            r#"{"bittorrent": {"client": {"listen": ":6881", "data_dir": "d", "limits": {"seed_ratio": -1}}}}"#,
            "less than 0",
        ),
        (
            r#"{"bittorrent": {"client": {"listen": ":6881", "data_dir": "d", "disk_workers": 65}}}"#,
            "disk_workers",
        ),
        (
            r#"{"bittorrent": {"client": {"listen": ":6881", "data_dir": "d", "console": {"address": ":0", "users": []}}}}"#,
            "user",
        ),
        (
            r#"{"bittorrent": {"client": {"listen": ":6881", "data_dir": "d", "nope": 1}}}"#,
            "unknown field",
        ),
    ] {
        refused(doc, want);
    }
    let c =
        loads(r#"{"bittorrent": {"client": {"listen": ":6881", "data_dir": "/var/lib/akit/bt"}}}"#);
    let b = c.bittorrent.and_then(|b| b.client).unwrap();
    assert_eq!(b.encryption, BtEncryption::Prefer);
    assert!(b.dht_on() && b.pex_on() && b.utp_on() && !b.lsd);
    assert_eq!(
        b.downloads(),
        std::path::Path::new("/var/lib/akit/bt").join("downloads")
    );
    assert_eq!(b.torrents(), 1000);
    assert_eq!(b.save(), Duration::from_secs(5));
    let l = b.limits_or_default();
    assert_eq!((l.conns_per_torrent(), l.slots()), (50, 4));
    assert!(b.disk_threads() >= 1 && b.disk_threads() <= 8);
    let c = loads(
        r#"{"bittorrent": {"client": {"listen": "127.0.0.1:0", "data_dir": "d", "download_dir": "x", "encryption": "require", "dht": false, "pex": false, "utp": false, "lsd": true, "disk_workers": 3, "save_interval": "1s", "limits": {"upload_rate": 1, "download_rate": 2, "max_conns_per_torrent": 3, "upload_slots": 5, "seed_ratio": 1.5, "seed_time": "1h", "max_torrents": 7}}}}"#,
    );
    let b = c.bittorrent.and_then(|b| b.client).unwrap();
    assert_eq!(b.downloads(), std::path::PathBuf::from("x"));
    assert_eq!(b.encryption, BtEncryption::Require);
    assert!(!b.dht_on() && !b.pex_on() && !b.utp_on() && b.lsd);
    assert_eq!((b.disk_threads(), b.save()), (3, Duration::from_secs(1)));
    let l = b.limits.unwrap();
    assert_eq!(
        (
            l.upload_rate,
            l.download_rate,
            l.conns_per_torrent(),
            l.slots(),
            l.torrents()
        ),
        (1, 2, 3, 5, 7)
    );
    assert!((l.seed_ratio - 1.5).abs() < f64::EPSILON);
    assert_eq!(l.seed_time, Duration::from_secs(3600));
}

/// What a later iteration runs is refused as not supported yet, naming its field.
#[test]
fn not_supported_yet() {
    let client = |m: &str| {
        format!(r#"{{"bittorrent": {{"client": {{"listen": ":6881", "data_dir": "d", {m}}}}}}}"#)
    };
    for (m, field) in [
        (r#""access": {}"#, "bittorrent.client.access"),
        (
            r#""disk_queue_bytes": 1"#,
            "bittorrent.client.disk_queue_bytes",
        ),
        (r#""rss": {}"#, "bittorrent.client.rss"),
        (
            r#""categories": [{"name": "a", "limits": {"max_conns": 1}}]"#,
            "bittorrent.client.categories[0].limits.max_conns",
        ),
        (
            r#""categories": [{"name": "a", "limits": {"upload_slots": 1}}]"#,
            "bittorrent.client.categories[0].limits.upload_slots",
        ),
        (
            r#""categories": [{"name": "a", "media_library": "m"}]"#,
            "bittorrent.client.categories[0].media_library",
        ),
        (
            r#""limits": {"max_conns": 1}"#,
            "bittorrent.client.limits.max_conns",
        ),
        (
            r#""limits": {"max_half_open": 1}"#,
            "bittorrent.client.limits.max_half_open",
        ),
    ] {
        refused(&client(m), &format!("{field}: not supported yet"));
    }
    for (m, field) in [
        (r#""max_peers": 1"#, "bittorrent.tracker.max_peers"),
        (r#""store": {}"#, "bittorrent.tracker.store"),
    ] {
        refused(
            &format!(r#"{{"bittorrent": {{"tracker": {{{m}}}}}}}"#),
            &format!("{field}: not supported yet"),
        );
    }
}

/// fit's `TestBitTorrentTrackerChecks`, but for the fields akit does not read yet (`auth`,
/// `store`, `max_peers`).
#[test]
fn tracker_checks() {
    let tracker = |js: &str| format!(r#"{{"bittorrent": {{"tracker": {js}}}}}"#);
    let h40 = "ab".repeat(20);
    let h64 = "cd".repeat(32);
    for (doc, want) in [
        (tracker(r#"{"listen": "nope"}"#), "is not a host and port"),
        (tracker(r#"{"interval": "-1s"}"#), "is negative"),
        (
            tracker(r#"{"interval": "10m", "min_interval": "20m"}"#),
            "min_interval 20m0s is longer than the interval",
        ),
        (
            tracker(r#"{"interval": "2h"}"#),
            "peer_ttl 1h0m0s is not longer than the interval",
        ),
        (
            tracker(r#"{"interval": "10m", "peer_ttl": "10m"}"#),
            "is not longer than the interval",
        ),
        (tracker(r#"{"allow": ["xyz"]}"#), "is not an info hash"),
        (
            tracker(&format!(r#"{{"allow": ["{}"]}}"#, &h40[..39])),
            "is not an info hash",
        ),
        (
            tracker(r#"{"allow_files": [" "]}"#),
            "a file name may not be empty",
        ),
        (tracker(r#"{"max_torrents": 2000000}"#), "max_torrents"),
        (
            tracker(r#"{"max_peers_per_torrent": 70000}"#),
            "max_peers_per_torrent",
        ),
        (tracker(r#"{"max_numwant": 1001}"#), "max_numwant"),
        (tracker(r#"{"max_scrape": 1001}"#), "max_scrape"),
        (tracker(r#"{"access": {"allow": ["nope"]}}"#), "nope"),
        (
            tracker(r#"{"console": {"address": ":0", "users": []}}"#),
            "user",
        ),
    ] {
        refused(&doc, want);
    }
    let c = loads(&format!(
        r#"{{"bittorrent": {{"tracker": {{"listen": ":6969", "allow": ["{h40}", "{h64}"], "allow_files": ["a.txt"], "access": {{"deny_files": ["d.txt"]}}}}}}}}"#
    ));
    let t = c.bittorrent.and_then(|b| b.tracker).unwrap();
    assert_eq!(t.interval_or(), Duration::from_secs(1800));
    assert_eq!(t.ttl(), Duration::from_secs(3600));
    assert_eq!(
        (t.torrents(), t.peers_per_torrent(), t.numwant(), t.scrape()),
        (100_000, 10_000, 200, 64)
    );
    assert_eq!(t.files(), vec!["a.txt".to_owned(), "d.txt".to_owned()]);
    let h = parse_info_hash(&format!(" {h64} ")).unwrap();
    assert_eq!((h[0], h[19]), (0xcd, 0xcd));
    assert_eq!(parse_info_hash(&h40), Some([0xab; 20]));
    assert!(parse_info_hash(&format!("{}zz", &h64[..62])).is_none());
    assert!(parse_info_hash("").is_none());
}

/// fit's `TestBitTorrentTrackerBinds`: the tracker's UDP port in the address clash check, which
/// clashes with another UDP port, not a TCP one, and its console with another TCP port.
#[test]
fn tracker_binds() {
    refused(
        r#"{"listeners": [{"name": "web", "address": ":7000"}], "bittorrent": {"client": {"listen": ":6881", "data_dir": "d"}, "tracker": {"listen": "127.0.0.1:6881", "console": {"address": ":7000", "users": [{"username": "a", "password_hash": "$2a$04$5xt5UjNNUVWTqOgf9FDZm.bfWVyXg.U2as/rOFV.Ny5IQstc/jC26"}]}}}}"#,
        "bittorrent.client listens on UDP :6881 already",
    );
    refused(
        r#"{"listeners": [{"name": "web", "address": ":7000"}], "bittorrent": {"tracker": {"console": {"address": ":7000", "users": [{"username": "a", "password_hash": "$2a$04$5xt5UjNNUVWTqOgf9FDZm.bfWVyXg.U2as/rOFV.Ny5IQstc/jC26"}]}}}}"#,
        "listens on TCP :7000 already",
    );
    // A TCP listener and the tracker's UDP port do not clash; nor does a tracker without UDP.
    loads(
        r#"{"listeners": [{"name": "web", "address": ":6969"}], "bittorrent": {"tracker": {"listen": ":6969"}}}"#,
    );
    loads(r#"{"listeners": [{"name": "web", "address": ":6969"}], "bittorrent": {"tracker": {}}}"#);
    // The client's port is TCP and UDP: an h3 listener's UDP side clashes with it.
    refused(
        r#"{"bittorrent": {"client": {"listen": ":6881", "data_dir": "d"}, "tracker": {"listen": ":6881"}}}"#,
        "listens on UDP",
    );
}

/// fit's category checks and the I153 fields' defaults.
#[test]
fn categories_and_watch() {
    let client = |m: &str| {
        format!(r#"{{"bittorrent": {{"client": {{"listen": ":6881", "data_dir": "/d", {m}}}}}}}"#)
    };
    for (m, want) in [
        (r#""categories": [{"name": "-x"}]"#, "is not a valid name"),
        (
            r#""categories": [{"name": "a"}, {"name": "a"}]"#,
            "another category is already named",
        ),
        (
            r#""categories": [{"name": "a", "dir": "../x"}]"#,
            "leads out of download_dir",
        ),
        (r#""categories": [{"dir": "x"}]"#, "missing required field"),
        (r#""watch_max_files": 10001"#, "watch_max_files"),
        (r#""watch_interval": "-1s""#, "is negative"),
        (r#""url_timeout": "-1s""#, "is negative"),
    ] {
        refused(&client(m), want);
    }
    let c = loads(&client(
        r#""categories": [{"name": "films", "dir": "f"}, {"name": "iso"}, {"name": "abs", "dir": "/x"}], "watch_dir": "/w", "limits": {"max_active_downloads": 3, "max_active_seeds": 5, "min_free_bytes": 7}"#,
    ));
    let b = c.bittorrent.and_then(|b| b.client).unwrap();
    let cat = |n: &str| b.category(n).unwrap();
    assert_eq!(
        b.category_dir(cat("films")),
        std::path::Path::new("/d/downloads/f")
    );
    assert_eq!(
        b.category_dir(cat("iso")),
        std::path::Path::new("/d/downloads/iso")
    );
    assert_eq!(b.category_dir(cat("abs")), std::path::Path::new("/x"));
    assert_eq!(
        (b.watch(), b.watch_files(), b.url_fetch_timeout()),
        (Duration::from_secs(10), 100, Duration::from_secs(30))
    );
    let l = b.limits.unwrap();
    assert_eq!(
        (l.max_active_downloads, l.max_active_seeds, l.free_bytes()),
        (3, 5, 7)
    );
    assert_eq!(BitTorrentLimits::default().free_bytes(), 1 << 30);
}

/// fit's schedule checks and `Window` (I154), and a category's limits over a torrent's.
#[test]
fn alt_speed_and_limits() {
    let alt = |m: &str| {
        format!(
            r#"{{"bittorrent": {{"client": {{"listen": ":6881", "data_dir": "/d", "alt_speed": {m}}}}}}}"#
        )
    };
    for (m, want) in [
        (
            r#"{"schedules": [{"days": ["mon", "nope"], "from": "08:00", "to": "18:00"}]}"#,
            "is not a day",
        ),
        (
            r#"{"schedules": [{"from": "8", "to": "18:00"}]}"#,
            "is not a time of day",
        ),
        (
            r#"{"schedules": [{"from": "24:00", "to": "18:00"}]}"#,
            "is not a time of day",
        ),
        (
            r#"{"schedules": [{"from": "08:60", "to": "18:00"}]}"#,
            "is not a time of day",
        ),
        (
            r#"{"schedules": [{"from": "08:00", "to": "08:00"}]}"#,
            "the range is empty",
        ),
        (
            r#"{"schedules": [{"from": "08:00"}]}"#,
            "missing required field",
        ),
        (r#"{"upload_rate": -1}"#, "upload_rate"),
    ] {
        refused(&alt(m), want);
    }
    let c = loads(&alt(
        r#"{"upload_rate": 5, "schedules": [{"days": ["weekdays"], "from": "22:00", "to": "06:00"}, {"days": ["weekend"], "from": "10:00", "to": "24:00"}, {"from": "12:00", "to": "00:00"}], "time_zone": "utc"}"#,
    ));
    let a = c
        .bittorrent
        .and_then(|b| b.client)
        .and_then(|c| c.alt_speed)
        .unwrap();
    assert_eq!((a.upload_rate, a.time_zone.as_str()), (5, "utc"));
    let (night, weekend, noon) = (&a.schedules[0], &a.schedules[1], &a.schedules[2]);
    // Friday 23:00 and Saturday 05:00 (begun on Friday); not Saturday 23:00 nor Monday 05:00.
    assert!(night.holds(5, 23 * 60) && night.holds(6, 5 * 60));
    assert!(!night.holds(6, 23 * 60) && !night.holds(1, 5 * 60) && !night.holds(2, 6 * 60));
    // Saturday to midnight, Sunday too; not Monday.
    assert!(
        weekend.holds(6, 23 * 60 + 59) && weekend.holds(0, 10 * 60) && !weekend.holds(1, 11 * 60)
    );
    assert!(!weekend.holds(6, 9 * 60 + 59));
    // 00:00 as the end is midnight at the end, every day.
    assert_eq!(noon.to, 24 * 60);
    assert!(noon.holds(3, 23 * 60) && !noon.holds(3, 11 * 60));
    let c = loads(
        r#"{"bittorrent": {"client": {"listen": ":6881", "data_dir": "/d", "categories": [{"name": "a", "limits": {"upload_rate": -1, "download_rate": 7, "seed_ratio": 2.5, "seed_time": "1h"}}]}}}"#,
    );
    let cat = c
        .bittorrent
        .and_then(|b| b.client)
        .unwrap()
        .categories
        .remove(0)
        .limits;
    assert_eq!(
        (cat.upload_rate, cat.download_rate, cat.seed_time),
        (-1, 7, Duration::from_secs(3600))
    );
    let own = BitTorrentTorrentLimits {
        download_rate: 9,
        ..BitTorrentTorrentLimits::default()
    };
    let eff = cat.over(&own);
    assert_eq!((eff.upload_rate, eff.download_rate), (-1, 9));
    assert!((eff.seed_ratio - 2.5).abs() < f64::EPSILON && !eff.is_zero());
    assert!(BitTorrentTorrentLimits::default().is_zero());
}

/// fit's `TestBitTorrentPublish` (I156), and the path patterns.
#[test]
fn publish_entries() {
    let head =
        r#"{"bittorrent": {"client": {"listen": ":6881", "data_dir": "/var/lib/bt", "publish": "#;
    let c = loads(&format!(
        "{head}{}}}}}}}",
        r#"[{"name": "isos", "dir": "/srv/isos", "web_seed": "https://example.com/isos/", "announce": ["https://example.com/announce", "udp://example.com:6969"], "private": true, "piece_length": 262144, "files": ["*.iso"], "exclude": ["**/tmp/**"], "check_interval": "1m", "max_files": 10, "max_total_bytes": 1000, "max_hashing": 2}, {"name": "all", "dir": "/srv/all", "mode": "whole_dir", "web_seed": "http://example.com/x/all"}]"#
    ));
    let p = c.bittorrent.and_then(|b| b.client).unwrap().publish;
    assert_eq!(p.len(), 2);
    assert!(!p[0].whole_dir && p[1].whole_dir && p[0].private);
    assert_eq!(
        (
            p[0].interval(),
            p[0].files_max(),
            p[0].total_bytes(),
            p[0].hashing()
        ),
        (Duration::from_secs(60), 10, 1000, 2)
    );
    let d = BitTorrentPublish::default();
    assert_eq!(
        (d.interval(), d.files_max(), d.total_bytes(), d.hashing()),
        (Duration::from_secs(10), 1000, 1 << 40, 1)
    );
    for (doc, want) in [
        (r#"[{"name": "-x", "dir": "/a"}]"#, "not a valid name"),
        (
            r#"[{"name": "a", "dir": "/a"}, {"name": "a", "dir": "/b"}]"#,
            r#"another publish entry is already named "a""#,
        ),
        (r#"[{"name": "a", "dir": " "}]"#, "dir may not be empty"),
        (r#"[{"name": "a", "dir": "/"}]"#, "root of a file system"),
        (r#"[{"name": "a"}]"#, "dir"),
        (r#"[{"name": "a", "dir": "/a", "mode": "each"}]"#, "mode"),
        (
            r#"[{"name": "a", "dir": "/a", "web_seed": "ftp://x/a/"}]"#,
            "not an http or https URL",
        ),
        (
            r#"[{"name": "a", "dir": "/a", "web_seed": "http://x/a/?k=v"}]"#,
            "query or a fragment",
        ),
        (
            r#"[{"name": "a", "dir": "/a", "mode": "whole_dir", "web_seed": "http://x/b/"}]"#,
            r#"last element must be the directory's, "a""#,
        ),
        (
            r#"[{"name": "a", "dir": "/a", "announce": ["ws://x/announce"]}]"#,
            "not an http, https or udp tracker URL",
        ),
        (
            r#"[{"name": "a", "dir": "/a", "piece_length": 30000}]"#,
            "not a power of two",
        ),
        (
            r#"[{"name": "a", "dir": "/a", "piece_length": 8192}]"#,
            "not a power of two",
        ),
        (
            r#"[{"name": "a", "dir": "/a", "piece_length": 134217728}]"#,
            "piece_length",
        ),
        (
            r#"[{"name": "a", "dir": "/a", "files": ["["]}]"#,
            "is not a pattern",
        ),
        (
            r#"[{"name": "a", "dir": "/a", "exclude": ["/abs"]}]"#,
            "is not a pattern",
        ),
        (
            r#"[{"name": "a", "dir": "/a", "check_interval": "-1s"}]"#,
            "is negative",
        ),
        (
            r#"[{"name": "a", "dir": "/a", "max_files": 200000}]"#,
            "max_files",
        ),
        (
            r#"[{"name": "a", "dir": "/a", "max_hashing": 17}]"#,
            "max_hashing",
        ),
    ] {
        refused(&format!("{head}{doc}}}}}}}"), want);
    }
    let many: Vec<String> = (0..=MAX_BT_PUBLISH)
        .map(|i| format!(r#"{{"name": "e{i}", "dir": "/a"}}"#))
        .collect();
    refused(
        &format!("{head}[{}]}}}}}}", many.join(",")),
        "publish entries, more than",
    );
    for (pat, path, want) in [
        ("*.iso", "a/b/x.iso", true),
        ("*.iso", "x.isa", false),
        ("releases/**/*.tar.gz", "releases/x.tar.gz", true),
        ("releases/**/*.tar.gz", "releases/a/b/x.tar.gz", true),
        ("releases/**/*.tar.gz", "other/x.tar.gz", false),
        ("**/tmp/**", "a/tmp/b", true),
        ("**/tmp/**", "tmp/x", true),
        ("x?.[a-c]", "x1.b", true),
        ("x?.[^a-c]", "x1.b", false),
    ] {
        assert_eq!(path_pattern_matches(pat, path), want, "{pat} {path}");
    }
    assert!(!valid_path_pattern("[") && !valid_path_pattern("/a") && !valid_path_pattern("a//b"));
    assert!(valid_path_pattern("a/**/b*") && valid_path_pattern("[a-z]?"));
}

/// fit's `TestBitTorrentTrackerAuth` (I159): a private tracker names a `media_auth` policy, which
/// must be there.
#[test]
fn tracker_auth() {
    let ok = r#"{"media_auth": [{"name": "bt", "users": [{"name": "a", "password": "secret-password", "play": ["bittorrent/*"]}]}],
        "bittorrent": {"tracker": {"auth": "bt"}}}"#;
    let c = from_json(ok, None).unwrap();
    assert_eq!(c.bittorrent.unwrap().tracker.unwrap().auth, "bt");
    let e = from_json(r#"{"bittorrent": {"tracker": {"auth": "nope"}}}"#, None)
        .unwrap_err()
        .to_string();
    assert!(
        e.contains("bittorrent.tracker.auth")
            && e.contains("there is no media_auth policy named \"nope\""),
        "{e}"
    );
    let c = from_json(r#"{"bittorrent": {"tracker": {"auth": ""}}}"#, None).unwrap();
    assert_eq!(c.bittorrent.unwrap().tracker.unwrap().auth, "");
}
