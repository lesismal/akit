//! akit's WebRTC media server, an SFU (I113, fit's `webrtc/media`): the role `webrtc.media`, one
//! UDP port of the listeners' engine that every PeerConnection shares (akio-webrtc's ICE lite
//! `RtcServer`), and the sessions the `whip` and `whep` handlers make on it, which publish a
//! stream by name (RFC 9725) and play it (draft-ietf-wish-whep).
//!
//! akit is an ICE lite agent: its only candidates are the port's addresses, known before any
//! offer, so an answer is complete the moment it is made, and the handler answers on the I/O
//! thread the offer arrived on. A publisher is answered with one codec of each kind, the first of
//! its offer's that `codecs` lists (akio's `accept_offer_with`), and its viewers with the same.
//! Its RTP packets are forwarded to each viewer as they arrive (`Peer::write_rtp`), on the thread
//! that received them; its viewers' key frame requests go back to it at most once a
//! `key_frame_interval`; it is told `max_bitrate` as REMB (`set_receive_bitrate_cap`), and what it
//! sends past two seconds' burst of it is dropped. Everything is bounded: PeerConnections in all,
//! streams, viewers a stream, a publisher's bitrate, the offer's size, and the time a session may
//! take to connect (`connect_timeout`, a timer of its own) and its peer may be silent
//! (`ice_timeout`, akio's consent timeouts).
//!
//! The member's access decides which peer addresses the port serves (the listener's accept
//! filter, set again by every apply). Sessions belong to the role: a reload that keeps `listen`
//! and `public_ips` keeps them and applies the rest to what comes next; one that changes those,
//! or leaves the role out, ends them.

use std::collections::HashMap;
use std::net::{IpAddr, SocketAddr, UdpSocket};
use std::sync::atomic::{AtomicBool, AtomicI64, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, OnceLock, PoisonError, RwLock, Weak};
use std::time::{Duration, Instant};

use akio::ListenerId;
use akio_webrtc::{
    Codec, Direction, IceConnectionState, KeyframeRequest, KeyframeRequestKind, MediaAdded,
    MediaKind, Mid, Peer, PeerHandler, RtcConfig, RtcServer, RtpPacket,
};
use akit_config::Config;
use akit_config::webrtc::{IceServer, WebRtc, WebRtcMedia};
use akit_observe::metrics::TextWriter;
use akit_roles::host::{Host, Info, MetricsFn, Role};

/// `webrtc.media`.
pub const MEDIA: Info = Info {
    id: "webrtc.media",
    set: |c| c.webrtc.as_ref().is_some_and(|w| w.media.is_some()),
    new,
};

/// The most media sections of a publisher forwarded: a WHIP client sends one audio and one video
/// track (fit's `maxTracks`).
const MAX_TRACKS: usize = 8;

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(PoisonError::into_inner)
}

fn read<T: Clone>(l: &RwLock<T>) -> T {
    l.read().unwrap_or_else(PoisonError::into_inner).clone()
}

fn write<T>(l: &RwLock<T>, v: T) {
    *l.write().unwrap_or_else(PoisonError::into_inner) = v;
}

// ---------------------------------------------------------------------------------------------
// The media servers of the process, by the engine the handlers are given.

type Servers = Mutex<Vec<(usize, Weak<Core>)>>;

fn servers() -> &'static Servers {
    static S: OnceLock<Servers> = OnceLock::new();
    S.get_or_init(|| Mutex::new(Vec::new()))
}

fn key_of(engine: &Arc<akio::Engine>) -> usize {
    Arc::as_ptr(engine) as usize
}

/// The media server the handlers of `engine` use, or `None` while none runs.
pub(crate) fn lookup(engine: &Arc<akio::Engine>) -> Option<Arc<Core>> {
    let k = key_of(engine);
    let mut s = lock(servers());
    s.retain(|(_, w)| w.strong_count() > 0);
    s.iter()
        .find(|(e, _)| *e == k)
        .and_then(|(_, w)| w.upgrade())
}

fn register(engine: &Arc<akio::Engine>, core: &Arc<Core>) {
    let k = key_of(engine);
    let mut s = lock(servers());
    s.retain(|(e, w)| *e != k && w.strong_count() > 0);
    s.push((k, Arc::downgrade(core)));
}

fn unregister(engine: &Arc<akio::Engine>, core: &Arc<Core>) {
    let k = key_of(engine);
    lock(servers()).retain(|(e, w)| !(*e == k && w.as_ptr() == Arc::as_ptr(core)));
}

// ---------------------------------------------------------------------------------------------
// Offers refused.

/// An offer refused, with the HTTP status that says why.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Refusal {
    pub(crate) status: u16,
    pub(crate) msg: String,
    /// When not zero, sent as `Retry-After`, in seconds.
    pub(crate) retry_after: u32,
}

fn refuse(status: u16, msg: impl Into<String>) -> Refusal {
    Refusal {
        status,
        msg: msg.into(),
        retry_after: 0,
    }
}

fn busy(msg: &str) -> Refusal {
    Refusal {
        status: 503,
        msg: msg.to_owned(),
        retry_after: 5,
    }
}

/// A session's kind: a publisher's (WHIP) or a viewer's (WHEP).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Kind {
    Publisher,
    Viewer,
}

/// An offer answered: the session's ID, and the answer's SDP.
pub(crate) struct Answer {
    pub(crate) id: String,
    pub(crate) sdp: String,
}

// ---------------------------------------------------------------------------------------------
// The SDP the server reads: the media sections' kinds, directions, codecs and ports.

struct Section {
    kind: Option<MediaKind>,
    port_zero: bool,
    direction: Direction,
    /// The `a=rtpmap` encoding names, in the `m=` line's order.
    codecs: Vec<String>,
}

fn sections(sdp: &str) -> Vec<Section> {
    let mut out: Vec<Section> = Vec::new();
    let mut order: Vec<String> = Vec::new();
    let mut names: HashMap<String, String> = HashMap::new();
    let finish =
        |out: &mut Vec<Section>, order: &mut Vec<String>, names: &mut HashMap<String, String>| {
            if let Some(s) = out.last_mut() {
                s.codecs = order
                    .iter()
                    .filter_map(|pt| names.get(pt).cloned())
                    .collect();
            }
            order.clear();
            names.clear();
        };
    let mut session_dir = Direction::SendRecv;
    for line in sdp.lines() {
        let line = line.trim_end();
        if let Some(m) = line.strip_prefix("m=") {
            finish(&mut out, &mut order, &mut names);
            let mut f = m.split(' ');
            let kind = match f.next() {
                Some("audio") => Some(MediaKind::Audio),
                Some("video") => Some(MediaKind::Video),
                _ => None,
            };
            let port_zero = f.next() == Some("0");
            let _proto = f.next();
            order = f.map(str::to_owned).collect();
            out.push(Section {
                kind,
                port_zero,
                direction: session_dir,
                codecs: Vec::new(),
            });
            continue;
        }
        let dir = match line {
            "a=sendonly" => Some(Direction::SendOnly),
            "a=recvonly" => Some(Direction::RecvOnly),
            "a=sendrecv" => Some(Direction::SendRecv),
            "a=inactive" => Some(Direction::Inactive),
            _ => None,
        };
        if let Some(d) = dir {
            match out.last_mut() {
                Some(s) => s.direction = d,
                None => session_dir = d,
            }
            continue;
        }
        if let Some(r) = line.strip_prefix("a=rtpmap:") {
            if let Some((pt, rest)) = r.split_once(' ') {
                let name = rest.split('/').next().unwrap_or("");
                names.insert(pt.to_owned(), name.to_owned());
            }
        }
    }
    finish(&mut out, &mut order, &mut names);
    out
}

/// The codec of an encoding name `codecs` may list, akio's.
fn codec_of(name: &str) -> Option<Codec> {
    match name.to_ascii_lowercase().as_str() {
        "opus" => Some(Codec::Opus),
        "vp8" => Some(Codec::Vp8),
        "vp9" => Some(Codec::Vp9),
        "h264" => Some(Codec::H264),
        "av1" => Some(Codec::Av1),
        _ => None,
    }
}

fn kind_of(c: Codec) -> MediaKind {
    if c == Codec::Opus {
        MediaKind::Audio
    } else {
        MediaKind::Video
    }
}

/// fit's `pickCodecs`: for each kind, the first codec of the offer's sending sections that is
/// allowed: the one codec of that kind the publisher is answered with.
fn pick_codecs(offer: &str, allowed: &[String]) -> Vec<Codec> {
    let mut picks: Vec<Codec> = Vec::new();
    for s in sections(offer) {
        let Some(kind) = s.kind else { continue };
        if s.port_zero || !matches!(s.direction, Direction::SendOnly | Direction::SendRecv) {
            continue;
        }
        if picks.iter().any(|c| kind_of(*c) == kind) {
            continue;
        }
        if let Some(c) = s
            .codecs
            .iter()
            .filter(|n| allowed.iter().any(|a| a.eq_ignore_ascii_case(n)))
            .find_map(|n| codec_of(n).filter(|c| kind_of(*c) == kind))
        {
            picks.push(c);
        }
    }
    picks
}

/// Whether an answer has a section (not refused) in `dirs`.
fn answers(sdp: &str, dirs: &[Direction]) -> bool {
    sections(sdp)
        .iter()
        .any(|s| s.kind.is_some() && !s.port_zero && dirs.contains(&s.direction))
}

// ---------------------------------------------------------------------------------------------
// The server.

/// What the server reads for each new session, swapped whole by an apply.
struct Settings {
    codecs: Vec<String>,
    ice: Vec<IceServer>,
    turn_urls: Vec<String>,
    turn_secret: Option<Vec<u8>>,
    ttl: Duration,
    max_pcs: usize,
    max_streams: usize,
    max_viewers: usize,
    bitrate: u64,
    sdp_bytes: usize,
    connect: Duration,
    ice_timeout: Duration,
    key_frames: Duration,
}

impl Settings {
    fn of(w: &WebRtc, m: &WebRtcMedia) -> Settings {
        Settings {
            codecs: if m.codecs.is_empty() {
                akit_config::webrtc::MEDIA_CODECS
                    .iter()
                    .map(|c| (*c).to_owned())
                    .collect()
            } else {
                m.codecs.clone()
            },
            ice: m.ice_servers.clone(),
            turn_urls: m.turn_urls.clone(),
            turn_secret: w
                .turn
                .as_ref()
                .filter(|t| !t.secret.is_empty())
                .map(|t| t.secret.as_bytes().to_vec()),
            ttl: m.credential_ttl(),
            max_pcs: m.max_pcs(),
            max_streams: m.max_stream_count(),
            max_viewers: m.max_viewer_count(),
            bitrate: m.bitrate(),
            sdp_bytes: m.sdp_bytes(),
            connect: m.connect(),
            ice_timeout: m.ice(),
            key_frames: m.key_frames(),
        }
    }

    /// A session's PeerConnection configuration, answering with `codecs` alone.
    fn peer_config(&self, codecs: &[Codec]) -> RtcConfig {
        let mut c = akio_webrtc::rtc_config().clear_codecs().set_rtp_mode(true);
        for codec in codecs {
            c = match codec {
                Codec::Opus => c.enable_opus(true),
                Codec::Vp8 => c.enable_vp8(true),
                Codec::Vp9 => c.enable_vp9(true),
                Codec::H264 => c.enable_h264(true),
                Codec::Av1 => c.enable_av1(true),
                _ => c,
            };
        }
        c.set_ice_timeouts(
            self.ice_timeout.min(Duration::from_secs(8)),
            self.ice_timeout,
        )
    }
}

/// A Mux of one member's `listen` and `public_ips`.
struct Running {
    key: String,
    member: WebRtcMedia,
    server: RtcServer,
    listener: ListenerId,
    addrs: Vec<SocketAddr>,
    /// Which start this is: sessions of an earlier one are no longer counted.
    epoch: u64,
}

/// A stream by name: its publisher, the codecs its viewers must accept, and its viewers.
pub(crate) struct Stream {
    name: String,
    codecs: Vec<Codec>,
    publisher: OnceLock<Peer>,
    /// The publisher's receiving sections, by kind.
    tracks: Mutex<Vec<(MediaKind, Mid)>>,
    /// The viewers, read for every packet forwarded, swapped whole when one joins or leaves.
    viewers: RwLock<Arc<Vec<Arc<Session>>>>,
    ended: AtomicBool,
    /// When a key frame was last asked of the publisher, in nanoseconds since `base`.
    last_kf: AtomicI64,
    /// The publisher's bitrate bucket: tokens (bytes) and when last filled.
    rate: Mutex<(f64, Option<Instant>)>,
}

/// A PeerConnection of a publisher or a viewer.
pub(crate) struct Session {
    id: String,
    kind: Kind,
    stream: Arc<Stream>,
    peer: OnceLock<Peer>,
    connected: AtomicBool,
    epoch: u64,
    /// A viewer's sending sections, by kind.
    mids: Mutex<Vec<(MediaKind, Mid)>>,
}

#[derive(Default)]
struct State {
    cur: Option<Running>,
    stopped: bool,
    sessions: HashMap<String, Arc<Session>>,
    streams: HashMap<String, Arc<Stream>>,
    /// PeerConnections in all, sessions and offers being answered.
    pcs: usize,
    /// Viewers being answered, by stream.
    pending: HashMap<String, usize>,
    epoch: u64,
}

/// Why an offer was refused, for `akit_webrtc_media_refused_total`.
const REFUSALS: [&str; 6] = [
    "peer_connections",
    "streams",
    "viewers",
    "conflict",
    "no_stream",
    "offer",
];
/// Why a session ended, for `akit_webrtc_media_sessions_ended_total`.
const ENDS: [&str; 5] = [
    "delete",
    "publisher_left",
    "failed",
    "connect_timeout",
    "reload",
];
const END_DELETE: usize = 0;
const END_PUBLISHER_LEFT: usize = 1;
const END_FAILED: usize = 2;
const END_TIMEOUT: usize = 3;
const END_RELOAD: usize = 4;

#[derive(Default)]
struct Meters {
    published: AtomicU64,
    viewed: AtomicU64,
    packets: AtomicU64,
    bytes: AtomicU64,
    dropped_bitrate: AtomicU64,
    kf_sent: AtomicU64,
    kf_held: AtomicU64,
    restarts: AtomicU64,
    refused: [AtomicU64; 6],
    ended: [AtomicU64; 5],
}

/// The role's state, shared with the handlers and the peers' callbacks.
pub(crate) struct Core {
    base: Instant,
    set: RwLock<Arc<Settings>>,
    st: Mutex<State>,
    access: Mutex<Option<Arc<akit_access::Policy>>>,
    m: Meters,
    me: Weak<Core>,
}

fn count(c: &AtomicU64) {
    c.fetch_add(1, Ordering::Relaxed);
}

pub(crate) fn hex_id() -> String {
    let mut b = [0u8; 16];
    if akio_crypto::rand::fill_random(&mut b).is_err() {
        // No OS randomness: the time's bits (never expected).
        let t = Instant::now().elapsed().as_nanos().to_le_bytes();
        b.copy_from_slice(t.get(..16).unwrap_or(&[0; 16]));
    }
    b.iter().map(|x| format!("{x:02x}")).collect()
}

impl Core {
    fn settings(&self) -> Arc<Settings> {
        read(&self.set)
    }

    fn refused(&self, i: usize) {
        if let Some(c) = self.m.refused.get(i) {
            count(c);
        }
    }

    /// Counts a PeerConnection about to be made, or says why it may not be; with the server and
    /// its start's epoch.
    fn reserve(&self, st: &mut State, s: &Settings) -> Result<(RtcServer, u64), Refusal> {
        let Some(cur) = st.cur.as_ref() else {
            return Err(busy("the media server is not running"));
        };
        if st.pcs >= s.max_pcs {
            self.refused(0);
            return Err(busy("too many PeerConnections"));
        }
        st.pcs = st.pcs.saturating_add(1);
        Ok((cur.server.clone(), cur.epoch))
    }

    /// Answers a publisher's offer for the stream `name`.
    pub(crate) fn publish(&self, name: &str, offer: &str) -> Result<Answer, Refusal> {
        let s = self.settings();
        let picks = pick_codecs(offer, &s.codecs);
        if picks.is_empty() {
            self.refused(5);
            return Err(refuse(
                422,
                "the offer sends no audio or video in a codec akit forwards",
            ));
        }
        let stream = Arc::new(Stream {
            name: name.to_owned(),
            codecs: picks.clone(),
            publisher: OnceLock::new(),
            tracks: Mutex::new(Vec::new()),
            viewers: RwLock::new(Arc::new(Vec::new())),
            ended: AtomicBool::new(false),
            last_kf: AtomicI64::new(i64::MIN / 2),
            rate: Mutex::new((0.0, None)),
        });
        let (server, epoch) = {
            let mut st = lock(&self.st);
            let r = self.reserve(&mut st, &s)?;
            let refusal = if st.streams.contains_key(name) {
                self.refused(3);
                Some(refuse(409, "the stream is published already"))
            } else if st.streams.len() >= s.max_streams {
                self.refused(1);
                Some(busy("too many streams"))
            } else {
                None
            };
            if let Some(e) = refusal {
                st.pcs = st.pcs.saturating_sub(1);
                return Err(e);
            }
            st.streams.insert(name.to_owned(), Arc::clone(&stream));
            r
        };
        let session = Arc::new(Session {
            id: hex_id(),
            kind: Kind::Publisher,
            stream: Arc::clone(&stream),
            peer: OnceLock::new(),
            connected: AtomicBool::new(false),
            epoch,
            mids: Mutex::new(Vec::new()),
        });
        let handler = Handler {
            core: self.me.clone(),
            session: Arc::downgrade(&session),
        };
        let answered = server
            .accept_offer_with(s.peer_config(&picks), offer, handler)
            .map_err(|e| refuse(400, format!("the offer cannot be answered: {e}")))
            .and_then(|(peer, sdp)| {
                if answers(&sdp, &[Direction::RecvOnly, Direction::SendRecv]) {
                    Ok((peer, sdp))
                } else {
                    peer.close();
                    Err(refuse(
                        422,
                        "the offer sends no audio or video in a codec akit forwards",
                    ))
                }
            });
        let (peer, sdp) = match answered {
            Ok(a) => a,
            Err(e) => {
                self.refused(5);
                let mut st = lock(&self.st);
                if st.epoch == epoch {
                    st.pcs = st.pcs.saturating_sub(1);
                    if st
                        .streams
                        .get(name)
                        .is_some_and(|x| Arc::ptr_eq(x, &stream))
                    {
                        st.streams.remove(name);
                    }
                }
                stream.ended.store(true, Ordering::Relaxed);
                return Err(e);
            }
        };
        let _ = session.peer.set(peer.clone());
        let _ = stream.publisher.set(peer.clone());
        {
            let mut st = lock(&self.st);
            if st.epoch != epoch || st.cur.is_none() {
                drop(st);
                peer.close();
                return Err(busy("the media server is restarting"));
            }
            st.sessions.insert(session.id.clone(), Arc::clone(&session));
        }
        count(&self.m.published);
        peer.set_receive_bitrate_cap(Some(s.bitrate));
        self.watch(&session, s.connect);
        Ok(Answer {
            id: session.id.clone(),
            sdp,
        })
    }

    /// Answers a viewer's offer for the stream `name`.
    pub(crate) fn view(&self, name: &str, offer: &str) -> Result<Answer, Refusal> {
        let s = self.settings();
        if sections(offer).iter().all(|x| x.kind.is_none()) {
            self.refused(5);
            return Err(refuse(400, "the offer is not SDP with media"));
        }
        let (server, epoch, stream) = {
            let mut st = lock(&self.st);
            let r = self.reserve(&mut st, &s)?;
            let stream = st
                .streams
                .get(name)
                .filter(|x| x.publisher.get().is_some())
                .cloned();
            let Some(stream) = stream else {
                st.pcs = st.pcs.saturating_sub(1);
                self.refused(4);
                return Err(refuse(404, "the stream is not published"));
            };
            let pending = st.pending.get(name).copied().unwrap_or(0);
            if read(&stream.viewers).len().saturating_add(pending) >= s.max_viewers {
                st.pcs = st.pcs.saturating_sub(1);
                self.refused(2);
                return Err(busy("too many viewers of the stream"));
            }
            let n = st.pending.entry(name.to_owned()).or_insert(0);
            *n = n.saturating_add(1);
            (r.0, r.1, stream)
        };
        let session = Arc::new(Session {
            id: hex_id(),
            kind: Kind::Viewer,
            stream: Arc::clone(&stream),
            peer: OnceLock::new(),
            connected: AtomicBool::new(false),
            epoch,
            mids: Mutex::new(Vec::new()),
        });
        let handler = Handler {
            core: self.me.clone(),
            session: Arc::downgrade(&session),
        };
        let answered = server
            .accept_offer_with(s.peer_config(&stream.codecs), offer, handler)
            .map_err(|e| refuse(400, format!("the offer cannot be answered: {e}")))
            .and_then(|(peer, sdp)| {
                if answers(&sdp, &[Direction::SendOnly, Direction::SendRecv]) {
                    Ok((peer, sdp))
                } else {
                    peer.close();
                    Err(refuse(
                        422,
                        "the offer receives none of the stream's tracks in its codecs",
                    ))
                }
            });
        let unpend = |st: &mut State| {
            if let Some(n) = st.pending.get_mut(name) {
                *n = n.saturating_sub(1);
                if *n == 0 {
                    st.pending.remove(name);
                }
            }
        };
        let (peer, sdp) = match answered {
            Ok(a) => a,
            Err(e) => {
                self.refused(5);
                let mut st = lock(&self.st);
                if st.epoch == epoch {
                    st.pcs = st.pcs.saturating_sub(1);
                    unpend(&mut st);
                }
                return Err(e);
            }
        };
        let _ = session.peer.set(peer.clone());
        {
            let mut st = lock(&self.st);
            if st.epoch != epoch || stream.ended.load(Ordering::Relaxed) {
                if st.epoch == epoch {
                    st.pcs = st.pcs.saturating_sub(1);
                    unpend(&mut st);
                }
                drop(st);
                peer.close();
                return Err(refuse(404, "the stream ended"));
            }
            unpend(&mut st);
            let mut v = (*read(&stream.viewers)).clone();
            v.push(Arc::clone(&session));
            write(&stream.viewers, Arc::new(v));
            st.sessions.insert(session.id.clone(), Arc::clone(&session));
        }
        count(&self.m.viewed);
        self.watch(&session, s.connect);
        Ok(Answer {
            id: session.id.clone(),
            sdp,
        })
    }

    /// Ends `session` when it has not connected within `connect`: a timer, about time.
    fn watch(&self, session: &Arc<Session>, connect: Duration) {
        let core = self.me.clone();
        let sess = Arc::downgrade(session);
        let _ = akio::timer::after_send(connect, move || {
            if let (Some(c), Some(s)) = (core.upgrade(), sess.upgrade()) {
                if !s.connected.load(Ordering::Relaxed) {
                    c.end(&s, END_TIMEOUT);
                }
            }
        });
    }

    /// Whether the session `id` of `kind` runs.
    pub(crate) fn has(&self, kind: Kind, id: &str) -> bool {
        lock(&self.st)
            .sessions
            .get(id)
            .is_some_and(|s| s.kind == kind)
    }

    /// Ends the session `id` of `kind`, as a DELETE does; whether there was one.
    pub(crate) fn end_id(&self, kind: Kind, id: &str) -> bool {
        let s = lock(&self.st).sessions.get(id).cloned();
        match s {
            Some(s) if s.kind == kind => self.end(&s, END_DELETE),
            _ => false,
        }
    }

    /// Ends `session`, and the viewers of a publisher's; whether it still ran.
    fn end(&self, session: &Arc<Session>, why: usize) -> bool {
        let ended = {
            let mut st = lock(&self.st);
            if !st
                .sessions
                .get(&session.id)
                .is_some_and(|s| Arc::ptr_eq(s, session))
            {
                return false;
            }
            st.sessions.remove(&session.id);
            if st.epoch == session.epoch {
                st.pcs = st.pcs.saturating_sub(1);
            }
            let mut ended = vec![Arc::clone(session)];
            ended.extend(self.forget(&mut st, session, why));
            ended
        };
        close(&ended);
        true
    }

    /// Forgets `session` in its stream; a publisher's stream ends, and its viewers are forgotten
    /// too and returned for the caller to close once the lock is released.
    fn forget(&self, st: &mut State, session: &Arc<Session>, why: usize) -> Vec<Arc<Session>> {
        if let Some(c) = self.m.ended.get(why) {
            count(c);
        }
        let stream = &session.stream;
        if session.kind == Kind::Viewer {
            let v: Vec<Arc<Session>> = read(&stream.viewers)
                .iter()
                .filter(|x| !Arc::ptr_eq(x, session))
                .cloned()
                .collect();
            write(&stream.viewers, Arc::new(v));
            return Vec::new();
        }
        stream.ended.store(true, Ordering::Relaxed);
        if st
            .streams
            .get(&stream.name)
            .is_some_and(|x| Arc::ptr_eq(x, stream))
        {
            st.streams.remove(&stream.name);
        }
        let viewer_why = if why == END_RELOAD {
            END_RELOAD
        } else {
            END_PUBLISHER_LEFT
        };
        let viewers = read(&stream.viewers);
        write(&stream.viewers, Arc::new(Vec::new()));
        let mut ended = Vec::new();
        for v in viewers.iter() {
            if st.sessions.get(&v.id).is_some_and(|x| Arc::ptr_eq(x, v)) {
                st.sessions.remove(&v.id);
                if st.epoch == v.epoch {
                    st.pcs = st.pcs.saturating_sub(1);
                }
                if let Some(c) = self.m.ended.get(viewer_why) {
                    count(c);
                }
                ended.push(Arc::clone(v));
            }
        }
        ended
    }

    /// Forgets every session and stream (a restart or a stop), returning the sessions to close.
    fn forget_all(&self, st: &mut State) -> Vec<Arc<Session>> {
        let all: Vec<Arc<Session>> = st.sessions.values().cloned().collect();
        let mut ended = Vec::new();
        for s in all {
            if st.sessions.get(&s.id).is_some_and(|x| Arc::ptr_eq(x, &s)) {
                st.sessions.remove(&s.id);
                ended.extend(self.forget(st, &s, END_RELOAD));
                ended.push(s);
            }
        }
        st.streams.clear();
        st.pending.clear();
        st.pcs = 0;
        ended
    }

    /// Asks the publisher of `stream` for a key frame, unless one was asked less than
    /// `key_frame_interval` ago.
    fn key_frame(&self, stream: &Stream) {
        let Some(publisher) = stream.publisher.get() else {
            return;
        };
        let Some(mid) = lock(&stream.tracks)
            .iter()
            .find(|(k, _)| *k == MediaKind::Video)
            .map(|(_, m)| *m)
        else {
            return;
        };
        let now = i64::try_from(self.base.elapsed().as_nanos()).unwrap_or(i64::MAX);
        let every = i64::try_from(self.settings().key_frames.as_nanos()).unwrap_or(i64::MAX);
        let last = stream.last_kf.load(Ordering::Relaxed);
        if now.saturating_sub(last) < every
            || stream
                .last_kf
                .compare_exchange(last, now, Ordering::Relaxed, Ordering::Relaxed)
                .is_err()
        {
            count(&self.m.kf_held);
            return;
        }
        if publisher
            .request_keyframe(mid, KeyframeRequestKind::Pli)
            .is_ok()
        {
            count(&self.m.kf_sent);
        }
    }

    /// Forwards a publisher's packet to its viewers, within `max_bitrate`.
    fn forward(&self, stream: &Stream, p: &RtpPacket) {
        let s = self.settings();
        let n = p.payload.len().saturating_add(12);
        if !allow(&stream.rate, n, s.bitrate, Instant::now()) {
            count(&self.m.dropped_bitrate);
            return;
        }
        count(&self.m.packets);
        self.m
            .bytes
            .fetch_add(u64::try_from(n).unwrap_or(0), Ordering::Relaxed);
        let Some(kind) = p.codec.map(kind_of) else {
            return;
        };
        for v in read(&stream.viewers).iter() {
            let mid = lock(&v.mids)
                .iter()
                .find(|(k, _)| *k == kind)
                .map(|(_, m)| *m);
            if let (Some(mid), Some(peer)) = (mid, v.peer.get()) {
                // A viewer not connected yet, or gone, drops it.
                let _ = peer.write_rtp(mid, p);
            }
        }
    }

    /// The `Link` header values that tell a peer of the ICE servers (RFC 9725 section 4.6):
    /// `ice_servers`, and `turn_urls` with credentials made from `webrtc.turn`'s secret for the
    /// session `id`.
    pub(crate) fn links(&self, id: &str) -> Vec<String> {
        let s = self.settings();
        let mut links = Vec::new();
        let mut add = |url: &str, user: &str, cred: &str| {
            let mut l = format!("<{url}>; rel=\"ice-server\"");
            if !user.is_empty() || !cred.is_empty() {
                l.push_str(&format!(
                    "; username=\"{user}\"; credential=\"{cred}\"; credential-type=\"password\""
                ));
            }
            links.push(l);
        };
        for is in &s.ice {
            for u in &is.urls {
                add(u, &is.username, &is.credential);
            }
        }
        if let Some(secret) = &s.turn_secret {
            if !s.turn_urls.is_empty() {
                let tag = format!("akit-{}", id.get(..16).unwrap_or(id));
                let (user, pass) = akio_turn::ephemeral_credentials(secret, &tag, s.ttl);
                for u in &s.turn_urls {
                    add(u, &user, &pass);
                }
            }
        }
        links
    }

    /// The largest offer taken.
    pub(crate) fn sdp_bytes(&self) -> usize {
        self.settings().sdp_bytes
    }

    /// The addresses the port advertises.
    #[cfg(test)]
    pub(crate) fn addrs(&self) -> Vec<SocketAddr> {
        lock(&self.st)
            .cur
            .as_ref()
            .map(|r| r.addrs.clone())
            .unwrap_or_default()
    }
}

/// fit's `rateState.allow`: takes `n` bytes from the bucket of `rate` bits a second, which holds
/// two seconds' worth.
#[allow(clippy::cast_precision_loss)]
fn allow(b: &Mutex<(f64, Option<Instant>)>, n: usize, rate: u64, now: Instant) -> bool {
    let mut g = lock(b);
    let bytes = rate as f64 / 8.0;
    let cap = bytes * 2.0;
    g.0 = match g.1 {
        None => cap,
        Some(at) => (g.0 + now.saturating_duration_since(at).as_secs_f64() * bytes).min(cap),
    };
    g.1 = Some(now);
    let n = n as f64;
    if g.0 < n {
        return false;
    }
    g.0 -= n;
    true
}

/// Closes the PeerConnections of sessions already forgotten. Never under the state's lock:
/// closing runs their callbacks.
fn close(sessions: &[Arc<Session>]) {
    for s in sessions {
        if let Some(p) = s.peer.get() {
            p.close();
        }
    }
}

/// A session's PeerConnection's callbacks.
struct Handler {
    core: Weak<Core>,
    session: Weak<Session>,
}

impl PeerHandler for Handler {
    fn on_media_added(&mut self, _p: &Peer, m: &MediaAdded) {
        let Some(s) = self.session.upgrade() else {
            return;
        };
        match s.kind {
            Kind::Publisher if matches!(m.direction, Direction::RecvOnly | Direction::SendRecv) => {
                let mut t = lock(&s.stream.tracks);
                if t.len() < MAX_TRACKS && !t.iter().any(|(k, _)| *k == m.kind) {
                    t.push((m.kind, m.mid));
                }
            }
            Kind::Viewer if matches!(m.direction, Direction::SendOnly | Direction::SendRecv) => {
                let mut t = lock(&s.mids);
                if t.len() < MAX_TRACKS && !t.iter().any(|(k, _)| *k == m.kind) {
                    t.push((m.kind, m.mid));
                }
            }
            _ => {}
        }
    }

    fn on_connected(&mut self, _p: &Peer) {
        let (Some(c), Some(s)) = (self.core.upgrade(), self.session.upgrade()) else {
            return;
        };
        if !s.connected.swap(true, Ordering::Relaxed) {
            // A viewer needs a key frame to start from; a publisher is asked for one at once.
            c.key_frame(&s.stream);
        }
    }

    fn on_rtp(&mut self, _p: &Peer, packet: &RtpPacket) {
        let (Some(c), Some(s)) = (self.core.upgrade(), self.session.upgrade()) else {
            return;
        };
        if s.kind == Kind::Publisher {
            c.forward(&s.stream, packet);
        }
    }

    fn on_keyframe_request(&mut self, _p: &Peer, _req: &KeyframeRequest) {
        if let (Some(c), Some(s)) = (self.core.upgrade(), self.session.upgrade()) {
            if s.kind == Kind::Viewer {
                c.key_frame(&s.stream);
            }
        }
    }

    fn on_ice_state(&mut self, _p: &Peer, state: IceConnectionState) {
        if state == IceConnectionState::Failed {
            if let (Some(c), Some(s)) = (self.core.upgrade(), self.session.upgrade()) {
                c.end(&s, END_FAILED);
            }
        }
    }

    fn on_close(&mut self, _p: &Peer) {
        if let (Some(c), Some(s)) = (self.core.upgrade(), self.session.upgrade()) {
            c.end(&s, END_FAILED);
        }
    }
}

// ---------------------------------------------------------------------------------------------
// The role.

struct Media {
    host: Host,
    core: Arc<Core>,
    /// The config's `engine.event_loops`, for the port's I/O threads.
    event_loops: usize,
}

fn new(host: Host) -> Result<Box<dyn Role>, String> {
    let core = Arc::new_cyclic(|me| Core {
        base: Instant::now(),
        set: RwLock::new(Arc::new(Settings::of(
            &WebRtc::default(),
            &WebRtcMedia::default(),
        ))),
        st: Mutex::new(State::default()),
        access: Mutex::new(None),
        m: Meters::default(),
        me: me.clone(),
    });
    Ok(Box::new(Media {
        host,
        core,
        event_loops: 0,
    }))
}

fn static_key(m: &WebRtcMedia) -> String {
    format!("{:?} {:?}", m.listen, m.public_ips)
}

/// The addresses the port advertises: `public_ips` with its port, the listen address, or for a
/// wildcard every interface's address that is up.
pub(crate) fn advertised(listen: SocketAddr, public: &[IpAddr]) -> Vec<SocketAddr> {
    if !public.is_empty() {
        return public
            .iter()
            .map(|ip| SocketAddr::new(*ip, listen.port()))
            .collect();
    }
    if !listen.ip().is_unspecified() {
        return vec![listen];
    }
    let v6 = listen.is_ipv6();
    let mut out: Vec<SocketAddr> = akio_sys::interfaces()
        .unwrap_or_default()
        .iter()
        .filter(|i| i.flags.is_up())
        .flat_map(|i| i.addrs.iter().map(|a| a.ip))
        .filter(|ip| ip.is_ipv4() || v6)
        .filter(|ip| match ip {
            IpAddr::V4(v4) => !v4.is_link_local(),
            IpAddr::V6(v6) => (v6.segments()[0] & 0xffc0) != 0xfe80,
        })
        .map(|ip| SocketAddr::new(ip, listen.port()))
        .collect();
    out.sort();
    out.dedup();
    out
}

impl Media {
    fn start(&self, m: &WebRtcMedia, key: String, epoch: u64) -> Result<Running, String> {
        let mut addr = crate::turn::listen_addr(&m.listen)
            .ok_or_else(|| format!("listen: {:?} is not an address to listen on", m.listen))?;
        if addr.port() == 0 {
            // The answers name the port: a free one is found first, and bound after, which
            // another process may take meanwhile; akio's A268 would bind first and name the
            // candidates after.
            let probe = UdpSocket::bind(SocketAddr::new(addr.ip(), 0))
                .map_err(|e| format!("listen on {}: {e}", m.listen))?;
            let port = probe
                .local_addr()
                .map_err(|e| format!("listen on {}: {e}", m.listen))?
                .port();
            drop(probe);
            addr.set_port(port);
        }
        let addrs = advertised(addr, &m.public_ips);
        let server = RtcServer::new(addrs.clone())
            .connect_timeout(None)
            .max_peers(0);
        let mut l = crate::udp_listener(addr, self.event_loops).serve(server.clone());
        if let Some(p) = lock(&self.core.access).clone() {
            l = l.filter(move |peer: &SocketAddr| p.allows(peer.ip()));
        }
        let listener = self
            .host
            .engine
            .add_listener(l)
            .map_err(|e| format!("listen on {}: {e}", m.listen))?;
        Ok(Running {
            key,
            member: m.clone(),
            server,
            listener,
            addrs,
            epoch,
        })
    }

    fn close_running(&self, r: &Running) {
        r.server.close_all();
        let _ = self
            .host
            .engine
            .shutdown_listener(r.listener, Duration::ZERO);
    }
}

impl Role for Media {
    fn apply(&mut self, config: &Config) -> Result<(), String> {
        self.event_loops = config.engine.event_loops;
        let Some(w) = config.webrtc.as_ref() else {
            return Ok(());
        };
        let Some(m) = w.media.as_ref() else {
            return Ok(());
        };
        let access = crate::turn::policy(m.access.as_ref(), config.access.as_ref())?;
        let settings = Arc::new(Settings::of(w, m));
        let key = static_key(m);
        let core = &self.core;
        let prev_access = std::mem::replace(&mut *lock(&core.access), access.clone());
        let same = lock(&core.st)
            .cur
            .as_ref()
            .map(|r| (r.key == key, r.listener));
        if let Some((true, listener)) = same {
            let _ = match access {
                Some(p) => self
                    .host
                    .engine
                    .set_listener_filter(listener, move |peer: &SocketAddr| p.allows(peer.ip())),
                None => self.host.engine.clear_listener_filter(listener),
            };
            write(&core.set, settings);
            return Ok(());
        }
        // A new port: the sessions end, the old Mux closes first (it may hold the address).
        let (old, ended, epoch) = {
            let mut st = lock(&core.st);
            if st.stopped {
                return Err("the role stopped".to_owned());
            }
            let ended = core.forget_all(&mut st);
            st.epoch = st.epoch.wrapping_add(1);
            (st.cur.take(), ended, st.epoch)
        };
        close(&ended);
        if let Some(o) = &old {
            self.close_running(o);
        }
        match self.start(m, key, epoch) {
            Ok(next) => {
                let addrs = next
                    .addrs
                    .iter()
                    .map(ToString::to_string)
                    .collect::<Vec<_>>()
                    .join(", ");
                if old.is_some() {
                    count(&core.m.restarts);
                    akit_observe::warn!(
                        Webrtc,
                        "webrtc.media: media on {addrs}; the server started again for a change of \
                         listen or public_ips, ending {} sessions",
                        ended.len()
                    );
                } else {
                    akit_observe::info!(Webrtc, "webrtc.media: media on {addrs}");
                }
                write(&core.set, settings);
                lock(&core.st).cur = Some(next);
                register(&self.host.engine, core);
                Ok(())
            }
            Err(e) => {
                *lock(&core.access) = prev_access;
                // The server as it was, started again (its sessions are gone).
                if let Some(o) = old {
                    match self.start(&o.member, o.key.clone(), epoch) {
                        Ok(again) => lock(&core.st).cur = Some(again),
                        Err(e2) => akit_observe::error!(
                            Webrtc,
                            "webrtc.media: the server as it was could not start again either: {e2}"
                        ),
                    }
                }
                Err(e)
            }
        }
    }

    fn stop(&mut self, _grace: Duration) {
        let core = &self.core;
        let (cur, ended) = {
            let mut st = lock(&core.st);
            st.stopped = true;
            let ended = core.forget_all(&mut st);
            (st.cur.take(), ended)
        };
        unregister(&self.host.engine, core);
        close(&ended);
        if let Some(r) = cur {
            self.close_running(&r);
        }
    }

    fn metrics(&self) -> Option<MetricsFn> {
        let core = Arc::downgrade(&self.core);
        Some(Arc::new(move |out: &mut Vec<u8>| {
            if let Some(c) = core.upgrade() {
                write_metrics(&c, out);
            }
        }))
    }
}

fn write_metrics(c: &Core, out: &mut Vec<u8>) {
    let (publishers, viewers, streams) = {
        let st = lock(&c.st);
        let p = st
            .sessions
            .values()
            .filter(|s| s.kind == Kind::Publisher)
            .count();
        (p, st.sessions.len().saturating_sub(p), st.streams.len())
    };
    let load = |a: &AtomicU64| a.load(Ordering::Relaxed);
    let n = |v: usize| u64::try_from(v).unwrap_or(u64::MAX);
    let mut w = TextWriter::new(std::mem::take(out));
    w.family(
        "akit_webrtc_media_peer_connections",
        "gauge",
        "PeerConnections of the WHIP and WHEP sessions, by role.",
    );
    w.sample_u64(
        "akit_webrtc_media_peer_connections",
        n(publishers),
        &[("role", "publisher")],
    );
    w.sample_u64(
        "akit_webrtc_media_peer_connections",
        n(viewers),
        &[("role", "viewer")],
    );
    w.family("akit_webrtc_media_streams", "gauge", "Streams published.");
    w.sample_u64("akit_webrtc_media_streams", n(streams), &[]);
    w.family(
        "akit_webrtc_media_sessions_total",
        "counter",
        "Sessions answered, by role.",
    );
    w.sample_u64(
        "akit_webrtc_media_sessions_total",
        load(&c.m.published),
        &[("role", "publisher")],
    );
    w.sample_u64(
        "akit_webrtc_media_sessions_total",
        load(&c.m.viewed),
        &[("role", "viewer")],
    );
    w.family(
        "akit_webrtc_media_refused_total",
        "counter",
        "Offers refused, by reason: a limit (peer_connections, streams, viewers), a stream \
         published already (conflict) or not at all (no_stream), or an offer that could not be \
         answered.",
    );
    for (name, v) in REFUSALS.iter().zip(&c.m.refused) {
        w.sample_u64(
            "akit_webrtc_media_refused_total",
            load(v),
            &[("reason", name)],
        );
    }
    w.family(
        "akit_webrtc_media_sessions_ended_total",
        "counter",
        "Sessions ended, by reason: a DELETE, their publisher gone, ICE or DTLS failed or the \
         peer silent, not connected in connect_timeout, or the server started again or stopped.",
    );
    for (name, v) in ENDS.iter().zip(&c.m.ended) {
        w.sample_u64(
            "akit_webrtc_media_sessions_ended_total",
            load(v),
            &[("reason", name)],
        );
    }
    for (name, typ, help, v, labels) in [
        (
            "akit_webrtc_media_received_packets_total",
            "counter",
            "RTP packets received from publishers and forwarded to their viewers.",
            load(&c.m.packets),
            None,
        ),
        (
            "akit_webrtc_media_received_bytes_total",
            "counter",
            "Bytes of the RTP packets received from publishers and forwarded.",
            load(&c.m.bytes),
            None,
        ),
        (
            "akit_webrtc_media_dropped_total",
            "counter",
            "RTP packets of publishers dropped, by reason: past max_bitrate.",
            load(&c.m.dropped_bitrate),
            Some(("reason", "bitrate")),
        ),
    ] {
        w.family(name, typ, help);
        match labels {
            Some(l) => w.sample_u64(name, v, &[l]),
            None => w.sample_u64(name, v, &[]),
        }
    }
    w.family(
        "akit_webrtc_media_key_frame_requests_total",
        "counter",
        "Viewers' key frame requests, by what became of them: sent to the publisher, or held \
         back by key_frame_interval.",
    );
    w.sample_u64(
        "akit_webrtc_media_key_frame_requests_total",
        load(&c.m.kf_sent),
        &[("result", "sent")],
    );
    w.sample_u64(
        "akit_webrtc_media_key_frame_requests_total",
        load(&c.m.kf_held),
        &[("result", "held")],
    );
    w.family(
        "akit_webrtc_media_restarts_total",
        "counter",
        "Times a config's change of listen or public_ips started the media server again, ending \
         its sessions.",
    );
    w.sample_u64("akit_webrtc_media_restarts_total", load(&c.m.restarts), &[]);
    *out = w.out;
}

#[cfg(test)]
mod tests;
