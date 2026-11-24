//! The Consul providers: fit's `provider/consul`. [`ConsulCatalog`] turns the services of a
//! Consul catalog and their tags into upstreams, routes and services; [`ConsulKv`] reads parts
//! from keys in Consul's KV store.
//!
//! Both speak Consul's HTTP API with akio's client, and follow it with blocking queries, as
//! fit's do (fit's decision 013): an index that goes back starts over from 0, one below 1 is
//! taken as 1, and the queries of one poll start at least a second apart. They run in the
//! control plane, on threads of their own ([`crate::watch`]).

use std::collections::BTreeMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{Duration, Instant};

use akio_crypto::base64::STANDARD;
use akio_http::client::ClientRequest;
use akit_config::value::Value;

use crate::api::{
    Api, ApiEngine, MAX_BODY_BYTES, parse_json, path_escape, query_escape, status_error,
};
use crate::kvpart::{self, Entry};
use crate::labels::{self, Group};
use crate::run::{Provider, Update};
use crate::watch::{Conn, Events, Follower, Log, Running, Source, Stop, Timings};

/// The timings of Consul's blocking queries: fit's `blockWait` and `minPollGap`.
#[derive(Debug, Clone, Copy)]
pub(crate) struct Blocking {
    /// How long a blocking query waits for a change; Consul adds up to a sixteenth of it.
    pub(crate) wait: Duration,
    /// The least time between the starts of two queries of one poll, so that an index that
    /// changes all the time does not make it spin.
    pub(crate) min_gap: Duration,
}

impl Default for Blocking {
    fn default() -> Blocking {
        Blocking {
            wait: Duration::from_secs(5 * 60),
            min_gap: Duration::from_secs(1),
        }
    }
}

/// What a provider reaches Consul with: where it is, and how.
#[derive(Clone)]
struct ConnSettings {
    conn: akit_config::ConsulConn,
    blocking: Blocking,
    max_body: usize,
}

/// A client of Consul's API, for one watch and the reads that go with it.
struct Client {
    s: ConnSettings,
    api: Api,
    token: String,
}

impl ConnSettings {
    /// Makes a client, reading the token and the TLS files.
    fn open(&self, engine: &ApiEngine) -> Result<Client, String> {
        let token = if self.conn.token_file.is_empty() {
            String::new()
        } else {
            let path = &self.conn.token_file;
            std::fs::read_to_string(path)
                .map_err(|e| format!("{path}: {e}"))?
                .trim()
                .to_owned()
        };
        let api = Api::open(engine, &self.conn.tls, self.max_body)?;
        Ok(Client {
            s: self.clone(),
            api,
            token,
        })
    }
}

impl Client {
    /// Asks Consul for `path` with `query`, returning its answer (`None` for a 404, as for keys
    /// that are not there) and its index. With `index` above 0 it is a blocking query, which
    /// waits for an index past it.
    fn get(
        &self,
        path: &str,
        query: &[(&str, &str)],
        index: u64,
        stop: &Stop,
    ) -> Result<(Option<Vec<u8>>, u64), String> {
        let mut q: Vec<(&str, String)> = query.iter().map(|(k, v)| (*k, (*v).to_owned())).collect();
        if !self.s.conn.datacenter.is_empty() {
            q.push(("dc", self.s.conn.datacenter.clone()));
        }
        let mut timeout = self.s.conn.request_timeout;
        if index > 0 {
            let wait = self.s.blocking.wait;
            q.push(("index", index.to_string()));
            q.push(("wait", akit_config::duration::format(wait)));
            timeout = timeout
                .saturating_add(wait)
                .saturating_add(wait.checked_div(16).unwrap_or_default());
        }
        // As Go's url.Values.Encode: by key.
        q.sort_by(|a, b| a.0.cmp(b.0));
        let mut url = format!("{}{path}", self.s.conn.endpoint);
        for (i, (k, v)) in q.iter().enumerate() {
            url.push(if i == 0 { '?' } else { '&' });
            url.push_str(&query_escape(k));
            url.push('=');
            url.push_str(&query_escape(v));
        }
        let mut req = ClientRequest::get(&url);
        if !self.token.is_empty() {
            req = req.header("X-Consul-Token", &self.token);
        }
        let what = format!("GET {path}");
        let r = self.api.call(req, timeout, stop, &what)?;
        let mut next = 0;
        if let Some(h) = r.headers.get("X-Consul-Index") {
            next = h
                .parse()
                .map_err(|_| format!("{what}: X-Consul-Index {h:?} is not a number"))?;
        }
        match r.status {
            200 => Ok((Some(r.body), next)),
            404 => Ok((None, next)),
            _ => Err(status_error(&r, &what)),
        }
    }

    /// Follows `path` with blocking queries until `stop` or a query fails, calling `ready` after
    /// the first answer and `changed` at each new index after it.
    fn poll(
        &self,
        path: &str,
        query: &[(&str, &str)],
        stop: &Stop,
        ready: &dyn Fn(),
        changed: &dyn Fn(),
    ) -> Result<(), String> {
        let mut index = 0u64;
        let mut last: Option<Instant> = None;
        let mut started = false;
        loop {
            if let Some(l) = last {
                let wait = self.s.blocking.min_gap.saturating_sub(l.elapsed());
                if !wait.is_zero() && !stop.sleep(wait) {
                    return Err("stopped".to_owned());
                }
            }
            last = Some(Instant::now());
            let (_, next) = self.get(path, query, index, stop)?;
            if !started {
                started = true;
                ready();
            } else if next != index {
                changed();
            }
            index = if next < index {
                0
            } else if next < 1 {
                1
            } else {
                next
            };
        }
    }
}

/// Escapes a key's path, segment by segment.
fn escape_path(key: &str) -> String {
    key.split('/')
        .map(path_escape)
        .collect::<Vec<_>>()
        .join("/")
}

/// A provider that follows Consul: the engine its client runs on, and its follower.
struct Followed {
    running: Option<(Running, Arc<ApiEngine>)>,
}

impl Followed {
    fn stop(&mut self) {
        if let Some((mut r, engine)) = self.running.take() {
            r.stop();
            drop(engine);
        }
    }
}

// ---- The KV provider. ----

/// The Consul KV provider: each key under its prefix holds a part, a JSON object of listeners,
/// routes, upstreams and services, and it provides them together, in the order of their keys.
pub struct ConsulKv {
    name: String,
    opts: akit_config::ConsulKvProvider,
    log: Log,
    timings: Timings,
    blocking: Blocking,
    max_body: usize,
    f: Followed,
}

impl std::fmt::Debug for ConsulKv {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ConsulKv")
            .field("name", &self.name)
            .finish_non_exhaustive()
    }
}

impl ConsulKv {
    /// A Consul KV provider named `name`, with the settings `opts`; `log` hears what it reports:
    /// keys left out and Consul failing.
    #[must_use]
    pub fn new(name: &str, opts: &akit_config::ConsulKvProvider, log: Log) -> ConsulKv {
        ConsulKv {
            name: name.to_owned(),
            opts: opts.clone(),
            log,
            timings: Timings::default(),
            blocking: Blocking::default(),
            max_body: MAX_BODY_BYTES,
            f: Followed { running: None },
        }
    }

    /// What it watches, as akit says it when it starts: fit's words.
    #[must_use]
    pub fn watching(opts: &akit_config::ConsulKvProvider) -> String {
        format!(
            "watching Consul's keys under {:?} at {}",
            opts.prefix, opts.conn.endpoint
        )
    }

    #[cfg(test)]
    pub(crate) fn set_timings(&mut self, t: Timings, b: Blocking) {
        self.timings = t;
        self.blocking = b;
    }
}

impl Provider for ConsulKv {
    fn name(&self) -> &str {
        &self.name
    }

    /// Follows the keys, sending what they provide: at once, after each change, and every
    /// `refresh_interval` whether or not it changed. When Consul cannot be reached, the last
    /// part stays, and Consul is tried again, from 1s to 30s apart.
    fn start(&mut self, update: Update) -> std::io::Result<()> {
        self.f.stop();
        let engine = Arc::new(ApiEngine::start(&self.name).map_err(std::io::Error::other)?);
        let source = KvSource {
            settings: ConnSettings {
                conn: self.opts.conn.clone(),
                blocking: self.blocking,
                max_body: self.max_body,
            },
            engine: Arc::clone(&engine),
            prefix: self.opts.prefix.clone(),
            max_keys: self.opts.max_keys,
        };
        let mut f = Follower::new(
            &self.name,
            self.opts.conn.refresh_interval,
            Arc::clone(&self.log),
            source,
        );
        f.timings = self.timings;
        self.f.running = Some((Running::start(f, update)?, engine));
        Ok(())
    }

    fn stop(&mut self) {
        self.f.stop();
    }
}

impl Drop for ConsulKv {
    fn drop(&mut self) {
        self.f.stop();
    }
}

struct KvSource {
    settings: ConnSettings,
    engine: Arc<ApiEngine>,
    prefix: String,
    max_keys: usize,
}

impl Source for KvSource {
    type Conn = KvConn;

    fn open(&mut self, _stop: &Stop) -> Result<Arc<KvConn>, String> {
        Ok(Arc::new(KvConn {
            c: self.settings.open(&self.engine)?,
            path: format!("/v1/kv/{}", escape_path(&self.prefix)),
            max_keys: self.max_keys,
        }))
    }
}

struct KvConn {
    c: Client,
    path: String,
    max_keys: usize,
}

const RECURSE: &[(&str, &str)] = &[("recurse", "true")];

impl Conn for KvConn {
    /// Polls the keys under the prefix.
    fn watch(&self, stop: &Stop, ev: &Events) -> Result<(), String> {
        self.c
            .poll(&self.path, RECURSE, stop, &|| ev.ready(), &|| ev.changed())
    }

    /// Reads the keys under the prefix and their values.
    fn read(&self, stop: &Stop) -> Result<(Vec<u8>, Vec<String>), String> {
        let (data, _) = self.c.get(&self.path, RECURSE, 0, stop)?;
        let mut entries = Vec::new();
        let what = format!("GET {}", self.path);
        if let Some(data) = data.filter(|d| !d.is_empty()) {
            let Value::Array(kvs) = parse_json(&data, &what)? else {
                return Err(format!("{what}: the answer is not an array"));
            };
            for kv in &kvs {
                let key = match kv.get("Key") {
                    Some(Value::String(k)) => k.clone(),
                    _ => return Err(format!("{what}: an entry without a Key")),
                };
                let value = match kv.get("Value") {
                    None | Some(Value::Null) => Vec::new(),
                    Some(Value::String(v)) => STANDARD.decode(v).map_err(|e| {
                        format!("{what}: key {key}: a Value that is not base64: {e}")
                    })?,
                    Some(_) => {
                        return Err(format!("{what}: key {key}: a Value that is not a string"));
                    }
                };
                entries.push(Entry { key, value });
            }
        }
        Ok(kvpart::part(entries, self.max_keys))
    }
}

// ---- The catalog provider. ----

/// The Consul catalog provider. Each service taken is an upstream of its instances whose health
/// checks pass, named for the service or its `<prefix>.name` tag; its instances' tags of the
/// form `<prefix>.<key>=<value>` are labels, as a container's are for the Docker provider
/// ([`crate::labels`]), and the first instance by node and ID writes the routes and services.
/// Tags without `=` are passed over.
pub struct ConsulCatalog {
    name: String,
    opts: akit_config::ConsulProvider,
    log: Log,
    timings: Timings,
    blocking: Blocking,
    max_body: usize,
    f: Followed,
}

impl std::fmt::Debug for ConsulCatalog {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ConsulCatalog")
            .field("name", &self.name)
            .finish_non_exhaustive()
    }
}

impl ConsulCatalog {
    /// A Consul catalog provider named `name`, with the settings `opts`; `log` hears what it
    /// reports: instances left out and Consul failing.
    #[must_use]
    pub fn new(name: &str, opts: &akit_config::ConsulProvider, log: Log) -> ConsulCatalog {
        ConsulCatalog {
            name: name.to_owned(),
            opts: opts.clone(),
            log,
            timings: Timings::default(),
            blocking: Blocking::default(),
            max_body: MAX_BODY_BYTES,
            f: Followed { running: None },
        }
    }

    /// What it watches, as akit says it when it starts: fit's words.
    #[must_use]
    pub fn watching(opts: &akit_config::ConsulProvider) -> String {
        format!("watching Consul's catalog at {}", opts.conn.endpoint)
    }

    #[cfg(test)]
    pub(crate) fn set_timings(&mut self, t: Timings, b: Blocking) {
        self.timings = t;
        self.blocking = b;
    }
}

impl Provider for ConsulCatalog {
    fn name(&self) -> &str {
        &self.name
    }

    /// Follows the catalog, sending what its services provide: at once, after each change to
    /// the catalog or to a health check, and every `refresh_interval` whether or not it
    /// changed. When Consul cannot be reached, the last part stays, and Consul is tried again,
    /// from 1s to 30s apart.
    fn start(&mut self, update: Update) -> std::io::Result<()> {
        self.f.stop();
        let engine = Arc::new(ApiEngine::start(&self.name).map_err(std::io::Error::other)?);
        let source = CatalogSource {
            settings: ConnSettings {
                conn: self.opts.conn.clone(),
                blocking: self.blocking,
                max_body: self.max_body,
            },
            engine: Arc::clone(&engine),
            opts: self.opts.clone(),
        };
        let mut f = Follower::new(
            &self.name,
            self.opts.conn.refresh_interval,
            Arc::clone(&self.log),
            source,
        );
        f.timings = self.timings;
        self.f.running = Some((Running::start(f, update)?, engine));
        Ok(())
    }

    fn stop(&mut self) {
        self.f.stop();
    }
}

impl Drop for ConsulCatalog {
    fn drop(&mut self) {
        self.f.stop();
    }
}

struct CatalogSource {
    settings: ConnSettings,
    engine: Arc<ApiEngine>,
    opts: akit_config::ConsulProvider,
}

impl Source for CatalogSource {
    type Conn = CatalogConn;

    fn open(&mut self, _stop: &Stop) -> Result<Arc<CatalogConn>, String> {
        Ok(Arc::new(CatalogConn {
            c: self.settings.open(&self.engine)?,
            opts: self.opts.clone(),
        }))
    }
}

struct CatalogConn {
    c: Client,
    opts: akit_config::ConsulProvider,
}

/// What `/v1/health/service` says of an instance.
#[derive(Debug, Clone, Default)]
pub(crate) struct Instance {
    pub(crate) node: String,
    pub(crate) node_address: String,
    pub(crate) id: String,
    pub(crate) tags: Vec<String>,
    pub(crate) address: String,
    pub(crate) port: u16,
}

fn str_of(v: Option<&Value>) -> String {
    match v {
        Some(Value::String(s)) => s.clone(),
        _ => String::new(),
    }
}

impl Instance {
    fn from_json(e: &Value) -> Instance {
        let node = e.get("Node");
        let svc = e.get("Service");
        Instance {
            node: str_of(node.and_then(|n| n.get("Node"))),
            node_address: str_of(node.and_then(|n| n.get("Address"))),
            id: str_of(svc.and_then(|s| s.get("ID"))),
            tags: match svc.and_then(|s| s.get("Tags")) {
                Some(Value::Array(a)) => a
                    .iter()
                    .filter_map(|t| match t {
                        Value::String(s) => Some(s.clone()),
                        _ => None,
                    })
                    .collect(),
                _ => Vec::new(),
            },
            address: str_of(svc.and_then(|s| s.get("Address"))),
            port: match svc.and_then(|s| s.get("Port")) {
                Some(Value::Number(n)) => n.parse().unwrap_or(0),
                _ => 0,
            },
        }
    }
}

const PATHS: [&str; 2] = ["/v1/catalog/services", "/v1/health/state/any"];

impl Conn for CatalogConn {
    /// Polls the catalog's services and the health checks: a service registered or gone changes
    /// the one, an instance passing or failing its checks the other.
    fn watch(&self, stop: &Stop, ev: &Events) -> Result<(), String> {
        let polls = stop.child();
        let waiting = AtomicUsize::new(PATHS.len());
        let ready = || {
            if waiting.fetch_sub(1, Ordering::SeqCst) == 1 {
                ev.ready();
            }
        };
        let changed = || ev.changed();
        // The first to end ends the other.
        std::thread::scope(|s| {
            let other = s.spawn(|| {
                let r = self.c.poll(PATHS[1], &[], &polls, &ready, &changed);
                polls.cancel();
                r
            });
            let r = self.c.poll(PATHS[0], &[], &polls, &ready, &changed);
            polls.cancel();
            let o = other
                .join()
                .unwrap_or_else(|_| Err("a poll panicked".to_owned()));
            // The error of the one that ended first, not the "stopped" of the other.
            match (r, o) {
                (Err(e), Err(o)) if e == "stopped" => Err(o),
                (r, _) => r,
            }
        })
    }

    /// Lists the catalog's services, and the instances of those taken whose checks pass.
    fn read(&self, stop: &Stop) -> Result<(Vec<u8>, Vec<String>), String> {
        let what = "GET /v1/catalog/services";
        let (data, _) = self.c.get(PATHS[0], &[], 0, stop)?;
        let services = match data {
            Some(d) => parse_json(&d, what)?,
            None => Value::Object(Vec::new()),
        };
        let Value::Object(services) = services else {
            return Err(format!("{what}: the answer is not an object"));
        };
        let mut services: Vec<(String, Vec<String>)> = services
            .into_iter()
            .map(|(name, tags)| {
                let tags = match tags {
                    Value::Array(a) => a
                        .into_iter()
                        .filter_map(|t| match t {
                            Value::String(s) => Some(s),
                            _ => None,
                        })
                        .collect(),
                    _ => Vec::new(),
                };
                (name, tags)
            })
            .collect();
        services.sort_by(|a, b| a.0.cmp(&b.0));
        let enable = format!("{}.enable=true", self.opts.tag_prefix);
        let mut problems = Vec::new();
        let mut instances: BTreeMap<String, Vec<Instance>> = BTreeMap::new();
        let mut taken = 0usize;
        for (name, tags) in &services {
            if !self.opts.expose_all && !tags.contains(&enable) {
                continue;
            }
            if taken == self.opts.max_services {
                problems.push(format!(
                    "service {name}: left out: max_services, {}, are taken",
                    self.opts.max_services
                ));
                continue;
            }
            taken = taken.saturating_add(1);
            let path = format!("/v1/health/service/{}", path_escape(name));
            let (data, _) = self.c.get(&path, &[("passing", "1")], 0, stop)?;
            let es = match data {
                Some(d) => match parse_json(&d, &format!("GET {path}"))? {
                    Value::Array(a) => a.iter().map(Instance::from_json).collect(),
                    _ => return Err(format!("GET {path}: the answer is not an array")),
                },
                None => Vec::new(),
            };
            instances.insert(name.clone(), es);
        }
        let (part, more) = translate(instances, &self.opts);
        problems.extend(more);
        Ok((part, problems))
    }
}

/// Turns the services' instances into a part, and says why it left out the instances it did.
/// The part is the same for the same instances, so that an unchanged one is seen to be.
pub(crate) fn translate(
    services: BTreeMap<String, Vec<Instance>>,
    opts: &akit_config::ConsulProvider,
) -> (Vec<u8>, Vec<String>) {
    let prefix = format!("{}.", opts.tag_prefix);
    let mut groups: BTreeMap<String, Group> = BTreeMap::new();
    let mut problems = Vec::new();
    for (svc, mut es) in services {
        es.sort_by(|a, b| a.node.cmp(&b.node).then_with(|| a.id.cmp(&b.id)));
        for e in &es {
            let mut problem = |msg: String| {
                problems.push(format!(
                    "service {svc}, instance {} on node {}: {msg}",
                    e.id, e.node
                ));
            };
            let tags = match tag_labels(&e.tags, &prefix) {
                Ok(t) => t,
                Err(err) => {
                    problem(err);
                    continue;
                }
            };
            let enable = tags.get(&format!("{prefix}enable")).map(String::as_str);
            if enable == Some("false") || !opts.expose_all && enable != Some("true") {
                continue;
            }
            let name = tags
                .get(&format!("{prefix}name"))
                .filter(|n| !n.is_empty())
                .cloned()
                .unwrap_or_else(|| svc.clone());
            if !labels::valid_name(&name) {
                problem(format!(
                    "{name:?} is not a valid name for an upstream: tag it {prefix}name=<name>"
                ));
                continue;
            }
            let kind = labels::Kind {
                label: "tag",
                owner: "service",
            };
            let l = match labels::parse(&tags, &prefix, &["enable", "name"], kind) {
                Ok(l) => l,
                Err(err) => {
                    problem(err);
                    continue;
                }
            };
            let host = if e.address.is_empty() {
                &e.node_address
            } else {
                &e.address
            };
            if host.is_empty() {
                problem("it has no address, nor has its node".to_owned());
                continue;
            }
            let port = if l.port == 0 { e.port } else { l.port };
            if port == 0 {
                problem(format!("it has no port: tag it {prefix}port=<port>"));
                continue;
            }
            let g = groups.entry(name).or_insert_with(|| Group {
                set: l.clone(),
                ..Group::default()
            });
            g.addrs.push(join_host_port(host, port));
            let svc_ports: Vec<(String, u16)> = g
                .set
                .services
                .iter()
                .filter(|(_, s)| s.port != 0)
                .map(|(k, s)| (k.clone(), s.port))
                .collect();
            for (key, p) in svc_ports {
                g.svc_addrs
                    .entry(key)
                    .or_default()
                    .push(join_host_port(host, p));
            }
        }
    }
    (labels::part(&groups), problems)
}

/// Go's `net.JoinHostPort`: an IPv6 address in brackets.
fn join_host_port(host: &str, port: u16) -> String {
    if host.contains(':') {
        format!("[{host}]:{port}")
    } else {
        format!("{host}:{port}")
    }
}

/// Reads the tags under `prefix` as labels, `key=value`.
fn tag_labels(tags: &[String], prefix: &str) -> Result<BTreeMap<String, String>, String> {
    let mut m = BTreeMap::new();
    for t in tags {
        if !t.starts_with(prefix) {
            continue;
        }
        let (k, v) = t
            .split_once('=')
            .ok_or_else(|| format!("tag {t}: is not <key>=<value>"))?;
        m.insert(k.to_owned(), v.to_owned());
    }
    Ok(m)
}

#[cfg(test)]
mod tests;
