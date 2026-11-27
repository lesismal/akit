//! The Admin API's endpoints for the BitTorrent roles (I157, fit's `admin/bittorrent.go` and
//! `admin/btevents.go`), which `akit bt` and the pages call, served through akit-admin's
//! [`Endpoint`](akit_admin::Endpoint) below `/bittorrent` (`Options::endpoints`, their bodies
//! streamed in through `Options::streamed`):
//!
//! ```text
//! GET    /bittorrent/torrents                 the torrents, in queue order: {"torrents",
//!                                             "total", "offset"}, filtered by state (a list
//!                                             with commas), category, q (in the name), paged
//!                                             by offset and limit
//! POST   /bittorrent/torrents                 add one: JSON {"magnet"|"url"|"torrent"
//!                                             (base64), "category", "paused", "peers",
//!                                             "limits"}; a .torrent file as
//!                                             application/x-bittorrent, the options in the
//!                                             query; or a form, multipart/form-data
//! GET    /bittorrent/torrents/{id}            one, with its files
//! POST   /bittorrent/torrents/{id}            {"action": "pause" | "resume" | "recheck"}
//! PATCH  /bittorrent/torrents/{id}            {"paused", "category", "limits" (null clears
//!                                             them), "queue_position"}
//! DELETE /bittorrent/torrents/{id}            remove it; ?delete_data=1 with its files
//! GET    /bittorrent/torrents/{id}/files      its files
//! GET    /bittorrent/torrents/{id}/peers      the peers connected
//! GET    /bittorrent/torrents/{id}/trackers   its trackers and web seeds
//! GET    /bittorrent/alt_speed                the alternative rates; PUT {"mode"}
//! GET    /bittorrent/publish                  the publish entries; .../{name} one;
//!                                             .../{name}/torrents/{id} a .torrent file
//! GET    /bittorrent/events                   the client's events, server-sent
//! GET    /bittorrent/tracker/stats            the tracker's totals
//! GET    /bittorrent/tracker/torrents         the allow-list's swarms (offset, limit);
//!                                             .../{hash} one torrent's swarm
//! GET    /bittorrent/tracker/passkey?user=    a private tracker's user's passkey
//! ```
//!
//! A torrent's `{id}` is its v1 info hash in hex, or its v2 hash, whose first 20 bytes are the
//! ID. The client's API reads and writes its state files: every call but the event stream's is
//! made on akio's logical pool, the request detached meanwhile, never on the API's I/O thread.
//! What akio cannot do yet is refused by name, 501: a sequential download and file priorities
//! (akio's A287), a torrent's `max_conns` and `upload_slots` (A284). A torrent's trackers are
//! listed with their state unknown and a recheck adds the torrent again, until akio's A288
//! ([`crate::client`]'s `inspect`).

use std::fmt::Write as _;
use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

use akio_http::{BodyEvent, Context, Headers, Method, Response};
use akit_config::bittorrent::{BitTorrentTorrentLimits, parse_info_hash};
use akit_config::value::{Value, write_string};
use akit_roles::host::{Apis, api_in};
use akit_roles::pages::{fail, json};

use crate::client::inspect::{AltSpeedStatus, FileStatus, PeerStatus, Trackers};
use crate::client::pages::{files_json, status_json, status_json_with};
use crate::client::publish_handler::entry_json;
use crate::client::{AddRequest, ApiError, ClientApi, Source, Status};

/// The paths the endpoints serve: this one and those below it.
pub const PREFIX: &str = "/bittorrent";

/// The endpoints, each path's pattern (`{x}` one segment) and its methods; GET serves HEAD too.
pub const ROUTES: &[(&str, &[&str])] = &[
    ("/bittorrent/torrents", &["GET", "POST"]),
    (
        "/bittorrent/torrents/{id}",
        &["GET", "POST", "PATCH", "DELETE"],
    ),
    ("/bittorrent/torrents/{id}/files", &["GET"]),
    ("/bittorrent/torrents/{id}/peers", &["GET"]),
    ("/bittorrent/torrents/{id}/trackers", &["GET"]),
    ("/bittorrent/alt_speed", &["GET", "PUT"]),
    ("/bittorrent/publish", &["GET"]),
    ("/bittorrent/publish/{name}", &["GET"]),
    ("/bittorrent/publish/{name}/torrents/{id}", &["GET"]),
    ("/bittorrent/events", &["GET"]),
    ("/bittorrent/tracker/stats", &["GET"]),
    ("/bittorrent/tracker/torrents", &["GET"]),
    ("/bittorrent/tracker/torrents/{hash}", &["GET"]),
    ("/bittorrent/tracker/passkey", &["GET"]),
];

/// fit's bounds: a .torrent file sent (as one fetched by URL), the JSON of an add with one in
/// base64, a form's other fields and parts, the other bodies, a page of torrents.
const MAX_UPLOAD_BYTES: usize = crate::client::MAX_URL_BYTES;
const MAX_ADD_JSON_BYTES: usize = MAX_UPLOAD_BYTES / 3 * 4 + (1 << 20);
const MAX_FORM_FIELD_BYTES: usize = 64 << 10;
const MAX_FORM_PARTS: usize = 16;
const MAX_PATCH_BYTES: usize = 1 << 20;
const DEFAULT_TORRENTS_PAGE: usize = 1000;
const MAX_TORRENTS_PAGE: usize = 10_000;

/// The endpoints for akit-admin's `Options::endpoints`, below [`PREFIX`], reading the running
/// roles from `apis`; their bodies stream in (`Options::streamed`, [`PREFIX`] among them).
#[must_use]
pub fn endpoint(apis: Apis) -> (&'static str, akit_admin::Endpoint) {
    (
        PREFIX,
        Arc::new(move |ctx: &mut Context| {
            let path = ctx.path().to_owned();
            serve(&apis, ctx, &path);
            true
        }),
    )
}

/// The route `path` is, and the segments its placeholders matched, percent-decoded.
fn route_of(path: &str) -> Option<(usize, Vec<String>)> {
    'routes: for (i, (pattern, _)) in ROUTES.iter().enumerate() {
        let mut p = pattern.split('/');
        let mut c = path.split('/');
        let mut params = Vec::new();
        loop {
            match (p.next(), c.next()) {
                (None, None) => return Some((i, params)),
                (Some(a), Some(seg)) if a.starts_with('{') => {
                    if seg.is_empty() {
                        continue 'routes;
                    }
                    match akio_http::message::percent_decode(seg) {
                        Some(d) => params.push(d),
                        None => continue 'routes,
                    }
                }
                (Some(a), Some(b)) if a == b => {}
                _ => continue 'routes,
            }
        }
    }
    None
}

fn not_allowed(methods: &[&str]) -> Response {
    let mut allow: Vec<&str> = methods.to_vec();
    if allow.contains(&"GET") {
        allow.insert(1, "HEAD");
    }
    let mut r = fail(405, "method not allowed");
    r.headers.set("Allow", allow.join(", "));
    r
}

/// An error of the client's API, as fit's `btError` answers it.
fn api_error(e: &ApiError) -> Response {
    let status = match e {
        ApiError::NotFound => 404,
        ApiError::Exists(_) => 409,
        ApiError::TooMany => 429,
        ApiError::Busy | ApiError::NotRunning | ApiError::TooManySubscribers => 503,
        ApiError::BadRequest(_) => 400,
        ApiError::Fetch(_) => 502,
        ApiError::NotSupported(_) => 501,
        ApiError::Io(_) => 500,
    };
    let mut r = fail(status, &e.to_string());
    if status == 503 {
        r.headers.set("Retry-After", "5");
    }
    r
}

fn not_supported(what: &str) -> ApiError {
    ApiError::NotSupported(what.to_owned())
}

/// A query parameter, `+` and percent-escapes decoded as Go's `Query` does; `None` when absent.
fn query(ctx: &Context, key: &str) -> Option<String> {
    let q = ctx.req().query()?;
    q.split('&')
        .filter_map(|kv| kv.split_once('=').or(Some((kv, ""))))
        .find(|(k, _)| *k == key)
        .map(|(_, v)| akio_http::message::percent_decode(&v.replace('+', " ")).unwrap_or_default())
}

/// A flag of a query or a form: 1, true, on; 0, false or none.
fn flag(name: &str, v: &str) -> Result<bool, String> {
    match v {
        "" | "0" | "false" => Ok(false),
        "1" | "true" | "on" => Ok(true),
        _ => Err(format!("{name} is 1 or true, 0 or false, not {v:?}")),
    }
}

/// The ID a request's `{id}` names: 40 hex digits, or 64 of a v2 hash, cut to 40.
fn torrent_id(s: &str) -> Result<String, Box<Response>> {
    parse_info_hash(s)
        .map(|h| crate::client::hex(&h))
        .ok_or_else(|| {
            Box::new(fail(
                404,
                "a torrent's ID is its info hash: 40 hex digits (v1) or 64 (v2)",
            ))
        })
}

fn client_of(apis: &Apis) -> Option<ClientApi> {
    api_in(apis, crate::client::ID).and_then(|a| ClientApi::of(&a))
}

fn no_client() -> Response {
    fail(404, "the bittorrent.client role does not run here")
}

/// Answers `f`'s response on akio's logical pool.
fn on_pool(ctx: &mut Context, f: impl FnOnce() -> Response + Send + 'static) {
    ctx.detach_to_pool(move |d| d.respond(f()));
}

fn serve(apis: &Apis, ctx: &mut Context, path: &str) {
    let Some((i, params)) = route_of(path) else {
        return ctx.respond(fail(404, "there is no such endpoint"));
    };
    let Some(&(pattern, methods)) = ROUTES.get(i) else {
        return ctx.respond(fail(404, "there is no such endpoint"));
    };
    let method = ctx.method().clone();
    let name = match method {
        Method::Head => "GET",
        ref m => m.as_str(),
    };
    if !methods.contains(&name) {
        return ctx.respond(not_allowed(methods));
    }
    if pattern == "/bittorrent/tracker/passkey" {
        // Off the API's I/O thread: a policy not found yet is looked up in akit-media's
        // registry.
        let (apis, target) = (Arc::clone(apis), ctx.req().target.clone());
        return on_pool(ctx, move || tracker_answer(&apis, &target, "passkey", &[]));
    }
    if let Some(rest) = pattern.strip_prefix("/bittorrent/tracker/") {
        return ctx.respond(tracker(apis, ctx, rest, &params));
    }
    let Some(api) = client_of(apis) else {
        return ctx.respond(no_client());
    };
    let p0 = params.first().cloned().unwrap_or_default();
    match (pattern, name) {
        ("/bittorrent/torrents", "GET") => list(api, ctx),
        ("/bittorrent/torrents", _) => add(api, ctx),
        ("/bittorrent/events", _) => events(&api, ctx),
        ("/bittorrent/alt_speed", "GET") => on_pool(ctx, move || alt_answer(&api)),
        ("/bittorrent/alt_speed", _) => {
            read_json(ctx, MAX_PATCH_BYTES, move |ctx, v| {
                let mode = match v.get("mode") {
                    Some(Value::String(m)) => m.clone(),
                    None => String::new(),
                    Some(_) => return ctx.respond(fail(400, "the body: mode is a string")),
                };
                on_pool(ctx, move || match api.set_alt_speed(&mode) {
                    Ok(_) => alt_answer(&api),
                    Err(e) => api_error(&e),
                });
            });
        }
        ("/bittorrent/publish", _) => on_pool(ctx, move || {
            let mut b = String::from("{\"entries\":[");
            for (i, e) in api.publishing().iter().enumerate() {
                if i > 0 {
                    b.push(',');
                }
                entry_json(&mut b, e, false);
            }
            b.push_str("]}\n");
            json(200, b)
        }),
        ("/bittorrent/publish/{name}", _) => on_pool(ctx, move || {
            match api.publishing().into_iter().find(|e| e.name == p0) {
                Some(e) => {
                    let mut b = String::new();
                    entry_json(&mut b, &e, false);
                    b.push('\n');
                    json(200, b)
                }
                None => fail(404, &format!("no publish entry is named {p0:?}")),
            }
        }),
        ("/bittorrent/publish/{name}/torrents/{id}", _) => {
            let id = params
                .get(1)
                .cloned()
                .unwrap_or_default()
                .to_ascii_lowercase();
            on_pool(ctx, move || {
                let found = api
                    .publishing()
                    .into_iter()
                    .filter(|e| e.name == p0)
                    .flat_map(|e| e.torrents)
                    .find(|t| t.id == id);
                match found {
                    Some(t) => {
                        let mut r = Response::with_body(
                            200,
                            "application/x-bittorrent",
                            t.torrent.as_ref().clone(),
                        );
                        let mut d = String::from("attachment; filename=");
                        write_string(&mut d, &format!("{}.torrent", t.name));
                        r.headers.set("Content-Disposition", &d);
                        r
                    }
                    None => fail(404, &format!("no torrent {id:?} is published by {p0:?}")),
                }
            });
        }
        _ => {
            let id = match torrent_id(&p0) {
                Ok(id) => id,
                Err(r) => return ctx.respond(*r),
            };
            match (pattern, name) {
                ("/bittorrent/torrents/{id}", "GET") => on_pool(ctx, move || {
                    match api.get(&id).and_then(|s| Ok((s, api.files(&id)?))) {
                        Ok((s, files)) => status_answer(200, &s, Some(&files)),
                        Err(e) => api_error(&e),
                    }
                }),
                ("/bittorrent/torrents/{id}", "POST") => action(api, ctx, id),
                ("/bittorrent/torrents/{id}", "PATCH") => patch(api, ctx, id),
                ("/bittorrent/torrents/{id}", _) => {
                    let del = match flag(
                        "delete_data",
                        &query(ctx, "delete_data").unwrap_or_default(),
                    ) {
                        Ok(d) => d,
                        Err(e) => return ctx.respond(fail(400, &e)),
                    };
                    on_pool(ctx, move || match api.remove(&id, del) {
                        Ok(()) => {
                            let mut b = String::from("{\"removed\":");
                            write_string(&mut b, &id);
                            let _ = writeln!(b, ",\"data_deleted\":{del}}}");
                            json(200, b)
                        }
                        Err(e) => api_error(&e),
                    });
                }
                ("/bittorrent/torrents/{id}/files", _) => {
                    on_pool(ctx, move || match api.files(&id) {
                        Ok(f) => {
                            let mut b = String::from("{\"files\":");
                            files_json(&mut b, &f);
                            b.push_str("}\n");
                            json(200, b)
                        }
                        Err(e) => api_error(&e),
                    });
                }
                ("/bittorrent/torrents/{id}/peers", _) => {
                    on_pool(ctx, move || match api.peers(&id) {
                        Ok(p) => json(200, peers_json(&p)),
                        Err(e) => api_error(&e),
                    });
                }
                _ => on_pool(ctx, move || match api.trackers(&id) {
                    Ok(t) => json(200, trackers_json(&t)),
                    Err(e) => api_error(&e),
                }),
            }
        }
    }
}

fn status_answer(code: u16, s: &Status, files: Option<&[FileStatus]>) -> Response {
    let mut b = String::new();
    status_json_with(&mut b, s, files);
    b.push('\n');
    json(code, b)
}

fn alt_answer(api: &ClientApi) -> Response {
    match api.alt_speed_status() {
        Ok(a) => json(200, alt_json(&a)),
        Err(e) => api_error(&e),
    }
}

fn alt_json(a: &AltSpeedStatus) -> String {
    let mut b = String::from("{\"mode\":");
    write_string(&mut b, &a.mode);
    let _ = writeln!(
        b,
        ",\"configured\":{},\"active\":{},\"upload_rate\":{},\"download_rate\":{}}}",
        a.configured, a.active, a.upload_rate, a.download_rate
    );
    b
}

fn peers_json(ps: &[PeerStatus]) -> String {
    let mut b = String::from("{\"peers\":[");
    for (i, p) in ps.iter().enumerate() {
        if i > 0 {
            b.push(',');
        }
        b.push_str("{\"addr\":");
        write_string(&mut b, &p.addr);
        b.push_str(",\"id\":");
        write_string(&mut b, &p.id);
        if !p.client.is_empty() {
            b.push_str(",\"client\":");
            write_string(&mut b, &p.client);
        }
        let _ = write!(
            b,
            ",\"outgoing\":{},\"utp\":{},\"encrypted\":{},\"seed\":{},\"pieces\":{},\"downloaded\":{},\"uploaded\":{},\"am_choking\":{},\"peer_choking\":{}}}",
            p.outgoing,
            p.utp,
            p.encrypted,
            p.seed,
            p.pieces,
            p.downloaded,
            p.uploaded,
            p.am_choking,
            p.peer_choking
        );
    }
    b.push_str("]}\n");
    b
}

/// fit's `Trackers`, every tracker's state unknown (fit's `unknownAnnounces`) and no sources,
/// until akio's A288.
fn trackers_json(t: &Trackers) -> String {
    let mut b = String::from("{\"trackers\":[");
    for (i, tr) in t.trackers.iter().enumerate() {
        if i > 0 {
            b.push(',');
        }
        b.push_str("{\"url\":");
        write_string(&mut b, &tr.url);
        let _ = write!(
            b,
            ",\"tier\":{},\"updating\":false,\"fails\":0,\"interval\":0,\"min_interval\":0,\"peers\":0,\"seeders\":-1,\"leechers\":-1,\"downloaded\":-1}}",
            tr.tier
        );
    }
    b.push_str("],\"sources\":[],\"web_seeds\":[");
    for (i, u) in t.web_seeds.iter().enumerate() {
        if i > 0 {
            b.push(',');
        }
        b.push_str("{\"url\":");
        write_string(&mut b, u);
        b.push_str(",\"active\":0,\"fails\":0,\"dropped\":false}");
    }
    b.push_str("]}\n");
    b
}

/// `GET /bittorrent/torrents`.
fn list(api: ClientApi, ctx: &mut Context) {
    let page = |key: &str, def: usize, max: usize| -> Result<usize, String> {
        match query(ctx, key) {
            None => Ok(def),
            Some(v) if v.is_empty() => Ok(def),
            Some(v) => match v.parse::<usize>() {
                Ok(n) if key == "limit" && (n == 0 || n > max) => {
                    Err(format!("limit is 1 to {max}, not {v}"))
                }
                Ok(n) => Ok(n),
                Err(_) => Err(format!("{key} is a number 0 or more, not {v:?}")),
            },
        }
    };
    let (offset, limit) = match (
        page("offset", 0, usize::MAX),
        page("limit", DEFAULT_TORRENTS_PAGE, MAX_TORRENTS_PAGE),
    ) {
        (Ok(o), Ok(l)) => (o, l),
        (Err(e), _) | (_, Err(e)) => return ctx.respond(fail(400, &e)),
    };
    let mut states = Vec::new();
    if let Some(v) = query(ctx, "state").filter(|v| !v.is_empty()) {
        for st in v.split(',').map(str::trim) {
            if !crate::client::STATES.contains(&st) {
                return ctx.respond(fail(
                    400,
                    &format!(
                        "a state is one of {}, not {st:?}",
                        crate::client::STATES.join(", ")
                    ),
                ));
            }
            states.push(st.to_owned());
        }
    }
    let category = query(ctx, "category");
    let text = query(ctx, "q").unwrap_or_default().to_lowercase();
    on_pool(ctx, move || {
        let all = match api.list() {
            Ok(l) => l,
            Err(e) => return api_error(&e),
        };
        let matched: Vec<Status> = all
            .into_iter()
            .filter(|s| {
                (states.is_empty() || states.contains(&s.state))
                    && category.as_ref().is_none_or(|c| *c == s.category)
                    && (text.is_empty() || s.name.to_lowercase().contains(&text))
            })
            .collect();
        let mut b = String::from("{\"torrents\":[");
        for (i, s) in matched.iter().skip(offset).take(limit).enumerate() {
            if i > 0 {
                b.push(',');
            }
            status_json(&mut b, s);
        }
        let _ = writeln!(b, "],\"total\":{},\"offset\":{offset}}}", matched.len());
        json(200, b)
    });
}

/// What an add asks for.
#[derive(Default)]
struct AddBody {
    magnet: String,
    url: String,
    torrent: Vec<u8>,
    category: String,
    paused: bool,
    sequential: bool,
    priorities: bool,
    peers: Vec<SocketAddr>,
    limits: Option<BitTorrentTorrentLimits>,
}

/// A media type's name, lower case, its parameters apart.
fn media_type(ctx: &Context) -> (String, String) {
    let ct = ctx.header("Content-Type").unwrap_or("");
    let (t, params) = ct.split_once(';').unwrap_or((ct, ""));
    (t.trim().to_ascii_lowercase(), params.to_owned())
}

/// `POST /bittorrent/torrents`.
fn add(api: ClientApi, ctx: &mut Context) {
    let (mt, params) = media_type(ctx);
    match mt.as_str() {
        "application/json" => {
            read_body(
                ctx,
                MAX_ADD_JSON_BYTES,
                "the body",
                move |ctx, data| match add_json(&data) {
                    Ok(b) => add_now(api, ctx, b),
                    Err(e) => ctx.respond(fail(400, &e)),
                },
            );
        }
        "application/x-bittorrent" => {
            let mut b = AddBody {
                category: query(ctx, "category").unwrap_or_default(),
                ..AddBody::default()
            };
            let flags = flag("paused", &query(ctx, "paused").unwrap_or_default()).and_then(|p| {
                b.paused = p;
                flag("sequential", &query(ctx, "sequential").unwrap_or_default())
            });
            match flags {
                Ok(s) => b.sequential = s,
                Err(e) => return ctx.respond(fail(400, &e)),
            }
            read_body(
                ctx,
                MAX_UPLOAD_BYTES,
                "the .torrent file",
                move |ctx, data| {
                    b.torrent = data;
                    add_now(api, ctx, b);
                },
            );
        }
        "multipart/form-data" => {
            let boundary = params
                .split(';')
                .filter_map(|p| p.trim().split_once('='))
                .find(|(k, _)| k.eq_ignore_ascii_case("boundary"))
                .map(|(_, v)| v.trim_matches('"').to_owned())
                .unwrap_or_default();
            if boundary.is_empty() {
                return ctx.respond(fail(400, "the form has no boundary"));
            }
            read_body(
                ctx,
                MAX_UPLOAD_BYTES + MAX_FORM_PARTS * MAX_FORM_FIELD_BYTES,
                "the form",
                move |ctx, data| match add_form(&data, &boundary) {
                    Ok(b) => add_now(api, ctx, b),
                    Err((code, e)) => ctx.respond(fail(code, &e)),
                },
            );
        }
        _ => ctx.respond(fail(
            415,
            "the body must be sent as application/json, application/x-bittorrent or \
             multipart/form-data",
        )),
    }
}

/// The limits a body gives, as fit's `btLimits`: `seed_time` a duration or nanoseconds.
fn limits_of(v: &Value) -> Result<BitTorrentTorrentLimits, ApiError> {
    let bad = |m: String| ApiError::BadRequest(m);
    if !matches!(v, Value::Object(_)) {
        return Err(bad("limits is an object".into()));
    }
    let int = |k: &str| -> Result<i64, ApiError> {
        match v.get(k) {
            None | Some(Value::Null) => Ok(0),
            Some(Value::Number(n)) => n
                .parse::<i64>()
                .map_err(|_| bad(format!("limits: {k} is a whole number, not {n}"))),
            Some(_) => Err(bad(format!("limits: {k} is a number"))),
        }
    };
    for k in ["max_conns", "upload_slots"] {
        if int(k)? != 0 {
            return Err(not_supported(&format!(
                "a torrent's {k} is not supported yet: it needs akio's A284"
            )));
        }
    }
    let seed_ratio = match v.get("seed_ratio") {
        None | Some(Value::Null) => 0.0,
        Some(Value::Number(n)) => n
            .parse::<f64>()
            .map_err(|_| bad(format!("limits: seed_ratio is a number, not {n}")))?,
        Some(_) => return Err(bad("limits: seed_ratio is a number".into())),
    };
    let seed_time = match v.get("seed_time") {
        None | Some(Value::Null) => Duration::ZERO,
        Some(Value::String(s)) => match akit_config::duration::parse(s) {
            Some(Ok(d)) => d,
            Some(Err(())) => {
                return Err(bad(format!(
                    "seed_time {s:?}: a negative seed time is not taken here; 0 takes the \
                     category's or the client's"
                )));
            }
            None => {
                return Err(bad(format!(
                    "seed_time {s:?} is not a duration, such as \"12h\""
                )));
            }
        },
        Some(Value::Number(n)) => match n.parse::<u64>() {
            Ok(ns) => Duration::from_nanos(ns),
            Err(_) => {
                return Err(bad(format!(
                    "seed_time is a duration, such as \"12h\", or nanoseconds 0 or more, not {n}"
                )));
            }
        },
        Some(_) => {
            return Err(bad(
                "seed_time is a duration, such as \"12h\", or nanoseconds".into(),
            ));
        }
    };
    Ok(BitTorrentTorrentLimits {
        upload_rate: int("upload_rate")?,
        download_rate: int("download_rate")?,
        seed_ratio,
        seed_time,
    })
}

fn parse_body(data: &[u8]) -> Result<Value, String> {
    let text = std::str::from_utf8(data).map_err(|_| "the body: not UTF-8".to_owned())?;
    akit_config::value::parse(text).map_err(|e| format!("the body: {e}"))
}

fn add_json(data: &[u8]) -> Result<AddBody, String> {
    let v = parse_body(data)?;
    if !matches!(v, Value::Object(_)) {
        return Err("the body: an object".into());
    }
    let s = |k: &str| -> Result<String, String> {
        match v.get(k) {
            None | Some(Value::Null) => Ok(String::new()),
            Some(Value::String(s)) => Ok(s.clone()),
            Some(_) => Err(format!("the body: {k} is a string")),
        }
    };
    let b = |k: &str| -> Result<bool, String> {
        match v.get(k) {
            None | Some(Value::Null) => Ok(false),
            Some(Value::Bool(b)) => Ok(*b),
            Some(_) => Err(format!("the body: {k} is true or false")),
        }
    };
    let mut out = AddBody {
        magnet: s("magnet")?,
        url: s("url")?,
        category: s("category")?,
        paused: b("paused")?,
        sequential: b("sequential")?,
        ..AddBody::default()
    };
    let t = s("torrent")?;
    if !t.is_empty() {
        out.torrent = akio_crypto::base64::STANDARD
            .decode(t.as_bytes())
            .map_err(|_| "the body: torrent is a .torrent file in base64".to_owned())?;
    }
    match v.get("priorities") {
        None | Some(Value::Null) => {}
        Some(Value::Array(a)) => out.priorities = !a.is_empty(),
        Some(_) => return Err("the body: priorities is a list".into()),
    }
    match v.get("peers") {
        None | Some(Value::Null) => {}
        Some(Value::Array(a)) => {
            for p in a {
                let Value::String(p) = p else {
                    return Err("the body: peers is a list of addresses".into());
                };
                out.peers.push(
                    p.parse()
                        .map_err(|_| format!("the body: peer {p:?} is not an address and port"))?,
                );
            }
        }
        Some(_) => return Err("the body: peers is a list".into()),
    }
    match v.get("limits") {
        None | Some(Value::Null) => {}
        Some(l) => out.limits = Some(limits_of(l).map_err(|e| e.to_string())?),
    }
    Ok(out)
}

/// The parts of a form, fit's `readAddForm`: the file in `torrent`, and `magnet`, `url`,
/// `category`, `paused` and `sequential`, each bounded.
fn add_form(data: &[u8], boundary: &str) -> Result<AddBody, (u16, String)> {
    let bad = |m: String| (400, m);
    let delim = format!("--{boundary}");
    let d = delim.as_bytes();
    let find = |hay: &[u8], from: usize, needle: &[u8]| -> Option<usize> {
        hay.get(from..)?
            .windows(needle.len())
            .position(|w| w == needle)
            .map(|p| p.saturating_add(from))
    };
    let mut out = AddBody::default();
    let mut at =
        find(data, 0, d).ok_or_else(|| bad("reading the form: no first boundary".into()))?;
    let mut n = 0usize;
    loop {
        at = at.saturating_add(d.len());
        if data.get(at..at.saturating_add(2)) == Some(b"--") {
            break;
        }
        // The line after the boundary.
        at = find(data, at, b"\r\n")
            .ok_or_else(|| bad("reading the form: a part is cut".into()))?
            .saturating_add(2);
        if n >= MAX_FORM_PARTS {
            return Err(bad(format!(
                "the form has more than {MAX_FORM_PARTS} parts"
            )));
        }
        n = n.saturating_add(1);
        let head_end = find(data, at, b"\r\n\r\n")
            .ok_or_else(|| bad("reading the form: a part's header is cut".into()))?;
        let head = String::from_utf8_lossy(data.get(at..head_end).unwrap_or_default()).into_owned();
        let body_start = head_end.saturating_add(4);
        let mut sep = b"\r\n".to_vec();
        sep.extend_from_slice(d);
        let body_end = find(data, body_start, &sep)
            .ok_or_else(|| bad("reading the form: a part has no end".into()))?;
        let body = data.get(body_start..body_end).unwrap_or_default();
        let name = head
            .lines()
            .find(|l| l.to_ascii_lowercase().starts_with("content-disposition:"))
            .and_then(|l| {
                l.split(';').map(str::trim).find_map(|p| {
                    p.strip_prefix("name=")
                        .map(|v| v.trim_matches('"').to_owned())
                })
            })
            .unwrap_or_default();
        let limit = if name == "torrent" {
            MAX_UPLOAD_BYTES
        } else {
            MAX_FORM_FIELD_BYTES
        };
        if body.len() > limit {
            return Err((
                413,
                format!("the form's {name} is larger than {limit} bytes"),
            ));
        }
        let v = String::from_utf8_lossy(body).trim().to_owned();
        match name.as_str() {
            "torrent" if !body.is_empty() => out.torrent = body.to_vec(),
            "magnet" => out.magnet = v,
            "url" => out.url = v,
            "category" => out.category = v,
            "paused" => out.paused = flag("paused", &v).map_err(bad)?,
            "sequential" => out.sequential = flag("sequential", &v).map_err(bad)?,
            _ => {}
        }
        at = body_end.saturating_add(2);
    }
    Ok(out)
}

/// Adds what `b` asks for, on the pool.
fn add_now(api: ClientApi, ctx: &mut Context, b: AddBody) {
    if b.sequential {
        return ctx.respond(api_error(&not_supported(
            "a sequential download is not supported yet: it needs akio's A287",
        )));
    }
    if b.priorities {
        return ctx.respond(api_error(&not_supported(
            "file priorities are not supported yet: they need akio's A287",
        )));
    }
    let given = [
        !b.magnet.is_empty(),
        !b.url.is_empty(),
        !b.torrent.is_empty(),
    ]
    .iter()
    .filter(|s| **s)
    .count();
    if given != 1 {
        return ctx.respond(fail(400, "give one of magnet, url and torrent"));
    }
    let req = AddRequest {
        paused: b.paused,
        peers: b.peers,
        category: b.category,
        limits: b.limits.unwrap_or_default(),
    };
    on_pool(ctx, move || {
        let added = if !b.url.is_empty() {
            api.add_url(&b.url, req)
        } else if !b.magnet.is_empty() {
            api.add(Source::Magnet(b.magnet), req)
        } else {
            api.add(Source::Torrent(b.torrent), req)
        };
        match added {
            Ok(s) => {
                let mut r = status_answer(201, &s, None);
                r.headers
                    .set("Location", format!("/bittorrent/torrents/{}", s.id));
                r
            }
            Err(e) => api_error(&e),
        }
    });
}

/// `POST /bittorrent/torrents/{id}`: an action.
fn action(api: ClientApi, ctx: &mut Context, id: String) {
    read_json(ctx, MAX_PATCH_BYTES, move |ctx, v| {
        let a = match v.get("action") {
            Some(Value::String(a)) => a.clone(),
            None | Some(Value::Null) => String::new(),
            Some(_) => return ctx.respond(fail(400, "the body: action is a string")),
        };
        if !matches!(a.as_str(), "pause" | "resume" | "recheck") {
            return ctx.respond(fail(
                400,
                &format!("the action is pause, resume or recheck, not {a:?}"),
            ));
        }
        on_pool(ctx, move || {
            let r = match a.as_str() {
                "pause" => api.set_paused(&id, true),
                "resume" => api.set_paused(&id, false),
                _ => api.recheck(&id),
            };
            match r.and_then(|_| api.get(&id)) {
                Ok(s) => status_answer(200, &s, None),
                Err(e) => api_error(&e),
            }
        });
    });
}

/// `PATCH /bittorrent/torrents/{id}`: the changes made in fit's order, each saved; the first
/// that fails is answered, those before it made.
fn patch(api: ClientApi, ctx: &mut Context, id: String) {
    read_json(ctx, MAX_PATCH_BYTES, move |ctx, v| {
        let bool_of = |k: &str| -> Result<Option<bool>, String> {
            match v.get(k) {
                None | Some(Value::Null) => Ok(None),
                Some(Value::Bool(b)) => Ok(Some(*b)),
                Some(_) => Err(format!("the body: {k} is true or false")),
            }
        };
        let parsed = (|| -> Result<_, ApiError> {
            let bad = ApiError::BadRequest;
            let paused = bool_of("paused").map_err(bad)?;
            if bool_of("sequential").map_err(bad)? == Some(true) {
                return Err(not_supported(
                    "a sequential download is not supported yet: it needs akio's A287",
                ));
            }
            let category = match v.get("category") {
                None | Some(Value::Null) => None,
                Some(Value::String(c)) => Some(c.clone()),
                Some(_) => return Err(bad("the body: category is a string".into())),
            };
            // The limits are read first: a bad one changes nothing.
            let limits = match v.get("limits") {
                None => None,
                Some(Value::Null) => Some(BitTorrentTorrentLimits::default()),
                Some(l) => Some(limits_of(l)?),
            };
            let pos = match v.get("queue_position") {
                None | Some(Value::Null) => None,
                Some(Value::Number(n)) => Some(
                    n.parse::<usize>()
                        .map_err(|_| bad(format!("a queue position is 0 or more, not {n}")))?,
                ),
                Some(_) => return Err(bad("the body: queue_position is a number".into())),
            };
            match v.get("priorities") {
                None | Some(Value::Null) => {}
                Some(Value::Array(a)) if a.is_empty() => {}
                Some(_) => {
                    return Err(not_supported(
                        "file priorities are not supported yet: they need akio's A287",
                    ));
                }
            }
            Ok((paused, category, limits, pos))
        })();
        let (paused, category, limits, pos) = match parsed {
            Ok(p) => p,
            Err(e) => return ctx.respond(api_error(&e)),
        };
        on_pool(ctx, move || {
            let steps = || -> Result<(), ApiError> {
                if let Some(p) = paused {
                    api.set_paused(&id, p)?;
                }
                if let Some(c) = &category {
                    api.set_category(&id, c)?;
                }
                if let Some(l) = limits {
                    api.set_torrent_limits(&id, l)?;
                }
                if let Some(p) = pos {
                    api.set_queue_position(&id, p)?;
                }
                Ok(())
            };
            match steps().and_then(|()| api.get(&id)) {
                Ok(s) => status_answer(200, &s, None),
                Err(e) => api_error(&e),
            }
        });
    });
}

/// `GET /bittorrent/events`: the client's events as server-sent events, after `Last-Event-ID`
/// (or `?after=`) when given.
fn events(api: &ClientApi, ctx: &mut Context) {
    let last = ctx
        .header("Last-Event-ID")
        .map(str::to_owned)
        .or_else(|| query(ctx, "after"))
        .unwrap_or_default();
    let after = if last.is_empty() {
        0
    } else {
        match last.trim().parse::<u64>() {
            Ok(n) => n,
            Err(_) => {
                return ctx.respond(fail(
                    400,
                    "Last-Event-ID, or after, is an event's ID: a number",
                ));
            }
        }
    };
    let mut h = Headers::new();
    h.set("Content-Type", "text/event-stream");
    h.set("Cache-Control", "no-cache");
    h.set("X-Accel-Buffering", "no");
    if ctx.method() == &Method::Head {
        if !api.events_open() {
            return ctx.respond(api_error(&ApiError::NotRunning));
        }
        let mut r = Response::new(200);
        r.headers = h;
        return ctx.respond(r);
    }
    let mut started = false;
    let r = api.subscribe(after, || {
        started = true;
        ctx.stream_response(200, h)
    });
    if let Err(e) = r {
        if !started {
            ctx.respond(api_error(&e));
        }
    }
}

/// The tracker's endpoints, answered at once from its statistics (no lock a request waits for).
fn tracker(apis: &Apis, ctx: &Context, rest: &str, params: &[String]) -> Response {
    tracker_answer(apis, &ctx.req().target, rest, params)
}

fn tracker_answer(apis: &Apis, target: &str, rest: &str, params: &[String]) -> Response {
    let Some(pages) = api_in(apis, crate::tracker::ID)
        .and_then(|a| a.downcast::<crate::tracker::pages::Pages>().ok())
    else {
        return fail(404, "the bittorrent.tracker role does not run here");
    };
    match rest {
        "stats" => pages.answer(true, target, "/api/stats"),
        "torrents" => pages.answer(true, target, "/api/torrents"),
        "torrents/{hash}" => {
            let h = params.first().map_or("", String::as_str);
            pages.answer(true, target, &format!("/api/torrents/{h}"))
        }
        _ => pages.passkey(target, true),
    }
}

/// Streams in the request's body, at most `max` bytes, and hands it to `done`; 413 past it, as
/// fit's `readBounded`. `what` names the body in the errors.
fn read_body(
    ctx: &mut Context,
    max: usize,
    what: &'static str,
    done: impl FnOnce(&mut Context, Vec<u8>) + Send + 'static,
) {
    let too_large = move || {
        let mut r = fail(413, &format!("{what} is larger than {max} bytes"));
        r.headers.set("Connection", "close");
        r
    };
    let declared = ctx
        .header("Content-Length")
        .and_then(|l| l.trim().parse::<u64>().ok());
    if declared.is_some_and(|n| n > u64::try_from(max).unwrap_or(u64::MAX)) {
        return ctx.respond(too_large());
    }
    if declared == Some(0) || (declared.is_none() && ctx.header("Transfer-Encoding").is_none()) {
        return done(ctx, Vec::new());
    }
    let mut data = Vec::with_capacity(
        declared
            .and_then(|n| usize::try_from(n).ok())
            .unwrap_or(0)
            .min(max),
    );
    let mut over = false;
    let mut done = Some(done);
    ctx.on_body(move |ctx, ev| {
        if over {
            return;
        }
        match ev {
            BodyEvent::Chunk(chunk) => {
                if data.len().saturating_add(chunk.len()) > max {
                    over = true;
                    data = Vec::new();
                    ctx.respond(too_large());
                } else {
                    data.extend_from_slice(chunk);
                }
            }
            BodyEvent::End(_) => {
                if let Some(done) = done.take() {
                    done(ctx, std::mem::take(&mut data));
                }
            }
            BodyEvent::TooLarge => over = true,
        }
    });
}

/// Reads a JSON body (`application/json` or `application/merge-patch+json`), an empty one as
/// `{}`, and hands it to `done`, as fit's `readBTBody`.
fn read_json(
    ctx: &mut Context,
    max: usize,
    done: impl FnOnce(&mut Context, Value) + Send + 'static,
) {
    let empty = ctx
        .header("Content-Length")
        .is_some_and(|l| l.trim() == "0")
        || (ctx.header("Content-Length").is_none() && ctx.header("Transfer-Encoding").is_none());
    if !empty {
        let (mt, _) = media_type(ctx);
        if mt != "application/json" && mt != "application/merge-patch+json" {
            return ctx.respond(fail(415, "the body must be sent as application/json"));
        }
    }
    read_body(ctx, max, "the body", move |ctx, data| {
        if data.is_empty() {
            return done(ctx, Value::Object(Vec::new()));
        }
        match parse_body(&data) {
            Ok(v @ Value::Object(_)) => done(ctx, v),
            Ok(_) => ctx.respond(fail(400, "the body: an object")),
            Err(e) => ctx.respond(fail(400, &e)),
        }
    });
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::indexing_slicing)]

    use std::collections::BTreeSet;
    use std::sync::Arc;

    use akit_roles::host::Manager;

    use super::ROUTES;
    use crate::client::tests::{admin_server, http_req};

    /// The endpoints below `/bittorrent/` that tests/compat/v1 holds (its baseline and the
    /// additions marked since), as akit-admin's `v1_surface_is_frozen` reads them.
    fn frozen() -> BTreeSet<(String, String)> {
        let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tests/compat/v1");
        let mut out = BTreeSet::new();
        for (file, skip) in [("admin-api.txt", 0), ("additions.txt", 2)] {
            for l in std::fs::read_to_string(dir.join(file)).unwrap().lines() {
                if l.starts_with('#') || l.is_empty() {
                    continue;
                }
                let f: Vec<&str> = l.split('\t').collect();
                if skip == 2 && (f[0] != "admin" || f[1] != "added") {
                    continue;
                }
                let f = &f[skip..];
                if f[0] == "endpoint" && f[2].starts_with("/bittorrent/") {
                    out.insert((f[1].to_owned(), f[2].to_owned()));
                }
            }
        }
        out
    }

    /// I157, the v1 freeze for the endpoints akit-bittorrent serves through akit-admin: every
    /// endpoint frozen is served, with no role running (404 saying so, not "no such endpoint"),
    /// and no method past them is.
    #[test]
    fn admin_surface_is_frozen() {
        let engine = akio::Engine::builder().build().unwrap();
        engine.start().unwrap();
        let roles = Manager::new(&[], Arc::new(engine), None);
        let server = admin_server(&roles);
        let mut seen = BTreeSet::new();
        for (pattern, _) in ROUTES {
            let path = pattern
                .replace("{id}", &"ab".repeat(20))
                .replace("{hash}", &"cd".repeat(20))
                .replace("{name}", "share");
            for method in ["GET", "HEAD", "POST", "PUT", "PATCH", "DELETE"] {
                let (status, _, body) = http_req(server.addr(), method, &path, &[], b"");
                let body = String::from_utf8_lossy(&body);
                let served = match method {
                    "HEAD" => status != 405,
                    _ => {
                        !(status == 405
                            || status == 404 && body.contains("there is no such endpoint"))
                    }
                };
                if served {
                    seen.insert((method.to_owned(), (*pattern).to_owned()));
                }
            }
        }
        let want = frozen();
        assert_eq!(
            seen, want,
            "the BitTorrent endpoints differ from tests/compat/v1 (mark an addition in \
             additions.txt)"
        );
        server.shutdown();
    }
}
