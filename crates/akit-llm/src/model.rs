//! fit's `llm/internal/model` for the mesh: the business codes, the roles, the mesh's methods and
//! the identities and links its replies carry, as JSON byte for byte fit's field names.

use akit_config::value::Value;

use crate::json::{self, Obj};

/// The business codes (fit's `model.Code*`).
pub const CODE_OK: i64 = 0;
/// A bad request.
pub const CODE_BAD_REQUEST: i64 = 1000;
/// Not authenticated (no hello yet).
pub const CODE_UNAUTHORIZED: i64 = 2000;
/// Not allowed.
pub const CODE_FORBIDDEN: i64 = 2001;
/// Not found.
pub const CODE_NOT_FOUND: i64 = 3000;
/// A conflict.
pub const CODE_CONFLICT: i64 = 3001;
/// Too many at once.
pub const CODE_RATE_LIMITED: i64 = 4000;
/// The peer that should serve it is offline, or a node cannot serve the model now.
pub const CODE_UNAVAILABLE: i64 = 4003;
/// An internal error.
pub const CODE_INTERNAL: i64 = 5000;

/// A node.
pub const ROLE_NODE: &str = "node";
/// A hub.
pub const ROLE_HUB: &str = "hub";
/// A proxy.
pub const ROLE_PROXY: &str = "proxy";
/// The caller of a method a role's own pages make; never over the mesh.
pub const ROLE_LOCAL: &str = "local";

/// The dialer's first call on every link: both sides learn each other's identity.
pub const METHOD_HELLO: &str = "/mesh/hello";
/// The identity, listener, links and dial targets of a role (its own pages).
pub const METHOD_PEERS: &str = "/mesh/peers";
/// The links and traffic of a role (its own pages).
pub const METHOD_STATUS: &str = "/mesh/status";

/// What a role says at hello.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Identity {
    /// `hub`, `node` or `proxy`.
    pub role: String,
    /// Stable, made at first start.
    pub id: String,
    /// Its name.
    pub name: String,
    /// The host's name.
    pub hostname: String,
    /// `akit <version>` (fit's say `fit …`).
    pub version: String,
    /// The process's ID.
    pub pid: i64,
    /// Its pages' URL.
    pub http_url: String,
    /// Where its listener can be dialed.
    pub rpc_addr: String,
    /// When it started, in seconds since 1970.
    pub started_at: i64,
}

impl Identity {
    /// As JSON.
    #[must_use]
    pub fn to_json(&self) -> String {
        self.obj().end()
    }

    fn obj(&self) -> Obj {
        Obj::new()
            .str("role", &self.role)
            .str("id", &self.id)
            .str("name", &self.name)
            .str("hostname", &self.hostname)
            .str("version", &self.version)
            .int("pid", self.pid)
            .str_omit("http_url", &self.http_url)
            .str_omit("rpc_addr", &self.rpc_addr)
            .int("started_at", self.started_at)
    }

    /// From a JSON value.
    #[must_use]
    pub fn from_value(v: &Value) -> Identity {
        Identity {
            role: json::get_str(v, "role"),
            id: json::get_str(v, "id"),
            name: json::get_str(v, "name"),
            hostname: json::get_str(v, "hostname"),
            version: json::get_str(v, "version"),
            pid: json::get_int(v, "pid"),
            http_url: json::get_str(v, "http_url"),
            rpc_addr: json::get_str(v, "rpc_addr"),
            started_at: json::get_int(v, "started_at"),
        }
    }

    /// From JSON.
    ///
    /// # Errors
    ///
    /// When `b` is not a JSON object.
    pub fn parse(b: &[u8]) -> Result<Identity, String> {
        match json::parse(b)? {
            v @ Value::Object(_) => Ok(Identity::from_value(&v)),
            _ => Err("not an object".to_owned()),
        }
    }
}

/// A link's round trips, in milliseconds.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct Rtt {
    /// The last.
    pub last: f64,
    /// A moving average weighing the recent ones most.
    pub avg: f64,
    /// The shortest.
    pub min: f64,
    /// How many.
    pub samples: i64,
}

impl Rtt {
    /// As JSON.
    #[must_use]
    pub fn to_json(&self) -> String {
        Obj::new()
            .float("last", self.last)
            .float("avg", self.avg)
            .float("min", self.min)
            .int("samples", self.samples)
            .end()
    }
}

/// A link's traffic and latency.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct LinkStats {
    /// The bytes of the frames received, as they crossed.
    pub bytes_in: u64,
    /// The bytes of the frames sent.
    pub bytes_out: u64,
    /// The round trips.
    pub rtt: Rtt,
}

impl LinkStats {
    /// As JSON.
    #[must_use]
    pub fn to_json(&self) -> String {
        Obj::new()
            .int("bytes_in", i64::try_from(self.bytes_in).unwrap_or(i64::MAX))
            .int(
                "bytes_out",
                i64::try_from(self.bytes_out).unwrap_or(i64::MAX),
            )
            .raw("rtt", &self.rtt.to_json())
            .end()
    }
}

/// One live link.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct PeerConn {
    /// The peer.
    pub identity: Identity,
    /// The link's number here.
    pub conn_id: i64,
    /// `in` or `out`.
    pub direction: String,
    /// The address an outbound link dialed.
    pub target: String,
    /// The peer's address.
    pub remote: String,
    /// When it linked, in seconds since 1970.
    pub connected_at: i64,
    /// Either side sent drain.
    pub draining: bool,
    /// The token an inbound link proved.
    pub token_id: String,
    /// An inbound link proved a shared password.
    pub shared_password: bool,
    /// Its traffic and latency.
    pub link: LinkStats,
}

impl PeerConn {
    /// As JSON (the identity's fields inline, as Go embeds them).
    #[must_use]
    pub fn to_json(&self) -> String {
        self.identity
            .obj()
            .int("conn_id", self.conn_id)
            .str("direction", &self.direction)
            .str_omit("target", &self.target)
            .str("remote", &self.remote)
            .int("connected_at", self.connected_at)
            .bool_omit("draining", self.draining)
            .str_omit("token_id", &self.token_id)
            .bool_omit("shared_password", self.shared_password)
            .raw("link", &self.link.to_json())
            .end()
    }
}

/// A dial target: a peer a role links to.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct DialReq {
    /// `host:port`.
    pub addr: String,
    /// The peer listener's shared password (deprecated).
    pub password: String,
    /// The token the peer's listener issued.
    pub token: String,
    /// The role the peer must have.
    pub role: String,
    /// The SHA-256 of the peer listener's certificate, in hex: over TLS.
    pub pin: String,
    /// The authorities its certificate must chain to: over TLS.
    pub ca_file: String,
    /// The name its certificate must have, the address's host when empty.
    pub server_name: String,
}

impl DialReq {
    /// Whether the target is dialed over TLS.
    #[must_use]
    pub fn tls(&self) -> bool {
        !self.pin.is_empty() || !self.ca_file.is_empty()
    }

    /// Whether `o` dials alike: the same pin, CA file and name.
    #[must_use]
    pub fn same_tls(&self, o: &DialReq) -> bool {
        self.pin == o.pin && self.ca_file == o.ca_file && self.server_name == o.server_name
    }
}

/// A role's listener.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ListenConfig {
    /// Empty: not listening.
    pub addr: String,
    /// The roles' shared password, for those without their own (deprecated).
    pub password: String,
    /// A node's.
    pub node_password: String,
    /// A proxy's.
    pub proxy_password: String,
    /// A hub's.
    pub hub_password: String,
    /// The certificate's file, over TLS.
    pub cert_file: String,
    /// Its key's file.
    pub key_file: String,
    /// A certificate the role makes.
    pub self_signed: bool,
    /// A certificate, PEM.
    pub cert: String,
    /// Its key, PEM.
    pub key: String,
}

impl ListenConfig {
    /// Whether links are taken over TLS.
    #[must_use]
    pub fn tls(&self) -> bool {
        !self.cert_file.is_empty() || !self.cert.is_empty() || self.self_signed
    }

    /// The password a dialer of `role` proves: the role's own, or the shared one.
    #[must_use]
    pub fn password_for(&self, role: &str) -> &str {
        let own = match role {
            ROLE_NODE => &self.node_password,
            ROLE_PROXY => &self.proxy_password,
            ROLE_HUB => &self.hub_password,
            _ => "",
        };
        if own.is_empty() { &self.password } else { own }
    }

    /// Whether any password is set.
    #[must_use]
    pub fn has_password(&self) -> bool {
        !self.password.is_empty()
            || !self.node_password.is_empty()
            || !self.proxy_password.is_empty()
            || !self.hub_password.is_empty()
    }
}

/// A dial target as the pages see it: no password, no token.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct DialTarget {
    /// `host:port`.
    pub addr: String,
    /// The role the peer must have.
    pub role: String,
    /// Over TLS.
    pub tls: bool,
    /// The pin.
    pub pin: String,
    /// The CA file.
    pub ca_file: String,
    /// The name.
    pub server_name: String,
    /// The ID of the token it is dialed with.
    pub token_id: String,
    /// Linked and said hello.
    pub connected: bool,
    /// The peer, once linked.
    pub peer: Option<Identity>,
    /// The last failure.
    pub error: String,
}

impl DialTarget {
    /// As JSON.
    #[must_use]
    pub fn to_json(&self) -> String {
        let mut o = Obj::new()
            .str("addr", &self.addr)
            .str("role", &self.role)
            .bool_omit("tls", self.tls)
            .str_omit("pin", &self.pin)
            .str_omit("ca_file", &self.ca_file)
            .str_omit("server_name", &self.server_name)
            .str_omit("token_id", &self.token_id)
            .bool("connected", self.connected);
        if let Some(p) = &self.peer {
            o = o.raw("peer", &p.to_json());
        }
        o.str_omit("error", &self.error).end()
    }
}

// ---- The node, the hub and the proxy (I105-I107) ----

/// A node's info, read by its peers (`NodeInfo`).
pub const METHOD_NODE_INFO: &str = "/node/info";
/// A model call forwarded upstream (a stream: `ProxyReq` opens it, the first data frame is a
/// `ProxyHead`, the others the response body, the end a `ProxyResult`).
pub const METHOD_NODE_PROXY: &str = "/node/proxy";
/// A node's push of its info when it changes.
pub const METHOD_NODE_UPDATE: &str = "/node/update";
/// A hub's model list (`HubModels`).
pub const METHOD_HUB_MODELS: &str = "/hub/models";
/// A model call through a hub, which picks a node and relays.
pub const METHOD_HUB_PROXY: &str = "/hub/proxy";
/// A hub's push of its model list when it changes.
pub const METHOD_HUB_MODELS_UPDATE: &str = "/hub/models/update";
/// A hub's node list.
pub const METHOD_HUB_NODE_LIST: &str = "/hub/node/list";
/// A hub's push of its node list.
pub const METHOD_HUB_NODE_LIST_UPDATE: &str = "/hub/node/list/update";

/// The APIs a model speaks, as `llm.node.models[].api` takes them.
pub const API_OPENAI: &str = "openai";
/// Anthropic's Messages.
pub const API_ANTHROPIC: &str = "anthropic";
/// Google's Gemini.
pub const API_GEMINI: &str = "gemini";
/// Azure OpenAI's deployments, called as OpenAI.
pub const API_AZURE_OPENAI: &str = "azure_openai";
/// Ollama's own API.
pub const API_OLLAMA: &str = "ollama";
/// Cohere v2.
pub const API_COHERE: &str = "cohere";

/// The APIs agents speak to a proxy: every kind's but `azure_openai`'s.
pub const PROTOCOLS: &[&str] = &[
    API_OPENAI,
    API_ANTHROPIC,
    API_GEMINI,
    API_OLLAMA,
    API_COHERE,
];

/// The API agents speak to reach a model of kind `api`.
#[must_use]
pub fn protocol(api: &str) -> &str {
    if api == API_AZURE_OPENAI {
        API_OPENAI
    } else {
        api
    }
}

/// The calls a model of each protocol takes: the path under its base URL, Gemini's method.
#[must_use]
pub fn proxy_paths(api: &str) -> &'static [&'static str] {
    match api {
        API_OPENAI => &[
            "chat/completions",
            "responses",
            "completions",
            "embeddings",
            "moderations",
            "images/generations",
            "images/edits",
            "images/variations",
            "audio/speech",
            "audio/transcriptions",
            "audio/translations",
            "rerank",
        ],
        API_ANTHROPIC => &["messages", "messages/count_tokens"],
        API_GEMINI => &[
            "generateContent",
            "streamGenerateContent",
            "countTokens",
            "embedContent",
            "batchEmbedContents",
        ],
        API_OLLAMA => &["api/chat", "api/generate", "api/embed"],
        API_COHERE => &["chat", "embed", "rerank"],
        _ => &[],
    }
}

/// Whether a model of kind `api` takes the call `path` of its protocol.
#[must_use]
pub fn serves(api: &str, path: &str) -> bool {
    if api == API_AZURE_OPENAI && matches!(path, "moderations" | "images/variations" | "rerank") {
        return false;
    }
    proxy_paths(protocol(api)).contains(&path)
}

/// The request headers a call may carry upstream (lower case).
pub const PROXY_HEADERS: &[&str] = &["anthropic-version", "anthropic-beta", "openai-beta"];

/// A model is callable.
pub const MODEL_READY: &str = "ready";
/// A model is turned off.
pub const MODEL_DISABLED: &str = "disabled";
/// A model's last check failed.
pub const MODEL_ERROR: &str = "error";

/// A model as peers see it: no endpoint, no key.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ModelView {
    /// Its name.
    pub name: String,
    /// Its API.
    pub api: String,
    /// `ready`, `disabled` or `error`.
    pub state: String,
    /// The last check's failure.
    pub error: String,
    /// Its description.
    pub description: String,
    /// Its calls under way.
    pub inflight: i64,
    /// Its bound on them; 0: none.
    pub max_inflight: i64,
    /// When it was last checked.
    pub checked_at: i64,
}

impl ModelView {
    /// Whether it may be called now.
    #[must_use]
    pub fn ready(&self) -> bool {
        self.state == MODEL_READY && (self.max_inflight <= 0 || self.inflight < self.max_inflight)
    }

    /// As JSON.
    #[must_use]
    pub fn to_json(&self) -> String {
        Obj::new()
            .str("name", &self.name)
            .str("api", &self.api)
            .str("state", &self.state)
            .str_omit("error", &self.error)
            .str_omit("description", &self.description)
            .int("inflight", self.inflight)
            .int_omit("max_inflight", self.max_inflight)
            .int_omit("checked_at", self.checked_at)
            .end()
    }

    /// From a JSON value.
    #[must_use]
    pub fn from_value(v: &Value) -> ModelView {
        ModelView {
            name: json::get_str(v, "name"),
            api: json::get_str(v, "api"),
            state: json::get_str(v, "state"),
            error: json::get_str(v, "error"),
            description: json::get_str(v, "description"),
            inflight: json::get_int(v, "inflight"),
            max_inflight: json::get_int(v, "max_inflight"),
            checked_at: json::get_int(v, "checked_at"),
        }
    }
}

/// What a node pushes and answers: its models and load (`NodeInfo`); the hardware is I108's.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct NodeInfo {
    /// Its ID.
    pub id: String,
    /// Its name.
    pub name: String,
    /// Its description.
    pub description: String,
    /// Its hardware, as JSON (`HWInfo`).
    pub hw: String,
    /// Its models.
    pub models: Vec<ModelView>,
    /// Its calls under way.
    pub inflight: i64,
    /// Its jobs running.
    pub jobs: i64,
    /// Whether hubs may manage it.
    pub hub_manage: bool,
    /// Grows with every info, so that a stale one is dropped.
    pub seq: i64,
    /// When it was made.
    pub updated_at: i64,
}

impl NodeInfo {
    /// As JSON.
    #[must_use]
    pub fn to_json(&self) -> String {
        Obj::new()
            .str("id", &self.id)
            .str("name", &self.name)
            .str_omit("description", &self.description)
            .raw("hw", if self.hw.is_empty() { "{}" } else { &self.hw })
            .raw(
                "models",
                &json::array(self.models.iter().map(ModelView::to_json)),
            )
            .int("inflight", self.inflight)
            .int("jobs", self.jobs)
            .bool("hub_manage", self.hub_manage)
            .int("seq", self.seq)
            .int("updated_at", self.updated_at)
            .end()
    }

    /// From a JSON value.
    #[must_use]
    pub fn from_value(v: &Value) -> NodeInfo {
        NodeInfo {
            id: json::get_str(v, "id"),
            name: json::get_str(v, "name"),
            description: json::get_str(v, "description"),
            hw: v.get("hw").map(json::write).unwrap_or_default(),
            models: json::get_arr(v, "models")
                .iter()
                .map(ModelView::from_value)
                .collect(),
            inflight: json::get_int(v, "inflight"),
            jobs: json::get_int(v, "jobs"),
            hub_manage: json::get_bool(v, "hub_manage"),
            seq: json::get_int(v, "seq"),
            updated_at: json::get_int(v, "updated_at"),
        }
    }
}

/// One model name across a hub's nodes.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct HubModel {
    /// Its name.
    pub name: String,
    /// Its API.
    pub api: String,
    /// The online nodes serving it.
    pub nodes: Vec<String>,
    /// How many of them are ready.
    pub ready: i64,
}

/// A hub's model list.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct HubModels {
    /// The hub's ID.
    pub hub: String,
    /// Its models.
    pub models: Vec<HubModel>,
    /// Grows with every change.
    pub seq: i64,
}

impl HubModels {
    /// As JSON.
    #[must_use]
    pub fn to_json(&self) -> String {
        Obj::new()
            .str("hub", &self.hub)
            .raw(
                "models",
                &json::array(self.models.iter().map(|m| {
                    Obj::new()
                        .str("name", &m.name)
                        .str("api", &m.api)
                        .strs("nodes", &m.nodes)
                        .int("ready", m.ready)
                        .end()
                })),
            )
            .int("seq", self.seq)
            .end()
    }

    /// From a JSON value.
    #[must_use]
    pub fn from_value(v: &Value) -> HubModels {
        HubModels {
            hub: json::get_str(v, "hub"),
            models: json::get_arr(v, "models")
                .iter()
                .map(|m| HubModel {
                    name: json::get_str(m, "name"),
                    api: json::get_str(m, "api"),
                    nodes: json::get_strs(m, "nodes"),
                    ready: json::get_int(m, "ready"),
                })
                .collect(),
            seq: json::get_int(v, "seq"),
        }
    }
}

/// A model call, forwarded byte for byte (`ProxyReq`).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ProxyReq {
    /// The protocol.
    pub api: String,
    /// The call: one of [`proxy_paths`].
    pub path: String,
    /// The model it names.
    pub model: String,
    /// The body, inline (base64 in the JSON, as Go writes bytes).
    pub body: Vec<u8>,
    /// The body's media type when it is not JSON.
    pub content_type: String,
    /// The query that goes upstream.
    pub query: String,
    /// The headers that go upstream ([`PROXY_HEADERS`]).
    pub header: Vec<(String, String)>,
    /// The body follows as the opener's data frames.
    pub body_stream: bool,
    /// A streamed body's length; 0 or -1 when unknown.
    pub body_size: i64,
    /// Node IDs not to pick.
    pub exclude: Vec<String>,
}

impl ProxyReq {
    /// As JSON.
    #[must_use]
    pub fn to_json(&self) -> String {
        let mut o = Obj::new()
            .str("api", &self.api)
            .str("path", &self.path)
            .str("model", &self.model);
        o = if self.body.is_empty() {
            o.raw("body", "null")
        } else {
            o.str("body", &akio_crypto::base64::STANDARD.encode(&self.body))
        };
        o = o
            .str_omit("content_type", &self.content_type)
            .str_omit("query", &self.query);
        if !self.header.is_empty() {
            let mut h = Obj::new();
            for (k, v) in &self.header {
                h = h.str(k, v);
            }
            o = o.raw("header", &h.end());
        }
        o = o
            .bool_omit("body_stream", self.body_stream)
            .int_omit("body_size", self.body_size);
        if !self.exclude.is_empty() {
            o = o.strs("exclude", &self.exclude);
        }
        o.end()
    }

    /// From JSON.
    ///
    /// # Errors
    ///
    /// Not a JSON object, or a body that is not base64.
    pub fn parse(b: &[u8]) -> Result<ProxyReq, String> {
        let v = json::parse(b)?;
        if !matches!(v, Value::Object(_)) {
            return Err("not an object".to_owned());
        }
        let body = match v.get("body") {
            Some(Value::String(s)) => akio_crypto::base64::STANDARD
                .decode(s)
                .map_err(|e| format!("body: {e}"))?,
            _ => Vec::new(),
        };
        let header = match v.get("header") {
            Some(Value::Object(ms)) => ms
                .iter()
                .filter_map(|(k, v)| match v {
                    Value::String(s) => Some((k.clone(), s.clone())),
                    _ => None,
                })
                .collect(),
            _ => Vec::new(),
        };
        Ok(ProxyReq {
            api: json::get_str(&v, "api"),
            path: json::get_str(&v, "path"),
            model: json::get_str(&v, "model"),
            body,
            content_type: json::get_str(&v, "content_type"),
            query: json::get_str(&v, "query"),
            header,
            body_stream: json::get_bool(&v, "body_stream"),
            body_size: json::get_int(&v, "body_size"),
            exclude: json::get_strs(&v, "exclude"),
        })
    }
}

/// The first data frame of a model call's stream: the upstream's status and media type.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ProxyHead {
    /// The status.
    pub status: i64,
    /// The media type.
    pub content_type: String,
    /// The node that served it.
    pub node: String,
}

impl ProxyHead {
    /// As JSON.
    #[must_use]
    pub fn to_json(&self) -> String {
        Obj::new()
            .int("status", self.status)
            .str("content_type", &self.content_type)
            .str("node", &self.node)
            .end()
    }

    /// From JSON.
    ///
    /// # Errors
    ///
    /// Not a JSON object.
    pub fn parse(b: &[u8]) -> Result<ProxyHead, String> {
        let v = json::parse(b)?;
        if !matches!(v, Value::Object(_)) {
            return Err("not an object".to_owned());
        }
        Ok(ProxyHead {
            status: json::get_int(&v, "status"),
            content_type: json::get_str(&v, "content_type"),
            node: json::get_str(&v, "node"),
        })
    }
}

/// The end of a model call's stream.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ProxyResult {
    /// The node that served it.
    pub node: String,
    /// The model.
    pub model: String,
    /// The upstream's status.
    pub status: i64,
    /// The response body's bytes.
    pub bytes: i64,
    /// How long it took.
    pub duration_ms: i64,
}

impl ProxyResult {
    /// As JSON.
    #[must_use]
    pub fn to_json(&self) -> String {
        Obj::new()
            .str("node", &self.node)
            .str("model", &self.model)
            .int("status", self.status)
            .int("bytes", self.bytes)
            .int("duration_ms", self.duration_ms)
            .end()
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]
    use super::*;

    #[test]
    fn identity_round_trip() {
        let id = Identity {
            role: ROLE_NODE.to_owned(),
            id: "node-0123456789abcdef".to_owned(),
            name: "gpu".to_owned(),
            hostname: "h".to_owned(),
            version: "akit 0.0.1".to_owned(),
            pid: 42,
            http_url: String::new(),
            rpc_addr: "10.0.0.1:7000".to_owned(),
            started_at: 1_700_000_000,
        };
        let j = id.to_json();
        assert_eq!(
            j,
            r#"{"role":"node","id":"node-0123456789abcdef","name":"gpu","hostname":"h","version":"akit 0.0.1","pid":42,"rpc_addr":"10.0.0.1:7000","started_at":1700000000}"#
        );
        assert_eq!(Identity::parse(j.as_bytes()).unwrap(), id);
        assert!(Identity::parse(b"[]").is_err());
    }

    #[test]
    fn proxy_req_round_trip() {
        let r = ProxyReq {
            api: API_OPENAI.to_owned(),
            path: "chat/completions".to_owned(),
            model: "m".to_owned(),
            body: br#"{"model":"m"}"#.to_vec(),
            header: vec![("openai-beta".to_owned(), "x".to_owned())],
            exclude: vec!["node-1".to_owned()],
            ..ProxyReq::default()
        };
        let j = r.to_json();
        assert!(j.contains(r#""body":"eyJtb2RlbCI6Im0ifQ==""#), "{j}");
        assert_eq!(ProxyReq::parse(j.as_bytes()).unwrap(), r);
        assert!(serves(API_AZURE_OPENAI, "chat/completions"));
        assert!(!serves(API_AZURE_OPENAI, "rerank"));
        assert!(serves(API_GEMINI, "streamGenerateContent"));
        assert!(!serves(API_OLLAMA, "chat"));
    }

    #[test]
    fn passwords_by_role() {
        let l = ListenConfig {
            password: "all".to_owned(),
            node_password: "n".to_owned(),
            ..ListenConfig::default()
        };
        assert_eq!(l.password_for(ROLE_NODE), "n");
        assert_eq!(l.password_for(ROLE_PROXY), "all");
        assert!(l.has_password());
        assert!(!ListenConfig::default().has_password());
    }
}
