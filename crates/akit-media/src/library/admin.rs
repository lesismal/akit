//! The Admin API's endpoints of the media libraries (I141, fit's `admin/medialib.go`):
//!
//! ```text
//! GET  /media/library                    every library, with its scans' status
//! GET  /media/library/{name}             one library: its status, problems and items, paged
//!                                        (offset, limit) and filtered (q, codec, error,
//!                                        min_height, max_height)
//! GET  /media/library/{name}/items/{id}  one item, whole
//! POST /media/library/{name}/rescan      scan the library again, at once
//!
//! GET    /media/library/{name}/uploads       the uploads under way (I149)
//! POST   /media/library/{name}/uploads       begin one: {"path", "size"}, path under upload_dir
//! GET    /media/library/{name}/uploads/{id}  one, its offset also in Upload-Offset (HEAD too)
//! PATCH  /media/library/{name}/uploads/{id}  write the body at Upload-Offset
//! DELETE /media/library/{name}/uploads/{id}  give it up
//! ```
//!
//! They read the libraries' state and answer on the API's thread; none touches the disk there.
//! The uploads' (fit's `admin/mediafiles.go`) make, write and remove files on the library's
//! pool, the request detached meanwhile: a PATCH's body streams in (the Admin API's `streamed`
//! prefixes), its reads held while what was gathered of it is written, so that an upload of any
//! size takes a bounded buffer.

use std::sync::Arc;

use akio_http::{BodyEvent, Context, Method, Response};
use akit_config::value::Value;

use super::catalog::{Item, Problem, time};
use super::{Library, Status, lookup};

/// The prefix of the endpoints.
pub const PREFIX: &str = "/media/library";

/// The bounds of a page of items.
const DEFAULT_PAGE: usize = 100;
const MAX_PAGE: usize = 1000;

fn json(status: u16, v: &Value) -> Response {
    let mut b = super::catalog::compact(v);
    b.push('\n');
    Response::with_body(status, "application/json", b.into_bytes())
}

fn error(status: u16, msg: &str) -> Response {
    json(
        status,
        &Value::Object(vec![("error".to_owned(), Value::String(msg.to_owned()))]),
    )
}

fn n(v: u64) -> Value {
    Value::Number(v.to_string())
}

fn obj(m: Vec<(&str, Value)>) -> Value {
    Value::Object(m.into_iter().map(|(k, v)| (k.to_owned(), v)).collect())
}

fn status_value(st: &Status) -> Value {
    let mut m = vec![
        ("scanning", Value::Bool(st.scanning)),
        ("pending", Value::Bool(st.pending)),
        ("scans", n(st.scans)),
    ];
    if let Some(t) = st.started {
        m.push(("started", time(t)));
    }
    if let Some(t) = st.ended {
        m.push(("ended", time(t)));
    }
    m.extend([
        ("files", n(st.files as u64)),
        ("probe", n(st.probe as u64)),
        ("probed", n(st.probed as u64)),
        ("reused", n(st.reused as u64)),
    ]);
    obj(m)
}

/// A library as `GET /media/library` lists it.
pub(crate) fn summary(l: &Library) -> Vec<(&'static str, Value)> {
    let cat = l.catalog();
    let errs = cat.items.iter().filter(|it| !it.error.is_empty()).count();
    let pool = l.pool().stats();
    let tables = l.tables();
    let mut m = vec![
        ("name", Value::String(l.name().to_owned())),
        (
            "roots",
            Value::Array(l.roots().into_iter().map(Value::String).collect()),
        ),
        ("status", status_value(&l.status())),
    ];
    if let Some(t) = cat.scanned {
        m.push(("scanned", time(t)));
    }
    m.extend([
        ("items", n(cat.items.len() as u64)),
        ("errors", n(errs as u64)),
        (
            "problems",
            n(cat.problems.len().saturating_add(cat.more_problems) as u64),
        ),
    ]);
    if !cat.truncated.is_empty() {
        m.push(("truncated", Value::String(cat.truncated.clone())));
    }
    m.extend([
        ("bytes", n(cat.bytes)),
        (
            "pool",
            obj(vec![
                ("workers", n(pool.workers as u64)),
                ("queue", n(pool.queue as u64)),
                ("queued", n(pool.queued as u64)),
                ("busy", n(pool.busy as u64)),
                ("refused", n(pool.refused)),
                ("expired", n(pool.expired)),
            ]),
        ),
        (
            "tables",
            obj(vec![
                ("tables", n(tables.tables as u64)),
                ("bytes", n(tables.bytes)),
                ("max", n(tables.max)),
                ("hits", n(tables.hits)),
                ("misses", n(tables.misses)),
            ]),
        ),
    ]);
    if let Some(up) = l.upload_dir() {
        m.push(("upload", up.to_value()));
    }
    m.extend([("uploads", n(l.uploads().len() as u64))]);
    m
}

/// An item as a page of `GET /media/library/{name}` lists it.
struct ItemSummary {
    value: Value,
    codecs: Vec<&'static str>,
    height: u32,
    problems: bool,
}

fn summarize(it: &Item) -> ItemSummary {
    let mut codecs: Vec<&'static str> = Vec::new();
    let (mut width, mut height) = (0u32, 0u32);
    let mut problems = !it.error.is_empty();
    for r in it.renditions.iter().chain(&it.audio) {
        if u64::from(r.width).saturating_mul(u64::from(r.height))
            > u64::from(width).saturating_mul(u64::from(height))
        {
            (width, height) = (r.width, r.height);
        }
        problems = problems || !r.error.is_empty();
        for t in &r.tracks {
            if t.codec != "unknown" && !codecs.contains(&t.codec) {
                codecs.push(t.codec);
            }
        }
    }
    problems = problems || it.subtitles.iter().any(|s| !s.error.is_empty());
    let mut m = vec![
        ("id", Value::String(it.id.clone())),
        ("title", Value::String(it.title.clone())),
        ("root", Value::String(it.root.clone())),
        ("path", Value::String(it.path.clone())),
        ("duration", super::catalog::secs(it.duration)),
        ("size", n(it.size)),
        ("modified", time(it.modified)),
    ];
    if width != 0 {
        m.push(("width", n(u64::from(width))));
    }
    if height != 0 {
        m.push(("height", n(u64::from(height))));
    }
    m.extend([
        (
            "codecs",
            Value::Array(
                codecs
                    .iter()
                    .map(|c| Value::String((*c).to_owned()))
                    .collect(),
            ),
        ),
        ("renditions", n(it.renditions.len() as u64)),
        ("audio", n(it.audio.len() as u64)),
        ("subtitles", n(it.subtitles.len() as u64)),
        ("poster", Value::Bool(it.poster.is_some())),
    ]);
    if !it.error.is_empty() {
        m.push(("error", Value::String(it.error.clone())));
    }
    m.push(("problems", Value::Bool(problems)));
    ItemSummary {
        value: obj(m),
        codecs,
        height,
        problems,
    }
}

/// Answers a request of the endpoints, `path` its path; false for a path that is not theirs.
pub fn serve(key: usize, ctx: &mut Context) -> bool {
    let path = ctx.path().to_owned();
    let Some(rest) = path.strip_prefix(PREFIX) else {
        return false;
    };
    if !(rest.is_empty() || rest.starts_with('/')) {
        return false;
    }
    let method = ctx.method().clone();
    if let Some((name, id)) = uploads_path(rest) {
        uploads(key, ctx, &name, id.as_deref());
        return true;
    }
    let read = matches!(method, Method::Get | Method::Head);
    let query = ctx.req().query().unwrap_or("").to_owned();
    let resp = route(key, rest, read, method == Method::Post, &query);
    ctx.respond(resp);
    true
}

/// `/{name}/uploads` and `/{name}/uploads/{id}`: the library's name, and the upload's ID.
fn uploads_path(rest: &str) -> Option<(String, Option<String>)> {
    let rest = rest.strip_prefix('/')?;
    let (name, more) = rest.split_once('/')?;
    let more = more.strip_prefix("uploads")?;
    match more {
        "" | "/" => Some((name.to_owned(), None)),
        m => {
            let id = m.strip_prefix('/')?;
            (!id.is_empty() && !id.contains('/')).then(|| (name.to_owned(), Some(id.to_owned())))
        }
    }
}

/// The largest JSON body of the files' endpoints.
const MAX_JSON_BODY: u64 = 64 << 10;

/// An upload's offset and length as tus's headers, and `no-store`.
fn upload_headers(r: Response, info: &super::UploadInfo) -> Response {
    r.header("Upload-Offset", info.offset.to_string())
        .header("Upload-Length", info.size.to_string())
        .header("Cache-Control", "no-store")
}

/// A change's error, as fit's `writeFileError` answers it.
fn file_error(e: &super::FileError) -> Response {
    let r = error(e.status(), &e.to_string());
    if e.status() == 503 || *e == super::FileError::UploadBusy {
        r.header("Retry-After", "1")
    } else {
        r
    }
}

/// Reads the request's JSON body, streamed or not, within [`MAX_JSON_BODY`], and calls `f` with
/// its members, or the error to answer; an empty body is no members.
pub(crate) fn read_json(
    ctx: &mut Context,
    f: impl FnOnce(&mut Context, Result<Vec<(String, Value)>, Box<Response>>) + Send + 'static,
) {
    fn parse(data: &[u8]) -> Result<Vec<(String, Value)>, Box<Response>> {
        if data.iter().all(u8::is_ascii_whitespace) {
            return Ok(Vec::new());
        }
        let text =
            std::str::from_utf8(data).map_err(|_| Box::new(error(400, "the body: not UTF-8")))?;
        match akit_config::value::parse(text) {
            Ok(Value::Object(m)) => Ok(m),
            Ok(_) => Err(Box::new(error(400, "the body: not a JSON object"))),
            Err(e) => Err(Box::new(error(400, &format!("the body: {e}")))),
        }
    }
    if ctx.body_reads().is_none() {
        let data = ctx.req().body.clone();
        return f(ctx, parse(&data));
    }
    if !ctx.limit_body(MAX_JSON_BODY) {
        return;
    }
    let mut data = Vec::new();
    let mut f = Some(f);
    ctx.on_body(move |ctx, ev| match ev {
        BodyEvent::Chunk(c) => data.extend_from_slice(c),
        BodyEvent::End(_) => {
            if let Some(f) = f.take() {
                f(ctx, parse(&data));
            }
        }
        BodyEvent::TooLarge => {}
    });
}

/// A JSON member: a string, or "" when it is not there.
fn member_str(m: &[(String, Value)], k: &str) -> Result<String, Box<Response>> {
    match m.iter().find(|(x, _)| x == k).map(|(_, v)| v) {
        None | Some(Value::Null) => Ok(String::new()),
        Some(Value::String(s)) => Ok(s.clone()),
        Some(_) => Err(Box::new(error(
            400,
            &format!("the body: {k} is not a string"),
        ))),
    }
}

/// Who asks: the console's user, or `admin-api`.
fn file_user(ctx: &Context) -> String {
    ctx.header(crate::workbench::CONSOLE_USER)
        .filter(|u| !u.is_empty())
        .unwrap_or("admin-api")
        .to_owned()
}

/// The uploads' endpoints.
fn uploads(key: usize, ctx: &mut Context, name: &str, id: Option<&str>) {
    let Some(l) = lookup(key, name) else {
        return ctx.respond(error(
            404,
            &format!("no media library named {name:?} runs here"),
        ));
    };
    let method = ctx.method().clone();
    match (id, &method) {
        (None, Method::Get | Method::Head) => {
            let list = l
                .uploads()
                .iter()
                .map(super::UploadInfo::to_value)
                .collect();
            ctx.respond(json(200, &obj(vec![("uploads", Value::Array(list))])));
        }
        (None, Method::Post) => {
            let user = file_user(ctx);
            let name = name.to_owned();
            read_json(ctx, move |ctx, m| {
                let m = match m {
                    Ok(m) => m,
                    Err(r) => return ctx.respond(*r),
                };
                let path = match member_str(&m, "path") {
                    Ok(p) => p,
                    Err(r) => return ctx.respond(*r),
                };
                let size = match m.iter().find(|(k, _)| k == "size").map(|(_, v)| v) {
                    None | Some(Value::Null) => 0,
                    Some(Value::Number(n)) => match n.parse::<i64>() {
                        Ok(n) => n,
                        Err(_) => {
                            return ctx.respond(error(400, "the body: size is not a whole number"));
                        }
                    },
                    Some(_) => return ctx.respond(error(400, "the body: size is not a number")),
                };
                let Some(det) = ctx.detach() else { return };
                let spec = super::UploadSpec { path, size, user };
                l.create_upload(&spec, move |r| {
                    det.respond(match r {
                        Ok(info) => upload_headers(json(201, &info.to_value()), &info)
                            .header("Location", format!("{PREFIX}/{name}/uploads/{}", info.id)),
                        Err(e) => file_error(&e),
                    });
                });
            });
        }
        (None, _) => ctx.respond(not_allowed("GET, HEAD, POST")),
        (Some(id), Method::Get | Method::Head) => match l.upload(id) {
            Some(info) => ctx.respond(upload_headers(json(200, &info.to_value()), &info)),
            None => ctx.respond(file_error(&super::FileError::NoUpload)),
        },
        (Some(id), Method::Delete) => {
            let Some(det) = ctx.detach() else { return };
            l.cancel_upload(id, move |r| {
                det.respond(match r {
                    Ok(()) => json(200, &obj(vec![("cancelled", Value::Bool(true))])),
                    Err(e) => file_error(&e),
                });
            });
        }
        (Some(id), Method::Patch) => patch(&l, ctx, id),
        (Some(_), _) => ctx.respond(not_allowed("GET, HEAD, PATCH, DELETE")),
    }
}

/// Ends a write whose body went before it ended: the upload goes on from what was written.
struct BodyGuard {
    chunk: Arc<super::Chunk>,
    ended: bool,
}

impl Drop for BodyGuard {
    fn drop(&mut self) {
        if !self.ended {
            self.chunk
                .write(&[], false, Some("the body ended before its end".to_owned()));
        }
    }
}

/// `PATCH /media/library/{name}/uploads/{id}`: the body written at `Upload-Offset`, read as it
/// arrives, each piece written on the library's pool, the body's reads held meanwhile.
fn patch(l: &Arc<Library>, ctx: &mut Context, id: &str) {
    let off = ctx
        .header("Upload-Offset")
        .and_then(|v| v.trim().parse::<u64>().ok());
    let Some(off) = off else {
        return ctx.respond(error(
            400,
            "the request needs Upload-Offset, where the body goes in the file",
        ));
    };
    let chunk = match l.write_upload(id, off) {
        Ok(c) => c,
        Err(e) => {
            let r = file_error(&e);
            return ctx.respond(match l.upload(id) {
                Some(info) => upload_headers(r, &info),
                None => r,
            });
        }
    };
    let remaining = chunk.remaining();
    let declared = ctx
        .header("Content-Length")
        .and_then(|v| v.trim().parse::<u64>().ok());
    if declared.is_some_and(|n| n > remaining) {
        chunk.abort(super::FileError::TooLarge(format!(
            "the body is longer than the upload's {} bytes",
            chunk.info().size
        )));
        return ctx.respond(file_error(&super::FileError::TooLarge(
            "the body is longer than what the upload still takes".to_owned(),
        )));
    }
    let reads = ctx.body_reads();
    let Some(r) = reads.clone() else {
        // A body read whole, as a test's or a server's that buffers it.
        let body = ctx.req().body.clone();
        let Some(det) = ctx.detach() else { return };
        chunk.set_hooks(hooks(det, None));
        return chunk.write(&body, true, None);
    };
    let mut guard = BodyGuard {
        chunk: Arc::clone(&chunk),
        ended: false,
    };
    ctx.on_body(move |_, ev| match ev {
        BodyEvent::Chunk(d) => guard.chunk.write(d, false, None),
        BodyEvent::End(_) => {
            guard.ended = true;
            guard.chunk.write(&[], true, None);
        }
        BodyEvent::TooLarge => {
            guard.ended = true;
            guard.chunk.abort(super::FileError::TooLarge(
                "the body is longer than what the upload still takes".to_owned(),
            ));
        }
    });
    if !ctx.limit_body(remaining) {
        return;
    }
    let Some(det) = ctx.detach() else { return };
    chunk.set_hooks(hooks(det, Some(r)));
}

/// What a write tells its request: its reads held while a piece is written, its answer at the
/// end.
fn hooks(det: akio_http::Detached, reads: Option<akio_http::BodyReads>) -> super::ChunkHooks {
    super::ChunkHooks {
        hold: Box::new(move |h| {
            if let Some(r) = &reads {
                r.hold(h);
            }
        }),
        done: Box::new(move |info, r| {
            det.respond(match r {
                Ok(()) => upload_headers(json(200, &info.to_value()), &info),
                Err(e) => {
                    let resp = file_error(&e);
                    if info.id.is_empty() || matches!(e, super::FileError::TooLarge(_)) {
                        resp
                    } else {
                        upload_headers(resp, &info)
                    }
                }
            });
        }),
    }
}

fn not_allowed(allow: &str) -> Response {
    error(405, "method not allowed").header("Allow", allow)
}

pub(crate) fn route(key: usize, rest: &str, read: bool, post: bool, query: &str) -> Response {
    if rest.is_empty() || rest == "/" {
        if !read {
            return not_allowed("GET, HEAD");
        }
        let libs = super::libraries(key);
        let list = libs.iter().map(|l| obj(summary(l))).collect();
        return json(200, &obj(vec![("libraries", Value::Array(list))]));
    }
    let rest = rest.trim_start_matches('/');
    let (name, more) = rest.split_once('/').unwrap_or((rest, ""));
    let library = |name: &str| -> Result<Arc<Library>, Box<Response>> {
        lookup(key, name).ok_or_else(|| {
            Box::new(error(
                404,
                &format!("no media library named {name:?} runs here"),
            ))
        })
    };
    if more.is_empty() {
        if !read {
            return not_allowed("GET, HEAD");
        }
        return match library(name) {
            Ok(l) => page(&l, query),
            Err(r) => *r,
        };
    }
    if more == "rescan" {
        if !post {
            return not_allowed("POST");
        }
        return match library(name) {
            Ok(l) => {
                let queued = l.rescan();
                json(
                    202,
                    &obj(vec![
                        ("queued", Value::Bool(queued)),
                        ("status", status_value(&l.status())),
                    ]),
                )
            }
            Err(r) => *r,
        };
    }
    if let Some(id) = more.strip_prefix("items/") {
        if !read {
            return not_allowed("GET, HEAD");
        }
        return match library(name) {
            Ok(l) => match l.item(id) {
                Some(it) => json(200, &it.to_value()),
                None => error(404, &format!("no item {id:?} in the library")),
            },
            Err(r) => *r,
        };
    }
    error(404, "there is no such endpoint")
}

/// `GET /media/library/{name}`: the library's status and problems, and a page of its items.
fn page(l: &Library, query: &str) -> Response {
    let q = |k: &str| crate::http::query_param(query, k).filter(|v| !v.is_empty());
    let mut offset = 0usize;
    let mut limit = DEFAULT_PAGE;
    for (k, to, max) in [
        ("offset", &mut offset, None),
        ("limit", &mut limit, Some(MAX_PAGE)),
    ] {
        if let Some(v) = q(k) {
            match v.parse::<usize>() {
                Ok(n) if max.is_none_or(|m| (1..=m).contains(&n)) => *to = n,
                _ => return error(400, &format!("{k} is not a number in range")),
            }
        }
    }
    let mut heights = [None::<u32>; 2];
    for (i, k) in ["min_height", "max_height"].into_iter().enumerate() {
        if let Some(v) = q(k) {
            match v.parse::<u32>() {
                Ok(n) => {
                    if let Some(h) = heights.get_mut(i) {
                        *h = Some(n);
                    }
                }
                Err(_) => return error(400, &format!("{k} is not a number of 0 or more")),
            }
        }
    }
    let [min_height, max_height] = heights;
    let text = q("q").map(|t| t.to_lowercase()).unwrap_or_default();
    let codec = q("codec").unwrap_or_default();
    let want_err = match q("error").as_deref() {
        None => None,
        Some("1" | "t" | "T" | "true" | "TRUE" | "True") => Some(true),
        Some("0" | "f" | "F" | "false" | "FALSE" | "False") => Some(false),
        Some(_) => return error(400, "error is true or false"),
    };
    let mut m = summary(l);
    let cat = l.catalog();
    let mut items = Vec::new();
    let mut matched = 0usize;
    for it in &cat.items {
        let is = summarize(it);
        let skip = (!text.is_empty()
            && !it.id.to_lowercase().contains(&text)
            && !it.title.to_lowercase().contains(&text))
            || (!codec.is_empty() && !is.codecs.contains(&codec.as_str()))
            || want_err.is_some_and(|w| is.problems != w)
            || min_height.is_some_and(|h| is.height < h)
            || max_height.is_some_and(|h| is.height > h);
        if skip {
            continue;
        }
        if matched >= offset && items.len() < limit {
            items.push(is.value);
        }
        matched = matched.saturating_add(1);
    }
    m.extend([
        (
            "problem_list",
            Value::Array(cat.problems.iter().map(Problem::to_value).collect()),
        ),
        ("matched", n(matched as u64)),
        ("offset", n(offset as u64)),
        ("limit", n(limit as u64)),
        ("page", Value::Array(items)),
    ]);
    json(200, &obj(m))
}
