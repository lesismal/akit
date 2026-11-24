//! `providers`, as fit's `config.Provider` and its checks (`checkProviders`): the sources of
//! config other than the files and the Admin API (I57's framework), one kind each. Kubernetes
//! comes with I59; Docker (I58), Consul and etcd (I61) are refused as not supported yet until
//! theirs.

use std::net::IpAddr;
use std::time::Duration;

use crate::decode::{Decoder, Obj};
use crate::json::Node;

/// A provider (`providers[]`): what it goes by, and its kind with that kind's settings.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Provider {
    /// fit's `Provider.ID()`: `name`, or the kind's name when left out.
    pub name: String,
    /// The one kind set, with its settings.
    pub kind: ProviderKind,
}

/// A provider's kind.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ProviderKind {
    /// `kubernetes` (I59): Ingresses; the Gateway API comes with I60.
    Kubernetes(KubernetesProvider),
}

impl ProviderKind {
    /// The kind's name, as a config gives it: `kubernetes`, ...
    #[must_use]
    pub fn name(&self) -> &'static str {
        match self {
            ProviderKind::Kubernetes(_) => "kubernetes",
        }
    }
}

/// fit's default `providers[].kubernetes.controller_name`.
pub const DEFAULT_KUBERNETES_CONTROLLER_NAME: &str = "lesismal.github.io/fit";
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

/// `providers[].kubernetes`, as fit's `config.KubernetesProvider`, with its defaults filled in.
/// akit serves Ingresses (`ingress`, I59); the Gateway API (and with it `addresses`) comes with
/// I60, and `allow_external_name_services` with the upstreams' `deny_internal`, both refused as
/// not supported yet.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct KubernetesProvider {
    /// `kubeconfig`: the kubeconfig file the cluster is reached with; when empty, the pod's
    /// service account inside a cluster, else `$KUBECONFIG` or `~/.kube/config`.
    pub kubeconfig: String,
    /// `context`: the kubeconfig's context; its `current-context` when empty.
    pub context: String,
    /// `controller_name`: the controller of the IngressClasses whose Ingresses akit serves;
    /// [`DEFAULT_KUBERNETES_CONTROLLER_NAME`] when left out.
    pub controller_name: String,
    /// `namespaces`: the namespaces whose Ingresses are served; every one when empty.
    pub namespaces: Vec<String>,
    /// `listen_host`: the address the listeners are bound on; every address when empty.
    pub listen_host: String,
    /// `backends`: where requests to a Service go.
    pub backends: KubernetesBackends,
    /// `max_objects`: the most objects of each kind kept; [`DEFAULT_KUBERNETES_MAX_OBJECTS`]
    /// when left out or 0.
    pub max_objects: usize,
    /// `request_timeout`: how long each request to the API server may take, but watches;
    /// [`DEFAULT_KUBERNETES_REQUEST_TIMEOUT`] when left out or 0.
    pub request_timeout: Duration,
    /// `ingress`: serve Ingresses, on these ports.
    pub ingress: Option<KubernetesIngress>,
    /// `certificates_dir`: where the certificates of the Secrets Ingresses name are written;
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

// I59: Kubernetes; docker (I58), consul, consul_kv and etcd (I61) when theirs land.
const PROVIDER_FIELDS: &[&str] = &["name", "kubernetes"];
const PROVIDER_FIT_ONLY: &[&str] = &["docker", "consul", "consul_kv", "etcd"];
// I59: Ingresses. addresses is the Gateways' status (I60); allow_external_name_services needs
// the upstreams' deny_internal.
const KUBERNETES_FIELDS: &[&str] = &[
    "kubeconfig",
    "context",
    "controller_name",
    "namespaces",
    "listen_host",
    "backends",
    "max_objects",
    "request_timeout",
    "ingress",
    "certificates_dir",
];
const KUBERNETES_FIT_ONLY: &[&str] = &["addresses", "allow_external_name_services"];
const KUBERNETES_INGRESS_FIELDS: &[&str] = &["http_port", "https_port"];

/// The names of the base config's sources, which no provider may take (fit's `baseSources`).
const BASE_SOURCES: [&str; 4] = ["file", "admin", "rollback", "certificates"];

/// A provider, `providers[i]` at `path`.
pub(crate) fn decode(d: &mut Decoder<'_>, n: &Node, path: &str) -> Option<Provider> {
    let mut o = d.object(n, path)?;
    let name = o.take("name").and_then(|v| d.string(v, &o.at("name")));
    let mut kinds: Vec<ProviderKind> = Vec::new();
    if let Some(v) = o.take("kubernetes") {
        if let Some(k) = kubernetes(d, v, &o.at("kubernetes")) {
            kinds.push(ProviderKind::Kubernetes(k));
        }
    }
    let given = PROVIDER_FIELDS
        .iter()
        .chain(PROVIDER_FIT_ONLY)
        .filter(|k| **k != "name" && o.has(k))
        .count();
    let off = o.off;
    o.finish(d, PROVIDER_FIELDS, PROVIDER_FIT_ONLY);
    // A kind not supported yet was reported by finish.
    if given > 1 || (given == 0 && kinds.is_empty()) {
        d.error(
            off,
            path,
            "set exactly one kind of provider: docker, consul, consul_kv, etcd or kubernetes",
        );
        return None;
    }
    let kind = kinds.pop()?;
    Some(Provider {
        name: name
            .filter(|n| !n.is_empty())
            .unwrap_or_else(|| kind.name().to_owned()),
        kind,
    })
}

/// fit's checks across the providers: names valid and unique, none a source of the base config.
/// Each provider is given with its file, path and position.
pub(crate) fn check<'a>(
    ds: &mut [Decoder<'_>],
    providers: impl Iterator<Item = (usize, &'a str, usize, &'a Provider)>,
) {
    let mut seen: Vec<&str> = Vec::new();
    for (file, path, off, p) in providers {
        let at = crate::error::join_field(path, "name");
        let id = p.name.as_str();
        let msg = if !valid_name(id) {
            Some(format!(
                "{id:?} is not a valid name: use lower-case letters, digits, '_', '.' and '-', starting with a letter or digit, at most 63"
            ))
        } else if BASE_SOURCES.contains(&id) {
            Some(format!(
                "{id:?} names a source of the base config: pick another name"
            ))
        } else if seen.contains(&id) {
            Some(format!(
                "another provider is already named {id:?}: give one a name"
            ))
        } else {
            None
        };
        if let (Some(msg), Some(d)) = (msg, ds.get_mut(file)) {
            d.error(off, &at, msg);
        }
        seen.push(id);
    }
}

/// fit's `providerPattern`: `^[a-z0-9][a-z0-9_.-]{0,62}$`.
fn valid_name(s: &str) -> bool {
    let mut b = s.bytes();
    s.len() <= 63
        && b.next()
            .is_some_and(|c| c.is_ascii_lowercase() || c.is_ascii_digit())
        && b.all(|c| {
            c.is_ascii_lowercase() || c.is_ascii_digit() || matches!(c, b'_' | b'.' | b'-')
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
    let off = o.off;
    o.finish(d, KUBERNETES_FIELDS, KUBERNETES_FIT_ONLY);
    if ingress.is_none() {
        // fit's provider always serves the Gateway API, which comes with I60: until then a
        // provider without ingress would serve nothing.
        d.not_yet(
            off,
            path,
            "the Gateway API (I60): set ingress to serve Ingresses",
        );
    }
    Some(KubernetesProvider {
        kubeconfig: kubeconfig.unwrap_or_default(),
        context: context.unwrap_or_default(),
        controller_name: controller_name
            .map(|c| c.0)
            .filter(|c| !c.is_empty())
            .unwrap_or_else(|| DEFAULT_KUBERNETES_CONTROLLER_NAME.to_owned()),
        namespaces: namespaces.into_iter().map(|(_, s)| s).collect(),
        listen_host: listen_host.map(|h| h.0).unwrap_or_default(),
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
    #![allow(clippy::unwrap_used, clippy::panic)]

    use super::{KubernetesBackends, KubernetesIngress, ProviderKind};
    use super::{valid_controller, valid_name};
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
               "namespaces": ["a", "b"], "listen_host": "127.0.0.1", "backends": "cluster_ip", "max_objects": 5,
               "request_timeout": "3s", "certificates_dir": "/d", "ingress": {"http_port": 8080, "https_port": 8443}}}]}"#,
            None,
        )
        .unwrap();
        assert_eq!(c.providers.len(), 2);
        assert_eq!(c.providers[0].name, "kubernetes");
        let ProviderKind::Kubernetes(k) = &c.providers[0].kind;
        assert_eq!(k.controller_name, "lesismal.github.io/fit");
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
        let ProviderKind::Kubernetes(k) = &c.providers[1].kind;
        assert_eq!(c.providers[1].name, "b");
        assert_eq!(k.backends, KubernetesBackends::ClusterIp);
        assert_eq!(
            (k.max_objects, k.request_timeout),
            (5, Duration::from_secs(3))
        );
        assert_eq!(k.namespaces, ["a", "b"]);
        assert_eq!(k.ingress.unwrap().https_port, 8443);

        for (src, want) in [
            (
                r#"{"kubernetes": {}}"#,
                "not supported yet: the Gateway API (I60): set ingress to serve Ingresses",
            ),
            (
                r#"{"kubernetes": {"ingress": {}, "addresses": ["1.2.3.4"]}}"#,
                "providers[0].kubernetes.addresses: not supported yet",
            ),
            (
                r#"{"kubernetes": {"ingress": {}, "allow_external_name_services": true}}"#,
                "allow_external_name_services: not supported yet",
            ),
            (
                r#"{"docker": {}}"#,
                "providers[0].docker: not supported yet",
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
                r#"{"kubernetes": {"ingress": {}, "controller_name": "fit"}}"#,
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
    fn names_and_controllers() {
        for ok in ["k8s", "a.b_c-d", "0"] {
            assert!(valid_name(ok), "{ok}");
        }
        for bad in ["", "K8s", "-a", &"a".repeat(64)] {
            assert!(!valid_name(bad), "{bad}");
        }
        for ok in [
            "lesismal.github.io/fit",
            "example.net/gateway-controller",
            "a/b/c",
        ] {
            assert!(valid_controller(ok), "{ok}");
        }
        for bad in ["fit", "/fit", "Example.net/x", "a-/x", "a/", "a/b c"] {
            assert!(!valid_controller(bad), "{bad}");
        }
    }
}
