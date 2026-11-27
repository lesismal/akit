//! `bt_publish` (I156, fit's `handler/btpublish`): a route's handler serving the publish entries
//! of the `bittorrent.client` role of its engine: `/` or `index.json`, the JSON index of their
//! torrents with their URLs relative to it; `<id>.torrent`, a .torrent file; `<id>.magnet`, a
//! magnet link as text; 503 while no client runs. Its params: `publish`, the entries served by
//! name (every entry when left out; a name the client has not listed with its error).

use std::fmt::Write as _;
use std::sync::{Arc, Mutex, Weak};

use akio_http::{Context, Response};
use akit_config::value::{Value, write_string};
use akit_runtime::upstream::EngineSlot;
use akit_runtime::{Handler, HandlerParams, Provision, ProvisionError, Provisioned};

use super::publish::EntryStatus;
use super::{Shared, lock};

// The client roles' shared state, by engine.
static BOARD: Mutex<Vec<(usize, Weak<Shared>)>> = Mutex::new(Vec::new());

/// Puts the role's shared state on the board for the engine `key`.
pub(super) fn register(key: usize, shared: &Arc<Shared>) {
    let mut b = lock(&BOARD);
    b.retain(|(k, w)| *k != key && w.strong_count() > 0);
    b.push((key, Arc::downgrade(shared)));
}

/// Takes it off.
pub(super) fn unregister(key: usize, shared: &Arc<Shared>) {
    lock(&BOARD).retain(|(k, w)| !(*k == key && w.ptr_eq(&Arc::downgrade(shared))));
}

fn shared_of(key: usize) -> Option<Arc<Shared>> {
    lock(&BOARD)
        .iter()
        .find(|(k, _)| *k == key)
        .and_then(|(_, w)| w.upgrade())
}

struct BtPublish {
    engine: EngineSlot,
    names: Vec<String>,
}

fn answer(status: u16, ty: &str, body: Vec<u8>) -> Response {
    let mut r = Response::new(status);
    r.headers.set("Content-Type", ty);
    r.headers.set("Cache-Control", "no-cache");
    r.body = body;
    r
}

fn entry_json(b: &mut String, e: &EntryStatus) {
    let s = |b: &mut String, k: &str, v: &str| {
        let _ = write!(b, "\"{k}\":");
        write_string(b, v);
    };
    b.push('{');
    s(b, "name", &e.name);
    b.push(',');
    s(b, "dir", &e.dir);
    b.push(',');
    s(b, "mode", &e.mode);
    if !e.web_seed.is_empty() {
        b.push(',');
        s(b, "web_seed", &e.web_seed);
    }
    b.push_str(",\"torrents\":[");
    for (i, t) in e.torrents.iter().enumerate() {
        if i > 0 {
            b.push(',');
        }
        b.push('{');
        s(b, "id", &t.id);
        b.push(',');
        s(b, "path", &t.path);
        b.push(',');
        s(b, "name", &t.name);
        let _ = write!(b, ",\"size\":{},\"files\":{},", t.size, t.files);
        s(b, "info_hash", &t.id);
        b.push(',');
        s(b, "info_hash_v2", &t.info_hash_v2);
        b.push(',');
        s(b, "magnet", &t.magnet);
        b.push(',');
        s(b, "built", &t.built);
        b.push(',');
        s(b, "state", &t.state);
        if !t.error.is_empty() {
            b.push(',');
            s(b, "error", &t.error);
        }
        b.push(',');
        s(b, "torrent_url", &format!("{}.torrent", t.id));
        b.push(',');
        s(b, "magnet_url", &format!("{}.magnet", t.id));
        b.push('}');
    }
    let _ = write!(b, "],\"pending\":{}", e.pending);
    if !e.scanned.is_empty() {
        b.push(',');
        s(b, "scanned", &e.scanned);
    }
    if !e.problems.is_empty() {
        b.push_str(",\"problems\":[");
        for (i, p) in e.problems.iter().enumerate() {
            if i > 0 {
                b.push(',');
            }
            write_string(b, p);
        }
        b.push(']');
    }
    if !e.error.is_empty() {
        b.push(',');
        s(b, "error", &e.error);
    }
    b.push('}');
}

impl BtPublish {
    /// The entries served, as their states now.
    fn entries(&self, shared: &Shared) -> Vec<EntryStatus> {
        let all: Vec<EntryStatus> = lock(&shared.pubs).iter().map(|s| lock(s).clone()).collect();
        if self.names.is_empty() {
            return all;
        }
        self.names
            .iter()
            .map(|n| {
                all.iter()
                    .find(|e| e.name == *n)
                    .cloned()
                    .unwrap_or_else(|| EntryStatus {
                        name: n.clone(),
                        error: "the client has no publish entry of this name".to_owned(),
                        ..EntryStatus::default()
                    })
            })
            .collect()
    }

    fn serve(&self, method: &str, target: &str) -> Response {
        if method != "GET" && method != "HEAD" {
            let mut r = answer(
                405,
                "text/plain; charset=utf-8",
                b"method not allowed\n".to_vec(),
            );
            r.headers.set("Allow", "GET, HEAD");
            return r;
        }
        let shared = self
            .engine
            .get()
            .map(crate::tracker::key_of)
            .and_then(shared_of)
            .filter(|s| lock(&s.run).is_some());
        let Some(shared) = shared else {
            let mut r = answer(
                503,
                "text/plain; charset=utf-8",
                b"the bittorrent.client role is not running\n".to_vec(),
            );
            r.headers.set("Retry-After", "5");
            return r;
        };
        let path = target.split('?').next().unwrap_or("");
        let last = path.rsplit('/').next().unwrap_or("");
        let entries = self.entries(&shared);
        if last.is_empty() || last == "index.json" {
            let mut b = String::from("{\"publish\":[");
            for (i, e) in entries.iter().enumerate() {
                if i > 0 {
                    b.push(',');
                }
                entry_json(&mut b, e);
            }
            b.push_str("]}\n");
            return answer(200, "application/json", b.into_bytes());
        }
        let find = |id: &str| {
            entries
                .iter()
                .flat_map(|e| e.torrents.iter())
                .find(|t| t.id.eq_ignore_ascii_case(id) && t.state != "error")
                .cloned()
        };
        if let Some(id) = last.strip_suffix(".torrent") {
            if let Some(t) = find(id) {
                let mut r = answer(200, "application/x-bittorrent", t.torrent.to_vec());
                let mut name = String::new();
                write_string(&mut name, &format!("{}.torrent", t.name));
                r.headers.set(
                    "Content-Disposition",
                    format!("attachment; filename={name}"),
                );
                return r;
            }
        } else if let Some(id) = last.strip_suffix(".magnet") {
            if let Some(t) = find(id) {
                return answer(
                    200,
                    "text/plain; charset=utf-8",
                    format!("{}\n", t.magnet).into_bytes(),
                );
            }
        }
        answer(
            404,
            "text/plain; charset=utf-8",
            b"no such published torrent\n".to_vec(),
        )
    }
}

impl akio_http::Service for BtPublish {
    fn call(&self, ctx: &mut Context) {
        let method = ctx.req().method.as_str().to_owned();
        let mut r = self.serve(&method, &ctx.req().target);
        if method == "HEAD" {
            r.body.clear();
        }
        ctx.respond(r);
    }
}

impl Handler for BtPublish {
    fn inline(&self) -> bool {
        true
    }
}

pub(crate) fn new(
    p: &Provision<'_, HandlerParams>,
) -> Result<Provisioned<dyn Handler>, ProvisionError> {
    let mut names = Vec::new();
    if let HandlerParams::Module(m) = p.params() {
        match &m.params {
            None | Some(Value::Null) => {}
            Some(Value::Object(members)) => {
                for (k, v) in members {
                    match (k.as_str(), v) {
                        ("publish", Value::Array(a)) => {
                            for n in a {
                                let Value::String(n) = n else {
                                    return Err(ProvisionError::field(
                                        "publish",
                                        "a publish entry's name is a string",
                                    ));
                                };
                                names.push(n.clone());
                            }
                        }
                        ("publish", _) => {
                            return Err(ProvisionError::field(
                                "publish",
                                "the names of publish entries, a list",
                            ));
                        }
                        (other, _) => {
                            return Err(ProvisionError::field(other, "unknown field"));
                        }
                    }
                }
            }
            Some(_) => return Err(ProvisionError::new("bt_publish's params are an object")),
        }
    }
    Ok(Provisioned::new(Arc::new(BtPublish {
        engine: p.env().engine().clone(),
        names,
    }) as Arc<dyn Handler>))
}

/// The handler on `engine` serving `names`, for a listener outside a config's routes.
#[must_use]
pub fn service(engine: EngineSlot, names: Vec<String>) -> akit_runtime::HandlerService {
    akit_runtime::HandlerService(Arc::new(BtPublish { engine, names }))
}
