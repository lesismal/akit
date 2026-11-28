//! Push targets (I146, fit's `channels/push.go`): a stream of the hub relayed to an outside RTMP
//! or SRT server, connected again when it drops.
//!
//! An SRT target is dialed on the role's engine with a session handler of the push's own, which
//! hears the handshake end and the session close: the push then subscribes to its stream, muxing
//! it into MPEG-TS as akio-media's own SRT push does, or waits its backoff on the manager's clock
//! and dials again, `retry_min` doubling up to `retry_max` as fit's.
//!
//! An RTMP target is akio-media's RTMP push, which tells its owner nothing as a session ends:
//! until it does (akio API request A298) the push hands its retries to the client's own
//! [`Reconnect`](akio_media::rtmp::client::Reconnect), the same backoff, and watches its stream
//! in the hub, starting a client again when a publisher comes back after the last one left. A
//! target that refuses the push for its credentials ends it until then; its status is read from
//! the client.
//!
//! Dials resolve host names, which blocks: they run on akit-media's pool, never on an I/O thread
//! or the clock's.

use std::net::{SocketAddr, ToSocketAddrs};
use std::sync::atomic::{AtomicU64, Ordering::Relaxed};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError, Weak};
use std::time::Duration;

use akio::Engine;
use akio_media::av::{Packet, Time, Tracks};
use akio_media::hub::{HubError, PeerInfo, Sink, SubscribeOptions, Subscription, Timeline};
use akio_media::mpegts::Muxer;
use akio_media::rtmp::client::{
    self as rtmp, ClientState, Reconnect, RtmpClient, RtmpClientConfig,
};
use akio_media::srt::{self, SrtError, SrtHandler, SrtSession, TS_PAYLOAD_SIZE};
use akit_config::channels::{PushTarget, PushUrl};
use akit_config::value::Value;

use super::Ctx;
use super::clock::Timer;

/// A target that took the stream and is sent it.
pub const PUSH_CONNECTED: &str = "connected";
/// A target being dialed.
pub const PUSH_CONNECTING: &str = "connecting";
/// A target that refused, dropped or could not be reached, waiting to be tried again.
pub const PUSH_RETRYING: &str = "retrying";
/// A target whose stream is not published, waiting for it.
pub const PUSH_WAITING: &str = "waiting";
/// A target the Admin API stopped.
pub const PUSH_STOPPED: &str = "stopped";

/// How long a push keeps its connection while its stream has no publisher, as a channel stopped
/// and started again or a publisher that reconnects.
const PUSH_WAIT: Duration = Duration::from_secs(5);
/// How long the watch of an RTMP push's stream waits for a publisher: as long as the push runs.
const WATCH_WAIT: Duration = Duration::from_secs(100 * 365 * 86_400);

const NOT_PUBLISHED: &str = "the stream is not published";
const NO_ENGINE: &str = "no engine to dial from";

/// A push's connection.
enum Conn {
    Rtmp(RtmpClient),
    Srt {
        session: SrtSession,
        sub: Option<Subscription>,
    },
}

/// A push's state, under its lock.
struct St {
    t: PushTarget,
    running: bool,
    removed: bool,
    /// Counts attempts and stops: a callback of another epoch is of an attempt that is over.
    epoch: u64,
    state: &'static str,
    err: String,
    /// When the state began, in nanoseconds since 1970.
    since: i64,
    backoff: Time,
    retry_at: i64,
    timer: Option<Box<dyn Timer>>,
    conn: Option<Conn>,
    /// The RTMP push's watch of its stream.
    watch: Option<Subscription>,
    /// The bytes of the SRT sessions ended; the bytes when the connection began.
    srt_sent: u64,
    bytes0: u64,
    /// The RTMP clients' sessions that streamed, of the clients ended.
    rtmp_connects: u64,
}

/// A push target: a stream of the hub relayed to an outside RTMP or SRT server. Its methods
/// never block.
pub struct Push {
    ctx: Arc<Ctx>,
    dial: Option<Arc<Engine>>,
    url: PushUrl,
    name: String,
    stream: String,
    me: Weak<Push>,
    st: Mutex<St>,
    pub(crate) connects: AtomicU64,
    pub(crate) failures: AtomicU64,
}

impl std::fmt::Debug for Push {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Push")
            .field("name", &self.name)
            .field("url", &self.url.redacted())
            .finish_non_exhaustive()
    }
}

fn nanos(d: Duration) -> Time {
    i64::try_from(d.as_nanos()).unwrap_or(i64::MAX)
}

impl Push {
    pub(crate) fn new(
        ctx: Arc<Ctx>,
        dial: Option<Arc<Engine>>,
        t: PushTarget,
        url: PushUrl,
    ) -> Arc<Push> {
        let backoff = nanos(t.push.retry_min_or());
        Arc::new_cyclic(|me| Push {
            ctx,
            dial,
            url,
            name: t.push.name.clone(),
            stream: t.push.stream.clone(),
            me: me.clone(),
            st: Mutex::new(St {
                t,
                running: false,
                removed: false,
                epoch: 0,
                state: PUSH_STOPPED,
                err: String::new(),
                since: 0,
                backoff,
                retry_at: 0,
                timer: None,
                conn: None,
                watch: None,
                srt_sent: 0,
                bytes0: 0,
                rtmp_connects: 0,
            }),
            connects: AtomicU64::new(0),
            failures: AtomicU64::new(0),
        })
    }

    fn lock(&self) -> MutexGuard<'_, St> {
        self.st.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// The target's name.
    #[must_use]
    pub fn name(&self) -> &str {
        &self.name
    }

    /// The stream it relays.
    #[must_use]
    pub fn stream(&self) -> &str {
        &self.stream
    }

    /// Whether `t` is the target as it connects: the same stream, url and timeout.
    pub(crate) fn same(&self, t: &PushTarget) -> bool {
        let st = self.lock();
        st.t.push.stream == t.push.stream
            && st.t.push.url == t.push.url
            && st.t.push.connect() == t.push.connect()
    }

    /// Takes `t`'s retry bounds and channel, for what it connects to is the same.
    pub(crate) fn retarget(&self, t: PushTarget) {
        let mut st = self.lock();
        let (lo, hi) = (nanos(t.push.retry_min_or()), nanos(t.push.retry_max_or()));
        st.backoff = st.backoff.max(lo).min(hi);
        st.t = t;
    }

    fn set(&self, st: &mut St, state: &'static str, err: String) {
        if st.state != state {
            st.since = self.ctx.clock.now();
        }
        st.state = state;
        st.err = err;
    }

    /// Starts a target that is stopped.
    pub fn start(&self) {
        let epoch = {
            let mut st = self.lock();
            if st.running || st.removed {
                return;
            }
            st.running = true;
            st.backoff = nanos(st.t.push.retry_min_or());
            st.epoch = st.epoch.wrapping_add(1);
            st.epoch
        };
        self.attempt(epoch);
    }

    /// Stops the target: its connection is closed, and it is not tried again until
    /// [`start`](Self::start).
    pub fn stop(&self) {
        let (conn, watch) = {
            let mut st = self.lock();
            if !st.running {
                return;
            }
            st.running = false;
            st.epoch = st.epoch.wrapping_add(1);
            self.set(&mut st, PUSH_STOPPED, String::new());
            release(&mut st)
        };
        close(conn, watch);
    }

    /// Connects the target again at once.
    pub fn restart(&self) {
        self.stop();
        self.start();
    }

    /// Ends the target for good.
    pub(crate) fn remove(&self) {
        self.stop();
        self.lock().removed = true;
    }

    /// Dials the target, or waits for its stream.
    fn attempt(&self, epoch: u64) {
        let mut st = self.lock();
        if epoch != st.epoch || !st.running {
            return;
        }
        let live = self
            .ctx
            .hub
            .stream_info(&self.stream)
            .is_some_and(|i| i.publishing);
        if self.dial.is_none() {
            if !live {
                return self.retry(&mut st, PUSH_WAITING, NOT_PUBLISHED.into(), false);
            }
            return self.retry(&mut st, PUSH_RETRYING, NO_ENGINE.into(), true);
        }
        if self.url.protocol == "rtmp" {
            // The watch of the stream starts a client whenever a publisher comes.
            self.set(&mut st, PUSH_WAITING, NOT_PUBLISHED.into());
            let me = self.me.clone();
            drop(st);
            submit(move || {
                if let Some(p) = me.upgrade() {
                    p.watch(epoch);
                }
            });
            return;
        }
        if !live {
            return self.retry(&mut st, PUSH_WAITING, NOT_PUBLISHED.into(), false);
        }
        self.set(&mut st, PUSH_CONNECTING, String::new());
        let me = self.me.clone();
        drop(st);
        submit(move || {
            if let Some(p) = me.upgrade() {
                p.dial_srt(epoch);
            }
        });
    }

    /// Arms the timer that tries `epoch` again: after the backoff, which then doubles, up to
    /// `retry_max`, with `grow`; after `retry_min`, for a stream not published, without.
    fn retry(&self, st: &mut St, state: &'static str, err: String, grow: bool) {
        self.set(st, state, err);
        let mut d = nanos(st.t.push.retry_min_or());
        if grow {
            d = st.backoff;
            st.backoff = st
                .backoff
                .saturating_mul(2)
                .min(nanos(st.t.push.retry_max_or()));
        }
        st.retry_at = self.ctx.clock.now().saturating_add(d);
        if let Some(t) = st.timer.take() {
            t.stop();
        }
        let epoch = st.epoch;
        let me = self.me.clone();
        st.timer = Some(self.ctx.clock.after(
            d,
            Box::new(move || {
                if let Some(p) = me.upgrade() {
                    p.attempt(epoch);
                }
            }),
        ));
    }

    /// Ends an attempt of `epoch` that failed, or a connection that dropped, and tries again
    /// after the backoff.
    fn fail(&self, epoch: u64, err: &str) {
        let (conn, watch) = {
            let mut st = self.lock();
            if epoch != st.epoch || !st.running {
                return;
            }
            let up = st.state == PUSH_CONNECTED;
            if up
                && self.ctx.clock.now().saturating_sub(st.since) >= nanos(st.t.push.retry_max_or())
            {
                // It stayed up: the next retry waits the least.
                st.backoff = nanos(st.t.push.retry_min_or());
            }
            self.failures.fetch_add(1, Relaxed);
            st.epoch = st.epoch.wrapping_add(1);
            let msg = self.redact(err, &st);
            let out = release(&mut st);
            self.retry(&mut st, PUSH_RETRYING, msg.clone(), true);
            akit_observe::warn!(
                Server,
                "push {} to {}: {msg}",
                self.name,
                self.url.redacted()
            );
            out
        };
        close(conn, watch);
    }

    /// `err` without the target's URL, a secret.
    fn redact(&self, err: &str, st: &St) -> String {
        if st.t.push.url.is_empty() {
            return err.to_owned();
        }
        err.replace(&st.t.push.url, &self.url.redacted())
    }

    // ---- SRT ----

    /// Dials an SRT target. On akit-media's pool: it resolves the host and waits briefly for
    /// the socket.
    fn dial_srt(&self, epoch: u64) {
        let Some(engine) = self.dial.clone() else {
            return self.fail(epoch, NO_ENGINE);
        };
        let connect = self.lock().t.push.connect();
        let addr = match resolve(&self.url.host) {
            Ok(a) => a,
            Err(e) => return self.fail(epoch, &e),
        };
        let mut cfg = srt::Config::default();
        cfg.latency = self.url.latency;
        cfg.stream_id.clone_from(&self.url.stream_id);
        if !self.url.passphrase.is_empty() {
            cfg.passphrase = Some(self.url.passphrase.clone());
        }
        if self.url.pbkeylen != 0 {
            cfg.pbkeylen = self.url.pbkeylen as usize;
        }
        cfg.handshake_timeout = connect;
        let h = SrtPush {
            push: self.me.clone(),
            epoch,
        };
        match srt::dial(&engine, addr, cfg, h) {
            Err(e) => self.fail(epoch, &e.to_string()),
            Ok(session) => {
                let mut st = self.lock();
                if epoch == st.epoch && st.running {
                    // The handshake may have ended first, and set the connection.
                    if st.conn.is_none() {
                        st.conn = Some(Conn::Srt { session, sub: None });
                    }
                } else {
                    drop(st);
                    session.close();
                }
            }
        }
    }

    /// The SRT handshake ended: the push subscribes to its stream, on akit-media's pool (the
    /// hub hands it the tracks and the cached pictures at once, and the session takes them).
    fn srt_connected(&self, epoch: u64, session: SrtSession) {
        {
            let mut st = self.lock();
            if epoch != st.epoch || !st.running {
                return;
            }
            self.connects.fetch_add(1, Relaxed);
            self.set(&mut st, PUSH_CONNECTED, String::new());
            st.bytes0 = bytes_of(&st);
        }
        let mut o = SubscribeOptions::new(PeerInfo::new("push", Some(session.remote()), ""));
        o.wait = Some(PUSH_WAIT);
        let sink = TsSink {
            session: session.clone(),
            muxer: None,
            timeline: Timeline::new(),
            video: Vec::new(),
            buf: Vec::new(),
        };
        match self.ctx.hub.subscribe(&self.stream, sink, o) {
            Err(e) => self.fail(epoch, &e.to_string()),
            Ok(sub) => {
                let mut st = self.lock();
                if epoch == st.epoch && st.running {
                    match &mut st.conn {
                        Some(Conn::Srt { sub: slot, .. }) => *slot = Some(sub),
                        // Connected before the dial returned it.
                        _ => {
                            st.conn = Some(Conn::Srt {
                                session,
                                sub: Some(sub),
                            });
                        }
                    }
                    return;
                }
                drop(st);
                sub.close();
            }
        }
    }

    // ---- RTMP ----

    /// Watches the stream of an RTMP push: each publisher that comes starts a client, unless
    /// one runs. On akit-media's pool.
    fn watch(&self, epoch: u64) {
        let sink = Watch {
            push: self.me.clone(),
            epoch,
        };
        let mut o = SubscribeOptions::new(PeerInfo::new("push", None, ""));
        o.wait = Some(WATCH_WAIT);
        match self.ctx.hub.subscribe(&self.stream, sink, o) {
            Err(e) => self.fail(epoch, &e.to_string()),
            Ok(sub) => {
                let mut st = self.lock();
                if epoch == st.epoch && st.running {
                    st.watch = Some(sub);
                } else {
                    drop(st);
                    sub.close();
                }
            }
        }
    }

    /// A publisher came to the stream: starts a client unless one runs. On akit-media's pool.
    fn ensure_rtmp(&self, epoch: u64) {
        let Some(engine) = self.dial.clone() else {
            return self.fail(epoch, NO_ENGINE);
        };
        let (url, t) = {
            let mut st = self.lock();
            if epoch != st.epoch || !st.running {
                return;
            }
            if let Some(Conn::Rtmp(c)) = &st.conn {
                if c.state() != ClientState::Closed {
                    return;
                }
                st.rtmp_connects = st.rtmp_connects.saturating_add(streamed(c));
            }
            st.conn = None;
            self.set(&mut st, PUSH_CONNECTING, String::new());
            (st.t.push.url.clone(), st.t.push.clone())
        };
        let mut cfg = RtmpClientConfig::default();
        cfg.connect_timeout = t.connect();
        cfg.handshake_timeout = t.connect();
        cfg.response_timeout = t.connect();
        cfg.source_wait = Some(PUSH_WAIT);
        cfg.reconnect = Some(
            Reconnect::new(&engine)
                .initial_delay(t.retry_min_or())
                .max_delay(t.retry_max_or()),
        );
        match rtmp::push(&engine, &url, &self.ctx.hub, &self.stream, cfg) {
            Err(e) => {
                let mut st = self.lock();
                if epoch == st.epoch && st.running {
                    self.failures.fetch_add(1, Relaxed);
                    let msg = self.redact(&e.to_string(), &st);
                    self.set(&mut st, PUSH_RETRYING, msg);
                }
            }
            Ok(c) => {
                let mut st = self.lock();
                if epoch == st.epoch && st.running {
                    st.conn = Some(Conn::Rtmp(c));
                } else {
                    drop(st);
                    c.close();
                }
            }
        }
    }

    /// What the target is doing.
    #[must_use]
    pub fn status(&self) -> PushStatus {
        let st = self.lock();
        let now = self.ctx.clock.now();
        let mut out = PushStatus {
            name: st.t.push.name.clone(),
            stream: st.t.push.stream.clone(),
            channel: st.t.channel.clone(),
            protocol: self.url.protocol.clone(),
            url: self.url.redacted(),
            state: st.state,
            error: st.err.clone(),
            since: st.since,
            retry_in: 0,
            connects: self.connects.load(Relaxed),
            failures: self.failures.load(Relaxed),
            bytes: bytes_of(&st),
            bitrate: 0,
        };
        if let Some(Conn::Rtmp(c)) = &st.conn {
            // Read from the client until it tells its owner (akio A298).
            let (state, err) = match c.state() {
                ClientState::Streaming => (PUSH_CONNECTED, String::new()),
                ClientState::Reconnecting => (
                    PUSH_RETRYING,
                    c.error().map(|e| e.to_string()).unwrap_or_default(),
                ),
                ClientState::Closed => match c.error() {
                    None | Some(rtmp::ClientError::Hub(_)) => {
                        (PUSH_WAITING, NOT_PUBLISHED.to_owned())
                    }
                    Some(e) => (PUSH_RETRYING, e.to_string()),
                },
                _ => (PUSH_CONNECTING, String::new()),
            };
            out.state = state;
            out.error = self.redact(&err, &st);
            out.connects = st.rtmp_connects.saturating_add(streamed(c));
        }
        if (out.state == PUSH_RETRYING || out.state == PUSH_WAITING) && st.timer.is_some() {
            out.retry_in = st.retry_at.saturating_sub(now).max(0);
        }
        if st.state == PUSH_CONNECTED {
            let d = now.saturating_sub(st.since);
            if d > 0 {
                let bits = u128::from(out.bytes.saturating_sub(st.bytes0)).saturating_mul(8);
                let per = bits
                    .saturating_mul(1_000_000_000)
                    .checked_div(u128::from(d.unsigned_abs()))
                    .unwrap_or(0);
                out.bitrate = u64::try_from(per).unwrap_or(u64::MAX);
            }
        }
        out
    }
}

/// The sessions of an RTMP client that streamed.
fn streamed(c: &RtmpClient) -> u64 {
    let r = c.reconnects();
    if r > 0 || c.state() == ClientState::Streaming {
        r.saturating_add(1)
    } else {
        0
    }
}

/// The bytes the target sent: the SRT payload; an RTMP push's are not counted until akio says
/// them (A298).
fn bytes_of(st: &St) -> u64 {
    let mut n = st.srt_sent;
    if let Some(Conn::Srt { session, .. }) = &st.conn {
        n = n.saturating_add(session.stats().bytes_sent);
    }
    n
}

/// Lets go of the target's connection, watch and timer, returning what closes them, to be
/// closed without the lock.
fn release(st: &mut St) -> (Option<Conn>, Option<Subscription>) {
    if let Some(t) = st.timer.take() {
        t.stop();
    }
    let conn = st.conn.take();
    if let Some(Conn::Srt { session, .. }) = &conn {
        st.srt_sent = st.srt_sent.saturating_add(session.stats().bytes_sent);
    }
    if let Some(Conn::Rtmp(c)) = &conn {
        st.rtmp_connects = st.rtmp_connects.saturating_add(streamed(c));
    }
    (conn, st.watch.take())
}

fn close(conn: Option<Conn>, watch: Option<Subscription>) {
    if let Some(w) = watch {
        w.close();
    }
    match conn {
        Some(Conn::Rtmp(c)) => c.close(),
        Some(Conn::Srt { session, sub }) => {
            if let Some(s) = sub {
                s.close();
            }
            session.close();
        }
        None => {}
    }
}

/// Runs `f` on akit-media's pool, for work that may block.
fn submit(f: impl FnOnce() + Send + 'static) {
    crate::pool::submit(Box::new(move |_late| f()));
}

/// The address of `host:port`, looked up with the system's resolver (it blocks).
fn resolve(host: &str) -> Result<SocketAddr, String> {
    host.to_socket_addrs()
        .map_err(|e| format!("cannot resolve the target: {e}"))?
        .next()
        .ok_or_else(|| "cannot resolve the target: no address".to_owned())
}

/// The handler of an SRT push's session.
struct SrtPush {
    push: Weak<Push>,
    epoch: u64,
}

impl SrtHandler for SrtPush {
    fn on_connect(&mut self, s: &SrtSession) {
        let (push, epoch, s) = (self.push.clone(), self.epoch, s.clone());
        // Off the session's I/O thread and its lock: the hub calls back at once.
        submit(move || {
            if let Some(p) = push.upgrade() {
                p.srt_connected(epoch, s);
            }
        });
    }

    fn on_data(&mut self, _data: Vec<u8>) {}

    fn on_close(&mut self, err: Option<SrtError>) {
        let (push, epoch) = (self.push.clone(), self.epoch);
        let msg = err.map_or_else(|| "the session ended".to_owned(), |e| e.to_string());
        // Off the session's lock, which closing the connection takes.
        submit(move || {
            if let Some(p) = push.upgrade() {
                p.fail(epoch, &msg);
            }
        });
    }
}

/// The hub sink of an SRT push: its stream muxed into MPEG-TS, 7 transport packets to an SRT
/// packet, as akio-media's SRT push sends it.
struct TsSink {
    session: SrtSession,
    muxer: Option<Muxer>,
    timeline: Timeline,
    video: Vec<bool>,
    buf: Vec<u8>,
}

impl TsSink {
    fn flush(&mut self) {
        let data = std::mem::take(&mut self.buf);
        for chunk in data.chunks(TS_PAYLOAD_SIZE) {
            if self.session.send(chunk).is_err() {
                break;
            }
        }
    }
}

impl Sink for TsSink {
    fn on_tracks(&mut self, tracks: &Tracks) {
        let mut m = Muxer::new(tracks);
        m.write_tables(&mut self.buf);
        self.muxer = Some(m);
        self.video = tracks.iter().map(|t| t.codec.is_video()).collect();
        self.timeline.restart();
        self.flush();
    }

    fn on_packet(&mut self, pkt: &Arc<Packet>) {
        let Some(m) = &mut self.muxer else { return };
        if !m.carries(pkt.track) {
            return;
        }
        let v = self.video.get(pkt.track).copied().unwrap_or(false);
        if v && pkt.keyframe {
            // So that a decoder joining at this keyframe finds the program.
            m.write_tables(&mut self.buf);
        }
        let dts = self.timeline.stamp(v, pkt.dts);
        let mut p = Packet::clone(pkt);
        p.pts = dts.saturating_add(pkt.cts());
        p.dts = dts;
        m.write_packet(&p, &mut self.buf);
        self.flush();
    }

    fn on_close(&mut self, _err: HubError) {
        self.session.close();
    }
}

/// The watch of an RTMP push's stream: a publisher that comes starts a client.
struct Watch {
    push: Weak<Push>,
    epoch: u64,
}

impl Sink for Watch {
    fn on_tracks(&mut self, _tracks: &Tracks) {
        let (push, epoch) = (self.push.clone(), self.epoch);
        // Off the publisher's thread and the stream's lock: the client subscribes to the hub.
        submit(move || {
            if let Some(p) = push.upgrade() {
                p.ensure_rtmp(epoch);
            }
        });
    }

    fn on_packet(&mut self, _pkt: &Arc<Packet>) {}
}

/// What a push target is doing, as the Admin API shows it (fit's `PushStatus`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PushStatus {
    /// Its name.
    pub name: String,
    /// The stream it relays.
    pub stream: String,
    /// The channel whose push it is; empty for `streaming.push`'s.
    pub channel: String,
    /// `rtmp` or `srt`.
    pub protocol: String,
    /// The target's scheme, host and app, never its stream key.
    pub url: String,
    /// `connected`, `connecting`, `retrying`, `waiting` or `stopped`.
    pub state: &'static str,
    /// Why it retries or waits.
    pub error: String,
    /// When its state began, in nanoseconds since 1970; 0 for never.
    pub since: i64,
    /// How long until the next attempt, while it retries or waits, in nanoseconds.
    pub retry_in: Time,
    /// Connections taken.
    pub connects: u64,
    /// Attempts that failed or dropped.
    pub failures: u64,
    /// What it sent.
    pub bytes: u64,
    /// The bits a second of its connection.
    pub bitrate: u64,
}

impl PushStatus {
    /// The status as the Admin API writes it.
    #[must_use]
    pub fn to_value(&self) -> Value {
        let mut m = vec![
            ("name".to_owned(), Value::String(self.name.clone())),
            ("stream".to_owned(), Value::String(self.stream.clone())),
        ];
        if !self.channel.is_empty() {
            m.push(("channel".to_owned(), Value::String(self.channel.clone())));
        }
        m.push(("protocol".to_owned(), Value::String(self.protocol.clone())));
        m.push(("url".to_owned(), Value::String(self.url.clone())));
        m.push(("state".to_owned(), Value::String(self.state.to_owned())));
        if !self.error.is_empty() {
            m.push(("error".to_owned(), Value::String(self.error.clone())));
        }
        if self.since != 0 {
            m.push((
                "since".to_owned(),
                Value::String(super::at::rfc3339(&akit_observe::Zone::utc(), self.since)),
            ));
        }
        if self.retry_in != 0 {
            m.push((
                "retry_in".to_owned(),
                Value::Number(super::channel::format_secs(self.retry_in)),
            ));
        }
        m.push((
            "connects".to_owned(),
            Value::Number(self.connects.to_string()),
        ));
        m.push((
            "failures".to_owned(),
            Value::Number(self.failures.to_string()),
        ));
        m.push(("bytes".to_owned(), Value::Number(self.bytes.to_string())));
        m.push((
            "bitrate".to_owned(),
            Value::Number(self.bitrate.to_string()),
        ));
        Value::Object(m)
    }
}
