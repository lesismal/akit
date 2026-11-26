//! `providers`, as fit's `config.Provider` and its checks (`checkProviders`): sources of config
//! other than the files and the Admin API (I57's framework), each of one kind. Docker (I58),
//! Kubernetes' Gateway API (I60) and Ingresses (I59), and Consul, Consul KV and etcd (I61, their
//! settings in [`crate::providers_kv`]) are read. A kind is a variant of [`ProviderKind`], an arm
//! of [`decode`] and its own field lists.
//!
//! Read when akit starts: a change takes a restart, as in fit.

use std::net::IpAddr;
use std::path::PathBuf;
use std::time::Duration;

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
    /// `kubernetes`: the Gateway API's Gateways and routes (I60), and Ingresses (I59).
    Kubernetes(KubernetesProvider),
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
            ProviderKind::Docker(_) => "docker",
            ProviderKind::Kubernetes(_) => "kubernetes",
            ProviderKind::Consul(_) => "consul",
            ProviderKind::ConsulKv(_) => "consul_kv",
            ProviderKind::Etcd(_) => "etcd",
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
/// The default `providers[].docker.label_prefix`: fit's `fit`, renamed (decision 037).
pub const DEFAULT_DOCKER_LABEL_PREFIX: &str = "akit";
/// fit's default `providers[].docker.max_containers`.
pub const DEFAULT_DOCKER_MAX_CONTAINERS: usize = 1000;
/// fit's default `providers[].docker.refresh_interval`.
pub const DEFAULT_DOCKER_REFRESH_INTERVAL: Duration = Duration::from_secs(30);
/// fit's default `providers[].docker.request_timeout`.
pub const DEFAULT_DOCKER_REQUEST_TIMEOUT: Duration = Duration::from_secs(10);

/// The default `providers[].kubernetes.controller_name`: fit's `lesismal.github.io/akit`, renamed
/// (decision 037).
pub const DEFAULT_KUBERNETES_CONTROLLER_NAME: &str = "lesismal.github.io/akit";
/// fit's default `providers[].kubernetes.max_objects`: of each kind.
pub const DEFAULT_KUBERNETES_MAX_OBJECTS: usize = 10_000;
/// fit's default `providers[].kubernetes.request_timeout`.
pub const DEFAULT_KUBERNETES_REQUEST_TIMEOUT: Duration = Duration::from_secs(10);

/// Where requests to a Service go (`providers[].kubernetes.backends`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum KubernetesBackends {
    /// `endpoints`, the default: its ready endpoints, from its EndpointSlices, balanced by akit.
    #[default]
    Endpoints,
    /// `cluster_ip`: its cluster IP, balanced by the cluster.
    ClusterIp,
}

/// `providers[].kubernetes`, as fit's `config.KubernetesProvider`, with its defaults filled in:
/// the Gateway API's Gateways of the GatewayClasses of `controller_name` and their routes (I60),
/// whose status akit writes back, and with `ingress` Ingresses too (I59).
/// `allow_external_name_services` waits for the upstreams' `deny_internal`, and is refused as not
/// supported yet.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct KubernetesProvider {
    /// `kubeconfig`: the kubeconfig file the cluster is reached with; when empty, the pod's
    /// service account inside a cluster, else `$KUBECONFIG` or `~/.kube/config`.
    pub kubeconfig: String,
    /// `context`: the kubeconfig's context; its `current-context` when empty.
    pub context: String,
    /// `controller_name`: the controller of the GatewayClasses whose Gateways akit serves, and
    /// of the IngressClasses whose Ingresses it serves; [`DEFAULT_KUBERNETES_CONTROLLER_NAME`]
    /// when left out.
    pub controller_name: String,
    /// `namespaces`: the namespaces whose Gateways, routes and Ingresses are served; every one
    /// when empty.
    pub namespaces: Vec<String>,
    /// `listen_host`: the address the listeners are bound on; every address when empty.
    pub listen_host: String,
    /// `addresses`: the addresses, IP addresses or host names, written into each Gateway's
    /// `status.addresses`; none are written when empty, and those there are kept.
    pub addresses: Vec<String>,
    /// `backends`: where requests to a Service go.
    pub backends: KubernetesBackends,
    /// `max_objects`: the most objects of each kind kept; [`DEFAULT_KUBERNETES_MAX_OBJECTS`]
    /// when left out or 0.
    pub max_objects: usize,
    /// `request_timeout`: how long each request to the API server may take, but watches;
    /// [`DEFAULT_KUBERNETES_REQUEST_TIMEOUT`] when left out or 0.
    pub request_timeout: Duration,
    /// `ingress`: serve Ingresses too, on these ports.
    pub ingress: Option<KubernetesIngress>,
    /// `certificates_dir`: where the certificates of the Secrets HTTPS listeners and Ingresses
    /// name are written;
    /// a directory of akit's own in the system's temporary directory, removed when akit stops,
    /// when empty.
    pub certificates_dir: String,
}

/// `providers[].kubernetes.ingress`: the ports Ingresses are served on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct KubernetesIngress {
    /// `http_port`: 80 when left out or 0.
    pub http_port: u16,
    /// `https_port`: 443 when left out or 0.
    pub https_port: u16,
}

/// The names of the sources of the base config, which no provider may take.
const BASE_SOURCES: [&str; 4] = ["file", "admin", "rollback", "certificates"];

// I58: the Docker provider, every field; I59: Kubernetes' Ingresses; I61: Consul, Consul KV and
// etcd.
const PROVIDER_FIELDS: &[&str] = &[
    "name",
    "docker",
    "kubernetes",
    "consul",
    "consul_kv",
    "etcd",
];
const PROVIDER_FIT_ONLY: &[&str] = &[];
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

// I59: Ingresses; I60: the Gateway API, addresses its Gateways' status.
// allow_external_name_services needs the upstreams' deny_internal.
const KUBERNETES_FIELDS: &[&str] = &[
    "kubeconfig",
    "context",
    "controller_name",
    "namespaces",
    "listen_host",
    "addresses",
    "backends",
    "max_objects",
    "request_timeout",
    "ingress",
    "certificates_dir",
];
const KUBERNETES_FIT_ONLY: &[&str] = &["allow_external_name_services"];
const KUBERNETES_INGRESS_FIELDS: &[&str] = &["http_port", "https_port"];

/// One of `providers`, at `path`. A provider of a kind not supported yet is refused by
/// [`Obj::finish`]; one with no kind at all here.
pub(crate) fn decode(d: &mut Decoder<'_>, n: &Node, path: &str) -> Option<Provider> {
    let mut o = d.object(n, path)?;
    let name = o
        .take("name")
        .and_then(|v| d.string(v, &o.at("name")))
        .unwrap_or_default();
    // Those not supported yet are refused by finish; each kind read has its arm here.
    let mut given = PROVIDER_FIT_ONLY.iter().filter(|k| o.has(k)).count();
    let mut kinds: Vec<ProviderKind> = Vec::new();
    if let Some(v) = o.take("docker") {
        given = given.saturating_add(1);
        kinds.extend(docker_provider(d, v, &o.at("docker")).map(ProviderKind::Docker));
    }
    if let Some(v) = o.take("kubernetes") {
        given = given.saturating_add(1);
        kinds.extend(kubernetes(d, v, &o.at("kubernetes")).map(ProviderKind::Kubernetes));
    }
    if let Some(v) = o.take("consul") {
        given = given.saturating_add(1);
        kinds.extend(consul_provider(d, v, &o.at("consul")).map(ProviderKind::Consul));
    }
    if let Some(v) = o.take("consul_kv") {
        given = given.saturating_add(1);
        kinds.extend(consul_kv_provider(d, v, &o.at("consul_kv")).map(ProviderKind::ConsulKv));
    }
    if let Some(v) = o.take("etcd") {
        given = given.saturating_add(1);
        kinds.extend(etcd_provider(d, v, &o.at("etcd")).map(ProviderKind::Etcd));
    }
    if given != 1 {
        d.error(
            o.off,
            path,
            "set exactly one kind of provider: docker, consul, consul_kv, etcd or kubernetes",
        );
    }
    o.finish(d, PROVIDER_FIELDS, PROVIDER_FIT_ONLY);
    let kind = kinds.pop().filter(|_| given == 1)?;
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
pub(crate) fn valid_prefix(s: &str) -> bool {
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

fn kubernetes(d: &mut Decoder<'_>, n: &Node, path: &str) -> Option<KubernetesProvider> {
    let mut o = d.object(n, path)?;
    let string = |d: &mut Decoder<'_>, o: &mut Obj<'_>, key: &str| {
        o.take(key)
            .and_then(|v| Some((d.string(v, &o.at(key))?, v.off)))
    };
    let kubeconfig = string(d, &mut o, "kubeconfig").map(|s| s.0);
    let context = string(d, &mut o, "context").map(|s| s.0);
    let controller_name = string(d, &mut o, "controller_name");
    if let Some((c, off)) = &controller_name {
        if !c.is_empty() && !valid_controller(c) {
            d.error(
                *off,
                &o.at("controller_name"),
                format!("{c:?} is not a domain-prefixed path, as example.net/gateway-controller"),
            );
        }
    }
    let namespaces = o
        .take("namespaces")
        .map(|v| d.strings(v, &o.at("namespaces")))
        .unwrap_or_default();
    let listen_host = string(d, &mut o, "listen_host");
    if let Some((h, off)) = &listen_host {
        if !h.is_empty() && h != "localhost" && h.parse::<IpAddr>().is_err() {
            d.error(
                *off,
                &o.at("listen_host"),
                format!("{h:?} is not an IP address or localhost"),
            );
        }
    }
    let addresses = o
        .take("addresses")
        .map(|v| {
            let at = o.at("addresses");
            let mut out = Vec::new();
            for (i, (off, a)) in d.strings(v, &at).into_iter().enumerate() {
                if a.parse::<IpAddr>().is_err()
                    && (a.starts_with('*') || crate::model::check_host(&a).is_err())
                {
                    d.error(
                        off,
                        &crate::error::join_index(&at, i),
                        format!("{a:?} is not an IP address or host name"),
                    );
                }
                out.push(a);
            }
            out
        })
        .unwrap_or_default();
    let backends = match string(d, &mut o, "backends") {
        None => KubernetesBackends::Endpoints,
        Some((b, off)) => match b.as_str() {
            "" | "endpoints" => KubernetesBackends::Endpoints,
            "cluster_ip" => KubernetesBackends::ClusterIp,
            _ => {
                d.error(
                    off,
                    &o.at("backends"),
                    format!("{b:?} is not one of endpoints, cluster_ip"),
                );
                KubernetesBackends::Endpoints
            }
        },
    };
    let max_objects = o
        .take("max_objects")
        .and_then(|v| d.int(v, &o.at("max_objects"), 0, i64::from(i32::MAX)))
        .and_then(|n| usize::try_from(n).ok())
        .filter(|n| *n != 0)
        .unwrap_or(DEFAULT_KUBERNETES_MAX_OBJECTS);
    let request_timeout = o
        .take("request_timeout")
        .and_then(|v| d.duration(v, &o.at("request_timeout")))
        .filter(|t| !t.is_zero())
        .unwrap_or(DEFAULT_KUBERNETES_REQUEST_TIMEOUT);
    let ingress = o.take("ingress").and_then(|v| {
        let at = o.at("ingress");
        let mut i = d.object(v, &at)?;
        let http_port = port(d, &mut i, "http_port", 80);
        let https_given = i.has("https_port");
        let https_port = port(d, &mut i, "https_port", 443);
        let off = i.off;
        let https_at = i.at("https_port");
        i.finish(d, KUBERNETES_INGRESS_FIELDS, &[]);
        if http_port == https_port {
            d.error(
                off,
                if https_given { &https_at } else { &at },
                format!("port {https_port} serves HTTP already: HTTPS needs a port of its own"),
            );
        }
        Some(KubernetesIngress {
            http_port,
            https_port,
        })
    });
    let certificates_dir = string(d, &mut o, "certificates_dir").map(|s| s.0);
    o.finish(d, KUBERNETES_FIELDS, KUBERNETES_FIT_ONLY);
    Some(KubernetesProvider {
        kubeconfig: kubeconfig.unwrap_or_default(),
        context: context.unwrap_or_default(),
        controller_name: controller_name
            .map(|c| c.0)
            .filter(|c| !c.is_empty())
            .unwrap_or_else(|| DEFAULT_KUBERNETES_CONTROLLER_NAME.to_owned()),
        namespaces: namespaces.into_iter().map(|(_, s)| s).collect(),
        listen_host: listen_host.map(|h| h.0).unwrap_or_default(),
        addresses,
        backends,
        max_objects,
        request_timeout,
        ingress,
        certificates_dir: certificates_dir.unwrap_or_default(),
    })
}

/// A port of `ingress`, `def` when left out or 0.
fn port(d: &mut Decoder<'_>, i: &mut Obj<'_>, key: &str, def: u16) -> u16 {
    i.take(key)
        .and_then(|v| d.int(v, &i.at(key), 0, 65535))
        .and_then(|n| u16::try_from(n).ok())
        .filter(|n| *n != 0)
        .unwrap_or(def)
}

/// fit's `controllerPattern`: a domain, then `/` and a path, as `example.net/gateway-controller`.
fn valid_controller(s: &str) -> bool {
    let Some((domain, path)) = s.split_once('/') else {
        return false;
    };
    let label = |l: &str| {
        let b = l.as_bytes();
        !b.is_empty()
            && b.iter()
                .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || *c == b'-')
            && b.first().is_some_and(|c| *c != b'-')
            && b.last().is_some_and(|c| *c != b'-')
    };
    domain.split('.').all(label)
        && !path.is_empty()
        && path
            .bytes()
            .all(|c| c.is_ascii_alphanumeric() || b"/-._~%!$&'()*+,;=:".contains(&c))
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

    use super::{KubernetesBackends, KubernetesIngress, ProviderKind, valid_controller};
    use std::time::Duration;

    fn errors(src: &str) -> String {
        crate::from_json(src, None).unwrap_err().to_string()
    }

    /// fit's `checkProviders` for the kubernetes kind, its defaults, and what is not supported
    /// yet.
    #[test]
    fn kubernetes() {
        let c = crate::from_json(
            r#"{"providers": [{"kubernetes": {"ingress": {}}},
              {"name": "b", "kubernetes": {"kubeconfig": "/k", "context": "c", "controller_name": "example.net/x",
               "namespaces": ["a", "b"], "listen_host": "127.0.0.1", "addresses": ["192.0.2.1", "lb.example.com"],
               "backends": "cluster_ip", "max_objects": 5,
               "request_timeout": "3s", "certificates_dir": "/d", "ingress": {"http_port": 8080, "https_port": 8443}}}]}"#,
            None,
        )
        .unwrap();
        assert_eq!(c.providers.len(), 2);
        assert_eq!(c.providers[0].name, "kubernetes");
        let ProviderKind::Kubernetes(k) = &c.providers[0].kind else {
            panic!("not kubernetes")
        };
        assert_eq!(k.controller_name, "lesismal.github.io/akit");
        assert_eq!(
            (k.max_objects, k.request_timeout),
            (10_000, Duration::from_secs(10))
        );
        assert_eq!(
            k.ingress,
            Some(KubernetesIngress {
                http_port: 80,
                https_port: 443
            })
        );
        let ProviderKind::Kubernetes(k) = &c.providers[1].kind else {
            panic!("not kubernetes")
        };
        assert_eq!(c.providers[1].name, "b");
        assert_eq!(k.backends, KubernetesBackends::ClusterIp);
        assert_eq!(
            (k.max_objects, k.request_timeout),
            (5, Duration::from_secs(3))
        );
        assert_eq!(k.namespaces, ["a", "b"]);
        assert_eq!(k.addresses, ["192.0.2.1", "lb.example.com"]);
        assert_eq!(k.ingress.unwrap().https_port, 8443);
        // The Gateway API alone, without Ingresses.
        let c = crate::from_json(r#"{"providers": [{"kubernetes": {}}]}"#, None).unwrap();
        let ProviderKind::Kubernetes(k) = &c.providers[0].kind else {
            panic!("not kubernetes")
        };
        assert!(k.ingress.is_none() && k.addresses.is_empty());

        for (src, want) in [
            (
                r#"{"kubernetes": {"addresses": ["1.2.3.4", "*.example.com"]}}"#,
                r#"providers[0].kubernetes.addresses[1]: "*.example.com" is not an IP address or host name"#,
            ),
            (
                r#"{"kubernetes": {"addresses": ["a_b"]}}"#,
                "is not an IP address or host name",
            ),
            (
                r#"{"kubernetes": {"ingress": {}, "allow_external_name_services": true}}"#,
                "allow_external_name_services: not supported yet",
            ),
            (
                r#"{"etcd": {}, "kubernetes": {"ingress": {}}}"#,
                "set exactly one kind of provider",
            ),
            (r#"{"name": "x"}"#, "set exactly one kind of provider"),
            (
                r#"{"docker": {}, "kubernetes": {"ingress": {}}}"#,
                "set exactly one kind of provider",
            ),
            (
                r#"{"name": "Bad", "kubernetes": {"ingress": {}}}"#,
                r#"providers[0].name: "Bad" is not a valid name"#,
            ),
            (
                r#"{"name": "file", "kubernetes": {"ingress": {}}}"#,
                "names a source of the base config",
            ),
            (
                r#"{"kubernetes": {"ingress": {}, "listen_host": "nowhere"}}"#,
                "is not an IP address or localhost",
            ),
            (
                r#"{"kubernetes": {"ingress": {}, "controller_name": "akit"}}"#,
                "is not a domain-prefixed path",
            ),
            (
                r#"{"kubernetes": {"ingress": {}, "backends": "pods"}}"#,
                "is not one of endpoints, cluster_ip",
            ),
            (
                r#"{"kubernetes": {"ingress": {"http_port": 70000}}}"#,
                "http_port",
            ),
            (
                r#"{"kubernetes": {"ingress": {"https_port": 80}}}"#,
                "port 80 serves HTTP already",
            ),
            (
                r#"{"kubernetes": {"ingress": {"tls_port": 1}}}"#,
                "tls_port: unknown field",
            ),
        ] {
            let e = errors(&format!(r#"{{"providers": [{src}]}}"#));
            assert!(e.contains(want), "{src}: {e}");
        }
        let e = errors(
            r#"{"providers": [{"kubernetes": {"ingress": {}}}, {"kubernetes": {"ingress": {}}}]}"#,
        );
        assert!(
            e.contains("providers[1].name: another provider is already named \"kubernetes\""),
            "{e}"
        );
    }

    #[test]
    fn controllers() {
        for ok in [
            "lesismal.github.io/akit",
            "example.net/gateway-controller",
            "a/b/c",
        ] {
            assert!(valid_controller(ok), "{ok}");
        }
        for bad in ["akit", "/akit", "Example.net/x", "a-/x", "a/", "a/b c"] {
            assert!(!valid_controller(bad), "{bad}");
        }
    }
}
