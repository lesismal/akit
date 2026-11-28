//! The streaming role (I122), as fit's package `streaming`: akio-media's hub, where each stream
//! is published once and played over every protocol, with listeners of the role's own on the
//! listeners' engine for RTMP, RTSP (interleaved RTP) and SRT, and a UDP port for WebRTC (I124).
//! Its handlers (I123, I124) serve the hub on routes, finding it by the engine ([`lookup`]).
//!
//! The WebRTC port (`streaming.webrtc`) is akio-webrtc's ICE-lite `RtcServer`, every peer on the
//! one port, carrying akio-media's WHIP and WHEP sessions (`RtcMedia`, `WhipServer`), which
//! `stream_whip` and `stream_whep` hand their POSTs to once checked ([`Rtc`]). Its host
//! candidates are `public_ips` at the port, or, for a wildcard address, every interface's address
//! that is up (not loopback, link-local or multicast), as fib's `hostAddrs`; a port of 0 is
//! chosen before the listener binds it (akio A268).
//!
//! Every listener may name a `media_auth` policy with `auth`, checked by the stream's path in the
//! hub's hooks, on the I/O thread the request arrived on: RTMP's publish and play with the
//! credentials of the stream name's or the app's query, RTSP's with the URL's query (Digest and
//! Basic wait for akio A264), SRT's in the server's connect hook with the stream ID's `s=` and
//! `u=`, or its query. The policy is read on every check, so a reload changes it for what follows.
//! A hook must answer before it returns, and nothing blocks an I/O thread (rule 1): a user whose
//! password only a bcrypt hash checks, and whom the policy has not found right before, is refused
//! once, as a try again, while the pool checks the hash and remembers it, so that the client's
//! next attempt passes at once.
//!
//! The role's `access`, then the global one, decides which client addresses the listeners serve,
//! on akio's accept filter (a refused RTMP or RTSP connection closed as it is accepted, a refused
//! SRT caller's datagrams dropped), set again by every apply without starting a listener again.
//!
//! A reload changes in place what it can: each listener's `auth`, and the access. A change of any
//! other field of a listener starts that listener again, closing its connections; a change of the
//! hub's limits starts everything again, ending every stream.

use std::net::SocketAddr;
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering::SeqCst};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

use akio::{Acceptor, Conn, Engine, ListenerId};
use akio_media::hub::{Hub, HubConfig, PeerInfo};
use akio_media::rtmp::{RtmpConfig, RtmpServer};
use akio_media::rtsp::{RtspConfig, RtspServer};
use akio_media::srt::{Mode, RejectReason, Request, SrtConfig, SrtServer, StreamId};
use akio_media::webrtc::{RtcMedia, RtcMediaConfig};
use akio_media::whip::{WhipConfig, WhipServer};
use akio_webrtc::RtcServer;
use akit_config::Config;
use akit_config::media::{
    IceServer, MediaAction as Action, StreamRtmp, StreamRtsp, StreamSrt, StreamWebRtc, Streaming,
};
use akit_roles::host::{Host, Info, MetricsFn, Role};

use crate::auth::{Credentials, Policy, Refusal, registry};
use crate::swap::Swap;

/// The role `streaming`, which `akit run` registers after `media_auth`.
pub const STREAMING: Info = Info {
    id: "streaming",
    set: |c| c.streaming.is_some(),
    new: |host| Ok(Box::new(Server::new(&host))),
};

/// The protocols a listener of the role serves, which its policies and metrics go by.
const RTMP: usize = 0;
const RTSP: usize = 1;
const SRT: usize = 2;
const PROTOS: [&str; 3] = ["rtmp", "rtsp", "srt"];

/// Why a listener refused a client's credentials, for the metrics.
const REASONS: [&str; 3] = ["unauthorized", "forbidden", "unavailable"];

fn lock<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap_or_else(PoisonError::into_inner)
}

/// A listener's policy, by name: none when the name is empty; a name the running config has no
/// policy of refuses everyone, as unavailable.
#[derive(Debug, Default)]
struct Guard {
    name: String,
    policy: Option<Arc<Policy>>,
}

impl Guard {
    /// Checks `c` for `a` on `stream`, without waiting: a bcrypt check it needs is started on the
    /// pool and [`Refusal::Pending`] returned, the client's next attempt passing at once.
    fn allow(&self, a: Action, stream: &str, c: &Credentials) -> Result<(), Refusal> {
        if self.name.is_empty() {
            return Ok(());
        }
        let Some(p) = &self.policy else {
            return Err(Refusal::Unavailable);
        };
        match p.allow(a, stream, c) {
            Ok(_) => Ok(()),
            Err(Refusal::Pending) => {
                p.check(a, stream, c, |_| {});
                Err(Refusal::Pending)
            }
            Err(r) => Err(r),
        }
    }
}

/// The role's counters.
#[derive(Debug, Default)]
struct Meters {
    published: AtomicU64,
    restarts: AtomicU64,
    refused: [[AtomicU64; 3]; 3],
}

/// What the hooks of the hub and the listeners share with the role.
#[derive(Debug, Default)]
struct Shared {
    auth: [Swap<Guard>; 3],
    meters: Meters,
}

impl Shared {
    /// Checks a client of listener `proto`, counting a refusal.
    fn check(&self, proto: usize, a: Action, stream: &str, c: &Credentials) -> Result<(), Refusal> {
        let Some(g) = self.auth.get(proto) else {
            return Ok(());
        };
        let r = g.with(|g| g.map_or(Ok(()), |g| g.allow(a, stream, c)));
        if let Err(e) = r {
            let i = match e.status() {
                401 => 0,
                403 => 1,
                _ => 2,
            };
            if let Some(n) = self.meters.refused.get(proto).and_then(|r| r.get(i)) {
                n.fetch_add(1, SeqCst);
            }
        }
        r
    }

    /// The hub's hook for a publish or play: RTMP's and RTSP's checked by their listener's policy
    /// with the query's credentials; SRT's were in its connect hook, and the handlers' in their
    /// guard, before the hub was asked.
    fn hook(&self, a: Action, path: &str, info: &PeerInfo) -> Result<(), String> {
        let proto = match info.protocol {
            "rtmp" => RTMP,
            "rtsp" => RTSP,
            _ => return self.counted(a),
        };
        match self.check(proto, a, path, &Credentials::from_query(&info.query)) {
            Ok(()) => self.counted(a),
            // The client may try again at once: the password is being checked.
            Err(Refusal::Pending) => Err("the password is being checked; try again".into()),
            Err(r) => Err(r.to_string()),
        }
    }

    fn counted(&self, a: Action) -> Result<(), String> {
        if a == Action::Publish {
            self.meters.published.fetch_add(1, SeqCst);
        }
        Ok(())
    }
}

/// The paths of akio-media's WHIP and WHEP server ([`Rtc::router`]), which `stream_whip` and
/// `stream_whep` map their routes' paths to (fit's `rtcWHIP`, `rtcWHEP` and `rtcResource`).
pub const RTC_WHIP: &str = "/whip/";
/// See [`RTC_WHIP`].
pub const RTC_WHEP: &str = "/whep/";
/// The path of a session's URL, followed by its ID. See [`RTC_WHIP`].
pub const RTC_RESOURCE: &str = "/rtc/session/";

/// The role's WebRTC port (I124): what `stream_whip` and `stream_whep` serve.
pub struct Rtc {
    /// akio-media's WHIP and WHEP endpoints, at [`RTC_WHIP`], [`RTC_WHEP`] and [`RTC_RESOURCE`].
    pub router: akio_http::Router,
    /// `max_offer_bytes`: the largest offer, or PATCH body, taken.
    pub max_offer_bytes: usize,
    whip: WhipServer,
    server: RtcServer,
}

impl std::fmt::Debug for Rtc {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Rtc")
            .field("whip", &self.whip)
            .field("max_offer_bytes", &self.max_offer_bytes)
            .finish_non_exhaustive()
    }
}

impl Rtc {
    /// The WHIP and WHEP sessions live.
    #[must_use]
    pub fn sessions(&self) -> usize {
        self.whip.sessions()
    }

    /// Ends every session and closes every peer.
    fn close(&self) {
        self.whip.close_all();
        self.server.close_all();
    }
}

/// What the role's handlers serve from (I123, I124): the hub, while the role runs.
#[derive(Debug, Clone)]
pub struct Current {
    /// The hub.
    pub hub: Hub,
    /// Which hub it is: another number for each hub the role makes, so that what a handler made
    /// for one hub is made again for the next.
    pub hub_no: u64,
    /// The WebRTC port, while `streaming.webrtc` is set.
    pub rtc: Option<Arc<Rtc>>,
    /// The channels (I145).
    pub channels: Option<Arc<crate::channels::Manager>>,
}

/// The numbers of the hubs ([`Current::hub_no`]).
static HUB_NOS: AtomicU64 = AtomicU64::new(1);

/// The role of each engine, by [`registry::key`]: what the handlers find it by, and its
/// counters.
#[derive(Debug, Default)]
pub struct Slot {
    cur: Swap<Current>,
    shared: Arc<Shared>,
}

impl Slot {
    /// Runs `f` with the hub the role serves now, or `None` while it does not run.
    pub fn with<R>(&self, f: impl FnOnce(Option<&Current>) -> R) -> R {
        self.cur.with(f)
    }
}

type Slots = Mutex<std::collections::HashMap<usize, Arc<Slot>>>;

fn slots() -> &'static Slots {
    static SLOTS: std::sync::LazyLock<Slots> =
        std::sync::LazyLock::new(|| Mutex::new(std::collections::HashMap::new()));
    &SLOTS
}

/// The slot of the engine `key`, made when first asked for: the handlers' and the role's.
#[must_use]
pub fn lookup(key: usize) -> Arc<Slot> {
    Arc::clone(lock(slots()).entry(key).or_default())
}

/// Writes the metrics of the streaming role of `engine`, when it has run there, as `/metrics`
/// shows them ([`Role::metrics`]).
pub fn write_metrics_of(engine: &Arc<Engine>, w: &mut dyn akit_observe::MetricWriter) {
    let slot = lock(slots()).get(&registry::key(engine)).cloned();
    if let Some(slot) = slot {
        write_metrics(&slot, w);
    }
}

/// An acceptor that closes the connections past `max` as they open (fit's `connLimit`): the
/// count of those open goes down when the peer goes.
struct Limited<A> {
    inner: A,
    open: Arc<AtomicUsize>,
    max: usize,
}

/// The key of the limit's hook among a connection's peer-gone hooks.
const LIMIT_HOOK: u64 = 0x6c69_6d69_7400_0001;

impl<A: Acceptor> Acceptor for Limited<A> {
    type Handler = A::Handler;

    fn on_open(&self, c: &Conn<A::Handler>) -> A::Handler {
        let h = self.inner.on_open(c);
        if self.open.fetch_add(1, SeqCst) >= self.max {
            self.open.fetch_sub(1, SeqCst);
            c.close();
            return h;
        }
        let open = Arc::clone(&self.open);
        c.inner().on_peer_gone(
            LIMIT_HOOK,
            Box::new(move || {
                open.fetch_sub(1, SeqCst);
            }),
        );
        h
    }

    fn on_shutdown(&self, c: &Conn<A::Handler>) {
        self.inner.on_shutdown(c);
    }

    fn transport(&self) -> Option<Box<dyn akio::Transport>> {
        self.inner.transport()
    }
}

/// A listener of the role: its engine listener, and the config it was started with.
struct Bound<C> {
    id: ListenerId,
    /// What, changed, starts it again (fit's `rtmpKey`, `rtspKey`, `srtKey`).
    key: String,
    /// The address it is bound to.
    addr: Option<SocketAddr>,
    cfg: C,
    /// The WebRTC port's server, for the WebRTC listener.
    rtc: Option<Arc<Rtc>>,
}

/// A local address to listen on, as the tunnel's: `:port` every address, `localhost` the
/// loopback one.
fn listen_addr(a: &str) -> Option<SocketAddr> {
    if let Ok(s) = a.parse() {
        return Some(s);
    }
    let (host, port) = a.rsplit_once(':')?;
    let port: u16 = port.parse().ok()?;
    let ip = match host.trim_start_matches('[').trim_end_matches(']') {
        "" => std::net::IpAddr::V6(std::net::Ipv6Addr::UNSPECIFIED),
        h if h.eq_ignore_ascii_case("localhost") => {
            std::net::IpAddr::V4(std::net::Ipv4Addr::LOCALHOST)
        }
        h => h.parse().ok()?,
    };
    Some(SocketAddr::new(ip, port))
}

/// Whether two listen addresses may take the same port: the old listener must go first.
fn same_address(a: &str, b: &str) -> bool {
    match (
        akit_config_bind_key(a).filter(|(_, p)| *p != 0),
        akit_config_bind_key(b).filter(|(_, p)| *p != 0),
    ) {
        (Some((ha, pa)), Some((hb, pb))) => pa == pb && (ha == hb || ha == "*" || hb == "*"),
        _ => false,
    }
}

fn akit_config_bind_key(a: &str) -> Option<(String, u16)> {
    let (h, p) = a.rsplit_once(':')?;
    let port = p.parse().ok()?;
    let h = h.trim_start_matches('[').trim_end_matches(']');
    let h = if h.is_empty() || h == "0.0.0.0" || h == "::" {
        "*".to_owned()
    } else if h.eq_ignore_ascii_case("localhost") {
        "127.0.0.1".to_owned()
    } else {
        h.to_ascii_lowercase()
    };
    Some((h, port))
}

/// The role: the hub and its listeners.
pub struct Server {
    engine: Arc<Engine>,
    slot: Arc<Slot>,
    shared: Arc<Shared>,
    hub: Option<Hub>,
    hub_no: u64,
    hub_key: String,
    /// The channels of the hub (I145), found by the Admin API by the engine.
    channels: Option<Arc<crate::channels::Manager>>,
    access: Option<Arc<akit_access::Policy>>,
    rtmp: Option<Bound<StreamRtmp>>,
    rtsp: Option<Bound<StreamRtsp>>,
    srt: Option<Bound<StreamSrt>>,
    webrtc: Option<Bound<StreamWebRtc>>,
    /// Where the listeners are bound, as [`StreamingApi`] tells it: set after each apply.
    bound: Arc<Mutex<Addrs>>,
}

/// Where each of the role's listeners is bound, by protocol: rtmp, rtsp, srt, webrtc.
type Addrs = [Option<SocketAddr>; 4];

/// The streaming role's handle (`Role::api`): where its listeners are bound, the port each got
/// when its `listen` said 0.
#[derive(Clone)]
pub struct StreamingApi(Arc<Mutex<Addrs>>);

impl std::fmt::Debug for StreamingApi {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("StreamingApi").finish_non_exhaustive()
    }
}

impl StreamingApi {
    /// The address the listener of `proto` (`rtmp`, `rtsp`, `srt`, `webrtc`) is bound to, or
    /// `None` while it does not run.
    #[must_use]
    pub fn addr(&self, proto: &str) -> Option<SocketAddr> {
        let i = ["rtmp", "rtsp", "srt", "webrtc"]
            .iter()
            .position(|p| *p == proto)?;
        self.0
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .get(i)
            .copied()
            .flatten()
    }
}

impl std::fmt::Debug for Server {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Server")
            .field("hub_key", &self.hub_key)
            .finish_non_exhaustive()
    }
}

/// The policy of the role's own `access`, then the config's global one.
fn access_policy(
    own: Option<&akit_config::Access>,
    global: Option<&akit_config::Access>,
) -> Result<Option<Arc<akit_access::Policy>>, String> {
    let err = |es: Vec<akit_access::Error>| {
        es.iter()
            .map(|e| format!("access.{}: {}", e.field, e.msg))
            .collect::<Vec<_>>()
            .join("; ")
    };
    let own = akit_access::compile(own, None).map_err(err)?;
    let global = akit_access::compile(global, None).map_err(err)?;
    Ok(akit_access::Policy::chain([own.as_ref(), global.as_ref()]))
}

fn rtmp_key(c: &StreamRtmp) -> String {
    format!(
        "{:?} {} {:?} {:?} {:?}",
        c.listen,
        c.conns(),
        c.handshake(),
        c.idle(),
        c.play_wait
    )
}

fn rtsp_key(c: &StreamRtsp) -> String {
    format!(
        "{:?} {} {:?} {:?}",
        c.listen,
        c.conns(),
        c.session(),
        c.play_wait
    )
}

fn srt_key(c: &StreamSrt) -> String {
    format!(
        "{:?} {} {:?} {:?} {:?} {:?}",
        c.listen,
        c.conns(),
        c.latency_or(),
        c.passphrase,
        c.peer_idle(),
        c.play_wait
    )
}

fn rtc_key(c: &StreamWebRtc) -> String {
    format!(
        "{:?} {:?} {:?} {} {:?} {:?} {}",
        c.listen,
        c.public_ips,
        c.ice_servers,
        c.sessions(),
        c.connect(),
        c.play_wait,
        c.offer_bytes()
    )
}

/// The `Link` header values that tell WHIP and WHEP clients of `servers` (RFC 9725 section 4.6),
/// as fib's `linkHeaders`.
fn ice_links(servers: &[IceServer]) -> Vec<String> {
    let mut out = Vec::new();
    for s in servers {
        for u in &s.urls {
            let mut v = format!("<{u}>; rel=\"ice-server\"");
            if !s.username.is_empty() || !s.credential.is_empty() {
                v.push_str(&format!(
                    "; username={:?}; credential={:?}; credential-type=\"password\"",
                    s.username, s.credential
                ));
            }
            out.push(v);
        }
    }
    out
}

/// Whether `ip` may be a host candidate: not loopback, link-local, multicast or unspecified
/// (fib's `hostAddr`).
fn host_candidate(ip: std::net::IpAddr) -> bool {
    use std::net::IpAddr;
    let ip = match ip {
        IpAddr::V6(v6) => v6.to_ipv4_mapped().map_or(IpAddr::V6(v6), IpAddr::V4),
        v4 => v4,
    };
    let link_local = match ip {
        IpAddr::V4(a) => a.is_link_local(),
        IpAddr::V6(a) => a.segments().first().is_some_and(|s| s & 0xffc0 == 0xfe80),
    };
    !(ip.is_loopback() || link_local || ip.is_multicast() || ip.is_unspecified())
}

/// The host candidates of a WebRTC port bound at `bound` (fib's `hostAddrs`): `public` at its
/// port; for a wildcard address every interface's address that is up and may be one (IPv4 only
/// for `0.0.0.0`); else the address itself.
fn host_addrs(bound: SocketAddr, public: &[String]) -> Result<Vec<SocketAddr>, String> {
    let mut out: Vec<SocketAddr> = Vec::new();
    let mut add = |a: SocketAddr| {
        if !out.contains(&a) {
            out.push(a);
        }
    };
    if !public.is_empty() {
        for p in public {
            let ip: std::net::IpAddr = p
                .parse()
                .map_err(|_| format!("public_ips: {p:?} is not an IP address"))?;
            add(SocketAddr::new(ip.to_canonical(), bound.port()));
        }
    } else if bound.ip().is_unspecified() {
        let ifaces = akio_sys::interfaces().map_err(|e| format!("the interfaces: {e}"))?;
        for i in ifaces
            .iter()
            .filter(|i| i.flags.is_up() && !i.flags.is_loopback())
        {
            for a in &i.addrs {
                if bound.is_ipv4() && !a.ip.is_ipv4() {
                    continue;
                }
                if host_candidate(a.ip) {
                    add(SocketAddr::new(a.ip, bound.port()));
                }
            }
        }
    } else {
        add(bound);
    }
    if out.is_empty() {
        return Err("no address to advertise: set public_ips".to_owned());
    }
    Ok(out)
}

fn wait(d: Duration) -> Option<Duration> {
    (!d.is_zero()).then_some(d)
}

/// The credentials and action of an SRT caller's stream ID (fit's `srtConnect`): `s=` a token,
/// or the password of `u=`; or the resource's query's token, or user and pass.
fn srt_credentials(id: &StreamId) -> (Action, Credentials) {
    let a = if id.mode == Mode::Publish {
        Action::Publish
    } else {
        Action::Play
    };
    let c = if !id.user.is_empty() && !id.session.is_empty() {
        Credentials {
            user: id.user.clone(),
            password: id.session.clone(),
            ..Credentials::default()
        }
    } else if !id.session.is_empty() {
        Credentials {
            token: id.session.clone(),
            ..Credentials::default()
        }
    } else {
        Credentials::from_query(&id.query)
    };
    (a, c)
}

impl Server {
    fn new(host: &Host) -> Server {
        let key = registry::key(&host.engine);
        let slot = lookup(key);
        Server {
            engine: Arc::clone(&host.engine),
            shared: Arc::clone(&slot.shared),
            slot,
            hub: None,
            hub_no: 0,
            hub_key: String::new(),
            channels: None,
            access: None,
            rtmp: None,
            rtsp: None,
            srt: None,
            webrtc: None,
            bound: Arc::default(),
        }
    }

    /// Notes where the listeners are bound now, for [`StreamingApi`].
    fn note_bound(&self) {
        *self.bound.lock().unwrap_or_else(PoisonError::into_inner) = [
            self.addr("rtmp"),
            self.addr("rtsp"),
            self.addr("srt"),
            self.addr("webrtc"),
        ];
    }

    /// The address a listener of the role is bound to, by its protocol (`rtmp`, `rtsp`, `srt`,
    /// `webrtc`),
    /// or `None` while it does not run: for tests and logs.
    #[must_use]
    pub fn addr(&self, proto: &str) -> Option<SocketAddr> {
        match proto {
            "rtmp" => self.rtmp.as_ref().and_then(|b| b.addr),
            "rtsp" => self.rtsp.as_ref().and_then(|b| b.addr),
            "srt" => self.srt.as_ref().and_then(|b| b.addr),
            "webrtc" => self.webrtc.as_ref().and_then(|b| b.addr),
            _ => None,
        }
    }

    /// Points each listener at its policy.
    fn set_auth(&self, cfg: &Streaming) {
        let key = registry::key(&self.engine);
        let names = [
            cfg.rtmp.as_ref().map(|r| r.auth.clone()),
            cfg.rtsp.as_ref().map(|r| r.auth.clone()),
            cfg.srt.as_ref().map(|r| r.auth.clone()),
        ];
        for (swap, name) in self.shared.auth.iter().zip(names) {
            let name = name.unwrap_or_default();
            // The role media_auth applies before this one: the policy is found now.
            let policy = (!name.is_empty())
                .then(|| registry::lookup(key, &name))
                .flatten();
            swap.set(Some(Arc::new(Guard { name, policy })));
        }
    }

    /// Makes the hub, with hooks that check the listeners' policies.
    fn make_hub(&self, cfg: &Streaming) -> Hub {
        let mut hc = HubConfig::default();
        hc.gop_cache_bytes = cfg.gop_cache();
        hc.max_streams = cfg.streams();
        hc.max_subscribers = cfg.players();
        let s = Arc::clone(&self.shared);
        hc.on_publish = Some(Arc::new(move |path: &str, info: &PeerInfo| {
            s.hook(Action::Publish, path, info)
        }));
        let s = Arc::clone(&self.shared);
        hc.on_subscribe = Some(Arc::new(move |path: &str, info: &PeerInfo| {
            s.hook(Action::Play, path, info)
        }));
        // A publisher leaving tells the role's channels too: one whose stream was taken, or
        // which found the hub full, claims it then (I145).
        let channels = crate::channels::unpublish_hook(registry::key(&self.engine));
        hc.on_unpublish = Some(Arc::new(move |path: &str| {
            akit_observe::info!(Webrtc, "streaming: {path} unpublished");
            channels(path);
        }));
        Hub::new(hc)
    }

    /// The accept filter of the role's access, or none.
    fn filter(&self) -> Option<impl Fn(&SocketAddr) -> bool + Send + Sync + 'static> {
        self.access
            .clone()
            .map(|p| move |a: &SocketAddr| p.allows(a.ip()))
    }

    /// Adds a listener of `acceptor` on `listen`, TCP or UDP, under the role's access.
    fn bind(
        &self,
        listen: &str,
        udp: bool,
        build: impl FnOnce(akio::Listener) -> akio::Listener,
    ) -> Result<(ListenerId, Option<SocketAddr>), String> {
        let addr =
            listen_addr(listen).ok_or_else(|| format!("{listen}: not an address to listen on"))?;
        let mut l = if udp {
            akio::Listener::udp(addr)
        } else {
            akit_runtime::scheduled(akio::Listener::tcp(addr))
        };
        l = build(l);
        if let Some(f) = self.filter() {
            l = l.filter(f);
        }
        let id = self
            .engine
            .add_listener(l)
            .map_err(|e| format!("listen on {listen}: {e}"))?;
        let bound = self
            .engine
            .listeners()
            .into_iter()
            .find(|i| i.id == id)
            .map(|i| i.local_addr);
        Ok((id, bound))
    }

    fn close<C>(&self, b: &Bound<C>) {
        let _ = self.engine.shutdown_listener(b.id, Duration::ZERO);
        if let Some(r) = &b.rtc {
            r.close();
        }
    }

    fn start_rtmp(&self, hub: &Hub, c: &StreamRtmp) -> Result<Bound<StreamRtmp>, String> {
        let mut rc = RtmpConfig::default();
        rc.handshake_timeout = c.handshake();
        rc.idle_timeout = c.idle();
        rc.play_wait = wait(c.play_wait);
        let srv = Limited {
            inner: RtmpServer::new(hub.clone(), rc),
            open: Arc::new(AtomicUsize::new(0)),
            max: c.conns(),
        };
        let (id, addr) = self.bind(&c.listen, false, |l| l.serve(srv))?;
        akit_observe::info!(Webrtc, "streaming: rtmp on {}", show(addr, &c.listen));
        Ok(Bound {
            id,
            key: rtmp_key(c),
            addr,
            cfg: c.clone(),
            rtc: None,
        })
    }

    fn start_rtsp(&self, hub: &Hub, c: &StreamRtsp) -> Result<Bound<StreamRtsp>, String> {
        let mut rc = RtspConfig::default();
        rc.session_timeout = c.session();
        rc.play_wait = wait(c.play_wait);
        let srv = Limited {
            inner: RtspServer::new(hub.clone(), rc),
            open: Arc::new(AtomicUsize::new(0)),
            max: c.conns(),
        };
        let (id, addr) = self.bind(&c.listen, false, |l| l.serve(srv))?;
        akit_observe::info!(Webrtc, "streaming: rtsp on {}", show(addr, &c.listen));
        Ok(Bound {
            id,
            key: rtsp_key(c),
            addr,
            cfg: c.clone(),
            rtc: None,
        })
    }

    fn start_srt(&self, hub: &Hub, c: &StreamSrt) -> Result<Bound<StreamSrt>, String> {
        let mut sc = SrtConfig::default();
        sc.latency = c.latency_or();
        if !c.passphrase.is_empty() {
            sc.passphrase = Some(c.passphrase.clone());
        }
        sc.play_wait = wait(c.play_wait);
        sc.max_conns = c.conns();
        sc.peer_idle_timeout = c.peer_idle();
        let s = Arc::clone(&self.shared);
        sc.on_connect = Some(Arc::new(move |_r: &Request, id: &StreamId| {
            // fit's syntax `r=live/cam?token=T` is not akio-media's: its resource would name
            // another stream (akio A265).
            if id.query.is_empty() && id.resource.contains('?') {
                return Err(RejectReason::BAD_REQUEST);
            }
            let (a, c) = srt_credentials(id);
            match s.check(SRT, a, &id.resource, &c) {
                Ok(()) => Ok(()),
                Err(r) => Err(match r.status() {
                    401 => RejectReason::UNAUTHORIZED,
                    403 => RejectReason::FORBIDDEN,
                    _ => RejectReason::OVERLOAD,
                }),
            }
        }));
        let srv = SrtServer::new(hub.clone(), sc);
        let (id, addr) = self.bind(&c.listen, true, |l| l.serve(srv))?;
        akit_observe::info!(Webrtc, "streaming: srt on {}", show(addr, &c.listen));
        Ok(Bound {
            id,
            key: srt_key(c),
            addr,
            cfg: c.clone(),
            rtc: None,
        })
    }

    /// The WebRTC port (I124): an ICE-lite `RtcServer` on a UDP listener, and the WHIP and WHEP
    /// server of its sessions in front of the hub.
    fn start_webrtc(&self, hub: &Hub, c: &StreamWebRtc) -> Result<Bound<StreamWebRtc>, String> {
        let mut addr = listen_addr(&c.listen)
            .ok_or_else(|| format!("{}: not an address to listen on", c.listen))?;
        if addr.port() == 0 {
            // The answers name the port before the listener binds it: a free one is found first,
            // as akio-media's own tests do (akio A268).
            let probe = std::net::UdpSocket::bind(addr)
                .map_err(|e| format!("listen on {}: {e}", c.listen))?;
            let port = probe
                .local_addr()
                .map_err(|e| format!("listen on {}: {e}", c.listen))?
                .port();
            drop(probe);
            addr.set_port(port);
        }
        let hosts = host_addrs(addr, &c.public_ips)?;
        let server = RtcMedia::rtc_server(hosts)
            .connect_timeout(Some(c.connect()))
            .max_peers(c.sessions());
        let media = RtcMedia::new(server.clone(), RtcMediaConfig::default());
        let mut wc = WhipConfig::default();
        wc.whip_prefix = RTC_WHIP.trim_end_matches('/').to_owned();
        wc.whep_prefix = RTC_WHEP.trim_end_matches('/').to_owned();
        wc.resource_prefix = RTC_RESOURCE.trim_end_matches('/').to_owned();
        wc.max_sessions = c.sessions();
        wc.cors_origin = None;
        wc.ice_links = ice_links(&c.ice_servers);
        let whip = WhipServer::new(hub.clone(), Arc::new(media), wc);
        let router = whip.routes(akio_http::Router::new());
        let rtc = Arc::new(Rtc {
            router,
            max_offer_bytes: c.offer_bytes(),
            whip,
            server: server.clone(),
        });
        let (id, bound) = match self.bind(&addr.to_string(), true, |l| l.serve(server)) {
            Ok(b) => b,
            Err(e) => {
                rtc.close();
                return Err(e);
            }
        };
        if !c.play_wait.is_zero() {
            akit_observe::warn!(
                Webrtc,
                "streaming.webrtc.play_wait: a WHEP offer for a stream with no publisher is answered 404 at once (akio A267)"
            );
        }
        akit_observe::info!(Webrtc, "streaming: webrtc on {}", show(bound, &c.listen));
        Ok(Bound {
            id,
            key: rtc_key(c),
            addr: bound,
            cfg: c.clone(),
            rtc: Some(rtc),
        })
    }

    /// Moves one listener to `want`, as fit's `restart`: kept when its key is the same; else the
    /// old one closed (first, when the two may share an address) and the new one started, the old
    /// one started again when the new one cannot start.
    fn restart<C>(
        &self,
        old: Option<Bound<C>>,
        want: Option<&C>,
        key: impl Fn(&C) -> String,
        listen: impl Fn(&C) -> String,
        start: impl Fn(&Self, &C) -> Result<Bound<C>, String>,
    ) -> (Option<Bound<C>>, Option<String>) {
        let Some(want) = want else {
            if let Some(b) = &old {
                self.close(b);
            }
            return (None, None);
        };
        let Some(old) = old else {
            return match start(self, want) {
                Ok(b) => (Some(b), None),
                Err(e) => (None, Some(e)),
            };
        };
        if old.key == key(want) {
            return (Some(old), None);
        }
        let to = listen(want);
        let shares = same_address(&listen(&old.cfg), &to)
            || old.addr.is_some_and(|a| same_address(&a.to_string(), &to));
        if shares {
            self.close(&old);
            return match start(self, want) {
                Ok(b) => (Some(b), None),
                Err(e) => match start(self, &old.cfg) {
                    Ok(back) => (Some(back), Some(e)),
                    Err(e2) => (
                        None,
                        Some(format!(
                            "{e}; the listener as it was could not start again: {e2}"
                        )),
                    ),
                },
            };
        }
        match start(self, want) {
            Ok(b) => {
                self.close(&old);
                (Some(b), None)
            }
            Err(e) => (Some(old), Some(e)),
        }
    }

    /// Closes every listener and the hub.
    fn stop_all(&mut self) {
        if let Some(b) = self.rtmp.take() {
            self.close(&b);
        }
        if let Some(b) = self.rtsp.take() {
            self.close(&b);
        }
        if let Some(b) = self.srt.take() {
            self.close(&b);
        }
        if let Some(b) = self.webrtc.take() {
            self.close(&b);
        }
        if let Some(m) = self.channels.take() {
            m.stop();
            crate::channels::unregister(registry::key(&self.engine), &m);
        }
        if let Some(h) = self.hub.take() {
            h.close();
        }
        self.slot.cur.set(None);
        self.note_bound();
    }
}

fn show(addr: Option<SocketAddr>, listen: &str) -> String {
    addr.map_or_else(|| listen.to_owned(), |a| a.to_string())
}

impl Role for Server {
    fn apply(&mut self, config: &Config) -> Result<(), String> {
        let Some(cfg) = config.streaming.as_ref() else {
            return Ok(());
        };
        self.access = access_policy(cfg.access.as_ref(), config.access.as_ref())?;
        self.set_auth(cfg);
        let hub_key = format!("{} {} {}", cfg.streams(), cfg.players(), cfg.gop_cache());
        if self.hub.is_some() && hub_key != self.hub_key {
            akit_observe::warn!(
                Webrtc,
                "streaming: the hub's limits changed: every stream and connection ends"
            );
            self.shared.meters.restarts.fetch_add(1, SeqCst);
            self.stop_all();
        }
        let hub = match &self.hub {
            Some(h) => h.clone(),
            None => {
                let h = self.make_hub(cfg);
                let key = registry::key(&self.engine);
                let m = Arc::new(crate::channels::Manager::new(crate::channels::Options {
                    hub: h.clone(),
                    libraries: key,
                    clock: None,
                    seed: 0,
                    dial: Some(Arc::clone(&self.engine)),
                }));
                crate::channels::register(key, &m);
                self.channels = Some(m);
                self.hub = Some(h.clone());
                self.hub_no = HUB_NOS.fetch_add(1, SeqCst);
                self.hub_key = hub_key;
                h
            }
        };
        // The access of the listeners kept, set again: the next connection goes by it.
        let ids = [
            self.rtmp.as_ref().map(|b| b.id),
            self.rtsp.as_ref().map(|b| b.id),
            self.srt.as_ref().map(|b| b.id),
            self.webrtc.as_ref().map(|b| b.id),
        ];
        for id in ids.into_iter().flatten() {
            let _ = match self.filter() {
                Some(f) => self.engine.set_listener_filter(id, f),
                None => self.engine.clear_listener_filter(id),
            };
        }
        let mut errs = Vec::new();
        let old = self.rtmp.take();
        let (rtmp, e) = self.restart(
            old,
            cfg.rtmp.as_ref(),
            rtmp_key,
            |c| c.listen.clone(),
            |s, c| s.start_rtmp(&hub, c),
        );
        self.rtmp = rtmp;
        errs.extend(e.map(|e| format!("rtmp: {e}")));
        let old = self.rtsp.take();
        let (rtsp, e) = self.restart(
            old,
            cfg.rtsp.as_ref(),
            rtsp_key,
            |c| c.listen.clone(),
            |s, c| s.start_rtsp(&hub, c),
        );
        self.rtsp = rtsp;
        errs.extend(e.map(|e| format!("rtsp: {e}")));
        let old = self.srt.take();
        let (srt, e) = self.restart(
            old,
            cfg.srt.as_ref(),
            srt_key,
            |c| c.listen.clone(),
            |s, c| s.start_srt(&hub, c),
        );
        self.srt = srt;
        errs.extend(e.map(|e| format!("srt: {e}")));
        let old = self.webrtc.take();
        let (webrtc, e) = self.restart(
            old,
            cfg.webrtc.as_ref(),
            rtc_key,
            |c| c.listen.clone(),
            |s, c| s.start_webrtc(&hub, c),
        );
        self.webrtc = webrtc;
        errs.extend(e.map(|e| format!("webrtc: {e}")));
        if let Some(m) = &self.channels {
            if let Err(e) = m.apply(&cfg.channels, &cfg.push_targets()) {
                errs.push(format!("channels: {e}"));
            }
        }
        self.note_bound();
        self.slot.cur.set(Some(Arc::new(Current {
            hub,
            hub_no: self.hub_no,
            rtc: self.webrtc.as_ref().and_then(|b| b.rtc.clone()),
            channels: self.channels.clone(),
        })));
        if errs.is_empty() {
            Ok(())
        } else {
            Err(errs.join("; "))
        }
    }

    fn stop(&mut self, _grace: Duration) {
        self.stop_all();
    }

    fn api(&self) -> Option<akit_roles::host::Api> {
        Some(Arc::new(StreamingApi(Arc::clone(&self.bound))))
    }

    /// The role's metrics on `/metrics` (`akit_streaming_*`).
    fn metrics(&self) -> Option<MetricsFn> {
        let slot = Arc::clone(&self.slot);
        Some(Arc::new(move |out: &mut Vec<u8>| {
            let mut w = akit_observe::metrics::TextWriter::new(std::mem::take(out));
            write_metrics(&slot, &mut w);
            *out = w.out;
        }))
    }
}

/// The role's metrics, as fit's, named akit's (decision 037).
fn write_metrics(slot: &Slot, w: &mut dyn akit_observe::MetricWriter) {
    let shared = &slot.shared;
    let (mut streams, mut publishing, mut players) = (0u64, 0u64, 0u64);
    slot.with(|c| {
        if let Some(c) = c {
            for s in c.hub.streams() {
                streams = streams.saturating_add(1);
                if s.publishing {
                    publishing = publishing.saturating_add(1);
                }
                players = players.saturating_add(s.subscribers as u64);
            }
        }
    });
    w.family(
        "akit_streaming_streams",
        "gauge",
        "Streams of the hub, by whether they have a publisher.",
    );
    w.sample_u64(
        "akit_streaming_streams",
        publishing,
        &[("state", "publishing")],
    );
    w.sample_u64(
        "akit_streaming_streams",
        streams.saturating_sub(publishing),
        &[("state", "waiting")],
    );
    w.family(
        "akit_streaming_players",
        "gauge",
        "Players of the hub's streams, over every protocol; an HLS stream and format counts once.",
    );
    w.sample_u64("akit_streaming_players", players, &[]);
    w.family(
        "akit_streaming_published_total",
        "counter",
        "Streams published.",
    );
    w.sample_u64(
        "akit_streaming_published_total",
        shared.meters.published.load(SeqCst),
        &[],
    );
    w.family(
        "akit_streaming_auth_refused_total",
        "counter",
        "Clients of the RTMP, RTSP and SRT listeners their media_auth policy refused, by protocol and reason: no or bad credentials (unauthorized), not allowed (forbidden), or the policy not loaded or a password still being checked (unavailable).",
    );
    for (p, name) in PROTOS.iter().enumerate() {
        for (r, reason) in REASONS.iter().enumerate() {
            let n = shared
                .meters
                .refused
                .get(p)
                .and_then(|x| x.get(r))
                .map_or(0, |n| n.load(SeqCst));
            w.sample_u64(
                "akit_streaming_auth_refused_total",
                n,
                &[("protocol", name), ("reason", reason)],
            );
        }
    }
    w.family(
        "akit_streaming_restarts_total",
        "counter",
        "Times a change of the hub's limits started the role again, ending every stream.",
    );
    w.sample_u64(
        "akit_streaming_restarts_total",
        shared.meters.restarts.load(SeqCst),
        &[],
    );
    // The channels' (I145).
    if let Some(m) = slot.with(|c| c.and_then(|c| c.channels.clone())) {
        m.write_metrics(w);
    }
}

#[cfg(test)]
mod tests;
