//! The handler `media_pages` (I125), as fit's `handler/mediapages`: akit's media pages under a
//! prefix, built into the binary (fit's, renamed: `tools/media-pages.py`), and the API behind them.
//! Under its prefix it serves an index, a room call over WHIP and WHEP, a publish page and a play
//! page; a login against a `media_auth` policy's users, which answers a token the policy signs,
//! set as the cookie `akit_media_token` too; and a room API ([`rooms`]), through which the
//! members of a call announce whether they publish and their microphone and camera, learn the
//! others' with Server-Sent Events, and talk in the room's chat ([`chat`]).
//!
//! With `libraries` and `channels` (I148), its library and player pages list the media
//! libraries' items and the streaming role's channels the viewer may play, and play them through
//! URLs signed for the viewer with the policy's `url_signing` ([`library`]); without them the
//! API's `library` and `play` answer 404, as fit's do.
//!
//! Nothing blocks an I/O thread: the room API's state is in memory under short locks, its events
//! are written without waiting (a browser that does not take them is sent only the latest state
//! once it has taken the last, its chat messages wait in a bounded queue that cuts the stream when
//! it overflows, and a browser that takes nothing for `stall_timeout` is cut), and a bcrypt
//! login runs on the crate's pool. Every part is bounded: rooms, members a room, watchers a room,
//! messages a room, the bytes of files across the rooms, a request's body, a member's life
//! without a heartbeat.

mod chat;
mod files;
pub mod library;
mod rooms;

use std::collections::HashMap;
use std::fmt::Write as _;
use std::sync::{Arc, Mutex, OnceLock, PoisonError};
use std::time::Duration;

use akio_http::{Context, Headers, Method, Response, Service};
use akit_config::media::{MediaAction as Action, MediaPages};
use akit_config::value::Value;
use akit_runtime::{Handler, HandlerParams, Provision, ProvisionError, Provisioned};

use crate::auth::{Grant, Refusal, registry};
use crate::http::{COOKIE, HandlerAuth, refuse};

pub use files::FIT_COMMIT;
pub use rooms::{Stats, set_chat_file_bytes, stats};

/// What the room API is bounded by, swapped whole with each config.
#[derive(Debug, Clone)]
pub(crate) struct Settings {
    pub(crate) max_rooms: usize,
    pub(crate) max_members: usize,
    pub(crate) max_watchers: usize,
    pub(crate) max_message: usize,
    pub(crate) max_file: u64,
    pub(crate) member_ttl: Duration,
    pub(crate) stall: Duration,
    pub(crate) chat: bool,
}

/// A file of the pages, as it is answered.
struct Asset {
    body: Vec<u8>,
    ctype: &'static str,
    etag: String,
    html: bool,
}

/// The `media_pages` handler.
struct Pages {
    prefix: String,
    login: bool,
    pages: bool,
    auth: HandlerAuth,
    set: Arc<Settings>,
    /// The rooms of this prefix on the engine, shared by the configs that serve it.
    hub: Arc<OnceLock<Arc<rooms::Hub>>>,
    assets: HashMap<String, Asset>,
    csp: String,
    /// `GET api/config`'s answer.
    conf: Vec<u8>,
    /// The library and player pages' API, with `libraries` or `channels`.
    lib: Option<Arc<library::Library>>,
}

fn lock<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap_or_else(PoisonError::into_inner)
}

/// `s` as a JSON string.
pub(crate) fn quote(s: &str) -> String {
    let mut out = String::with_capacity(s.len().saturating_add(2));
    out.push('"');
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            '<' => out.push_str("\\u003c"),
            '>' => out.push_str("\\u003e"),
            '&' => out.push_str("\\u0026"),
            '\u{2028}' => out.push_str("\\u2028"),
            '\u{2029}' => out.push_str("\\u2029"),
            c if (c as u32) < 0x20 => {
                let _ = write!(out, "\\u{:04x}", c as u32);
            }
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

/// `s` as a JSON string, as the pages' API writes one: for clients and tests.
#[must_use]
pub fn quote_json(s: &str) -> String {
    quote(s)
}

/// A list of strings as a JSON array.
pub(crate) fn quote_list(v: &[String]) -> String {
    let items: Vec<String> = v.iter().map(|s| quote(s)).collect();
    format!("[{}]", items.join(","))
}

/// A JSON body as an object's members, or `None` when it is not JSON or not an object (as Go's
/// `json.Unmarshal` into a struct refuses; `null` is an empty object, as Go takes it).
pub(crate) fn object(body: &[u8]) -> Option<Vec<(String, Value)>> {
    let s = std::str::from_utf8(body).ok()?;
    match akit_config::value::parse(s).ok()? {
        Value::Object(m) => Some(m),
        Value::Null => Some(Vec::new()),
        _ => None,
    }
}

/// A member of `o` by its name, as Go's `json.Unmarshal` finds a field: the exact name first,
/// then one that differs only in case.
pub(crate) fn member<'v>(o: &'v [(String, Value)], key: &str) -> Option<&'v Value> {
    o.iter()
        .find(|(k, _)| k == key)
        .or_else(|| o.iter().find(|(k, _)| k.eq_ignore_ascii_case(key)))
        .map(|(_, v)| v)
}

/// A string member; `Err` when it is there and not a string.
pub(crate) fn str_member(o: &[(String, Value)], key: &str) -> Result<String, ()> {
    match member(o, key) {
        None | Some(Value::Null) => Ok(String::new()),
        Some(Value::String(s)) => Ok(s.clone()),
        Some(_) => Err(()),
    }
}

/// A boolean member; `Err` when it is there and not a boolean.
pub(crate) fn bool_member(o: &[(String, Value)], key: &str) -> Result<bool, ()> {
    match member(o, key) {
        None | Some(Value::Null) => Ok(false),
        Some(Value::Bool(b)) => Ok(*b),
        Some(_) => Err(()),
    }
}

/// Answers with a status and a line of plain text.
pub(crate) fn plain(ctx: &mut Context, status: u16, msg: &str, fields: &[(&str, &str)]) {
    let mut r = Response::with_body(
        status,
        "text/plain; charset=utf-8",
        format!("{msg}\n").into_bytes(),
    )
    .header("Cache-Control", "no-store");
    for (n, v) in fields {
        r = r.header(n, v);
    }
    ctx.respond(r);
}

/// Answers with a status and a JSON body.
pub(crate) fn json(ctx: &mut Context, status: u16, body: Vec<u8>, fields: &[(&str, &str)]) {
    let mut r =
        Response::with_body(status, "application/json", body).header("Cache-Control", "no-store");
    for (n, v) in fields {
        r = r.header(n, v);
    }
    ctx.respond(r);
}

/// The request's body, when it is at most `limit` bytes; else the request is answered 413.
pub(crate) fn body(ctx: &mut Context, limit: usize) -> Option<Vec<u8>> {
    let declared = ctx
        .header("content-length")
        .and_then(|v| v.trim().parse::<u64>().ok());
    let limit64 = u64::try_from(limit).unwrap_or(u64::MAX);
    if declared.is_some_and(|n| n > limit64) || ctx.body().len() > limit {
        plain(ctx, 413, "the body is too large", &[]);
        return None;
    }
    Some(ctx.body().to_vec())
}

/// Whether `p` is a path of plain segments, as `/media`.
fn path_prefix(p: &str) -> bool {
    p.starts_with('/')
        && p.split('/').skip(1).all(|seg| {
            !seg.is_empty()
                && seg
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'_' | b'~' | b'-'))
        })
}

pub(crate) fn new_pages(
    p: &Provision<'_, HandlerParams>,
) -> Result<Provisioned<dyn Handler>, ProvisionError> {
    let HandlerParams::MediaPages(h) = p.params() else {
        return Err(ProvisionError::new(
            "the media_pages handler's params are not media_pages'",
        ));
    };
    let prefix = h.prefix_or().to_owned();
    if !path_prefix(&prefix) {
        return Err(ProvisionError::field(
            "prefix",
            format!(
                "{:?} is not a path of segments of letters, digits, '.', '_', '~' and '-', as /media",
                h.prefix
            ),
        ));
    }
    if h.login && h.auth.is_empty() {
        return Err(ProvisionError::field(
            "login",
            "a login needs auth, a media_auth policy with users",
        ));
    }
    if h.login && h.policy.as_ref().is_some_and(|p| p.users.is_empty()) {
        return Err(ProvisionError::field(
            "login",
            format!("the media_auth policy {:?} has no users to log in", h.auth),
        ));
    }
    let members = h.members();
    let set = Arc::new(Settings {
        max_rooms: h.rooms(),
        max_members: members,
        max_watchers: members.saturating_mul(2),
        max_message: h.message_bytes(),
        max_file: h.file_bytes(),
        member_ttl: h.ttl(),
        stall: h.stall(),
        chat: h.chat,
    });
    let mut origins = Vec::new();
    for v in [&h.whip, &h.whep, &h.hls] {
        if let Ok(Some(o)) = akit_config::media::page_endpoint(v) {
            if !origins.contains(&o) {
                origins.push(o);
            }
        }
    }
    let key = Arc::new(OnceLock::new());
    let lib = library::provision(h, Arc::clone(&key), &mut origins)?;
    origins.sort();
    let auth = HandlerAuth::new(p.env().engine(), &h.auth, h.policy.as_ref());
    let public = |a: Action| h.policy.as_ref().is_some_and(|p| p.is_public(a));
    let lib_conf = lib.as_ref().map_or("null", |(_, c)| c.as_str());
    let conf = config_json(h, public(Action::Publish), public(Action::Play), lib_conf).into_bytes();
    let hub = Arc::new(OnceLock::new());
    let (hk, key_prefix, hset) = (Arc::clone(&hub), prefix.clone(), Arc::clone(&set));
    p.env().engine().when_set(move |engine| {
        let k = registry::key(engine);
        let _ = key.set(k);
        let _ = hk.set(rooms::acquire(k, &key_prefix, hset));
    });
    let mut pages = Pages {
        prefix,
        login: h.login,
        pages: h.pages,
        auth,
        set,
        hub: Arc::clone(&hub),
        assets: HashMap::new(),
        csp: String::new(),
        conf,
        lib: lib.map(|(l, _)| Arc::new(l)),
    };
    if pages.pages {
        pages.build_assets(&h.title, &origins);
    }
    Ok(
        Provisioned::new(Arc::new(pages) as Arc<dyn Handler>).on_cleanup(move || {
            if let Some(h) = hub.get() {
                rooms::release(h);
            }
        }),
    )
}

/// `GET api/config`'s answer, as fit's: a JSON object of its keys in order.
fn config_json(h: &MediaPages, public_publish: bool, public_play: bool, library: &str) -> String {
    let or = |s: &str, d: &str| {
        if s.is_empty() {
            d.to_owned()
        } else {
            s.to_owned()
        }
    };
    let ice: Vec<String> = h
        .ice_servers
        .iter()
        .map(|s| {
            let mut o = format!("{{\"urls\":{}", quote_list(&s.urls));
            if !s.credential.is_empty() {
                let _ = write!(o, ",\"credential\":{}", quote(&s.credential));
            }
            if !s.username.is_empty() {
                let _ = write!(o, ",\"username\":{}", quote(&s.username));
            }
            o.push('}');
            o
        })
        .collect();
    let links = if h.links.is_empty() {
        "null".to_owned()
    } else {
        let l: Vec<String> = h
            .links
            .iter()
            .map(|l| format!("{{\"name\":{},\"url\":{}}}", quote(&l.name), quote(&l.url)))
            .collect();
        format!("[{}]", l.join(","))
    };
    let ttl = u64::try_from(h.ttl().as_millis()).unwrap_or(u64::MAX);
    format!(
        concat!(
            "{{\"auth\":{auth},\"call\":{{\"cam\":{cam},\"mic\":{mic}}},",
            "\"chat\":{{\"enabled\":{chat},\"maxFileBytes\":{file}}},\"hls\":{hls},",
            "\"iceServers\":[{ice}],\"library\":{library},\"links\":{links},\"login\":{login},",
            "\"public\":{{\"play\":{play},\"publish\":{publish}}},",
            "\"room\":{{\"heartbeatMs\":{hb},\"maxMembers\":{members},\"maxMessage\":{msg}}},",
            "\"title\":{title},\"whep\":{whep},\"whip\":{whip}}}"
        ),
        auth = !h.auth.is_empty(),
        cam = h.cam_default,
        mic = h.mic_default,
        chat = h.chat,
        file = h.file_bytes(),
        hls = quote(&h.hls),
        ice = ice.join(","),
        library = library,
        links = links,
        login = h.login,
        play = public_play,
        publish = public_publish,
        hb = ttl / 3,
        members = h.members(),
        msg = h.message_bytes(),
        title = quote(&or(&h.title, "akit media")),
        whep = quote(&or(&h.whep, akit_config::media::DEFAULT_PAGES_WHEP)),
        whip = quote(&or(&h.whip, akit_config::media::DEFAULT_PAGES_WHIP)),
    )
}

/// The ETag of a body: 12 bytes of its SHA-256, in base64url.
fn etag(body: &[u8]) -> String {
    let sum = akio_crypto::hash::Sha256::digest(body);
    let short = sum.get(..12).unwrap_or(&sum[..]);
    format!("\"{}\"", akio_crypto::base64::URL_SAFE_NO_PAD.encode(short))
}

impl Pages {
    /// The pages' answers: the index, with the prefix and the title in it, its script and style,
    /// and hls.js, each with an ETag; the index with a Content Security Policy that lets the
    /// pages reach the endpoints' origins and nothing else.
    fn build_assets(&mut self, title: &str, origins: &[String]) {
        let extra = if origins.is_empty() {
            String::new()
        } else {
            format!(" {}", origins.join(" "))
        };
        self.csp = format!(
            "default-src 'self'; script-src 'self'; style-src 'self'; img-src 'self' data: blob:{extra}; media-src 'self' blob:{extra}; connect-src 'self'{extra}; worker-src 'self' blob:; object-src 'none'; base-uri 'self'; frame-ancestors 'none'; form-action 'self'"
        );
        let title = if title.is_empty() {
            "akit media"
        } else {
            title
        };
        for (name, body) in files::FILES {
            let (ctype, body) = match *name {
                "index.html" => {
                    let s = String::from_utf8_lossy(body)
                        .replace("{{base}}", &format!("{}/", self.prefix))
                        .replace("{{title}}", title);
                    ("text/html; charset=utf-8", s.into_bytes())
                }
                n if n.ends_with(".js") => ("text/javascript; charset=utf-8", body.to_vec()),
                n if n.ends_with(".css") => ("text/css; charset=utf-8", body.to_vec()),
                _ => ("application/octet-stream", body.to_vec()),
            };
            self.add(name, ctype, body);
        }
        // hls.js, which the player page plays HLS with, and its licence (Apache-2.0): the
        // console's copy, as fit's one copy is its workbench's.
        self.add(
            "vendor/hls.min.js",
            "text/javascript; charset=utf-8",
            HLS_JS.to_vec(),
        );
        self.add(
            "vendor/hls.js-LICENSE.txt",
            "text/plain; charset=utf-8",
            HLS_LICENSE.to_vec(),
        );
    }

    fn add(&mut self, name: &str, ctype: &'static str, body: Vec<u8>) {
        let etag = etag(&body);
        self.assets.insert(
            name.to_owned(),
            Asset {
                body,
                ctype,
                etag,
                html: ctype.starts_with("text/html"),
            },
        );
    }

    fn asset(&self, ctx: &mut Context, rest: &str) {
        let name = match rest {
            "" | "call" | "publish" | "play" | "library" | "watch" => "index.html",
            n => n,
        };
        let Some(a) = self.assets.get(name) else {
            return plain(ctx, 404, "not found", &[]);
        };
        if !matches!(ctx.method(), Method::Get | Method::Head) {
            return plain(ctx, 405, "the pages take GET", &[("Allow", "GET, HEAD")]);
        }
        if ctx
            .header("if-none-match")
            .is_some_and(|v| v.contains(a.etag.as_str()))
        {
            ctx.respond(
                Response::new(304)
                    .header("Etag", &a.etag)
                    .header("Cache-Control", "no-cache"),
            );
            return;
        }
        let mut r = Response::with_body(200, a.ctype, a.body.clone())
            .header("Cache-Control", "no-cache")
            .header("Etag", &a.etag)
            .header("X-Content-Type-Options", "nosniff")
            .header("Referrer-Policy", "no-referrer");
        if a.html {
            r = r
                .header("Content-Security-Policy", &self.csp)
                .header("X-Frame-Options", "DENY")
                .header(
                    "Permissions-Policy",
                    "camera=(self), microphone=(self), display-capture=(self)",
                );
        }
        ctx.respond(r);
    }

    fn api(&self, ctx: &mut Context, rest: &str) {
        let post = *ctx.method() == Method::Post;
        match rest {
            "config" => {
                if *ctx.method() != Method::Get {
                    return plain(ctx, 405, "config takes GET", &[("Allow", "GET")]);
                }
                json(ctx, 200, self.conf.clone(), &[]);
            }
            "login" => {
                if !post {
                    return plain(ctx, 405, "login takes POST", &[("Allow", "POST")]);
                }
                self.login(ctx);
            }
            "logout" => {
                if !post {
                    return plain(ctx, 405, "logout takes POST", &[("Allow", "POST")]);
                }
                let c = cookie(ctx, "", -1);
                json(ctx, 200, b"{}".to_vec(), &[("Set-Cookie", &c)]);
            }
            r if r == "library" || r.starts_with("library/") => {
                let rest = r.strip_prefix("library").unwrap_or("");
                library::library_api(self.lib.as_ref(), &self.auth, ctx, rest);
            }
            "play" => library::play(self.lib.as_ref(), &self.auth, self.set.max_message, ctx),
            r if r.starts_with("rooms/") => {
                let Some(hub) = self.hub.get() else {
                    return plain(ctx, 503, "the media pages are not ready", &[]);
                };
                rooms::serve(
                    ctx,
                    hub,
                    &self.auth,
                    &self.set,
                    r.strip_prefix("rooms/").unwrap_or(""),
                );
            }
            _ => plain(ctx, 404, "no such API", &[]),
        }
    }

    /// Checks a user's password against the policy, and answers with a token the policy signs,
    /// set as the cookie too.
    fn login(&self, ctx: &mut Context) {
        if !self.login {
            return plain(ctx, 404, "the pages have no login", &[]);
        }
        let Some(b) = body(ctx, self.set.max_message) else {
            return;
        };
        let parsed = object(&b).and_then(|o| {
            let user = str_member(&o, "user").ok()?;
            let password = str_member(&o, "password").ok()?;
            Some((user, password))
        });
        let Some((user, password)) =
            parsed.filter(|(u, p)| !u.is_empty() && u.len() <= 128 && p.len() <= 1024)
        else {
            return plain(
                ctx,
                400,
                "the body must be JSON: {\"user\", \"password\"}",
                &[],
            );
        };
        let Some(policy) = self.auth.policy().cloned() else {
            return refuse(ctx, Refusal::Unavailable, "");
        };
        let answer = move |ctx: &mut Context, r: Result<(String, Grant), Refusal>| {
            let (subject, grant) = match r {
                Ok(v) => v,
                Err(e) => return refuse(ctx, e, ""),
            };
            let (tok, exp) = match policy.issue(&subject, &grant, Duration::ZERO) {
                Ok(v) => v,
                Err(e) => return plain(ctx, 500, &e, &[]),
            };
            let now = i64::try_from(
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .map_or(0, |d| d.as_secs()),
            )
            .unwrap_or(i64::MAX);
            let max_age = exp.saturating_sub(now);
            let body = format!(
                "{{\"expires\":{exp},\"play\":{},\"publish\":{},\"subject\":{},\"token\":{}}}",
                quote_list(&grant.play),
                quote_list(&grant.publish),
                quote(&subject),
                quote(&tok)
            );
            let c = cookie(ctx, &tok, max_age);
            json(ctx, 200, body.into_bytes(), &[("Set-Cookie", &c)]);
        };
        // A password checked at once is answered at once; a bcrypt hash is checked on the pool
        // while the request waits.
        match self.auth.policy().map(|p| p.login_now(&user, &password)) {
            Some(Err(Refusal::Pending)) => {}
            Some(r) => return answer(ctx, r),
            None => return refuse(ctx, Refusal::Unavailable, ""),
        }
        let Some(policy) = self.auth.policy().cloned() else {
            return refuse(ctx, Refusal::Unavailable, "");
        };
        let Some(s) = ctx.suspend() else {
            return refuse(ctx, Refusal::Busy, "");
        };
        policy.login(&user, &password, move |r| {
            s.resume(move |ctx| answer(ctx, r));
        });
    }
}

/// The `Set-Cookie` of a token, as Go's `http.Cookie` writes it: HttpOnly, SameSite=Strict,
/// Secure over TLS, for the whole site, as the endpoints are on other paths. A negative
/// `max_age` removes it.
fn cookie(ctx: &Context, tok: &str, max_age: i64) -> String {
    let mut c = format!("{COOKIE}={tok}; Path=/");
    match max_age {
        n if n > 0 => {
            let _ = write!(c, "; Max-Age={n}");
        }
        n if n < 0 => c.push_str("; Max-Age=0"),
        _ => {}
    }
    c.push_str("; HttpOnly");
    if ctx.local(akit_runtime::TLS).is_some() {
        c.push_str("; Secure");
    }
    c.push_str("; SameSite=Strict");
    c
}

/// hls.js and its licence: the console's copy.
const HLS_JS: &[u8] = include_bytes!("../../../akit-admin/console/web/vendor/hls.min.js");
const HLS_LICENSE: &[u8] = include_bytes!("../../../akit-admin/console/web/vendor/hls.LICENSE.txt");

impl Service for Pages {
    fn call(&self, ctx: &mut Context) {
        let path = ctx.path().to_owned();
        if path == self.prefix {
            if !self.pages {
                return plain(ctx, 404, "not found", &[]);
            }
            let q = ctx.req().query().map_or(String::new(), |q| format!("?{q}"));
            ctx.respond(Response::new(308).header("Location", format!("{}/{q}", self.prefix)));
            return;
        }
        let Some(rest) = path
            .strip_prefix(self.prefix.as_str())
            .and_then(|r| r.strip_prefix('/'))
        else {
            return plain(ctx, 404, "not found", &[]);
        };
        if let Some(api) = rest.strip_prefix("api/") {
            return self.api(ctx, api);
        }
        self.asset(ctx, rest);
    }
}

impl Handler for Pages {}

/// The headers of a room's event stream.
pub(crate) fn sse_headers() -> Headers {
    let mut h = Headers::new();
    h.set("Content-Type", "text/event-stream; charset=utf-8");
    h.set("Cache-Control", "no-store");
    h.set("X-Accel-Buffering", "no");
    h
}
