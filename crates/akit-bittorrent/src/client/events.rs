//! The client's events (I157, fit's `bittorrent/events.go` and `admin/btevents.go`): torrents
//! added and removed, a torrent's state changing, completing or failing, a torrent reaching its
//! seed limit, and the alternative rates switching, for the Admin API's `/bittorrent/events`
//! stream and the pages.
//!
//! The role publishes them from its API's calls and its manager's looks, under the role's lock,
//! and publishing never waits: each subscriber is a server-sent event stream written as the event
//! comes (akio's `BodyStream`, which queues what the connection has not taken), and one whose
//! connection has more than [`SUBSCRIBER_BYTES`] waiting is told `lagged` and its stream ended, to
//! reconnect from the last ID it had. The role keeps the last [`EVENT_HISTORY`] events for a
//! subscriber that reconnects with `Last-Event-ID`: the ones it missed first, or `reset` when
//! some are no longer kept (or the role started again), after which it reads the torrents again.
//! A comment goes out every [`HEARTBEAT`] while there is no event (a timer about time: proxies
//! and clients see the stream alive), which is also when a stream whose client went away is
//! noticed and dropped. When the role stops, each stream gets `end` and is finished.

use std::collections::VecDeque;
use std::fmt::Write as _;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, Weak};
use std::time::Duration;

use akio_http::BodyStream;
use akit_config::value::write_string;

use super::lock;

/// The most subscribers at once, fit's `MaxSubscribers`: one more is answered 503.
pub const MAX_SUBSCRIBERS: usize = 32;

/// How many of the last events the role keeps for a subscriber to start after, fit's
/// `EventHistory`.
pub const EVENT_HISTORY: usize = 1024;

/// The bytes a subscriber's connection may have waiting before it is dropped: fit's
/// `SubscriberBuffer` of 256 events, as bytes on the wire (an event is some 300 bytes).
pub const SUBSCRIBER_BYTES: usize = 256 << 10;

/// How often a stream with no event gets a comment, fit's `btHeartbeat`.
pub const HEARTBEAT: Duration = Duration::from_secs(15);

/// The types of the events, fit's.
pub const ADDED: &str = "added";
/// A torrent was removed (`data_deleted` says its files went with it).
pub const REMOVED: &str = "removed";
/// A torrent's state changed from `previous` to `state`.
pub const STATE: &str = "state";
/// A torrent has every piece.
pub const COMPLETED: &str = "completed";
/// A torrent's storage failed (`error`).
pub const ERROR: &str = "error";
/// A torrent reached its seed ratio or seed time and paused.
pub const SEED_LIMIT: &str = "seed_limit";
/// The alternative rates started (`state` `on`) or stopped (`off`) holding.
pub const ALT_SPEED: &str = "alt_speed";

/// Something that happened to the role or one of its torrents, fit's `Event`.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Event {
    /// Numbers the role's events from 1, in the order they happened, since the role started.
    pub id: u64,
    /// When, RFC 3339.
    pub time: String,
    /// One of the types above.
    pub kind: &'static str,
    /// The torrent's ID, name and category, for the events of a torrent.
    pub torrent: String,
    /// Its name.
    pub name: String,
    /// Its category.
    pub category: String,
    /// The torrent's state, or `on` and `off` for [`ALT_SPEED`].
    pub state: String,
    /// The state before, for [`STATE`].
    pub previous: String,
    /// What went wrong.
    pub error: String,
    /// A [`REMOVED`] deleted the torrent's files.
    pub data_deleted: bool,
}

impl Event {
    /// The event as fit's JSON writes it.
    #[must_use]
    pub fn json(&self) -> String {
        let mut b = format!("{{\"id\":{},\"time\":", self.id);
        write_string(&mut b, &self.time);
        b.push_str(",\"type\":");
        write_string(&mut b, self.kind);
        for (k, v) in [
            ("torrent", &self.torrent),
            ("name", &self.name),
            ("category", &self.category),
            ("state", &self.state),
            ("previous", &self.previous),
            ("error", &self.error),
        ] {
            if !v.is_empty() {
                let _ = write!(b, ",\"{k}\":");
                write_string(&mut b, v);
            }
        }
        if self.data_deleted {
            b.push_str(",\"data_deleted\":true");
        }
        b.push('}');
        b
    }

    /// The event as a server-sent event.
    fn sse(&self) -> String {
        format!(
            "id: {}\nevent: {}\ndata: {}\n\n",
            self.id,
            self.kind,
            self.json()
        )
    }
}

/// A notice that is not one of the role's events: `reset`, `lagged`, `end`.
fn notice(event: &str, msg: &str) -> String {
    let mut b = format!("event: {event}\ndata: {{\"message\":");
    write_string(&mut b, msg);
    b.push_str("}\n\n");
    b
}

struct State {
    seq: u64,
    ring: VecDeque<Event>,
    subs: Vec<BodyStream>,
    closed: bool,
    /// A heartbeat is scheduled.
    beat: bool,
}

/// The role's events: the last ones kept, and the streams they go to.
pub(crate) struct Hub {
    st: Mutex<State>,
    /// Subscribers dropped for not taking their events fast enough.
    pub(crate) dropped: AtomicU64,
    me: Weak<Hub>,
}

impl std::fmt::Debug for Hub {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Hub").finish_non_exhaustive()
    }
}

/// Why a subscription was refused.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Refused {
    /// The role stopped.
    Closed,
    /// [`MAX_SUBSCRIBERS`] listen already.
    Full,
    /// The response could not be started (the connection went away).
    Gone,
}

impl Hub {
    pub(crate) fn new() -> Arc<Hub> {
        Arc::new_cyclic(|me| Hub {
            st: Mutex::new(State {
                seq: 0,
                ring: VecDeque::new(),
                subs: Vec::new(),
                closed: false,
                beat: false,
            }),
            dropped: AtomicU64::new(0),
            me: me.clone(),
        })
    }

    /// Writes `b` to each stream, dropping those that failed or fell behind.
    fn write_all(&self, st: &mut State, b: &[u8]) {
        let mut kept = Vec::with_capacity(st.subs.len());
        for body in st.subs.drain(..) {
            if body.queued_bytes() > SUBSCRIBER_BYTES {
                let msg = notice(
                    "lagged",
                    &format!(
                        "more than {SUBSCRIBER_BYTES} bytes of events were waiting: reconnect \
                         with Last-Event-ID"
                    ),
                );
                let _ = body.write(msg.as_bytes());
                body.finish();
                self.dropped.fetch_add(1, Ordering::Relaxed);
                continue;
            }
            if body.write(b).is_ok() {
                kept.push(body);
            }
        }
        st.subs = kept;
    }

    /// Numbers `e`, keeps it, and writes it to every stream. It never waits.
    pub(crate) fn emit(&self, mut e: Event) {
        let mut st = lock(&self.st);
        if st.closed {
            return;
        }
        st.seq = st.seq.saturating_add(1);
        e.id = st.seq;
        if e.time.is_empty() {
            e.time = super::now_rfc3339();
        }
        let b = e.sse();
        if st.ring.len() >= EVENT_HISTORY {
            st.ring.pop_front();
        }
        st.ring.push_back(e);
        if !st.subs.is_empty() {
            self.write_all(&mut st, b.as_bytes());
        }
    }

    /// Subscribes the stream `start` begins, after the event `after` (0 for none): the events
    /// kept since go first, `reset` before them when some in between are no longer kept or
    /// `after` is past the last (the role started again). `start` is called under the hub's lock,
    /// once there is room: it must not block.
    pub(crate) fn subscribe(
        &self,
        after: u64,
        start: impl FnOnce() -> Option<BodyStream>,
    ) -> Result<(), Refused> {
        let mut st = lock(&self.st);
        if st.closed {
            return Err(Refused::Closed);
        }
        if st.subs.len() >= MAX_SUBSCRIBERS {
            return Err(Refused::Full);
        }
        let body = start().ok_or(Refused::Gone)?;
        let mut b = String::from("retry: 5000\n: akit bittorrent events\n\n");
        let oldest = st.ring.front().map_or(st.seq.saturating_add(1), |e| e.id);
        let gap = after > st.seq || (after > 0 && after.saturating_add(1) < oldest);
        if gap {
            b.push_str(&notice(
                "reset",
                "events were missed, or akit started again: read the torrents again",
            ));
        }
        if after > 0 && after < st.seq {
            for e in st.ring.iter().filter(|e| e.id > after) {
                b.push_str(&e.sse());
            }
        }
        if body.write(b.as_bytes()).is_err() {
            return Err(Refused::Gone);
        }
        st.subs.push(body);
        if !st.beat {
            st.beat = self.schedule_beat();
        }
        Ok(())
    }

    /// Schedules the next heartbeat; whether it was.
    fn schedule_beat(&self) -> bool {
        let me = self.me.clone();
        akio::timer::after_send(HEARTBEAT, move || {
            if let Some(h) = me.upgrade() {
                h.beat();
            }
        })
        .is_ok()
    }

    fn beat(&self) {
        let mut st = lock(&self.st);
        st.beat = false;
        if st.closed || st.subs.is_empty() {
            return;
        }
        let b = format!(": heartbeat {}\n\n", super::now_rfc3339());
        self.write_all(&mut st, b.as_bytes());
        if !st.subs.is_empty() {
            st.beat = self.schedule_beat();
        }
    }

    /// Whether the role has not stopped.
    pub(crate) fn open(&self) -> bool {
        !lock(&self.st).closed
    }

    /// The subscribers now.
    pub(crate) fn subscribers(&self) -> usize {
        lock(&self.st).subs.len()
    }

    /// Ends every stream with `end`, as the role stops; later subscriptions are refused.
    pub(crate) fn close(&self) {
        let mut st = lock(&self.st);
        st.closed = true;
        let msg = notice("end", "the bittorrent.client role stopped");
        for body in st.subs.drain(..) {
            let _ = body.write(msg.as_bytes());
            body.finish();
        }
    }
}
