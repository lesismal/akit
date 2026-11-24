//! The fields the provider reads of the kinds it watches, as the API server writes them in JSON;
//! everything else is left out (fit's `types.go`). An object is decoded from its JSON once, when
//! listed or when its watch event comes, and kept so.

use akit_config::value::Value;

/// An object's metadata.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct Meta {
    pub(crate) name: String,
    pub(crate) namespace: String,
    pub(crate) resource_version: String,
    pub(crate) creation_timestamp: String,
    pub(crate) labels: Vec<(String, String)>,
    pub(crate) annotations: Vec<(String, String)>,
}

impl Meta {
    /// How a store keys an object: its namespace and name.
    pub(crate) fn key(&self) -> String {
        if self.namespace.is_empty() {
            self.name.clone()
        } else {
            format!("{}/{}", self.namespace, self.name)
        }
    }

    pub(crate) fn label(&self, k: &str) -> Option<&str> {
        find(&self.labels, k)
    }

    pub(crate) fn annotation(&self, k: &str) -> Option<&str> {
        find(&self.annotations, k)
    }
}

fn find<'a>(kv: &'a [(String, String)], k: &str) -> Option<&'a str> {
    kv.iter().find(|(n, _)| n == k).map(|(_, v)| v.as_str())
}

/// A networking.k8s.io/v1 IngressClass.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct IngressClass {
    pub(crate) meta: Meta,
    pub(crate) controller: String,
}

/// A networking.k8s.io/v1 Ingress.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct Ingress {
    pub(crate) meta: Meta,
    pub(crate) class_name: Option<String>,
    pub(crate) default_backend: Option<Backend>,
    pub(crate) tls: Vec<IngressTls>,
    pub(crate) rules: Vec<IngressRule>,
}

/// One of an Ingress's `tls`.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct IngressTls {
    pub(crate) hosts: Vec<String>,
    pub(crate) secret_name: String,
}

/// An Ingress's paths for a host, or for any; `paths` is `None` without `http`.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct IngressRule {
    pub(crate) host: String,
    pub(crate) paths: Option<Vec<IngressPath>>,
}

/// A path of a rule and its backend.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct IngressPath {
    pub(crate) path: String,
    pub(crate) path_type: String,
    pub(crate) backend: Backend,
}

/// An Ingress's backend: a Service's port, by number or name, or another resource (`service`
/// `None`).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct Backend {
    pub(crate) service: Option<ServiceRef>,
}

/// A backend's Service and port.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct ServiceRef {
    pub(crate) name: String,
    pub(crate) port_name: String,
    /// 0 when given by name.
    pub(crate) port_number: i64,
}

/// A core/v1 Service.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct Service {
    pub(crate) meta: Meta,
    pub(crate) typ: String,
    pub(crate) cluster_ip: String,
    pub(crate) external_name: String,
    pub(crate) ports: Vec<ServicePort>,
}

/// One of a Service's ports.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct ServicePort {
    pub(crate) name: String,
    pub(crate) protocol: String,
    pub(crate) port: i64,
    pub(crate) app_protocol: Option<String>,
}

/// A discovery.k8s.io/v1 EndpointSlice.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct EndpointSlice {
    pub(crate) meta: Meta,
    pub(crate) endpoints: Vec<Endpoint>,
    pub(crate) ports: Vec<SlicePort>,
}

/// One of a slice's endpoints.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct Endpoint {
    pub(crate) addresses: Vec<String>,
    pub(crate) ready: Option<bool>,
}

/// One of a slice's ports.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct SlicePort {
    pub(crate) name: Option<String>,
    pub(crate) protocol: Option<String>,
    pub(crate) port: Option<i64>,
}

/// A core/v1 Secret, of type `kubernetes.io/tls`: its `tls.crt` and `tls.key`.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct Secret {
    pub(crate) meta: Meta,
    pub(crate) typ: String,
    pub(crate) crt: Vec<u8>,
    pub(crate) key: Vec<u8>,
}

/// An object of one of the kinds watched.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Object {
    IngressClass(IngressClass),
    Ingress(Ingress),
    Service(Service),
    Slice(EndpointSlice),
    Secret(Secret),
}

impl Object {
    pub(crate) fn meta(&self) -> &Meta {
        match self {
            Object::IngressClass(o) => &o.meta,
            Object::Ingress(o) => &o.meta,
            Object::Service(o) => &o.meta,
            Object::Slice(o) => &o.meta,
            Object::Secret(o) => &o.meta,
        }
    }
}

/// The kinds of object, as [`decode`] reads them.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Shape {
    IngressClass,
    Ingress,
    Service,
    Slice,
    Secret,
}

fn s(v: &Value, k: &str) -> String {
    match v.get(k) {
        Some(Value::String(s)) => s.clone(),
        _ => String::new(),
    }
}

fn opt_s(v: &Value, k: &str) -> Option<String> {
    match v.get(k) {
        Some(Value::String(s)) => Some(s.clone()),
        _ => None,
    }
}

fn int(v: &Value, k: &str) -> Option<i64> {
    match v.get(k) {
        Some(Value::Number(n)) => n.parse().ok(),
        _ => None,
    }
}

fn arr<'a>(v: &'a Value, k: &str) -> &'a [Value] {
    match v.get(k) {
        Some(Value::Array(a)) => a,
        _ => &[],
    }
}

fn strings(v: &Value, k: &str) -> Vec<String> {
    arr(v, k)
        .iter()
        .filter_map(|x| match x {
            Value::String(s) => Some(s.clone()),
            _ => None,
        })
        .collect()
}

fn map(v: &Value, k: &str) -> Vec<(String, String)> {
    match v.get(k) {
        Some(Value::Object(ms)) => ms
            .iter()
            .filter_map(|(k, v)| match v {
                Value::String(s) => Some((k.clone(), s.clone())),
                _ => None,
            })
            .collect(),
        _ => Vec::new(),
    }
}

const NULL: Value = Value::Null;

fn obj<'a>(v: &'a Value, k: &str) -> &'a Value {
    v.get(k).unwrap_or(&NULL)
}

pub(crate) fn meta(v: &Value) -> Meta {
    let m = obj(v, "metadata");
    Meta {
        name: s(m, "name"),
        namespace: s(m, "namespace"),
        resource_version: s(m, "resourceVersion"),
        creation_timestamp: s(m, "creationTimestamp"),
        labels: map(m, "labels"),
        annotations: map(m, "annotations"),
    }
}

fn backend(v: &Value) -> Backend {
    let svc = v.get("service").filter(|s| matches!(s, Value::Object(_)));
    Backend {
        service: svc.map(|svc| {
            let port = obj(svc, "port");
            ServiceRef {
                name: s(svc, "name"),
                port_name: s(port, "name"),
                port_number: int(port, "number").unwrap_or(0),
            }
        }),
    }
}

/// Decodes `v`, an object of `shape`.
///
/// # Errors
///
/// When `v` is not an object with metadata naming it.
pub(crate) fn decode(shape: Shape, v: &Value) -> Result<Object, String> {
    if !matches!(v, Value::Object(_)) || !matches!(v.get("metadata"), Some(Value::Object(_))) {
        return Err("not an object with metadata".to_owned());
    }
    let meta = meta(v);
    if meta.name.is_empty() {
        return Err("an object without a name".to_owned());
    }
    let spec = obj(v, "spec");
    Ok(match shape {
        Shape::IngressClass => Object::IngressClass(IngressClass {
            meta,
            controller: s(spec, "controller"),
        }),
        Shape::Ingress => Object::Ingress(Ingress {
            meta,
            class_name: opt_s(spec, "ingressClassName"),
            default_backend: spec
                .get("defaultBackend")
                .filter(|b| matches!(b, Value::Object(_)))
                .map(backend),
            tls: arr(spec, "tls")
                .iter()
                .map(|t| IngressTls {
                    hosts: strings(t, "hosts"),
                    secret_name: s(t, "secretName"),
                })
                .collect(),
            rules: arr(spec, "rules")
                .iter()
                .map(|r| IngressRule {
                    host: s(r, "host"),
                    paths: r
                        .get("http")
                        .filter(|h| matches!(h, Value::Object(_)))
                        .map(|h| {
                            arr(h, "paths")
                                .iter()
                                .map(|p| IngressPath {
                                    path: s(p, "path"),
                                    path_type: s(p, "pathType"),
                                    backend: backend(obj(p, "backend")),
                                })
                                .collect()
                        }),
                })
                .collect(),
        }),
        Shape::Service => Object::Service(Service {
            meta,
            typ: s(spec, "type"),
            cluster_ip: s(spec, "clusterIP"),
            external_name: s(spec, "externalName"),
            ports: arr(spec, "ports")
                .iter()
                .map(|p| ServicePort {
                    name: s(p, "name"),
                    protocol: s(p, "protocol"),
                    port: int(p, "port").unwrap_or(0),
                    app_protocol: opt_s(p, "appProtocol"),
                })
                .collect(),
        }),
        Shape::Slice => Object::Slice(EndpointSlice {
            meta,
            endpoints: arr(v, "endpoints")
                .iter()
                .map(|e| Endpoint {
                    addresses: strings(e, "addresses"),
                    ready: match obj(e, "conditions").get("ready") {
                        Some(Value::Bool(b)) => Some(*b),
                        _ => None,
                    },
                })
                .collect(),
            ports: arr(v, "ports")
                .iter()
                .map(|p| SlicePort {
                    name: opt_s(p, "name"),
                    protocol: opt_s(p, "protocol"),
                    port: int(p, "port"),
                })
                .collect(),
        }),
        Shape::Secret => {
            let data = obj(v, "data");
            let b64 = |k: &str| -> Result<Vec<u8>, String> {
                match data.get(k) {
                    Some(Value::String(s)) => akio_crypto::base64::STANDARD
                        .decode(s)
                        .map_err(|_| format!("secret {}: {k} is not base64", meta.key())),
                    _ => Ok(Vec::new()),
                }
            };
            let (crt, key) = (b64("tls.crt")?, b64("tls.key")?);
            Object::Secret(Secret {
                typ: s(v, "type"),
                crt,
                key,
                meta,
            })
        }
    })
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::panic)]

    use super::{Object, Shape, decode};
    use akit_config::value::parse;

    /// fit's `TestDecodeKinds`: each kind's fields, and objects that are not.
    #[test]
    fn decode_kinds() {
        let v = parse(
            r#"{"metadata": {"namespace": "ns", "name": "i", "annotations": {"a": "b"}},
              "spec": {"ingressClassName": "fit", "rules": [{"host": "h", "http": {"paths":
              [{"path": "/", "pathType": "Prefix", "backend": {"service": {"name": "s",
              "port": {"name": "http"}}}}]}}, {"host": "x"}]}}"#,
        )
        .unwrap();
        let Object::Ingress(i) = decode(Shape::Ingress, &v).unwrap() else {
            panic!()
        };
        assert_eq!(i.meta.key(), "ns/i");
        assert_eq!(i.meta.annotation("a"), Some("b"));
        assert_eq!(i.class_name.as_deref(), Some("fit"));
        let paths = i.rules[0].paths.as_ref().unwrap();
        assert_eq!(paths[0].backend.service.as_ref().unwrap().port_name, "http");
        assert!(i.rules[1].paths.is_none());
        let v = parse(r#"{"metadata": {"name": "s"}, "type": "kubernetes.io/tls", "data": {"tls.crt": "aGk="}}"#).unwrap();
        let Object::Secret(s) = decode(Shape::Secret, &v).unwrap() else {
            panic!()
        };
        assert_eq!((s.crt.as_slice(), s.key.len()), (&b"hi"[..], 0));
        for bad in [r#"[]"#, r#"{"spec": {}}"#, r#"{"metadata": {}}"#] {
            assert!(
                decode(Shape::Service, &parse(bad).unwrap()).is_err(),
                "{bad}"
            );
        }
        let v = parse(r#"{"metadata": {"name": "s"}, "data": {"tls.crt": "!!"}}"#).unwrap();
        assert!(decode(Shape::Secret, &v).is_err());
    }
}
