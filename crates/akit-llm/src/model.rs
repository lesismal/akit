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
