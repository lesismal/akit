//! The library and player pages' API (I148), as fit's `handler/mediapages/library.go`:
//! `api/library`, the libraries and the channels the viewer may play; `api/library/<name>`, a
//! page of a library's items the viewer may play, searched and paged; and `api/play`, the URL the
//! player plays an item or a channel from, signed for the viewer with the policy's
//! `url_signing`.
//!
//! The catalogs are read from the role `media_library`'s libraries and the channels from the
//! streaming role's manager, both of the handler's engine, without blocking: a viewer's password
//! a bcrypt hash must check is checked on the crate's pool while the request waits.

use std::fmt::Write as _;
use std::sync::{Arc, OnceLock};

use akio_http::{Context, Method};
use akit_config::media::{MAX_PAGES_LIBRARIES, MediaAction as Action, MediaPages};
use akit_runtime::ProvisionError;

use super::{body, json, object, plain, quote, quote_list, str_member};
use crate::auth::{Grant, Refusal};
use crate::http::{HandlerAuth, credentials, refuse};
use crate::library::catalog::{Catalog, Item, seconds_text};

/// The items a page lists when the request names no limit (fit's `DefaultPageItems`).
pub const DEFAULT_PAGE_ITEMS: usize = 60;
/// The most items a page lists (fit's `MaxPageItems`).
pub const MAX_PAGE_ITEMS: usize = 500;
const MAX_QUERY_BYTES: usize = 128;
const MAX_LISTED_CHANNELS: usize = 1000;
const PLAY_SESSION_BYTES: usize = 12;
const RETRY_AFTER: &str = "1";

/// What the library and player pages serve, compiled.
pub(crate) struct Library {
    /// The libraries listed, in the config's order.
    names: Vec<String>,
    /// An item's playlist, `{library}` and `{id}` in it.
    vod: String,
    /// List the streaming role's channels, played from `hls`.
    channels: bool,
    hls: String,
    /// The engine's registry key, once the engine is set.
    key: Arc<OnceLock<usize>>,
}

/// Checks `libraries`, `vod` and `channels` (fit's `provisionLibrary`): the library pages, and
/// `GET api/config`'s `library`; `None` when they are off. `vod`'s origin, when it is on another
/// server, joins `origins`.
pub(crate) fn provision(
    h: &MediaPages,
    key: Arc<OnceLock<usize>>,
    origins: &mut Vec<String>,
) -> Result<Option<(Library, String)>, ProvisionError> {
    if h.libraries.is_empty() && !h.channels {
        if !h.vod.is_empty() {
            return Err(ProvisionError::field(
                "vod",
                "vod is the URL of the libraries' items: name libraries too",
            ));
        }
        return Ok(None);
    }
    if h.libraries.len() > MAX_PAGES_LIBRARIES {
        return Err(ProvisionError::field(
            "libraries",
            format!(
                "{} libraries: at most {MAX_PAGES_LIBRARIES}",
                h.libraries.len()
            ),
        ));
    }
    let mut names: Vec<String> = Vec::new();
    for (i, name) in h.libraries.iter().enumerate() {
        if names.contains(name) {
            return Err(ProvisionError::field(
                &format!("libraries[{i}]"),
                format!("{name:?} is listed twice"),
            ));
        }
        names.push(name.clone());
    }
    let vod = h.vod_or().to_owned();
    if !names.is_empty() {
        match vod_template(&vod) {
            Ok(Some(o)) => {
                if !origins.contains(&o) {
                    origins.push(o);
                }
            }
            Ok(None) => {}
            Err(e) => return Err(ProvisionError::field("vod", e)),
        }
    }
    if h.channels && h.hls.is_empty() {
        return Err(ProvisionError::field(
            "channels",
            "channels are played from hls: set hls, as /hls/{stream}/index.m3u8",
        ));
    }
    let conf = format!(
        "{{\"channels\":{},\"libraries\":{}}}",
        h.channels,
        quote_list(&names)
    );
    Ok(Some((
        Library {
            names,
            vod,
            channels: h.channels,
            hls: h.hls.clone(),
            key,
        },
        conf,
    )))
}

/// Checks `vod`'s URL template (fit's `vodTemplate`): a path, or an http or https URL whose last
/// segment, a playlist's name, follows `{id}/`; its origin when it is on another server.
fn vod_template(v: &str) -> Result<Option<String>, String> {
    let bad = || {
        format!(
            "{} is not a path or an http or https URL of at most 512 characters ending in {{id}}/ and a playlist's name, as /vod/{{library}}/{{id}}/index.m3u8",
            quote(v)
        )
    };
    let Some(i) = v.rfind('/') else {
        return Err(bad());
    };
    let (dir, last) = v.split_at(i.saturating_add(1));
    if v.len() > 512
        || v.contains([' ', '\t', '\r', '\n', '\0', '<', '>', '"', '\'', '?', '#'])
        || !dir.ends_with("{id}/")
        || last.is_empty()
        || last.contains('{')
        || v.matches("{id}").count() != 1
    {
        return Err(bad());
    }
    if v.starts_with('/') && !v.starts_with("//") {
        return Ok(None);
    }
    let u = v.replace("{library}", "l").replace("{id}", "i");
    let Some((scheme, rest)) = u.split_once("://") else {
        return Err(bad());
    };
    // As Go's url.Parse reads it: the scheme without case, the host after any user information.
    let scheme = scheme.to_ascii_lowercase();
    if scheme != "http" && scheme != "https" {
        return Err(bad());
    }
    let authority = rest.split(['/', '?', '#']).next().unwrap_or("");
    let host = authority.rsplit_once('@').map_or(authority, |(_, h)| h);
    if host.is_empty() {
        return Err(bad());
    }
    Ok(Some(format!("{scheme}://{host}")))
}

/// Who a request of the library API is, and what it may play.
struct Viewer {
    subject: String,
    grant: Grant,
    /// Playing is public, or there is no policy.
    all: bool,
}

impl Viewer {
    fn may(&self, stream: &str) -> bool {
        self.all || self.grant.allows(Action::Play, stream)
    }
}

/// Finds the viewer of the request and calls `next` with it, or answers the request as the policy
/// refuses it: without credentials, 401 (fit's `whois`). A password a bcrypt hash must check is
/// checked on the crate's pool while the request waits.
fn whois(
    auth: &HandlerAuth,
    ctx: &mut Context,
    next: impl FnOnce(&mut Context, Viewer) + Send + 'static,
) {
    let all = Viewer {
        subject: String::new(),
        grant: Grant::default(),
        all: true,
    };
    if !auth.named() {
        return next(ctx, all);
    }
    let Some(policy) = auth.policy().cloned() else {
        return refuse(ctx, Refusal::Unavailable, "");
    };
    if policy.is_public(Action::Play) {
        return next(ctx, all);
    }
    let creds = credentials(ctx.req());
    let finish = |ctx: &mut Context, r: Result<(String, Grant), Refusal>| match r {
        Ok((subject, grant)) => next(
            ctx,
            Viewer {
                subject,
                grant,
                all: false,
            },
        ),
        Err(e) => refuse(ctx, e, ""),
    };
    match policy.whois_now(&creds) {
        Err(Refusal::Pending) => {}
        r => return finish(ctx, r),
    }
    let Some(s) = ctx.suspend() else {
        return refuse(ctx, Refusal::Busy, "");
    };
    policy.whois(&creds, move |r| s.resume(move |ctx| finish(ctx, r)));
}

impl Library {
    fn key(&self) -> Option<usize> {
        self.key.get().copied()
    }

    /// A library's catalog once it has been scanned.
    fn catalog(&self, name: &str) -> Option<Arc<Catalog>> {
        let l = crate::library::lookup(self.key()?, name)?;
        let cat = l.catalog();
        cat.scanned.is_some().then_some(cat)
    }

    fn channel_list(&self) -> Vec<crate::channels::ChannelStatus> {
        self.key()
            .and_then(crate::channels::lookup)
            .map(|m| m.channels())
            .unwrap_or_default()
    }

    /// The URL of an item's playlist, and its directory, where its poster is.
    fn playlist(&self, lib: &str, id: &str) -> (String, String) {
        let u = self.vod.replace("{library}", lib).replace("{id}", id);
        let dir = u
            .rfind('/')
            .and_then(|i| u.get(..=i))
            .unwrap_or_default()
            .to_owned();
        (u, dir)
    }
}

/// `api/library` and `api/library/<name>`; `rest` is what follows `library`.
pub(crate) fn library_api(
    lib: Option<&Arc<Library>>,
    auth: &HandlerAuth,
    ctx: &mut Context,
    rest: &str,
) {
    let Some(lib) = lib else {
        return plain(ctx, 404, "the pages have no library", &[]);
    };
    if *ctx.method() != Method::Get {
        return plain(ctx, 405, "the library takes GET", &[("Allow", "GET")]);
    }
    let name = rest.strip_prefix('/');
    if !rest.is_empty() && name.is_none_or(|n| !lib.names.iter().any(|x| x == n)) {
        return plain(ctx, 404, "no such library", &[]);
    }
    let lib = Arc::clone(lib);
    let name = name.map(str::to_owned);
    whois(auth, ctx, move |ctx, v| match name {
        None => overview(&lib, ctx, &v),
        Some(n) => items(&lib, ctx, &v, &n),
    });
}

/// The libraries, and the channels the viewer may play.
fn overview(lib: &Library, ctx: &mut Context, v: &Viewer) {
    let mut out = String::from("{");
    if !v.subject.is_empty() {
        let _ = write!(out, "\"subject\":{},", quote(&v.subject));
    }
    out.push_str("\"libraries\":[");
    for (i, name) in lib.names.iter().enumerate() {
        if i > 0 {
            out.push(',');
        }
        let _ = write!(out, "{{\"name\":{}", quote(name));
        match lib.catalog(name).and_then(|c| c.scanned) {
            Some(t) => {
                let secs = t
                    .duration_since(std::time::UNIX_EPOCH)
                    .map_or(0, |d| d.as_secs());
                let _ = write!(out, ",\"ready\":true,\"scanned\":{secs}}}");
            }
            None => out.push_str(",\"ready\":false}"),
        }
    }
    out.push_str("],\"channels\":[");
    if lib.channels {
        let mut n = 0usize;
        for ch in lib.channel_list() {
            if n == MAX_LISTED_CHANNELS {
                break;
            }
            if !v.may(&ch.name) {
                continue;
            }
            if n > 0 {
                out.push(',');
            }
            n = n.saturating_add(1);
            let _ = write!(
                out,
                "{{\"name\":{},\"state\":{}",
                quote(&ch.name),
                quote(ch.state)
            );
            for (k, it) in [("now", &ch.now), ("next", &ch.next)] {
                if let Some(it) = it.as_ref().filter(|it| !it.title.is_empty()) {
                    let _ = write!(out, ",\"{k}\":{}", quote(&it.title));
                }
            }
            out.push('}');
        }
    }
    out.push_str("]}");
    json(ctx, 200, out.into_bytes(), &[]);
}

/// A number parameter: `d` when empty.
fn int_param(s: Option<&str>, d: usize) -> Option<usize> {
    match s {
        None | Some("") => Some(d),
        Some(s) => s.parse::<i64>().ok().and_then(|n| usize::try_from(n).ok()),
    }
}

/// A page of a library's items the viewer may play, those that can be served, in ID order:
/// `offset` and `limit`, and `q`, matched without case in the ID or the title.
fn items(lib: &Library, ctx: &mut Context, v: &Viewer, name: &str) {
    let Some(cat) = lib.catalog(name) else {
        return plain(
            ctx,
            503,
            "the library has not been scanned yet",
            &[("Retry-After", RETRY_AFTER)],
        );
    };
    let query = ctx.req().query().unwrap_or("").to_owned();
    let q = |k: &str| crate::http::query_param(&query, k);
    let (offset, limit, search) = (q("offset"), q("limit"), q("q").unwrap_or_default());
    let offset = int_param(offset.as_deref(), 0);
    let limit = int_param(limit.as_deref(), DEFAULT_PAGE_ITEMS);
    let (Some(offset), Some(limit)) = (offset, limit) else {
        return bad_page(ctx);
    };
    if !(1..=MAX_PAGE_ITEMS).contains(&limit) || search.len() > MAX_QUERY_BYTES {
        return bad_page(ctx);
    }
    let search = search.to_lowercase();
    let prefix = format!("vod/{name}/");
    let mut total = 0usize;
    let mut listed = Vec::new();
    for it in &cat.items {
        if !it.error.is_empty()
            || !v.may(&format!("{prefix}{}", it.id))
            || (!search.is_empty()
                && !it.id.to_lowercase().contains(&search)
                && !it.title.to_lowercase().contains(&search))
        {
            continue;
        }
        total = total.saturating_add(1);
        if total <= offset || listed.len() == limit {
            continue;
        }
        listed.push(item_json(lib, name, it));
    }
    let out = format!(
        "{{\"library\":{},\"total\":{total},\"offset\":{offset},\"items\":[{}]}}",
        quote(name),
        listed.join(",")
    );
    json(ctx, 200, out.into_bytes(), &[]);
}

fn bad_page(ctx: &mut Context) {
    plain(
        ctx,
        400,
        &format!(
            "offset is 0 or more, limit 1 to {MAX_PAGE_ITEMS}, and q at most {MAX_QUERY_BYTES} bytes"
        ),
        &[],
    );
}

/// An item as the library page shows it.
fn item_json(lib: &Library, name: &str, it: &Item) -> String {
    let (mut w, mut h) = (0u32, 0u32);
    let mut renditions = Vec::new();
    for r in it.renditions.iter().filter(|r| r.error.is_empty()) {
        let area = |w: u32, h: u32| u64::from(w).saturating_mul(u64::from(h));
        if area(r.width, r.height) > area(w, h) {
            (w, h) = (r.width, r.height);
        }
        renditions.push(r.name.clone());
    }
    let mut out = format!(
        "{{\"id\":{},\"title\":{},\"duration\":{}",
        quote(&it.id),
        quote(&it.title),
        seconds_text(it.duration)
    );
    if w != 0 {
        let _ = write!(out, ",\"width\":{w}");
    }
    if h != 0 {
        let _ = write!(out, ",\"height\":{h}");
    }
    let _ = write!(out, ",\"renditions\":{}", quote_list(&renditions));
    if !it.audio.is_empty() {
        let _ = write!(out, ",\"audio\":{}", it.audio.len());
    }
    if !it.subtitles.is_empty() {
        let _ = write!(out, ",\"subtitles\":{}", it.subtitles.len());
    }
    if let Some(p) = &it.poster {
        let (_, dir) = lib.playlist(name, &it.id);
        let ext = std::path::Path::new(&p.path)
            .extension()
            .map(|e| format!(".{}", e.to_string_lossy().to_lowercase()))
            .unwrap_or_default();
        let _ = write!(out, ",\"poster\":{}", quote(&format!("{dir}poster{ext}")));
    }
    out.push('}');
    out
}

/// What `api/play` is asked: an item of a library, or a channel.
struct PlayRequest {
    library: String,
    id: String,
    channel: String,
}

/// `api/play`: the URL the player plays an item or a channel from, for the viewer. With the
/// policy's `url_signing`, and playing not public, a playback URL signed for the viewer, its
/// address and a session of its own, good for `url_signing`'s `ttl`; otherwise the plain URL,
/// which the login's cookie, or the page's token, lets the player in to.
pub(crate) fn play(
    lib: Option<&Arc<Library>>,
    auth: &HandlerAuth,
    max_body: usize,
    ctx: &mut Context,
) {
    let Some(lib) = lib else {
        return plain(ctx, 404, "the pages have no library", &[]);
    };
    if *ctx.method() != Method::Post {
        return plain(ctx, 405, "play takes POST", &[("Allow", "POST")]);
    }
    let Some(b) = body(ctx, max_body) else {
        return;
    };
    let parsed = object(&b).and_then(|o| {
        Some(PlayRequest {
            library: str_member(&o, "library").ok()?,
            id: str_member(&o, "id").ok()?,
            channel: str_member(&o, "channel").ok()?,
        })
    });
    let Some(req) =
        parsed.filter(|r| r.channel.is_empty() != (r.library.is_empty() || r.id.is_empty()))
    else {
        return plain(
            ctx,
            400,
            "the body must be JSON: {\"library\", \"id\"} or {\"channel\"}",
            &[],
        );
    };
    let lib = Arc::clone(lib);
    let a = auth.clone();
    whois(auth, ctx, move |ctx, v| play_for(&lib, &a, ctx, &v, &req));
}

fn play_for(lib: &Library, auth: &HandlerAuth, ctx: &mut Context, v: &Viewer, req: &PlayRequest) {
    let (stream, url, title, kind) = if req.channel.is_empty() {
        if !lib.names.contains(&req.library) {
            return plain(ctx, 404, "no such library", &[]);
        }
        let Some(cat) = lib.catalog(&req.library) else {
            return plain(
                ctx,
                503,
                "the library has not been scanned yet",
                &[("Retry-After", RETRY_AFTER)],
            );
        };
        let Some(it) = cat.item(&req.id).filter(|it| it.error.is_empty()) else {
            return plain(ctx, 404, "no such item", &[]);
        };
        let (u, _) = lib.playlist(&req.library, &it.id);
        (
            format!("vod/{}/{}", req.library, it.id),
            u,
            it.title.clone(),
            "vod",
        )
    } else {
        if !lib.channels || !lib.channel_list().iter().any(|c| c.name == req.channel) {
            return plain(ctx, 404, "no such channel", &[]);
        }
        (
            req.channel.clone(),
            lib.hls.replace("{stream}", &req.channel),
            req.channel.clone(),
            "channel",
        )
    };
    if !v.may(&stream) {
        return refuse(ctx, Refusal::Forbidden, "");
    }
    let mut out_url = url.clone();
    let mut expires = None;
    let policy = auth.policy().filter(|p| !v.all && p.url_signing());
    if let Some(p) = policy {
        let path = path_of(&url);
        let Some(res) = crate::urlsign::resource_of(&path) else {
            return plain(
                ctx,
                500,
                &format!(
                    "the URL {} is not one a playback URL is signed for",
                    quote(&path)
                ),
                &[],
            );
        };
        let ttl = i64::try_from(p.url_ttl().as_secs()).unwrap_or(i64::MAX);
        let expiry = crate::urlsign::now_secs().saturating_add(ttl);
        let xff: Vec<&str> = ctx.req().headers.get_all("x-forwarded-for").collect();
        let viewer = crate::auth::Viewer {
            subject: v.subject.clone(),
            address: p.viewer_addr(ctx.peer().ip(), &xff),
            session: new_session(),
        };
        match p.sign_url(&res, &viewer, expiry, "") {
            Ok(query) => {
                let sep = if url.contains('?') { '&' } else { '?' };
                out_url = format!("{url}{sep}{query}");
                expires = Some(expiry);
            }
            Err(e) => {
                return plain(
                    ctx,
                    503,
                    &format!("the playback URL could not be signed: {e}"),
                    &[("Retry-After", RETRY_AFTER)],
                );
            }
        }
    }
    let mut out = format!("{{\"url\":{}", quote(&out_url));
    if let Some(e) = expires {
        let _ = write!(out, ",\"expires\":{e}");
    }
    let _ = write!(
        out,
        ",\"signed\":{},\"title\":{},\"kind\":{}}}",
        expires.is_some(),
        quote(&title),
        quote(kind)
    );
    json(ctx, 200, out.into_bytes(), &[]);
}

/// The path of a URL: itself for a path, the part after the host for an http or https URL.
fn path_of(u: &str) -> String {
    let Some((_, rest)) = u.split_once("://") else {
        return u.split(['?', '#']).next().unwrap_or("").to_owned();
    };
    rest.find('/')
        .and_then(|i| rest.get(i..))
        .map_or_else(String::new, |p| {
            p.split(['?', '#']).next().unwrap_or("").to_owned()
        })
}

/// A new playback session's ID: random bytes, in base64url without padding.
fn new_session() -> String {
    let mut b = [0u8; PLAY_SESSION_BYTES];
    if akio_crypto::rand::fill_random(&mut b).is_err() {
        let t = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |d| d.as_nanos());
        let tb = t.to_be_bytes();
        b.copy_from_slice(tb.get(4..).unwrap_or(&[0; PLAY_SESSION_BYTES]));
    }
    akio_crypto::base64::URL_SAFE_NO_PAD.encode(b)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// fit's `TestLibraryParams`' templates.
    #[test]
    fn vod_templates() {
        for (v, want) in [
            ("/vod/{library}/{id}/index.m3u8", Ok(None)),
            ("/v/{id}/master.m3u8", Ok(None)),
            (
                "https://cdn.example/vod/{library}/{id}/index.m3u8",
                Ok(Some("https://cdn.example".to_owned())),
            ),
        ] {
            assert_eq!(vod_template(v), want, "{v}");
        }
        for bad in [
            "",
            "/vod/{library}/{id}",
            "/vod/{library}/{id}/",
            "/vod/{id}/{id}/index.m3u8",
            "/vod/{library}/{id}/{x}.m3u8",
            "/vod/{library}/{id}/index.m3u8?x=1",
            "ftp://h/{id}/index.m3u8",
            "//h/{id}/index.m3u8",
            "vod/{id}/index.m3u8",
        ] {
            assert!(vod_template(bad).is_err(), "{bad:?}");
        }
    }

    #[test]
    fn paths() {
        assert_eq!(path_of("/vod/a/b/index.m3u8"), "/vod/a/b/index.m3u8");
        assert_eq!(
            path_of("https://h:1/vod/a/index.m3u8?x"),
            "/vod/a/index.m3u8"
        );
        assert_eq!(path_of("https://h"), "");
    }
}
