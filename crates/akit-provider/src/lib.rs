#![forbid(unsafe_code)]
#![warn(clippy::unwrap_used)]
//! akit's config providers: the framework (I57), and Docker, Kubernetes, Consul and etcd on it
//! (I58–I61).
//!
//! A provider watches a source of config other than the files and the Admin API and sends what
//! it finds there as a [`Part`]: a JSON object with `listeners`, `routes`, `upstreams` and
//! `services`, and nothing else. Parts are merged into the base config, the one the files and
//! the Admin API write, by fit's rules (fit's decision 009, [`merge`]): the base config wins over
//! every provider, and a provider over those after it; the merged config is checked, compiled and
//! applied as a whole, through the path every config takes (`akit_admin::Configs`), and a part
//! that would leave it invalid is refused, the provider's part before it staying. Where each
//! element of the merged config came from is recorded, for the Admin API's
//! `GET /config/effective`.
//!
//! Providers run in the control plane, never blocking an akio I/O thread: their parts reach the
//! control thread through [`Updates`], at most one waiting per provider.
//!
//! Part of [akit](https://github.com/lesismal/akit); see `docs/dev-plan.md` for the iterations
//! that fill this crate in.

mod api;
mod consul;
pub mod docker;
mod etcd;
#[cfg(any(test, feature = "fuzzing"))]
#[doc(hidden)]
pub mod fuzzing;
pub mod kubernetes;
mod kvpart;
mod labels;
mod merge;
mod run;
#[cfg(test)]
mod testing;
mod watch;

pub use consul::{ConsulCatalog, ConsulKv};
pub use etcd::Etcd;
pub use merge::{KINDS, MAX_PART_BYTES, Merged, Part, Shadowed, Source, merge, parse};
pub use run::{Kinds, Make, Provider, Update, Updates};
pub use watch::{Log, stderr_log};

/// The kinds of provider that follow a service's HTTP API (I61): `consul`, `consul_kv` and
/// `etcd`, each reporting to standard error. For [`Kinds::new`].
pub const API_KINDS: &[(&str, Make<akit_config::Provider>)] = &[
    ("consul", make_consul),
    ("consul_kv", make_consul_kv),
    ("etcd", make_etcd),
];

fn make_consul(
    name: &str,
    pc: &akit_config::Provider,
) -> Result<(Box<dyn Provider>, String), String> {
    match &pc.kind {
        akit_config::ProviderKind::Consul(c) => Ok((
            Box::new(ConsulCatalog::new(name, c, stderr_log())),
            ConsulCatalog::watching(c),
        )),
        other => Err(format!(
            "provider {name}: a {} provider is not consul",
            other.name()
        )),
    }
}

fn make_consul_kv(
    name: &str,
    pc: &akit_config::Provider,
) -> Result<(Box<dyn Provider>, String), String> {
    match &pc.kind {
        akit_config::ProviderKind::ConsulKv(c) => Ok((
            Box::new(ConsulKv::new(name, c, stderr_log())),
            ConsulKv::watching(c),
        )),
        other => Err(format!(
            "provider {name}: a {} provider is not consul_kv",
            other.name()
        )),
    }
}

fn make_etcd(
    name: &str,
    pc: &akit_config::Provider,
) -> Result<(Box<dyn Provider>, String), String> {
    match &pc.kind {
        akit_config::ProviderKind::Etcd(e) => Ok((
            Box::new(Etcd::new(name, e, stderr_log())),
            Etcd::watching(e),
        )),
        other => Err(format!(
            "provider {name}: a {} provider is not etcd",
            other.name()
        )),
    }
}
