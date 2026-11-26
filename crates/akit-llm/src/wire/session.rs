//! fit's `Session` and `Stream` (`llm/internal/wire/session.go`, `stream.go`, `pump.go`), on
//! callbacks: calls, flow-controlled streams and pushes multiplexed over one transport.
//!
//! The dialer's streams have odd IDs, the listener's even ones. Each stream has a window of
//! [`Options::window`] bytes each way: a sender spends credit on every data frame (one byte for an
//! empty one) and the receiver gives it back once half the window has been consumed, so a slow
//! reader holds its own stream only. A link whose send queue passes its limit holds every stream's
//! data ([`Session::hold_streams`]) until it drains.
//!
//! Nothing here blocks or waits: a call's answer, a stream's data, its credit and its end come to
//! callbacks, run on whichever thread brought them (the link's I/O thread, a timer's, another
//! stream's), never under a lock of this module's.

use std::collections::{HashMap, VecDeque};
use std::fmt;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, OnceLock, PoisonError};
use std::time::{Duration, Instant};

use akio::timer::{SendTimer, after_send};

use super::frame::{self, FLAG_ERROR, Type};
use super::{Error, decode_error, encode_error, not_supported};
use crate::model::{CODE_RATE_LIMITED, Rtt};

/// A stream's window each way, by default.
pub const DEFAULT_WINDOW: usize = 256 << 10;
/// How long a stream may see no data either way, by default.
pub const DEFAULT_STREAM_IDLE: Duration = Duration::from_secs(120);
/// The calls and streams a peer may have running here at once, by default.
pub const DEFAULT_MAX_INBOUND: usize = 1024;

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(PoisonError::into_inner)
}

/// Nanoseconds since the first call, never 0.
pub(crate) fn now_nanos() -> u64 {
    static EPOCH: OnceLock<Instant> = OnceLock::new();
    let e = EPOCH.get_or_init(Instant::now);
    u64::try_from(e.elapsed().as_nanos())
        .unwrap_or(u64::MAX)
        .saturating_add(1)
}

fn since(nanos: u64) -> Duration {
    Duration::from_nanos(now_nanos().saturating_sub(nanos))
}

/// A session's options. Both ends must use the same window.
#[derive(Debug, Clone, Copy)]
pub struct Options {
    /// The dialing side: odd stream IDs.
    pub dialer: bool,
    /// A stream's receive window, in payload bytes.
    pub window: usize,
    /// A stream with no data either way for this long ends; zero: never.
    pub stream_idle: Duration,
    /// The largest frame, encoded or inflated.
    pub max_frame: usize,
    /// The smallest payload compressed; 0: never.
    pub compress_min: usize,
    /// The calls and streams the peer may have running here at once; one more is refused at once
    /// with a rate-limited error. 0: no bound.
    pub max_inbound: usize,
}

impl Default for Options {
    fn default() -> Options {
        Options {
            dialer: false,
            window: DEFAULT_WINDOW,
            stream_idle: DEFAULT_STREAM_IDLE,
            max_frame: frame::DEFAULT_MAX_FRAME,
            compress_min: frame::DEFAULT_COMPRESS,
            max_inbound: DEFAULT_MAX_INBOUND,
        }
    }
}

/// A call or push the peer sent.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Request {
    /// Its method.
    pub method: String,
    /// Its payload.
    pub payload: Vec<u8>,
}

/// What serves the calls, streams and pushes a peer starts. Every method runs on the thread that
/// received the frame, the link's I/O thread, in the order the frames came, and must not block:
/// a call is answered through its [`Reply`] whenever the answer is ready, a stream is ended with
/// [`Stream::end`].
pub trait Handler: Send + Sync + 'static {
    /// A call: answer it through `reply`, now or later.
    fn request(&self, _s: &Session, _req: Request, reply: Reply) {
        reply.answer(Err(not_supported("requests")));
    }
    /// A stream the peer opened: serve it, and end it with [`Stream::end`].
    fn stream(&self, _s: &Session, st: Stream) {
        st.end(Err(not_supported("streams")));
    }
    /// A push, in the order they came.
    fn push(&self, _s: &Session, _msg: Request) {}
    /// The peer is draining: start nothing new on this session.
    fn drain(&self, _s: &Session) {}
}

struct Refuse;
impl Handler for Refuse {}

type SendFn = Box<dyn Fn(Vec<u8>) -> Result<(), String> + Send + Sync>;
type CallDone = Box<dyn FnOnce(Result<Vec<u8>, Error>) + Send>;
type OnClose = Box<dyn FnOnce(&Error) + Send>;
type OnFinish = Box<dyn FnOnce(Option<&Error>) + Send>;
type PumpEnd = Box<dyn FnOnce(Result<(), Error>, usize) + Send>;

struct Call {
    done: CallDone,
    timer: Option<SendTimer>,
}

struct State {
    close_err: Option<Error>,
    next_id: u32,
    calls: HashMap<u32, Call>,
    /// The peer's calls being served: whether each was canceled.
    served: HashMap<u32, bool>,
    streams: HashMap<u32, Stream>,
    /// The calls served and the streams the peer opened.
    inbound: usize,
    on_close: Vec<OnClose>,
}

/// The round trips of a link's pings (fit's `rttMeter`): the last, a moving average weighing the
/// recent ones most, and the shortest, in milliseconds.
#[derive(Debug, Default)]
struct RttMeter {
    s: Rtt,
    at: u64,
}

const RTT_ALPHA: f64 = 0.3;

impl RttMeter {
    fn add(&mut self, d: Duration) {
        #[allow(clippy::cast_precision_loss)]
        let ms = d.as_micros() as f64 / 1000.0;
        if self.s.samples == 0 {
            self.s.avg = ms;
            self.s.min = ms;
        } else {
            self.s.avg += RTT_ALPHA * (ms - self.s.avg);
            self.s.min = self.s.min.min(ms);
        }
        self.s.last = ms;
        self.s.samples = self.s.samples.saturating_add(1);
        self.at = now_nanos();
    }
}

pub(crate) struct Inner {
    opts: Options,
    send: SendFn,
    handler: OnceLock<Arc<dyn Handler>>,
    st: Mutex<State>,
    last_recv: AtomicU64,
    last_send: AtomicU64,
    ping_seq: AtomicU64,
    ping_at: AtomicU64,
    drain_sent: AtomicBool,
    peer_drain: AtomicBool,
    held: AtomicBool,
    rx: AtomicU64,
    tx: AtomicU64,
    rtt: Mutex<RttMeter>,
    quiet: OnceLock<Arc<dyn Fn() + Send + Sync>>,
}

/// One side of a session. Cheap to clone; every clone is the same session.
#[derive(Clone)]
pub struct Session(pub(crate) Arc<Inner>);

impl fmt::Debug for Session {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Session")
            .field("dialer", &self.0.opts.dialer)
            .finish_non_exhaustive()
    }
}

impl Session {
    /// A session whose frames go out through `send`, which must not block. Set its handler with
    /// [`Session::set_handler`] before it receives a frame; until then the peer's calls and
    /// streams are refused.
    #[must_use]
    pub fn new(
        opts: Options,
        send: impl Fn(Vec<u8>) -> Result<(), String> + Send + Sync + 'static,
    ) -> Session {
        Session(Arc::new(Inner {
            opts,
            send: Box::new(send),
            handler: OnceLock::new(),
            st: Mutex::new(State {
                close_err: None,
                next_id: if opts.dialer { 1 } else { 2 },
                calls: HashMap::new(),
                served: HashMap::new(),
                streams: HashMap::new(),
                inbound: 0,
                on_close: Vec::new(),
            }),
            last_recv: AtomicU64::new(now_nanos()),
            last_send: AtomicU64::new(0),
            ping_seq: AtomicU64::new(0),
            ping_at: AtomicU64::new(0),
            drain_sent: AtomicBool::new(false),
            peer_drain: AtomicBool::new(false),
            held: AtomicBool::new(false),
            rx: AtomicU64::new(0),
            tx: AtomicU64::new(0),
            rtt: Mutex::new(RttMeter::default()),
            quiet: OnceLock::new(),
        }))
    }

    /// Sets what serves the peer's calls, streams and pushes; only the first call counts.
    pub fn set_handler(&self, h: Arc<dyn Handler>) {
        let _ = self.0.handler.set(h);
    }

    /// Sets `f` to run whenever a call or stream of this session ends, for one who waits for
    /// them all to end (a drain); only the first call counts.
    pub fn set_quiet(&self, f: Arc<dyn Fn() + Send + Sync>) {
        let _ = self.0.quiet.set(f);
    }

    fn handler(&self) -> Arc<dyn Handler> {
        self.0
            .handler
            .get()
            .cloned()
            .unwrap_or_else(|| Arc::new(Refuse))
    }

    fn quiet(&self) {
        if let Some(q) = self.0.quiet.get() {
            q();
        }
    }

    /// The options.
    #[must_use]
    pub fn options(&self) -> &Options {
        &self.0.opts
    }

    /// Whether `other` is this session.
    #[must_use]
    pub fn same(&self, other: &Session) -> bool {
        Arc::ptr_eq(&self.0, &other.0)
    }

    /// Why the session closed, `None` while it is open.
    #[must_use]
    pub fn err(&self) -> Option<Error> {
        lock(&self.0.st).close_err.clone()
    }

    /// Whether the session closed.
    #[must_use]
    pub fn is_closed(&self) -> bool {
        lock(&self.0.st).close_err.is_some()
    }

    /// Runs `f` once the session closes, with why; at once if it has.
    pub fn on_close(&self, f: impl FnOnce(&Error) + Send + 'static) {
        let mut g = lock(&self.0.st);
        if let Some(e) = g.close_err.clone() {
            drop(g);
            f(&e);
            return;
        }
        g.on_close.push(Box::new(f));
    }

    /// How long ago the last frame came.
    #[must_use]
    pub fn since_recv(&self) -> Duration {
        since(self.0.last_recv.load(Ordering::Relaxed))
    }

    /// When a frame other than a ping or pong was last sent (`now_nanos`), 0 if none.
    #[must_use]
    pub(crate) fn last_send(&self) -> u64 {
        self.0.last_send.load(Ordering::Relaxed)
    }

    /// How long the last ping has waited for its pong, zero when answered.
    #[must_use]
    pub fn unanswered(&self) -> Duration {
        match self.0.ping_at.load(Ordering::Relaxed) {
            0 => Duration::ZERO,
            t => since(t),
        }
    }

    /// The bytes of the frames received and sent, as they crossed.
    #[must_use]
    pub fn traffic(&self) -> (u64, u64) {
        (
            self.0.rx.load(Ordering::Relaxed),
            self.0.tx.load(Ordering::Relaxed),
        )
    }

    /// The round trips of the pings answered.
    #[must_use]
    pub fn rtt(&self) -> Rtt {
        let mut s = lock(&self.0.rtt).s;
        s.avg = (s.avg * 1000.0).round() / 1000.0;
        s
    }

    /// When the last round trip was measured (`now_nanos`), 0 before any.
    pub(crate) fn rtt_at(&self) -> u64 {
        lock(&self.0.rtt).at
    }

    /// The calls and streams not ended, both ways.
    #[must_use]
    pub fn inflight(&self) -> usize {
        let g = lock(&self.0.st);
        g.calls
            .len()
            .saturating_add(g.served.len())
            .saturating_add(g.streams.len())
    }

    /// Tells the peer this side is going away (a drain frame, once): it starts nothing new here,
    /// and what runs goes on.
    pub fn drain(&self) {
        if !self.0.drain_sent.swap(true, Ordering::AcqRel) {
            let _ = self.write(Type::Drain, 0, 0, "", &[]);
        }
    }

    /// Whether either side sent drain: new work goes elsewhere.
    #[must_use]
    pub fn draining(&self) -> bool {
        self.0.drain_sent.load(Ordering::Acquire) || self.0.peer_drain.load(Ordering::Acquire)
    }

    /// Whether the peer sent drain.
    #[must_use]
    pub fn peer_draining(&self) -> bool {
        self.0.peer_drain.load(Ordering::Acquire)
    }

    /// Closes the session: every call and stream fails with [`Error::Closed`], the calls being
    /// served are answered no more. Only the first cause counts.
    pub fn close(&self, cause: Error) {
        let (calls, streams, waiters) = {
            let mut g = lock(&self.0.st);
            if g.close_err.is_some() {
                return;
            }
            g.close_err = Some(cause.clone());
            (
                std::mem::take(&mut g.calls),
                std::mem::take(&mut g.streams),
                std::mem::take(&mut g.on_close),
            )
        };
        for (_, c) in calls {
            if let Some(t) = c.timer {
                t.cancel();
            }
            (c.done)(Err(Error::Closed));
        }
        for (_, st) in streams {
            st.finish(Some(Error::Closed), None);
        }
        for w in waiters {
            w(&cause);
        }
        self.quiet();
    }

    fn alloc(g: &mut State) -> u32 {
        loop {
            let id = g.next_id;
            g.next_id = g.next_id.wrapping_add(2);
            if g.next_id < id {
                g.next_id = 2u32.wrapping_sub(id % 2);
            }
            if id == 0 || g.calls.contains_key(&id) || g.streams.contains_key(&id) {
                continue;
            }
            return id;
        }
    }

    fn write(
        &self,
        kind: Type,
        flags: u8,
        id: u32,
        method: &str,
        payload: &[u8],
    ) -> Result<(), Error> {
        let b = frame::marshal(
            kind,
            flags,
            id,
            method,
            payload,
            self.0.opts.compress_min,
            self.0.opts.max_frame,
        )?;
        let n = u64::try_from(b.len()).unwrap_or(u64::MAX);
        if let Err(e) = (self.0.send)(b) {
            self.close(Error::Send(e));
            return Err(Error::Closed);
        }
        self.0.tx.fetch_add(n, Ordering::Relaxed);
        if kind != Type::Ping && kind != Type::Pong {
            self.0.last_send.store(now_nanos(), Ordering::Relaxed);
        }
        Ok(())
    }

    /// Calls `method` with `payload` and hands its answer to `done`, once: the peer's answer, its
    /// error, [`Error::Timeout`] after `timeout` (zero: none; the peer is told to cancel) or
    /// [`Error::Closed`]. `done` runs on whichever thread settles it, perhaps this one.
    pub fn call(
        &self,
        method: &str,
        payload: &[u8],
        timeout: Duration,
        done: impl FnOnce(Result<Vec<u8>, Error>) + Send + 'static,
    ) {
        let id = {
            let mut g = lock(&self.0.st);
            if g.close_err.is_some() {
                drop(g);
                return done(Err(Error::Closed));
            }
            let id = Self::alloc(&mut g);
            g.calls.insert(
                id,
                Call {
                    done: Box::new(done),
                    timer: None,
                },
            );
            id
        };
        if let Err(e) = self.write(Type::Request, 0, id, method, payload) {
            if let Some(c) = self.drop_call(id) {
                (c.done)(Err(e));
            }
            return;
        }
        if timeout.is_zero() {
            return;
        }
        let me = self.clone();
        let timer = after_send(timeout, move || {
            if let Some(c) = me.drop_call(id) {
                let _ = me.write(Type::StreamCancel, 0, id, "", &[]);
                (c.done)(Err(Error::Timeout));
            }
        });
        let Ok(timer) = timer else {
            // No timer: the call is bounded by the session's life.
            return;
        };
        let mut g = lock(&self.0.st);
        match g.calls.get_mut(&id) {
            Some(c) => c.timer = Some(timer),
            None => {
                drop(g);
                timer.cancel();
            }
        }
    }

    fn drop_call(&self, id: u32) -> Option<Call> {
        let c = lock(&self.0.st).calls.remove(&id);
        if c.is_some() {
            self.quiet();
        }
        c
    }

    /// Sends a one-way message.
    ///
    /// # Errors
    ///
    /// [`Error::Closed`]; a frame too large.
    pub fn push(&self, method: &str, payload: &[u8]) -> Result<(), Error> {
        if self.is_closed() {
            return Err(Error::Closed);
        }
        self.write(Type::Push, 0, 0, method, payload)
    }

    /// Sends a ping carrying its sequence; the pong refreshes the last frame's time and, for the
    /// latest ping, clears [`Session::unanswered`].
    ///
    /// # Errors
    ///
    /// [`Error::Closed`].
    pub fn ping(&self) -> Result<(), Error> {
        let seq = self
            .0
            .ping_seq
            .fetch_add(1, Ordering::AcqRel)
            .wrapping_add(1);
        let _ =
            self.0
                .ping_at
                .compare_exchange(0, now_nanos(), Ordering::AcqRel, Ordering::Acquire);
        self.write(Type::Ping, 0, 0, "", &seq.to_be_bytes())
    }

    /// Opens a stream of `method` with `payload`, at once: the peer's refusal comes as the
    /// stream's end.
    ///
    /// # Errors
    ///
    /// [`Error::Closed`]; a frame too large.
    pub fn open_stream(&self, method: &str, payload: &[u8]) -> Result<Stream, Error> {
        let st = {
            let mut g = lock(&self.0.st);
            if g.close_err.is_some() {
                return Err(Error::Closed);
            }
            let id = Self::alloc(&mut g);
            let st = Stream::new(self, id, method, payload.to_vec(), true);
            g.streams.insert(id, st.clone());
            st
        };
        if let Err(e) = self.write(Type::StreamOpen, 0, st.id(), method, payload) {
            st.finish(Some(e.clone()), None);
            return Err(e);
        }
        st.arm_idle();
        Ok(st)
    }

    /// Handles one message the transport received. An error means the peer broke the protocol:
    /// the session is closed, and the transport drops the link.
    ///
    /// # Errors
    ///
    /// Why the message broke the protocol.
    pub fn receive(&self, msg: &[u8]) -> Result<(), Error> {
        self.0.rx.fetch_add(
            u64::try_from(msg.len()).unwrap_or(u64::MAX),
            Ordering::Relaxed,
        );
        let r = frame::unmarshal(msg, self.0.opts.max_frame).and_then(|f| self.dispatch(f));
        if let Err(e) = &r {
            self.close(e.clone());
        }
        r
    }

    fn dispatch(&self, f: frame::Frame) -> Result<(), Error> {
        self.0.last_recv.store(now_nanos(), Ordering::Relaxed);
        match f.kind {
            Type::Ping => {
                let _ = self.write(Type::Pong, 0, 0, "", &f.payload);
            }
            Type::Pong => {
                if let Ok(b) = <[u8; 8]>::try_from(f.payload.as_slice()) {
                    if u64::from_be_bytes(b) == self.0.ping_seq.load(Ordering::Acquire) {
                        let at = self.0.ping_at.swap(0, Ordering::AcqRel);
                        if at != 0 {
                            lock(&self.0.rtt).add(since(at));
                        }
                    }
                }
            }
            Type::Drain => {
                if !self.0.peer_drain.swap(true, Ordering::AcqRel) {
                    self.handler().drain(self);
                }
            }
            Type::Push => {
                if !self.is_closed() {
                    self.handler().push(
                        self,
                        Request {
                            method: f.method,
                            payload: f.payload,
                        },
                    );
                }
            }
            Type::Request => return self.serve_request(f),
            Type::Response => {
                let c = lock(&self.0.st).calls.remove(&f.id);
                // Otherwise canceled here: a late answer is dropped.
                if let Some(c) = c {
                    if let Some(t) = c.timer {
                        t.cancel();
                    }
                    if f.flags & FLAG_ERROR != 0 {
                        (c.done)(Err(decode_error(&f.payload)));
                    } else {
                        (c.done)(Ok(f.payload));
                    }
                    self.quiet();
                }
            }
            Type::StreamOpen => return self.serve_stream(f),
            Type::StreamCancel => {
                let st = {
                    let mut g = lock(&self.0.st);
                    if let Some(c) = g.served.get_mut(&f.id) {
                        *c = true;
                    }
                    g.streams.get(&f.id).cloned()
                };
                if let Some(st) = st {
                    st.finish(Some(Error::Canceled), None);
                }
            }
            Type::StreamData | Type::StreamEnd | Type::Credit | Type::StreamKeepalive => {
                let Some(st) = lock(&self.0.st).streams.get(&f.id).cloned() else {
                    return Ok(()); // ended here already
                };
                match f.kind {
                    Type::StreamData => st.on_data_frame(f.payload),
                    Type::StreamEnd => st.on_end(f.flags, f.payload),
                    Type::StreamKeepalive => {
                        if !st.is_finished() {
                            st.touch();
                        }
                    }
                    _ => {
                        let Ok(b) = <[u8; 4]>::try_from(f.payload.as_slice()) else {
                            return Err(Error::Protocol("bad credit frame".to_owned()));
                        };
                        st.on_credit_frame(u32::from_be_bytes(b));
                    }
                }
            }
        }
        Ok(())
    }

    /// Checks an ID the peer chose: its parity, and that it is free.
    fn check_peer_id(&self, g: &State, id: u32) -> Result<(), Error> {
        let peer_odd = !self.0.opts.dialer;
        if id == 0 || (id % 2 == 1) != peer_odd {
            return Err(Error::Protocol(format!("stream id {id} has our parity")));
        }
        if g.served.contains_key(&id) || g.streams.contains_key(&id) {
            return Err(Error::Protocol(format!("stream id {id} in use")));
        }
        Ok(())
    }

    fn over_limit(&self, g: &State) -> bool {
        self.0.opts.max_inbound > 0 && g.inbound >= self.0.opts.max_inbound
    }

    fn serve_request(&self, f: frame::Frame) -> Result<(), Error> {
        {
            let mut g = lock(&self.0.st);
            if g.close_err.is_some() {
                return Ok(());
            }
            self.check_peer_id(&g, f.id)?;
            if self.over_limit(&g) {
                drop(g);
                let _ = self.write(
                    Type::Response,
                    FLAG_ERROR,
                    f.id,
                    "",
                    &encode_error(&too_busy()),
                );
                return Ok(());
            }
            g.served.insert(f.id, false);
            g.inbound = g.inbound.saturating_add(1);
        }
        let reply = Reply {
            s: Some(self.clone()),
            id: f.id,
        };
        self.handler().request(
            self,
            Request {
                method: f.method,
                payload: f.payload,
            },
            reply,
        );
        Ok(())
    }

    fn serve_stream(&self, f: frame::Frame) -> Result<(), Error> {
        let st = {
            let mut g = lock(&self.0.st);
            if g.close_err.is_some() {
                return Ok(());
            }
            self.check_peer_id(&g, f.id)?;
            if self.over_limit(&g) {
                drop(g);
                let _ = self.write(
                    Type::StreamEnd,
                    FLAG_ERROR,
                    f.id,
                    "",
                    &encode_error(&too_busy()),
                );
                return Ok(());
            }
            let st = Stream::new(self, f.id, &f.method, f.payload, false);
            g.streams.insert(f.id, st.clone());
            g.inbound = g.inbound.saturating_add(1);
            st
        };
        st.arm_idle();
        self.handler().stream(self, st);
        Ok(())
    }

    /// Holds (`true`) every stream's data frames, as if the peer granted no credit, or lets them
    /// go and wakes their senders: for a transport that paces the session by its send queue.
    pub fn hold_streams(&self, hold: bool) {
        self.0.held.store(hold, Ordering::Release);
        if !hold {
            self.wake_streams();
        }
    }

    /// Holds every stream's data frames, waking nobody: the link's pacing, from its send.
    pub(crate) fn hold_streams_quiet(&self) {
        self.0.held.store(true, Ordering::Release);
    }

    fn wake_streams(&self) {
        let streams: Vec<Stream> = lock(&self.0.st).streams.values().cloned().collect();
        for st in streams {
            st.resume();
        }
    }

    fn remove_stream(&self, st: &Stream) {
        {
            let mut g = lock(&self.0.st);
            if g.streams
                .get(&st.id())
                .is_some_and(|s| Arc::ptr_eq(&s.0, &st.0))
            {
                g.streams.remove(&st.id());
                if !st.0.opener {
                    g.inbound = g.inbound.saturating_sub(1);
                }
            }
        }
        self.quiet();
    }
}

fn too_busy() -> Error {
    Error::code(
        CODE_RATE_LIMITED,
        "too many concurrent requests and streams on this connection",
    )
}

/// How a call the peer made is answered: once, now or later, from any thread. Dropped without an
/// answer, it answers an internal error.
pub struct Reply {
    s: Option<Session>,
    id: u32,
}

impl fmt::Debug for Reply {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Reply").field("id", &self.id).finish()
    }
}

impl Reply {
    /// Whether the caller gave up (it canceled, or the session closed): an answer goes nowhere.
    #[must_use]
    pub fn canceled(&self) -> bool {
        let Some(s) = &self.s else { return true };
        let g = lock(&s.0.st);
        g.close_err.is_some() || g.served.get(&self.id).is_none_or(|c| *c)
    }

    /// Answers the call.
    pub fn answer(mut self, r: Result<Vec<u8>, Error>) {
        self.settle(r);
    }

    fn settle(&mut self, r: Result<Vec<u8>, Error>) {
        let Some(s) = self.s.take() else { return };
        let live = {
            let mut g = lock(&s.0.st);
            let c = g.served.remove(&self.id);
            if c.is_some() {
                g.inbound = g.inbound.saturating_sub(1);
            }
            g.close_err.is_none() && c == Some(false)
        };
        s.quiet();
        if !live {
            return; // the caller gave up, or the session closed
        }
        let w = match &r {
            Ok(data) => s.write(Type::Response, 0, self.id, "", data),
            Err(e) => s.write(Type::Response, FLAG_ERROR, self.id, "", &encode_error(e)),
        };
        if w == Err(Error::FrameTooLarge) {
            let _ = s.write(
                Type::Response,
                FLAG_ERROR,
                self.id,
                "",
                &encode_error(&Error::FrameTooLarge),
            );
        }
    }
}

impl Drop for Reply {
    fn drop(&mut self) {
        if self.s.is_some() {
            self.settle(Err(Error::Other("the call was not answered".to_owned())));
        }
    }
}

// ---- Streams ----

/// What a stream's reader is handed ([`Stream::on_data`]).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Event {
    /// A data frame.
    Data(Vec<u8>),
    /// The stream ended: `Ok` when the opener half-closed (on the responder's side) or the
    /// responder ended it without an error (on the opener's, see [`Stream::result`]); otherwise
    /// why.
    End(Result<(), Error>),
}

type Receiver = Box<dyn FnMut(Event) + Send>;

struct SState {
    queue: VecDeque<Vec<u8>>,
    buffered: usize,
    consumed: usize,
    credit: i64,
    peer_ended: bool,
    sent_end: bool,
    finished: bool,
    err: Option<Error>,
    result: Option<Vec<u8>>,
    on_finish: Vec<OnFinish>,
    receiver: Option<Receiver>,
    has_receiver: bool,
    delivered: bool,
    delivering: bool,
    again: bool,
    credit_fn: Option<Box<dyn FnOnce() + Send>>,
    timers: Vec<SendTimer>,
}

struct StreamInner {
    s: Session,
    id: u32,
    method: String,
    payload: Vec<u8>,
    opener: bool,
    st: Mutex<SState>,
    rx: AtomicU64,
    tx: AtomicU64,
    touched: AtomicU64,
}

/// One flow-controlled stream. The opener sends data and half-closes with
/// [`Stream::close_send`]; the responder sends data and ends the whole stream with
/// [`Stream::end`], whose result the opener gets as its end. Either side's failure (a cancel, the
/// idle timeout, the session's close) ends it for both. Cheap to clone.
#[derive(Clone)]
pub struct Stream(Arc<StreamInner>);

impl fmt::Debug for Stream {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Stream")
            .field("id", &self.0.id)
            .field("method", &self.0.method)
            .finish_non_exhaustive()
    }
}

fn cost(p: &[u8]) -> usize {
    p.len().max(1)
}

fn credit_frame(n: usize) -> [u8; 4] {
    u32::try_from(n).unwrap_or(u32::MAX).to_be_bytes()
}

impl Stream {
    fn new(s: &Session, id: u32, method: &str, payload: Vec<u8>, opener: bool) -> Stream {
        Stream(Arc::new(StreamInner {
            s: s.clone(),
            id,
            method: method.to_owned(),
            payload,
            opener,
            st: Mutex::new(SState {
                queue: VecDeque::new(),
                buffered: 0,
                consumed: 0,
                credit: i64::try_from(s.0.opts.window).unwrap_or(i64::MAX),
                peer_ended: false,
                sent_end: false,
                finished: false,
                err: None,
                result: None,
                on_finish: Vec::new(),
                receiver: None,
                has_receiver: false,
                delivered: false,
                delivering: false,
                again: false,
                credit_fn: None,
                timers: Vec::new(),
            }),
            rx: AtomicU64::new(0),
            tx: AtomicU64::new(0),
            touched: AtomicU64::new(now_nanos()),
        }))
    }

    /// The stream's ID.
    #[must_use]
    pub fn id(&self) -> u32 {
        self.0.id
    }

    /// The method it was opened with.
    #[must_use]
    pub fn method(&self) -> &str {
        &self.0.method
    }

    /// The payload it was opened with.
    #[must_use]
    pub fn payload(&self) -> &[u8] {
        &self.0.payload
    }

    /// The session it runs on.
    #[must_use]
    pub fn session(&self) -> &Session {
        &self.0.s
    }

    /// Whether this side opened it.
    #[must_use]
    pub fn opener(&self) -> bool {
        self.0.opener
    }

    /// Whether `other` is this stream.
    #[must_use]
    pub fn same(&self, other: &Stream) -> bool {
        Arc::ptr_eq(&self.0, &other.0)
    }

    /// The bytes of the data frames received and sent.
    #[must_use]
    pub fn bytes(&self) -> (u64, u64) {
        (
            self.0.rx.load(Ordering::Relaxed),
            self.0.tx.load(Ordering::Relaxed),
        )
    }

    /// The responder's result, once it ended the stream without an error (opener's side).
    #[must_use]
    pub fn result(&self) -> Option<Vec<u8>> {
        lock(&self.0.st).result.clone()
    }

    /// Why the stream ended, `None` while it runs or when the responder ended it without an
    /// error.
    #[must_use]
    pub fn err(&self) -> Option<Error> {
        lock(&self.0.st).err.clone()
    }

    /// Whether the stream ended.
    #[must_use]
    pub fn is_finished(&self) -> bool {
        lock(&self.0.st).finished
    }

    fn touch(&self) {
        self.0.touched.store(now_nanos(), Ordering::Relaxed);
    }

    /// Starts the idle timeout: a timer that ends the stream once it has seen no data for the
    /// session's `stream_idle`, set again for what is left whenever it finds data came since.
    fn arm_idle(&self) {
        let idle = self.0.s.0.opts.stream_idle;
        if !idle.is_zero() {
            self.idle_after(idle);
        }
    }

    fn idle_after(&self, d: Duration) {
        let me = self.clone();
        let Ok(t) = after_send(d, move || {
            let idle = me.0.s.0.opts.stream_idle;
            let quiet = since(me.0.touched.load(Ordering::Relaxed));
            if me.is_finished() {
                return;
            }
            if quiet >= idle {
                me.abort(Error::StreamIdle);
            } else {
                me.idle_after(idle.saturating_sub(quiet));
            }
        }) else {
            return;
        };
        self.keep_timer(t);
    }

    fn keep_timer(&self, t: SendTimer) {
        let mut g = lock(&self.0.st);
        if g.finished {
            drop(g);
            t.cancel();
            return;
        }
        // Spent timers are let go once a few gather.
        if g.timers.len() >= 8 {
            let _ = g.timers.remove(0);
        }
        g.timers.push(t);
    }

    /// Ends the stream with [`Error::Timeout`] after `d`, as the opener's total bound.
    pub fn deadline(&self, d: Duration) {
        let me = self.clone();
        if let Ok(t) = after_send(d, move || me.abort(Error::Timeout)) {
            self.keep_timer(t);
        }
    }

    /// Sends `data` as one frame if the peer has granted credit and the link lets the stream
    /// send, and reports whether it did; it never waits. With no credit, [`Stream::on_credit`]
    /// says when to try again.
    ///
    /// # Errors
    ///
    /// The stream ended, or this side half-closed it.
    pub fn try_send(&self, data: &[u8]) -> Result<bool, Error> {
        let n = cost(data);
        {
            let mut g = lock(&self.0.st);
            if g.finished {
                return Err(g.err.clone().unwrap_or(Error::StreamEnded));
            }
            if g.sent_end {
                return Err(Error::StreamEnded);
            }
            if !self.can_send(&g) {
                return Ok(false);
            }
            g.credit = g
                .credit
                .saturating_sub(i64::try_from(n).unwrap_or(i64::MAX));
        }
        if let Err(e) = self.0.s.write(Type::StreamData, 0, self.0.id, "", data) {
            let mut g = lock(&self.0.st);
            g.credit = g
                .credit
                .saturating_add(i64::try_from(n).unwrap_or(i64::MAX));
            return Err(e);
        }
        self.0.tx.fetch_add(
            u64::try_from(data.len()).unwrap_or(u64::MAX),
            Ordering::Relaxed,
        );
        self.touch();
        Ok(true)
    }

    fn can_send(&self, g: &SState) -> bool {
        g.credit > 0 && !self.0.s.0.held.load(Ordering::Acquire)
    }

    /// Whether another frame may go now.
    #[must_use]
    pub fn credit(&self) -> bool {
        let g = lock(&self.0.st);
        self.can_send(&g)
    }

    /// Arranges for `f` to run once, when the stream may send again or ends, and reports true;
    /// false, without running `f`, when it may send already or has ended (either can change at
    /// once on another thread, so a caller told false tries to send again). One at a time: a
    /// second replaces the first.
    pub fn on_credit(&self, f: impl FnOnce() + Send + 'static) -> bool {
        let mut g = lock(&self.0.st);
        if g.finished || self.can_send(&g) {
            return false;
        }
        g.credit_fn = Some(Box::new(f));
        true
    }

    fn on_credit_frame(&self, n: u32) {
        let f = {
            let mut g = lock(&self.0.st);
            g.credit = g.credit.saturating_add(i64::from(n));
            if self.can_send(&g) {
                g.credit_fn.take()
            } else {
                None
            }
        };
        if let Some(f) = f {
            f();
        }
    }

    fn resume(&self) {
        let f = {
            let mut g = lock(&self.0.st);
            if self.can_send(&g) {
                g.credit_fn.take()
            } else {
                None
            }
        };
        if let Some(f) = f {
            f();
        }
    }

    /// Half-closes the opener's side: the responder's reader gets its end.
    ///
    /// # Errors
    ///
    /// On the responder's side, which ends the stream with [`Stream::end`].
    pub fn close_send(&self) -> Result<(), Error> {
        if !self.0.opener {
            return Err(Error::Other(
                "wire: the responder ends a stream by returning from its handler".to_owned(),
            ));
        }
        {
            let mut g = lock(&self.0.st);
            if g.finished || g.sent_end {
                return Ok(());
            }
            g.sent_end = true;
        }
        self.0.s.write(Type::StreamEnd, 0, self.0.id, "", &[])
    }

    /// Sends a keep-alive frame every `interval` (zero: a quarter of the idle timeout) until the
    /// returned flag is set or the stream ends: it resets the idle timeout on both sides, for a
    /// side still working with nothing to send yet.
    #[must_use]
    pub fn keep_alive(&self, interval: Duration) -> Arc<AtomicBool> {
        let stop = Arc::new(AtomicBool::new(false));
        let interval = if interval.is_zero() {
            self.0
                .s
                .0
                .opts
                .stream_idle
                .checked_div(4)
                .unwrap_or_default()
        } else {
            interval
        };
        if !interval.is_zero() {
            self.keep_tick(interval, Arc::clone(&stop));
        }
        stop
    }

    fn keep_tick(&self, interval: Duration, stop: Arc<AtomicBool>) {
        let me = self.clone();
        let t = after_send(interval, move || {
            if stop.load(Ordering::Acquire) || me.is_finished() {
                return;
            }
            if me
                .0
                .s
                .write(Type::StreamKeepalive, 0, me.0.id, "", &[])
                .is_err()
            {
                return;
            }
            me.touch();
            me.keep_tick(interval, stop);
        });
        if let Ok(t) = t {
            self.keep_timer(t);
        }
    }

    /// Runs `f` once the stream ends, with why (`None` when the responder ended it without an
    /// error); at once if it has. On whichever thread ends it; it must not block.
    pub fn on_finish(&self, f: impl FnOnce(Option<&Error>) + Send + 'static) {
        let mut g = lock(&self.0.st);
        if g.finished {
            let e = g.err.clone();
            drop(g);
            f(e.as_ref());
            return;
        }
        g.on_finish.push(Box::new(f));
    }

    /// Ends the stream for both sides with [`Error::Canceled`].
    pub fn cancel(&self) {
        self.abort(Error::Canceled);
    }

    /// Ends the stream here; whether this call ended it.
    fn finish(&self, err: Option<Error>, result: Option<Vec<u8>>) -> bool {
        let (on_finish, credit_fn, timers) = {
            let mut g = lock(&self.0.st);
            if g.finished {
                return false;
            }
            g.finished = true;
            g.err = err.clone();
            g.result = result;
            (
                std::mem::take(&mut g.on_finish),
                g.credit_fn.take(),
                std::mem::take(&mut g.timers),
            )
        };
        for t in timers {
            t.cancel();
        }
        for f in on_finish {
            f(err.as_ref());
        }
        if let Some(f) = credit_fn {
            f();
        }
        self.deliver();
        self.0.s.remove_stream(self);
        true
    }

    /// Fails the stream here and tells the peer: the opener cancels, the responder ends it with
    /// the error.
    pub fn abort(&self, err: Error) {
        if !self.finish(Some(err.clone()), None) {
            return;
        }
        if self.0.opener {
            let _ = self.0.s.write(Type::StreamCancel, 0, self.0.id, "", &[]);
        } else {
            let _ = self.0.s.write(
                Type::StreamEnd,
                FLAG_ERROR,
                self.0.id,
                "",
                &encode_error(&err),
            );
        }
    }

    /// Ends the stream from the responder's side with its result: data, or an error. Nothing on
    /// the opener's side, or once it ended.
    pub fn end(&self, r: Result<Vec<u8>, Error>) {
        if self.0.opener || !self.finish(Some(Error::StreamEnded), None) {
            return;
        }
        let w = match &r {
            Ok(data) => self.0.s.write(Type::StreamEnd, 0, self.0.id, "", data),
            Err(e) => self
                .0
                .s
                .write(Type::StreamEnd, FLAG_ERROR, self.0.id, "", &encode_error(e)),
        };
        if w == Err(Error::FrameTooLarge) {
            let _ = self.0.s.write(
                Type::StreamEnd,
                FLAG_ERROR,
                self.0.id,
                "",
                &encode_error(&Error::FrameTooLarge),
            );
        }
    }

    fn on_data_frame(&self, p: Vec<u8>) {
        let over = {
            let mut g = lock(&self.0.st);
            if g.finished || g.peer_ended {
                return;
            }
            g.buffered = g.buffered.saturating_add(cost(&p));
            self.0.rx.fetch_add(
                u64::try_from(p.len()).unwrap_or(u64::MAX),
                Ordering::Relaxed,
            );
            // The sender may overshoot the window by one frame at most.
            let opts = &self.0.s.0.opts;
            let over = g.buffered > opts.window.saturating_add(opts.max_frame);
            if !over {
                g.queue.push_back(p);
            }
            over
        };
        if over {
            self.abort(Error::FlowControl);
            return;
        }
        self.touch();
        self.deliver();
    }

    fn on_end(&self, flags: u8, payload: Vec<u8>) {
        if !self.0.opener {
            lock(&self.0.st).peer_ended = true;
            self.deliver();
            return;
        }
        if flags & FLAG_ERROR != 0 {
            self.finish(Some(decode_error(&payload)), None);
        } else {
            self.finish(None, Some(payload));
        }
    }

    /// Hands the stream's data frames to `f` as they come, in order, and then its end once
    /// ([`Event::End`]). Frames that came before are handed over first, on this thread. `f` runs
    /// on whichever thread delivers a frame, the link's I/O thread among them, never twice at
    /// once, and must not block. A frame counts against the window until [`Stream::release`]
    /// gives its bytes back: how a reader slower than the sender holds it back.
    pub fn on_data(&self, f: impl FnMut(Event) + Send + 'static) {
        {
            let mut g = lock(&self.0.st);
            g.receiver = Some(Box::new(f));
            g.has_receiver = true;
        }
        self.deliver();
    }

    fn deliver(&self) {
        let mut g = lock(&self.0.st);
        if !g.has_receiver || g.delivered {
            return;
        }
        if g.delivering {
            g.again = true;
            return;
        }
        let Some(mut f) = g.receiver.take() else {
            return;
        };
        g.delivering = true;
        loop {
            if let Some(p) = g.queue.pop_front() {
                drop(g);
                if p.is_empty() {
                    self.release_n(1);
                }
                f(Event::Data(p));
                g = lock(&self.0.st);
                continue;
            }
            let end = if g.finished {
                Some(g.err.clone().map_or(Ok(()), Err))
            } else if g.peer_ended {
                Some(Ok(()))
            } else {
                None
            };
            if let Some(end) = end {
                g.delivered = true;
                g.delivering = false;
                drop(g);
                f(Event::End(end));
                return;
            }
            if !g.again {
                g.delivering = false;
                g.receiver = Some(f);
                return;
            }
            g.again = false;
        }
    }

    /// Gives back `n` bytes of the frames [`Stream::on_data`] handed over, consumed now, so that
    /// the sender may send as much again. A frame of no bytes is given back by `on_data` itself.
    pub fn release(&self, n: usize) {
        if n > 0 {
            self.release_n(n);
        }
    }

    fn release_n(&self, n: usize) {
        let grant = {
            let mut g = lock(&self.0.st);
            g.buffered = g.buffered.saturating_sub(n);
            g.consumed = g.consumed.saturating_add(n);
            if !g.finished && !g.peer_ended && g.consumed >= self.0.s.0.opts.window / 2 {
                std::mem::take(&mut g.consumed)
            } else {
                0
            }
        };
        if grant > 0 {
            let _ = self
                .0
                .s
                .write(Type::Credit, 0, self.0.id, "", &credit_frame(grant));
        }
    }
}

// ---- Pump ----

struct PumpState {
    queue: VecDeque<Vec<u8>>,
    frames: usize,
    src_end: Option<Result<(), Error>>,
    ended: bool,
    sending: bool,
    first: Option<Box<dyn FnOnce() + Send>>,
    end: Option<PumpEnd>,
}

struct Pump {
    src: Stream,
    dst: Stream,
    st: Mutex<PumpState>,
}

/// Forwards `src`'s data frames to `dst` as they come (fit's `wire.Pump`): a frame goes on at
/// once while `dst`'s peer has granted credit and waits otherwise, held against `src`'s window,
/// so a slow reader behind `dst` holds back the writer behind `src` and nothing piles up between
/// them. `end` runs once: `Ok` when `src` ended and every frame went on, or why `src` or `dst`
/// failed; with how many frames `src` delivered. Neither stream is ended here: `end` decides.
/// `first`, when given, runs before the first frame goes on.
pub fn pump(
    src: &Stream,
    dst: &Stream,
    first: Option<Box<dyn FnOnce() + Send>>,
    end: impl FnOnce(Result<(), Error>, usize) + Send + 'static,
) {
    let p = Arc::new(Pump {
        src: src.clone(),
        dst: dst.clone(),
        st: Mutex::new(PumpState {
            queue: VecDeque::new(),
            frames: 0,
            src_end: None,
            ended: false,
            sending: false,
            first,
            end: Some(Box::new(end)),
        }),
    });
    let q = Arc::clone(&p);
    src.on_data(move |ev| q.frame(ev));
}

impl Pump {
    fn frame(self: &Arc<Self>, ev: Event) {
        let mut g = lock(&self.st);
        if g.ended {
            drop(g);
            if let Event::Data(d) = ev {
                self.src.release(d.len());
            }
            return;
        }
        match ev {
            Event::End(r) => g.src_end = Some(r),
            Event::Data(d) => {
                g.frames = g.frames.saturating_add(1);
                if let Some(f) = g.first.take() {
                    drop(g);
                    f();
                    g = lock(&self.st);
                }
                g.queue.push_back(d);
            }
        }
        drop(g);
        self.drain();
    }

    fn drain(self: &Arc<Self>) {
        let mut g = lock(&self.st);
        if g.sending || g.ended {
            return;
        }
        g.sending = true;
        loop {
            let Some(data) = g.queue.pop_front() else {
                g.sending = false;
                if let Some(r) = g.src_end.clone() {
                    return self.finish(g, r);
                }
                return;
            };
            drop(g);
            let r = self.dst.try_send(&data);
            g = lock(&self.st);
            match r {
                Err(e) => {
                    g.sending = false;
                    g.queue.push_front(data);
                    return self.finish(g, Err(e));
                }
                Ok(true) => {
                    drop(g);
                    self.src.release(data.len());
                    g = lock(&self.st);
                }
                Ok(false) => {
                    g.queue.push_front(data);
                    g.sending = false;
                    drop(g);
                    let me = Arc::clone(self);
                    if !self.dst.on_credit(move || me.drain()) {
                        self.drain();
                    }
                    return;
                }
            }
        }
    }

    fn finish(&self, mut g: MutexGuard<'_, PumpState>, r: Result<(), Error>) {
        if g.ended {
            return;
        }
        g.ended = true;
        let n: usize = g.queue.drain(..).map(|q| q.len()).sum();
        let frames = g.frames;
        let end = g.end.take();
        drop(g);
        if n > 0 {
            self.src.release(n);
        }
        if let Some(end) = end {
            end(r, frames);
        }
    }
}

// ---- In memory ----

/// A dialer's and a listener's sessions joined in memory, with the semantics of a link (fit's
/// `wire.Pipe`): each side's frames are received by the other on a thread of the pipe's own, in
/// order. For tests, and a role's own pages. Closing either session ends the pipe.
#[must_use]
pub fn pipe(
    opts: Options,
    dialer: Arc<dyn Handler>,
    listener: Arc<dyn Handler>,
) -> (Session, Session) {
    use std::sync::mpsc;
    type Tx = mpsc::Sender<Option<Vec<u8>>>;
    let (to_l, from_d) = mpsc::channel::<Option<Vec<u8>>>();
    let (to_d, from_l) = mpsc::channel::<Option<Vec<u8>>>();
    let send = |tx: Tx| {
        let tx = Mutex::new(tx);
        move |b: Vec<u8>| lock(&tx).send(Some(b)).map_err(|e| e.to_string())
    };
    let d = Session::new(
        Options {
            dialer: true,
            ..opts
        },
        send(to_l.clone()),
    );
    let l = Session::new(
        Options {
            dialer: false,
            ..opts
        },
        send(to_d.clone()),
    );
    d.set_handler(dialer);
    l.set_handler(listener);
    // Either side's close ends both readers, and so the pipe.
    for s in [&d, &l] {
        let (a, b) = (Mutex::new(to_l.clone()), Mutex::new(to_d.clone()));
        s.on_close(move |_| {
            let _ = lock(&a).send(None);
            let _ = lock(&b).send(None);
        });
    }
    let run = |rx: mpsc::Receiver<Option<Vec<u8>>>, s: Session, other: Session| {
        let _ = std::thread::Builder::new()
            .name("akit-llm-pipe".to_owned())
            .spawn(move || {
                while let Ok(Some(m)) = rx.recv() {
                    if s.receive(&m).is_err() {
                        break;
                    }
                }
                s.close(Error::Closed);
                other.close(Error::Closed);
            });
    };
    run(from_d, l.clone(), d.clone());
    run(from_l, d.clone(), l.clone());
    (d, l)
}

#[cfg(test)]
pub(crate) mod tests;
