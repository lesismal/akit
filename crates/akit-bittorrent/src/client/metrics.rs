//! The client's metrics (I157, fit's `bittorrent/metrics.go`), as the Admin API's `/metrics`
//! carries them. The manager's looks (each event, and every second) and the role's API calls
//! account what each torrent counted since the last look into totals of its category, and keep a
//! snapshot that a scrape reads under a lock of its own, never the role's: a scrape does not wait
//! for an apply or an API call writing the state.
//!
//! akio-bittorrent counts a torrent's bytes and failed pieces from when its client added it, and
//! forgets them when it is removed; the totals add what each torrent counted since the last look,
//! so that the counters never go back while akit runs, through removals and new clients alike. A
//! torrent's rates are worked out by the looks, over a second or more between two, as
//! akio-bittorrent gives none.

use std::collections::BTreeMap;
use std::time::{Duration, Instant};

use akit_observe::metrics::TextWriter;

use super::{Rec, Run, Shared, lock};

/// The states a torrent's status reports, as `akit_bittorrent_torrents` writes each: fit's
/// `States`.
pub(crate) const STATES: [&str; 9] = [
    "metadata",
    "checking",
    "downloading",
    "seeding",
    "queued",
    "paused",
    "stopped",
    "removed",
    "error",
];

/// The shortest time a torrent's rates are worked out over.
const RATE_SPAN: Duration = Duration::from_secs(1);

/// What a torrent's torrent in akio-bittorrent counted when the metrics last looked, and its
/// rates.
#[derive(Debug, Clone, Default)]
pub(crate) struct Counted {
    down: u64,
    up: u64,
    fails: u64,
    /// When its rates were last worked out, and its counts then.
    at: Option<Instant>,
    at_down: u64,
    at_up: u64,
    /// Bytes a second, over the last span.
    pub(crate) down_rate: f64,
    pub(crate) up_rate: f64,
}

#[derive(Debug, Clone, Copy, Default)]
struct Totals {
    down: u64,
    up: u64,
    fails: u64,
}

/// A category's line of a snapshot.
#[derive(Debug, Clone, Default)]
struct Cat {
    name: String,
    totals: Totals,
    down_rate: f64,
    up_rate: f64,
    peers: usize,
    seeders: usize,
    states: [usize; STATES.len()],
}

/// The totals across clients, and the last look's snapshot.
#[derive(Debug, Default)]
pub(crate) struct Meters {
    totals: BTreeMap<String, Totals>,
    cats: Vec<Cat>,
    running: bool,
    alt: bool,
    published: Vec<(String, usize)>,
}

/// Adds what `r`'s torrent counted since the last look to its category's totals, and works out
/// its rates when a span has passed.
pub(crate) fn account_one(r: &mut Rec, m: &mut Meters) {
    let Some(t) = &r.torrent else {
        r.counted.down_rate = 0.0;
        r.counted.up_rate = 0.0;
        return;
    };
    let st = t.stats();
    let c = &mut r.counted;
    // Another torrent in akio-bittorrent (a magnet link placed, a recheck): its counts start
    // again from zero.
    if st.downloaded < c.down || st.uploaded < c.up || st.hash_fails < c.fails {
        *c = Counted::default();
    }
    let tot = m.totals.entry(r.entry.category.clone()).or_default();
    tot.down = tot
        .down
        .saturating_add(st.downloaded.saturating_sub(c.down));
    tot.up = tot.up.saturating_add(st.uploaded.saturating_sub(c.up));
    tot.fails = tot
        .fails
        .saturating_add(st.hash_fails.saturating_sub(c.fails));
    c.down = st.downloaded;
    c.up = st.uploaded;
    c.fails = st.hash_fails;
    let now = Instant::now();
    match c.at {
        None => {
            c.at = Some(now);
            c.at_down = st.downloaded;
            c.at_up = st.uploaded;
        }
        Some(at) => {
            let span = now.saturating_duration_since(at);
            if span >= RATE_SPAN {
                let secs = span.as_secs_f64();
                #[allow(clippy::cast_precision_loss)]
                {
                    c.down_rate = st.downloaded.saturating_sub(c.at_down) as f64 / secs;
                    c.up_rate = st.uploaded.saturating_sub(c.at_up) as f64 / secs;
                }
                c.at = Some(now);
                c.at_down = st.downloaded;
                c.at_up = st.uploaded;
            }
        }
    }
    if st.status == akio_bittorrent::client::TorrentStatus::Paused {
        c.down_rate = 0.0;
        c.up_rate = 0.0;
    }
}

impl Run {
    /// Accounts every torrent and keeps the snapshot a scrape writes.
    pub(crate) fn account(&mut self, m: &mut Meters) {
        let mut cats: BTreeMap<String, Cat> = BTreeMap::new();
        for r in &mut self.recs {
            account_one(r, m);
            let c = cats.entry(r.entry.category.clone()).or_default();
            if let Some(t) = &r.torrent {
                let st = t.stats();
                c.peers = c.peers.saturating_add(st.peers);
                c.seeders = c.seeders.saturating_add(st.seeders);
            }
            c.down_rate += r.counted.down_rate;
            c.up_rate += r.counted.up_rate;
            if let Some(i) = STATES.iter().position(|s| *s == r.last_state) {
                if let Some(n) = c.states.get_mut(i) {
                    *n = n.saturating_add(1);
                }
            }
        }
        for (name, t) in &m.totals {
            cats.entry(name.clone()).or_default().totals = *t;
        }
        m.cats = cats
            .into_iter()
            .map(|(name, mut c)| {
                c.name = name;
                c
            })
            .collect();
        m.running = true;
        m.alt = self.alt.active;
        m.published = self
            .publish
            .statuses()
            .iter()
            .map(|e| {
                let e = lock(e);
                (e.name.clone(), e.torrents.len())
            })
            .collect();
    }
}

/// The role stopped: the client no longer runs.
pub(crate) fn stopped(shared: &Shared) {
    let mut m = lock(&shared.meters);
    m.running = false;
    m.alt = false;
    m.published.clear();
    for c in &mut m.cats {
        c.down_rate = 0.0;
        c.up_rate = 0.0;
        c.peers = 0;
        c.seeders = 0;
        c.states = [0; STATES.len()];
    }
}

/// Writes the role's metrics: per category (`""` for the torrents of none) the payload bytes
/// downloaded and uploaded, the rates, the peers connected, the torrents in each state and the
/// pieces that failed their check; whether the client runs, whether the alternative rates hold,
/// the event stream's subscribers and those dropped; per publish entry, its torrents. It takes
/// no lock of the role's.
pub(crate) fn write(shared: &Shared, out: &mut Vec<u8>) {
    let (cats, running, alt, published) = {
        let m = lock(&shared.meters);
        (m.cats.clone(), m.running, m.alt, m.published.clone())
    };
    let subscribers = shared.events.subscribers();
    let dropped = shared
        .events
        .dropped
        .load(std::sync::atomic::Ordering::Relaxed);
    let n = |v: usize| u64::try_from(v).unwrap_or(u64::MAX);
    let mut w = TextWriter::new(std::mem::take(out));
    w.family(
        "akit_bittorrent_client_running",
        "gauge",
        "Whether the BitTorrent client runs (1) or not (0), as when its port could not be taken.",
    );
    w.sample_u64("akit_bittorrent_client_running", u64::from(running), &[]);
    let fams: [(&str, &str, &str); 7] = [
        (
            "akit_bittorrent_downloaded_bytes_total",
            "counter",
            "Payload bytes the torrents of a category downloaded since akit started, removed torrents' included.",
        ),
        (
            "akit_bittorrent_uploaded_bytes_total",
            "counter",
            "Payload bytes the torrents of a category uploaded since akit started, removed torrents' included.",
        ),
        (
            "akit_bittorrent_download_rate_bytes",
            "gauge",
            "Bytes per second the torrents of a category download, over a second or more.",
        ),
        (
            "akit_bittorrent_upload_rate_bytes",
            "gauge",
            "Bytes per second the torrents of a category upload, over a second or more.",
        ),
        (
            "akit_bittorrent_peers",
            "gauge",
            "Peer connections of the torrents of a category.",
        ),
        (
            "akit_bittorrent_seeders",
            "gauge",
            "Peer connections of the torrents of a category to peers that have every piece.",
        ),
        (
            "akit_bittorrent_hash_failures_total",
            "counter",
            "Pieces of the torrents of a category that failed their check and were downloaded again, since akit started.",
        ),
    ];
    for (i, (name, typ, help)) in fams.iter().enumerate() {
        w.family(name, typ, help);
        for c in &cats {
            let l = [("category", c.name.as_str())];
            #[allow(clippy::cast_precision_loss)]
            let v = match i {
                0 => c.totals.down as f64,
                1 => c.totals.up as f64,
                2 => c.down_rate,
                3 => c.up_rate,
                4 => c.peers as f64,
                5 => c.seeders as f64,
                _ => c.totals.fails as f64,
            };
            w.sample(name, v, &l);
        }
    }
    w.family(
        "akit_bittorrent_torrents",
        "gauge",
        "Torrents of a category in each state, as their status reports it.",
    );
    for c in &cats {
        for (s, v) in STATES.iter().zip(c.states) {
            w.sample_u64(
                "akit_bittorrent_torrents",
                n(v),
                &[("category", c.name.as_str()), ("state", s)],
            );
        }
    }
    w.family(
        "akit_bittorrent_alt_speed_active",
        "gauge",
        "Whether the alternative rates hold (1) or not (0).",
    );
    w.sample_u64("akit_bittorrent_alt_speed_active", u64::from(alt), &[]);
    w.family(
        "akit_bittorrent_event_subscribers",
        "gauge",
        "Subscribers to the role's events, as the Admin API's /bittorrent/events streams.",
    );
    w.sample_u64("akit_bittorrent_event_subscribers", n(subscribers), &[]);
    w.family(
        "akit_bittorrent_event_subscribers_dropped_total",
        "counter",
        "Subscribers to the role's events dropped for not taking them fast enough.",
    );
    w.sample_u64(
        "akit_bittorrent_event_subscribers_dropped_total",
        dropped,
        &[],
    );
    if !published.is_empty() {
        w.family(
            "akit_bittorrent_published_torrents",
            "gauge",
            "Torrents a publish entry seeds, or has built and cannot seed.",
        );
        for (name, k) in &published {
            w.sample_u64(
                "akit_bittorrent_published_torrents",
                n(*k),
                &[("entry", name.as_str())],
            );
        }
    }
    *out = w.out;
}
