//! fit's media members of the config (I120): `media_auth`, the named policies of who may publish
//! and play which streams (fit's I115, `config/mediaauth.go`), and `streaming`, the streaming
//! role's hub and its RTMP, RTSP, SRT and WebRTC listeners (fit's I116, `config/streaming.go`),
//! with fit's fields, defaults, bounds and checks.
//!
//! The members are read as strictly as the rest of the config. What is left out is kept empty
//! (a string), zero (a number, a duration) or `None`; the methods give fit's defaults for it.
//! A policy's `keys` and `url_signing` are read since I143 and I144, the role's `channels` since
//! I145 and its `push` since I146 ([`crate::channels`]). What fit reads here and akit does not run
//! yet is refused as not supported yet: Multipath TCP, RTMP's `max_message_bytes` and RTSP's
//! `auth_methods`, `realm` and `idle_timeout` (akio requests A263 and A264).

use std::time::Duration;

use crate::decode::{Decoder, Obj};
use crate::error::{join_field, join_index};
use crate::json::Node;
use crate::model::Access;
use crate::roles::{
    access_of, bool_opt, boolean, check_listen, count, duration, list, required, text, text_at,
};

/// fit's `DefaultMediaTokenTTL`: how long a token akit signs is good for.
pub const DEFAULT_MEDIA_TOKEN_TTL: Duration = Duration::from_secs(12 * 3600);
/// The fewest bytes a `media_auth` secret may have.
pub const MIN_MEDIA_SECRET: usize = 16;
/// The fewest characters a static token may have.
pub const MIN_MEDIA_TOKEN: usize = 16;
/// The longest stream pattern.
pub const MAX_MEDIA_PATTERN: usize = 256;

/// What a `media_auth` policy grants: publishing a stream, and playing it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum MediaAction {
    /// `publish`.
    Publish,
    /// `play`.
    Play,
}

impl MediaAction {
    /// The action's name in the config.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            MediaAction::Publish => "publish",
            MediaAction::Play => "play",
        }
    }

    /// The action named `s`.
    #[must_use]
    pub fn parse(s: &str) -> Option<MediaAction> {
        match s {
            "publish" => Some(MediaAction::Publish),
            "play" => Some(MediaAction::Play),
            _ => None,
        }
    }
}

/// A named `media_auth` policy, as fit's `config.MediaAuth`: who may publish and play which
/// streams, for the streaming role's listeners and the handlers that take media, which name it
/// with their `auth`.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct MediaAuth {
    /// `name`: letters, digits, '_', '.' and '-', starting with a letter or digit.
    pub name: String,
    /// `tokens`: static bearer tokens, each with the streams it may publish and play.
    pub tokens: Vec<MediaToken>,
    /// `users`: users with a password, each with the streams they may publish and play.
    pub users: Vec<MediaUser>,
    /// `secret`: the key, at least 16 bytes, tokens akit signs are signed with; empty for none.
    pub secret: String,
    /// `secret_file`: a file holding the secret, read each time the config is applied; empty for
    /// none.
    pub secret_file: String,
    /// `public`: the actions anyone may take without credentials.
    pub public: Vec<MediaAction>,
    /// `token_ttl`: how long a token akit issues is good for; [`DEFAULT_MEDIA_TOKEN_TTL`] when
    /// left out.
    pub token_ttl: Duration,
    /// `keys` (I143): a keyring of signing keys in place of `secret` or `secret_file`; a token is
    /// signed with one and checked against all.
    pub keys: Vec<MediaKey>,
    /// `url_signing` (I144): playback URLs that expire; off when `None`.
    pub url_signing: Option<UrlSigning>,
}

/// fit's `url_signing` defaults and bounds.
pub const DEFAULT_URL_TTL: Duration = Duration::from_secs(600);
/// See [`DEFAULT_URL_TTL`].
pub const DEFAULT_PLAY_GRACE: Duration = Duration::from_secs(1800);
/// See [`DEFAULT_URL_TTL`].
pub const DEFAULT_SESSION_IDLE: Duration = Duration::from_secs(120);
/// See [`DEFAULT_URL_TTL`].
pub const DEFAULT_TRACKED_SESSIONS: usize = 100_000;
/// The most patterns `referers` and `origins` may list each.
pub const MAX_URL_ORIGINS: usize = 256;

/// A policy's `url_signing`, as fit's `config.URLSigning` (I144): playback URLs signed for one
/// viewer and one resource, checked by `vod`, `hls` and `httpflv`.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct UrlSigning {
    /// `ttl`: [`DEFAULT_URL_TTL`] when zero.
    pub ttl: Duration,
    /// `bind_address`: `subnet` binds a URL to the viewer's /24 or /64, else to its address.
    pub subnet: bool,
    /// `play_grace`: [`DEFAULT_PLAY_GRACE`] when zero.
    pub play_grace: Duration,
    /// `cookie`: hand the grant on as the cookie `akit_media_grant`.
    pub cookie: bool,
    /// `max_sessions`: no cap when 0.
    pub max_sessions: usize,
    /// `on_limit`: `end_oldest`, rather than `refuse`.
    pub end_oldest: bool,
    /// `session_idle`: [`DEFAULT_SESSION_IDLE`] when zero.
    pub session_idle: Duration,
    /// `max_tracked_sessions`: [`DEFAULT_TRACKED_SESSIONS`] when 0.
    pub max_tracked_sessions: usize,
    /// `referers`: origin patterns, lower case; any when empty.
    pub referers: Vec<String>,
    /// `origins`: origin patterns, lower case; any when empty.
    pub origins: Vec<String>,
    /// `trusted_proxies`: addresses and networks whose `X-Forwarded-For` is believed.
    pub trusted_proxies: Vec<String>,
}

impl UrlSigning {
    /// `ttl`, or its default.
    #[must_use]
    pub fn url_ttl(&self) -> Duration {
        or(self.ttl, DEFAULT_URL_TTL)
    }

    /// `play_grace`, or its default.
    #[must_use]
    pub fn grace(&self) -> Duration {
        or(self.play_grace, DEFAULT_PLAY_GRACE)
    }

    /// `session_idle`, or its default.
    #[must_use]
    pub fn idle(&self) -> Duration {
        or(self.session_idle, DEFAULT_SESSION_IDLE)
    }

    /// `max_tracked_sessions`, or its default.
    #[must_use]
    pub fn tracked(&self) -> usize {
        or_count(self.max_tracked_sessions, DEFAULT_TRACKED_SESSIONS)
    }
}

/// Whether `s` is an origin pattern of `referers` and `origins` (fit's `ValidOriginPattern`):
/// `http://` or `https://`, then 1 to 253 of letters, digits, '.', '*', '[', ']', ':' and '-'.
#[must_use]
pub fn valid_origin_pattern(s: &str) -> bool {
    let rest = s
        .strip_prefix("https://")
        .or_else(|| s.strip_prefix("http://"));
    rest.is_some_and(|r| {
        (1..=253).contains(&r.len())
            && r.bytes().all(|b| {
                b.is_ascii_alphanumeric() || matches!(b, b'.' | b'*' | b'[' | b']' | b':' | b'-')
            })
    })
}

/// `s` as an address or a network (fit's `ParseAddrOrPrefix`): the address and its prefix length.
#[must_use]
pub fn addr_or_prefix(s: &str) -> Option<(std::net::IpAddr, u8)> {
    let (a, bits) = match s.split_once('/') {
        Some((a, b)) => (a, Some(b.parse::<u8>().ok()?)),
        None => (s, None),
    };
    let ip: std::net::IpAddr = a.parse().ok()?;
    let max = if ip.is_ipv4() { 32 } else { 128 };
    let bits = bits.unwrap_or(max);
    (bits <= max).then_some((ip, bits))
}

const URL_SIGNING_FIELDS: &[&str] = &[
    "ttl",
    "bind_address",
    "play_grace",
    "cookie",
    "max_sessions",
    "on_limit",
    "session_idle",
    "max_tracked_sessions",
    "referers",
    "origins",
    "trusted_proxies",
];

fn url_signing(d: &mut Decoder<'_>, n: &Node, path: &str) -> Option<UrlSigning> {
    let mut o = d.object(n, path)?;
    let mut u = UrlSigning {
        ttl: duration(d, &mut o, "ttl"),
        ..UrlSigning::default()
    };
    if let Some((v, off)) = text_at(d, &mut o, "bind_address") {
        match v.as_str() {
            "" | "ip" => {}
            "subnet" => u.subnet = true,
            _ => d.error(
                off,
                &o.at("bind_address"),
                format!("{v:?} is not one of \"ip\", \"subnet\""),
            ),
        }
    }
    u.play_grace = duration(d, &mut o, "play_grace");
    u.cookie = boolean(d, &mut o, "cookie");
    u.max_sessions = count(d, &mut o, "max_sessions", i64::MAX);
    if let Some((v, off)) = text_at(d, &mut o, "on_limit") {
        match v.as_str() {
            "" | "refuse" => {}
            "end_oldest" => u.end_oldest = true,
            _ => d.error(
                off,
                &o.at("on_limit"),
                format!("{v:?} is not one of \"refuse\", \"end_oldest\""),
            ),
        }
    }
    u.session_idle = duration(d, &mut o, "session_idle");
    u.max_tracked_sessions = count(d, &mut o, "max_tracked_sessions", i64::MAX);
    for (key, slot) in [("referers", &mut u.referers), ("origins", &mut u.origins)] {
        let Some(v) = o.take(key) else { continue };
        let p = o.at(key);
        let list = d.strings(v, &p);
        if list.len() > MAX_URL_ORIGINS {
            d.error(v.off, &p, format!("at most {MAX_URL_ORIGINS} origins"));
        }
        for (i, (off, s)) in list.into_iter().enumerate() {
            if !valid_origin_pattern(&s) {
                d.error(
                    off,
                    &join_index(&p, i),
                    format!(
                        "{s:?} is not an origin: http:// or https:// and a host, * matching any run of characters, as https://*.example.com"
                    ),
                );
            }
            slot.push(s.to_ascii_lowercase());
        }
    }
    if let Some(v) = o.take("trusted_proxies") {
        let p = o.at("trusted_proxies");
        for (i, (off, s)) in d.strings(v, &p).into_iter().enumerate() {
            if addr_or_prefix(&s).is_none() {
                d.error(
                    off,
                    &join_index(&p, i),
                    format!(
                        "{s:?} is not an IP address or a network, as in \"10.0.0.1\" or \"10.0.0.0/8\""
                    ),
                );
            }
            u.trusted_proxies.push(s);
        }
    }
    o.finish(d, URL_SIGNING_FIELDS, &[]);
    Some(u)
}

/// The most keys a policy's keyring may hold (fit's `MaxMediaKeys`).
pub const MAX_MEDIA_KEYS: usize = 256;

/// One key of a `media_auth` policy's keyring (fit's `config.MediaKey`, I143).
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct MediaKey {
    /// `key`: the key's name, 1 to 64 letters, digits, '_', '.', '~' and '-'.
    pub key: String,
    /// `secret`: the key's secret, at least 16 bytes; empty when `secret_file` is set.
    pub secret: String,
    /// `secret_file`: a file holding it, read each time the config is applied, which
    /// `media_tokens` rotates; empty when `secret` is set.
    pub secret_file: String,
}

/// Whether `s` is a key's name: 1 to 64 letters, digits, '.', '_', '~' and '-'.
#[must_use]
pub fn valid_key_name(s: &str) -> bool {
    (1..=64).contains(&s.len())
        && s.bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'_' | b'~' | b'-'))
}

const MEDIA_KEY_FIELDS: &[&str] = &["key", "secret", "secret_file"];

fn media_key(d: &mut Decoder<'_>, n: &Node, path: &str) -> Option<MediaKey> {
    let mut o = d.object(n, path)?;
    let mut k = MediaKey::default();
    let (name, off) = required(d, &mut o, "key");
    if off != o.off && !valid_key_name(&name) {
        d.error(
            off,
            &o.at("key"),
            format!("{name:?} is not a key name: 1 to 64 letters, digits, '.', '_', '~' and '-'"),
        );
    }
    k.key = name;
    let secret = text_at(d, &mut o, "secret");
    k.secret_file = text(d, &mut o, "secret_file");
    o.finish(d, MEDIA_KEY_FIELDS, &[]);
    k.secret = secret.as_ref().map(|(s, _)| s.clone()).unwrap_or_default();
    if k.secret.is_empty() == k.secret_file.is_empty() {
        d.error(n.off, path, "set secret or secret_file, one of the two");
    } else if let Some((s, soff)) = secret.filter(|(s, _)| !s.is_empty()) {
        if s.len() < MIN_MEDIA_SECRET {
            d.error(
                soff,
                &o_at(path, "secret"),
                format!(
                    "the secret is {} bytes: use at least {MIN_MEDIA_SECRET}",
                    s.len()
                ),
            );
        }
    }
    Some(k)
}

fn o_at(path: &str, key: &str) -> String {
    join_field(path, key)
}

impl MediaAuth {
    /// `token_ttl`, or its default.
    #[must_use]
    pub fn ttl(&self) -> Duration {
        or(self.token_ttl, DEFAULT_MEDIA_TOKEN_TTL)
    }

    /// Whether anyone may take `a` without credentials.
    #[must_use]
    pub fn is_public(&self, a: MediaAction) -> bool {
        self.public.contains(&a)
    }
}

/// A static bearer token of a `media_auth` policy.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct MediaToken {
    /// `name`: what logs call the token (`token:<name>`); empty for `token`.
    pub name: String,
    /// `token`: the token, at least 16 characters; empty when `token_sha256` is set.
    pub token: String,
    /// `token_sha256`: the token's SHA-256 in hex, 64 hex digits; empty when `token` is set.
    pub token_sha256: String,
    /// `publish`: the stream patterns it may publish.
    pub publish: Vec<String>,
    /// `play`: the stream patterns it may play.
    pub play: Vec<String>,
}

impl MediaToken {
    /// `token_sha256` decoded, or `None` when it is not 64 hex digits.
    #[must_use]
    pub fn hash_bytes(&self) -> Option<[u8; 32]> {
        let b = self.token_sha256.as_bytes();
        if b.len() != 64 {
            return None;
        }
        let mut out = [0u8; 32];
        for (o, pair) in out.iter_mut().zip(b.chunks_exact(2)) {
            let hex = |c: u8| (c as char).to_digit(16);
            let (hi, lo) = (hex(*pair.first()?)?, hex(*pair.get(1)?)?);
            *o = u8::try_from(hi.checked_mul(16)?.checked_add(lo)?).ok()?;
        }
        Some(out)
    }
}

/// A user of a `media_auth` policy.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct MediaUser {
    /// `name`: no colon or control character.
    pub name: String,
    /// `password`: the password; empty when `password_hash` is set.
    pub password: String,
    /// `password_hash`: its bcrypt hash; empty when `password` is set.
    pub password_hash: String,
    /// `publish`: the stream patterns the user may publish.
    pub publish: Vec<String>,
    /// `play`: the stream patterns the user may play.
    pub play: Vec<String>,
}

/// Whether `name` is a user name of `media_auth`: not empty, with no colon or control character
/// fit refuses (NUL, CR, LF, tab).
#[must_use]
pub fn valid_user_name(name: &str) -> bool {
    !name.is_empty() && !name.contains([':', '\0', '\r', '\n', '\t'])
}

/// Whether `p` is a stream pattern of `media_auth`: 1 to 256 letters, digits, '.', '_', '~',
/// '-', '/' and '*'.
#[must_use]
pub fn valid_media_pattern(p: &str) -> bool {
    !p.is_empty()
        && p.len() <= MAX_MEDIA_PATTERN
        && p.bytes().all(|b| {
            b.is_ascii_alphanumeric() || matches!(b, b'.' | b'_' | b'~' | b'/' | b'*' | b'-')
        })
}

// The defaults of `streaming`, as fit's.

/// fit's `DefaultStreamMaxStreams`.
pub const DEFAULT_STREAM_MAX_STREAMS: usize = 64;
/// fit's `DefaultStreamMaxPlayers`.
pub const DEFAULT_STREAM_MAX_PLAYERS: usize = 256;
/// fit's `DefaultStreamGOPCacheBytes`.
pub const DEFAULT_STREAM_GOP_CACHE_BYTES: usize = 8 << 20;
/// fit's `DefaultStreamMaxConns`.
pub const DEFAULT_STREAM_MAX_CONNS: usize = 1024;
/// fit's `DefaultRTMPHandshakeTimeout`.
pub const DEFAULT_RTMP_HANDSHAKE_TIMEOUT: Duration = Duration::from_secs(10);
/// fit's `DefaultRTMPIdleTimeout`.
pub const DEFAULT_RTMP_IDLE_TIMEOUT: Duration = Duration::from_secs(30);
/// fit's `DefaultRTSPSessionTimeout`.
pub const DEFAULT_RTSP_SESSION_TIMEOUT: Duration = Duration::from_secs(60);
/// fit's `DefaultSRTLatency`.
pub const DEFAULT_SRT_LATENCY: Duration = Duration::from_millis(120);
/// fit's `DefaultSRTPeerIdleTimeout`.
pub const DEFAULT_SRT_PEER_IDLE_TIMEOUT: Duration = Duration::from_secs(5);
/// fit's `DefaultStreamWebRTCMaxSessions`.
pub const DEFAULT_STREAM_WEBRTC_MAX_SESSIONS: usize = 256;
/// fit's `DefaultStreamWebRTCConnectTimeout`.
pub const DEFAULT_STREAM_WEBRTC_CONNECT_TIMEOUT: Duration = Duration::from_secs(15);
/// fit's `DefaultStreamWebRTCMaxOfferBytes`.
pub const DEFAULT_STREAM_WEBRTC_MAX_OFFER_BYTES: usize = 32 << 10;
/// fit's `MaxStreamWebRTCOfferBytes`: the most `max_offer_bytes` may be.
pub const MAX_STREAM_WEBRTC_OFFER_BYTES: usize = 64 << 10;

/// `streaming`, fit's streaming role: one media hub where a stream is published once and played
/// over every protocol, with RTMP, RTSP and SRT listeners of its own and the WebRTC port of the
/// `stream_whip` and `stream_whep` handlers.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Streaming {
    /// `max_streams`: the most streams at once; [`DEFAULT_STREAM_MAX_STREAMS`] when 0.
    pub max_streams: usize,
    /// `max_players`: the most players of one stream; [`DEFAULT_STREAM_MAX_PLAYERS`] when 0.
    pub max_players: usize,
    /// `gop_cache_bytes`: each stream's cached group of pictures;
    /// [`DEFAULT_STREAM_GOP_CACHE_BYTES`] when 0.
    pub gop_cache_bytes: usize,
    /// `rtmp`: the RTMP listener; none when left out.
    pub rtmp: Option<StreamRtmp>,
    /// `rtsp`: the RTSP listener; none when left out.
    pub rtsp: Option<StreamRtsp>,
    /// `srt`: the SRT listener; none when left out.
    pub srt: Option<StreamSrt>,
    /// `webrtc`: the UDP port of `stream_whip` and `stream_whep`; none when left out.
    pub webrtc: Option<StreamWebRtc>,
    /// `access`: the client addresses the role's listeners serve, before the global `access`.
    pub access: Option<Access>,
    /// `channels` (I145): library items played into the hub as live streams.
    pub channels: Vec<crate::channels::StreamChannel>,
    /// `push` (I146): outside servers streams of the hub are relayed to.
    pub push: Vec<crate::channels::StreamPush>,
}

impl Streaming {
    /// Every push target, `push`'s and the channels', named (fit's `PushTargets`).
    #[must_use]
    pub fn push_targets(&self) -> Vec<crate::channels::PushTarget> {
        crate::channels::push_targets(&self.push, &self.channels)
    }
}

impl Streaming {
    /// `max_streams`, or its default.
    #[must_use]
    pub fn streams(&self) -> usize {
        or_count(self.max_streams, DEFAULT_STREAM_MAX_STREAMS)
    }

    /// `max_players`, or its default.
    #[must_use]
    pub fn players(&self) -> usize {
        or_count(self.max_players, DEFAULT_STREAM_MAX_PLAYERS)
    }

    /// `gop_cache_bytes`, or its default.
    #[must_use]
    pub fn gop_cache(&self) -> usize {
        or_count(self.gop_cache_bytes, DEFAULT_STREAM_GOP_CACHE_BYTES)
    }
}

/// `streaming.rtmp`.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct StreamRtmp {
    /// `listen`: the TCP address, as `:1935`.
    pub listen: String,
    /// `auth`: the `media_auth` policy a publish and a play must satisfy; empty for none.
    pub auth: String,
    /// `max_conns`: the most connections at once; [`DEFAULT_STREAM_MAX_CONNS`] when 0.
    pub max_conns: usize,
    /// `handshake_timeout`: from connecting to publishing or playing;
    /// [`DEFAULT_RTMP_HANDSHAKE_TIMEOUT`] when 0.
    pub handshake_timeout: Duration,
    /// `idle_timeout`: a publisher that sends nothing; [`DEFAULT_RTMP_IDLE_TIMEOUT`] when 0.
    pub idle_timeout: Duration,
    /// `play_wait`: how long a player may wait for a publisher; none when 0.
    pub play_wait: Duration,
}

impl StreamRtmp {
    /// `max_conns`, or its default.
    #[must_use]
    pub fn conns(&self) -> usize {
        or_count(self.max_conns, DEFAULT_STREAM_MAX_CONNS)
    }

    /// `handshake_timeout`, or its default.
    #[must_use]
    pub fn handshake(&self) -> Duration {
        or(self.handshake_timeout, DEFAULT_RTMP_HANDSHAKE_TIMEOUT)
    }

    /// `idle_timeout`, or its default.
    #[must_use]
    pub fn idle(&self) -> Duration {
        or(self.idle_timeout, DEFAULT_RTMP_IDLE_TIMEOUT)
    }
}

/// `streaming.rtsp`.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct StreamRtsp {
    /// `listen`: the TCP address, as `:8554`.
    pub listen: String,
    /// `auth`: the `media_auth` policy RTSP requests must satisfy; empty for none.
    pub auth: String,
    /// `max_conns`: the most connections at once; [`DEFAULT_STREAM_MAX_CONNS`] when 0.
    pub max_conns: usize,
    /// `session_timeout`: a session that sends nothing; [`DEFAULT_RTSP_SESSION_TIMEOUT`] when 0.
    /// akio's RTSP server holds every connection to it, a session or not (fit's `idle_timeout`,
    /// 60 s as this, waits for akio A264).
    pub session_timeout: Duration,
    /// `play_wait`: how long a player may outlive its publisher; none when 0.
    pub play_wait: Duration,
}

impl StreamRtsp {
    /// `max_conns`, or its default.
    #[must_use]
    pub fn conns(&self) -> usize {
        or_count(self.max_conns, DEFAULT_STREAM_MAX_CONNS)
    }

    /// `session_timeout`, or its default.
    #[must_use]
    pub fn session(&self) -> Duration {
        or(self.session_timeout, DEFAULT_RTSP_SESSION_TIMEOUT)
    }
}

/// `streaming.srt`.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct StreamSrt {
    /// `listen`: the UDP address, as `:8890`.
    pub listen: String,
    /// `auth`: the `media_auth` policy a caller must satisfy; empty for none.
    pub auth: String,
    /// `max_conns`: the most sessions at once; [`DEFAULT_STREAM_MAX_CONNS`] when 0.
    pub max_conns: usize,
    /// `latency`: the receive latency asked for; [`DEFAULT_SRT_LATENCY`] when 0.
    pub latency: Duration,
    /// `passphrase`: 10 to 79 characters; empty for none.
    pub passphrase: String,
    /// `peer_idle_timeout`: a peer that sends nothing; [`DEFAULT_SRT_PEER_IDLE_TIMEOUT`] when 0.
    pub peer_idle_timeout: Duration,
    /// `play_wait`: how long a player may wait for a publisher; none when 0.
    pub play_wait: Duration,
}

impl StreamSrt {
    /// `max_conns`, or its default.
    #[must_use]
    pub fn conns(&self) -> usize {
        or_count(self.max_conns, DEFAULT_STREAM_MAX_CONNS)
    }

    /// `latency`, or its default.
    #[must_use]
    pub fn latency_or(&self) -> Duration {
        or(self.latency, DEFAULT_SRT_LATENCY)
    }

    /// `peer_idle_timeout`, or its default.
    #[must_use]
    pub fn peer_idle(&self) -> Duration {
        or(self.peer_idle_timeout, DEFAULT_SRT_PEER_IDLE_TIMEOUT)
    }
}

/// `streaming.webrtc`: the UDP port the PeerConnections of `stream_whip` and `stream_whep`
/// share.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct StreamWebRtc {
    /// `listen`: the UDP address, as `:8188`.
    pub listen: String,
    /// `public_ips`: the addresses peers are told to reach the port at.
    pub public_ips: Vec<String>,
    /// `ice_servers`: STUN and TURN servers the peers are told of.
    pub ice_servers: Vec<IceServer>,
    /// `max_sessions`: the most sessions at once; [`DEFAULT_STREAM_WEBRTC_MAX_SESSIONS`] when 0.
    pub max_sessions: usize,
    /// `connect_timeout`: from the answer to a connected PeerConnection;
    /// [`DEFAULT_STREAM_WEBRTC_CONNECT_TIMEOUT`] when 0.
    pub connect_timeout: Duration,
    /// `play_wait`: how long a WHEP offer may wait for a publisher; none when 0.
    pub play_wait: Duration,
    /// `max_offer_bytes`: the largest offer; [`DEFAULT_STREAM_WEBRTC_MAX_OFFER_BYTES`] when 0.
    pub max_offer_bytes: usize,
}

impl StreamWebRtc {
    /// `max_sessions`, or its default.
    #[must_use]
    pub fn sessions(&self) -> usize {
        or_count(self.max_sessions, DEFAULT_STREAM_WEBRTC_MAX_SESSIONS)
    }

    /// `connect_timeout`, or its default.
    #[must_use]
    pub fn connect(&self) -> Duration {
        or(self.connect_timeout, DEFAULT_STREAM_WEBRTC_CONNECT_TIMEOUT)
    }

    /// `max_offer_bytes`, or its default.
    #[must_use]
    pub fn offer_bytes(&self) -> usize {
        or_count(self.max_offer_bytes, DEFAULT_STREAM_WEBRTC_MAX_OFFER_BYTES)
    }
}

/// A STUN or TURN server, as fit's `config.ICEServer`.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct IceServer {
    /// `urls`: its `stun:`, `stuns:`, `turn:` or `turns:` URLs.
    pub urls: Vec<String>,
    /// `username`: a TURN server's user name.
    pub username: String,
    /// `credential`: a TURN server's password.
    pub credential: String,
}

fn or(d: Duration, def: Duration) -> Duration {
    if d.is_zero() { def } else { d }
}

fn or_count(n: usize, def: usize) -> usize {
    if n == 0 { def } else { n }
}

// What akit reads, and what fit reads that akit does not yet, object by object.

const MEDIA_AUTH_FIELDS: &[&str] = &[
    // I143: the keyring; I144: signed URLs.
    "keys",
    "url_signing",
    "name",
    "tokens",
    "users",
    "secret",
    "secret_file",
    "public",
    "token_ttl",
];
const MEDIA_AUTH_FIT_ONLY: &[&str] = &[];
const MEDIA_TOKEN_FIELDS: &[&str] = &["name", "token", "token_sha256", "publish", "play"];
const MEDIA_USER_FIELDS: &[&str] = &["name", "password", "password_hash", "publish", "play"];
const STREAMING_FIELDS: &[&str] = &[
    "max_streams",
    "max_players",
    "gop_cache_bytes",
    "rtmp",
    "rtsp",
    "srt",
    "webrtc",
    "access",
    // I145, I146.
    "channels",
    "push",
];
const STREAMING_FIT_ONLY: &[&str] = &[];
const STREAM_RTMP_FIELDS: &[&str] = &[
    "listen",
    "auth",
    "max_conns",
    "handshake_timeout",
    "idle_timeout",
    "play_wait",
];
// Multipath TCP is not offered by akio's listeners; max_message_bytes waits for akio A263.
const STREAM_RTMP_FIT_ONLY: &[&str] = &["mptcp", "max_message_bytes"];
const STREAM_RTSP_FIELDS: &[&str] = &[
    "listen",
    "auth",
    "max_conns",
    "session_timeout",
    "play_wait",
];
// Digest and Basic, and a connection without a session timed apart, wait for akio A264 (a request
// hook in akio's RTSP server, and its idle deadline).
const STREAM_RTSP_FIT_ONLY: &[&str] = &["mptcp", "auth_methods", "realm", "idle_timeout"];
const STREAM_SRT_FIELDS: &[&str] = &[
    "listen",
    "auth",
    "max_conns",
    "latency",
    "passphrase",
    "peer_idle_timeout",
    "play_wait",
];
const STREAM_WEBRTC_FIELDS: &[&str] = &[
    "listen",
    "public_ips",
    "ice_servers",
    "max_sessions",
    "connect_timeout",
    "play_wait",
    "max_offer_bytes",
];
const STREAM_ICE_SERVER_FIELDS: &[&str] = &["urls", "username", "credential"];

/// A name a member gives a `media_auth` policy by: checked once every file is read.
#[derive(Debug, Clone)]
pub(crate) struct AuthRef {
    pub(crate) name: String,
    pub(crate) off: usize,
    pub(crate) path: String,
    /// A channel's `library` (I145), not a policy: checked against `media_library`.
    pub(crate) library: bool,
}

/// Decodes `media_auth`, with fit's checks of each policy and of the list.
pub(crate) fn media_auth(d: &mut Decoder<'_>, n: &Node, path: &str) -> Vec<MediaAuth> {
    let got = d.array(n, path, |d, n, p| {
        policy(d, n, p).map(|m| (m, n.off, p.to_owned()))
    });
    let mut seen: Vec<&str> = Vec::new();
    for (m, off, p) in &got {
        if !m.name.is_empty() && crate::model::valid_name(&m.name) {
            if seen.contains(&m.name.as_str()) {
                d.error(
                    *off,
                    &join_field(p, "name"),
                    format!("another media_auth policy is already named {:?}", m.name),
                );
            }
            seen.push(&m.name);
        }
    }
    got.into_iter().map(|(m, _, _)| m).collect()
}

fn patterns(d: &mut Decoder<'_>, o: &mut Obj<'_>, key: &str) -> Vec<String> {
    let Some(v) = o.take(key) else {
        return Vec::new();
    };
    let p = o.at(key);
    let mut out = Vec::new();
    for (i, (off, s)) in d.strings(v, &p).into_iter().enumerate() {
        if !valid_media_pattern(&s) {
            d.error(
                off,
                &join_index(&p, i),
                format!(
                    "{s:?} is not a stream pattern: 1 to 256 letters, digits, '.', '_', '~', \
                     '-', '/' and '*'"
                ),
            );
        }
        out.push(s);
    }
    out
}

fn policy(d: &mut Decoder<'_>, n: &Node, path: &str) -> Option<MediaAuth> {
    let mut o = d.object(n, path)?;
    let mut m = MediaAuth::default();
    let (name, off) = required(d, &mut o, "name");
    if off != o.off && !crate::model::valid_name(&name) {
        d.error(
            off,
            &o.at("name"),
            format!(
                "{name:?} is not a valid name: use letters, digits, '_', '.' and '-', starting \
                 with a letter or digit"
            ),
        );
    }
    m.name = name;
    let tokens = list(d, &mut o, "tokens", token);
    let users = list(d, &mut o, "users", user);
    let secret = text_at(d, &mut o, "secret");
    let secret_file = text_at(d, &mut o, "secret_file");
    let keys_at = o.take("keys").map(|v| (v.off, o.at("keys"), v));
    if let Some((koff, kp, v)) = &keys_at {
        let got = d.array(v, kp, |d, n, p| {
            media_key(d, n, p).map(|k| (k, n.off, p.to_owned()))
        });
        if got.len() > MAX_MEDIA_KEYS {
            d.error(*koff, kp, format!("at most {MAX_MEDIA_KEYS} keys"));
        }
        let mut seen: Vec<String> = Vec::new();
        for (k, off, p) in &got {
            if valid_key_name(&k.key) {
                if seen.contains(&k.key) {
                    d.error(
                        *off,
                        &join_field(p, "key"),
                        format!("another key is already named {:?}", k.key),
                    );
                }
                seen.push(k.key.clone());
            }
        }
        m.keys = got.into_iter().map(|(k, _, _)| k).collect();
    }
    let url_at = o
        .take("url_signing")
        .map(|v| (v.off, o.at("url_signing"), v));
    if let Some((_, up, v)) = &url_at {
        m.url_signing = url_signing(d, v, up);
    }
    if let Some(v) = o.take("public") {
        let p = o.at("public");
        for (i, (off, s)) in d.strings(v, &p).into_iter().enumerate() {
            match MediaAction::parse(&s) {
                Some(a) if m.public.contains(&a) => {
                    d.error(off, &join_index(&p, i), format!("{s:?} is listed twice"));
                }
                Some(a) => m.public.push(a),
                None => d.error(
                    off,
                    &join_index(&p, i),
                    format!("{s:?} is not one of \"publish\", \"play\""),
                ),
            }
        }
    }
    m.token_ttl = duration(d, &mut o, "token_ttl");
    o.finish(d, MEDIA_AUTH_FIELDS, MEDIA_AUTH_FIT_ONLY);
    if tokens.is_empty()
        && users.is_empty()
        && secret.as_ref().is_none_or(|(s, _)| s.is_empty())
        && secret_file.as_ref().is_none_or(|(s, _)| s.is_empty())
        && m.keys.is_empty()
        && m.public.is_empty()
    {
        d.error(
            n.off,
            path,
            "the policy lets no one in: set tokens, users, secret, secret_file, keys or public",
        );
    }
    if let (Some((uoff, up, _)), true) = (
        &url_at,
        m.keys.is_empty()
            && secret.as_ref().is_none_or(|(s, _)| s.is_empty())
            && secret_file.as_ref().is_none_or(|(s, _)| s.is_empty()),
    ) {
        d.error(
            *uoff,
            up,
            "url_signing needs a key to sign with: set secret, secret_file or keys",
        );
    }
    let single = secret.as_ref().is_some_and(|(s, _)| !s.is_empty())
        || secret_file.as_ref().is_some_and(|(s, _)| !s.is_empty());
    if let (false, true, Some((koff, kp, _))) = (m.keys.is_empty(), single, &keys_at) {
        d.error(
            *koff,
            kp,
            "set keys, or secret or secret_file, one of the two",
        );
    }
    match (&secret, &secret_file) {
        (Some((s, _)), Some((f, off))) if !s.is_empty() && !f.is_empty() => {
            d.error(
                *off,
                &join_field(path, "secret_file"),
                "set secret or secret_file, one of the two",
            );
        }
        (Some((s, off)), _) if !s.is_empty() && s.len() < MIN_MEDIA_SECRET => {
            d.error(
                *off,
                &join_field(path, "secret"),
                format!(
                    "the secret is {} bytes: use at least {MIN_MEDIA_SECRET}",
                    s.len()
                ),
            );
        }
        _ => {}
    }
    m.secret = secret.map(|(s, _)| s).unwrap_or_default();
    m.secret_file = secret_file.map(|(s, _)| s).unwrap_or_default();
    let mut names: Vec<String> = Vec::new();
    for (t, off, p) in &tokens {
        if !t.name.is_empty() {
            if names.contains(&t.name) {
                d.error(
                    *off,
                    &join_field(p, "name"),
                    format!("another token is already named {:?}", t.name),
                );
            }
            names.push(t.name.clone());
        }
    }
    m.tokens = tokens.into_iter().map(|(t, _, _)| t).collect();
    let mut names: Vec<String> = Vec::new();
    for (u, off, p) in &users {
        if valid_user_name(&u.name) {
            if names.contains(&u.name) {
                d.error(
                    *off,
                    &join_field(p, "name"),
                    format!("another user is already named {:?}", u.name),
                );
            }
            names.push(u.name.clone());
        }
    }
    m.users = users.into_iter().map(|(u, _, _)| u).collect();
    Some(m)
}

fn token(d: &mut Decoder<'_>, n: &Node, path: &str) -> Option<MediaToken> {
    let mut o = d.object(n, path)?;
    let mut t = MediaToken {
        name: text(d, &mut o, "name"),
        ..MediaToken::default()
    };
    let tok = text_at(d, &mut o, "token");
    let sum = text_at(d, &mut o, "token_sha256");
    t.publish = patterns(d, &mut o, "publish");
    t.play = patterns(d, &mut o, "play");
    o.finish(d, MEDIA_TOKEN_FIELDS, &[]);
    let has = |v: &Option<(String, usize)>| v.as_ref().is_some_and(|(s, _)| !s.is_empty());
    if has(&tok) == has(&sum) {
        d.error(n.off, path, "set token or token_sha256, one of the two");
    } else if let Some((s, off)) = tok.as_ref().filter(|(s, _)| !s.is_empty()) {
        if s.len() < MIN_MEDIA_TOKEN {
            d.error(
                *off,
                &join_field(path, "token"),
                format!(
                    "the token is {} characters: use at least {MIN_MEDIA_TOKEN}",
                    s.len()
                ),
            );
        }
    }
    t.token = tok.map(|(s, _)| s).unwrap_or_default();
    if let Some((s, off)) = sum {
        t.token_sha256 = s;
        if !t.token_sha256.is_empty() && !t.token.is_empty() {
            // Reported above.
        } else if !t.token_sha256.is_empty() && t.hash_bytes().is_none() {
            d.error(
                off,
                &join_field(path, "token_sha256"),
                "not a SHA-256 in hex: 64 hex digits",
            );
        }
    }
    Some(t)
}

fn user(d: &mut Decoder<'_>, n: &Node, path: &str) -> Option<MediaUser> {
    let mut o = d.object(n, path)?;
    let mut u = MediaUser::default();
    let (name, off) = required(d, &mut o, "name");
    if off != o.off && !valid_user_name(&name) {
        d.error(
            off,
            &o.at("name"),
            format!(
                "{name:?} is not a user name: it is empty, or has a colon or a control character"
            ),
        );
    }
    u.name = name;
    let password = text_at(d, &mut o, "password");
    let hash = text_at(d, &mut o, "password_hash");
    u.publish = patterns(d, &mut o, "publish");
    u.play = patterns(d, &mut o, "play");
    o.finish(d, MEDIA_USER_FIELDS, &[]);
    let has = |v: &Option<(String, usize)>| v.as_ref().is_some_and(|(s, _)| !s.is_empty());
    if has(&password) == has(&hash) {
        d.error(n.off, path, "set password or password_hash, one of the two");
    } else if let Some((h, off)) = hash.as_ref().filter(|(h, _)| !h.is_empty()) {
        if akio_crypto::bcrypt::Hash::parse(h).is_err() {
            d.error(
                *off,
                &join_field(path, "password_hash"),
                "not a bcrypt hash: make one with akit hash-password",
            );
        }
    }
    u.password = password.map(|(s, _)| s).unwrap_or_default();
    u.password_hash = hash.map(|(s, _)| s).unwrap_or_default();
    Some(u)
}

/// Decodes `streaming`, with fit's checks, and the `media_auth` names its listeners give, which
/// are checked once every file is read.
pub(crate) fn streaming(
    d: &mut Decoder<'_>,
    n: &Node,
    path: &str,
) -> Option<(Streaming, Vec<AuthRef>)> {
    let mut o = d.object(n, path)?;
    let mut s = Streaming {
        max_streams: count(d, &mut o, "max_streams", i64::MAX),
        max_players: count(d, &mut o, "max_players", i64::MAX),
        gop_cache_bytes: count(d, &mut o, "gop_cache_bytes", i64::MAX),
        ..Streaming::default()
    };
    let mut refs = Vec::new();
    let mut auth = |d: &mut Decoder<'_>, o: &mut Obj<'_>| -> String {
        match text_at(d, o, "auth") {
            Some((name, off)) if !name.is_empty() => {
                refs.push(AuthRef {
                    name: name.clone(),
                    off,
                    path: o.at("auth"),
                    library: false,
                });
                name
            }
            _ => String::new(),
        }
    };
    if let Some(v) = o.take("rtmp") {
        let p = o.at("rtmp");
        if let Some(mut r) = d.object(v, &p) {
            let listen = listen(d, &mut r);
            let a = auth(d, &mut r);
            s.rtmp = Some(StreamRtmp {
                listen,
                auth: a,
                max_conns: count(d, &mut r, "max_conns", i64::MAX),
                handshake_timeout: duration(d, &mut r, "handshake_timeout"),
                idle_timeout: duration(d, &mut r, "idle_timeout"),
                play_wait: duration(d, &mut r, "play_wait"),
            });
            r.finish(d, STREAM_RTMP_FIELDS, STREAM_RTMP_FIT_ONLY);
        }
    }
    if let Some(v) = o.take("rtsp") {
        let p = o.at("rtsp");
        if let Some(mut r) = d.object(v, &p) {
            let listen = listen(d, &mut r);
            let a = auth(d, &mut r);
            s.rtsp = Some(StreamRtsp {
                listen,
                auth: a,
                max_conns: count(d, &mut r, "max_conns", i64::MAX),
                session_timeout: duration(d, &mut r, "session_timeout"),
                play_wait: duration(d, &mut r, "play_wait"),
            });
            r.finish(d, STREAM_RTSP_FIELDS, STREAM_RTSP_FIT_ONLY);
        }
    }
    if let Some(v) = o.take("srt") {
        let p = o.at("srt");
        if let Some(mut r) = d.object(v, &p) {
            let listen = listen(d, &mut r);
            let a = auth(d, &mut r);
            let mut srt = StreamSrt {
                listen,
                auth: a,
                max_conns: count(d, &mut r, "max_conns", i64::MAX),
                latency: duration(d, &mut r, "latency"),
                ..StreamSrt::default()
            };
            if let Some((pass, off)) = text_at(d, &mut r, "passphrase") {
                let n = pass.len();
                if n != 0 && !(10..=79).contains(&n) {
                    d.error(
                        off,
                        &r.at("passphrase"),
                        format!("the passphrase is {n} characters: SRT takes 10 to 79"),
                    );
                }
                srt.passphrase = pass;
            }
            srt.peer_idle_timeout = duration(d, &mut r, "peer_idle_timeout");
            srt.play_wait = duration(d, &mut r, "play_wait");
            r.finish(d, STREAM_SRT_FIELDS, &[]);
            s.srt = Some(srt);
        }
    }
    if let Some(v) = o.take("webrtc") {
        let p = o.at("webrtc");
        s.webrtc = webrtc(d, v, &p);
    }
    s.access = access_of(d, &mut o);
    let mut libs = Vec::new();
    s.channels = crate::channels::channels(d, &mut o, &mut libs);
    s.push = crate::channels::pushes(d, &mut o);
    crate::channels::check_pushes(d, (o.off, path), &s.push, &s.channels);
    refs.extend(libs.into_iter().map(|l| AuthRef {
        name: l.name,
        off: l.off,
        path: l.path,
        library: true,
    }));
    o.finish(d, STREAMING_FIELDS, STREAMING_FIT_ONLY);
    if s.rtmp.is_none()
        && s.rtsp.is_none()
        && s.srt.is_none()
        && s.webrtc.is_none()
        && s.channels.is_empty()
    {
        d.error(
            n.off,
            path,
            "nothing takes streams in: set rtmp, rtsp, srt, webrtc or channels",
        );
    }
    Some((s, refs))
}

/// A listener's `listen`: required, a host and port, recorded for the clash check.
fn listen(d: &mut Decoder<'_>, o: &mut Obj<'_>) -> String {
    let (address, off) = required(d, o, "listen");
    if off != o.off {
        d.address_at(&o.at("listen"), off);
        check_listen(d, &address, off, &o.at("listen"));
    }
    address
}

fn webrtc(d: &mut Decoder<'_>, n: &Node, path: &str) -> Option<StreamWebRtc> {
    let mut o = d.object(n, path)?;
    let mut w = StreamWebRtc {
        listen: listen(d, &mut o),
        ..StreamWebRtc::default()
    };
    if let Some(v) = o.take("public_ips") {
        let p = o.at("public_ips");
        for (i, (off, s)) in d.strings(v, &p).into_iter().enumerate() {
            match s.parse::<std::net::IpAddr>() {
                Err(_) => d.error(
                    off,
                    &join_index(&p, i),
                    format!("{s:?} is not an IP address"),
                ),
                Ok(ip) if ip.is_unspecified() => {
                    d.error(off, &join_index(&p, i), format!("{s:?} is a wildcard"));
                }
                Ok(_) => {}
            }
            w.public_ips.push(s);
        }
    }
    w.ice_servers = list(d, &mut o, "ice_servers", ice_server)
        .into_iter()
        .map(|(s, _, _)| s)
        .collect();
    w.max_sessions = count(d, &mut o, "max_sessions", i64::MAX);
    w.connect_timeout = duration(d, &mut o, "connect_timeout");
    w.play_wait = duration(d, &mut o, "play_wait");
    let max = i64::try_from(MAX_STREAM_WEBRTC_OFFER_BYTES).unwrap_or(i64::MAX);
    w.max_offer_bytes = count(d, &mut o, "max_offer_bytes", max);
    o.finish(d, STREAM_WEBRTC_FIELDS, &[]);
    Some(w)
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
    o.finish(d, STREAM_ICE_SERVER_FIELDS, &[]);
    if !link_safe(&s.username) || !link_safe(&s.credential) {
        d.error(
            n.off,
            path,
            "the username and credential may not hold a double quote, a backslash or a control \
             character",
        );
    }
    Some(s)
}

/// Whether `u` is a `stun:`, `stuns:`, `turn:` or `turns:` URL with a host, which a Link header
/// can carry (fit's `iceURL`).
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

/// Whether `s` can be a quoted parameter of a Link header (fit's `linkSafe`).
fn link_safe(s: &str) -> bool {
    !s.chars()
        .any(|c| c == '"' || c == '\\' || c < '\u{20}' || c == '\u{7f}')
}

/// Reports each name of `refs` that `policies` has no policy of, as fit's
/// `there is no media_auth policy named`.
pub(crate) fn check_refs(d: &mut Decoder<'_>, refs: &[AuthRef], policies: &[MediaAuth]) {
    for r in refs.iter().filter(|r| !r.library) {
        if !policies.iter().any(|p| p.name == r.name) {
            d.error(
                r.off,
                &r.path,
                format!("there is no media_auth policy named {:?}", r.name),
            );
        }
    }
}

// I123: the handlers that play the streaming role's streams over HTTP.

/// fit's default `hls` `target_duration`.
pub const DEFAULT_HLS_TARGET_DURATION: Duration = Duration::from_secs(2);
/// fit's default `hls` `playlist_size`.
pub const DEFAULT_HLS_PLAYLIST_SIZE: usize = 6;
/// fit's most `hls` `playlist_size`.
pub const MAX_HLS_PLAYLIST_SIZE: usize = 100;
/// fit's default `hls` `max_bytes`.
pub const DEFAULT_HLS_MAX_BYTES: u64 = 32 << 20;
/// fit's default `hls` `idle_timeout`.
pub const DEFAULT_HLS_IDLE_TIMEOUT: Duration = Duration::from_secs(30);

/// The `httpflv` handler's params, as fit's `streaming.HTTPFLV`: the role's streams played as
/// HTTP-FLV, and as WebSocket-FLV to an upgrade.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct HttpFlv {
    /// `prefix`: the part of the path before the stream's; `/` when left out.
    pub prefix: String,
    /// `auth`: the `media_auth` policy a player must satisfy; empty for none.
    pub auth: String,
    /// `wait`: how long a player may wait for a publisher; none when 0.
    pub wait: Duration,
    /// `cors`: sent as `Access-Control-Allow-Origin`; none when empty.
    pub cors: String,
    /// The policy `auth` names, from the config, for the handler to compile when the role
    /// `media_auth` has not applied it yet.
    pub policy: Option<MediaAuth>,
}

/// The segment format of an `hls` playlist (`format`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum HlsFormat {
    /// `fmp4`, fragmented MP4 (the default).
    #[default]
    Fmp4,
    /// `ts`, MPEG-TS.
    Ts,
}

/// The `hls` handler's params, as fit's `streaming.HLS`.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Hls {
    /// `prefix`: the part of the path before the stream's; `/` when left out.
    pub prefix: String,
    /// `auth`: the `media_auth` policy every request must satisfy; empty for none.
    pub auth: String,
    /// `format`: the segment format of a playlist's URL without `?format=`.
    pub format: HlsFormat,
    /// `target_duration`: [`DEFAULT_HLS_TARGET_DURATION`] when 0.
    pub target_duration: Duration,
    /// `part_duration`: Low-Latency HLS's parts; off when 0.
    pub part_duration: Duration,
    /// `playlist_size`: [`DEFAULT_HLS_PLAYLIST_SIZE`] when 0.
    pub playlist_size: usize,
    /// `max_bytes`: [`DEFAULT_HLS_MAX_BYTES`] when 0.
    pub max_bytes: u64,
    /// `play_wait`: how long a playlist request may wait for a publisher; none when 0.
    pub play_wait: Duration,
    /// `idle_timeout`: [`DEFAULT_HLS_IDLE_TIMEOUT`] when 0.
    pub idle_timeout: Duration,
    /// `cors`: sent as `Access-Control-Allow-Origin`; none when empty.
    pub cors: String,
    /// The policy `auth` names, from the config ([`HttpFlv::policy`]).
    pub policy: Option<MediaAuth>,
}

impl Hls {
    /// `target_duration`, or its default.
    #[must_use]
    pub fn target(&self) -> Duration {
        or(self.target_duration, DEFAULT_HLS_TARGET_DURATION)
    }

    /// `playlist_size`, or its default.
    #[must_use]
    pub fn playlist(&self) -> usize {
        or_count(self.playlist_size, DEFAULT_HLS_PLAYLIST_SIZE)
    }

    /// `max_bytes`, or its default.
    #[must_use]
    pub fn bytes(&self) -> u64 {
        if self.max_bytes == 0 {
            DEFAULT_HLS_MAX_BYTES
        } else {
            self.max_bytes
        }
    }

    /// `idle_timeout`, or its default.
    #[must_use]
    pub fn idle(&self) -> Duration {
        or(self.idle_timeout, DEFAULT_HLS_IDLE_TIMEOUT)
    }
}

const HTTPFLV_FIELDS: &[&str] = &["prefix", "auth", "wait", "cors"];
const HLS_FIELDS: &[&str] = &[
    "prefix",
    "auth",
    "format",
    "target_duration",
    "part_duration",
    "playlist_size",
    "max_bytes",
    "play_wait",
    "idle_timeout",
    "cors",
];

/// A handler's `prefix`: it starts with `/`, and ends with one once read (fit's `provision`).
fn prefix(d: &mut Decoder<'_>, o: &mut Obj<'_>) -> String {
    match text_at(d, o, "prefix") {
        Some((p, off)) if !p.is_empty() => {
            if !p.starts_with('/') {
                d.error(off, &o.at("prefix"), format!("{p:?} does not start with /"));
            }
            if p.ends_with('/') { p } else { format!("{p}/") }
        }
        _ => "/".to_owned(),
    }
}

/// Decodes the `httpflv` handler's params; none is every default.
pub(crate) fn httpflv(d: &mut Decoder<'_>, params: Option<&Node>, path: &str) -> Option<HttpFlv> {
    let Some(n) = params else {
        return Some(HttpFlv {
            prefix: "/".into(),
            ..HttpFlv::default()
        });
    };
    let mut o = d.object(n, path)?;
    let h = HttpFlv {
        prefix: prefix(d, &mut o),
        auth: text(d, &mut o, "auth"),
        wait: duration(d, &mut o, "wait"),
        cors: text(d, &mut o, "cors"),
        policy: None,
    };
    o.finish(d, HTTPFLV_FIELDS, &[]);
    Some(h)
}

/// Decodes the `hls` handler's params; none is every default.
pub(crate) fn hls(d: &mut Decoder<'_>, params: Option<&Node>, path: &str) -> Option<Hls> {
    let Some(n) = params else {
        return Some(Hls {
            prefix: "/".into(),
            ..Hls::default()
        });
    };
    let mut o = d.object(n, path)?;
    let mut h = Hls {
        prefix: prefix(d, &mut o),
        auth: text(d, &mut o, "auth"),
        ..Hls::default()
    };
    if let Some((f, off)) = text_at(d, &mut o, "format") {
        match f.as_str() {
            "" | "fmp4" => {}
            "ts" => h.format = HlsFormat::Ts,
            _ => d.error(
                off,
                &o.at("format"),
                format!("{f:?} is not one of \"fmp4\", \"ts\""),
            ),
        }
    }
    h.target_duration = duration(d, &mut o, "target_duration");
    if let Some(v) = o.take("part_duration") {
        let p = o.at("part_duration");
        if let Some(t) = d.duration(v, &p) {
            if !t.is_zero() && !(Duration::from_millis(100)..=Duration::from_secs(5)).contains(&t) {
                d.error(
                    v.off,
                    &p,
                    format!("{} is not between 100ms and 5s", crate::duration::format(t)),
                );
            }
            h.part_duration = t;
        }
    }
    let max = i64::try_from(MAX_HLS_PLAYLIST_SIZE).unwrap_or(i64::MAX);
    h.playlist_size = count(d, &mut o, "playlist_size", max);
    h.max_bytes = o
        .take("max_bytes")
        .and_then(|v| d.int(v, &o.at("max_bytes"), 0, i64::MAX))
        .and_then(|n| u64::try_from(n).ok())
        .unwrap_or(0);
    h.play_wait = duration(d, &mut o, "play_wait");
    h.idle_timeout = duration(d, &mut o, "idle_timeout");
    h.cors = text(d, &mut o, "cors");
    o.finish(d, HLS_FIELDS, &[]);
    Some(h)
}

/// fit's `media_tokens` defaults and bounds (`handler/mediatokens`, I143).
pub const DEFAULT_TOKENS_PREFIX: &str = "/media-tokens";
/// See [`DEFAULT_TOKENS_PREFIX`].
pub const DEFAULT_TOKENS_MAX_TTL: Duration = Duration::from_secs(24 * 3600);
/// See [`DEFAULT_TOKENS_PREFIX`].
pub const DEFAULT_TOKENS_MAX_TOKENS: usize = 100;
/// See [`DEFAULT_TOKENS_PREFIX`].
pub const MAX_TOKENS_MAX_TOKENS: usize = 1000;
/// See [`DEFAULT_TOKENS_PREFIX`].
pub const DEFAULT_TOKENS_MAX_BODY_BYTES: usize = 65536;
/// See [`DEFAULT_TOKENS_PREFIX`].
pub const MIN_TOKENS_MAX_BODY_BYTES: usize = 256;
/// See [`DEFAULT_TOKENS_PREFIX`].
pub const MAX_TOKENS_MAX_BODY_BYTES: usize = 1 << 20;
/// See [`DEFAULT_TOKENS_PREFIX`].
pub const DEFAULT_TOKENS_REVOKE_TIMEOUT: Duration = Duration::from_secs(10);

/// The `media_tokens` handler's params, as fit's `mediatokens.Tokens` (I143): a route a site's
/// backend calls to sign tokens in batches and to revoke a key.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct MediaTokens {
    /// `prefix`: [`DEFAULT_TOKENS_PREFIX`] when empty.
    pub prefix: String,
    /// `auth`: the `media_auth` policy that signs the tokens; required.
    pub auth: String,
    /// `prefixes`: the stream-name prefixes the route may sign for; any when empty.
    pub prefixes: Vec<String>,
    /// `max_ttl`: [`DEFAULT_TOKENS_MAX_TTL`] when zero.
    pub max_ttl: Duration,
    /// `max_tokens`: [`DEFAULT_TOKENS_MAX_TOKENS`] when 0.
    pub max_tokens: usize,
    /// `max_body_bytes`: [`DEFAULT_TOKENS_MAX_BODY_BYTES`] when 0.
    pub max_body_bytes: usize,
    /// `revoke_timeout`: [`DEFAULT_TOKENS_REVOKE_TIMEOUT`] when zero.
    pub revoke_timeout: Duration,
    /// `url_base` (I144): what `/urls` answers start with, `/` trimmed; empty for paths alone.
    pub url_base: String,
    /// `url_prefixes` (I144): the path prefixes `/urls` may sign; any when empty.
    pub url_prefixes: Vec<String>,
    /// The policy `auth` names, from the config.
    pub policy: Option<MediaAuth>,
}

impl MediaTokens {
    /// `prefix`, or its default.
    #[must_use]
    pub fn prefix_or(&self) -> &str {
        if self.prefix.is_empty() {
            DEFAULT_TOKENS_PREFIX
        } else {
            &self.prefix
        }
    }

    /// `max_ttl`, or its default.
    #[must_use]
    pub fn ttl_bound(&self) -> Duration {
        or(self.max_ttl, DEFAULT_TOKENS_MAX_TTL)
    }

    /// `max_tokens`, or its default.
    #[must_use]
    pub fn tokens(&self) -> usize {
        or_count(self.max_tokens, DEFAULT_TOKENS_MAX_TOKENS)
    }

    /// `max_body_bytes`, or its default.
    #[must_use]
    pub fn body_bytes(&self) -> usize {
        or_count(self.max_body_bytes, DEFAULT_TOKENS_MAX_BODY_BYTES)
    }

    /// `revoke_timeout`, or its default.
    #[must_use]
    pub fn revoke(&self) -> Duration {
        or(self.revoke_timeout, DEFAULT_TOKENS_REVOKE_TIMEOUT)
    }
}

const MEDIA_TOKENS_FIELDS: &[&str] = &[
    "prefix",
    "auth",
    "prefixes",
    "max_ttl",
    "max_tokens",
    "max_body_bytes",
    "revoke_timeout",
    // I144.
    "url_base",
    "url_prefixes",
];
const MEDIA_TOKENS_FIT_ONLY: &[&str] = &[];

/// Decodes the `media_tokens` handler's params, with fit's checks (`Tokens.Provision`); its
/// policy and its route's middleware are checked once every file is read.
pub(crate) fn media_tokens(
    d: &mut Decoder<'_>,
    params: Option<&Node>,
    off: usize,
    path: &str,
) -> Option<MediaTokens> {
    let Some(n) = params else {
        d.error(off, &join_field(path, "auth"), "missing required field");
        return None;
    };
    let mut o = d.object(n, path)?;
    let mut t = MediaTokens::default();
    if let Some((p, poff)) = text_at(d, &mut o, "prefix") {
        if !p.is_empty() && !path_prefix(&p) {
            d.error(
                poff,
                &o.at("prefix"),
                format!(
                    "{p:?} is not a path of segments of letters, digits, '.', '_', '~' and '-', as /media-tokens"
                ),
            );
        }
        t.prefix = p;
    }
    let (auth, aoff) = required(d, &mut o, "auth");
    if aoff != o.off && auth.is_empty() {
        d.error(
            aoff,
            &o.at("auth"),
            "media_tokens needs a media_auth policy with secret_file, or a keyring of secret_files: auth names none",
        );
    }
    t.auth = auth;
    t.prefixes = patterns(d, &mut o, "prefixes");
    t.max_ttl = duration(d, &mut o, "max_ttl");
    if let Some(v) = o.take("max_tokens") {
        let max = i64::try_from(MAX_TOKENS_MAX_TOKENS).unwrap_or(i64::MAX);
        if let Some(x) = d.int(v, &o.at("max_tokens"), 0, max) {
            t.max_tokens = usize::try_from(x).unwrap_or(0);
        }
    }
    if let Some(v) = o.take("max_body_bytes") {
        let p = o.at("max_body_bytes");
        if let Some(x) = d.int(v, &p, 0, i64::MAX) {
            let x = usize::try_from(x).unwrap_or(usize::MAX);
            if x != 0 && !(MIN_TOKENS_MAX_BODY_BYTES..=MAX_TOKENS_MAX_BODY_BYTES).contains(&x) {
                d.error(
                    v.off,
                    &p,
                    format!(
                        "a body is at least {MIN_TOKENS_MAX_BODY_BYTES} and at most {MAX_TOKENS_MAX_BODY_BYTES} bytes"
                    ),
                );
            }
            t.max_body_bytes = x;
        }
    }
    t.revoke_timeout = duration(d, &mut o, "revoke_timeout");
    if let Some((b, boff)) = text_at(d, &mut o, "url_base") {
        if !b.is_empty() {
            let ok = (|| {
                let rest = b
                    .strip_prefix("https://")
                    .or_else(|| b.strip_prefix("http://"))?;
                let host = rest.split('/').next()?;
                (!host.is_empty() && !host.contains('@') && !b.contains(['?', '#', ' ']))
                    .then_some(())
            })()
            .is_some();
            if !ok {
                d.error(
                    boff,
                    &o.at("url_base"),
                    format!("{b:?} is not http:// or https://, a host and maybe a path, as https://media.example.com"),
                );
            }
        }
        t.url_base = b.trim_end_matches('/').to_owned();
    }
    if let Some(v) = o.take("url_prefixes") {
        let p = o.at("url_prefixes");
        for (i, (off, s)) in d.strings(v, &p).into_iter().enumerate() {
            let ok = s.starts_with('/')
                && s.len() <= 256
                && !s.contains("..")
                && s.bytes().all(|b| {
                    b.is_ascii_alphanumeric() || matches!(b, b'.' | b'_' | b'~' | b'/' | b'-')
                });
            if !ok {
                d.error(
                    off,
                    &join_index(&p, i),
                    format!("{s:?} is not a path prefix, as /vod/films/"),
                );
            }
            t.url_prefixes.push(s);
        }
    }
    o.finish(d, MEDIA_TOKENS_FIELDS, MEDIA_TOKENS_FIT_ONLY);
    Some(t)
}

/// fit's check of a `media_tokens` policy at provisioning: one with `secret_file`, or a keyring
/// whose every key has one, so that a revoke can rotate a file and outlive a restart.
pub(crate) fn tokens_policy_problem(p: &MediaAuth) -> Option<String> {
    if p.keys.is_empty() {
        return p.secret_file.is_empty().then(|| {
            format!(
                "media_tokens needs a media_auth policy with secret_file, or a keyring of secret_files: {:?} has neither",
                p.name
            )
        });
    }
    p.keys.iter().find(|k| k.secret_file.is_empty()).map(|k| {
        format!(
            "media_tokens needs a secret_file for every key of media_auth {:?}'s keyring: key {:?} has none",
            p.name, k.key
        )
    })
}

#[cfg(test)]
mod tests;

/// The `stream_whip` and `stream_whep` handlers' params, as fit's `streaming.StreamWHIP` and
/// `StreamWHEP` (I124): WHIP publishes into the role's hub, WHEP plays from it, on the role's
/// WebRTC port.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct StreamRtc {
    /// `stream`: the stream's path; when empty, the path after `prefix` a POST is sent to.
    pub stream: String,
    /// `prefix`: the part of the path before the stream's; `/whip/` or `/whep/` when left out.
    pub prefix: String,
    /// `auth`: the `media_auth` policy a POST must satisfy; empty for none.
    pub auth: String,
    /// The policy `auth` names, from the config ([`HttpFlv::policy`]).
    pub policy: Option<MediaAuth>,
}

const STREAM_RTC_FIELDS: &[&str] = &["stream", "prefix", "auth"];

/// Whether `p` is a stream's path (fit's `validStream`): segments of letters, digits, '.', '_',
/// '~' and '-', joined by '/', none of them `.` or `..`, at most 256 bytes.
#[must_use]
pub fn valid_stream(p: &str) -> bool {
    p.len() <= 256
        && !p.is_empty()
        && p.split('/').all(|seg| {
            !seg.is_empty()
                && seg != "."
                && seg != ".."
                && seg
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'_' | b'~' | b'-'))
        })
}

/// Decodes the `stream_whip` (`def` `/whip/`) or `stream_whep` (`/whep/`) handler's params; none
/// is every default.
pub(crate) fn stream_rtc(
    d: &mut Decoder<'_>,
    params: Option<&Node>,
    path: &str,
    def: &str,
) -> Option<StreamRtc> {
    let Some(n) = params else {
        return Some(StreamRtc {
            prefix: def.into(),
            ..StreamRtc::default()
        });
    };
    let mut o = d.object(n, path)?;
    let mut h = StreamRtc::default();
    if let Some((s, off)) = text_at(d, &mut o, "stream") {
        if !s.is_empty() && !valid_stream(&s) {
            d.error(
                off,
                &o.at("stream"),
                format!(
                    "{s:?} is not a stream's path: segments of letters, digits, '.', '_', '~' and '-' joined by /"
                ),
            );
        }
        h.stream = s;
    }
    // An empty prefix is the default, as fit's.
    h.prefix = match text_at(d, &mut o, "prefix") {
        Some((p, off)) if !p.is_empty() => {
            if !p.starts_with('/') {
                d.error(off, &o.at("prefix"), format!("{p:?} does not start with /"));
            }
            if p.ends_with('/') { p } else { format!("{p}/") }
        }
        _ => def.to_owned(),
    };
    h.auth = text(d, &mut o, "auth");
    o.finish(d, STREAM_RTC_FIELDS, &[]);
    Some(h)
}

/// fit's `media_pages` defaults (`handler/mediapages`).
pub const DEFAULT_PAGES_PREFIX: &str = "/media";
/// See [`DEFAULT_PAGES_PREFIX`].
pub const DEFAULT_PAGES_WHIP: &str = "/whip/{stream}";
/// See [`DEFAULT_PAGES_PREFIX`].
pub const DEFAULT_PAGES_WHEP: &str = "/whep/{stream}";
/// See [`DEFAULT_PAGES_PREFIX`].
pub const DEFAULT_PAGES_MAX_ROOMS: usize = 64;
/// See [`DEFAULT_PAGES_PREFIX`].
pub const DEFAULT_PAGES_MAX_MEMBERS: usize = 16;
/// See [`DEFAULT_PAGES_PREFIX`].
pub const DEFAULT_PAGES_MAX_MESSAGE_BYTES: usize = 1024;
/// See [`DEFAULT_PAGES_PREFIX`].
pub const DEFAULT_PAGES_MAX_FILE_BYTES: u64 = 5 << 20;
/// See [`DEFAULT_PAGES_PREFIX`].
pub const DEFAULT_PAGES_MEMBER_TTL: Duration = Duration::from_secs(30);
/// See [`DEFAULT_PAGES_PREFIX`].
pub const DEFAULT_PAGES_STALL_TIMEOUT: Duration = Duration::from_secs(30);

/// A URL the media pages show for a stream (`links[]`).
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct PageLink {
    /// `name`: what the URL is, as RTMP.
    pub name: String,
    /// `url`: with `{stream}`, `{token}` and `{host}` for the page to fill in.
    pub url: String,
}

/// The `media_pages` handler's params, as fit's `mediapages.Pages` (I125): akit's media pages
/// under a prefix, their login and their room API.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct MediaPages {
    /// `prefix`: [`DEFAULT_PAGES_PREFIX`] when empty.
    pub prefix: String,
    /// `title`: `akit media` when empty.
    pub title: String,
    /// `auth`: the `media_auth` policy of the room API, the login and the tokens; empty for none.
    pub auth: String,
    /// `login`: a login form for the policy's users.
    pub login: bool,
    /// `pages`: serve the pages (true when left out).
    pub pages: bool,
    /// `whip`: [`DEFAULT_PAGES_WHIP`] when empty.
    pub whip: String,
    /// `whep`: [`DEFAULT_PAGES_WHEP`] when empty.
    pub whep: String,
    /// `hls`: a stream's HLS playlist; none when empty.
    pub hls: String,
    /// `links`.
    pub links: Vec<PageLink>,
    /// `ice_servers`: the pages' PeerConnections' iceServers.
    pub ice_servers: Vec<IceServer>,
    /// `max_rooms`: [`DEFAULT_PAGES_MAX_ROOMS`] when 0.
    pub max_rooms: usize,
    /// `max_members`: [`DEFAULT_PAGES_MAX_MEMBERS`] when 0.
    pub max_members: usize,
    /// `max_message_bytes`: [`DEFAULT_PAGES_MAX_MESSAGE_BYTES`] when 0.
    pub max_message_bytes: usize,
    /// `mic_default`.
    pub mic_default: bool,
    /// `cam_default`.
    pub cam_default: bool,
    /// `chat` (true when left out).
    pub chat: bool,
    /// `max_file_bytes`: [`DEFAULT_PAGES_MAX_FILE_BYTES`] when 0.
    pub max_file_bytes: u64,
    /// `member_ttl`: [`DEFAULT_PAGES_MEMBER_TTL`] when 0.
    pub member_ttl: Duration,
    /// `stall_timeout`: [`DEFAULT_PAGES_STALL_TIMEOUT`] when 0.
    pub stall_timeout: Duration,
    /// The policy `auth` names, from the config ([`HttpFlv::policy`]).
    pub policy: Option<MediaAuth>,
}

impl MediaPages {
    /// `prefix`, or its default.
    #[must_use]
    pub fn prefix_or(&self) -> &str {
        if self.prefix.is_empty() {
            DEFAULT_PAGES_PREFIX
        } else {
            &self.prefix
        }
    }

    /// `max_rooms`, or its default.
    #[must_use]
    pub fn rooms(&self) -> usize {
        or_count(self.max_rooms, DEFAULT_PAGES_MAX_ROOMS)
    }

    /// `max_members`, or its default.
    #[must_use]
    pub fn members(&self) -> usize {
        or_count(self.max_members, DEFAULT_PAGES_MAX_MEMBERS)
    }

    /// `max_message_bytes`, or its default.
    #[must_use]
    pub fn message_bytes(&self) -> usize {
        or_count(self.max_message_bytes, DEFAULT_PAGES_MAX_MESSAGE_BYTES)
    }

    /// `max_file_bytes`, or its default.
    #[must_use]
    pub fn file_bytes(&self) -> u64 {
        if self.max_file_bytes == 0 {
            DEFAULT_PAGES_MAX_FILE_BYTES
        } else {
            self.max_file_bytes
        }
    }

    /// `member_ttl`, or its default.
    #[must_use]
    pub fn ttl(&self) -> Duration {
        or(self.member_ttl, DEFAULT_PAGES_MEMBER_TTL)
    }

    /// `stall_timeout`, or its default.
    #[must_use]
    pub fn stall(&self) -> Duration {
        or(self.stall_timeout, DEFAULT_PAGES_STALL_TIMEOUT)
    }
}

const MEDIA_PAGES_FIELDS: &[&str] = &[
    "prefix",
    "title",
    "auth",
    "login",
    "pages",
    "whip",
    "whep",
    "hls",
    "links",
    "ice_servers",
    "max_rooms",
    "max_members",
    "max_message_bytes",
    "mic_default",
    "cam_default",
    "chat",
    "max_file_bytes",
    "member_ttl",
    "stall_timeout",
];
const MEDIA_PAGES_FIT_ONLY: &[&str] = &["libraries", "vod", "channels"];
const PAGE_LINK_FIELDS: &[&str] = &["name", "url"];

/// Whether `p` is a path of plain segments, as `/media` (fit's `pathPrefix`).
fn path_prefix(p: &str) -> bool {
    p.starts_with('/')
        && p.split('/').skip(1).all(|seg| {
            !seg.is_empty()
                && seg
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'_' | b'~' | b'-'))
        })
}

/// Checks an endpoint's URL template (fit's `endpoint`): its origin when it is on another
/// server, `None` for a path of this one.
///
/// # Errors
///
/// What the template is not.
pub fn page_endpoint(v: &str) -> Result<Option<String>, String> {
    if v.len() > 512
        || !v.contains("{stream}")
        || v.contains([' ', '\t', '\r', '\n', '\0', '<', '>', '"', '\''])
    {
        return Err(format!(
            "{v:?} is not a URL of at most 512 characters with {{stream}} in it"
        ));
    }
    if v.starts_with('/') && !v.starts_with("//") {
        return Ok(None);
    }
    let u = v.replace("{stream}", "s");
    let bad = || format!("{v:?} is neither a path nor an http or https URL");
    let (scheme, rest) = u.split_once("://").ok_or_else(bad)?;
    if !scheme.eq_ignore_ascii_case("http") && !scheme.eq_ignore_ascii_case("https") {
        return Err(bad());
    }
    let host = rest.split(['/', '?', '#']).next().unwrap_or("");
    if host.is_empty() {
        return Err(bad());
    }
    Ok(Some(format!("{}://{host}", scheme.to_ascii_lowercase())))
}

/// Decodes the `media_pages` handler's params; none is every default.
pub(crate) fn media_pages(
    d: &mut Decoder<'_>,
    params: Option<&Node>,
    path: &str,
) -> Option<MediaPages> {
    let Some(n) = params else {
        return Some(MediaPages {
            pages: true,
            chat: true,
            ..MediaPages::default()
        });
    };
    let mut o = d.object(n, path)?;
    let mut p = MediaPages::default();
    if let Some((v, off)) = text_at(d, &mut o, "prefix") {
        if !v.is_empty() && !path_prefix(&v) {
            d.error(
                off,
                &o.at("prefix"),
                format!(
                    "{v:?} is not a path of segments of letters, digits, '.', '_', '~' and '-', as /media"
                ),
            );
        }
        p.prefix = v;
    }
    if let Some((v, off)) = text_at(d, &mut o, "title") {
        if v.chars().count() > 80 || v.contains(['<', '>', '&', '"', '\'', '\r', '\n']) {
            d.error(
                off,
                &o.at("title"),
                "a title is at most 80 characters, without <, >, &, quotes or line breaks",
            );
        }
        p.title = v;
    }
    p.auth = text(d, &mut o, "auth");
    p.login = boolean(d, &mut o, "login");
    p.pages = bool_opt(d, &mut o, "pages").unwrap_or(true);
    for (key, slot) in [
        ("whip", &mut p.whip),
        ("whep", &mut p.whep),
        ("hls", &mut p.hls),
    ] {
        if let Some((v, off)) = text_at(d, &mut o, key) {
            if !v.is_empty() {
                if let Err(e) = page_endpoint(&v) {
                    d.error(off, &o.at(key), e);
                }
            }
            *slot = v;
        }
    }
    p.links = list(d, &mut o, "links", page_link)
        .into_iter()
        .map(|(l, _, _)| l)
        .collect();
    p.ice_servers = list(d, &mut o, "ice_servers", ice_server)
        .into_iter()
        .map(|(s, _, _)| s)
        .collect();
    p.max_rooms = count(d, &mut o, "max_rooms", i64::MAX);
    p.max_members = count(d, &mut o, "max_members", 256);
    if let Some(v) = o.take("max_message_bytes") {
        if let Some(n) = d.int(v, &o.at("max_message_bytes"), 0, 65536) {
            if n != 0 && n < 256 {
                d.error(
                    v.off,
                    &o.at("max_message_bytes"),
                    format!("{n} is less than 256"),
                );
            }
            p.max_message_bytes = usize::try_from(n).unwrap_or(0);
        }
    }
    p.mic_default = boolean(d, &mut o, "mic_default");
    p.cam_default = boolean(d, &mut o, "cam_default");
    p.chat = bool_opt(d, &mut o, "chat").unwrap_or(true);
    if let Some(v) = o.take("max_file_bytes") {
        if let Some(n) = d.int(v, &o.at("max_file_bytes"), 0, i64::MAX) {
            if n != 0 && !(1024..=32 << 20).contains(&n) {
                d.error(
                    v.off,
                    &o.at("max_file_bytes"),
                    "a file is at least 1024 and at most 33554432 bytes",
                );
            }
            p.max_file_bytes = u64::try_from(n).unwrap_or(0);
        }
    }
    if let Some(v) = o.take("member_ttl") {
        if let Some(t) = d.duration(v, &o.at("member_ttl")) {
            if !t.is_zero() && t < Duration::from_secs(3) {
                d.error(
                    v.off,
                    &o.at("member_ttl"),
                    format!("{} is shorter than 3s", crate::duration::format(t)),
                );
            }
            p.member_ttl = t;
        }
    }
    p.stall_timeout = duration(d, &mut o, "stall_timeout");
    o.finish(d, MEDIA_PAGES_FIELDS, MEDIA_PAGES_FIT_ONLY);
    Some(p)
}

fn page_link(d: &mut Decoder<'_>, n: &Node, path: &str) -> Option<PageLink> {
    let mut o = d.object(n, path)?;
    let mut l = PageLink::default();
    if let Some(v) = o.require(d, "name") {
        l.name = d.string(v, &o.at("name")).unwrap_or_default();
    }
    if let Some(v) = o.require(d, "url") {
        l.url = d.string(v, &o.at("url")).unwrap_or_default();
    }
    o.finish(d, PAGE_LINK_FIELDS, &[]);
    let bad = |s: &str| s.contains(['\r', '\n', '\0', '<', '>', '"']);
    if l.name.is_empty()
        || l.name.chars().count() > 40
        || l.url.is_empty()
        || l.url.chars().count() > 512
        || bad(&l.name)
        || bad(&l.url)
    {
        d.error(
            n.off,
            path,
            "a link has a name of at most 40 characters and a URL of at most 512, without line breaks, <, > or quotes",
        );
    }
    Some(l)
}
