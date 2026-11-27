//! The proxy's methods for its own pages (I109d, fit's `llm/proxy/{role,keys,nodes,stats,
//! catalog}.go`), the caller `local` alone:
//!
//! ```text
//! /proxy/models        every model with its routes (fit's []CatalogModel)
//! /proxy/stats         the calls counted per model since the role started (StatsResp)
//! /proxy/keys/list     the API keys, without their hashes, with their use since the start
//! /proxy/keys/create   {name}: a key made, answered with the key, shown this once
//! /proxy/keys/delete   {id}: one removed
//! /proxy/profile/set   {name}: the proxy's name
//! /proxy/node/list     the nodes behind each hub, and those linked directly (NodesResp)
//! /proxy/api/get|set   the agents' API's settings (APISettings)
//! ```
//!
//! A change is an edit of the proxy's member, answered once applied. The API's certificate files
//! are read on the role's pool, never on the caller's thread.

use std::collections::HashMap;
use std::sync::{Arc, Mutex, Weak};
use std::time::Duration;

use akit_config::roles::{LlmProxyApi, MAX_LLM_KEYS};
use akit_config::value::Value;

use super::{Catalog, Core, lock};
use crate::hub::{NodeView, summary, traffic_by_node};
use crate::json::{self, Obj, Req};
use crate::mesh::{Answer, Router, bad_request};
use crate::model::{CODE_NOT_FOUND, CODE_UNAVAILABLE, Identity, ROLE_HUB, ROLE_NODE};
use crate::role::{EDIT_TIMEOUT, count_setting, duration_setting, number};
use crate::wire::Error;

/// `/proxy/models`.
pub const METHOD_PROXY_MODELS: &str = "/proxy/models";
/// `/proxy/stats`.
pub const METHOD_PROXY_STATS: &str = "/proxy/stats";
/// `/proxy/keys/list`.
pub const METHOD_PROXY_KEYS_LIST: &str = "/proxy/keys/list";
/// `/proxy/keys/create`.
pub const METHOD_PROXY_KEYS_CREATE: &str = "/proxy/keys/create";
/// `/proxy/keys/delete`.
pub const METHOD_PROXY_KEYS_DELETE: &str = "/proxy/keys/delete";
/// `/proxy/profile/set`.
pub const METHOD_PROXY_PROFILE_SET: &str = "/proxy/profile/set";
/// `/proxy/node/list`.
pub const METHOD_PROXY_NODES: &str = "/proxy/node/list";
/// `/proxy/api/get`.
pub const METHOD_PROXY_API_GET: &str = "/proxy/api/get";
/// `/proxy/api/set`.
pub const METHOD_PROXY_API_SET: &str = "/proxy/api/set";

/// Every API key starts so: `ah-` and 32 random bytes, unpadded base64url.
pub const KEY_PREFIX: &str = "ah-";
/// How many of a key's first characters the pages keep showing.
pub const SHOWN_PREFIX: usize = 10;
/// The models counted apart; calls to more are counted under `other`.
pub const MAX_STAT_MODELS: usize = 1024;

// ---- The calls counted (fit's `stats`) ----

#[derive(Debug, Clone, Default, PartialEq, Eq)]
struct ModelStats {
    calls: i64,
    errors: i64,
    bytes: i64,
    inflight: i64,
    last_at: i64,
}

impl ModelStats {
    fn to_json(&self, model: &str) -> String {
        Obj::new()
            .str("model", model)
            .int("calls", self.calls)
            .int("errors", self.errors)
            .int("bytes", self.bytes)
            .int("inflight", self.inflight)
            .int_omit("last_at", self.last_at)
            .end()
    }
}

/// The calls counted per model since the role started, in memory: [`MAX_STAT_MODELS`] names at
/// most, the others under `other`.
#[derive(Debug)]
pub struct Stats {
    since: i64,
    models: Mutex<HashMap<String, ModelStats>>,
}

impl Default for Stats {
    fn default() -> Stats {
        Stats {
            since: akit_roles::linktoken::unix_now(),
            models: Mutex::default(),
        }
    }
}

impl Stats {
    /// Counts a call to `name` begun; [`Stats::end`] with what [`Stats::begin`] gave ends it.
    #[must_use]
    pub fn begin(&self, name: &str) -> String {
        let mut g = lock(&self.models);
        let key = if g.contains_key(name) || g.len() < MAX_STAT_MODELS {
            name.to_owned()
        } else {
            "other".to_owned()
        };
        let m = g.entry(key.clone()).or_default();
        m.calls = m.calls.saturating_add(1);
        m.inflight = m.inflight.saturating_add(1);
        m.last_at = akit_roles::linktoken::unix_now();
        key
    }

    /// Ends a call to `key` that relayed `bytes` of its response, failed or not.
    pub fn end(&self, key: &str, bytes: i64, failed: bool) {
        if let Some(m) = lock(&self.models).get_mut(key) {
            m.inflight = m.inflight.saturating_sub(1);
            m.bytes = m.bytes.saturating_add(bytes);
            if failed {
                m.errors = m.errors.saturating_add(1);
            }
        }
    }

    /// `/proxy/stats`' answer (fit's `StatsResp`): the models sorted, and their total.
    #[must_use]
    pub fn snapshot(&self) -> String {
        let g = lock(&self.models);
        let mut names: Vec<&String> = g.keys().collect();
        names.sort();
        let mut total = ModelStats::default();
        let mut models = Vec::with_capacity(names.len());
        for n in names {
            if let Some(m) = g.get(n) {
                total.calls = total.calls.saturating_add(m.calls);
                total.errors = total.errors.saturating_add(m.errors);
                total.bytes = total.bytes.saturating_add(m.bytes);
                total.inflight = total.inflight.saturating_add(m.inflight);
                total.last_at = total.last_at.max(m.last_at);
                models.push(m.to_json(n));
            }
        }
        Obj::new()
            .int("since", self.since)
            .raw("total", &total.to_json(""))
            .raw("models", &json::array(models))
            .end()
    }
}

// ---- The catalog as the pages list it ----

/// A model of the list: whether a route is ready, and its routes (a hub's, how they sort, JSON).
type Entry = (bool, Vec<(bool, String, String)>);

impl Catalog {
    /// `/proxy/models`' answer (fit's `catalog.list`): each model name and API with its routes,
    /// the direct nodes first, then the hubs, by name; the models by name, then API.
    pub(super) fn list_json(&self) -> String {
        let mut by: HashMap<(String, String), Entry> = HashMap::new();
        for (id, (peer, info)) in &self.nodes {
            for m in &info.models {
                let e = by.entry((m.name.clone(), m.api.clone())).or_default();
                let r = Obj::new()
                    .str("kind", "node")
                    .str("id", id)
                    .str("name", &peer.name)
                    .bool("ready", m.ready())
                    .int("inflight", m.inflight)
                    .end();
                e.0 |= m.ready();
                e.1.push((false, format!("{}{id}", peer.name), r));
            }
        }
        for (id, (peer, hm)) in &self.hubs {
            for m in &hm.models {
                let e = by.entry((m.name.clone(), m.api.clone())).or_default();
                let r = Obj::new()
                    .str("kind", "hub")
                    .str("id", id)
                    .str("name", &peer.name)
                    .bool("ready", m.ready > 0)
                    .int("inflight", 0)
                    .int_omit("nodes", i64::try_from(m.nodes.len()).unwrap_or(i64::MAX))
                    .int_omit("ready_nodes", m.ready)
                    .end();
                e.0 |= m.ready > 0;
                e.1.push((true, format!("{}{id}", peer.name), r));
            }
        }
        let mut keys: Vec<(String, String)> = by.keys().cloned().collect();
        keys.sort();
        json::array(keys.into_iter().filter_map(|k| {
            let (ready, mut routes) = by.remove(&k)?;
            routes.sort_by(|a, b| (a.0, &a.1).cmp(&(b.0, &b.1)));
            Some(
                Obj::new()
                    .str("name", &k.0)
                    .str("api", &k.1)
                    .bool("ready", ready)
                    .raw("routes", &json::array(routes.into_iter().map(|r| r.2)))
                    .end(),
            )
        }))
    }
}

/// A hub's node list, as it pushed it last (fit's `HubNodeList`): its nodes as JSON.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(super) struct HubList {
    pub(super) nodes: String,
    pub(super) total: i64,
    pub(super) seq: i64,
}

impl HubList {
    pub(super) fn from_value(v: &Value) -> HubList {
        HubList {
            nodes: match v.get("nodes") {
                Some(n @ Value::Array(_)) => json::write(n),
                _ => "[]".to_owned(),
            },
            total: json::get_int(v, "total"),
            seq: json::get_int(v, "seq"),
        }
    }
}

impl Core {
    /// `/proxy/node/list`' answer (fit's `NodesResp`): the nodes behind each hub, as it listed
    /// them last, and the nodes linked directly, with their links and the traffic of this
    /// proxy's calls to them.
    pub(super) fn nodes_json(&self) -> String {
        let m = &self.base.mesh;
        let peers = m.peers();
        let mut links: HashMap<String, Vec<crate::model::PeerConn>> = HashMap::new();
        let mut hubs: HashMap<String, Identity> = HashMap::new();
        for p in peers {
            match p.identity.role.as_str() {
                ROLE_NODE => links.entry(p.identity.id.clone()).or_default().push(p),
                ROLE_HUB => {
                    hubs.insert(p.identity.id.clone(), p.identity);
                }
                _ => {}
            }
        }
        let traffic = traffic_by_node(m.routes());
        let cat = lock(&self.cat);
        for (id, (peer, _)) in &cat.lists {
            hubs.entry(id.clone()).or_insert_with(|| peer.clone());
        }
        let mut hs: Vec<(&String, &Identity)> = hubs.iter().collect();
        hs.sort_by(|a, b| (&a.1.name, a.0).cmp(&(&b.1.name, b.0)));
        let hub_views: Vec<String> = hs
            .into_iter()
            .map(|(id, peer)| {
                let l = cat
                    .lists
                    .get(id)
                    .map(|(_, l)| l.clone())
                    .unwrap_or_default();
                Obj::new()
                    .raw("hub", &peer.to_json())
                    .bool("online", m.online(id))
                    .raw("nodes", if l.nodes.is_empty() { "[]" } else { &l.nodes })
                    .int("total", l.total)
                    .int_omit("seq", l.seq)
                    .end()
            })
            .collect();
        let now = akit_roles::linktoken::unix_now();
        let mut ds: Vec<(&String, &(Identity, crate::model::NodeInfo))> =
            cat.nodes.iter().collect();
        ds.sort_by(|a, b| (&(a.1).1.name, a.0).cmp(&(&(b.1).1.name, b.0)));
        let direct: Vec<String> = ds
            .into_iter()
            .map(|(id, (_, info))| {
                let mut info = info.clone();
                info.id.clone_from(id);
                let n = NodeView {
                    info,
                    online: m.online(id),
                    conns: links.remove(id).unwrap_or_default(),
                    last_seen: now,
                };
                summary(&n, traffic.get(id).copied().unwrap_or_default())
            })
            .collect();
        drop(cat);
        Obj::new()
            .raw("hubs", &json::array(hub_views))
            .raw("direct", &json::array(direct))
            .end()
    }

    /// The agents' API's settings, their defaults filled in (fit's `APISettings`); all empty
    /// without an API.
    pub(super) fn api_json(&self) -> String {
        let Some(cfg) = lock(&self.api_want).clone() else {
            return Obj::new()
                .str("address", "")
                .str("cert_file", "")
                .str("key_file", "")
                .int("max_body_bytes", 0)
                .int("max_conns", 0)
                .int("max_inflight", 0)
                .str("first_byte_timeout", "")
                .str("timeout", "")
                .end();
        };
        let d = akit_config::duration::format;
        let n = |v: usize| i64::try_from(v).unwrap_or(i64::MAX);
        Obj::new()
            .str("address", &cfg.address)
            .str("cert_file", &cfg.cert_file)
            .str("key_file", &cfg.key_file)
            .int(
                "max_body_bytes",
                i64::try_from(cfg.body()).unwrap_or(i64::MAX),
            )
            .int("max_conns", n(cfg.conns()))
            .int("max_inflight", n(cfg.inflight()))
            .str("first_byte_timeout", &d(cfg.first_byte()))
            .str("timeout", &d(cfg.total()))
            .str_omit("url", &self.api_url())
            .end()
    }

    /// Where the agents' API is served, `""` when it is not.
    pub(super) fn api_url(&self) -> String {
        let Some(addr) = self.api_addr() else {
            return String::new();
        };
        let secure = !lock(&self.api_cfg).cert_file.is_empty();
        format!("{}://{addr}", if secure { "https" } else { "http" })
    }

    /// The keys as the pages list them (fit's `KeyView`): no hash; their use since the start.
    pub(super) fn keys_json(&self) -> String {
        let keys = lock(&self.keys).clone();
        let usage = lock(&self.usage);
        json::array(keys.iter().map(|k| {
            let (last, calls) = usage.get(&k.id).copied().unwrap_or_default();
            key_view(&k.id, &k.name, &k.prefix, k.created_at, last, calls).end()
        }))
    }
}

fn key_view(id: &str, name: &str, prefix: &str, created: i64, last: i64, calls: i64) -> Obj {
    Obj::new()
        .str("id", id)
        .str("name", name)
        .str("prefix", prefix)
        .int("created_at", created)
        .int("last_used_at", last)
        .int("calls", calls)
}

/// A key made (fit's `newKey`): its secret, shown once, and what the config keeps.
///
/// # Errors
///
/// A name that is empty or longer than 64 characters; no random bytes.
pub fn new_key(name: &str) -> Result<(akit_config::roles::LlmProxyKey, String), Error> {
    let name = name.trim();
    if name.is_empty() {
        return Err(bad_request("name is required"));
    }
    if name.chars().count() > 64 {
        return Err(bad_request("name is too long (64 characters at most)"));
    }
    let mut b = [0u8; 32];
    akio_crypto::rand::fill_random(&mut b).map_err(|e| Error::Other(e.to_string()))?;
    let secret = format!(
        "{KEY_PREFIX}{}",
        akio_crypto::base64::URL_SAFE_NO_PAD.encode(b)
    );
    let mut id = [0u8; 8];
    akio_crypto::rand::fill_random(&mut id).map_err(|e| Error::Other(e.to_string()))?;
    let hash = akit_roles::linktoken::hex(&akio_crypto::hash::Sha256::digest(secret.as_bytes()));
    Ok((
        akit_config::roles::LlmProxyKey {
            id: akit_roles::linktoken::hex(&id),
            name: name.to_owned(),
            prefix: secret.get(..SHOWN_PREFIX).unwrap_or_default().to_owned(),
            hash,
            created_at: akit_roles::linktoken::unix_now(),
        },
        secret,
    ))
}

fn key_value(k: &akit_config::roles::LlmProxyKey) -> Value {
    let mut o = vec![
        ("id".to_owned(), Value::String(k.id.clone())),
        ("name".to_owned(), Value::String(k.name.clone())),
    ];
    if !k.prefix.is_empty() {
        o.push(("prefix".to_owned(), Value::String(k.prefix.clone())));
    }
    o.push(("hash".to_owned(), Value::String(k.hash.clone())));
    if k.created_at != 0 {
        o.push(("created_at".to_owned(), number(k.created_at)));
    }
    Value::Object(o)
}

/// Changes the member's `api_keys` with `f` (fit's `editKeys`): no more than [`MAX_LLM_KEYS`].
fn edit_keys(
    c: &Core,
    f: impl FnOnce(&mut Vec<Value>) -> Result<(), Error> + Send + 'static,
    done: impl FnOnce(Result<(), Error>) + Send + 'static,
) {
    c.base.edit(
        move |m| {
            let mut keys = match m.iter().find(|(k, _)| k == "api_keys").map(|(_, v)| v) {
                None | Some(Value::Null) => Vec::new(),
                Some(Value::Array(a)) => a.clone(),
                Some(_) => return Err(bad_request("api_keys is not a list")),
            };
            f(&mut keys)?;
            if keys.len() > MAX_LLM_KEYS {
                return Err(bad_request(format!("at most {MAX_LLM_KEYS} keys")));
            }
            crate::role::set(m, "api_keys", Value::Array(keys));
            Ok(())
        },
        done,
    );
}

fn loopback(host: &str) -> bool {
    akit_config::roles::loopback(host)
}

/// The `api` object to write, its empty and default fields left out (fit's `setAPI`), with its
/// certificate and key files, read to check them; `None` for an empty address, no API.
fn api_value(r: &Req) -> Result<Option<(Value, String, String)>, Error> {
    use akit_config::roles as cr;
    let addr = r.str("address")?.trim().to_owned();
    if addr.is_empty() {
        return Ok(None);
    }
    let bad_addr = || bad_request("the address is a host and port, as 127.0.0.1:9301");
    let (host, port) = addr.rsplit_once(':').ok_or_else(bad_addr)?;
    if port.is_empty() {
        return Err(bad_addr());
    }
    let host = host.trim_start_matches('[').trim_end_matches(']');
    let cert = r.str("cert_file")?.trim().to_owned();
    let key = r.str("key_file")?.trim().to_owned();
    if cert.is_empty() != key.is_empty() {
        return Err(bad_request("cert_file and key_file go together"));
    }
    if cert.is_empty() && !loopback(host) {
        return Err(bad_request(format!(
            "{addr} is not a loopback address: set cert_file and key_file, so that keys are not sent in the clear"
        )));
    }
    let n = |d: usize| i64::try_from(d).unwrap_or(i64::MAX);
    let body = count_setting(
        "max_body_bytes",
        r.int("max_body_bytes")?,
        i64::try_from(cr::DEFAULT_LLM_MAX_BODY_BYTES).unwrap_or(i64::MAX),
    )?;
    let conns = count_setting(
        "max_conns",
        r.int("max_conns")?,
        n(cr::DEFAULT_LLM_PROXY_MAX_CONNS),
    )?;
    let inflight = count_setting(
        "max_inflight",
        r.int("max_inflight")?,
        n(cr::DEFAULT_LLM_PROXY_MAX_INFLIGHT),
    )?;
    let first = duration_setting(
        "first_byte_timeout",
        &r.str("first_byte_timeout")?,
        Duration::ZERO,
        cr::DEFAULT_LLM_FIRST_BYTE_TIMEOUT,
    )?;
    let total = duration_setting(
        "timeout",
        &r.str("timeout")?,
        Duration::ZERO,
        cr::DEFAULT_LLM_CALL_TIMEOUT,
    )?;
    let mut o = vec![("address".to_owned(), Value::String(addr))];
    for (k, v) in [("cert_file", &cert), ("key_file", &key)] {
        if !v.is_empty() {
            o.push((k.to_owned(), Value::String(v.clone())));
        }
    }
    for (k, v) in [
        ("max_body_bytes", body),
        ("max_conns", conns),
        ("max_inflight", inflight),
    ] {
        if v != 0 {
            o.push((k.to_owned(), number(v)));
        }
    }
    for (k, v) in [("first_byte_timeout", first), ("timeout", total)] {
        if !v.is_empty() {
            o.push((k.to_owned(), Value::String(v)));
        }
    }
    Ok(Some((Value::Object(o), cert, key)))
}

/// Reads the API's certificate and key, as the API would serve them. Reads files: the role's
/// pool's.
fn check_cert(cert: &str, key: &str) -> Result<(), Error> {
    let bad = |e: String| bad_request(format!("the certificate: {e}"));
    let c = std::fs::read(cert).map_err(|e| bad(e.to_string()))?;
    let k = std::fs::read(key).map_err(|e| bad(e.to_string()))?;
    let chain = akio::tls::Certificate::from_pem(&c).map_err(|e| bad(e.to_string()))?;
    let pk = akio::tls::PrivateKey::from_pem(&k).map_err(|e| bad(e.to_string()))?;
    akio::tls::ServerConfig::builder(chain, pk)
        .build()
        .map(|_| ())
        .map_err(|e| bad(e.to_string()))
}

fn with(me: &Weak<Core>, a: Answer, f: impl FnOnce(&Arc<Core>, Answer)) {
    match me.upgrade() {
        Some(c) => f(&c, a),
        None => a.err(Error::Closed),
    }
}

fn parsed(req: &[u8], a: &Answer) -> Option<Req> {
    match Req::parse(req) {
        Ok(r) => Some(r),
        Err(e) => {
            a.err(e);
            None
        }
    }
}

/// Answers `a` once an edit is applied: with what `f` makes of the proxy then.
fn answer_with(
    me: &Weak<Core>,
    a: Answer,
    f: impl FnOnce(&Core) -> String + Send + 'static,
) -> impl FnOnce(Result<(), Error>) + Send + 'static {
    let me = me.clone();
    move |r| match (r, me.upgrade()) {
        (Ok(()), Some(c)) => a.ok(f(&c)),
        (Ok(()), None) => a.err(Error::Closed),
        (Err(e), _) => a.err(e),
    }
}

/// Registers the methods on `r`, for the proxy's own pages alone.
#[allow(clippy::too_many_lines)]
pub(super) fn register(r: &mut Router, me: &Weak<Core>) {
    let m = me.clone();
    r.handle(METHOD_PROXY_MODELS, &[], Duration::ZERO, move |_, _, a| {
        with(&m, a, |c, a| a.ok(lock(&c.cat).list_json()));
    });
    let m = me.clone();
    r.handle(METHOD_PROXY_STATS, &[], Duration::ZERO, move |_, _, a| {
        with(&m, a, |c, a| a.ok(c.stats.snapshot()));
    });
    let m = me.clone();
    r.handle(
        METHOD_PROXY_KEYS_LIST,
        &[],
        Duration::ZERO,
        move |_, _, a| {
            with(&m, a, |c, a| a.ok(c.keys_json()));
        },
    );
    let m = me.clone();
    r.handle(
        METHOD_PROXY_KEYS_CREATE,
        &[],
        EDIT_TIMEOUT,
        move |_, req, a| {
            let Some(rq) = parsed(req, &a) else { return };
            let made = rq.str("name").and_then(|n| new_key(&n));
            let (k, secret) = match made {
                Ok(v) => v,
                Err(e) => return a.err(e),
            };
            with(&m, a, |c, a| {
                let v = key_value(&k);
                let answer = key_view(&k.id, &k.name, &k.prefix, k.created_at, 0, 0)
                    .str("key", &secret)
                    .end();
                let (id, name) = (k.id.clone(), k.name.clone());
                edit_keys(
                    c,
                    move |keys| {
                        keys.push(v);
                        Ok(())
                    },
                    move |r| match r {
                        Ok(()) => {
                            akit_observe::info!(Llm, "llm.proxy: api key {id} ({name}) created");
                            a.ok(answer);
                        }
                        Err(e) => a.err(e),
                    },
                );
            });
        },
    );
    let m = me.clone();
    r.handle(
        METHOD_PROXY_KEYS_DELETE,
        &[],
        EDIT_TIMEOUT,
        move |_, req, a| {
            let Some(rq) = parsed(req, &a) else { return };
            let id = match rq.str("id") {
                Ok(v) => v,
                Err(e) => return a.err(e),
            };
            with(&m, a, |c, a| {
                let id2 = id.clone();
                edit_keys(
                    c,
                    move |keys| {
                        let i = keys
                            .iter()
                            .position(|k| json::get_str(k, "id") == id2)
                            .ok_or_else(|| {
                                Error::code(CODE_NOT_FOUND, format!("no api key {id2:?}"))
                            })?;
                        keys.remove(i);
                        Ok(())
                    },
                    move |r| match r {
                        Ok(()) => {
                            akit_observe::info!(Llm, "llm.proxy: api key {id} deleted");
                            a.ok("{}".to_owned());
                        }
                        Err(e) => a.err(e),
                    },
                );
            });
        },
    );
    let m = me.clone();
    r.handle(
        METHOD_PROXY_PROFILE_SET,
        &[],
        EDIT_TIMEOUT,
        move |_, req, a| {
            let Some(rq) = parsed(req, &a) else { return };
            let name = match rq.str("name") {
                Ok(n) => n.trim().to_owned(),
                Err(e) => return a.err(e),
            };
            if name.is_empty() || name.chars().count() > 64 {
                return a.err(bad_request("name must have 1 to 64 characters"));
            }
            with(&m, a, |c, a| {
                c.base.set_name(name, move |r| match r {
                    Ok(()) => a.ok("{}".to_owned()),
                    Err(e) => a.err(e),
                });
            });
        },
    );
    let m = me.clone();
    r.handle(METHOD_PROXY_NODES, &[], Duration::ZERO, move |_, _, a| {
        with(&m, a, |c, a| a.ok(c.nodes_json()));
    });
    let m = me.clone();
    r.handle(METHOD_PROXY_API_GET, &[], Duration::ZERO, move |_, _, a| {
        with(&m, a, |c, a| a.ok(c.api_json()));
    });
    let m = me.clone();
    r.handle(METHOD_PROXY_API_SET, &[], EDIT_TIMEOUT, move |_, req, a| {
        let Some(rq) = parsed(req, &a) else { return };
        let api = match api_value(&rq) {
            Ok(v) => v,
            Err(e) => return a.err(e),
        };
        let m2 = m.clone();
        with(&m, a, move |c, a| {
            let Some((v, cert, key)) = api else {
                return c.base.edit(
                    |ms| {
                        ms.retain(|(k, _)| k != "api");
                        Ok(())
                    },
                    answer_with(&m2, a, Core::api_json),
                );
            };
            let write = move |c: &Core, a: Answer| {
                c.base.edit(
                    move |ms| {
                        match ms.iter_mut().find(|(k, _)| k == "api") {
                            Some((_, old)) => *old = v,
                            None => ms.push(("api".to_owned(), v)),
                        }
                        Ok(())
                    },
                    answer_with(&m2, a, Core::api_json),
                );
            };
            if cert.is_empty() {
                return write(c, a);
            }
            // The certificate is read on the role's pool.
            let (c2, a2) = (Arc::clone(c), a.clone());
            let job = move || match check_cert(&cert, &key) {
                Ok(()) => write(&c2, a2),
                Err(e) => a2.err(e),
            };
            if c.base.work.try_execute(Box::new(job)).is_err() {
                a.err(Error::code(
                    CODE_UNAVAILABLE,
                    "the role is busy; try again in a moment",
                ));
            }
        });
    });
}

/// The API's settings as applied, for `/proxy/api/get`.
pub(super) fn api_of(p: &akit_config::roles::LlmProxy) -> Option<LlmProxyApi> {
    p.api.clone().filter(|a| !a.address.is_empty())
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::indexing_slicing)]
    use super::*;
    use crate::model::{HubModel, HubModels, MODEL_READY, ModelView, NodeInfo};

    /// fit's `TestStatsOverflow`: past the bound, calls are counted under `other`; sorted.
    #[test]
    fn stats_overflow() {
        let s = Stats::default();
        for i in 0..MAX_STAT_MODELS + 2 {
            let k = s.begin(&format!("m{i:04}"));
            s.end(&k, 1, i % 2 == 0);
        }
        let v = json::parse(s.snapshot().as_bytes()).unwrap();
        let models = json::get_arr(&v, "models");
        assert_eq!(models.len(), MAX_STAT_MODELS + 1);
        assert_eq!(json::get_str(models.last().unwrap(), "model"), "other");
        let total = v.get("total").unwrap();
        assert_eq!(
            json::get_int(total, "calls"),
            i64::try_from(MAX_STAT_MODELS + 2).unwrap()
        );
        assert_eq!(json::get_int(total, "inflight"), 0);
        assert_eq!(json::get_int(total, "bytes"), 1026);
    }

    fn ready(name: &str, api: &str) -> ModelView {
        ModelView {
            name: name.into(),
            api: api.into(),
            state: MODEL_READY.into(),
            ..ModelView::default()
        }
    }

    fn peer(id: &str) -> Identity {
        Identity {
            id: id.into(),
            name: id.into(),
            ..Identity::default()
        }
    }

    /// fit's `TestCatalogOrder` of the list: the models by name then API, the routes the nodes
    /// first.
    #[test]
    fn catalog_list() {
        let mut c = Catalog::default();
        c.set_node(
            &peer("n1"),
            NodeInfo {
                models: vec![ready("m", "openai"), ready("m", "anthropic")],
                ..NodeInfo::default()
            },
        );
        c.set_hub(
            &peer("h1"),
            HubModels {
                models: vec![HubModel {
                    name: "m".into(),
                    api: "openai".into(),
                    nodes: vec!["x".into(), "y".into()],
                    ready: 1,
                }],
                ..HubModels::default()
            },
        );
        let v = json::parse(c.list_json().as_bytes()).unwrap();
        let Value::Array(list) = v else { panic!() };
        assert_eq!(list.len(), 2);
        assert_eq!(json::get_str(&list[0], "api"), "anthropic");
        let routes = json::get_arr(&list[1], "routes");
        assert_eq!(json::get_str(&routes[0], "kind"), "node");
        assert_eq!(json::get_str(&routes[1], "kind"), "hub");
        assert_eq!(json::get_int(&routes[1], "nodes"), 2);
        assert!(json::get_bool(&list[1], "ready"));
    }

    /// fit's `newKey`: `ah-` and 43 characters, its prefix shown, its hash kept.
    #[test]
    fn keys_made() {
        let (k, secret) = new_key(" a ").unwrap();
        assert_eq!(k.name, "a");
        assert!(
            secret.starts_with(KEY_PREFIX) && secret.len() == 46,
            "{secret}"
        );
        assert_eq!(k.prefix, &secret[..SHOWN_PREFIX]);
        assert_eq!(k.id.len(), 16);
        assert!(new_key(" ").is_err());
        assert!(new_key(&"n".repeat(65)).is_err());
    }

    /// fit's `setAPI` checks.
    #[test]
    fn api_settings_checked() {
        let p = |s: &str| api_value(&Req::parse(s.as_bytes()).unwrap());
        assert!(p(r#"{"address": ""}"#).unwrap().is_none());
        let (v, _, _) = p(r#"{"address": "127.0.0.1:9301", "max_conns": 256, "timeout": "1h"}"#)
            .unwrap()
            .unwrap();
        assert_eq!(
            json::write(&v),
            r#"{"address":"127.0.0.1:9301","timeout":"1h0m0s"}"#
        );
        for bad in [
            r#"{"address": "nope"}"#,
            r#"{"address": "10.0.0.1:9301"}"#,
            r#"{"address": "127.0.0.1:1", "cert_file": "x"}"#,
            r#"{"address": "127.0.0.1:1", "max_inflight": -1}"#,
        ] {
            assert!(p(bad).is_err(), "{bad}");
        }
    }
}
