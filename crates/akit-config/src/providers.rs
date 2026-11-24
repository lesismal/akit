//! `providers`, as fit's `config.Provider` and its checks (`checkProviders`): sources of config
//! other than the files and the Admin API (I57's framework), each of one kind. Docker (I58) is
//! read; Consul, Consul KV, etcd and Kubernetes (I59–I61) are refused as not supported yet.
//!
//! Read when akit starts: a change takes a restart, as in fit.

use std::path::PathBuf;
use std::time::Duration;

use crate::decode::{Decoder, Obj};
use crate::error::join_field;
use crate::json::{Kind, Node};

/// A provider: its name and its kind's settings.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Provider {
    /// `name`: what the provider goes by in the Admin API's `/config/effective` and in the log;
    /// its kind (`docker`) when left out.
    pub name: String,
    /// Its kind, with the kind's settings.
    pub kind: ProviderKind,
}

/// A provider's kind: exactly one is set.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ProviderKind {
    /// `docker`: routes, upstreams and services from the labels of running Docker containers.
    Docker(DockerProvider),
}

impl ProviderKind {
    /// The kind's name, as the config writes it.
    #[must_use]
    pub fn name(&self) -> &'static str {
        match self {
            ProviderKind::Docker(_) => "docker",
        }
    }
}

/// Where the Docker daemon's API is.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DockerEndpoint {
    /// `unix://<path>`: a Unix-domain socket.
    Unix(PathBuf),
    /// `tcp://<host>:<port>`: plain HTTP over TCP, as to a socket proxy.
    Tcp(String),
}

/// `providers[].docker`, as fit's `config.DockerProvider`, with its defaults filled in.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DockerProvider {
    /// `endpoint`: [`DEFAULT_DOCKER_ENDPOINT`] when left out.
    pub endpoint: DockerEndpoint,
    /// `expose_all`: take every running container with routes or services in its labels, not
    /// only those labelled `<label_prefix>.enable=true`.
    pub expose_all: bool,
    /// `network`: the network whose address of a container's is used, when it is on several
    /// and has no `<label_prefix>.network` label; the first by name when empty.
    pub network: String,
    /// `use_published_ports`: send to the port a container's port is published on, on the host.
    pub use_published_ports: bool,
    /// `label_prefix`: what the labels read start with; [`DEFAULT_DOCKER_LABEL_PREFIX`] when left
    /// out.
    pub label_prefix: String,
    /// `max_containers`: the most containers taken; [`DEFAULT_DOCKER_MAX_CONTAINERS`] when left
    /// out or 0.
    pub max_containers: usize,
    /// `refresh_interval`: how often the containers are listed again whether or not an event
    /// came; [`DEFAULT_DOCKER_REFRESH_INTERVAL`] when left out or 0.
    pub refresh_interval: Duration,
    /// `request_timeout`: how long listing the containers may take, connecting included;
    /// [`DEFAULT_DOCKER_REQUEST_TIMEOUT`] when left out or 0.
    pub request_timeout: Duration,
}

/// fit's default `providers[].docker.endpoint`.
pub const DEFAULT_DOCKER_ENDPOINT: &str = "unix:///var/run/docker.sock";
/// fit's default `providers[].docker.label_prefix`.
pub const DEFAULT_DOCKER_LABEL_PREFIX: &str = "fit";
/// fit's default `providers[].docker.max_containers`.
pub const DEFAULT_DOCKER_MAX_CONTAINERS: usize = 1000;
/// fit's default `providers[].docker.refresh_interval`.
pub const DEFAULT_DOCKER_REFRESH_INTERVAL: Duration = Duration::from_secs(30);
/// fit's default `providers[].docker.request_timeout`.
pub const DEFAULT_DOCKER_REQUEST_TIMEOUT: Duration = Duration::from_secs(10);

/// The names of the sources of the base config, which no provider may take.
const BASE_SOURCES: [&str; 4] = ["file", "admin", "rollback", "certificates"];

// I58: the Docker provider, every field. Consul, Consul KV and etcd wait for I61, Kubernetes for
// I59 and I60.
const PROVIDER_FIELDS: &[&str] = &["name", "docker"];
const PROVIDER_FIT_ONLY: &[&str] = &["consul", "consul_kv", "etcd", "kubernetes"];
const DOCKER_FIELDS: &[&str] = &[
    "endpoint",
    "expose_all",
    "network",
    "use_published_ports",
    "label_prefix",
    "max_containers",
    "refresh_interval",
    "request_timeout",
];

/// One of `providers`, at `path`. A provider of a kind not supported yet is refused by
/// [`Obj::finish`]; one with no kind at all here.
pub(crate) fn decode(d: &mut Decoder<'_>, n: &Node, path: &str) -> Option<Provider> {
    let mut o = d.object(n, path)?;
    let name = o
        .take("name")
        .and_then(|v| d.string(v, &o.at("name")))
        .unwrap_or_default();
    let kinds = PROVIDER_FIT_ONLY.iter().filter(|k| o.has(k)).count();
    let docker = o.take("docker");
    let kind = match docker {
        Some(v) => docker_provider(d, v, &o.at("docker")).map(ProviderKind::Docker),
        None => {
            if kinds == 0 {
                d.error(
                    o.off,
                    path,
                    "set exactly one kind of provider: docker, consul, consul_kv, etcd or kubernetes",
                );
            }
            None
        }
    };
    if docker.is_some() && kinds > 0 {
        d.error(
            o.off,
            path,
            "set exactly one kind of provider: docker, consul, consul_kv, etcd or kubernetes",
        );
    }
    o.finish(d, PROVIDER_FIELDS, PROVIDER_FIT_ONLY);
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

/// fit's `namePattern`, which a label prefix follows.
fn valid_prefix(s: &str) -> bool {
    let mut b = s.bytes();
    b.next().is_some_and(|c| c.is_ascii_alphanumeric())
        && b.all(|c| c.is_ascii_alphanumeric() || matches!(c, b'_' | b'.' | b'-'))
}

/// Where `ep` says the daemon is, as fit's `DockerProvider.Dial` and its checks read it.
fn endpoint(ep: &str) -> Result<DockerEndpoint, String> {
    if let Some(path) = ep.strip_prefix("unix://") {
        if path.is_empty() {
            return Err(format!("{ep:?} has no path"));
        }
        return Ok(DockerEndpoint::Unix(PathBuf::from(path)));
    }
    if let Some(hp) = ep.strip_prefix("tcp://") {
        // Go's net.SplitHostPort: a host (bracketed when it has colons) and a port.
        let ok = match hp.strip_prefix('[') {
            Some(rest) => rest
                .split_once(']')
                .and_then(|(_, p)| p.strip_prefix(':'))
                .is_some_and(|port| !port.contains(':')),
            None => hp.rsplit_once(':').is_some_and(|(h, _)| !h.contains(':')),
        };
        if !ok || hp.contains(['/', '?', '#']) {
            return Err(format!("{ep:?} has no host and port"));
        }
        return Ok(DockerEndpoint::Tcp(hp.to_owned()));
    }
    Err(format!(
        "{ep:?} is neither unix://<path> nor tcp://<host>:<port>"
    ))
}

fn default_endpoint() -> DockerEndpoint {
    let path = DEFAULT_DOCKER_ENDPOINT
        .strip_prefix("unix://")
        .unwrap_or(DEFAULT_DOCKER_ENDPOINT);
    DockerEndpoint::Unix(PathBuf::from(path))
}

fn flag(d: &mut Decoder<'_>, o: &mut Obj<'_>, key: &str) -> bool {
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

fn docker_provider(d: &mut Decoder<'_>, n: &Node, path: &str) -> Option<DockerProvider> {
    let mut o = d.object(n, path)?;
    let endpoint = match o.take("endpoint") {
        Some(v) => {
            let at = o.at("endpoint");
            let s = d.string(v, &at)?;
            if s.is_empty() {
                default_endpoint()
            } else {
                match endpoint(&s) {
                    Ok(e) => e,
                    Err(msg) => {
                        d.error(v.off, &at, msg);
                        DockerEndpoint::Unix(PathBuf::new())
                    }
                }
            }
        }
        None => default_endpoint(),
    };
    let expose_all = flag(d, &mut o, "expose_all");
    let use_published_ports = flag(d, &mut o, "use_published_ports");
    let network = o
        .take("network")
        .and_then(|v| d.string(v, &o.at("network")))
        .unwrap_or_default();
    let label_prefix = match o.take("label_prefix") {
        Some(v) => {
            let at = o.at("label_prefix");
            let s = d.string(v, &at).unwrap_or_default();
            if !s.is_empty() && !valid_prefix(&s) {
                d.error(
                    v.off,
                    &at,
                    format!(
                        "{s:?} is not a valid prefix: use letters, digits, '_', '.' and '-', starting with a letter or digit"
                    ),
                );
            }
            s
        }
        None => String::new(),
    };
    let max_containers = o
        .take("max_containers")
        .and_then(|v| d.int(v, &o.at("max_containers"), 0, i64::MAX))
        .and_then(|n| usize::try_from(n).ok())
        .filter(|n| *n != 0)
        .unwrap_or(DEFAULT_DOCKER_MAX_CONTAINERS);
    let refresh_interval = o
        .take("refresh_interval")
        .and_then(|v| d.duration(v, &o.at("refresh_interval")))
        .filter(|t| !t.is_zero())
        .unwrap_or(DEFAULT_DOCKER_REFRESH_INTERVAL);
    let request_timeout = o
        .take("request_timeout")
        .and_then(|v| d.duration(v, &o.at("request_timeout")))
        .filter(|t| !t.is_zero())
        .unwrap_or(DEFAULT_DOCKER_REQUEST_TIMEOUT);
    o.finish(d, DOCKER_FIELDS, &[]);
    Some(DockerProvider {
        endpoint,
        expose_all,
        network,
        use_published_ports,
        label_prefix: if label_prefix.is_empty() {
            DEFAULT_DOCKER_LABEL_PREFIX.to_owned()
        } else {
            label_prefix
        },
        max_containers,
        refresh_interval,
        request_timeout,
    })
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::panic, clippy::indexing_slicing)]

    use super::{DockerEndpoint, endpoint, valid_provider_name};

    /// fit's `TestDockerProviderDial` and the endpoint checks of `checkProviders`.
    #[test]
    fn endpoints() {
        assert_eq!(
            endpoint("unix:///var/run/docker.sock"),
            Ok(DockerEndpoint::Unix("/var/run/docker.sock".into()))
        );
        assert_eq!(
            endpoint("tcp://proxy:2375"),
            Ok(DockerEndpoint::Tcp("proxy:2375".into()))
        );
        assert_eq!(
            endpoint("tcp://[::1]:2375"),
            Ok(DockerEndpoint::Tcp("[::1]:2375".into()))
        );
        for bad in [
            "unix://",
            "tcp://proxy",
            "http://x:1",
            "/var/run/docker.sock",
        ] {
            assert!(endpoint(bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn provider_names() {
        for ok in ["docker", "d2", "a.b-c_d", &"a".repeat(63)] {
            assert!(valid_provider_name(ok), "{ok}");
        }
        for bad in ["", "Docker", "-a", "a b", &"a".repeat(64)] {
            assert!(!valid_provider_name(bad), "{bad}");
        }
    }
}
