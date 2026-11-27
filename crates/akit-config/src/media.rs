//! fit's media members of the config (I120): `media_auth`, the named policies of who may publish
//! and play which streams (fit's I115, `config/mediaauth.go`), and `streaming`, the streaming
//! role's hub and its RTMP, RTSP, SRT and WebRTC listeners (fit's I116, `config/streaming.go`),
//! with fit's fields, defaults, bounds and checks.
//!
//! The members are read as strictly as the rest of the config. What is left out is kept empty
//! (a string), zero (a number, a duration) or `None`; the methods give fit's defaults for it.
//! What fit reads here and akit does not run yet is refused as not supported yet: a policy's
//! `keys` and `url_signing` (fit's I119 and the media library's track), the role's `channels`
//! and `push` (the media library's track), Multipath TCP, RTMP's `max_message_bytes` and RTSP's
//! `auth_methods`, `realm` and `idle_timeout` (akio requests A263 and A264).

use std::time::Duration;

use crate::decode::{Decoder, Obj};
use crate::error::{join_field, join_index};
use crate::json::{Kind, Node};
use crate::model::Access;
use crate::roles::{access_of, check_listen, count, duration, list, required, text, text_at};

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
    "name",
    "tokens",
    "users",
    "secret",
    "secret_file",
    "public",
    "token_ttl",
];
// keys: fit's I119 (media_tokens' keyring); url_signing: the media library's track.
const MEDIA_AUTH_FIT_ONLY: &[&str] = &["keys", "url_signing"];
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
];
// The media library's track.
const STREAMING_FIT_ONLY: &[&str] = &["channels", "push"];
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
        && m.public.is_empty()
    {
        d.error(
            n.off,
            path,
            "the policy lets no one in: set tokens, users, secret, secret_file or public",
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
    o.finish(d, STREAMING_FIELDS, STREAMING_FIT_ONLY);
    if s.rtmp.is_none() && s.rtsp.is_none() && s.srt.is_none() && s.webrtc.is_none() {
        let fit_only = matches!(&n.kind, Kind::Object(ms) if ms.iter().any(|m| STREAMING_FIT_ONLY.contains(&m.key.as_str())));
        // A role of channels alone is the media library's track: refused as not supported yet.
        if !fit_only {
            d.error(
                n.off,
                path,
                "nothing takes streams in: set rtmp, rtsp, srt, webrtc or channels",
            );
        }
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
    for r in refs {
        if !policies.iter().any(|p| p.name == r.name) {
            d.error(
                r.off,
                &r.path,
                format!("there is no media_auth policy named {:?}", r.name),
            );
        }
    }
}

#[cfg(test)]
mod tests;
