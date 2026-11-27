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
        (r#""categories": []"#, "bittorrent.client.categories"),
        (r#""watch_dir": "w""#, "bittorrent.client.watch_dir"),
        (
            r#""watch_interval": "1s""#,
            "bittorrent.client.watch_interval",
        ),
        (
            r#""watch_max_files": 1"#,
            "bittorrent.client.watch_max_files",
        ),
        (r#""url_timeout": "1s""#, "bittorrent.client.url_timeout"),
        (r#""alt_speed": {}"#, "bittorrent.client.alt_speed"),
        (r#""access": {}"#, "bittorrent.client.access"),
        (
            r#""disk_queue_bytes": 1"#,
            "bittorrent.client.disk_queue_bytes",
        ),
        (r#""publish": []"#, "bittorrent.client.publish"),
        (r#""rss": {}"#, "bittorrent.client.rss"),
        (
            r#""limits": {"max_conns": 1}"#,
            "bittorrent.client.limits.max_conns",
        ),
        (
            r#""limits": {"max_half_open": 1}"#,
            "bittorrent.client.limits.max_half_open",
        ),
        (
            r#""limits": {"max_active_downloads": 1}"#,
            "bittorrent.client.limits.max_active_downloads",
        ),
        (
            r#""limits": {"max_active_seeds": 1}"#,
            "bittorrent.client.limits.max_active_seeds",
        ),
        (
            r#""limits": {"min_free_bytes": 1}"#,
            "bittorrent.client.limits.min_free_bytes",
        ),
    ] {
        refused(&client(m), &format!("{field}: not supported yet"));
    }
    for (m, field) in [
        (r#""max_peers": 1"#, "bittorrent.tracker.max_peers"),
        (r#""auth": "a""#, "bittorrent.tracker.auth"),
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
