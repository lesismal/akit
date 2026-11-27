//! fit's WebRTC roles of the config (I110): the TURN server `webrtc.turn`, the media server
//! `webrtc.media` of the `whip` and `whep` handlers, and the relay `webrtc.proxy` of the
//! `webrtc_proxy` handler, with fit's fields, defaults, bounds and checks (`config/webrtc.go`),
//! and the three handlers' params (`webrtc/media/handler.go`, `webrtc/media/proxyhttp.go`).
//!
//! The members are read as strictly as the rest of the config. What is left out is kept empty
//! (a string, a list), zero (a number, a duration) or `None`; the methods give fit's defaults
//! for it.

use std::net::{IpAddr, SocketAddr};
use std::time::Duration;

use crate::decode::{Decoder, Obj};
use crate::error::{join_field, join_index, join_key};
use crate::json::{Kind, Node};
use crate::model::{Access, Certificate};

// ---------------------------------------------------------------------------------------------
// The defaults, fit's.

/// `max_allocations` when left out or 0.
pub const DEFAULT_TURN_MAX_ALLOCATIONS: usize = 1024;
/// `max_allocations_per_user` when left out or 0.
pub const DEFAULT_TURN_MAX_ALLOCATIONS_PER_USER: usize = 16;
/// `user_bandwidth` when left out or 0: 2 MiB a second.
pub const DEFAULT_TURN_USER_BANDWIDTH: u64 = 2 << 20;
/// `default_lifetime` when left out.
pub const DEFAULT_TURN_LIFETIME: Duration = Duration::from_secs(600);
/// `max_lifetime` when left out.
pub const DEFAULT_TURN_MAX_LIFETIME: Duration = Duration::from_secs(3600);
/// `nonce_lifetime` when left out.
pub const DEFAULT_TURN_NONCE_LIFETIME: Duration = Duration::from_secs(600);
/// `max_permissions` when left out or 0.
pub const DEFAULT_TURN_MAX_PERMISSIONS: usize = 64;
/// `max_channels` when left out or 0.
pub const DEFAULT_TURN_MAX_CHANNELS: usize = 64;
/// `max_stream_message` when left out or 0: 8 KiB.
pub const DEFAULT_TURN_MAX_STREAM_MESSAGE: usize = 8 << 10;
/// `stream_idle_timeout` when left out.
pub const DEFAULT_TURN_STREAM_IDLE_TIMEOUT: Duration = Duration::from_secs(30);
/// The least `max_stream_message` may be: a ChannelData message carrying a datagram of a common
/// MTU.
pub const MIN_TURN_STREAM_MESSAGE: usize = 2048;

/// `max_peer_connections` when left out or 0.
pub const DEFAULT_MEDIA_MAX_PEER_CONNECTIONS: usize = 1024;
/// `max_streams` when left out or 0.
pub const DEFAULT_MEDIA_MAX_STREAMS: usize = 64;
/// `max_viewers` when left out or 0.
pub const DEFAULT_MEDIA_MAX_VIEWERS: usize = 256;
/// `max_bitrate` when left out or 0, in bits a second.
pub const DEFAULT_MEDIA_MAX_BITRATE: u64 = 8_000_000;
/// `max_sdp_bytes` when left out or 0: 32 KiB.
pub const DEFAULT_MEDIA_MAX_SDP_BYTES: usize = 32 << 10;
/// `connect_timeout` when left out.
pub const DEFAULT_MEDIA_CONNECT_TIMEOUT: Duration = Duration::from_secs(30);
/// `ice_timeout` when left out.
pub const DEFAULT_MEDIA_ICE_TIMEOUT: Duration = Duration::from_secs(30);
/// `key_frame_interval` when left out.
pub const DEFAULT_MEDIA_KEY_FRAME_INTERVAL: Duration = Duration::from_millis(500);
/// `turn_credential_ttl` when left out.
pub const DEFAULT_MEDIA_TURN_CREDENTIAL_TTL: Duration = Duration::from_secs(3600);
/// The codecs `codecs` may name, fit's (fib's default codecs), by encoding name; every one when
/// left out.
pub const MEDIA_CODECS: &[&str] = &["opus", "VP8", "VP9", "H264", "AV1"];

/// `max_sessions` when left out or 0.
pub const DEFAULT_PROXY_MAX_SESSIONS: usize = 4096;
/// `max_relays` when left out or 0.
pub const DEFAULT_PROXY_MAX_RELAYS: usize = 4096;
/// `session_timeout` when left out.
pub const DEFAULT_PROXY_SESSION_TIMEOUT: Duration = Duration::from_secs(30);
/// `idle_timeout` when left out.
pub const DEFAULT_PROXY_IDLE_TIMEOUT: Duration = Duration::from_secs(30);

/// The `webrtc_proxy` handler's `timeout` when left out.
pub const DEFAULT_WEBRTC_PROXY_TIMEOUT: Duration = Duration::from_secs(10);

// ---------------------------------------------------------------------------------------------
// The model.

/// `webrtc`: what akit runs for WebRTC beside the gateway, on its engine.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct WebRtc {
    /// `turn`: a TURN server (RFC 8656); none when left out.
    pub turn: Option<TurnServer>,
    /// `media`: the UDP port of the `whip` and `whep` handlers' PeerConnections; none when left
    /// out, and those handlers answer 503.
    pub media: Option<WebRtcMedia>,
    /// `proxy`: the UDP port of the `webrtc_proxy` handler's relays; none when left out, and that
    /// handler answers 503.
    pub proxy: Option<WebRtcProxy>,
}

/// `webrtc.turn`: a TURN server, where it listens, whom it lets in, where it relays from, and
/// its limits.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct TurnServer {
    /// `listen`: the UDP addresses clients reach the server at; at least one.
    pub listen: Vec<String>,
    /// `listen_tcp`: TCP addresses clients reach the server at.
    pub listen_tcp: Vec<String>,
    /// `listen_tls`: TLS addresses clients reach the server at, with `tls`'s certificates.
    pub listen_tls: Vec<String>,
    /// `mptcp`: Multipath TCP on `listen_tcp` and `listen_tls` where the kernel offers it.
    pub mptcp: bool,
    /// `tls`: the certificates of `listen_tls`, from files.
    pub tls: Option<TurnTls>,
    /// `access`: the client addresses the server serves, before the global access.
    pub access: Option<Access>,
    /// `realm`: the realm of the credentials.
    pub realm: String,
    /// `users`: the users with fixed credentials.
    pub users: Vec<TurnUser>,
    /// `secret`: the shared secret of time-limited credentials, as coturn's
    /// `static-auth-secret`; empty for none.
    pub secret: String,
    /// `relay_ip`: the address the relayed sockets bind to, one of the host's, not a wildcard.
    pub relay_ip: Option<IpAddr>,
    /// `public_ip`: the address clients are told for their relayed sockets instead.
    pub public_ip: Option<IpAddr>,
    /// `port_min`: the lowest port of the relayed sockets; 0 with `port_max` 0 for the kernel's.
    pub port_min: u16,
    /// `port_max`: the highest port of the relayed sockets.
    pub port_max: u16,
    /// `max_allocations`: 0 for the default ([`TurnServer::max_allocs`]).
    pub max_allocations: usize,
    /// `max_allocations_per_user`: 0 for the default.
    pub max_allocations_per_user: usize,
    /// `max_permissions`: 0 for the default.
    pub max_permissions: usize,
    /// `max_channels`: 0 for the default.
    pub max_channels: usize,
    /// `max_stream_message`: 0 for the default, at least [`MIN_TURN_STREAM_MESSAGE`] otherwise.
    pub max_stream_message: usize,
    /// `stream_idle_timeout`: zero for the default.
    pub stream_idle_timeout: Duration,
    /// `user_bandwidth`: bytes a second a username each way; 0 for the default.
    pub user_bandwidth: u64,
    /// `default_lifetime`: zero for the default.
    pub default_lifetime: Duration,
    /// `max_lifetime`: zero for the default.
    pub max_lifetime: Duration,
    /// `nonce_lifetime`: zero for the default.
    pub nonce_lifetime: Duration,
    /// `nonce_secret`: the key nonces are made with; empty for a random one.
    pub nonce_secret: String,
    /// `allowed_peers`: the peers allowed even when denied otherwise.
    pub allowed_peers: Vec<PeerRule>,
    /// `denied_peers`: the peers denied beyond the internal ranges denied by default.
    pub denied_peers: Vec<PeerRule>,
}

/// `webrtc.turn.tls`: the certificates of `listen_tls`, from files.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct TurnTls {
    /// `certificates`: a client is given the first whose names match the one it asks for, or
    /// the first of all.
    pub certificates: Vec<Certificate>,
}

/// One of `webrtc.turn.users`.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct TurnUser {
    /// `name`: the username.
    pub name: String,
    /// `password`: the password; empty with `key`.
    pub password: String,
    /// `key`: the long-term key the password derives, `MD5(name ":" realm ":" password)` in
    /// hex, optionally after `0x`; empty with `password`.
    pub key: String,
}

impl TurnUser {
    /// The key decoded: 16 bytes, or `None` without one or with one that is not 32 hex digits.
    #[must_use]
    pub fn key_bytes(&self) -> Option<[u8; 16]> {
        let hex = self
            .key
            .strip_prefix("0x")
            .or_else(|| self.key.strip_prefix("0X"))
            .unwrap_or(&self.key);
        if hex.len() != 32 {
            return None;
        }
        let mut out = [0u8; 16];
        for (i, o) in out.iter_mut().enumerate() {
            let pair = hex.get(i.checked_mul(2)?..i.checked_mul(2)?.checked_add(2)?)?;
            *o = u8::from_str_radix(pair, 16).ok()?;
        }
        Some(out)
    }
}

/// One of `allowed_peers` or `denied_peers`: a prefix, and a port when the entry names one.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PeerRule {
    /// The prefix's address, masked to its length, IPv4-mapped addresses unmapped.
    pub addr: IpAddr,
    /// The prefix's length in bits.
    pub bits: u8,
    /// The entry's port, 0 for every port.
    pub port: u16,
}

impl PeerRule {
    /// Parses an entry: a prefix (`10.1.0.0/16`), an address, which is a prefix of its whole
    /// length, or an address and a port (`10.0.0.5:8189`, `[fd00::1]:9`), as fit's
    /// `ParsePeerRule`.
    #[must_use]
    pub fn parse(s: &str) -> Option<PeerRule> {
        if let Some((a, bits)) = s.split_once('/') {
            if a.contains('%') || bits.is_empty() || !bits.bytes().all(|b| b.is_ascii_digit()) {
                return None;
            }
            let ip: IpAddr = a.parse().ok()?;
            let bits: u8 = bits.parse().ok()?;
            let max = if ip.is_ipv4() { 32 } else { 128 };
            if bits > max {
                return None;
            }
            // fit's netip.ParsePrefix keeps a prefix's family: a mapped address stays IPv6.
            return Some(PeerRule {
                addr: mask(ip, bits),
                bits,
                port: 0,
            });
        }
        if let Ok(ip) = s.parse::<IpAddr>() {
            let ip = crate::model::unmap(ip);
            return Some(PeerRule {
                addr: ip,
                bits: full(ip),
                port: 0,
            });
        }
        let sa: SocketAddr = s.parse().ok()?;
        if sa.port() == 0 {
            return None;
        }
        if let SocketAddr::V6(v6) = sa {
            if v6.scope_id() != 0 {
                return None;
            }
        }
        let ip = crate::model::unmap(sa.ip());
        Some(PeerRule {
            addr: ip,
            bits: full(ip),
            port: sa.port(),
        })
    }

    /// Whether the rule's prefix holds `ip` (IPv4-mapped addresses unmapped first).
    #[must_use]
    pub fn contains(&self, ip: IpAddr) -> bool {
        let ip = crate::model::unmap(ip);
        ip.is_ipv4() == self.addr.is_ipv4() && mask(ip, self.bits) == self.addr
    }
}

fn full(ip: IpAddr) -> u8 {
    if ip.is_ipv4() { 32 } else { 128 }
}

/// `ip` with all but its first `bits` bits cleared.
fn mask(ip: IpAddr, bits: u8) -> IpAddr {
    match ip {
        IpAddr::V4(v4) => {
            let m = u32::MAX
                .checked_shl(32u32.saturating_sub(u32::from(bits)))
                .unwrap_or(0);
            IpAddr::V4((u32::from(v4) & m).into())
        }
        IpAddr::V6(v6) => {
            let m = u128::MAX
                .checked_shl(128u32.saturating_sub(u32::from(bits)))
                .unwrap_or(0);
            IpAddr::V6((u128::from(v6) & m).into())
        }
    }
}

fn or(v: usize, default: usize) -> usize {
    if v == 0 { default } else { v }
}

fn dur_or(v: Duration, default: Duration) -> Duration {
    if v.is_zero() { default } else { v }
}

impl TurnServer {
    /// `max_allocations`, or its default.
    #[must_use]
    pub fn max_allocs(&self) -> usize {
        or(self.max_allocations, DEFAULT_TURN_MAX_ALLOCATIONS)
    }

    /// `max_allocations_per_user`, or its default.
    #[must_use]
    pub fn max_allocs_per_user(&self) -> usize {
        or(
            self.max_allocations_per_user,
            DEFAULT_TURN_MAX_ALLOCATIONS_PER_USER,
        )
    }

    /// `max_permissions`, or its default.
    #[must_use]
    pub fn max_perms(&self) -> usize {
        or(self.max_permissions, DEFAULT_TURN_MAX_PERMISSIONS)
    }

    /// `max_channels`, or its default.
    #[must_use]
    pub fn max_chans(&self) -> usize {
        or(self.max_channels, DEFAULT_TURN_MAX_CHANNELS)
    }

    /// `max_stream_message`, or its default.
    #[must_use]
    pub fn stream_message(&self) -> usize {
        or(self.max_stream_message, DEFAULT_TURN_MAX_STREAM_MESSAGE)
    }

    /// `stream_idle_timeout`, or its default.
    #[must_use]
    pub fn stream_idle(&self) -> Duration {
        dur_or(self.stream_idle_timeout, DEFAULT_TURN_STREAM_IDLE_TIMEOUT)
    }

    /// `user_bandwidth`, or its default.
    #[must_use]
    pub fn bandwidth(&self) -> u64 {
        if self.user_bandwidth == 0 {
            DEFAULT_TURN_USER_BANDWIDTH
        } else {
            self.user_bandwidth
        }
    }

    /// `default_lifetime`, or its default.
    #[must_use]
    pub fn lifetime(&self) -> Duration {
        dur_or(self.default_lifetime, DEFAULT_TURN_LIFETIME)
    }

    /// `max_lifetime`, or its default.
    #[must_use]
    pub fn longest_lifetime(&self) -> Duration {
        dur_or(self.max_lifetime, DEFAULT_TURN_MAX_LIFETIME)
    }

    /// `nonce_lifetime`, or its default.
    #[must_use]
    pub fn nonce_life(&self) -> Duration {
        dur_or(self.nonce_lifetime, DEFAULT_TURN_NONCE_LIFETIME)
    }
}

/// `webrtc.media`: the media side of akit's WHIP and WHEP.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct WebRtcMedia {
    /// `listen`: the UDP address every peer's media reaches akit at.
    pub listen: String,
    /// `public_ips`: the addresses peers are told to reach the port at, instead of the
    /// interfaces' own.
    pub public_ips: Vec<IpAddr>,
    /// `access`: the peer addresses the port serves, before the global access.
    pub access: Option<Access>,
    /// `ice_servers`: STUN and TURN servers told to the peers in `Link` headers.
    pub ice_servers: Vec<IceServer>,
    /// `turn_urls`: URLs of `webrtc.turn` as peers reach it, told with time-limited credentials.
    pub turn_urls: Vec<String>,
    /// `turn_credential_ttl`: zero for the default.
    pub turn_credential_ttl: Duration,
    /// `codecs`: the codecs publishers may send, by encoding name as written; every one of
    /// [`MEDIA_CODECS`] when left out.
    pub codecs: Vec<String>,
    /// `max_peer_connections`: 0 for the default.
    pub max_peer_connections: usize,
    /// `max_streams`: 0 for the default.
    pub max_streams: usize,
    /// `max_viewers`: 0 for the default.
    pub max_viewers: usize,
    /// `max_bitrate`: bits a second a publisher may send; 0 for the default.
    pub max_bitrate: u64,
    /// `max_sdp_bytes`: 0 for the default.
    pub max_sdp_bytes: usize,
    /// `connect_timeout`: zero for the default.
    pub connect_timeout: Duration,
    /// `ice_timeout`: zero for the default.
    pub ice_timeout: Duration,
    /// `key_frame_interval`: zero for the default.
    pub key_frame_interval: Duration,
}

impl WebRtcMedia {
    /// `max_peer_connections`, or its default.
    #[must_use]
    pub fn max_pcs(&self) -> usize {
        or(
            self.max_peer_connections,
            DEFAULT_MEDIA_MAX_PEER_CONNECTIONS,
        )
    }

    /// `max_streams`, or its default.
    #[must_use]
    pub fn max_stream_count(&self) -> usize {
        or(self.max_streams, DEFAULT_MEDIA_MAX_STREAMS)
    }

    /// `max_viewers`, or its default.
    #[must_use]
    pub fn max_viewer_count(&self) -> usize {
        or(self.max_viewers, DEFAULT_MEDIA_MAX_VIEWERS)
    }

    /// `max_bitrate`, or its default.
    #[must_use]
    pub fn bitrate(&self) -> u64 {
        if self.max_bitrate == 0 {
            DEFAULT_MEDIA_MAX_BITRATE
        } else {
            self.max_bitrate
        }
    }

    /// `max_sdp_bytes`, or its default.
    #[must_use]
    pub fn sdp_bytes(&self) -> usize {
        or(self.max_sdp_bytes, DEFAULT_MEDIA_MAX_SDP_BYTES)
    }

    /// `connect_timeout`, or its default.
    #[must_use]
    pub fn connect(&self) -> Duration {
        dur_or(self.connect_timeout, DEFAULT_MEDIA_CONNECT_TIMEOUT)
    }

    /// `ice_timeout`, or its default.
    #[must_use]
    pub fn ice(&self) -> Duration {
        dur_or(self.ice_timeout, DEFAULT_MEDIA_ICE_TIMEOUT)
    }

    /// `key_frame_interval`, or its default.
    #[must_use]
    pub fn key_frames(&self) -> Duration {
        dur_or(self.key_frame_interval, DEFAULT_MEDIA_KEY_FRAME_INTERVAL)
    }

    /// `turn_credential_ttl`, or its default.
    #[must_use]
    pub fn credential_ttl(&self) -> Duration {
        dur_or(self.turn_credential_ttl, DEFAULT_MEDIA_TURN_CREDENTIAL_TTL)
    }
}

/// A STUN or TURN server, as `RTCIceServer` has it.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct IceServer {
    /// `urls`: its `stun:`, `stuns:`, `turn:` or `turns:` URLs.
    pub urls: Vec<String>,
    /// `username`: a TURN server's username.
    pub username: String,
    /// `credential`: a TURN server's password.
    pub credential: String,
}

/// `webrtc.proxy`: the media side of the `webrtc_proxy` handler.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct WebRtcProxy {
    /// `listen`: the UDP address peers reach the media servers behind akit at.
    pub listen: String,
    /// `public_ips`: the addresses peers are told to reach the port at.
    pub public_ips: Vec<IpAddr>,
    /// `access`: the peer addresses the port relays, before the global access.
    pub access: Option<Access>,
    /// `max_sessions`: 0 for the default.
    pub max_sessions: usize,
    /// `max_relays`: 0 for the default.
    pub max_relays: usize,
    /// `session_timeout`: zero for the default.
    pub session_timeout: Duration,
    /// `idle_timeout`: zero for the default.
    pub idle_timeout: Duration,
    /// `relay_ip`: the address the sockets toward the media servers leave from; the route's
    /// pick when left out.
    pub relay_ip: Option<IpAddr>,
    /// `port_min`: the lowest local port of the sockets toward the media servers.
    pub port_min: u16,
    /// `port_max`: the highest.
    pub port_max: u16,
}

impl WebRtcProxy {
    /// `max_sessions`, or its default.
    #[must_use]
    pub fn sessions(&self) -> usize {
        or(self.max_sessions, DEFAULT_PROXY_MAX_SESSIONS)
    }

    /// `max_relays`, or its default.
    #[must_use]
    pub fn relays(&self) -> usize {
        or(self.max_relays, DEFAULT_PROXY_MAX_RELAYS)
    }

    /// `session_timeout`, or its default.
    #[must_use]
    pub fn session_idle(&self) -> Duration {
        dur_or(self.session_timeout, DEFAULT_PROXY_SESSION_TIMEOUT)
    }

    /// `idle_timeout`, or its default.
    #[must_use]
    pub fn idle(&self) -> Duration {
        dur_or(self.idle_timeout, DEFAULT_PROXY_IDLE_TIMEOUT)
    }
}

/// The params of a `whip` or `whep` handler.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct WhipWhep {
    /// `stream`: the stream's name; empty for the last segment of the POST's path.
    pub stream: String,
}

/// What a `webrtc_proxy` handler's offers ask of its `auth`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MediaAction {
    /// `publish`: a WHIP endpoint.
    Publish,
    /// `play`: a WHEP endpoint.
    Play,
}

/// The params of a `webrtc_proxy` handler.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct WebRtcProxyHandler {
    /// `upstream`: the upstream of the media servers.
    pub upstream: String,
    /// `media_addrs`: where a media server's peers are relayed to, by its address in the
    /// upstream: an address and port, or an IP alone (port 0, the answer's candidate's).
    pub media_addrs: Vec<(String, SocketAddr)>,
    /// `timeout`: zero for the default.
    pub timeout: Duration,
    /// `max_sdp_bytes`: 0 for the default.
    pub max_sdp_bytes: usize,
    /// `action`: what an offer asks of `auth`.
    pub action: Option<MediaAction>,
}

impl WebRtcProxyHandler {
    /// `timeout`, or its default.
    #[must_use]
    pub fn timeout(&self) -> Duration {
        dur_or(self.timeout, DEFAULT_WEBRTC_PROXY_TIMEOUT)
    }

    /// `max_sdp_bytes`, or its default.
    #[must_use]
    pub fn sdp_bytes(&self) -> usize {
        or(self.max_sdp_bytes, DEFAULT_MEDIA_MAX_SDP_BYTES)
    }
}

/// Whether `s` is a stream's name: 1 to 128 letters, digits, `.`, `_`, `~` and `-`, what a path
/// segment holds without escapes.
#[must_use]
pub fn stream_name(s: &str) -> bool {
    (1..=128).contains(&s.len())
        && s.bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'_' | b'~' | b'-'))
}

/// Whether `u` is a `stun:`, `stuns:`, `turn:` or `turns:` URL with a host, as RFC 7064 and RFC
/// 7065 write them, which a `Link` header can carry.
#[must_use]
pub fn ice_url(u: &str) -> bool {
    let Some((scheme, rest)) = u.split_once(':') else {
        return false;
    };
    matches!(
        scheme.to_ascii_lowercase().as_str(),
        "stun" | "stuns" | "turn" | "turns"
    ) && !rest.is_empty()
        && !rest.starts_with('/')
        && !rest.contains(['<', '>', ' ', '\t', '\r', '\n', '"'])
}

/// Whether `s` can be a quoted parameter of a `Link` header: no double quote, backslash or
/// control character.
#[must_use]
pub fn link_safe(s: &str) -> bool {
    !s.chars()
        .any(|c| c == '"' || c == '\\' || c < '\u{20}' || c == '\u{7f}')
}

// ---------------------------------------------------------------------------------------------
// What akit reads, object by object.

const WEBRTC_FIELDS: &[&str] = &["turn", "media", "proxy"];
const TURN_FIELDS: &[&str] = &[
    "listen",
    "listen_tcp",
    "listen_tls",
    "mptcp",
    "tls",
    "access",
    "realm",
    "users",
    "secret",
    "relay_ip",
    "public_ip",
    "port_min",
    "port_max",
    "max_allocations",
    "max_allocations_per_user",
    "max_permissions",
    "max_channels",
    "max_stream_message",
    "stream_idle_timeout",
    "user_bandwidth",
    "default_lifetime",
    "max_lifetime",
    "nonce_lifetime",
    "nonce_secret",
    "allowed_peers",
    "denied_peers",
];
const TURN_TLS_FIELDS: &[&str] = &["certificates"];
const TURN_USER_FIELDS: &[&str] = &["name", "password", "key"];
const MEDIA_FIELDS: &[&str] = &[
    "listen",
    "public_ips",
    "access",
    "ice_servers",
    "turn_urls",
    "turn_credential_ttl",
    "codecs",
    "max_peer_connections",
    "max_streams",
    "max_viewers",
    "max_bitrate",
    "max_sdp_bytes",
    "connect_timeout",
    "ice_timeout",
    "key_frame_interval",
];
const ICE_SERVER_FIELDS: &[&str] = &["urls", "username", "credential"];
const WEBRTC_PROXY_ROLE_FIELDS: &[&str] = &[
    "listen",
    "public_ips",
    "access",
    "max_sessions",
    "max_relays",
    "session_timeout",
    "idle_timeout",
    "relay_ip",
    "port_min",
    "port_max",
];
/// The params of `whip` and `whep` akit reads; `auth` waits for `media_auth`.
pub(crate) const WHIP_FIELDS: &[&str] = &["stream"];
pub(crate) const WHIP_FIT_ONLY: &[&str] = &["auth"];
/// The params of `webrtc_proxy` akit reads; `auth` waits for `media_auth`.
pub(crate) const WEBRTC_PROXY_FIELDS: &[&str] = &[
    "upstream",
    "media_addrs",
    "timeout",
    "max_sdp_bytes",
    "action",
];
pub(crate) const WEBRTC_PROXY_FIT_ONLY: &[&str] = &["auth"];

// ---------------------------------------------------------------------------------------------
// The decoders.

fn text(d: &mut Decoder<'_>, o: &mut Obj<'_>, key: &str) -> String {
    o.take(key)
        .and_then(|v| d.string(v, &o.at(key)))
        .unwrap_or_default()
}

fn duration(d: &mut Decoder<'_>, o: &mut Obj<'_>, key: &str) -> Duration {
    o.take(key)
        .and_then(|v| d.duration(v, &o.at(key)))
        .unwrap_or_default()
}

fn count(d: &mut Decoder<'_>, o: &mut Obj<'_>, key: &str) -> usize {
    o.take(key)
        .and_then(|v| d.int(v, &o.at(key), 0, i64::MAX))
        .and_then(|n| usize::try_from(n).ok())
        .unwrap_or(0)
}

fn port(d: &mut Decoder<'_>, o: &mut Obj<'_>, key: &str) -> u16 {
    o.take(key)
        .and_then(|v| d.int(v, &o.at(key), 0, 65535))
        .and_then(|n| u16::try_from(n).ok())
        .unwrap_or(0)
}

fn boolean(d: &mut Decoder<'_>, o: &mut Obj<'_>, key: &str) -> bool {
    let Some(v) = o.take(key) else {
        return false;
    };
    match &v.kind {
        Kind::Bool(b) => *b,
        other => {
            d.error(
                v.off,
                &o.at(key),
                format!("expected a boolean, got {}", other.name()),
            );
            false
        }
    }
}

/// A list of strings, each with its position and path.
fn strings(d: &mut Decoder<'_>, o: &mut Obj<'_>, key: &str) -> Vec<(String, usize, String)> {
    let Some(v) = o.take(key) else {
        return Vec::new();
    };
    let p = o.at(key);
    d.strings(v, &p)
        .into_iter()
        .enumerate()
        .map(|(i, (off, s))| (s, off, join_index(&p, i)))
        .collect()
}

/// The addresses of a listen list, each checked and recorded for the bindings' check.
fn addresses(d: &mut Decoder<'_>, o: &mut Obj<'_>, key: &str) -> Vec<String> {
    strings(d, o, key)
        .into_iter()
        .map(|(a, off, p)| {
            if let Err(msg) = crate::model::check_address(&a) {
                d.error(off, &p, msg);
            }
            d.address_at(&p, off);
            a
        })
        .collect()
}

/// An IP address in a field that takes one: not a wildcard; `None` (after an error) otherwise.
fn ip_field(
    d: &mut Decoder<'_>,
    s: &str,
    off: usize,
    path: &str,
    wildcard: &str,
) -> Option<IpAddr> {
    // A zone (`fe80::1%eth0`) is not an address here, as fit's `netip.ParseAddr` with no zone.
    match s.parse::<IpAddr>() {
        Ok(ip) if ip.is_unspecified() => {
            d.error(off, path, format!("{s:?} is a wildcard{wildcard}"));
            None
        }
        Ok(ip) => Some(ip),
        Err(_) => {
            d.error(off, path, format!("{s:?} is not an IP address"));
            None
        }
    }
}

/// `public_ips`: addresses, none a wildcard.
fn public_ips(d: &mut Decoder<'_>, o: &mut Obj<'_>) -> Vec<IpAddr> {
    strings(d, o, "public_ips")
        .into_iter()
        .filter_map(|(s, off, p)| ip_field(d, &s, off, &p, ""))
        .collect()
}

/// `port_min` and `port_max`, set together and in order.
fn port_range(d: &mut Decoder<'_>, o: &mut Obj<'_>) -> (u16, u16) {
    let min_off = o.key_off("port_min");
    let min = port(d, o, "port_min");
    let max = port(d, o, "port_max");
    if (min == 0) != (max == 0) {
        d.error(o.off, &o.path, "set port_min and port_max together");
    } else if min > max {
        d.error(
            min_off.unwrap_or(o.off),
            &o.at("port_min"),
            format!("port_min {min} is above port_max {max}"),
        );
    }
    (min, max)
}

fn access_of(d: &mut Decoder<'_>, o: &mut Obj<'_>) -> Option<Access> {
    let v = o.take("access")?;
    crate::model::access(d, v, &o.at("access"))
}

/// Decodes `webrtc`.
pub(crate) fn webrtc(d: &mut Decoder<'_>, n: &Node, path: &str) -> Option<WebRtc> {
    let mut o = d.object(n, path)?;
    let media = o
        .take("media")
        .and_then(|v| media(d, v, &join_field(path, "media")));
    let proxy = o
        .take("proxy")
        .and_then(|v| proxy(d, v, &join_field(path, "proxy")));
    let turn_at = o.take("turn").map(|v| (v, join_field(path, "turn")));
    let turn = turn_at.and_then(|(v, p)| turn(d, v, &p));
    o.finish(d, WEBRTC_FIELDS, &[]);
    // The media server makes TURN credentials from webrtc.turn's secret.
    if let Some(m) = &media {
        if !m.turn_urls.is_empty() && turn.as_ref().is_none_or(|t| t.secret.is_empty()) {
            let at = join_field(&join_field(path, "media"), "turn_urls");
            d.error(
                n.off,
                &at,
                "akit makes their credentials from webrtc.turn's secret: set webrtc.turn with a \
                 secret",
            );
        }
    }
    Some(WebRtc { turn, media, proxy })
}

fn turn(d: &mut Decoder<'_>, n: &Node, path: &str) -> Option<TurnServer> {
    let mut o = d.object(n, path)?;
    let mut t = TurnServer::default();
    let listen_node = o.require(d, "listen");
    if let Some(v) = listen_node {
        let p = o.at("listen");
        t.listen = d
            .strings(v, &p)
            .into_iter()
            .enumerate()
            .map(|(i, (off, a))| {
                let ip = join_index(&p, i);
                if let Err(msg) = crate::model::check_address(&a) {
                    d.error(off, &ip, msg);
                }
                d.address_at(&ip, off);
                a
            })
            .collect();
        if t.listen.is_empty() {
            d.error(v.off, &p, "at least one address is needed");
        }
    }
    t.listen_tcp = addresses(d, &mut o, "listen_tcp");
    t.listen_tls = addresses(d, &mut o, "listen_tls");
    t.mptcp = boolean(d, &mut o, "mptcp");
    let tls_off = o.key_off("tls");
    t.tls = o.take("tls").and_then(|v| turn_tls(d, v, &o.at("tls")));
    match (t.listen_tls.is_empty(), &t.tls) {
        (false, None) => d.error(
            o.off,
            &o.at("tls"),
            "listen_tls needs certificates: set tls.certificates",
        ),
        (false, Some(tls)) if tls.certificates.is_empty() => d.error(
            tls_off.unwrap_or(o.off),
            &o.at("tls"),
            "listen_tls needs certificates: set tls.certificates",
        ),
        (true, Some(_)) => d.error(
            tls_off.unwrap_or(o.off),
            &o.at("tls"),
            "tls is for listen_tls, which is empty",
        ),
        _ => {}
    }
    t.access = access_of(d, &mut o);
    if let Some(v) = o.require(d, "realm") {
        t.realm = d.string(v, &o.at("realm")).unwrap_or_default();
        if t.realm.trim().is_empty() {
            d.error(v.off, &o.at("realm"), "the realm is empty");
        }
    }
    if let Some(v) = o.take("users") {
        let p = o.at("users");
        t.users = d.array(v, &p, turn_user);
        let mut seen: Vec<&str> = Vec::new();
        if let Kind::Array(items) = &v.kind {
            for (i, (u, item)) in t.users.iter().zip(items).enumerate() {
                let at = join_index(&p, i);
                if u.name.is_empty() {
                    d.error(item.off, &join_field(&at, "name"), "the name is empty");
                } else if seen.contains(&u.name.as_str()) {
                    d.error(
                        item.off,
                        &join_field(&at, "name"),
                        format!("another user is already named {:?}", u.name),
                    );
                }
                seen.push(&u.name);
                if u.password.is_empty() == u.key.is_empty() {
                    d.error(item.off, &at, "set password or key, one of the two");
                } else if !u.key.is_empty() && u.key_bytes().is_none() {
                    d.error(
                        item.off,
                        &join_field(&at, "key"),
                        "the key is not 32 hex digits",
                    );
                }
            }
        }
    }
    t.secret = text(d, &mut o, "secret");
    if t.users.is_empty() && t.secret.is_empty() {
        d.error(
            o.off,
            &o.path,
            "no one could log in: set users, secret or both",
        );
    }
    if let Some(v) = o.require(d, "relay_ip") {
        let p = o.at("relay_ip");
        if let Some(s) = d.string(v, &p) {
            t.relay_ip = ip_field(
                d,
                &s,
                v.off,
                &p,
                ": name one of the host's addresses, which clients are told",
            );
        }
    }
    if let Some(v) = o.take("public_ip") {
        let p = o.at("public_ip");
        if let Some(s) = d.string(v, &p) {
            t.public_ip = ip_field(d, &s, v.off, &p, "");
            if let (Some(pub_ip), Some(relay)) = (t.public_ip, t.relay_ip) {
                if crate::model::unmap(pub_ip).is_ipv4() != crate::model::unmap(relay).is_ipv4() {
                    d.error(v.off, &p, format!("{s} is not of relay_ip's family"));
                }
            }
        }
    }
    (t.port_min, t.port_max) = port_range(d, &mut o);
    t.max_allocations = count(d, &mut o, "max_allocations");
    t.max_allocations_per_user = count(d, &mut o, "max_allocations_per_user");
    t.max_permissions = count(d, &mut o, "max_permissions");
    t.max_channels = count(d, &mut o, "max_channels");
    let msg_off = o.key_off("max_stream_message");
    t.max_stream_message = count(d, &mut o, "max_stream_message");
    if t.max_stream_message != 0 && t.max_stream_message < MIN_TURN_STREAM_MESSAGE {
        d.error(
            msg_off.unwrap_or(o.off),
            &o.at("max_stream_message"),
            format!(
                "max_stream_message {} is below {MIN_TURN_STREAM_MESSAGE}, too small for a \
                 datagram",
                t.max_stream_message
            ),
        );
    }
    t.stream_idle_timeout = duration(d, &mut o, "stream_idle_timeout");
    t.user_bandwidth = o
        .take("user_bandwidth")
        .and_then(|v| d.int(v, &o.at("user_bandwidth"), 0, i64::MAX))
        .and_then(|n| u64::try_from(n).ok())
        .unwrap_or(0);
    t.default_lifetime = duration(d, &mut o, "default_lifetime");
    let max_off = o.key_off("max_lifetime");
    t.max_lifetime = duration(d, &mut o, "max_lifetime");
    if !t.max_lifetime.is_zero() && t.max_lifetime < t.lifetime() {
        d.error(
            max_off.unwrap_or(o.off),
            &o.at("max_lifetime"),
            format!(
                "max_lifetime {} is below default_lifetime",
                crate::duration::format(t.max_lifetime)
            ),
        );
    }
    t.nonce_lifetime = duration(d, &mut o, "nonce_lifetime");
    t.nonce_secret = text(d, &mut o, "nonce_secret");
    for (key, into) in [
        ("allowed_peers", &mut t.allowed_peers),
        ("denied_peers", &mut t.denied_peers),
    ] {
        for (s, off, p) in strings(d, &mut o, key) {
            match PeerRule::parse(&s) {
                Some(r) => into.push(r),
                None => d.error(
                    off,
                    &p,
                    format!(
                        "{s:?} is not an address, an address and port, or a prefix, as in \
                         \"10.1.0.0/16\""
                    ),
                ),
            }
        }
    }
    o.finish(d, TURN_FIELDS, &[]);
    Some(t)
}

fn turn_tls(d: &mut Decoder<'_>, n: &Node, path: &str) -> Option<TurnTls> {
    let mut o = d.object(n, path)?;
    let certificates = o
        .require(d, "certificates")
        .map(|v| d.array(v, &o.at("certificates"), crate::model::certificate))
        .unwrap_or_default();
    o.finish(d, TURN_TLS_FIELDS, &[]);
    Some(TurnTls { certificates })
}

fn turn_user(d: &mut Decoder<'_>, n: &Node, path: &str) -> Option<TurnUser> {
    let mut o = d.object(n, path)?;
    let name = match o.require(d, "name") {
        Some(v) => d.string(v, &o.at("name")).unwrap_or_default(),
        None => String::new(),
    };
    let password = text(d, &mut o, "password");
    let key = text(d, &mut o, "key");
    o.finish(d, TURN_USER_FIELDS, &[]);
    Some(TurnUser {
        name,
        password,
        key,
    })
}

/// A `listen` that is required, checked and recorded for the bindings' check.
fn listen(d: &mut Decoder<'_>, o: &mut Obj<'_>) -> String {
    let Some(v) = o.require(d, "listen") else {
        return String::new();
    };
    let p = o.at("listen");
    let Some(a) = d.string(v, &p) else {
        return String::new();
    };
    if let Err(msg) = crate::model::check_address(&a) {
        d.error(v.off, &p, msg);
    }
    d.address_at(&p, v.off);
    a
}

fn media(d: &mut Decoder<'_>, n: &Node, path: &str) -> Option<WebRtcMedia> {
    let mut o = d.object(n, path)?;
    let mut m = WebRtcMedia {
        listen: listen(d, &mut o),
        public_ips: public_ips(d, &mut o),
        access: access_of(d, &mut o),
        ..WebRtcMedia::default()
    };
    if let Some(v) = o.take("ice_servers") {
        m.ice_servers = d.array(v, &o.at("ice_servers"), ice_server);
    }
    for (u, off, p) in strings(d, &mut o, "turn_urls") {
        if !ice_url(&u) || !u.to_ascii_lowercase().starts_with("turn") {
            d.error(off, &p, format!("{u:?} is not a turn: or turns: URL"));
        }
        m.turn_urls.push(u);
    }
    m.turn_credential_ttl = duration(d, &mut o, "turn_credential_ttl");
    for (c, off, p) in strings(d, &mut o, "codecs") {
        if !MEDIA_CODECS.iter().any(|k| k.eq_ignore_ascii_case(&c)) {
            d.error(
                off,
                &p,
                format!("{c:?} is not one of {}", MEDIA_CODECS.join(", ")),
            );
        }
        m.codecs.push(c);
    }
    m.max_peer_connections = count(d, &mut o, "max_peer_connections");
    m.max_streams = count(d, &mut o, "max_streams");
    m.max_viewers = count(d, &mut o, "max_viewers");
    m.max_bitrate = o
        .take("max_bitrate")
        .and_then(|v| d.int(v, &o.at("max_bitrate"), 0, i64::MAX))
        .and_then(|n| u64::try_from(n).ok())
        .unwrap_or(0);
    m.max_sdp_bytes = count(d, &mut o, "max_sdp_bytes");
    m.connect_timeout = duration(d, &mut o, "connect_timeout");
    m.ice_timeout = duration(d, &mut o, "ice_timeout");
    m.key_frame_interval = duration(d, &mut o, "key_frame_interval");
    o.finish(d, MEDIA_FIELDS, &[]);
    Some(m)
}

fn ice_server(d: &mut Decoder<'_>, n: &Node, path: &str) -> Option<IceServer> {
    let mut o = d.object(n, path)?;
    let mut s = IceServer::default();
    if let Some(v) = o.require(d, "urls") {
        let p = o.at("urls");
        let urls = d.strings(v, &p);
        if urls.is_empty() {
            d.error(v.off, &p, "at least one URL is needed");
        }
        for (i, (off, u)) in urls.into_iter().enumerate() {
            if !ice_url(&u) {
                d.error(
                    off,
                    &join_index(&p, i),
                    format!("{u:?} is not a stun:, stuns:, turn: or turns: URL"),
                );
            }
            s.urls.push(u);
        }
    }
    s.username = text(d, &mut o, "username");
    s.credential = text(d, &mut o, "credential");
    if !link_safe(&s.username) || !link_safe(&s.credential) {
        d.error(
            o.off,
            path,
            "the username and credential may not hold a double quote, a backslash or a control \
             character",
        );
    }
    o.finish(d, ICE_SERVER_FIELDS, &[]);
    Some(s)
}

fn proxy(d: &mut Decoder<'_>, n: &Node, path: &str) -> Option<WebRtcProxy> {
    let mut o = d.object(n, path)?;
    let mut p = WebRtcProxy {
        listen: listen(d, &mut o),
        public_ips: public_ips(d, &mut o),
        access: access_of(d, &mut o),
        max_sessions: count(d, &mut o, "max_sessions"),
        max_relays: count(d, &mut o, "max_relays"),
        session_timeout: duration(d, &mut o, "session_timeout"),
        idle_timeout: duration(d, &mut o, "idle_timeout"),
        ..WebRtcProxy::default()
    };
    if let Some(v) = o.take("relay_ip") {
        let at = o.at("relay_ip");
        if let Some(s) = d.string(v, &at) {
            p.relay_ip = ip_field(
                d,
                &s,
                v.off,
                &at,
                ": leave relay_ip out for the route's pick",
            );
        }
    }
    (p.port_min, p.port_max) = port_range(d, &mut o);
    o.finish(d, WEBRTC_PROXY_ROLE_FIELDS, &[]);
    Some(p)
}

/// `auth`, the name of a `media_auth` policy, which akit does not read yet (decision 039): refused
/// as not supported yet, naming the module, so that the gate finds the field.
fn auth(d: &mut Decoder<'_>, o: &mut Obj<'_>, module: &str) {
    if let Some(v) = o.take("auth") {
        d.not_yet(
            v.off,
            &o.at("auth"),
            &format!(
                "the {module:?} handler's auth: a media_auth policy, which akit does not read yet"
            ),
        );
    }
}

/// The params of a `whip` or `whep` handler; `module` is its name, for its messages.
pub(crate) fn whip_whep(
    d: &mut Decoder<'_>,
    params: Option<&Node>,
    path: &str,
    module: &str,
) -> Option<WhipWhep> {
    let Some(n) = params else {
        return Some(WhipWhep::default());
    };
    let mut o = d.object(n, path)?;
    auth(d, &mut o, module);
    let mut w = WhipWhep::default();
    if let Some(v) = o.take("stream") {
        let p = o.at("stream");
        if let Some(s) = d.string(v, &p) {
            if !s.is_empty() && !stream_name(&s) {
                d.error(
                    v.off,
                    &p,
                    format!(
                        "{s:?} is not a name of 1 to 128 letters, digits, '.', '_', '~' and '-'"
                    ),
                );
            }
            w.stream = s;
        }
    }
    o.finish(d, WHIP_FIELDS, WHIP_FIT_ONLY);
    Some(w)
}

/// The params of a `webrtc_proxy` handler.
pub(crate) fn webrtc_proxy(
    d: &mut Decoder<'_>,
    params: Option<&Node>,
    path: &str,
    off: usize,
) -> Option<WebRtcProxyHandler> {
    let Some(n) = params else {
        d.error(off, &join_field(path, "upstream"), "missing required field");
        return None;
    };
    let mut o = d.object(n, path)?;
    auth(d, &mut o, "webrtc_proxy");
    let mut h = WebRtcProxyHandler::default();
    if let Some(v) = o.require(d, "upstream") {
        h.upstream = d.string(v, &o.at("upstream")).unwrap_or_default();
    }
    if let Some(v) = o.take("media_addrs") {
        let p = o.at("media_addrs");
        for (koff, k, val) in d.string_map(v, &p) {
            let addr = val.parse::<SocketAddr>().ok().or_else(|| {
                val.parse::<IpAddr>()
                    .ok()
                    .filter(|ip| !ip.is_unspecified())
                    .map(|ip| SocketAddr::new(ip, 0))
            });
            match addr {
                Some(a) => h
                    .media_addrs
                    .push((k, SocketAddr::new(crate::model::unmap(a.ip()), a.port()))),
                None => d.error(
                    koff,
                    &join_key(&p, &k),
                    format!("{val:?} is not an IP address, or an IP and a port"),
                ),
            }
        }
    }
    h.timeout = duration(d, &mut o, "timeout");
    h.max_sdp_bytes = count(d, &mut o, "max_sdp_bytes");
    if let Some(v) = o.take("action") {
        let p = o.at("action");
        match d.string(v, &p).as_deref() {
            Some("publish") => h.action = Some(MediaAction::Publish),
            Some("play") => h.action = Some(MediaAction::Play),
            Some(other) => d.error(
                v.off,
                &p,
                format!("{other:?} is not one of \"publish\", \"play\""),
            ),
            None => {}
        }
    }
    o.finish(d, WEBRTC_PROXY_FIELDS, WEBRTC_PROXY_FIT_ONLY);
    Some(h)
}

#[cfg(test)]
mod tests;
