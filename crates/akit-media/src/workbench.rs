//! The Admin API's endpoints behind the console's media pages (I149, fit's `admin/media.go`):
//!
//! ```text
//! POST /media/preview                     a short-lived URL of an item, {"library", "item"},
//!                                         or of a channel, {"channel"}, for the console's user
//! GET  /media/auth                        every media_auth policy: its url_signing, keys (never
//!                                         their secrets), sessions, refusals, media_tokens routes
//! GET  /media/auth/{policy}/sessions      a page of its playback sessions: offset, limit,
//!                                         subject, resource
//! POST /media/auth/{policy}/sessions/end  end one: {"subject", "id"}
//! POST /media/auth/{policy}/rotate        rotate a key, {"key"}, every key, {"all": true}, or
//!                                         the single secret, {}, through its file
//! POST /media/auth/{policy}/urls          a test URL, as media_tokens' /urls signs one
//! GET  /media/stats                       the libraries, vod handlers, policies and channels
//! ```
//!
//! They read the running config ([`View`]), which the caller parses, and stat or write secret
//! files: they run on the crate's pool, the request suspended, never on an I/O thread. A URL
//! they answer is the only secret they carry, short-lived, sent with `Cache-Control: no-store`.

use std::net::IpAddr;
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use akio_http::{Context, Method, Response};
use akit_config::media::MediaAction as Action;
use akit_config::value::Value;
use akit_config::{Config, Handler, Protocol};

use crate::library::catalog::{compact, time};

/// How long a preview's URL is good for.
pub const PREVIEW_TTL: Duration = Duration::from_secs(120);
/// The header the console sets on what it passes to the Admin API: the user logged in.
pub const CONSOLE_USER: &str = "Akit-Console-User";
/// And the host the browser addressed, for the host of a URL on a listener of every address.
pub const CONSOLE_HOST: &str = "Akit-Console-Host";
/// The paths these endpoints answer, under the Admin API.
pub const PREFIXES: &[&str] = &["/media/preview", "/media/auth", "/media/stats"];

const DEFAULT_SESSIONS_PAGE: usize = 100;
const MAX_SESSIONS_PAGE: usize = 1000;
const MAX_TEST_TTL: Duration = Duration::from_secs(3600);
const MAX_STATS_RESOURCES: usize = 50;
const MAX_BODY: usize = 64 << 10;

/// A listener that serves HTTP, as a URL reaches it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Listener {
    /// Its name.
    pub name: String,
    /// `http` or `https`.
    pub scheme: &'static str,
    /// The host it listens on; empty for every address.
    pub host: String,
    /// The port it is bound to.
    pub port: u16,
}

impl Listener {
    /// Its URLs' origin: its own host, or else the console's (`console_host`, as the browser sent
    /// it), or else 127.0.0.1.
    #[must_use]
    pub fn origin(&self, console_host: &str) -> String {
        let mut host = self.host.clone();
        if host.is_empty() {
            host = host_of(console_host);
        }
        if host.is_empty() {
            host = "127.0.0.1".to_owned();
        }
        if host.contains(':') {
            host = format!("[{host}]");
        }
        format!("{}://{host}:{}", self.scheme, self.port)
    }
}

/// The host of `hostport`, a `Host` field, without brackets; empty when it holds anything but
/// letters, digits, '.', '-', ':', '[' and ']', or an unspecified address (fit's `hostOf`).
#[must_use]
pub fn host_of(hostport: &str) -> String {
    if hostport.is_empty()
        || hostport.len() > 270
        || !hostport
            .bytes()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, b'.' | b'-' | b':' | b'[' | b']'))
    {
        return String::new();
    }
    let host = if let Some(rest) = hostport.strip_prefix('[') {
        match rest.split_once(']') {
            Some((h, _)) => h,
            None => return String::new(),
        }
    } else if hostport.matches(':').count() == 1 {
        hostport.split(':').next().unwrap_or("")
    } else {
        hostport
    };
    if host.is_empty() || host.contains(['[', ']']) || host.len() > 253 {
        return String::new();
    }
    match host.parse::<IpAddr>() {
        Ok(a) if a.is_unspecified() => String::new(),
        Ok(_) => host.to_owned(),
        Err(_) if host.contains(':') => String::new(),
        Err(_) => host.to_owned(),
    }
}

/// A `vod` or `hls` route of the running config, with what a URL of it needs.
#[derive(Debug, Clone)]
struct MediaRoute {
    name: String,
    hls: bool,
    library: String,
    prefix: String,
    auth: String,
    cors: String,
    listener: Option<usize>,
}

/// A `media_tokens` route of the running config.
#[derive(Debug, Clone)]
struct TokenRoute {
    name: String,
    auth: String,
    url_base: String,
    url_prefixes: Vec<String>,
    max_ttl: Duration,
}

/// What the media pages read of the running config and its listeners (fit's `mediaView`).
#[derive(Debug, Clone)]
pub struct View {
    config: Option<Arc<Config>>,
    listeners: Vec<Listener>,
    routes: Vec<MediaRoute>,
    tokens: Vec<TokenRoute>,
}

/// Makes a [`View`] of the config running now: the caller's, which parses it.
pub type ViewFn = Arc<dyn Fn() -> View + Send + Sync>;

impl View {
    /// The view of `config` (none when the running config does not parse), whose listeners
    /// serving HTTP are bound to `ports` (by name).
    #[must_use]
    pub fn new(config: Option<Arc<Config>>, ports: &[(String, u16)]) -> View {
        let mut listeners = Vec::new();
        let (ls, rs): (&[akit_config::Listener], &[akit_config::Route]) = match &config {
            Some(c) => (&c.listeners, &c.routes),
            None => (&[], &[]),
        };
        for l in ls {
            let http = l.protocols.is_empty()
                || l.protocols
                    .iter()
                    .any(|p| matches!(p, Protocol::Http11 | Protocol::H2 | Protocol::H2c));
            if l.redirect_https != 0 || !http {
                continue;
            }
            let (host, port) = split_host_port(&l.address);
            let port = ports
                .iter()
                .find(|(n, _)| *n == l.name)
                .map_or(port, |(_, p)| *p);
            listeners.push(Listener {
                name: l.name.clone(),
                scheme: if l.tls.is_some() { "https" } else { "http" },
                host: host_of(&host),
                port,
            });
        }
        let mut routes = Vec::new();
        let mut tokens = Vec::new();
        for (i, r) in rs.iter().enumerate() {
            let name = r
                .name
                .clone()
                .unwrap_or_else(|| format!("#{}", i.saturating_add(1)));
            let mut mr = match &r.handler {
                Handler::Vod(v) => MediaRoute {
                    name,
                    hls: false,
                    library: v.library.clone(),
                    prefix: if v.prefix.is_empty() {
                        format!("/vod/{}/", v.library)
                    } else {
                        v.prefix.clone()
                    },
                    auth: v.auth.clone(),
                    cors: v.cors.clone(),
                    listener: None,
                },
                Handler::Hls(h) => MediaRoute {
                    name,
                    hls: true,
                    library: String::new(),
                    prefix: if h.prefix.is_empty() {
                        "/".to_owned()
                    } else {
                        h.prefix.clone()
                    },
                    auth: h.auth.clone(),
                    cors: h.cors.clone(),
                    listener: None,
                },
                Handler::MediaTokens(t) => {
                    tokens.push(TokenRoute {
                        name,
                        auth: t.auth.clone(),
                        url_base: t.url_base.trim_end_matches('/').to_owned(),
                        url_prefixes: t.url_prefixes.clone(),
                        max_ttl: t.ttl_bound(),
                    });
                    continue;
                }
                _ => continue,
            };
            if !mr.prefix.ends_with('/') {
                mr.prefix.push('/');
            }
            mr.listener = listeners
                .iter()
                .position(|l| r.listeners.first().is_none_or(|n| *n == l.name));
            routes.push(mr);
        }
        View {
            config,
            listeners,
            routes,
            tokens,
        }
    }

    /// The first route serving a library's items (`vod`), or the channels (`hls`).
    fn route(&self, hls: bool, library: &str) -> Option<&MediaRoute> {
        self.routes
            .iter()
            .find(|r| r.hls == hls && (hls || r.library == library))
    }

    /// The `vod` or `hls` route whose prefix is the longest that starts `p`.
    fn route_of(&self, p: &str) -> Option<&MediaRoute> {
        self.routes
            .iter()
            .filter(|r| p.starts_with(&r.prefix))
            .max_by_key(|r| r.prefix.len())
    }

    /// The running config's `media_auth` policies.
    fn policies(&self) -> &[akit_config::media::MediaAuth] {
        self.config.as_ref().map_or(&[], |c| &c.media_auth)
    }

    fn listener(&self, r: &MediaRoute) -> Option<&Listener> {
        r.listener.and_then(|i| self.listeners.get(i))
    }

    /// The origins of the listeners serving `vod` and `hls` routes, for the console's
    /// Content-Security-Policy (fit's `sources`), the console's host `console_host`.
    #[must_use]
    pub fn origins(&self, console_host: &str) -> Vec<String> {
        let mut out: Vec<String> = Vec::new();
        for r in &self.routes {
            if let Some(l) = self.listener(r) {
                let o = l.origin(console_host);
                if !out.contains(&o) {
                    out.push(o);
                }
            }
        }
        out
    }
}

/// The host and port of a listener's address, `:8080` or `[::1]:80`; port 0 when it has none.
fn split_host_port(a: &str) -> (String, u16) {
    let (h, p) = match a.rsplit_once(':') {
        Some((h, p)) => (h, p),
        None => (a, ""),
    };
    let h = h.trim_start_matches('[').trim_end_matches(']');
    (h.to_owned(), p.parse().unwrap_or(0))
}

/// Whether the request is one of these endpoints; then it is answered.
pub fn serve(key: usize, view: &ViewFn, ctx: &mut Context) -> bool {
    let path = ctx.path().to_owned();
    if !PREFIXES
        .iter()
        .any(|p| path == *p || path.strip_prefix(p).is_some_and(|r| r.starts_with('/')))
    {
        return false;
    }
    let method = ctx.method().clone();
    let user = ctx.header(CONSOLE_USER).unwrap_or("").to_owned();
    let host = ctx.header(CONSOLE_HOST).unwrap_or("").to_owned();
    let query = ctx.req().query().unwrap_or("").to_owned();
    let body = ctx.req().body.clone();
    if body.len() > MAX_BODY {
        ctx.respond(error(413, "the body is too large"));
        return true;
    }
    let req = Req {
        key,
        method,
        path,
        query,
        body,
        user,
        host,
    };
    let Some(s) = ctx.suspend() else {
        ctx.respond(error(503, "the request cannot wait now").header("Retry-After", "1"));
        return true;
    };
    let view = Arc::clone(view);
    crate::pool::submit(Box::new(move |late| {
        let resp = if late {
            error(503, "the media endpoints are busy").header("Retry-After", "1")
        } else {
            req.answer(&view)
        };
        s.resume(move |ctx| ctx.respond(resp));
    }));
    true
}

/// A request, as the pool answers it.
struct Req {
    key: usize,
    method: Method,
    path: String,
    query: String,
    body: Vec<u8>,
    user: String,
    host: String,
}

fn json(status: u16, v: &Value) -> Response {
    let mut b = compact(v);
    b.push('\n');
    Response::with_body(status, "application/json", b.into_bytes())
}

fn no_store(v: &Value) -> Response {
    json(200, v).header("Cache-Control", "no-store")
}

fn error(status: u16, msg: &str) -> Response {
    json(status, &obj(vec![("error", s(msg))]))
}

fn not_allowed(allow: &str) -> Response {
    error(405, "method not allowed").header("Allow", allow)
}

fn obj(m: Vec<(&str, Value)>) -> Value {
    Value::Object(m.into_iter().map(|(k, v)| (k.to_owned(), v)).collect())
}

fn s(v: &str) -> Value {
    Value::String(v.to_owned())
}

fn n(v: u64) -> Value {
    Value::Number(v.to_string())
}

fn ni(v: i64) -> Value {
    Value::Number(v.to_string())
}

fn strings(v: &[String]) -> Value {
    Value::Array(v.iter().map(|x| s(x)).collect())
}

/// Go's `time.Duration.String`, as fit's answers write durations.
fn go_duration(d: Duration) -> String {
    akit_config::duration::format(d)
}

/// A JSON body: its members, or an error answer. An empty body is no members.
fn members(body: &[u8]) -> Result<Vec<(String, Value)>, Box<Response>> {
    if body.iter().all(u8::is_ascii_whitespace) {
        return Ok(Vec::new());
    }
    let text =
        std::str::from_utf8(body).map_err(|_| Box::new(error(400, "the body: not UTF-8")))?;
    match akit_config::value::parse(text) {
        Ok(Value::Object(m)) => Ok(m),
        Ok(Value::Null) => Ok(Vec::new()),
        Ok(_) => Err(Box::new(error(400, "the body: not a JSON object"))),
        Err(e) => Err(Box::new(error(400, &format!("the body: {e}")))),
    }
}

fn str_of(m: &[(String, Value)], k: &str) -> Result<String, Box<Response>> {
    match m.iter().find(|(x, _)| x == k).map(|(_, v)| v) {
        None | Some(Value::Null) => Ok(String::new()),
        Some(Value::String(v)) => Ok(v.clone()),
        Some(_) => Err(Box::new(error(
            400,
            &format!("the body: {k} is not a string"),
        ))),
    }
}

fn bool_of(m: &[(String, Value)], k: &str) -> Result<bool, Box<Response>> {
    match m.iter().find(|(x, _)| x == k).map(|(_, v)| v) {
        None | Some(Value::Null) => Ok(false),
        Some(Value::Bool(v)) => Ok(*v),
        Some(_) => Err(Box::new(error(
            400,
            &format!("the body: {k} is not a boolean"),
        ))),
    }
}

/// Whether `v` may be a URL's or a token's subject: 1 to 128 bytes, no control characters.
fn valid_subject(v: &str) -> bool {
    !v.is_empty() && v.len() <= 128 && !v.chars().any(|c| c < ' ' || c == '\u{7f}')
}

/// A new playback session's ID: 12 random bytes, base64url.
fn new_session() -> String {
    let mut b = [0u8; 12];
    let _ = akio_crypto::rand::fill_random(&mut b);
    akio_crypto::base64::URL_SAFE_NO_PAD.encode(b)
}

fn now_secs() -> i64 {
    crate::urlsign::now_secs()
}

fn nanos_time(ns: i64) -> Value {
    let t = UNIX_EPOCH
        .checked_add(Duration::from_nanos(u64::try_from(ns.max(0)).unwrap_or(0)))
        .unwrap_or(UNIX_EPOCH);
    time(t)
}

impl Req {
    fn answer(&self, view: &ViewFn) -> Response {
        let get = matches!(self.method, Method::Get | Method::Head);
        let post = self.method == Method::Post;
        let path = self.path.as_str();
        if path == "/media/preview" {
            return if post {
                self.preview(&view())
            } else {
                not_allowed("POST")
            };
        }
        if path == "/media/stats" {
            return if get {
                self.stats(&view())
            } else {
                not_allowed("GET, HEAD")
            };
        }
        let rest = path.strip_prefix("/media/auth").unwrap_or("");
        if rest.is_empty() || rest == "/" {
            return if get {
                self.auth(&view())
            } else {
                not_allowed("GET, HEAD")
            };
        }
        let rest = rest.trim_start_matches('/');
        let (name, sub) = rest.split_once('/').unwrap_or((rest, ""));
        let Some(p) = crate::auth::registry::lookup(self.key, name) else {
            return error(
                404,
                &format!("no media_auth policy named {name:?} runs here"),
            );
        };
        match sub {
            "sessions" if get => self.sessions(&p, &view()),
            "sessions" => not_allowed("GET, HEAD"),
            "sessions/end" if post => self.end_session(&p),
            "rotate" if post => self.rotate(&p),
            "urls" if post => self.test_url(&p, &view()),
            "sessions/end" | "rotate" | "urls" => not_allowed("POST"),
            _ => error(404, "no such endpoint"),
        }
    }

    /// `POST /media/preview`.
    fn preview(&self, v: &View) -> Response {
        let m = match members(&self.body) {
            Ok(m) => m,
            Err(r) => return *r,
        };
        let (library, item, channel) = match (
            str_of(&m, "library"),
            str_of(&m, "item"),
            str_of(&m, "channel"),
        ) {
            (Ok(a), Ok(b), Ok(c)) => (a, b, c),
            (Err(r), _, _) | (_, Err(r), _) | (_, _, Err(r)) => return *r,
        };
        let user = if self.user.is_empty() {
            "admin-api"
        } else {
            &self.user
        };
        let subject = format!("workbench:{user}");
        if !valid_subject(&subject) {
            return error(
                400,
                "the console user's name does not make a subject: 1 to 118 bytes with no control characters",
            );
        }
        let (rt, target, stream, poster) =
            if !channel.is_empty() && library.is_empty() && item.is_empty() {
                let Some(rt) = v.route(true, "") else {
                    return error(404, "no hls route serves the channels");
                };
                let found = crate::channels::lookup(self.key).and_then(|m| m.channel(&channel));
                if found.is_none() {
                    return error(404, &format!("no channel named {channel:?} runs here"));
                }
                (
                    rt,
                    format!("{}{channel}/index.m3u8", rt.prefix),
                    channel.clone(),
                    String::new(),
                )
            } else if channel.is_empty() && !library.is_empty() && !item.is_empty() {
                let Some(rt) = v.route(false, &library) else {
                    return error(404, &format!("no vod route serves the library {library}"));
                };
                let l = crate::library::lookup(self.key, &library);
                let Some(l) = l.filter(|l| l.catalog().scanned.is_some()) else {
                    return error(
                        503,
                        &format!("the media library {library:?} is not loaded, or not scanned yet"),
                    )
                    .header("Retry-After", "1");
                };
                let Some(it) = l.item(&item).filter(|it| it.error.is_empty()) else {
                    return error(404, &format!("no item {item:?} the library serves"));
                };
                let poster = it.poster.as_ref().map_or(String::new(), |p| {
                    let ext = std::path::Path::new(&p.path)
                        .extension()
                        .map_or(String::new(), |e| e.to_string_lossy().to_lowercase());
                    format!("{}{}/poster.{ext}", rt.prefix, it.id)
                });
                (
                    rt,
                    format!("{}{}/index.m3u8", rt.prefix, it.id),
                    format!("vod/{library}/{}", it.id),
                    poster,
                )
            } else {
                return error(
                    400,
                    "the body names an item, {\"library\", \"item\"}, or a channel, {\"channel\"}",
                );
            };
        let Some(listener) = v.listener(rt) else {
            return error(
                404,
                &format!("no listener serves HTTP for the route {}", rt.name),
            );
        };
        let origin = listener.origin(&self.host);
        let ttl = i64::try_from(PREVIEW_TTL.as_secs()).unwrap_or(120);
        let mut expiry = now_secs().saturating_add(ttl);
        let mut session = String::new();
        let (query, signed) = if rt.auth.is_empty() {
            (String::new(), "public")
        } else {
            let Some(p) = crate::auth::registry::lookup(self.key, &rt.auth) else {
                return error(
                    503,
                    &format!("the media_auth policy {:?} is not loaded", rt.auth),
                )
                .header("Retry-After", "1");
            };
            if p.is_public(Action::Play) {
                (String::new(), "public")
            } else if p.url_signing() {
                let Some(res) = crate::urlsign::resource_of(&target) else {
                    return error(
                        400,
                        &format!("{target:?} is not a path a URL is signed for"),
                    );
                };
                session = new_session();
                let viewer = crate::auth::Viewer {
                    subject: subject.clone(),
                    address: None,
                    session: session.clone(),
                };
                match p.sign_url(&res, &viewer, expiry, "") {
                    Ok(q) => (q, "url"),
                    Err(_) => {
                        return error(503, "the URL cannot be signed: the policy changed");
                    }
                }
            } else {
                let grant = crate::auth::Grant {
                    publish: Vec::new(),
                    play: vec![stream.clone()],
                };
                match p.issue(&subject, &grant, PREVIEW_TTL) {
                    Ok((tok, exp)) => {
                        expiry = exp;
                        (
                            format!("token={}", crate::handlers::query_escape(&tok)),
                            "token",
                        )
                    }
                    Err(_) => {
                        return error(400, &format!("no token can be issued for {stream:?}"));
                    }
                }
            }
        };
        let mut url = format!("{origin}{target}");
        let mut poster_url = if poster.is_empty() {
            String::new()
        } else {
            format!("{origin}{poster}")
        };
        if !query.is_empty() {
            url = format!("{url}?{query}");
            if !poster_url.is_empty() {
                poster_url = format!("{poster_url}?{query}");
            }
        }
        no_store(&obj(vec![
            ("url", s(&url)),
            ("poster", s(&poster_url)),
            ("expires", ni(expiry)),
            ("subject", s(&subject)),
            ("session", s(&session)),
            ("signed", s(signed)),
            ("route", s(&rt.name)),
            ("cors", s(&rt.cors)),
        ]))
    }

    /// `GET /media/auth`.
    fn auth(&self, v: &View) -> Response {
        let mut out = Vec::new();
        for c in v.policies() {
            let Some(p) = crate::auth::registry::lookup(self.key, &c.name) else {
                continue;
            };
            let live: Vec<_> = p.sessions().into_iter().filter(|s| !s.ended).collect();
            let mut subjects: Vec<&str> = live.iter().map(|s| s.subject.as_str()).collect();
            subjects.sort_unstable();
            subjects.dedup();
            let mut m = vec![
                ("name", s(&c.name)),
                ("url_signing", Value::Bool(p.url_signing())),
            ];
            if let Some(u) = &c.url_signing {
                m.push((
                    "settings",
                    obj(vec![
                        ("ttl", s(&go_duration(u.url_ttl()))),
                        ("play_grace", s(&go_duration(u.grace()))),
                        ("session_idle", s(&go_duration(u.idle()))),
                        ("max_sessions", n(u.max_sessions as u64)),
                        (
                            "on_limit",
                            s(if u.end_oldest { "end_oldest" } else { "refuse" }),
                        ),
                        ("cookie", Value::Bool(u.cookie)),
                        ("bind_address", s(if u.subnet { "subnet" } else { "ip" })),
                        ("referers", strings(&u.referers)),
                        ("origins", strings(&u.origins)),
                    ]),
                ));
            }
            m.extend([
                ("keys", keys_of(&p)),
                ("sessions", n(live.len() as u64)),
                ("subjects", n(subjects.len() as u64)),
                ("refusals", refusals(&p)),
            ]);
            let routes = v
                .tokens
                .iter()
                .filter(|t| t.auth == c.name)
                .map(|t| {
                    obj(vec![
                        ("route", s(&t.name)),
                        ("url_base", s(&t.url_base)),
                        ("url_prefixes", strings(&t.url_prefixes)),
                        ("max_ttl", s(&go_duration(t.max_ttl))),
                    ])
                })
                .collect();
            m.push(("token_routes", Value::Array(routes)));
            out.push(obj(m));
        }
        json(200, &obj(vec![("policies", Value::Array(out))]))
    }

    /// `GET /media/auth/{policy}/sessions`.
    fn sessions(&self, p: &crate::auth::Policy, v: &View) -> Response {
        let q = |k: &str| crate::http::query_param(&self.query, k).unwrap_or_default();
        let (mut offset, mut limit) = (0usize, DEFAULT_SESSIONS_PAGE);
        for (k, to, max) in [
            ("offset", &mut offset, None),
            ("limit", &mut limit, Some(MAX_SESSIONS_PAGE)),
        ] {
            let x = q(k);
            if x.is_empty() {
                continue;
            }
            match x.parse::<usize>() {
                Ok(n) if max.is_none_or(|m| (1..=m).contains(&n)) => *to = n,
                _ => return error(400, &format!("{k} is not a number in range")),
            }
        }
        let (subject, resource) = (q("subject"), q("resource"));
        let all = p.sessions();
        let mut page = Vec::new();
        let mut matched = 0usize;
        for ss in &all {
            if (!subject.is_empty() && ss.subject != subject) || !ss.resource.starts_with(&resource)
            {
                continue;
            }
            if matched >= offset && page.len() < limit {
                page.push(ss);
            }
            matched = matched.saturating_add(1);
        }
        let mut counts: Vec<(String, u64)> = Vec::new();
        for ss in &page {
            if counts.last().is_none_or(|(c, _)| *c != ss.subject) {
                counts.push((ss.subject.clone(), 0));
            }
        }
        for ss in all.iter().filter(|s| !s.ended) {
            if let Ok(i) = counts.binary_search_by(|(c, _)| c.as_str().cmp(&ss.subject)) {
                if let Some(c) = counts.get_mut(i) {
                    c.1 = c.1.saturating_add(1);
                }
            }
        }
        let max_sessions = v
            .policies()
            .iter()
            .find(|c| c.name == p.name())
            .and_then(|c| c.url_signing.as_ref())
            .map_or(0, |u| u.max_sessions);
        let page_v = page.iter().map(|ss| session_value(ss)).collect();
        let subjects_v = counts
            .into_iter()
            .map(|(subj, k)| obj(vec![("subject", s(&subj)), ("sessions", n(k))]))
            .collect();
        json(
            200,
            &obj(vec![
                ("limit", n(limit as u64)),
                ("matched", n(matched as u64)),
                ("max_sessions", n(max_sessions as u64)),
                ("offset", n(offset as u64)),
                ("page", Value::Array(page_v)),
                ("subjects", Value::Array(subjects_v)),
                ("total", n(all.len() as u64)),
            ]),
        )
    }

    /// `POST /media/auth/{policy}/sessions/end`.
    fn end_session(&self, p: &crate::auth::Policy) -> Response {
        let m = match members(&self.body) {
            Ok(m) => m,
            Err(r) => return *r,
        };
        let (subject, id) = match (str_of(&m, "subject"), str_of(&m, "id")) {
            (Ok(a), Ok(b)) => (a, b),
            (Err(r), _) | (_, Err(r)) => return *r,
        };
        if subject.is_empty() || id.is_empty() {
            return error(400, "the body names a session: {\"subject\", \"id\"}");
        }
        if !p.end_session(&subject, &id) {
            return error(404, "the policy has no such session playing");
        }
        json(200, &obj(vec![("ended", Value::Bool(true))]))
    }

    /// `POST /media/auth/{policy}/rotate`: each secret written anew to its file, then used.
    fn rotate(&self, p: &crate::auth::Policy) -> Response {
        let m = match members(&self.body) {
            Ok(m) => m,
            Err(r) => return *r,
        };
        let (key, all) = match (str_of(&m, "key"), bool_of(&m, "all")) {
            (Ok(a), Ok(b)) => (a, b),
            (Err(r), _) | (_, Err(r)) => return *r,
        };
        let keys = p.keys();
        let mut targets: Vec<(String, String)> = Vec::new();
        if keys.is_empty() {
            if !key.is_empty() || all {
                return error(400, "the policy has a single secret: name no key");
            }
            let f = p.secret_file();
            if f.is_empty() {
                return error(409, "the secret is written in the config: rotate it there");
            }
            targets.push((String::new(), f));
        } else if all {
            if !key.is_empty() {
                return error(400, "name a key, or all, not both");
            }
            for k in keys {
                match p.key_file(&k).filter(|f| !f.is_empty()) {
                    Some(f) => targets.push((k, f)),
                    None => {
                        return error(
                            409,
                            &format!("the key {k:?} is written in the config: rotate it there"),
                        );
                    }
                }
            }
        } else if key.is_empty() {
            return error(400, "name a key to rotate, or all: true");
        } else {
            match p.key_file(&key) {
                Some(f) if !f.is_empty() => targets.push((key, f)),
                Some(_) => {
                    return error(
                        409,
                        &format!("the key {key:?} is written in the config: rotate it there"),
                    );
                }
                None => return error(404, &format!("no key named {key:?}")),
            }
        }
        let mut rotated = Vec::new();
        for (k, file) in &targets {
            let secret = crate::tokens::new_secret();
            let ok = !secret.is_empty() && crate::tokens::write_secret(file, &secret).is_ok();
            let used = ok
                && if k.is_empty() {
                    p.rotate(&secret).is_ok()
                } else {
                    p.rotate_key(k, &secret).is_ok()
                };
            if !used {
                // Neither the error nor the answer names the file.
                let done: Vec<String> = rotated.iter().map(|r: &String| format!("{r:?}")).collect();
                return error(
                    500,
                    &format!(
                        "the new secret was not written; keys rotated before: [{}]",
                        done.join(" ")
                    ),
                );
            }
            rotated.push(k.clone());
        }
        json(
            200,
            &obj(vec![
                ("rotated", strings(&rotated)),
                ("since", ni(now_secs())),
            ]),
        )
    }

    /// `POST /media/auth/{policy}/urls`.
    fn test_url(&self, p: &crate::auth::Policy, v: &View) -> Response {
        if !p.url_signing() {
            return error(404, "the media_auth policy has no url_signing");
        }
        let m = match members(&self.body) {
            Ok(m) => m,
            Err(r) => return *r,
        };
        let mut f = [""; 5].map(str::to_owned);
        for (i, k) in ["subject", "path", "address", "ttl", "route"]
            .iter()
            .enumerate()
        {
            match str_of(&m, k) {
                Ok(x) => {
                    if let Some(slot) = f.get_mut(i) {
                        *slot = x;
                    }
                }
                Err(r) => return *r,
            }
        }
        let [subject, path, address, ttl_s, route] = f;
        if !valid_subject(&subject) {
            return error(
                400,
                &format!("{subject:?} is not a subject: 1 to 128 bytes with no control characters"),
            );
        }
        let mut viewer = crate::auth::Viewer {
            subject: subject.clone(),
            address: None,
            session: new_session(),
        };
        if !address.is_empty() {
            match address.parse::<IpAddr>() {
                Ok(a) => viewer.address = Some(a),
                Err(_) => return error(400, &format!("{address:?} is not an IP address")),
            }
        }
        let Some(res) = crate::urlsign::resource_of(&path) else {
            return error(
                400,
                &format!(
                    "{path:?} is not a path a URL is signed for, as /vod/films/films/bbb/index.m3u8 or /live/cam.flv"
                ),
            );
        };
        let (mut max_ttl, mut base) = (MAX_TEST_TTL, String::new());
        let mut ttl = p.url_ttl().min(max_ttl);
        if !route.is_empty() {
            let Some(t) = v
                .tokens
                .iter()
                .find(|t| t.name == route && t.auth == p.name())
            else {
                return error(
                    404,
                    &format!("no media_tokens route {route:?} signs for the policy"),
                );
            };
            if !t.url_prefixes.is_empty() && !t.url_prefixes.iter().any(|pre| path.starts_with(pre))
            {
                return error(
                    400,
                    &format!(
                        "{path:?} starts with none of the url_prefixes the route may sign for: {}",
                        t.url_prefixes.join(", ")
                    ),
                );
            }
            max_ttl = t.max_ttl;
            base.clone_from(&t.url_base);
            ttl = p.url_ttl().min(max_ttl);
        } else if let Some(l) = v.route_of(&path).and_then(|rt| v.listener(rt)) {
            base = l.origin(&self.host);
        }
        if !ttl_s.is_empty() {
            match akit_config::duration::parse(&ttl_s) {
                Some(Ok(d)) if d >= Duration::from_secs(1) => {
                    if d > max_ttl {
                        return error(
                            400,
                            &format!(
                                "the lifetime {} is longer than {}",
                                go_duration(d),
                                go_duration(max_ttl)
                            ),
                        );
                    }
                    ttl = d;
                }
                _ => {
                    return error(
                        400,
                        &format!("{ttl_s:?} is not a duration of a second or more, as 10m"),
                    );
                }
            }
        }
        let expiry = now_secs().saturating_add(i64::try_from(ttl.as_secs()).unwrap_or(i64::MAX));
        match p.sign_url(&res, &viewer, expiry, "") {
            Ok(q) => no_store(&obj(vec![
                ("expires", ni(expiry)),
                ("session", s(&viewer.session)),
                ("subject", s(&subject)),
                ("url", s(&format!("{base}{path}?{q}"))),
            ])),
            Err(_) => error(503, "the URL cannot be signed: the policy changed"),
        }
    }

    /// `GET /media/stats`.
    fn stats(&self, v: &View) -> Response {
        let libs: Vec<Value> = crate::library::libraries(self.key)
            .iter()
            .map(|l| {
                let sum = crate::library::admin::summary(l);
                let pick = |k: &str| {
                    sum.iter()
                        .find(|(x, _)| *x == k)
                        .map_or(Value::Null, |(_, v)| v.clone())
                };
                obj(vec![
                    ("name", pick("name")),
                    ("items", pick("items")),
                    ("errors", pick("errors")),
                    ("pool", pick("pool")),
                    ("tables", pick("tables")),
                ])
            })
            .collect();
        let mut pols = Vec::new();
        for c in v.policies() {
            let Some(p) = crate::auth::registry::lookup(self.key, &c.name) else {
                continue;
            };
            let live: Vec<_> = p.sessions().into_iter().filter(|s| !s.ended).collect();
            let mut by: Vec<(String, u64)> = Vec::new();
            for ss in &live {
                match by.iter_mut().find(|(r, _)| *r == ss.resource) {
                    Some(e) => e.1 = e.1.saturating_add(1),
                    None => by.push((ss.resource.clone(), 1)),
                }
            }
            by.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
            by.truncate(MAX_STATS_RESOURCES);
            pols.push(obj(vec![
                ("name", s(p.name())),
                ("sessions", n(live.len() as u64)),
                ("refusals", refusals(&p)),
                (
                    "resources",
                    Value::Array(
                        by.into_iter()
                            .map(|(r, k)| obj(vec![("resource", s(&r)), ("sessions", n(k))]))
                            .collect(),
                    ),
                ),
            ]));
        }
        let mut chs = Vec::new();
        if let Some(m) = crate::channels::lookup(self.key) {
            let pushes = m.pushes();
            for c in m.channels() {
                let push = pushes
                    .iter()
                    .filter(|p| p.channel == c.name)
                    .map(|p| {
                        let mut o = vec![("name", s(&p.name)), ("state", s(p.state))];
                        if !p.error.is_empty() {
                            o.push(("error", s(&p.error)));
                        }
                        o.extend([("bytes", n(p.bytes)), ("bitrate", n(p.bitrate))]);
                        obj(o)
                    })
                    .collect();
                chs.push(obj(vec![
                    ("name", s(&c.name)),
                    ("library", s(&c.library)),
                    ("state", s(c.state)),
                    ("players", n(m.subscribers(&c.name) as u64)),
                    ("push", Value::Array(push)),
                ]));
            }
        }
        let millis = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_or(0, |d| u64::try_from(d.as_millis()).unwrap_or(u64::MAX));
        json(
            200,
            &obj(vec![
                ("channels", Value::Array(chs)),
                ("libraries", Value::Array(libs)),
                ("policies", Value::Array(pols)),
                ("time", n(millis)),
                ("vod", crate::vod::stats_value()),
            ]),
        )
    }
}

/// A session as the endpoints list it.
fn session_value(ss: &crate::urlsign::SessionView) -> Value {
    let mut m = vec![
        ("subject", s(&ss.subject)),
        ("id", s(&ss.id)),
        ("resource", s(&ss.resource)),
        ("address", s(&ss.address)),
        ("start", nanos_time(ss.start)),
        ("last", nanos_time(ss.last)),
    ];
    if ss.ended {
        m.push(("ended", Value::Bool(true)));
    }
    obj(m)
}

/// A policy's URL refusals by reason, the reasons sorted, as Go writes a map.
fn refusals(p: &crate::auth::Policy) -> Value {
    let mut r = p.url_refusals();
    r.sort_by(|a, b| a.0.cmp(b.0));
    obj(r.into_iter().map(|(k, v)| (k, n(v))).collect())
}

/// A policy's keys, never their secrets nor their files' paths: a policy with a single secret
/// has one, named "". Their files' times are read here, on the pool.
fn keys_of(p: &crate::auth::Policy) -> Value {
    let modified = |file: &str| -> Option<Value> {
        std::fs::metadata(file)
            .and_then(|m| m.modified())
            .ok()
            .map(time)
    };
    let key = |name: &str, default: bool, file: &str| {
        let mut m = vec![
            ("name", s(name)),
            ("default", Value::Bool(default)),
            ("file", Value::Bool(!file.is_empty())),
        ];
        if !file.is_empty() {
            if let Some(t) = modified(file) {
                m.push(("modified", t));
            }
        }
        obj(m)
    };
    let names = p.keys();
    if names.is_empty() {
        return Value::Array(vec![key("", true, &p.secret_file())]);
    }
    Value::Array(
        names
            .iter()
            .enumerate()
            .map(|(i, k)| key(k, i == 0, &p.key_file(k).unwrap_or_default()))
            .collect(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hosts() {
        for (h, want) in [
            ("example.com:9400", "example.com"),
            ("[::1]:9400", "::1"),
            ("10.0.0.1", "10.0.0.1"),
            ("0.0.0.0:80", ""),
            ("a b", ""),
            ("[::1", ""),
            ("", ""),
        ] {
            assert_eq!(host_of(h), want, "{h}");
        }
        let l = Listener {
            name: "web".into(),
            scheme: "http",
            host: String::new(),
            port: 8080,
        };
        assert_eq!(
            l.origin("console.example:9400"),
            "http://console.example:8080"
        );
        assert_eq!(l.origin(""), "http://127.0.0.1:8080");
    }
}
