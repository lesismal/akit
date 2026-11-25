//! `acme.shared_storage` (I62), as fit's `config.SharedStorage` (fit's decision 014): where
//! instances of akit on several hosts keep what ACME obtains, with the locks and challenge
//! answers that let them share it, in Redis, etcd or Consul's KV store; with its defaults filled
//! in and fit's checks (`checkSharedStorage`, `checkRedisStorage`).

use std::time::Duration;

use crate::decode::Decoder;
use crate::json::Node;
use crate::providers::flag;
use crate::providers_kv::{
    ClientTlsFiles, DEFAULT_CONSUL_ENDPOINT, DEFAULT_ETCD_ENDPOINT, client_tls_files, endpoint_at,
    string_at,
};

// I62: every field of the shared storage and its three kinds.
const SHARED_STORAGE_FIELDS: &[&str] = &[
    "redis",
    "etcd",
    "consul",
    "prefix",
    "lock_ttl",
    "request_timeout",
];
const REDIS_STORAGE_FIELDS: &[&str] = &[
    "address",
    "username",
    "password_file",
    "db",
    "tls",
    "ca_file",
    "cert_file",
    "key_file",
];
const ETCD_STORAGE_FIELDS: &[&str] = &[
    "endpoints",
    "username",
    "password_file",
    "ca_file",
    "cert_file",
    "key_file",
];
const CONSUL_STORAGE_FIELDS: &[&str] = &[
    "endpoint",
    "datacenter",
    "token_file",
    "ca_file",
    "cert_file",
    "key_file",
];

/// fit's default `acme.shared_storage.prefix`.
pub const DEFAULT_SHARED_STORAGE_PREFIX: &str = "fit/";
/// fit's default `acme.shared_storage.lock_ttl`.
pub const DEFAULT_SHARED_STORAGE_LOCK_TTL: Duration = Duration::from_secs(30);
/// The shortest `acme.shared_storage.lock_ttl`: what Consul's sessions need, and more than a
/// renewal takes.
pub const MIN_SHARED_STORAGE_LOCK_TTL: Duration = Duration::from_secs(10);
/// fit's default `acme.shared_storage.request_timeout`.
pub const DEFAULT_SHARED_STORAGE_TIMEOUT: Duration = Duration::from_secs(10);
/// fit's default `acme.shared_storage.redis.address`.
pub const DEFAULT_REDIS_ADDRESS: &str = "127.0.0.1:6379";

/// `acme.shared_storage`, as fit's `config.SharedStorage`: one kind of store, and what every
/// kind shares.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SharedStorage {
    /// `redis`, `etcd` or `consul`: exactly one is set.
    pub kind: SharedStorageKind,
    /// `prefix`: what the keys kept start with, as `fit/certificates/<CA>/<name>.pem`; instances
    /// that share certificates share it. [`DEFAULT_SHARED_STORAGE_PREFIX`] when left out or
    /// empty.
    pub prefix: String,
    /// `lock_ttl`: how long a lock outlives an instance that stopped holding it without giving
    /// it back; its holder renews it every third of that. At least
    /// [`MIN_SHARED_STORAGE_LOCK_TTL`]; [`DEFAULT_SHARED_STORAGE_LOCK_TTL`] when left out or 0.
    pub lock_ttl: Duration,
    /// `request_timeout`: how long each request to the store may take, connecting included;
    /// [`DEFAULT_SHARED_STORAGE_TIMEOUT`] when left out or 0.
    pub request_timeout: Duration,
}

/// The kind of a [`SharedStorage`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SharedStorageKind {
    /// `redis`: a standalone Redis server or a primary (not Redis Cluster).
    Redis(RedisStorage),
    /// `etcd`: an etcd cluster, through its v3 JSON gateway.
    Etcd(EtcdStorage),
    /// `consul`: Consul's KV store.
    Consul(ConsulStorage),
}

impl SharedStorageKind {
    /// fit's name of the kind.
    #[must_use]
    pub fn name(&self) -> &'static str {
        match self {
            SharedStorageKind::Redis(_) => "redis",
            SharedStorageKind::Etcd(_) => "etcd",
            SharedStorageKind::Consul(_) => "consul",
        }
    }
}

/// `acme.shared_storage.redis`, as fit's `config.RedisStorage`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RedisStorage {
    /// `address`: the server's host and port; [`DEFAULT_REDIS_ADDRESS`] when left out or empty.
    pub address: String,
    /// `username`: the ACL user authenticated as, with `password_file`; the default user when
    /// empty.
    pub username: String,
    /// `password_file`: a file holding the password, read at each connection; none when empty.
    pub password_file: String,
    /// `db`: the database selected; 0 when left out.
    pub db: u32,
    /// `tls`: connect over TLS.
    pub tls: bool,
    /// `ca_file`, `cert_file` and `key_file`, over TLS.
    pub tls_files: ClientTlsFiles,
}

/// `acme.shared_storage.etcd`, as fit's `config.EtcdStorage`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EtcdStorage {
    /// `endpoints`: each member's client API, `http://` or `https://` and a host and port,
    /// tried in turn when one fails; [`DEFAULT_ETCD_ENDPOINT`] alone when left out.
    pub endpoints: Vec<String>,
    /// `username`: the user authenticated as, with `password_file`; none when empty.
    pub username: String,
    /// `password_file`: a file holding `username`'s password, read each time akit
    /// authenticates.
    pub password_file: String,
    /// `ca_file`, `cert_file` and `key_file`, over https.
    pub tls: ClientTlsFiles,
}

/// `acme.shared_storage.consul`, as fit's `config.ConsulStorage`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConsulStorage {
    /// `endpoint`: Consul's HTTP API, as of the local agent, `http://` or `https://` and a host
    /// and port; [`DEFAULT_CONSUL_ENDPOINT`] when left out or empty.
    pub endpoint: String,
    /// `datacenter`: the datacenter whose KV store is used; the agent's when empty.
    pub datacenter: String,
    /// `token_file`: a file holding the ACL token sent, read when the store is made; none when
    /// empty.
    pub token_file: String,
    /// `ca_file`, `cert_file` and `key_file`, over https.
    pub tls: ClientTlsFiles,
}

/// Whether `s` is a host and port as Go's `net.SplitHostPort` takes one apart: a bracketed IPv6
/// host, or a host without a colon, then `:` and a port (which may be empty).
fn host_and_port(s: &str) -> bool {
    match s.strip_prefix('[') {
        Some(r) => r.split_once(']').is_some_and(|(h, p)| {
            !h.contains(']') && p.strip_prefix(':').is_some_and(|p| !p.contains(':'))
        }),
        None => s.rsplit_once(':').is_some_and(|(h, _)| !h.contains(':')),
    }
}

/// `acme.shared_storage`, as fit's `config.SharedStorage` and `checkSharedStorage`; `None`
/// when it does not set exactly one kind (an error).
pub(crate) fn shared_storage(d: &mut Decoder<'_>, n: &Node, path: &str) -> Option<SharedStorage> {
    let mut o = d.object(n, path)?;
    let mut kinds = Vec::new();
    if let Some(v) = o.take("redis") {
        kinds.extend(redis(d, v, &o.at("redis")).map(SharedStorageKind::Redis));
    }
    if let Some(v) = o.take("etcd") {
        kinds.extend(etcd(d, v, &o.at("etcd")).map(SharedStorageKind::Etcd));
    }
    if let Some(v) = o.take("consul") {
        kinds.extend(consul(d, v, &o.at("consul")).map(SharedStorageKind::Consul));
    }
    let prefix = string_at(d, &mut o, "prefix")
        .map(|s| s.0)
        .filter(|p| !p.is_empty())
        .unwrap_or_else(|| DEFAULT_SHARED_STORAGE_PREFIX.to_owned());
    let mut lock_ttl = DEFAULT_SHARED_STORAGE_LOCK_TTL;
    if let Some(v) = o.take("lock_ttl") {
        if let Some(t) = d.duration(v, &o.at("lock_ttl")).filter(|t| !t.is_zero()) {
            if t < MIN_SHARED_STORAGE_LOCK_TTL {
                d.error(
                    v.off,
                    &o.at("lock_ttl"),
                    format!(
                        "{} is too short: at least 10s, which Consul's sessions need, and more than a renewal takes",
                        crate::duration::format(t)
                    ),
                );
            }
            lock_ttl = t;
        }
    }
    let request_timeout = o
        .take("request_timeout")
        .and_then(|v| d.duration(v, &o.at("request_timeout")))
        .filter(|t| !t.is_zero())
        .unwrap_or(DEFAULT_SHARED_STORAGE_TIMEOUT);
    let set = ["redis", "etcd", "consul"]
        .iter()
        .filter(|k| o.has(k))
        .count();
    o.finish(d, SHARED_STORAGE_FIELDS, &[]);
    if set != 1 {
        d.error(
            n.off,
            path,
            "set exactly one kind of shared storage: redis, etcd or consul",
        );
        return None;
    }
    Some(SharedStorage {
        kind: kinds.pop()?,
        prefix,
        lock_ttl,
        request_timeout,
    })
}

/// `acme.shared_storage.redis`, as fit's `checkRedisStorage`.
fn redis(d: &mut Decoder<'_>, n: &Node, path: &str) -> Option<RedisStorage> {
    let mut o = d.object(n, path)?;
    let address = match string_at(d, &mut o, "address") {
        Some((a, off)) if !a.is_empty() => {
            if !host_and_port(&a) {
                d.error(
                    off,
                    &o.at("address"),
                    format!("{a:?} is not a host and port"),
                );
            }
            a
        }
        _ => DEFAULT_REDIS_ADDRESS.to_owned(),
    };
    let username = string_at(d, &mut o, "username");
    let password_file = string_at(d, &mut o, "password_file")
        .map(|s| s.0)
        .unwrap_or_default();
    if let Some((u, off)) = &username {
        if !u.is_empty() && password_file.is_empty() {
            d.error(*off, &o.at("username"), "username needs password_file");
        }
    }
    let db = o
        .take("db")
        .and_then(|v| d.int(v, &o.at("db"), 0, i64::from(u32::MAX)))
        .and_then(|n| u32::try_from(n).ok())
        .unwrap_or(0);
    let tls = flag(d, &mut o, "tls");
    let ca = string_at(d, &mut o, "ca_file");
    let cert = string_at(d, &mut o, "cert_file");
    let key = string_at(d, &mut o, "key_file");
    let set = |f: &Option<(String, usize)>| f.as_ref().filter(|f| !f.0.is_empty()).map(|f| f.1);
    if !tls {
        if let Some(off) = set(&ca) {
            d.error(off, &o.at("ca_file"), "ca_file needs tls");
        }
        if let Some(off) = set(&cert) {
            d.error(off, &o.at("cert_file"), "cert_file needs tls");
        }
    }
    match (set(&cert), set(&key)) {
        (Some(off), None) => d.error(off, &o.at("cert_file"), "cert_file needs key_file"),
        (None, Some(off)) => d.error(off, &o.at("key_file"), "key_file needs cert_file"),
        _ => {}
    }
    let file = |f: Option<(String, usize)>| f.map(|f| f.0).unwrap_or_default();
    let tls_files = ClientTlsFiles {
        ca_file: file(ca),
        cert_file: file(cert),
        key_file: file(key),
    };
    o.finish(d, REDIS_STORAGE_FIELDS, &[]);
    Some(RedisStorage {
        address,
        username: username.map(|u| u.0).unwrap_or_default(),
        password_file,
        db,
        tls,
        tls_files,
    })
}

/// `acme.shared_storage.etcd`, with fit's checks.
fn etcd(d: &mut Decoder<'_>, n: &Node, path: &str) -> Option<EtcdStorage> {
    let mut o = d.object(n, path)?;
    let mut endpoints = Vec::new();
    if let Some(v) = o.take("endpoints") {
        let at = o.at("endpoints");
        for (i, (off, ep)) in d.strings(v, &at).into_iter().enumerate() {
            if let Err(msg) = crate::model::check_endpoint(&ep) {
                d.error(off, &format!("{at}[{i}]"), msg);
            }
            if !ep.is_empty() {
                endpoints.push(ep);
            }
        }
    }
    if endpoints.is_empty() {
        endpoints.push(DEFAULT_ETCD_ENDPOINT.to_owned());
    }
    let username = string_at(d, &mut o, "username");
    let password_file = string_at(d, &mut o, "password_file");
    let tls = client_tls_files(d, &mut o);
    match (&username, &password_file) {
        (Some((u, off)), p) if !u.is_empty() && p.as_ref().is_none_or(|p| p.0.is_empty()) => {
            d.error(*off, &o.at("username"), "username needs password_file");
        }
        (u, Some((p, off))) if !p.is_empty() && u.as_ref().is_none_or(|u| u.0.is_empty()) => {
            d.error(*off, &o.at("password_file"), "password_file needs username");
        }
        _ => {}
    }
    o.finish(d, ETCD_STORAGE_FIELDS, &[]);
    Some(EtcdStorage {
        endpoints,
        username: username.map(|u| u.0).unwrap_or_default(),
        password_file: password_file.map(|p| p.0).unwrap_or_default(),
        tls,
    })
}

/// `acme.shared_storage.consul`, with fit's checks.
fn consul(d: &mut Decoder<'_>, n: &Node, path: &str) -> Option<ConsulStorage> {
    let mut o = d.object(n, path)?;
    let endpoint = endpoint_at(d, &mut o, "endpoint", DEFAULT_CONSUL_ENDPOINT);
    let datacenter = string_at(d, &mut o, "datacenter")
        .map(|s| s.0)
        .unwrap_or_default();
    let token_file = string_at(d, &mut o, "token_file")
        .map(|s| s.0)
        .unwrap_or_default();
    let tls = client_tls_files(d, &mut o);
    o.finish(d, CONSUL_STORAGE_FIELDS, &[]);
    Some(ConsulStorage {
        endpoint,
        datacenter,
        token_file,
        tls,
    })
}

#[cfg(test)]
mod tests {
    use super::host_and_port;

    #[test]
    fn hosts_and_ports_as_go() {
        for ok in ["a:1", "127.0.0.1:6379", "[::1]:6379", "a:", ":1"] {
            assert!(host_and_port(ok), "{ok}");
        }
        for bad in ["nohost", "::1:6379", "[::1]", "[::1]:1:2", "["] {
            assert!(!host_and_port(bad), "{bad}");
        }
    }
}
