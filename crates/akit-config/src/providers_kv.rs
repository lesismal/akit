//! The settings of the providers that follow a service's HTTP API (I61): `providers[].consul`,
//! `providers[].consul_kv` and `providers[].etcd`, as fit's `config.ConsulProvider`,
//! `config.ConsulKVProvider` and `config.EtcdProvider`, with their defaults filled in and fit's
//! checks (`checkProviders`, `checkEndpoint`, `checkTLSFiles`).

use std::time::Duration;

use crate::decode::{Decoder, Obj};
use crate::json::Node;
use crate::model::check_endpoint;
use crate::providers::{flag, valid_prefix};

// I61: every field of the three kinds.
const CONSUL_PROVIDER_FIELDS: &[&str] = &[
    "endpoint",
    "datacenter",
    "token_file",
    "ca_file",
    "cert_file",
    "key_file",
    "expose_all",
    "tag_prefix",
    "max_services",
    "refresh_interval",
    "request_timeout",
];
const CONSUL_KV_PROVIDER_FIELDS: &[&str] = &[
    "endpoint",
    "datacenter",
    "token_file",
    "ca_file",
    "cert_file",
    "key_file",
    "prefix",
    "max_keys",
    "refresh_interval",
    "request_timeout",
];
const ETCD_PROVIDER_FIELDS: &[&str] = &[
    "endpoints",
    "username",
    "password_file",
    "ca_file",
    "cert_file",
    "key_file",
    "prefix",
    "max_keys",
    "refresh_interval",
    "request_timeout",
];

/// fit's default `providers[].consul.endpoint` and `providers[].consul_kv.endpoint`: the local
/// agent.
pub const DEFAULT_CONSUL_ENDPOINT: &str = "http://127.0.0.1:8500";
/// fit's default `providers[].consul.tag_prefix`.
pub const DEFAULT_CONSUL_TAG_PREFIX: &str = "fit";
/// fit's default `providers[].consul.max_services`.
pub const DEFAULT_CONSUL_MAX_SERVICES: usize = 1000;
/// fit's default `providers[].etcd.endpoints`.
pub const DEFAULT_ETCD_ENDPOINT: &str = "http://127.0.0.1:2379";
/// fit's default `prefix` of the Consul KV and etcd providers.
pub const DEFAULT_KV_PREFIX: &str = "fit/";
/// fit's default `max_keys` of the Consul KV and etcd providers.
pub const DEFAULT_KV_MAX_KEYS: usize = 1000;
/// fit's default `refresh_interval` of the providers that watch an API.
pub const DEFAULT_WATCH_REFRESH_INTERVAL: Duration = Duration::from_secs(30);
/// fit's default `request_timeout` of the providers that watch an API.
pub const DEFAULT_WATCH_REQUEST_TIMEOUT: Duration = Duration::from_secs(10);

/// The TLS files of a client of an HTTP API: `ca_file`, `cert_file` and `key_file`, each empty
/// when left out.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ClientTlsFiles {
    /// `ca_file`: the CAs the server's certificate is verified with over https; the system's when
    /// empty.
    pub ca_file: String,
    /// `cert_file`: the client certificate sent over https, with `key_file`.
    pub cert_file: String,
    /// `key_file`: `cert_file`'s private key.
    pub key_file: String,
}

/// How a provider reaches Consul's HTTP API: what `consul` and `consul_kv` share.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConsulConn {
    /// `endpoint`: `http://` or `https://` and a host and port; [`DEFAULT_CONSUL_ENDPOINT`] when
    /// left out.
    pub endpoint: String,
    /// `datacenter`: the datacenter read; the agent's when empty.
    pub datacenter: String,
    /// `token_file`: a file holding the ACL token sent, read each time a watch starts; none when
    /// empty.
    pub token_file: String,
    /// `ca_file`, `cert_file` and `key_file`.
    pub tls: ClientTlsFiles,
    /// `refresh_interval`; [`DEFAULT_WATCH_REFRESH_INTERVAL`] when left out or 0.
    pub refresh_interval: Duration,
    /// `request_timeout`, beyond the five minutes a blocking query waits;
    /// [`DEFAULT_WATCH_REQUEST_TIMEOUT`] when left out or 0.
    pub request_timeout: Duration,
}

/// `providers[].consul`, as fit's `config.ConsulProvider`: upstreams of the services of a
/// Consul catalog, of their instances whose health checks pass, and the routes and services
/// their tags write.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConsulProvider {
    /// Where Consul is, and how it is asked.
    pub conn: ConsulConn,
    /// `expose_all`: take every service, not only those tagged `<tag_prefix>.enable=true`.
    pub expose_all: bool,
    /// `tag_prefix`: what the tags read start with; [`DEFAULT_CONSUL_TAG_PREFIX`] when left out.
    pub tag_prefix: String,
    /// `max_services`: the most services taken; [`DEFAULT_CONSUL_MAX_SERVICES`] when left out
    /// or 0.
    pub max_services: usize,
}

/// `providers[].consul_kv`, as fit's `config.ConsulKVProvider`: parts from the keys under a
/// prefix in Consul's KV store.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConsulKvProvider {
    /// Where Consul is, and how it is asked.
    pub conn: ConsulConn,
    /// `prefix`: the keys read start with it; [`DEFAULT_KV_PREFIX`] when left out or empty.
    pub prefix: String,
    /// `max_keys`: the most keys taken; [`DEFAULT_KV_MAX_KEYS`] when left out or 0.
    pub max_keys: usize,
}

/// `providers[].etcd`, as fit's `config.EtcdProvider`: parts from the keys under a prefix in
/// etcd, through the JSON gateway to its v3 API.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EtcdProvider {
    /// `endpoints`: each member's client API, `http://` or `https://` and a host and port, tried
    /// in turn; [`DEFAULT_ETCD_ENDPOINT`] alone when left out.
    pub endpoints: Vec<String>,
    /// `username`: the user authenticated as, with `password_file`; none when empty.
    pub username: String,
    /// `password_file`: a file holding `username`'s password, read each time a watch starts.
    pub password_file: String,
    /// `ca_file`, `cert_file` and `key_file`.
    pub tls: ClientTlsFiles,
    /// `prefix`: the keys read start with it; [`DEFAULT_KV_PREFIX`] when left out or empty.
    pub prefix: String,
    /// `max_keys`: the most keys taken; [`DEFAULT_KV_MAX_KEYS`] when left out or 0.
    pub max_keys: usize,
    /// `refresh_interval`; [`DEFAULT_WATCH_REFRESH_INTERVAL`] when left out or 0.
    pub refresh_interval: Duration,
    /// `request_timeout`: each request, and how long a watch may take to start;
    /// [`DEFAULT_WATCH_REQUEST_TIMEOUT`] when left out or 0.
    pub request_timeout: Duration,
}

/// A string member with its position.
fn string_at(d: &mut Decoder<'_>, o: &mut Obj<'_>, key: &str) -> Option<(String, usize)> {
    o.take(key)
        .and_then(|v| Some((d.string(v, &o.at(key))?, v.off)))
}

/// A count member, at least 0: `default` when left out or 0.
fn count_at(d: &mut Decoder<'_>, o: &mut Obj<'_>, key: &str, default: usize) -> usize {
    o.take(key)
        .and_then(|v| d.int(v, &o.at(key), 0, i64::MAX))
        .and_then(|n| usize::try_from(n).ok())
        .filter(|n| *n != 0)
        .unwrap_or(default)
}

/// A duration member: `default` when left out or 0.
fn duration_at(d: &mut Decoder<'_>, o: &mut Obj<'_>, key: &str, default: Duration) -> Duration {
    o.take(key)
        .and_then(|v| d.duration(v, &o.at(key)))
        .filter(|t| !t.is_zero())
        .unwrap_or(default)
}

/// An endpoint member, checked by fit's `checkEndpoint`: `default` when left out or empty.
fn endpoint_at(d: &mut Decoder<'_>, o: &mut Obj<'_>, key: &str, default: &str) -> String {
    match string_at(d, o, key) {
        Some((ep, off)) if !ep.is_empty() => {
            if let Err(msg) = check_endpoint(&ep) {
                d.error(off, &o.at(key), msg);
            }
            ep
        }
        _ => default.to_owned(),
    }
}

/// `ca_file`, `cert_file` and `key_file`, with fit's `checkTLSFiles`.
fn client_tls_files(d: &mut Decoder<'_>, o: &mut Obj<'_>) -> ClientTlsFiles {
    let ca_file = string_at(d, o, "ca_file").map(|s| s.0).unwrap_or_default();
    let cert_file = string_at(d, o, "cert_file");
    let key_file = string_at(d, o, "key_file");
    match (&cert_file, &key_file) {
        (Some((c, off)), k) if !c.is_empty() && k.as_ref().is_none_or(|k| k.0.is_empty()) => {
            d.error(*off, &o.at("cert_file"), "cert_file needs key_file");
        }
        (c, Some((k, off))) if !k.is_empty() && c.as_ref().is_none_or(|c| c.0.is_empty()) => {
            d.error(*off, &o.at("key_file"), "key_file needs cert_file");
        }
        _ => {}
    }
    ClientTlsFiles {
        ca_file,
        cert_file: cert_file.map(|c| c.0).unwrap_or_default(),
        key_file: key_file.map(|k| k.0).unwrap_or_default(),
    }
}

/// What `consul` and `consul_kv` share.
fn consul_conn(d: &mut Decoder<'_>, o: &mut Obj<'_>) -> ConsulConn {
    let endpoint = endpoint_at(d, o, "endpoint", DEFAULT_CONSUL_ENDPOINT);
    let datacenter = string_at(d, o, "datacenter")
        .map(|s| s.0)
        .unwrap_or_default();
    let token_file = string_at(d, o, "token_file")
        .map(|s| s.0)
        .unwrap_or_default();
    let tls = client_tls_files(d, o);
    let refresh_interval = duration_at(d, o, "refresh_interval", DEFAULT_WATCH_REFRESH_INTERVAL);
    let request_timeout = duration_at(d, o, "request_timeout", DEFAULT_WATCH_REQUEST_TIMEOUT);
    ConsulConn {
        endpoint,
        datacenter,
        token_file,
        tls,
        refresh_interval,
        request_timeout,
    }
}

/// `providers[].consul`, as fit's `config.ConsulProvider` and its checks.
pub(crate) fn consul_provider(d: &mut Decoder<'_>, n: &Node, path: &str) -> Option<ConsulProvider> {
    let mut o = d.object(n, path)?;
    let conn = consul_conn(d, &mut o);
    let expose_all = flag(d, &mut o, "expose_all");
    let tag_prefix = match string_at(d, &mut o, "tag_prefix") {
        Some((p, off)) if !p.is_empty() => {
            if !valid_prefix(&p) {
                d.error(
                    off,
                    &o.at("tag_prefix"),
                    format!(
                        "{p:?} is not a valid prefix: use letters, digits, '_', '.' and '-', starting with a letter or digit"
                    ),
                );
            }
            p
        }
        _ => DEFAULT_CONSUL_TAG_PREFIX.to_owned(),
    };
    let max_services = count_at(d, &mut o, "max_services", DEFAULT_CONSUL_MAX_SERVICES);
    o.finish(d, CONSUL_PROVIDER_FIELDS, &[]);
    Some(ConsulProvider {
        conn,
        expose_all,
        tag_prefix,
        max_services,
    })
}

/// A KV provider's `prefix`: [`DEFAULT_KV_PREFIX`] when left out or empty.
fn kv_prefix(d: &mut Decoder<'_>, o: &mut Obj<'_>) -> String {
    string_at(d, o, "prefix")
        .map(|s| s.0)
        .filter(|p| !p.is_empty())
        .unwrap_or_else(|| DEFAULT_KV_PREFIX.to_owned())
}

/// `providers[].consul_kv`, as fit's `config.ConsulKVProvider` and its checks.
pub(crate) fn consul_kv_provider(
    d: &mut Decoder<'_>,
    n: &Node,
    path: &str,
) -> Option<ConsulKvProvider> {
    let mut o = d.object(n, path)?;
    let conn = consul_conn(d, &mut o);
    let prefix = kv_prefix(d, &mut o);
    let max_keys = count_at(d, &mut o, "max_keys", DEFAULT_KV_MAX_KEYS);
    o.finish(d, CONSUL_KV_PROVIDER_FIELDS, &[]);
    Some(ConsulKvProvider {
        conn,
        prefix,
        max_keys,
    })
}

/// `providers[].etcd`, as fit's `config.EtcdProvider` and its checks.
pub(crate) fn etcd_provider(d: &mut Decoder<'_>, n: &Node, path: &str) -> Option<EtcdProvider> {
    let mut o = d.object(n, path)?;
    let mut endpoints = Vec::new();
    if let Some(v) = o.take("endpoints") {
        let at = o.at("endpoints");
        for (i, (off, ep)) in d.strings(v, &at).into_iter().enumerate() {
            if let Err(msg) = check_endpoint(&ep) {
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
    let prefix = kv_prefix(d, &mut o);
    let max_keys = count_at(d, &mut o, "max_keys", DEFAULT_KV_MAX_KEYS);
    let refresh_interval = duration_at(
        d,
        &mut o,
        "refresh_interval",
        DEFAULT_WATCH_REFRESH_INTERVAL,
    );
    let request_timeout = duration_at(d, &mut o, "request_timeout", DEFAULT_WATCH_REQUEST_TIMEOUT);
    o.finish(d, ETCD_PROVIDER_FIELDS, &[]);
    Some(EtcdProvider {
        endpoints,
        username: username.map(|u| u.0).unwrap_or_default(),
        password_file: password_file.map(|p| p.0).unwrap_or_default(),
        tls,
        prefix,
        max_keys,
        refresh_interval,
        request_timeout,
    })
}
