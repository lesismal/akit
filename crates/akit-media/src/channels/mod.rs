//! The streaming role's channels (I145, fit's package `streaming/channels`, decision 043): a
//! channel plays items of a media library into the role's hub as a live stream, in order,
//! shuffled, looped or on a schedule in a time zone.
//!
//! A channel never blocks an I/O thread. Its items' movies are read through the library
//! ([`crate::library::Library::movie`]), and their samples on the library's file pool, up to
//! `read_ahead` and `max_buffer_bytes` ahead of what is sent. What was read is sent into the hub
//! from the channel's timer ([`clock::Clock`]), each packet when its decode time is due, so that
//! the stream runs at the pace of the samples' timestamps. The next item is loaded and read
//! while the one before it plays, and its timestamps carry on from where that one ends, so that
//! players see no gap and no jump; its tracks are set anew when they differ
//! (`Publisher::set_tracks`), which HLS marks with a discontinuity.
//!
//! A push target ([`Push`], I146) relays a stream of the hub, a channel or any other, to an
//! outside RTMP or SRT server, connected again with backoff.
//!
//! The role makes a [`Manager`] for its hub and moves it to every config applied: channels and
//! push targets are changed in place, a new list or schedule taking effect from the next item, a
//! target whose stream, url and timeout are the same kept connected. The Admin API finds it by
//! the engine ([`lookup`]): `/streaming/channels` and `/streaming/push`.

pub(crate) mod at;
mod channel;
pub mod clock;
mod list;
mod push;
mod source;

#[cfg(test)]
mod tests;

use std::collections::HashMap;
use std::sync::atomic::Ordering::Relaxed;
use std::sync::{Arc, Mutex, PoisonError};

use akio_http::{Context, Method, Response};
use akio_media::av::Time;
use akio_media::hub::Hub;
use akit_config::channels::{ChannelAt, StreamChannel};
use akit_config::value::Value;
use akit_observe::{MetricWriter, Zone};

use akit_config::channels::PushTarget;
pub use channel::{
    ActionError, Channel, ChannelStatus, ItemStatus, STATE_IDLE, STATE_LOADING, STATE_PLAYING,
    STATE_STOPPED, STATE_WAITING,
};
use clock::{Clock, WallClock};
use list::Rng;
pub use push::{
    PUSH_CONNECTED, PUSH_CONNECTING, PUSH_RETRYING, PUSH_STOPPED, PUSH_WAITING, Push, PushStatus,
};

/// A slot of a channel's schedule.
#[derive(Debug, Clone)]
pub(crate) struct Slot {
    pub(crate) at: ChannelAt,
    pub(crate) item: String,
}

/// A channel's config, compiled: immutable, and replaced whole by a reload.
#[derive(Debug)]
pub(crate) struct Settings {
    pub(crate) name: String,
    pub(crate) library: String,
    pub(crate) items: Vec<String>,
    pub(crate) shuffle: bool,
    pub(crate) looped: bool,
    pub(crate) slots: Vec<Slot>,
    pub(crate) zone: Zone,
    pub(crate) rendition: String,
    pub(crate) read_ahead: Time,
    pub(crate) max_bytes: u64,
}

fn compile(c: &StreamChannel) -> Result<Settings, String> {
    let zone = Zone::load(c.zone()).map_err(|e| {
        format!(
            "channel {}: {:?} is not a time zone: an IANA name, as Asia/Shanghai, or Local ({e})",
            c.name,
            c.zone()
        )
    })?;
    Ok(Settings {
        name: c.name.clone(),
        library: c.library.clone(),
        items: c.items.clone(),
        shuffle: c.shuffle,
        looped: c.looped,
        slots: c
            .schedule
            .iter()
            .map(|s| Slot {
                at: s.at,
                item: s.item.clone(),
            })
            .collect(),
        zone,
        rendition: c.rendition.clone(),
        read_ahead: i64::try_from(c.read_ahead_or().as_nanos()).unwrap_or(i64::MAX),
        max_bytes: c.buffer_bytes(),
    })
}

/// What a manager runs with (fit's `Options`).
#[allow(missing_debug_implementations)]
pub struct Options {
    /// The role's hub, which channels publish into.
    pub hub: Hub,
    /// The engine the media libraries are found by ([`crate::library::key`]).
    pub libraries: usize,
    /// The wall clock when `None`.
    pub clock: Option<Arc<dyn Clock>>,
    /// Seeds shuffle's order; a random seed when 0.
    pub seed: u64,
    /// The engine push targets dial from; none dial without it.
    pub dial: Option<Arc<akio::Engine>>,
}

/// What a manager's channels share.
pub(crate) struct Ctx {
    pub(crate) hub: Hub,
    pub(crate) libraries: usize,
    pub(crate) clock: Arc<dyn Clock>,
    rng: Mutex<Rng>,
}

impl Ctx {
    /// Puts `v` in a random order.
    pub(crate) fn shuffle(&self, v: &mut [usize]) {
        self.rng
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .shuffle(v);
    }
}

/// The errors of [`Manager::apply`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ApplyError {
    /// A channel that does not compile.
    Invalid(String),
    /// The manager has stopped.
    Closed,
}

impl std::fmt::Display for ApplyError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ApplyError::Invalid(e) => f.write_str(e),
            ApplyError::Closed => f.write_str("channels: the streaming role has stopped"),
        }
    }
}

impl std::error::Error for ApplyError {}

#[derive(Default)]
struct Inner {
    channels: HashMap<String, Arc<Channel>>,
    pushes: HashMap<String, Arc<Push>>,
    stopped: bool,
}

/// Runs the channels of a hub.
pub struct Manager {
    ctx: Arc<Ctx>,
    dial: Option<Arc<akio::Engine>>,
    inner: Mutex<Inner>,
}

impl std::fmt::Debug for Manager {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Manager").finish_non_exhaustive()
    }
}

impl Manager {
    /// A manager with nothing to run, which [`apply`](Self::apply) gives its channels.
    #[must_use]
    pub fn new(o: Options) -> Manager {
        let seed = if o.seed == 0 {
            let mut b = [0u8; 8];
            let _ = akio_crypto::rand::fill_random(&mut b);
            u64::from_le_bytes(b)
        } else {
            o.seed
        };
        Manager {
            ctx: Arc::new(Ctx {
                hub: o.hub,
                libraries: o.libraries,
                clock: o.clock.unwrap_or_else(|| Arc::new(WallClock)),
                rng: Mutex::new(Rng::new(seed)),
            }),
            dial: o.dial,
            inner: Mutex::new(Inner::default()),
        }
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, Inner> {
        self.inner.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Moves the manager to `chs` and `targets`: a channel or push target of the same name is
    /// changed in place, and those left out end. A channel's new list, schedule, library or
    /// rendition takes effect from its next item; a push target whose stream, url or timeout
    /// changes connects again.
    ///
    /// # Errors
    ///
    /// A channel or target that does not compile, which changes nothing; a manager stopped.
    pub fn apply(&self, chs: &[StreamChannel], targets: &[PushTarget]) -> Result<(), ApplyError> {
        let mut sets = Vec::with_capacity(chs.len());
        let mut errs = Vec::new();
        for c in chs {
            match compile(c) {
                Ok(s) => sets.push(Arc::new(s)),
                Err(e) => errs.push(e),
            }
        }
        let mut urls = Vec::with_capacity(targets.len());
        for t in targets {
            match akit_config::channels::parse_push_url(&t.push.url) {
                Ok(u) => urls.push(u),
                Err(e) => errs.push(format!("push {}: {e}", t.push.name)),
            }
        }
        if !errs.is_empty() {
            return Err(ApplyError::Invalid(errs.join("; ")));
        }
        type Start = (Arc<Channel>, Option<Arc<Settings>>);
        let (start, end, push_start, push_end) = {
            let mut g = self.lock();
            if g.stopped {
                return Err(ApplyError::Closed);
            }
            let mut start: Vec<Start> = Vec::new();
            let mut keep = Vec::new();
            for s in sets {
                keep.push(s.name.clone());
                if let Some(c) = g.channels.get(&s.name) {
                    start.push((Arc::clone(c), Some(s)));
                    continue;
                }
                let c = Channel::new(Arc::clone(&self.ctx), Arc::clone(&s));
                g.channels.insert(s.name.clone(), Arc::clone(&c));
                start.push((c, None));
            }
            let gone: Vec<String> = g
                .channels
                .keys()
                .filter(|k| !keep.contains(k))
                .cloned()
                .collect();
            let end: Vec<Arc<Channel>> = gone.iter().filter_map(|k| g.channels.remove(k)).collect();
            let mut push_start = Vec::new();
            let mut push_end = Vec::new();
            let mut keep = Vec::new();
            for (t, u) in targets.iter().zip(urls) {
                keep.push(t.push.name.clone());
                if let Some(p) = g.pushes.get(&t.push.name) {
                    if p.same(t) {
                        p.retarget(t.clone());
                        continue;
                    }
                    push_end.push(Arc::clone(p));
                }
                let p = Push::new(Arc::clone(&self.ctx), self.dial.clone(), t.clone(), u);
                g.pushes.insert(t.push.name.clone(), Arc::clone(&p));
                push_start.push(p);
            }
            let gone: Vec<String> = g
                .pushes
                .keys()
                .filter(|k| !keep.contains(k))
                .cloned()
                .collect();
            push_end.extend(gone.iter().filter_map(|k| g.pushes.remove(k)));
            (start, end, push_start, push_end)
        };
        // Ended first, so that a channel renamed frees its stream.
        for c in end {
            c.remove();
        }
        for p in push_end {
            p.remove();
        }
        for (c, s) in start {
            match s {
                Some(s) => c.apply(s),
                None => c.start(),
            }
        }
        for p in push_start {
            p.start();
        }
        Ok(())
    }

    /// Ends every channel; the manager runs nothing more.
    pub fn stop(&self) {
        let (chs, ps): (Vec<Arc<Channel>>, Vec<Arc<Push>>) = {
            let mut g = self.lock();
            g.stopped = true;
            (
                g.channels.drain().map(|(_, c)| c).collect(),
                g.pushes.drain().map(|(_, p)| p).collect(),
            )
        };
        for c in chs {
            c.remove();
        }
        for p in ps {
            p.remove();
        }
    }

    /// The push target named `name`.
    #[must_use]
    pub fn push(&self, name: &str) -> Option<Arc<Push>> {
        self.lock().pushes.get(name).cloned()
    }

    /// The push targets, sorted by name.
    #[must_use]
    pub fn push_list(&self) -> Vec<Arc<Push>> {
        let mut v: Vec<Arc<Push>> = self.lock().pushes.values().cloned().collect();
        v.sort_by(|a, b| a.name().cmp(b.name()));
        v
    }

    /// The status of every push target, sorted by name.
    #[must_use]
    pub fn pushes(&self) -> Vec<PushStatus> {
        self.push_list().iter().map(|p| p.status()).collect()
    }

    /// The status of the push targets of `stream`, as a channel's status shows them.
    fn pushes_of(&self, stream: &str) -> Vec<Value> {
        self.push_list()
            .iter()
            .filter(|p| p.stream() == stream)
            .map(|p| p.status().to_value())
            .collect()
    }

    /// The status of channel `c`, with its push targets, as the Admin API writes it.
    fn channel_value(&self, c: &Channel) -> Value {
        c.status().to_value(self.pushes_of(c.name()))
    }

    /// The channel named `name`.
    #[must_use]
    pub fn channel(&self, name: &str) -> Option<Arc<Channel>> {
        self.lock().channels.get(name).cloned()
    }

    /// The channels, sorted by name.
    #[must_use]
    pub fn list(&self) -> Vec<Arc<Channel>> {
        let mut v: Vec<Arc<Channel>> = self.lock().channels.values().cloned().collect();
        v.sort_by(|a, b| a.name().cmp(b.name()));
        v
    }

    /// The status of every channel, sorted by name.
    #[must_use]
    pub fn channels(&self) -> Vec<ChannelStatus> {
        self.list().iter().map(|c| c.status()).collect()
    }

    /// How many subscribers the hub has of `stream`: players over every protocol, and a
    /// segmenter for each HLS format played; 0 for a stream the hub does not have.
    #[must_use]
    pub fn subscribers(&self, stream: &str) -> usize {
        self.ctx
            .hub
            .stream_info(stream)
            .map_or(0, |i| i.subscribers)
    }

    /// Writes the metrics of the manager's channels (fit's, named `akit_streaming_*`).
    pub fn write_metrics(&self, w: &mut dyn MetricWriter) {
        let chs = self.list();
        let sts: Vec<ChannelStatus> = chs.iter().map(|c| c.status()).collect();
        w.family(
            "akit_streaming_channels",
            "gauge",
            "Channels, by state: playing, loading an item, idle with nothing to play, waiting for their stream's path, or stopped.",
        );
        for st in [
            STATE_PLAYING,
            STATE_LOADING,
            STATE_IDLE,
            STATE_WAITING,
            STATE_STOPPED,
        ] {
            let n = sts.iter().filter(|s| s.state == st).count();
            #[allow(clippy::cast_precision_loss)]
            w.sample("akit_streaming_channels", n as f64, &[("state", st)]);
        }
        w.family(
            "akit_streaming_channel_items_total",
            "counter",
            "Items a channel started (played), or skipped as its library could not serve them (failed).",
        );
        for c in &chs {
            #[allow(clippy::cast_precision_loss)]
            {
                w.sample(
                    "akit_streaming_channel_items_total",
                    c.played.load(Relaxed) as f64,
                    &[("channel", c.name()), ("result", "played")],
                );
                w.sample(
                    "akit_streaming_channel_items_total",
                    c.failed.load(Relaxed) as f64,
                    &[("channel", c.name()), ("result", "failed")],
                );
            }
        }
        w.family(
            "akit_streaming_channel_underruns_total",
            "counter",
            "Times a channel had nothing read to send when it was due, its file pool or disk too slow, and its stream paused.",
        );
        for c in &chs {
            #[allow(clippy::cast_precision_loss)]
            w.sample(
                "akit_streaming_channel_underruns_total",
                c.underruns.load(Relaxed) as f64,
                &[("channel", c.name())],
            );
        }
        let ps = self.push_list();
        let pst: Vec<PushStatus> = ps.iter().map(|p| p.status()).collect();
        w.family(
            "akit_streaming_push_targets",
            "gauge",
            "Push targets, by state: connected, connecting, retrying after a failure, waiting for their stream, or stopped.",
        );
        for st in [
            PUSH_CONNECTED,
            PUSH_CONNECTING,
            PUSH_RETRYING,
            PUSH_WAITING,
            PUSH_STOPPED,
        ] {
            let n = pst.iter().filter(|s| s.state == st).count();
            w.sample_u64("akit_streaming_push_targets", n as u64, &[("state", st)]);
        }
        w.family(
            "akit_streaming_push_connects_total",
            "counter",
            "Connections of a push target, by whether the target took the stream (ok) or not (failed).",
        );
        for s in &pst {
            w.sample_u64(
                "akit_streaming_push_connects_total",
                s.connects,
                &[("target", &s.name), ("result", "ok")],
            );
            w.sample_u64(
                "akit_streaming_push_connects_total",
                s.failures,
                &[("target", &s.name), ("result", "failed")],
            );
        }
        w.family(
            "akit_streaming_push_bytes_total",
            "counter",
            "Bytes a push target sent: the payload SRT sent (an RTMP push's wait for akio A298).",
        );
        for s in &pst {
            w.sample_u64(
                "akit_streaming_push_bytes_total",
                s.bytes,
                &[("target", &s.name)],
            );
        }
    }
}

type Registry = Mutex<HashMap<usize, Arc<Manager>>>;

fn registry() -> &'static Registry {
    static REG: std::sync::LazyLock<Registry> = std::sync::LazyLock::new(Registry::default);
    &REG
}

/// Makes `m` the manager [`lookup`] finds for the engine `key`.
pub fn register(key: usize, m: &Arc<Manager>) {
    registry()
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .insert(key, Arc::clone(m));
}

/// Forgets the engine `key`'s manager if it is `m`.
pub fn unregister(key: usize, m: &Arc<Manager>) {
    let mut g = registry().lock().unwrap_or_else(PoisonError::into_inner);
    if g.get(&key).is_some_and(|cur| Arc::ptr_eq(cur, m)) {
        g.remove(&key);
    }
}

/// The manager of the streaming role whose libraries are the engine `key`'s.
#[must_use]
pub fn lookup(key: usize) -> Option<Arc<Manager>> {
    registry()
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .get(&key)
        .cloned()
}

// ---- the Admin API ----

/// Where the Admin API serves the channels.
pub const PREFIX: &str = "/streaming/channels";

fn json(status: u16, v: &Value) -> Response {
    Response::with_body(
        status,
        "application/json",
        crate::library::catalog::compact(v).into_bytes(),
    )
}

fn error(status: u16, msg: &str) -> Response {
    json(
        status,
        &Value::Object(vec![("error".to_owned(), Value::String(msg.to_owned()))]),
    )
}

/// Answers a request of `/streaming/channels` (fit's `admin/channels.go`): the list, one
/// channel, and an action on one (`{"action": "start"}`, `"stop"`, `"skip"` with an `item` or
/// none); false for a path that is not theirs.
///
///   GET  /streaming/channels          every channel
///   GET  /streaming/channels/{name}   one channel, by its stream's path
///   POST /streaming/channels/{name}   an action
pub fn serve(key: usize, ctx: &mut Context) -> bool {
    let path = ctx.path().to_owned();
    let Some(rest) = path.strip_prefix(PREFIX) else {
        return false;
    };
    if !(rest.is_empty() || rest.starts_with('/')) {
        return false;
    }
    let method = ctx.method().clone();
    let body = ctx.body().to_vec();
    let resp = route(key, rest, &method, &body);
    ctx.respond(resp);
    true
}

fn route(key: usize, rest: &str, method: &Method, body: &[u8]) -> Response {
    let read = matches!(method, Method::Get | Method::Head);
    let name = rest.trim_start_matches('/');
    if name.is_empty() && !read {
        return error(405, "method not allowed").header("Allow", "GET, HEAD");
    }
    if !read && *method != Method::Post {
        return error(405, "method not allowed").header("Allow", "GET, HEAD, POST");
    }
    let Some(m) = lookup(key) else {
        return error(404, "the streaming role does not run here");
    };
    if name.is_empty() {
        let list = m.list().iter().map(|c| m.channel_value(c)).collect();
        return json(
            200,
            &Value::Object(vec![("channels".to_owned(), Value::Array(list))]),
        );
    }
    let Some(c) = m.channel(name) else {
        return error(404, &format!("no channel named {name:?}"));
    };
    if read {
        return json(200, &m.channel_value(&c));
    }
    let Some((action, item)) = action_of(body) else {
        return error(400, "the body must be JSON: {\"action\": \"start\"}");
    };
    let r = match action.as_str() {
        "start" => {
            c.start();
            Ok(())
        }
        "stop" => {
            c.stop();
            Ok(())
        }
        "skip" => c.skip(&item).map_err(Some),
        _ => Err(None),
    };
    match r {
        Ok(()) => json(200, &m.channel_value(&c)),
        Err(Some(e @ ActionError::Stopped)) => error(409, &e.to_string()),
        Err(Some(e @ ActionError::NoItem)) => error(404, &e.to_string()),
        Err(None) => error(
            400,
            "channels: the action is start, stop or skip; a push target's start, stop or restart",
        ),
    }
}

/// An action's body: its `action` and `item`.
fn action_of(body: &[u8]) -> Option<(String, String)> {
    let s = std::str::from_utf8(body).ok()?;
    let Value::Object(m) = akit_config::value::parse(s).ok()? else {
        return None;
    };
    let field = |k: &str| match m.iter().find(|(n, _)| n == k).map(|(_, v)| v) {
        Some(Value::String(s)) => Some(s.clone()),
        None | Some(Value::Null) => Some(String::new()),
        Some(_) => None,
    };
    Some((field("action")?, field("item")?))
}

/// Where the Admin API serves the push targets.
pub const PUSH_PREFIX: &str = "/streaming/push";

/// Answers a request of `/streaming/push` (fit's `admin/channels.go`): the list, and an action
/// on one (`{"action": "start"}`, `"stop"` or `"restart"`); false for a path that is not theirs.
///
///   GET  /streaming/push          every push target
///   POST /streaming/push/{name}   an action
pub fn serve_push(key: usize, ctx: &mut Context) -> bool {
    let path = ctx.path().to_owned();
    let Some(rest) = path.strip_prefix(PUSH_PREFIX) else {
        return false;
    };
    if !(rest.is_empty() || rest.starts_with('/')) {
        return false;
    }
    let method = ctx.method().clone();
    let body = ctx.body().to_vec();
    let resp = route_push(key, rest, &method, &body);
    ctx.respond(resp);
    true
}

fn route_push(key: usize, rest: &str, method: &Method, body: &[u8]) -> Response {
    let read = matches!(method, Method::Get | Method::Head);
    let name = rest.trim_start_matches('/');
    if name.is_empty() && !read {
        return error(405, "method not allowed").header("Allow", "GET, HEAD");
    }
    if !name.is_empty() && *method != Method::Post {
        return error(405, "method not allowed").header("Allow", "POST");
    }
    let Some(m) = lookup(key) else {
        return error(404, "the streaming role does not run here");
    };
    if name.is_empty() {
        let list = m.pushes().iter().map(PushStatus::to_value).collect();
        return json(
            200,
            &Value::Object(vec![("push".to_owned(), Value::Array(list))]),
        );
    }
    let Some(p) = m.push(name) else {
        return error(404, &format!("no push target named {name:?}"));
    };
    let Some((action, _)) = action_of(body) else {
        return error(400, "the body must be JSON: {\"action\": \"restart\"}");
    };
    match action.as_str() {
        "start" => p.start(),
        "stop" => p.stop(),
        "restart" => p.restart(),
        _ => {
            return error(
                400,
                "channels: the action is start, stop or skip; a push target's start, stop or restart",
            );
        }
    }
    json(200, &p.status().to_value())
}
