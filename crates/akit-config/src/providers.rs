//! `providers`, as fit's `config.Provider` and its checks (`checkProviders`): sources of config
//! other than the files and the Admin API (I57's framework), each of one kind. Consul, Consul
//! KV and etcd (I61, [`crate::providers_kv`]) are read; Docker (I58) and Kubernetes (I59, I60)
//! are refused as not supported yet.
//!
//! Read when akit starts: a change takes a restart, as in fit.

use crate::decode::{Decoder, Obj};
use crate::error::join_field;
use crate::json::{Kind, Node};
use crate::providers_kv::{
    ConsulKvProvider, ConsulProvider, EtcdProvider, consul_kv_provider, consul_provider,
    etcd_provider,
};

/// A provider: its name and its kind's settings.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Provider {
    /// `name`: what the provider goes by in the Admin API's `/config/effective` and in the log;
    /// its kind (`consul`) when left out.
    pub name: String,
    /// Its kind, with the kind's settings.
    pub kind: ProviderKind,
}

/// A provider's kind: exactly one is set.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ProviderKind {
    /// `consul`: upstreams, routes and services from the services of a Consul catalog (I61).
    Consul(ConsulProvider),
    /// `consul_kv`: parts from the keys under a prefix in Consul's KV store (I61).
    ConsulKv(ConsulKvProvider),
    /// `etcd`: parts from the keys under a prefix in etcd (I61).
    Etcd(EtcdProvider),
}

impl ProviderKind {
    /// The kind's name, as the config writes it.
    #[must_use]
    pub fn name(&self) -> &'static str {
        match self {
            ProviderKind::Consul(_) => "consul",
            ProviderKind::ConsulKv(_) => "consul_kv",
            ProviderKind::Etcd(_) => "etcd",
        }
    }
}

/// The names of the sources of the base config, which no provider may take.
const BASE_SOURCES: [&str; 4] = ["file", "admin", "rollback", "certificates"];

// I61: the Consul, Consul KV and etcd providers. Docker waits for I58, Kubernetes for I59 and
// I60.
const PROVIDER_FIELDS: &[&str] = &["name", "consul", "consul_kv", "etcd"];
const PROVIDER_FIT_ONLY: &[&str] = &["docker", "kubernetes"];

/// Every kind fit has, in the order its message names them.
const KINDS: [&str; 5] = ["docker", "consul", "consul_kv", "etcd", "kubernetes"];

/// One of `providers`, at `path`. A provider of a kind not supported yet is refused by
/// [`Obj::finish`]; one with no kind, or several, here.
pub(crate) fn decode(d: &mut Decoder<'_>, n: &Node, path: &str) -> Option<Provider> {
    let mut o = d.object(n, path)?;
    let name = o
        .take("name")
        .and_then(|v| d.string(v, &o.at("name")))
        .unwrap_or_default();
    let kinds = KINDS.iter().filter(|k| o.has(k)).count();
    if kinds != 1 {
        d.error(
            o.off,
            path,
            "set exactly one kind of provider: docker, consul, consul_kv, etcd or kubernetes",
        );
    }
    let mut kind = None;
    if let Some(v) = o.take("consul") {
        kind = consul_provider(d, v, &o.at("consul")).map(ProviderKind::Consul);
    }
    if let Some(v) = o.take("consul_kv") {
        kind = consul_kv_provider(d, v, &o.at("consul_kv")).map(ProviderKind::ConsulKv);
    }
    if let Some(v) = o.take("etcd") {
        kind = etcd_provider(d, v, &o.at("etcd")).map(ProviderKind::Etcd);
    }
    o.finish(d, PROVIDER_FIELDS, PROVIDER_FIT_ONLY);
    if kinds != 1 {
        return None;
    }
    let kind = kind?;
    let name = if name.is_empty() {
        kind.name().to_owned()
    } else {
        name
    };
    Some(Provider { name, kind })
}

/// The checks across providers, fit's `checkProviders`: each name valid, not a source of the
/// base config, and unique. `items` are each provider's file, position, path and value.
pub(crate) fn check<'a>(
    ds: &mut [Decoder<'_>],
    items: impl Iterator<Item = (usize, usize, &'a str, &'a Provider)>,
) {
    let mut seen: Vec<&str> = Vec::new();
    for (file, off, path, p) in items {
        let msg = if !valid_provider_name(&p.name) {
            Some(format!(
                "{:?} is not a valid name: use lower-case letters, digits, '_', '.' and '-', starting with a letter or digit, at most 63",
                p.name
            ))
        } else if BASE_SOURCES.contains(&p.name.as_str()) {
            Some(format!(
                "{:?} names a source of the base config: pick another name",
                p.name
            ))
        } else if seen.contains(&p.name.as_str()) {
            Some(format!(
                "another provider is already named {:?}: give one a name",
                p.name
            ))
        } else {
            None
        };
        if let (Some(msg), Some(d)) = (msg, ds.get_mut(file)) {
            d.error(off, &join_field(path, "name"), msg);
        }
        seen.push(&p.name);
    }
}

/// fit's `providerPattern`: `^[a-z0-9][a-z0-9_.-]{0,62}$`.
fn valid_provider_name(s: &str) -> bool {
    let mut b = s.bytes();
    s.len() <= 63
        && b.next()
            .is_some_and(|c| c.is_ascii_lowercase() || c.is_ascii_digit())
        && b.all(|c| {
            c.is_ascii_lowercase() || c.is_ascii_digit() || matches!(c, b'_' | b'.' | b'-')
        })
}

/// fit's `namePattern`, which a label or tag prefix follows.
pub(crate) fn valid_prefix(s: &str) -> bool {
    let mut b = s.bytes();
    b.next().is_some_and(|c| c.is_ascii_alphanumeric())
        && b.all(|c| c.is_ascii_alphanumeric() || matches!(c, b'_' | b'.' | b'-'))
}

/// A boolean member, false when left out.
pub(crate) fn flag(d: &mut Decoder<'_>, o: &mut Obj<'_>, key: &str) -> bool {
    match o.take(key) {
        None => false,
        Some(v) => match &v.kind {
            Kind::Bool(b) => *b,
            other => {
                let msg = format!("expected a boolean, got {}", other.name());
                d.error(v.off, &o.at(key), msg);
                false
            }
        },
    }
}
