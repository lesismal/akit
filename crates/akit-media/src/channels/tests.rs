//! fit's `streaming/channels` tests ported (I145): channels on fit's fixtures (`tests/data/
//! medialib`), on a clock the test moves. A test waits for the library's pool to have nothing to
//! do, never for time.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing,
    clippy::arithmetic_side_effects,
    clippy::cast_possible_wrap
)]

use std::path::Path;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering::SeqCst};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use akio_http::{Method, Response};
use akio_media::av::{Codec, Packet, Time, Track, Tracks};
use akio_media::hub::{Hub, HubConfig, HubError, PeerInfo, Sink, SubscribeOptions, Subscription};
use akit_config::channels::{ChannelSlot, StreamChannel, parse_channel_at};
use akit_config::medialib::MediaLibrary;

use super::channel::SEND_AHEAD;
use super::clock::{Clock, Timer};
use super::source::{FROM_ADMIN, FROM_LIST, FROM_SCHEDULE};
use super::*;
use crate::library::{self, Library};

const MS: Time = 1_000_000;
const SEC: Time = 1000 * MS;
const WAIT: Duration = Duration::from_secs(20);

/// The test's start: a Friday, 2026-11-06 09:59:58 UTC.
fn t0() -> i64 {
    (akit_observe::time::days_from_civil(2026, 11, 6) * 86_400 + 9 * 3600 + 59 * 60 + 58) * SEC
}

// ---- a clock the test moves ----

struct FakeTimer {
    at: i64,
    f: Mutex<Option<Box<dyn FnOnce() + Send>>>,
    stopped: AtomicBool,
}

struct FakeStop(Arc<FakeTimer>);

impl Timer for FakeStop {
    fn stop(&self) {
        self.0.stopped.store(true, SeqCst);
    }
}

/// A clock whose timers run, in the order they fall due, from [`FakeClock::advance`].
struct FakeClock {
    st: Mutex<(i64, Vec<Arc<FakeTimer>>)>,
}

impl FakeClock {
    fn new(at: i64) -> Arc<FakeClock> {
        Arc::new(FakeClock {
            st: Mutex::new((at, Vec::new())),
        })
    }

    /// Moves the clock by `d`, running each timer that falls due on the way at its time.
    fn advance(&self, d: Time) {
        let end = self.st.lock().unwrap().0 + d;
        loop {
            let next = {
                let mut g = self.st.lock().unwrap();
                g.1.retain(|t| !t.stopped.load(SeqCst) && t.f.lock().unwrap().is_some());
                let next =
                    g.1.iter()
                        .filter(|t| t.at <= end)
                        .min_by_key(|t| t.at)
                        .cloned();
                match &next {
                    Some(t) => g.0 = g.0.max(t.at),
                    None => g.0 = end,
                }
                next
            };
            let Some(t) = next else { return };
            let f = t.f.lock().unwrap().take();
            if let Some(f) = f {
                f();
            }
        }
    }
}

impl Clock for FakeClock {
    fn now(&self) -> i64 {
        self.st.lock().unwrap().0
    }

    fn after(&self, d: Time, f: Box<dyn FnOnce() + Send>) -> Box<dyn Timer> {
        let mut g = self.st.lock().unwrap();
        let t = Arc::new(FakeTimer {
            at: g.0 + d.max(0),
            f: Mutex::new(Some(f)),
            stopped: AtomicBool::new(false),
        });
        g.1.push(Arc::clone(&t));
        Box::new(FakeStop(t))
    }
}

// ---- the rig: a hub, a library of the fixtures and a manager on a fake clock ----

static KEYS: AtomicUsize = AtomicUsize::new(3 << 40);

struct Rig {
    key: usize,
    lib: Arc<Library>,
    hub: Hub,
    clock: Arc<FakeClock>,
    m: Arc<Manager>,
}

impl Drop for Rig {
    fn drop(&mut self) {
        self.m.stop();
        self.hub.close();
        library::remove(self.key);
        self.lib.join(WAIT);
    }
}

fn data() -> String {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/data/medialib/lib")
        .to_string_lossy()
        .into_owned()
}

fn rig() -> Rig {
    let key = KEYS.fetch_add(1, SeqCst);
    library::apply(
        key,
        &[MediaLibrary {
            name: "lib".into(),
            roots: vec![data()],
            ..MediaLibrary::default()
        }],
    )
    .unwrap();
    let lib = library::lookup(key, "lib").unwrap();
    lib.wait_status(WAIT, |s| s.scans >= 1 && !s.scanning)
        .expect("the library's first scan");
    let hub = Hub::new(HubConfig::default());
    let clock = FakeClock::new(t0());
    let m = Arc::new(Manager::new(Options {
        hub: hub.clone(),
        libraries: key,
        clock: Some(Arc::clone(&clock) as Arc<dyn Clock>),
        seed: 7,
    }));
    Rig {
        key,
        lib,
        hub,
        clock,
        m,
    }
}

fn ch(name: &str, items: &[&str]) -> StreamChannel {
    StreamChannel {
        name: name.into(),
        library: "lib".into(),
        items: items.iter().map(|s| (*s).to_owned()).collect(),
        looped: true,
        ..StreamChannel::default()
    }
}

fn slot(at: &str, item: &str) -> ChannelSlot {
    ChannelSlot {
        at_text: at.into(),
        at: parse_channel_at(at).unwrap(),
        item: item.into(),
    }
}

impl Rig {
    fn apply(&self, chs: &[StreamChannel]) {
        self.m.apply(chs).unwrap();
        self.settle();
    }

    /// Waits until no channel has a load or a read under way.
    fn settle(&self) {
        for _ in 0..1000 {
            assert!(
                self.lib.pool().wait_idle(WAIT),
                "the library's pool is busy"
            );
            let busy = self.m.list().iter().any(|c| {
                let st = c.lock();
                st.reading || st.loading
            });
            if !busy {
                return;
            }
        }
        panic!("a channel is still reading");
    }

    /// Moves the clock by `d`, a step at a time, letting reads end between steps as a quick
    /// disk would.
    fn run(&self, d: Time) {
        let step = 5 * MS;
        let mut gone = 0;
        while gone < d {
            self.clock.advance(step);
            self.settle();
            gone += step;
        }
    }

    fn run_to(&self, at: i64) {
        self.run(at - self.clock.now());
    }

    /// A recorder subscribed to `path`, waiting for it to be published.
    fn play(&self, path: &str) -> (Arc<Recorder>, Subscription) {
        let rec = Arc::new(Recorder {
            clock: Arc::clone(&self.clock),
            recs: Mutex::new(Vec::new()),
        });
        let sub = self
            .hub
            .subscribe(
                path,
                RecSink(Arc::clone(&rec)),
                SubscribeOptions::new(PeerInfo::default()).wait(Duration::from_secs(3600)),
            )
            .unwrap();
        (rec, sub)
    }

    fn status(&self, name: &str) -> ChannelStatus {
        self.m.channel(name).unwrap().status()
    }
}

/// What a player of a channel heard, and when.
#[derive(Clone)]
struct Rec {
    at: i64,
    tracks: Option<Tracks>,
    pkt: Option<Arc<Packet>>,
}

struct Recorder {
    clock: Arc<FakeClock>,
    recs: Mutex<Vec<Rec>>,
}

struct RecSink(Arc<Recorder>);

impl Sink for RecSink {
    fn on_tracks(&mut self, tracks: &Tracks) {
        let at = self.0.clock.now();
        self.0.recs.lock().unwrap().push(Rec {
            at,
            tracks: Some(tracks.clone()),
            pkt: None,
        });
    }

    fn on_packet(&mut self, pkt: &Arc<Packet>) {
        let at = self.0.clock.now();
        self.0.recs.lock().unwrap().push(Rec {
            at,
            tracks: None,
            pkt: Some(Arc::clone(pkt)),
        });
    }

    fn on_close(&mut self, _err: HubError) {}
}

impl Recorder {
    fn all(&self) -> Vec<Rec> {
        self.recs.lock().unwrap().clone()
    }
}

/// What a recorder heard, split at each `on_tracks`.
fn segments(recs: &[Rec]) -> Vec<Vec<Rec>> {
    let mut out: Vec<Vec<Rec>> = Vec::new();
    for r in recs {
        if r.tracks.is_some() {
            out.push(vec![r.clone()]);
        } else if let Some(last) = out.last_mut() {
            last.push(r.clone());
        }
    }
    out
}

/// fit's `checkPaced`: every packet sent when its decode time fell due on the timeline of the
/// first, within [`SEND_AHEAD`] early and 100ms late; decode times never going back on a
/// track, nor PTS before DTS; a track's packets following one another without a gap of more
/// than `gap`.
fn check_paced(recs: &[Rec], gap: Time) {
    let mut first: Option<(i64, Time)> = None;
    let mut last: HashMap<usize, Time> = HashMap::new();
    for seg in segments(recs) {
        for r in &seg[1..] {
            let p = r.pkt.as_ref().unwrap();
            let (start, dts0) = *first.get_or_insert((r.at, p.dts));
            let due = start + (p.dts - dts0);
            assert!(
                due - r.at <= SEND_AHEAD && r.at - due <= 100 * MS,
                "a packet of DTS {} was sent at {}, due at {}",
                p.dts,
                r.at - t0(),
                due - t0()
            );
            if let Some(&l) = last.get(&p.track) {
                assert!(
                    p.dts > l && p.dts - l <= gap,
                    "track {}: DTS {} after {l}",
                    p.track,
                    p.dts
                );
            }
            assert!(p.pts >= p.dts, "PTS {} before DTS {}", p.pts, p.dts);
            last.insert(p.track, p.dts);
        }
    }
}

const GAP: Time = 50 * MS;

/// fit's `TestBackToBack`.
#[test]
fn back_to_back() {
    let r = rig();
    let (rec, _sub) = r.play("tv/one");
    r.apply(&[ch("tv/one", &["films/bbb", "sintel"])]);
    r.run(4500 * MS);
    let recs = rec.all();
    let segs = segments(&recs);
    assert!(segs.len() >= 4, "{} items played in 4.5s", segs.len());
    // bbb at its 360p rendition (128x96), then sintel's, then bbb again.
    let t = |i: usize| segs[i][0].tracks.clone().unwrap();
    assert_eq!((t(0)[0].codec, t(0)[0].width), (Codec::H264, 128));
    assert_eq!((t(1).len(), t(2).len(), t(2)[0].width), (2, 2, 128));
    check_paced(&recs, GAP);
    // Each item of a second, its packets over about a second.
    for (i, seg) in segs[..3].iter().enumerate() {
        let a = seg[1].pkt.as_ref().unwrap();
        let b = seg.last().unwrap().pkt.as_ref().unwrap();
        let d = b.dts - a.dts;
        assert!((900 * MS..=1100 * MS).contains(&d), "item {i} spans {d}");
        assert!(a.keyframe && a.track == 0, "item {i} starts with {a:?}");
    }
    let st = r.status("tv/one");
    assert_eq!(st.state, STATE_PLAYING);
    assert!(
        st.played >= 4 && st.failed == 0 && st.underruns == 0,
        "{st:?}"
    );
    assert_eq!(st.now.unwrap().from, FROM_LIST);
    let info = r.hub.stream_info("tv/one").unwrap();
    assert!(info.publishing && info.publisher.protocol == "channel");
}

/// fit's `TestNoLoop`.
#[test]
fn no_loop() {
    let r = rig();
    let (rec, _sub) = r.play("tv/once");
    let mut c = ch("tv/once", &["films/bbb"]);
    c.looped = false;
    r.apply(&[c]);
    r.run(3 * SEC);
    assert_eq!(segments(&rec.all()).len(), 1);
    let st = r.status("tv/once");
    assert!(st.state == STATE_IDLE && st.now.is_none(), "{st:?}");
    // Still published, quiet.
    assert!(r.hub.stream_info("tv/once").unwrap().publishing);
    // A start of a channel that runs does nothing; a stop and a start play the list played out
    // again from the first.
    let c = r.m.channel("tv/once").unwrap();
    c.start();
    c.stop();
    assert_eq!(c.status().state, STATE_STOPPED);
    assert_eq!(c.skip(""), Err(ActionError::Stopped));
    assert!(r.hub.stream_info("tv/once").is_none_or(|i| !i.publishing));
    c.start();
    r.settle();
    r.run(200 * MS);
    let st = c.status();
    assert_eq!(st.state, STATE_PLAYING);
    assert_eq!(st.now.unwrap().item, "films/bbb");
    assert_eq!(st.played, 2);
}

/// fit's `TestSameTracks`: an item played twice carries on the first's tracks, set once, and
/// its timestamps.
#[test]
fn same_tracks() {
    let r = rig();
    let (rec, _sub) = r.play("tv/same");
    r.apply(&[ch("tv/same", &["films/bbb", "films/bbb"])]);
    r.run(2500 * MS);
    let recs = rec.all();
    assert_eq!(segments(&recs).len(), 1, "the tracks were set again");
    check_paced(&recs, GAP);
    assert!(r.status("tv/same").played >= 3);
}

/// fit's `TestSkipAndSkipTo`.
#[test]
fn skip_and_skip_to() {
    let r = rig();
    let (rec, _sub) = r.play("tv/skip");
    r.apply(&[ch("tv/skip", &["films/bbb", "sintel", "clips/late"])]);
    r.run(300 * MS);
    let c = r.m.channel("tv/skip").unwrap();
    assert_eq!(c.status().now.unwrap().item, "films/bbb");
    c.skip("").unwrap();
    r.settle();
    r.run(200 * MS);
    assert_eq!(c.status().now.unwrap().item, "sintel");
    assert_eq!(c.skip("nope"), Err(ActionError::NoItem));
    c.skip("films/bbb").unwrap();
    r.settle();
    r.run(200 * MS);
    let now = c.status().now.unwrap();
    assert_eq!((now.item.as_str(), now.from), ("films/bbb", FROM_ADMIN));
    // Then the list goes on after sintel, the last it played.
    r.run(SEC);
    assert_eq!(c.status().now.unwrap().item, "clips/late");
    check_paced(&rec.all(), 150 * MS);
}

/// For each item `recs` hold, when its first picture is shown on the channel's timeline, after
/// t0: the timeline of the first packet's DTS, sent when due.
fn shown_at(recs: &[Rec]) -> Vec<Time> {
    let mut out = Vec::new();
    let mut start: Option<(i64, Time)> = None;
    for seg in segments(recs) {
        let mut first: Option<Time> = None;
        for r in &seg[1..] {
            let p = r.pkt.as_ref().unwrap();
            start.get_or_insert((r.at, p.dts));
            if p.track == 0 && first.is_none_or(|f| p.pts < f) {
                first = Some(p.pts);
            }
        }
        if let (Some(f), Some((s, d0))) = (first, start) {
            out.push(s + (f - d0) - t0());
        }
    }
    out
}

fn shown_near(shown: &[Time], at: Time) -> bool {
    shown.iter().any(|s| (s - at).abs() <= 5 * MS)
}

/// fit's `TestScheduleStartsOnTime`.
#[test]
fn schedule_starts_on_time() {
    let r = rig();
    let (rec, _sub) = r.play("tv/sched");
    // t0 is 09:59:58: bbb plays, again, until sintel's slot at 10:00:00.
    let mut c = ch("tv/sched", &["films/bbb"]);
    c.schedule = vec![
        slot("10:00", "sintel"),
        slot("2026-11-06 10:00:05", "clips/late"),
    ];
    r.apply(&[c]);
    let st = r.status("tv/sched");
    assert_eq!(st.next_slot, Some((t0() + 2 * SEC, "sintel".to_owned())));
    let now_at = |at: Time| {
        r.run_to(t0() + at);
        r.status("tv/sched").now.map(|n| n.item).unwrap_or_default()
    };
    // bbb from 09:59:58, again at :59, sintel at 10:00:00, bbb after it, and late at 10:00:05,
    // once.
    for (at, item) in [
        (500 * MS, "films/bbb"),
        (1500 * MS, "films/bbb"),
        (2500 * MS, "sintel"),
        (3500 * MS, "films/bbb"),
        (7500 * MS, "clips/late"),
        (8500 * MS, "films/bbb"),
    ] {
        assert_eq!(now_at(at), item, "at 09:59:58 + {}ms", at / MS);
    }
    let recs = rec.all();
    check_paced(&recs, 250 * MS);
    // Each slot's first picture shown at its start, within a tick.
    let shown = shown_at(&recs);
    for at in [2 * SEC, 7 * SEC] {
        assert!(
            shown_near(&shown, at),
            "no item shown from 09:59:58 + {at}: {shown:?}"
        );
    }
}

/// fit's `TestScheduleCutsIn`: a slot falling half way through an item cuts in at its time.
#[test]
fn schedule_cuts_in() {
    let r = rig();
    let (rec, _sub) = r.play("tv/cut");
    r.clock.advance(500 * MS); // 09:59:58.5
    let mut c = ch("tv/cut", &["films/bbb"]);
    c.schedule = vec![slot("10:00:00", "sintel")];
    c.time_zone = "UTC".into();
    r.apply(&[c]);
    r.run(2500 * MS);
    let recs = rec.all();
    check_paced(&recs, GAP);
    let shown = shown_at(&recs);
    assert!(
        shown_near(&shown, 2 * SEC),
        "sintel not shown from 10:00:00: {shown:?}"
    );
    let item = r.status("tv/cut").now.unwrap().item;
    assert!(item == "films/bbb" || item == "sintel", "{item}");
}

/// fit's `TestScheduleJoined`: a slot under way when the channel starts is joined where it is;
/// one passed over whole is not.
#[test]
fn schedule_joined() {
    let r = rig();
    r.clock.advance(2500 * MS); // 10:00:00.5
    let (rec, _sub) = r.play("tv/join");
    let mut c = ch("tv/join", &["films/bbb"]);
    c.schedule = vec![slot("sat-fri 10:00", "sintel")];
    r.apply(std::slice::from_ref(&c));
    r.run(100 * MS);
    let now = r.status("tv/join").now.unwrap();
    assert_eq!(
        (now.item.as_str(), now.from, now.offset, now.slot),
        ("sintel", FROM_SCHEDULE, 500 * MS, Some(t0() + 2 * SEC))
    );
    r.run(1500 * MS);
    check_paced(&rec.all(), GAP);
    assert_eq!(r.status("tv/join").now.unwrap().item, "films/bbb");
    let r2 = rig();
    r2.clock.advance(10 * SEC);
    let mut c2 = c.clone();
    c2.schedule = vec![slot("10:00", "sintel")];
    r2.apply(&[c2]);
    r2.run(100 * MS);
    assert_eq!(r2.status("tv/join").now.unwrap().item, "films/bbb");
}

/// fit's `TestScheduleOnly`.
#[test]
fn schedule_only() {
    let r = rig();
    let mut c = ch("tv/slots", &[]);
    c.schedule = vec![slot("10:00", "sintel")];
    r.apply(&[c]);
    assert_eq!(r.status("tv/slots").state, STATE_IDLE);
    let (rec, _sub) = r.play("tv/slots");
    r.run(2200 * MS);
    let st = r.status("tv/slots");
    assert_eq!(
        (st.state, st.now.unwrap().item.as_str()),
        (STATE_PLAYING, "sintel")
    );
    r.run(1500 * MS);
    let st = r.status("tv/slots");
    assert_eq!(st.state, STATE_IDLE);
    assert_eq!(st.next_slot.unwrap().0, t0() + 2 * SEC + 86_400 * SEC);
    assert_eq!(segments(&rec.all()).len(), 1);
}

/// fit's `TestReloadTakesEffectNextItem`.
#[test]
fn reload_takes_effect_next_item() {
    let r = rig();
    let (rec, _sub) = r.play("tv/reload");
    let mut c = ch("tv/reload", &["films/bbb", "sintel"]);
    c.read_ahead = Duration::from_millis(500);
    r.apply(std::slice::from_ref(&c));
    r.run(200 * MS);
    let first = r.m.channel("tv/reload").unwrap();
    c.items = vec!["clips/late".into()];
    r.apply(std::slice::from_ref(&c));
    assert!(Arc::ptr_eq(&first, &r.m.channel("tv/reload").unwrap()));
    assert_eq!(first.status().now.unwrap().item, "films/bbb");
    r.run(2 * SEC);
    assert_eq!(first.status().now.unwrap().item, "clips/late");
    check_paced(&rec.all(), 250 * MS);
    // A channel renamed is another: the old one's stream ends.
    c.name = "tv/renamed".into();
    r.apply(std::slice::from_ref(&c));
    assert!(r.m.channel("tv/reload").is_none() && r.m.channel("tv/renamed").is_some());
    assert!(r.hub.stream_info("tv/reload").is_none_or(|i| !i.publishing));
    // Nothing left: every channel ends.
    r.apply(&[]);
    assert!(r.m.channels().is_empty());
}

/// fit's `TestShuffle`: every item once each round, never the same twice in a row.
#[test]
fn shuffle() {
    let r = rig();
    let mut c = ch("tv/shuffle", &["films/bbb", "sintel", "clips/late"]);
    c.shuffle = true;
    r.apply(&[c]);
    let mut seen: Vec<String> = Vec::new();
    while seen.len() < 6 {
        r.run(100 * MS);
        let st = r.status("tv/shuffle");
        if st.played > seen.len() as u64 {
            seen.push(st.now.unwrap().item);
        }
    }
    for round in 0..2 {
        let mut got = seen[round * 3..round * 3 + 3].to_vec();
        got.sort();
        got.dedup();
        assert_eq!(got.len(), 3, "round {round}: {seen:?}");
    }
    assert_ne!(seen[2], seen[3], "{seen:?}");
}

/// fit's `TestItemsSkipped`.
#[test]
fn items_skipped() {
    let r = rig();
    let (rec, _sub) = r.play("tv/skipped");
    r.apply(&[ch("tv/skipped", &["nope", "broken", "films/bbb"])]);
    r.run(300 * MS);
    let st = r.status("tv/skipped");
    assert!(st.failed >= 2, "{st:?}");
    assert_eq!(st.now.unwrap().item, "films/bbb");
    assert_eq!(segments(&rec.all()).len(), 1);
    // Every item failing: the channel idles, and tries again later.
    r.apply(&[ch("tv/none", &["nope"])]);
    r.run(100 * MS);
    let st = r.status("tv/none");
    assert!(
        st.state == STATE_IDLE && st.error.contains("nope"),
        "{st:?}"
    );
    let failed = st.failed;
    r.run(channel::RETRY_IDLE + 100 * MS);
    assert!(r.status("tv/none").failed > failed, "not tried again");
    // A library that does not run.
    let mut c = ch("tv/nolib", &["films/bbb"]);
    c.library = "nolib".into();
    r.apply(&[c]);
    r.run(10 * MS);
    let st = r.status("tv/nolib");
    assert!(
        st.state == STATE_IDLE && st.error.contains("library"),
        "{st:?}"
    );
}

/// fit's `TestStreamTaken`.
#[test]
fn stream_taken() {
    let r = rig();
    let tracks: Tracks = Arc::from(vec![Track::new(Codec::Aac, vec![0x11, 0x90])]);
    let other = r
        .hub
        .publish("tv/taken", tracks, PeerInfo::new("rtmp", None, ""))
        .unwrap();
    r.apply(&[ch("tv/taken", &["films/bbb"])]);
    let st = r.status("tv/taken");
    assert!(st.state == STATE_WAITING && !st.error.is_empty(), "{st:?}");
    other.close();
    drop(other);
    r.run(channel::RETRY_IDLE + 100 * MS);
    assert_eq!(r.status("tv/taken").state, STATE_PLAYING);
}

/// fit's `TestStopStart`: timestamps run on across a stop.
#[test]
fn stop_start() {
    let r = rig();
    let (rec, _sub) = r.play("tv/ss");
    r.apply(&[ch("tv/ss", &["films/bbb", "sintel"])]);
    r.run(1500 * MS);
    let c = r.m.channel("tv/ss").unwrap();
    c.stop();
    assert!(r.hub.stream_info("tv/ss").is_none_or(|i| !i.publishing));
    r.run(SEC);
    c.start();
    r.settle();
    r.run(300 * MS);
    assert_eq!(c.status().state, STATE_PLAYING);
    let mut last = -1;
    for x in rec.all() {
        if let Some(p) = x.pkt.as_ref().filter(|p| p.track == 0) {
            assert!(p.dts > last, "DTS {} after {last}", p.dts);
            last = p.dts;
        }
    }
    assert_eq!(r.m.channels().len(), 1);
}

/// fit's `TestLateTicksPause`: a timer 2s late, as a stalled read would make it, goes on from
/// there rather than sending 2s at once.
#[test]
fn late_ticks_pause() {
    let r = rig();
    r.apply(&[ch("tv/late", &["films/bbb"])]);
    r.run(100 * MS);
    let c = r.m.channel("tv/late").unwrap();
    let now = r.clock.now();
    c.locked(|c, st| c.tick(st, now + 2 * SEC));
    assert_eq!(c.underruns.load(SeqCst), 1);
}

/// fit's `TestSources`, what akit's source keeps.
#[test]
fn sources() {
    let r = rig();
    let bbb = r.lib.item("films/bbb").unwrap();
    let name = |i: Option<usize>| bbb.renditions[i.unwrap()].name.clone();
    assert_eq!(name(source::pick_rendition(&bbb, "")), "360p");
    assert_eq!(name(source::pick_rendition(&bbb, "nope")), "360p");
    assert_eq!(name(source::pick_rendition(&bbb, "360p")), "360p");
    let sintel = r.lib.item("sintel").unwrap();
    assert_eq!(
        sintel.renditions[source::pick_rendition(&sintel, "sd").unwrap()].name,
        "sd"
    );
    for (codec, kind, want) in [
        ("h264", "video", (true, true)),
        ("av1", "audio", (true, false)),
        ("opus", "audio", (false, true)),
        ("mjpeg", "video", (false, false)),
    ] {
        assert_eq!(source::playable(kind, codec), want, "{codec} {kind}");
    }
    let choice = |offset: Time| source::Choice {
        item: "x".into(),
        from: FROM_LIST,
        offset,
        slot: 0,
    };
    let load = |it: Arc<library::Item>, offset: Time| {
        let src = Arc::new(
            source::Source::new(
                Arc::clone(&r.lib),
                it,
                "",
                choice(offset),
                list::ListState::default(),
            )
            .unwrap(),
        );
        let (tx, rx) = std::sync::mpsc::channel();
        src.load(move |res| tx.send(res).unwrap());
        rx.recv_timeout(WAIT).unwrap().unwrap();
        src
    };
    // A rendition of video alone takes the item's audio rendition.
    let mut it = (*bbb).clone();
    let best = source::pick_rendition(&bbb, "").unwrap();
    let mut video = bbb.renditions[best].clone();
    video.tracks.truncate(1);
    it.renditions = vec![video];
    let src = load(Arc::new(it.clone()), 0);
    let rends = src.cur().rends();
    assert_eq!(rends.len(), 2);
    assert!(rends[1].audio);
    // Audio alone, started half way, from the sample shown there.
    it.renditions.clear();
    let src = load(Arc::new(it.clone()), 500 * MS);
    {
        let mut cur = src.cur();
        assert!(cur.place(500 * MS, 0, 0, &[]));
        let (_, end) = cur.next_of(0).unwrap();
        assert!(end > 500 * MS, "starts at a sample ending {end}");
    }
    // Nothing to play: no renditions.
    it.audio.clear();
    let e = source::Source::new(
        Arc::clone(&r.lib),
        Arc::new(it),
        "",
        choice(0),
        list::ListState::default(),
    )
    .unwrap_err();
    assert_eq!(e, source::SourceError::NoMedia);
    // An hour in: the video's last keyframe.
    let src = load(Arc::clone(&bbb), 3600 * SEC);
    {
        let mut cur = src.cur();
        cur.place(3600 * SEC, 0, 0, &[]);
        assert!(cur.at_end(), "the head past the end");
    }
    // NAL units of another length than 4 bytes are not carried.
    let avc = |b: u8| Track::new(Codec::H264, vec![1, 0x64, 0, 0x1f, b]);
    assert!(source::nal_size4(&avc(0xff)) && !source::nal_size4(&avc(0xfd)));
    assert!(!source::nal_size4(&Track::new(Codec::H265, vec![0; 23])));
    assert!(source::nal_size4(&Track::new(Codec::Aac, Vec::new())));
    // A sample larger than the limit.
    let src = load(Arc::clone(&bbb), 0);
    src.cur().place(0, 0, 0, &[]);
    assert_eq!(
        src.read(SEC, 1 << 20, 10).unwrap_err(),
        source::SourceError::Sample
    );
}

/// fit's `TestSlots`: a slot over at the start is passed; one of an item not known is due until
/// started.
#[test]
fn slots() {
    let r = rig();
    let mut c = ch("tv/s", &["films/bbb"]);
    c.schedule = vec![slot("09:00", "sintel"), slot("09:59:57", "nope")];
    r.apply(&[c]);
    let ch = r.m.channel("tv/s").unwrap();
    {
        let mut st = ch.lock();
        // 09:00's sintel, a second long, is over: passed over. 09:59:57's item is not known:
        // due until started, but the channel's start passed it.
        assert!(ch.due_slot(&st, t0()).is_none());
        st.slot_done = i64::MIN;
        let due = ch.due_slot(&st, t0()).map(|(_, s)| s.item);
        assert_eq!(due.as_deref(), Some("nope"));
        st.slot_done = i64::MIN;
        assert!(ch.due_slot(&st, t0() - 3600 * SEC + 2500 * MS).is_some());
        assert!(ch.due_slot(&st, t0() - 3600 * SEC + 4 * SEC).is_none());
    }
    r.run(100 * MS);
    assert_eq!(ch.status().now.unwrap().item, "films/bbb");
}

/// fit's `TestChannelEdges`, what does not compile and what is stale.
#[test]
fn edges() {
    let r = rig();
    let mut bad = ch("x", &["a"]);
    bad.time_zone = "Mars/Base".into();
    assert!(matches!(r.m.apply(&[bad]), Err(ApplyError::Invalid(_))));
    r.apply(&[ch("tv/e", &["films/bbb"])]);
    let c = r.m.channel("tv/e").unwrap();
    assert_eq!(c.name(), "tv/e");
    r.run(100 * MS);
    // A stale timer, a stale load and read.
    let (id, epoch, src) = {
        let st = c.lock();
        (st.timer_id, st.epoch, st.playing().unwrap())
    };
    c.fire(id + 100);
    c.loaded(epoch + 1, &src, Ok(()));
    c.read_done(epoch + 1, &src, Ok((Vec::new(), 0)));
    // A read that fails ends the item where it was read to.
    c.lock().reading = true;
    c.read_done(epoch, &src, Err(source::SourceError::Nothing));
    assert!(c.status().error.contains("nothing"), "{:?}", c.status());
    r.run(2 * SEC);
    assert_eq!(c.status().state, STATE_PLAYING);
    // Removed for good: a start does nothing.
    c.remove();
    c.start();
    assert_eq!(c.status().state, STATE_STOPPED);
    c.stop();
}

/// fit's `TestMetricsAndRegistry`.
#[test]
fn metrics_and_registry() {
    struct W(Vec<String>);
    impl MetricWriter for W {
        fn family(&mut self, _: &str, _: &str, _: &str) {}
        fn sample(&mut self, name: &str, v: f64, labels: &[(&str, &str)]) {
            let l: Vec<String> = labels.iter().map(|(k, v)| format!("{k}={v}")).collect();
            self.0.push(format!("{name}{{{}}} {v}", l.join(",")));
        }
        fn histogram(
            &mut self,
            _: &str,
            _: &akit_observe::metrics::HistogramSnapshot,
            _: &[(&str, &str)],
        ) {
        }
    }
    let r = rig();
    r.apply(&[ch("tv/m", &["films/bbb"])]);
    r.run(100 * MS);
    let mut w = W(Vec::new());
    r.m.write_metrics(&mut w);
    for want in [
        "akit_streaming_channels{state=playing} 1",
        "akit_streaming_channel_items_total{channel=tv/m,result=played} 1",
    ] {
        assert!(w.0.iter().any(|s| s == want), "{want} not in {:?}", w.0);
    }
    let key = KEYS.fetch_add(1, SeqCst);
    register(key, &r.m);
    let other = Arc::new(Manager::new(Options {
        hub: r.hub.clone(),
        libraries: key,
        clock: None,
        seed: 0,
    }));
    unregister(key, &other);
    assert!(lookup(key).is_some_and(|m| Arc::ptr_eq(&m, &r.m)));
    unregister(key, &r.m);
    assert!(lookup(key).is_none());
    r.m.stop();
    assert_eq!(r.m.apply(&[]), Err(ApplyError::Closed));
    assert!(r.m.channels().is_empty());
}

/// fit's `TestSubscribers`.
#[test]
fn subscribers() {
    let r = rig();
    assert_eq!(r.m.subscribers("tv/p"), 0);
    let (_rec, _sub) = r.play("tv/p");
    r.apply(&[ch("tv/p", &["films/bbb"])]);
    r.run(50 * MS);
    assert_eq!(r.m.subscribers("tv/p"), 1);
}

/// fit's `admin/channels_test.go`: the list, a channel, its actions and their refusals.
#[test]
fn admin() {
    let r = rig();
    let body = |resp: &Response| String::from_utf8(resp.body.clone()).unwrap();
    let get = |p: &str| route(r.key, p, &Method::Get, b"");
    let post = |p: &str, b: &str| route(r.key, p, &Method::Post, b.as_bytes());
    assert_eq!(get("").status, 404);
    register(r.key, &r.m);
    r.apply(&[ch("tv/a", &["films/bbb", "sintel"])]);
    r.run(100 * MS);
    let list = get("");
    assert_eq!(list.status, 200);
    let text = body(&list);
    assert!(
        text.starts_with(r#"{"channels":[{"name":"tv/a","library":"lib","state":"playing","order":"sequence","loop":true,"items":2,"schedule":0,"time_zone":"UTC","now":{"item":"films/bbb""#),
        "{text}"
    );
    assert!(text.contains(r#""push":[]"#), "{text}");
    assert_eq!(get("/tv/a").status, 200);
    assert_eq!(get("/tv/none").status, 404);
    assert_eq!(route(r.key, "", &Method::Post, b"").status, 405);
    assert_eq!(route(r.key, "/tv/a", &Method::Delete, b"").status, 405);
    let skip = post("/tv/a", r#"{"action": "skip", "item": "sintel"}"#);
    assert_eq!(skip.status, 200, "{}", body(&skip));
    assert_eq!(
        post("/tv/a", r#"{"action": "skip", "item": "nope"}"#).status,
        404
    );
    assert_eq!(post("/tv/a", r#"{"action": "dance"}"#).status, 400);
    assert_eq!(post("/tv/a", "nonsense").status, 400);
    let stop = post("/tv/a", r#"{"action": "stop"}"#);
    assert!(body(&stop).contains(r#""state":"stopped""#));
    assert_eq!(post("/tv/a", r#"{"action": "skip"}"#).status, 409);
    assert_eq!(post("/tv/a", r#"{"action": "start"}"#).status, 200);
    unregister(r.key, &r.m);
}
