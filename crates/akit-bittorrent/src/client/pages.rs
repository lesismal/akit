//! The client's console (I152): its torrents, read-only, at `GET /api/torrents` and
//! `/api/torrents/<id>`, as fit's I197 served them until its Admin API and pages (I157, I158).
//! A call reads the role's state under its lock, which file I/O of the API's may hold: the answer
//! is made on akio's logical pool, never on the console's I/O thread.

use std::fmt::Write as _;

use akio_http::Context;
use akit_config::bittorrent::BitTorrentTorrentLimits;
use akit_config::value::write_string;
use akit_roles::pages::{Module, fail, json, no_endpoint};

use super::inspect::FileStatus;
use super::{ApiError, ClientApi, Status};

/// The role's module of the pages.
#[derive(Debug)]
pub(crate) struct Pages {
    api: ClientApi,
}

impl Pages {
    pub(crate) fn new(api: ClientApi) -> Pages {
        Pages { api }
    }

    /// The answer to a GET of `path`, made on the caller's thread.
    pub(crate) fn answer(&self, path: &str) -> akio_http::Response {
        let err = |e: ApiError| match e {
            ApiError::NotFound => fail(404, &e.to_string()),
            ApiError::NotRunning => fail(503, &e.to_string()),
            e => fail(400, &e.to_string()),
        };
        match path {
            "/api/torrents" => match self.api.list() {
                Ok(list) => {
                    let mut b = String::from("{\"torrents\":[");
                    for (i, s) in list.iter().enumerate() {
                        if i > 0 {
                            b.push(',');
                        }
                        status_json(&mut b, s);
                    }
                    let _ = writeln!(b, "],\"total\":{}}}", list.len());
                    json(200, b)
                }
                Err(e) => err(e),
            },
            p => match p.strip_prefix("/api/torrents/") {
                Some(id) => match self.api.get(id) {
                    Ok(s) => {
                        let mut b = String::new();
                        status_json(&mut b, &s);
                        b.push('\n');
                        json(200, b)
                    }
                    Err(e) => err(e),
                },
                None => no_endpoint(),
            },
        }
    }
}

/// A torrent's limits as fit's JSON writes them: `seed_time` in nanoseconds.
pub(crate) fn limits_json(b: &mut String, l: &BitTorrentTorrentLimits) {
    let _ = write!(
        b,
        "{{\"upload_rate\":{},\"download_rate\":{},\"max_conns\":0,\"upload_slots\":0,\"seed_ratio\":{},\"seed_time\":{}}}",
        l.upload_rate,
        l.download_rate,
        l.seed_ratio,
        l.seed_time.as_nanos()
    );
}

/// A status as fit's JSON writes it, with `files` when given (I157: `GET
/// /bittorrent/torrents/<id>`).
pub(crate) fn status_json(b: &mut String, s: &Status) {
    status_json_with(b, s, None);
}

/// [`status_json`], with the torrent's files.
pub(crate) fn status_json_with(b: &mut String, s: &Status, files: Option<&[FileStatus]>) {
    let str_field = |b: &mut String, k: &str, v: &str| {
        b.push('"');
        b.push_str(k);
        b.push_str("\":");
        write_string(b, v);
        b.push(',');
    };
    b.push('{');
    str_field(b, "id", &s.id);
    str_field(b, "name", &s.name);
    str_field(b, "info_hash", &s.id);
    if !s.info_hash_v2.is_empty() {
        str_field(b, "info_hash_v2", &s.info_hash_v2);
    }
    if !s.magnet.is_empty() {
        str_field(b, "magnet", &s.magnet);
    }
    str_field(b, "dir", &s.dir);
    str_field(b, "state", &s.state);
    if !s.error.is_empty() {
        str_field(b, "error", &s.error);
    }
    let _ = write!(b, "\"paused\":{},\"sequential\":false,", s.paused);
    str_field(b, "added", &s.added);
    if !s.category.is_empty() {
        str_field(b, "category", &s.category);
    }
    if let Some(l) = &s.limits {
        b.push_str("\"limits\":");
        limits_json(b, l);
        b.push(',');
    }
    if let Some(l) = &s.effective_limits {
        b.push_str("\"effective_limits\":");
        limits_json(b, l);
        b.push(',');
    }
    let _ = write!(b, "\"queue_position\":{},", s.queue_position);
    if !s.held.is_empty() {
        str_field(b, "held", &s.held);
    }
    let _ = write!(
        b,
        "\"complete\":{},\"size\":{},\"wanted\":{},\"completed\":{},\"progress\":{},\"pieces\":{},\"pieces_have\":{},\"downloaded\":{},\"uploaded\":{},\"download_rate\":{},\"upload_rate\":{},\"peers\":{},\"seeders\":{}",
        s.complete,
        s.size,
        s.size,
        s.completed,
        s.progress,
        s.pieces,
        s.pieces_have,
        s.downloaded,
        s.uploaded,
        s.download_rate,
        s.upload_rate,
        s.peers,
        s.seeders
    );
    if let Some(fs) = files.filter(|f| !f.is_empty()) {
        b.push_str(",\"files\":");
        files_json(b, fs);
    }
    b.push('}');
}

/// A torrent's files as fit's JSON writes them.
pub(crate) fn files_json(b: &mut String, fs: &[FileStatus]) {
    b.push('[');
    for (i, f) in fs.iter().enumerate() {
        if i > 0 {
            b.push(',');
        }
        let _ = write!(b, "{{\"index\":{},\"path\":", f.index);
        write_string(b, &f.path);
        let _ = write!(b, ",\"length\":{}", f.length);
        if let Some(c) = f.completed {
            let _ = write!(b, ",\"completed\":{c}");
        }
        let _ = write!(b, ",\"priority\":{}}}", f.priority);
    }
    b.push(']');
}

impl Module for Pages {
    fn id(&self) -> &'static str {
        super::ID
    }

    fn serve(&self, ctx: &mut Context, _user: &str, path: &str) {
        if ctx.req().method != akio_http::Method::Get {
            ctx.respond(no_endpoint());
            return;
        }
        if !path.starts_with("/api/torrents") {
            ctx.respond(no_endpoint());
            return;
        }
        // The role's lock may be held by an API call writing the state: answered on akio's
        // logical pool, the request detached meanwhile (503 when the pool is full).
        let api = self.api.clone();
        let path = path.to_owned();
        ctx.detach_to_pool(move |d| d.respond(Pages { api }.answer(&path)));
    }
}
