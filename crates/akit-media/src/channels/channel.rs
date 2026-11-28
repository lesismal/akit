//! A channel (fit's `channels/channel.go`): items of a library published into the hub under the
//! channel's name, sent on the channel's timer as each packet's decode time falls due.
//!
//! Every method takes the channel's lock for a few steps and never blocks. Work that calls out,
//! a movie read through the library (which may answer at once, on the caller's thread) or a read
//! submitted to its pool, is queued under the lock and run once it is released.

use std::collections::VecDeque;
use std::sync::atomic::{AtomicU64, Ordering::Relaxed};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError, Weak};

use akio_media::av::{Packet, Time, Tracks};
use akio_media::hub::{PeerInfo, Publisher};
use akit_observe::Zone;

use super::clock::Timer;
use super::list::ListState;
use super::source::{Choice, FROM_ADMIN, FROM_LIST, FROM_SCHEDULE, Source, SourceError};
use super::{Ctx, Settings, Slot};
use crate::library::{self, Library};

/// A channel sending an item.
pub const STATE_PLAYING: &str = "playing";
/// A channel with nothing sent yet of the item it loads.
pub const STATE_LOADING: &str = "loading";
/// A channel with nothing to play now: its list played out, its items not served, or between
/// its slots; it stays published.
pub const STATE_IDLE: &str = "idle";
/// A channel whose stream has another publisher, or that the hub has no room for; it tries
/// again.
pub const STATE_WAITING: &str = "waiting";
/// A channel the Admin API stopped.
pub const STATE_STOPPED: &str = "stopped";

const MS: Time = 1_000_000;
/// How far ahead of its time a packet may be sent, so that a tick sends what falls due within
/// it at once.
pub(crate) const SEND_AHEAD: Time = 5 * MS;
/// How late a packet may be sent before the channel takes it that it paused, its reads too
/// slow, and sends on from there rather than catching up.
const MAX_LATE: Time = 1000 * MS;
/// The most media one read on the pool takes.
const READ_SPAN: Time = 1000 * MS;
/// How soon a read or a load the library's pool refused is tried again.
const RETRY_BUSY: Time = 200 * MS;
/// How soon a channel with nothing to play, or whose stream is taken, tries again.
pub(crate) const RETRY_IDLE: Time = 5000 * MS;
/// The least time between the last packet sent and the first of an item cut in.
const MIN_GAP: Time = MS;
/// How far past a slot's start an item chosen to follow another still starts it.
const SLOT_SLACK: Time = 50 * MS;
/// No time: a wall time before any.
const NEVER: i64 = i64::MIN;

/// The errors of the Admin API's actions on a channel.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ActionError {
    /// A skip of a channel that is stopped.
    Stopped,
    /// A skip to an item its library does not hold.
    NoItem,
}

impl std::fmt::Display for ActionError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            ActionError::Stopped => "the channel is stopped",
            ActionError::NoItem => "the library holds no such item",
        })
    }
}

/// What a channel has read and not sent: a packet, or the start of an item, whose tracks the
/// stream takes there.
enum Entry {
    Pkt(Packet),
    Start(Arc<Source>),
}

/// The item a channel plays.
struct Playing {
    src: Arc<Source>,
    /// When its first packet was due, in nanoseconds since 1970.
    started: i64,
}

type Job = Box<dyn FnOnce() + Send>;

/// A channel's state, under its lock.
pub(crate) struct St {
    pub(crate) set: Arc<Settings>,
    pub(crate) running: bool,
    removed: bool,
    /// Counts what drops the items read and being read: a stop, a skip, a slot cutting in. An
    /// outcome of a read or a load of another generation is dropped.
    pub(crate) epoch: u64,
    publisher: Option<Publisher>,
    /// The stream's tracks, as last set.
    tracks: Option<Tracks>,
    pub_err: String,
    timer: Option<Box<dyn Timer>>,
    timer_at: i64,
    pub(crate) timer_id: u64,
    // The timeline: a packet whose DTS is d is due at anchor_wall + (d - anchor_dts).
    pub(crate) anchored: bool,
    anchor_wall: i64,
    anchor_dts: Time,
    sent_dts: Time,
    /// Where the presentation of the last item read ends, where the next one's starts.
    last_end: Time,
    /// The least DTS of the next item's first packet, after a cut.
    next_dts: Time,
    /// The last item's delay.
    last_delay: Time,
    /// By track, past the DTS of the last packet queued and sent: where the next item's
    /// packets may start.
    queued: Vec<Time>,
    sent: Vec<Time>,
    queue: VecDeque<Entry>,
    q_bytes: u64,
    /// The DTS of the last packet queued.
    q_end: Time,
    /// The item being read.
    src: Option<Arc<Source>>,
    /// A read of `src` on the pool.
    pub(crate) reading: bool,
    /// An item's movies being read.
    pub(crate) loading: bool,
    /// Items that failed in a row.
    fails: usize,
    now: Option<Playing>,
    /// The last item queued, until it starts.
    next: Option<Arc<Source>>,
    /// When a channel that found nothing to play, or a busy pool, chooses again.
    wait_until: i64,
    pub(crate) list: ListState,
    /// The start of the latest slot started, or passed over: a slot whose start is later
    /// starts when it falls due.
    pub(crate) slot_done: i64,
    last_err: String,
    /// Work to run once the lock is released.
    jobs: Vec<Job>,
}

#[cfg(test)]
impl St {
    /// The item playing.
    pub(crate) fn playing(&self) -> Option<Arc<Source>> {
        self.now.as_ref().map(|p| Arc::clone(&p.src))
    }
}

/// A channel of the streaming role: items of a library published into the hub under the
/// channel's name. Its methods never block.
pub struct Channel {
    ctx: Arc<Ctx>,
    name: String,
    me: Weak<Channel>,
    st: Mutex<St>,
    pub(crate) played: AtomicU64,
    pub(crate) failed: AtomicU64,
    pub(crate) underruns: AtomicU64,
}

impl std::fmt::Debug for Channel {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Channel")
            .field("name", &self.name)
            .finish_non_exhaustive()
    }
}

impl Channel {
    pub(crate) fn new(ctx: Arc<Ctx>, set: Arc<Settings>) -> Arc<Channel> {
        let name = set.name.clone();
        Arc::new_cyclic(|me| Channel {
            ctx,
            name,
            me: me.clone(),
            st: Mutex::new(St {
                set,
                running: false,
                removed: false,
                epoch: 0,
                publisher: None,
                tracks: None,
                pub_err: String::new(),
                timer: None,
                timer_at: NEVER,
                timer_id: 0,
                anchored: false,
                anchor_wall: 0,
                anchor_dts: 0,
                sent_dts: 0,
                last_end: 0,
                next_dts: 0,
                last_delay: 0,
                queued: Vec::new(),
                sent: Vec::new(),
                queue: VecDeque::new(),
                q_bytes: 0,
                q_end: 0,
                src: None,
                reading: false,
                loading: false,
                fails: 0,
                now: None,
                next: None,
                wait_until: NEVER,
                list: ListState::default(),
                slot_done: NEVER,
                last_err: String::new(),
                jobs: Vec::new(),
            }),
            played: AtomicU64::new(0),
            failed: AtomicU64::new(0),
            underruns: AtomicU64::new(0),
        })
    }

    /// The channel's name, its stream's path.
    #[must_use]
    pub fn name(&self) -> &str {
        &self.name
    }

    pub(crate) fn lock(&self) -> MutexGuard<'_, St> {
        self.st.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Runs `f` under the lock, then the work it queued.
    pub(crate) fn locked<R>(&self, f: impl FnOnce(&Channel, &mut St) -> R) -> R {
        let (r, jobs) = {
            let mut st = self.lock();
            let r = f(self, &mut st);
            (r, std::mem::take(&mut st.jobs))
        };
        for j in jobs {
            j();
        }
        r
    }

    fn now(&self) -> i64 {
        self.ctx.clock.now()
    }

    fn library(&self, st: &St) -> Option<Arc<Library>> {
        library::lookup(self.ctx.libraries, &st.set.library)
    }

    /// Starts a channel that is stopped: it claims its stream and plays, its list from where
    /// it was. It does nothing to one that runs.
    pub fn start(&self) {
        self.locked(|c, st| {
            if st.running || st.removed {
                return;
            }
            st.running = true;
            st.epoch = st.epoch.wrapping_add(1);
            if st.list.pos >= st.list.order.len() {
                // A list played out, without loop: from the first again.
                st.list.pos = 0;
            }
            let now = c.now();
            c.pass_slots(st, now);
            c.claim(st, now);
        });
    }

    /// Stops the channel: its stream is unpublished and nothing more is read, until
    /// [`start`](Self::start).
    pub fn stop(&self) {
        self.locked(|c, st| c.halt(st));
    }

    fn halt(&self, st: &mut St) {
        if !st.running {
            return;
        }
        st.running = false;
        rewind(st);
        drop_read(st);
        st.now = None;
        disarm(st);
        if let Some(p) = st.publisher.take() {
            p.close();
        }
    }

    /// Ends the channel for good, as a reload that leaves it out does.
    pub(crate) fn remove(&self) {
        self.locked(|c, st| {
            c.halt(st);
            st.removed = true;
        });
    }

    /// Moves the channel to `s`, from its next item. A slot whose start has passed is not
    /// started by it.
    pub(crate) fn apply(&self, s: Arc<Settings>) {
        self.locked(|c, st| {
            st.set = s;
            if !st.running {
                return;
            }
            let now = c.now();
            c.pass_slots(st, now);
            if st.publisher.is_some() && st.src.is_none() && !st.loading && st.next.is_none() {
                // Nothing chosen to come: choose now, from the new settings.
                st.wait_until = NEVER;
                c.arm(st, now);
            }
        });
    }

    /// Ends what the channel plays, and plays `item` next, or the next item of its list when
    /// `item` is empty.
    ///
    /// # Errors
    ///
    /// A channel stopped, or an item its library does not hold.
    pub fn skip(&self, item: &str) -> Result<(), ActionError> {
        self.locked(|c, st| {
            if !st.running {
                return Err(ActionError::Stopped);
            }
            let now = c.now();
            if !item.is_empty() {
                let held = c.library(st).is_some_and(|l| l.item(item).is_some());
                if !held {
                    return Err(ActionError::NoItem);
                }
                c.cut(
                    st,
                    now,
                    Some(Choice {
                        item: item.to_owned(),
                        from: FROM_ADMIN,
                        offset: 0,
                        slot: 0,
                    }),
                );
                return Ok(());
            }
            c.cut(st, now, None);
            Ok(())
        })
    }

    /// Makes the channel the publisher of its stream, its tracks to come, and starts playing;
    /// or tries again later when the stream has another publisher, or the hub no room.
    fn claim(&self, st: &mut St, now: i64) {
        match self
            .ctx
            .hub
            .reserve(&self.name, PeerInfo::new("channel", None, ""))
        {
            Err(e) => {
                st.pub_err = e.to_string();
                self.arm(st, now.saturating_add(RETRY_IDLE));
            }
            Ok(p) => {
                st.publisher = Some(p);
                st.pub_err.clear();
                st.tracks = None;
                self.advance(st, now, true);
            }
        }
    }

    /// The channel time now: where the timeline is at, or, before anything was sent, where it
    /// starts.
    pub(crate) fn channel_now(st: &St, now: i64) -> Time {
        if !st.anchored {
            return origin(st);
        }
        st.anchor_dts
            .saturating_add(now.saturating_sub(st.anchor_wall))
    }

    /// When channel time `d` falls due: before anything was sent, as though the first packet
    /// were sent now.
    fn wall_of(st: &St, now: i64, d: Time) -> i64 {
        if !st.anchored {
            return now.saturating_add(d.saturating_sub(origin(st)));
        }
        st.anchor_wall
            .saturating_add(d.saturating_sub(st.anchor_dts))
    }

    /// Chooses the item to play after what was read, and loads it; `join` lets it join a slot
    /// under way. With nothing to play the channel idles.
    fn advance(&self, st: &mut St, now: i64, join: bool) {
        // When the next item's presentation starts, if its delay is the last one's.
        let from = st
            .next_dts
            .max(Self::channel_now(st, now))
            .saturating_add(st.last_delay);
        let at = Self::wall_of(st, now, st.last_end.max(from));
        match self.choose(st, at, join) {
            None => self.idle(st, now),
            Some(ch) => self.load(st, now, ch),
        }
    }

    /// Arms the channel's timer for when it has something to play: its next slot, or
    /// [`RETRY_IDLE`] from now.
    fn idle(&self, st: &mut St, now: i64) {
        self.wait(st, now.saturating_add(RETRY_IDLE));
        if let Some(next) = self.next_slot(st, now) {
            self.arm(st, next);
        }
    }

    /// Makes the channel choose its next item no sooner than `at`.
    fn wait(&self, st: &mut St, at: i64) {
        st.wait_until = at;
        self.arm(st, at);
    }

    /// The item to play from `at`: a slot that starts then, or has started and not been, joined
    /// where it is with `join`; or the next item of the list; `None` when there is none.
    fn choose(&self, st: &mut St, at: i64, join: bool) -> Option<Choice> {
        if let Some((start, sl)) = self.due_slot(st, at) {
            let off = at.saturating_sub(start);
            if join || off <= SLOT_SLACK {
                st.slot_done = start;
                return Some(Choice {
                    item: sl.item,
                    from: FROM_SCHEDULE,
                    offset: off.max(0),
                    slot: start,
                });
            }
        }
        let set = Arc::clone(&st.set);
        let ctx = Arc::clone(&self.ctx);
        let mut shuffle = |v: &mut [usize]| ctx.shuffle(v);
        let id = st
            .list
            .next(&set.items, set.shuffle, set.looped, &mut shuffle)?;
        Some(Choice {
            item: id,
            from: FROM_LIST,
            offset: 0,
            slot: 0,
        })
    }

    /// The slot with the latest start at or before `at`, when it is later than `slot_done` and
    /// its item, when known, would still be playing at `at`.
    pub(crate) fn due_slot(&self, st: &St, at: i64) -> Option<(i64, Slot)> {
        let s = &st.set;
        let mut best: Option<(i64, &Slot)> = None;
        for sl in &s.slots {
            if let Some(start) = super::at::prev(&sl.at, &s.zone, at) {
                if best.is_none_or(|(b, _)| start >= b) {
                    best = Some((start, sl));
                }
            }
        }
        let (start, sl) = best?;
        if start <= st.slot_done {
            return None;
        }
        let d = self.item_duration(st, &sl.item);
        if d > 0 && at.saturating_sub(start) >= d {
            return None;
        }
        Some((start, sl.clone()))
    }

    /// Marks the slots whose start has passed as started, but for one under way, which the
    /// channel's next choice joins.
    fn pass_slots(&self, st: &mut St, now: i64) {
        let set = Arc::clone(&st.set);
        for sl in &set.slots {
            let Some(start) = super::at::prev(&sl.at, &set.zone, now) else {
                continue;
            };
            if start <= st.slot_done {
                continue;
            }
            let d = self.item_duration(st, &sl.item);
            if d > 0 && now.saturating_sub(start) < d && st.now.is_none() {
                // Under way, to be joined.
                continue;
            }
            st.slot_done = start;
        }
    }

    /// The first slot start after `t`.
    pub(crate) fn next_slot(&self, st: &St, t: i64) -> Option<i64> {
        let s = &st.set;
        s.slots
            .iter()
            .filter_map(|sl| super::at::next(&sl.at, &s.zone, t))
            .min()
    }

    /// The first slot after `t`, and its item.
    fn next_slot_item(st: &St, t: i64) -> Option<(i64, String)> {
        let s = &st.set;
        s.slots
            .iter()
            .filter_map(|sl| super::at::next(&sl.at, &s.zone, t).map(|n| (n, sl.item.clone())))
            .min_by_key(|(n, _)| *n)
    }

    /// How long the library's item `id` plays; 0 when it is not known.
    pub(crate) fn item_duration(&self, st: &St, id: &str) -> Time {
        self.library(st).and_then(|l| l.item(id)).map_or(0, |it| {
            i64::try_from(it.duration.as_nanos()).unwrap_or(i64::MAX)
        })
    }

    /// Starts loading `ch`'s item: its tracks picked and their movies read, then placed after
    /// what was read.
    fn load(&self, st: &mut St, now: i64, ch: Choice) {
        let src = match self.library(st) {
            None => Err(SourceError::NoLibrary),
            Some(l) => {
                if l.catalog().scanned.is_none() {
                    // The first scan has not ended: the item is chosen again then.
                    st.list.undo(&ch);
                    st.last_err = SourceError::NotReady.to_string();
                    self.wait(st, now.saturating_add(RETRY_BUSY));
                    return;
                }
                match l.item(&ch.item) {
                    None => Err(SourceError::NoItem),
                    Some(it) => Source::new(l, it, &st.set.rendition, ch.clone(), st.list.clone()),
                }
            }
        };
        let src = match src {
            Ok(s) => Arc::new(s),
            Err(e) => return self.load_failed(st, now, &format!("{}: {e}", ch.item)),
        };
        st.loading = true;
        let epoch = st.epoch;
        let me = self.me.clone();
        st.jobs.push(Box::new(move || {
            let s2 = Arc::clone(&src);
            src.load(move |r| {
                if let Some(c) = me.upgrade() {
                    c.loaded(epoch, &s2, r);
                }
            });
        }));
    }

    /// Skips an item that cannot be played, choosing the next, or idles once every item in a
    /// row failed.
    fn load_failed(&self, st: &mut St, now: i64, err: &str) {
        self.failed.fetch_add(1, Relaxed);
        st.fails = st.fails.saturating_add(1);
        st.last_err = err.to_owned();
        akit_observe::warn!(Server, "channel {}: skipped: {err}", self.name);
        if st.fails > st.set.items.len() {
            st.fails = 0;
            self.idle(st, now);
            return;
        }
        let join = st.now.is_none() && st.queue.is_empty();
        self.advance(st, now, join);
    }

    /// Places `src` on the timeline once its movies are read, queues its start and reads it.
    pub(crate) fn loaded(&self, epoch: u64, src: &Arc<Source>, r: Result<(), SourceError>) {
        self.locked(|c, st| {
            if epoch != st.epoch || !st.running {
                return;
            }
            st.loading = false;
            let now = c.now();
            match r {
                Err(SourceError::Library(library::Error::Busy)) => {
                    st.list.undo(&src.choice);
                    c.wait(st, now.saturating_add(RETRY_BUSY));
                    return;
                }
                Err(e) => return c.load_failed(st, now, &format!("{}: {e}", src.choice.item)),
                Ok(()) => {}
            }
            let mut dts = st.next_dts;
            if st.anchored {
                dts = dts.max(Self::channel_now(st, now));
            }
            let (base, delay, end) = {
                let mut cur = src.cur();
                if !cur.place(src.choice.offset, st.last_end, dts, &st.queued) {
                    drop(cur);
                    return c.load_failed(
                        st,
                        now,
                        &format!("{}: {}", src.choice.item, SourceError::Nothing),
                    );
                }
                (cur.base, cur.delay, cur.end)
            };
            // A list item, or a slot's, is cut where the next slot starts.
            let pres = base.saturating_add(delay);
            let at = Self::wall_of(st, now, pres);
            if let Some(next) = c.next_slot(st, at) {
                let cut = pres.saturating_add(next.saturating_sub(at));
                if cut < end {
                    src.cur().cut = cut;
                }
            }
            st.last_delay = delay;
            st.fails = 0;
            st.src = Some(Arc::clone(src));
            st.next = Some(Arc::clone(src));
            st.queue.push_back(Entry::Start(Arc::clone(src)));
            st.q_end = base;
            c.read(st, now);
            c.arm(st, now);
        });
    }

    /// How far ahead of now what was read reaches.
    fn buffered(st: &St, now: i64) -> Time {
        if st.queue.is_empty() {
            return 0;
        }
        st.q_end
            .saturating_sub(Self::channel_now(st, now).max(st.sent_dts))
    }

    /// Whether less than `read_ahead` and `max_buffer_bytes` is read ahead.
    fn want_more(st: &St, now: i64) -> bool {
        Self::buffered(st, now) < st.set.read_ahead && st.q_bytes < st.set.max_bytes
    }

    /// Reads more of the item being read on the library's pool, when less than `read_ahead`
    /// and `max_buffer_bytes` is read ahead.
    pub(crate) fn read(&self, st: &mut St, now: i64) {
        let Some(src) = st.src.clone() else { return };
        if st.reading || !Self::want_more(st, now) {
            return;
        }
        st.reading = true;
        let epoch = st.epoch;
        let max = st.set.max_bytes;
        let budget = max.saturating_sub(st.q_bytes).max(1);
        let me = self.me.clone();
        let busy_me = self.me.clone();
        let s1 = Arc::clone(&src);
        let s2 = Arc::clone(&src);
        let run: Job = Box::new(move || {
            let r = s1.read(READ_SPAN, budget, max);
            if let Some(c) = me.upgrade() {
                c.read_done(epoch, &s1, r);
            }
        });
        let busy: Job = Box::new(move || {
            if let Some(c) = busy_me.upgrade() {
                c.read_done(epoch, &s2, Err(SourceError::Library(library::Error::Busy)));
            }
        });
        if !src.lib.submit(run, Some(busy)) {
            st.reading = false;
            self.arm(st, now.saturating_add(RETRY_BUSY));
        }
    }

    /// Queues what a read gave, and loads the next item once the one read has been read whole.
    pub(crate) fn read_done(
        &self,
        epoch: u64,
        src: &Arc<Source>,
        r: Result<(Vec<Packet>, u64), SourceError>,
    ) {
        self.locked(|c, st| {
            if epoch != st.epoch || !st.running {
                return;
            }
            st.reading = false;
            let now = c.now();
            let (pkts, n, err) = match r {
                Ok((p, n)) => (p, n, None),
                Err(e) => (Vec::new(), 0, Some(e)),
            };
            for p in pkts {
                st.q_end = p.dts;
                if st.queued.len() <= p.track {
                    st.queued.resize(p.track.saturating_add(1), 0);
                }
                if let Some(q) = st.queued.get_mut(p.track) {
                    *q = p.dts.saturating_add(MIN_GAP);
                }
                st.queue.push_back(Entry::Pkt(p));
            }
            st.q_bytes = st.q_bytes.saturating_add(n);
            match err {
                Some(SourceError::Library(library::Error::Busy)) => {
                    c.arm(st, now.saturating_add(RETRY_BUSY));
                    return;
                }
                Some(e) => {
                    // The item ends where it was read to.
                    akit_observe::warn!(
                        Server,
                        "channel {}: {} ends early: {e}",
                        c.name,
                        src.choice.item
                    );
                    st.last_err = e.to_string();
                    let mut cur = src.cur();
                    cur.done = true;
                    cur.end = st.q_end.max(cur.base);
                    cur.cut = 0;
                    cur.cut_hit = false;
                }
                None => {}
            }
            let (done, end, cut, cut_hit) = {
                let cur = src.cur();
                (cur.done, cur.end, cur.cut, cur.cut_hit)
            };
            if done {
                st.last_end = if cut_hit { cut } else { end };
                st.next_dts = 0;
                st.src = None;
                if Self::want_more(st, now) {
                    c.advance(st, now, false);
                }
            } else {
                c.read(st, now);
            }
            if !st.queue.is_empty() {
                c.arm(st, now);
            }
        });
    }

    /// Ends what plays now, and plays `ch`, or the next item when `None`: a skip, or a slot
    /// falling due.
    fn cut(&self, st: &mut St, now: i64, ch: Option<Choice>) {
        rewind(st);
        drop_read(st);
        st.last_end = 0;
        st.next_dts = Self::channel_now(st, now);
        if st.anchored && st.next_dts <= st.sent_dts {
            st.next_dts = st.sent_dts.saturating_add(MIN_GAP);
        }
        if st.publisher.is_none() {
            return;
        }
        match ch {
            None => self.advance(st, now, false),
            Some(ch) => self.load(st, now, ch),
        }
    }

    /// Sets the channel's timer for `at`, unless it is set for earlier.
    fn arm(&self, st: &mut St, at: i64) {
        if st.timer.is_some() && st.timer_at <= at {
            return;
        }
        disarm(st);
        st.timer_id = st.timer_id.wrapping_add(1);
        let id = st.timer_id;
        st.timer_at = at;
        let me = self.me.clone();
        let d = at.saturating_sub(self.now()).max(0);
        st.timer = Some(self.ctx.clock.after(
            d,
            Box::new(move || {
                if let Some(c) = me.upgrade() {
                    c.fire(id);
                }
            }),
        ));
    }

    /// The channel's timer.
    pub(crate) fn fire(&self, id: u64) {
        self.locked(|c, st| {
            if id != st.timer_id {
                return;
            }
            st.timer = None;
            if !st.running {
                return;
            }
            let now = c.now();
            c.tick(st, now);
        });
    }

    /// Does what is due: a slot cutting in, the packets due sent, the next item chosen and
    /// read, and the timer set for what falls due next.
    pub(crate) fn tick(&self, st: &mut St, now: i64) {
        if st.publisher.is_none() {
            return self.claim(st, now);
        }
        if let Some((start, sl)) = self.due_slot(st, now) {
            st.slot_done = start;
            return self.cut(
                st,
                now,
                Some(Choice {
                    item: sl.item,
                    from: FROM_SCHEDULE,
                    offset: now.saturating_sub(start).max(0),
                    slot: start,
                }),
            );
        }
        self.send(st, now);
        if st.src.is_none() && !st.loading && now >= st.wait_until && Self::want_more(st, now) {
            let join = st.queue.is_empty();
            self.advance(st, now, join);
        }
        self.read(st, now);
        if st.queue.is_empty() && st.src.is_none() && !st.loading {
            st.now = None;
        }
        match st.queue.front() {
            Some(Entry::Start(_)) => self.arm(st, now),
            Some(Entry::Pkt(p)) => {
                let at = Self::wall_of(st, now, p.dts).saturating_sub(SEND_AHEAD);
                self.arm(st, at);
            }
            None => {}
        }
        if let Some(next) = self.next_slot(st, now) {
            self.arm(st, next);
        }
    }

    /// Sends the packets that are due, and starts the items whose first packet is.
    fn send(&self, st: &mut St, now: i64) {
        while let Some(e) = st.queue.front() {
            match e {
                Entry::Start(src) => {
                    let src = Arc::clone(src);
                    st.queue.pop_front();
                    let (base, avs) = {
                        let cur = src.cur();
                        (cur.base, cur.avs.clone())
                    };
                    if !st.anchored {
                        st.anchored = true;
                        st.anchor_wall = now;
                        st.anchor_dts = base;
                    }
                    if let Some(avs) = avs {
                        if !same_tracks(st.tracks.as_ref(), &avs) {
                            // New codec configurations: players take them, and HLS marks a
                            // discontinuity. An item whose tracks are the last one's carries
                            // on without, its timestamps running on.
                            if let Some(p) = &st.publisher {
                                p.set_tracks(avs.clone());
                            }
                            st.tracks = Some(avs);
                        }
                    }
                    let started = Self::wall_of(st, now, base);
                    if st.next.as_ref().is_some_and(|n| Arc::ptr_eq(n, &src)) {
                        st.next = None;
                    }
                    akit_observe::info!(
                        Server,
                        "channel {}: playing {} ({})",
                        self.name,
                        src.item.id,
                        src.choice.from
                    );
                    st.now = Some(Playing { src, started });
                    self.played.fetch_add(1, Relaxed);
                    st.last_err.clear();
                }
                Entry::Pkt(p) => {
                    let due = Self::wall_of(st, now, p.dts);
                    if due > now.saturating_add(SEND_AHEAD) {
                        break;
                    }
                    let Some(Entry::Pkt(p)) = st.queue.pop_front() else {
                        break;
                    };
                    let late = now.saturating_sub(due);
                    if late > MAX_LATE {
                        // It paused, its reads too slow: it goes on from here.
                        self.underruns.fetch_add(1, Relaxed);
                        st.anchor_wall = now;
                        st.anchor_dts = p.dts;
                        if let Some(n) = &mut st.now {
                            n.started = n.started.saturating_add(late);
                        }
                    }
                    st.sent_dts = st.sent_dts.max(p.dts);
                    if st.sent.len() <= p.track {
                        st.sent.resize(p.track.saturating_add(1), 0);
                    }
                    if let Some(s) = st.sent.get_mut(p.track) {
                        *s = p.dts.saturating_add(MIN_GAP);
                    }
                    st.q_bytes = st.q_bytes.saturating_sub(p.data.len() as u64);
                    if let Some(pb) = &st.publisher {
                        pb.write(p);
                    }
                }
            }
        }
    }

    /// What the channel is doing.
    #[must_use]
    pub fn status(&self) -> ChannelStatus {
        let st = self.lock();
        let s = &st.set;
        let now = self.now();
        let mut out = ChannelStatus {
            name: self.name.clone(),
            library: s.library.clone(),
            state: STATE_IDLE,
            error: String::new(),
            order: if s.shuffle { "shuffle" } else { "sequence" },
            looped: s.looped,
            items: s.items.len(),
            schedule: s.slots.len(),
            time_zone: s.zone.name().to_owned(),
            zone: s.zone.clone(),
            now: None,
            next: None,
            next_slot: None,
            buffered: 0,
            buffered_bytes: 0,
            played: self.played.load(Relaxed),
            failed: self.failed.load(Relaxed),
            underruns: self.underruns.load(Relaxed),
        };
        out.state = if !st.running {
            STATE_STOPPED
        } else if st.publisher.is_none() {
            out.error.clone_from(&st.pub_err);
            STATE_WAITING
        } else if st.now.is_some() {
            STATE_PLAYING
        } else if st.loading || st.src.is_some() || !st.queue.is_empty() {
            STATE_LOADING
        } else {
            STATE_IDLE
        };
        if out.error.is_empty() {
            out.error.clone_from(&st.last_err);
        }
        if let Some(p) = &st.now {
            let mut is = item_status(&p.src, p.started);
            is.position = now
                .saturating_sub(p.started)
                .max(0)
                .saturating_add(is.offset);
            out.now = Some(is);
        }
        if let Some(n) = &st.next {
            if st.now.as_ref().is_none_or(|p| !Arc::ptr_eq(&p.src, n)) {
                let base = n.cur().base;
                out.next = Some(item_status(n, Self::wall_of(&st, now, base)));
            }
        }
        if st.running {
            out.buffered = Self::buffered(&st, now).max(0);
            out.buffered_bytes = st.q_bytes;
            out.next_slot = Self::next_slot_item(&st, now);
        }
        out
    }
}

/// Puts the list back where it was when the item playing was taken from it, so that items read
/// ahead and dropped are played.
fn rewind(st: &mut St) {
    if let Some(p) = &st.now {
        st.list = p.src.list_after.clone();
    }
}

/// Drops what was read and is being read.
fn drop_read(st: &mut St) {
    st.epoch = st.epoch.wrapping_add(1);
    st.queued.clone_from(&st.sent);
    st.queue.clear();
    st.q_bytes = 0;
    st.src = None;
    st.next = None;
    st.reading = false;
    st.loading = false;
}

fn disarm(st: &mut St) {
    if let Some(t) = st.timer.take() {
        t.stop();
    }
}

/// Where the timeline starts once the first packet is sent: the first item queued, or the next
/// item's.
fn origin(st: &St) -> Time {
    match st.queue.front() {
        Some(Entry::Start(s)) => s.cur().base,
        Some(Entry::Pkt(p)) => p.dts,
        None => st.next_dts,
    }
}

/// Whether `a` are the tracks `b` are: the same codecs with the same configurations.
fn same_tracks(a: Option<&Tracks>, b: &Tracks) -> bool {
    a.is_some_and(|a| {
        a.len() == b.len()
            && a.iter().zip(b.iter()).all(|(x, y)| {
                x.codec == y.codec
                    && x.config == y.config
                    && x.width == y.width
                    && x.height == y.height
                    && x.sample_rate == y.sample_rate
                    && x.channels == y.channels
            })
    })
}

fn item_status(src: &Source, started: i64) -> ItemStatus {
    ItemStatus {
        item: src.item.id.clone(),
        title: src.item.title.clone(),
        rendition: src.rendition.clone(),
        from: src.choice.from,
        slot: (src.choice.from == FROM_SCHEDULE).then_some(src.choice.slot),
        offset: src.choice.offset,
        duration: i64::try_from(src.item.duration.as_nanos()).unwrap_or(i64::MAX),
        position: 0,
        started,
    }
}

/// What a channel is doing, as the Admin API shows it (fit's `ChannelStatus`).
#[derive(Debug, Clone)]
pub struct ChannelStatus {
    /// Its name.
    pub name: String,
    /// Its library.
    pub library: String,
    /// `playing`, `loading`, `idle`, `waiting` or `stopped`.
    pub state: &'static str,
    /// Why it waits, or the last item it skipped or ended early.
    pub error: String,
    /// `sequence` or `shuffle`.
    pub order: &'static str,
    /// `loop`.
    pub looped: bool,
    /// The items listed.
    pub items: usize,
    /// The slots of its schedule.
    pub schedule: usize,
    /// Its time zone's name.
    pub time_zone: String,
    zone: Zone,
    /// The item playing.
    pub now: Option<ItemStatus>,
    /// The item read to follow it.
    pub next: Option<ItemStatus>,
    /// The schedule's next start, in nanoseconds since 1970, and its item.
    pub next_slot: Option<(i64, String)>,
    /// How far ahead of what was sent the channel has read, in nanoseconds.
    pub buffered: Time,
    /// Its bytes.
    pub buffered_bytes: u64,
    /// Items started.
    pub played: u64,
    /// Items skipped as unplayable.
    pub failed: u64,
    /// Pauses for reads too slow.
    pub underruns: u64,
}

/// An item a channel plays, or will (fit's `ItemStatus`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ItemStatus {
    /// Its ID.
    pub item: String,
    /// Its title.
    pub title: String,
    /// The rendition played.
    pub rendition: String,
    /// `list`, `schedule` or `admin`.
    pub from: &'static str,
    /// The start of the slot it plays, from the schedule.
    pub slot: Option<i64>,
    /// Where in the item it started, in nanoseconds.
    pub offset: Time,
    /// Its length.
    pub duration: Time,
    /// Where it is now.
    pub position: Time,
    /// When its first packet was due, or will be, in nanoseconds since 1970.
    pub started: i64,
}

impl ChannelStatus {
    /// The status as the Admin API writes it, its push targets `push`.
    #[must_use]
    pub fn to_value(&self, push: Vec<akit_config::value::Value>) -> akit_config::value::Value {
        use akit_config::value::Value;
        let secs = |t: Time| Value::Number(format_secs(t));
        let time = |t: i64| Value::String(super::at::rfc3339(&self.zone, t));
        let item = |i: &ItemStatus| {
            let mut m = vec![
                ("item".to_owned(), Value::String(i.item.clone())),
                ("title".to_owned(), Value::String(i.title.clone())),
                ("rendition".to_owned(), Value::String(i.rendition.clone())),
                ("from".to_owned(), Value::String(i.from.to_owned())),
            ];
            if let Some(s) = i.slot {
                m.push(("slot".to_owned(), time(s)));
            }
            m.push(("offset".to_owned(), secs(i.offset)));
            m.push(("duration".to_owned(), secs(i.duration)));
            if i.position != 0 {
                m.push(("position".to_owned(), secs(i.position)));
            }
            m.push(("started".to_owned(), time(i.started)));
            Value::Object(m)
        };
        let mut m = vec![
            ("name".to_owned(), Value::String(self.name.clone())),
            ("library".to_owned(), Value::String(self.library.clone())),
            ("state".to_owned(), Value::String(self.state.to_owned())),
        ];
        if !self.error.is_empty() {
            m.push(("error".to_owned(), Value::String(self.error.clone())));
        }
        m.push(("order".to_owned(), Value::String(self.order.to_owned())));
        m.push(("loop".to_owned(), Value::Bool(self.looped)));
        m.push(("items".to_owned(), Value::Number(self.items.to_string())));
        m.push((
            "schedule".to_owned(),
            Value::Number(self.schedule.to_string()),
        ));
        m.push((
            "time_zone".to_owned(),
            Value::String(self.time_zone.clone()),
        ));
        if let Some(i) = &self.now {
            m.push(("now".to_owned(), item(i)));
        }
        if let Some(i) = &self.next {
            m.push(("next".to_owned(), item(i)));
        }
        if let Some((at, it)) = &self.next_slot {
            m.push((
                "next_slot".to_owned(),
                Value::Object(vec![
                    ("at".to_owned(), time(*at)),
                    ("item".to_owned(), Value::String(it.clone())),
                ]),
            ));
        }
        m.push(("buffered".to_owned(), secs(self.buffered)));
        m.push((
            "buffered_bytes".to_owned(),
            Value::Number(self.buffered_bytes.to_string()),
        ));
        m.push(("played".to_owned(), Value::Number(self.played.to_string())));
        m.push(("failed".to_owned(), Value::Number(self.failed.to_string())));
        m.push((
            "underruns".to_owned(),
            Value::Number(self.underruns.to_string()),
        ));
        m.push(("push".to_owned(), Value::Array(push)));
        Value::Object(m)
    }
}

/// Seconds of `t` nanoseconds, as Go's encoding/json writes a float64.
pub(crate) fn format_secs(t: Time) -> String {
    if t < 0 {
        return format!("-{}", format_secs(t.saturating_neg()));
    }
    library::catalog::seconds_text(std::time::Duration::from_nanos(
        u64::try_from(t).unwrap_or(0),
    ))
}
