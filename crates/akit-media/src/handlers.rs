//! The handlers that play the streaming role's streams over HTTP (I123), as fit's
//! `streaming/handlers.go`: `httpflv` (HTTP-FLV, and WebSocket-FLV to an upgrade) on akio-media's
//! `HttpFlv`, and `hls` (fMP4 or MPEG-TS segments, Low-Latency HLS optional) on its `Hls`.
//!
//! A handler finds the role and its policy by the engine once the engine is there
//! ([`akit_upstream::EngineSlot::when_set`]), and reads them per request without a lock: the
//! policy through its per-thread cache, the hub through the role's slot. It answers 503 while the
//! role does not run. Every request is checked by `auth` ([`crate::http::HandlerAuth`], refused as
//! unavailable while the policy is not there, never let on unchecked), its tokens
//! written `REDACTED` in the target for the access log. A playlist fetched with `?token=` names
//! what it lists with the token too, so that a player that sends no header (Safari, ffmpeg) plays
//! on.
//!
//! akio-media's services read the stream from the request's path: a handler with a `prefix`
//! hands them the path without it (the access log then writes the path so), and `hls` keeps one
//! `Hls` a format (akio request A266). An `hls` handler's segments are shared by the handlers of
//! the same params, so that a reload that keeps them keeps its segments and their numbering.
//!
//! A channel with `encrypt` (I147) is served by `Hls` services of its own, whose segments and
//! parts are encrypted with AES-128 (akio-media's `HlsEncryption`) under keys of the channel's
//! library, a key every `key_rotation` segments, derived as fit derives them from the stream, a
//! session of the muxer's own and the key's number ([`ChannelKeys`]); akio-media serves the keys
//! beside the segments (`key-<n>.key`, `?format=ts` added for MPEG-TS's, so that the handler
//! routes a key to the muxer that issued it), to a request the handler's `auth` let in, and an
//! `hls` handler without `auth` serves none. A channel whose encryption changes (a reload, its library's secret) gets
//! services of the new encryption at the next request, never one served clear.

use std::collections::HashMap;
use std::sync::{Arc, Mutex, OnceLock, PoisonError};
use std::time::Duration;

use akio_http::{Context, Response, Service};
use akio_media::hls::{
    Hls as AkioHls, HlsConfig, HlsEncryption, HlsKey, KeyProvider, SegmentFormat,
};
use akio_media::httpflv::{HttpFlv as AkioFlv, HttpFlvConfig};
use akit_config::media::HlsFormat;
use akit_runtime::{
    Handler, HandlerParams, Info, Provision, ProvisionError, Provisioned, set_request_path,
};

use crate::auth::registry;
use crate::http::{Access, HandlerAuth, redact};
use crate::streaming::{Slot, lookup};
use crate::swap::Swap;

/// The handler modules of this crate, for [`akit_runtime::Registry::new`].
pub static MODULES: &[Info<HandlerParams>] = &[
    Info::handler(
        "http.handlers.httpflv",
        "Plays the streaming role's streams as HTTP-FLV, and as WebSocket-FLV to a WebSocket upgrade: /live/cam.flv plays live/cam.",
        new_flv,
    ),
    Info::handler(
        "http.handlers.hls",
        "Plays the streaming role's streams over HLS, fMP4 or MPEG-TS segments, Low-Latency HLS optional: /live/cam/index.m3u8 plays live/cam.",
        new_hls,
    ),
    Info::handler(
        "http.handlers.stream_whip",
        "Takes WebRTC streams published with WHIP (RFC 9725) into the streaming role's hub, which plays them over every protocol.",
        crate::rtc::new_whip,
    ),
    Info::handler(
        "http.handlers.stream_whep",
        "Plays the streaming role's streams to WebRTC viewers with WHEP, whatever they were published over.",
        crate::rtc::new_whep,
    ),
    Info::handler(
        "http.handlers.vod",
        "Serves a media library's items as video on demand over HLS, fMP4 or MPEG-TS segments cut from the files as they are stored, and the files themselves with Range.",
        crate::vod::new_vod,
    ),
    Info::handler(
        "http.handlers.media_tokens",
        "Issues time-limited media_auth tokens in batches, for a route a business backend calls, and revokes the tokens of one key of the policy's keyring, or of all of them, by rotating their secrets.",
        crate::tokens::new_tokens,
    ),
    Info::handler(
        "http.handlers.media_pages",
        "Serves akit's media pages under a prefix: an index with a login, a room call, publish and play pages over WHIP and WHEP, and the room API through which the members of a call announce their microphone and camera and talk in its chat.",
        crate::pages::new_pages,
    ),
];

pub use akit_config::media::valid_stream;

/// Answers with a line of text, the request's tokens written `REDACTED` first, as every answer.
fn text(ctx: &mut Context, status: u16, msg: &str) {
    redact(ctx);
    ctx.respond(Response::with_body(
        status,
        "text/plain; charset=utf-8",
        format!("{msg}\n").into_bytes(),
    ));
}

/// What a handler finds once the engine is there.
struct Found {
    role: Arc<Slot>,
    hls: Option<Arc<HlsShared>>,
}

/// What both handlers share: their prefix, policy and role.
struct Base {
    prefix: String,
    cors: String,
    auth: HandlerAuth,
    found: Arc<OnceLock<Found>>,
}

impl Base {
    /// The engine's role and policy, looked for once the engine is there; `hls` the shared
    /// segments of an `hls` handler's params.
    fn new(
        p: &Provision<'_, HandlerParams>,
        prefix: &str,
        cors: &str,
        auth: (&str, Option<&akit_config::media::MediaAuth>),
        hls: Option<(String, HlsConfig, SegmentFormat)>,
    ) -> Result<Base, ProvisionError> {
        if !prefix.starts_with('/') {
            return Err(ProvisionError::new(format!(
                "prefix: {prefix:?} does not start with /"
            )));
        }
        let auth = HandlerAuth::new(p.env().engine(), auth.0, auth.1);
        let found = Arc::new(OnceLock::new());
        let f = Arc::clone(&found);
        p.env().engine().when_set(move |engine| {
            let key = registry::key(engine);
            let hls = hls.map(|(params, cfg, def)| shared(key, params, cfg, def));
            let _ = f.set(Found {
                role: lookup(key),
                hls,
            });
        });
        Ok(Base {
            prefix: prefix.to_owned(),
            cors: cors.to_owned(),
            auth,
            found,
        })
    }

    /// The path after the prefix, or `None`.
    fn rest<'p>(&self, path: &'p str) -> Option<&'p str> {
        path.strip_prefix(self.prefix.as_str())
    }
}

// ---- httpflv ----

struct Flv {
    base: Base,
    cfg: HttpFlvConfig,
    /// akio-media's service of the role's current hub, by the hub's number.
    cur: Swap<(u64, AkioFlv)>,
    make: Mutex<()>,
}

fn new_flv(p: &Provision<'_, HandlerParams>) -> Result<Provisioned<dyn Handler>, ProvisionError> {
    let HandlerParams::HttpFlv(h) = p.params() else {
        return Err(ProvisionError::new(
            "the httpflv handler's params are not httpflv's",
        ));
    };
    let mut cfg = HttpFlvConfig::default();
    cfg.play_wait = (!h.wait.is_zero()).then_some(h.wait);
    cfg.cors_origin = (!h.cors.is_empty()).then(|| h.cors.clone());
    let base = Base::new(p, &h.prefix, &h.cors, (&h.auth, h.policy.as_ref()), None)?;
    Ok(Provisioned::new(Arc::new(Flv {
        base,
        cfg,
        cur: Swap::default(),
        make: Mutex::new(()),
    }) as Arc<dyn Handler>))
}

impl Flv {
    /// The stream a path names: after the prefix, before `.flv`.
    fn stream<'p>(&self, path: &'p str) -> Option<&'p str> {
        self.base
            .rest(path)
            .and_then(|r| r.strip_suffix(".flv"))
            .filter(|s| valid_stream(s))
    }

    /// akio-media's service of the hub numbered `hub_no`, made anew when the role's hub is another.
    fn service(&self, hub_no: u64, hub: &akio_media::Hub) -> AkioFlv {
        let have = self
            .cur
            .with(|c| c.filter(|(g, _)| *g == hub_no).map(|(_, f)| f.clone()));
        if let Some(f) = have {
            return f;
        }
        let _g = self.make.lock().unwrap_or_else(PoisonError::into_inner);
        let f = AkioFlv::new(hub.clone(), self.cfg.clone());
        self.cur.set(Some(Arc::new((hub_no, f.clone()))));
        f
    }
}

impl Service for Flv {
    fn call(&self, ctx: &mut Context) {
        let Some(stream) = self.stream(ctx.path()).map(str::to_owned) else {
            return text(ctx, 404, "no stream in the path: /<stream>.flv");
        };
        let Some(found) = self.base.found.get() else {
            return text(ctx, 503, "the streaming role is not running");
        };
        let Some((hub_no, hub)) = found.role.with(|c| c.map(|c| (c.hub_no, c.hub.clone()))) else {
            return text(ctx, 503, "the streaming role is not running");
        };
        let flv = self.service(hub_no, &hub);
        let unprefixed = self.base.prefix != "/";
        let checked = stream.clone();
        // The stream's resource, as a signed URL is signed for: /live/cam/ for /live/cam.flv.
        let path = ctx.path();
        let resource = format!("{}/", path.strip_suffix(".flv").unwrap_or(path));
        self.base.auth.guard_resource(
            ctx,
            &checked,
            &resource,
            &self.base.cors,
            move |ctx, _: Access| {
                if unprefixed {
                    set_request_path(ctx, &format!("/{stream}.flv"));
                }
                flv.call(ctx);
            },
        );
    }
}

impl Handler for Flv {}

// ---- hls ----

/// The segments of one `hls` params on one engine, shared by the handlers that have them
/// (fit's `hlsShared`).
struct HlsShared {
    key: (usize, String),
    cfg: HlsConfig,
    def: SegmentFormat,
    refs: Mutex<usize>,
    /// akio-media's services of the role's current hub, by the hub's number: fMP4 and MPEG-TS.
    cur: Swap<(u64, AkioHls, AkioHls)>,
    make: Mutex<()>,
    /// The services of the channels with `encrypt` (I147), by stream: the encryption they were
    /// made with, and the two services.
    enc: Mutex<HashMap<String, (EncWant, AkioHls, AkioHls)>>,
}

/// The encryption services were made with: the hub's number, the library, its keys' ID and the
/// rotation.
#[derive(Debug, Clone, PartialEq, Eq)]
struct EncWant {
    hub_no: u64,
    library: String,
    keys: String,
    rotation: u32,
}

/// The keys of a channel's encrypted HLS (fit's `hlsShared.encrypt`): key `n` of a session
/// derived from the library's secret, as fit derives it, of `<stream>/<session>` and `n`. A
/// muxer that starts again starts a session of its own, so that its keys and IVs never repeat.
///
/// The MPEG-TS service's keys are named `key-<n>.key?format=ts`, so that a key request reaches
/// the muxer that issued it: both formats mux the stream, each with keys of its own.
struct ChannelKeys {
    keys: Arc<crate::library::keys::Keys>,
    sessions: Mutex<HashMap<String, String>>,
    ts: bool,
}

impl ChannelKeys {
    fn new_session() -> String {
        let mut b = [0u8; 8];
        if akio_crypto::rand::fill_random(&mut b).is_err() {
            let t = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map_or(0, |d| d.as_nanos());
            b.copy_from_slice(t.to_be_bytes().get(8..).unwrap_or(&[0; 8]));
        }
        crate::library::catalog::hex(&b)
    }
}

impl KeyProvider for ChannelKeys {
    fn key(&self, path: &str, n: u64) -> HlsKey {
        let mut m = self.sessions.lock().unwrap_or_else(PoisonError::into_inner);
        if n == 0 || !m.contains_key(path) {
            m.insert(path.to_owned(), ChannelKeys::new_session());
        }
        let session = m.get(path).cloned().unwrap_or_default();
        drop(m);
        let key = self.keys.key(
            crate::library::keys::KEY_CHANNEL,
            &format!("{path}/{session}"),
            n,
        );
        if self.ts {
            HlsKey::with_uri(key, format!("key-{n}.key?format=ts"))
        } else {
            HlsKey::new(key)
        }
    }
}

type SharedMap = Mutex<HashMap<(usize, String), Arc<HlsShared>>>;

fn shared_map() -> &'static SharedMap {
    static MAP: std::sync::LazyLock<SharedMap> =
        std::sync::LazyLock::new(|| Mutex::new(HashMap::new()));
    &MAP
}

/// The shared segments of `params` on the engine `key`, one more handler holding them.
fn shared(key: usize, params: String, cfg: HlsConfig, def: SegmentFormat) -> Arc<HlsShared> {
    let mut m = shared_map().lock().unwrap_or_else(PoisonError::into_inner);
    let k = (key, params);
    let sh = Arc::clone(m.entry(k.clone()).or_insert_with(|| {
        Arc::new(HlsShared {
            key: k,
            cfg,
            def,
            refs: Mutex::new(0),
            cur: Swap::default(),
            make: Mutex::new(()),
            enc: Mutex::new(HashMap::new()),
        })
    }));
    {
        let mut refs = sh.refs.lock().unwrap_or_else(PoisonError::into_inner);
        *refs = refs.saturating_add(1);
    }
    sh
}

/// One handler fewer holds `sh`: dropped with the last.
fn release(sh: &HlsShared) {
    let mut m = shared_map().lock().unwrap_or_else(PoisonError::into_inner);
    let mut refs = sh.refs.lock().unwrap_or_else(PoisonError::into_inner);
    *refs = refs.saturating_sub(1);
    if *refs == 0 {
        m.remove(&sh.key);
    }
}

impl HlsShared {
    fn services(&self, hub_no: u64, hub: &akio_media::Hub) -> (AkioHls, AkioHls) {
        let have = self.cur.with(|c| {
            c.filter(|(g, _, _)| *g == hub_no)
                .map(|(_, a, b)| (a.clone(), b.clone()))
        });
        if let Some(s) = have {
            return s;
        }
        let _g = self.make.lock().unwrap_or_else(PoisonError::into_inner);
        if let Some(s) = self.cur.get().filter(|c| c.0 == hub_no) {
            return (s.1.clone(), s.2.clone());
        }
        let mut fmp4 = self.cfg.clone();
        fmp4.format = SegmentFormat::Fmp4;
        let mut ts = self.cfg.clone();
        ts.format = SegmentFormat::MpegTs;
        let s = (
            AkioHls::new(hub.clone(), fmp4),
            AkioHls::new(hub.clone(), ts),
        );
        self.cur
            .set(Some(Arc::new((hub_no, s.0.clone(), s.1.clone()))));
        s
    }

    /// The services of `stream`: those of the channel's encryption when it encrypts (made anew
    /// when it changed), else the handler's; `Err` with why for a channel whose library or keys
    /// are not loaded.
    fn services_of(
        &self,
        hub_no: u64,
        hub: &akio_media::Hub,
        channels: Option<&Arc<crate::channels::Manager>>,
        stream: &str,
    ) -> Result<(AkioHls, AkioHls), &'static str> {
        let enc = channels.and_then(|m| m.encryption(stream));
        let Some((library, rotation)) = enc else {
            let mut m = self.enc.lock().unwrap_or_else(PoisonError::into_inner);
            if !m.is_empty() {
                // A channel no longer encrypted: its services go.
                m.remove(stream);
            }
            drop(m);
            return Ok(self.services(hub_no, hub));
        };
        let keys = crate::library::lookup(self.key.0, &library)
            .and_then(|l| l.keys())
            .ok_or("the channel's media library, and its keys, are not loaded")?;
        let want = EncWant {
            hub_no,
            library,
            keys: keys.id().to_owned(),
            rotation,
        };
        let mut m = self.enc.lock().unwrap_or_else(PoisonError::into_inner);
        if let Some((w, a, b)) = m.get(stream) {
            if *w == want {
                return Ok((a.clone(), b.clone()));
            }
        }
        let make = |format: SegmentFormat| {
            let mut c = self.cfg.clone();
            c.format = format;
            let mut e = HlsEncryption::new(NoKeys).rotate_every(rotation);
            e.keys = Arc::new(ChannelKeys {
                keys: Arc::clone(&keys),
                sessions: Mutex::new(HashMap::new()),
                ts: format == SegmentFormat::MpegTs,
            });
            c.encryption = Some(e);
            AkioHls::new(hub.clone(), c)
        };
        let s = (make(SegmentFormat::Fmp4), make(SegmentFormat::MpegTs));
        m.insert(stream.to_owned(), (want, s.0.clone(), s.1.clone()));
        Ok(s)
    }
}

/// A provider never asked for a key: replaced at once by the channel's.
struct NoKeys;

impl KeyProvider for NoKeys {
    fn key(&self, _path: &str, _n: u64) -> HlsKey {
        HlsKey::new([0; 16])
    }
}

struct HlsHandler {
    base: Base,
}

fn new_hls(p: &Provision<'_, HandlerParams>) -> Result<Provisioned<dyn Handler>, ProvisionError> {
    let HandlerParams::Hls(h) = p.params() else {
        return Err(ProvisionError::new(
            "the hls handler's params are not hls's",
        ));
    };
    let mut cfg = HlsConfig::default();
    cfg.target_duration = h.target();
    cfg.part_duration = (!h.part_duration.is_zero()).then_some(h.part_duration);
    cfg.playlist_size = h.playlist();
    cfg.max_bytes = usize::try_from(h.bytes()).unwrap_or(usize::MAX);
    cfg.play_wait = (!h.play_wait.is_zero()).then_some(h.play_wait);
    cfg.idle_timeout = h.idle();
    cfg.cors_origin = (!h.cors.is_empty()).then(|| h.cors.clone());
    let def = match h.format {
        HlsFormat::Fmp4 => SegmentFormat::Fmp4,
        HlsFormat::Ts => SegmentFormat::MpegTs,
    };
    // fit's key of the shared handler: the params that shape the segments.
    let params = format!(
        "{:?} {:?} {:?} {:?} {} {} {:?} {:?} {:?}",
        h.prefix,
        h.format,
        cfg.target_duration,
        cfg.part_duration,
        cfg.playlist_size,
        cfg.max_bytes,
        cfg.play_wait,
        cfg.idle_timeout,
        h.cors
    );
    let base = Base::new(
        p,
        &h.prefix,
        &h.cors,
        (&h.auth, h.policy.as_ref()),
        Some((params, cfg, def)),
    )?;
    let found = Arc::clone(&base.found);
    Ok(
        Provisioned::new(Arc::new(HlsHandler { base }) as Arc<dyn Handler>).on_cleanup(move || {
            if let Some(sh) = found.get().and_then(|f| f.hls.as_ref()) {
                release(sh);
            }
        }),
    )
}

/// `uri` with `query` added, unless it has the query's first parameter already.
fn add_query(uri: &str, query: &str, first: &str) -> String {
    if uri.is_empty() || uri.contains(&format!("{first}=")) {
        return uri.to_owned();
    }
    let sep = if uri.contains('?') { '&' } else { '?' };
    format!("{uri}{sep}{query}")
}

/// `playlist` with `query` added to the query of every URI it names: its lines that are not
/// tags, and the `URI` attributes of its tags (fit's `withQuery`).
#[must_use]
pub fn with_query(playlist: &str, query: &str) -> String {
    let first = query.split_once('=').map_or(query, |(k, _)| k);
    let mut out = String::with_capacity(playlist.len().saturating_add(256));
    for (i, line) in playlist.split('\n').enumerate() {
        if i > 0 {
            out.push('\n');
        }
        let l = line.trim_end_matches('\r');
        let cr = &line[l.len()..];
        if l.is_empty() {
            out.push_str(line);
        } else if l.starts_with('#') {
            let mut rest = l;
            while let Some(at) = rest.find("URI=\"") {
                let (head, tail) = rest.split_at(at.saturating_add(5));
                out.push_str(head);
                let end = tail.find('"').unwrap_or(tail.len());
                let (uri, after) = tail.split_at(end);
                out.push_str(&add_query(uri, query, first));
                rest = after;
            }
            out.push_str(rest);
            out.push_str(cr);
        } else {
            out.push_str(&add_query(l, query, first));
            out.push_str(cr);
        }
    }
    out
}

/// `s` escaped as a query value, as Go's `url.QueryEscape`.
pub(crate) fn query_escape(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for b in s.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(b as char);
            }
            b' ' => out.push('+'),
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}

impl Service for HlsHandler {
    fn call(&self, ctx: &mut Context) {
        let path = ctx.path().to_owned();
        let parsed = self.base.rest(&path).and_then(|rest| {
            let (stream, name) = rest.rsplit_once('/')?;
            valid_stream(stream).then(|| (stream.to_owned(), name.to_owned()))
        });
        let Some((stream, name)) = parsed else {
            return text(ctx, 404, "no stream in the path: /<stream>/index.m3u8");
        };
        let Some(found) = self.base.found.get() else {
            return text(ctx, 503, "the streaming role is not running");
        };
        let (Some((hub_no, hub, channels)), Some(sh)) = (
            found
                .role
                .with(|c| c.map(|c| (c.hub_no, c.hub.clone(), c.channels.clone()))),
            found.hls.clone(),
        ) else {
            return text(ctx, 503, "the streaming role is not running");
        };
        // I147: an encrypted channel's keys go only to whom auth lets play it.
        let key = name.starts_with("key-") && name.ends_with(".key");
        if key
            && !self.base.auth.named()
            && channels
                .as_ref()
                .is_some_and(|m| m.encryption(&stream).is_some())
        {
            return text(
                ctx,
                403,
                "the keys of an encrypted channel are served only by an hls handler with auth",
            );
        }
        let unprefixed = self.base.prefix != "/";
        let checked = stream.clone();
        // The stream's resource, as a signed URL is signed for: the path up to the playlist's or
        // segment's name.
        let resource = path
            .rfind('/')
            .and_then(|i| path.get(..=i))
            .unwrap_or("/")
            .to_owned();
        self.base.auth.guard_resource(
            ctx,
            &checked,
            &resource,
            &self.base.cors,
            move |ctx, access: Access| {
                let (fmp4, ts) = match sh.services_of(hub_no, &hub, channels.as_ref(), &stream) {
                    Ok(s) => s,
                    Err(why) => {
                        redact(ctx);
                        return ctx.respond(
                            Response::with_body(
                                503,
                                "text/plain; charset=utf-8",
                                format!("{why}\n").into_bytes(),
                            )
                            .header("Retry-After", "1"),
                        );
                    }
                };
                let playlist = name.ends_with(".m3u8");
                let format = if playlist || key {
                    match ctx.req().query_param("format") {
                        Some("ts") => SegmentFormat::MpegTs,
                        Some("fmp4") => SegmentFormat::Fmp4,
                        _ => sh.def,
                    }
                } else if name.ends_with(".ts") {
                    SegmentFormat::MpegTs
                } else if name.ends_with(".m4s") || name.ends_with(".mp4") {
                    SegmentFormat::Fmp4
                } else {
                    sh.def
                };
                // A playlist fetched with a token hands it on to what it names; one fetched with a
                // signed URL, a grant that lives play_grace past the URL.
                let (query, cookie) = if playlist {
                    access.pass_on(Duration::ZERO)
                } else {
                    (String::new(), String::new())
                };
                if !query.is_empty() || !cookie.is_empty() {
                    ctx.add_response_filter(move |resp, _| {
                        if resp.status != 200 {
                            return;
                        }
                        if !query.is_empty() {
                            if let Ok(body) = std::str::from_utf8(&resp.body) {
                                resp.body = with_query(body, &query).into_bytes();
                            }
                        }
                        if !cookie.is_empty() {
                            resp.headers.append("Set-Cookie", &cookie);
                        }
                    });
                }
                if unprefixed {
                    set_request_path(ctx, &format!("/{stream}/{name}"));
                }
                match format {
                    SegmentFormat::MpegTs => ts.call(ctx),
                    _ => fmp4.call(ctx),
                }
            },
        );
    }
}

impl Handler for HlsHandler {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn streams() {
        for good in ["live/cam", "a", "a.b/c_d~e-f"] {
            assert!(valid_stream(good), "{good}");
        }
        for bad in [
            "",
            "/a",
            "a/",
            "a//b",
            "a/../b",
            ".",
            "a b",
            &"a".repeat(257),
        ] {
            assert!(!valid_stream(bad), "{bad:?}");
        }
    }

    /// fit's `TestWithQuery`.
    #[test]
    fn playlists_carry_the_token() {
        let pl = "#EXTM3U\r\n#EXT-X-MAP:URI=\"init-0.mp4\"\r\n#EXTINF:2.0,\r\nseg-1.m4s\r\n#EXT-X-PRELOAD-HINT:TYPE=PART,URI=\"part-2.0.m4s?x=1\"\r\nseg-2.m4s?token=old\r\n";
        assert_eq!(
            with_query(pl, "token=a%2Bb"),
            "#EXTM3U\r\n#EXT-X-MAP:URI=\"init-0.mp4?token=a%2Bb\"\r\n#EXTINF:2.0,\r\nseg-1.m4s?token=a%2Bb\r\n#EXT-X-PRELOAD-HINT:TYPE=PART,URI=\"part-2.0.m4s?x=1&token=a%2Bb\"\r\nseg-2.m4s?token=old\r\n"
        );
        assert_eq!(query_escape("a+b /c"), "a%2Bb+%2Fc");
    }
}
