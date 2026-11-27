//! The Admin API's endpoints of the media libraries (I141, fit's `admin/medialib.go`):
//!
//! ```text
//! GET  /media/library                    every library, with its scans' status
//! GET  /media/library/{name}             one library: its status, problems and items, paged
//!                                        (offset, limit) and filtered (q, codec, error,
//!                                        min_height, max_height)
//! GET  /media/library/{name}/items/{id}  one item, whole
//! POST /media/library/{name}/rescan      scan the library again, at once
//! ```
//!
//! They read the libraries' state and answer on the API's thread; none touches the disk.

use std::sync::Arc;

use akio_http::{Context, Method, Response};
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
fn summary(l: &Library) -> Vec<(&'static str, Value)> {
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
        ("uploads", n(0)),
    ]);
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
    let read = matches!(method, Method::Get | Method::Head);
    let query = ctx.req().query().unwrap_or("").to_owned();
    let resp = route(key, rest, read, method == Method::Post, &query);
    ctx.respond(resp);
    true
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
