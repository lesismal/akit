//! fit's roles of the config (I98): the tunnel's `tunnel.hub`, `tunnel.node` and `tunnel.proxy`,
//! and the LLM feature's `llm.hub`, `llm.node` and `llm.proxy`, with fit's fields, defaults,
//! bounds and checks (`config/tunnel.go`, `config/llm.go`, `config/linktoken.go`), and the link
//! tokens their listeners issue (fit's decision 035).
//!
//! The members are read as strictly as the rest of the config. What is left out is kept empty
//! (a string), zero (a number, a duration) or `None`; the methods give fit's defaults for it.

use std::net::IpAddr;
use std::time::Duration;

use crate::decode::{Decoder, Obj};
use crate::error::{join_field, join_index};
use crate::json::{Kind, Node};
use crate::model::{Access, Console};

/// The prefix of every link token akit issues: fit's `fitt1.`, renamed (decision 037). A token
/// is the prefix, its ID (16 hex digits), a dot and its secret in unpadded base64url. The hash a
/// listener keeps is of the secret alone, so a token fit issued links to akit written with this
/// prefix in place of fit's (decision 038).
pub const LINK_TOKEN_PREFIX: &str = "akitt1.";
/// The most tokens one listener issues.
pub const MAX_LINK_TOKENS: usize = 1024;
/// The most members one token is bound to.
pub const MAX_LINK_TOKEN_MEMBERS: usize = 256;

/// A token a listener issued, which a node, proxy or hub links with in place of a shared
/// password: the tunnel hub's, and an LLM role's listener's. Only a hash of its secret is kept.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct LinkToken {
    /// `id`: 16 lowercase hex digits, unique among the listener's.
    pub id: String,
    /// `name`: what the token is for, 1 to 64 characters.
    pub name: String,
    /// `hash`: the SHA-256 of the key the token's secret derives, 64 hex digits.
    pub hash: String,
    /// `created_at`: when the token was made, in seconds since 1970; 0 when left out.
    pub created_at: i64,
    /// `expires_at`: when the token expires, in seconds since 1970; 0, never.
    pub expires_at: i64,
    /// `note`: anything else about it, up to 1024 bytes.
    pub note: String,
    /// `members`: the members that first linked with it, which no other token may link as.
    pub members: Vec<String>,
}

impl LinkToken {
    /// Whether the token has expired at `now`, in seconds since 1970.
    #[must_use]
    pub fn expired(&self, now: i64) -> bool {
        self.expires_at > 0 && now >= self.expires_at
    }
}

/// Whether `id` is a token's ID: 16 lowercase hex digits.
#[must_use]
pub fn valid_link_token_id(id: &str) -> bool {
    id.len() == 16
        && id
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}

/// Why `tok`, a link token a dialer is given, is not one, or `None`.
#[must_use]
pub fn check_link_token_string(tok: &str) -> Option<String> {
    let ok = tok.strip_prefix(LINK_TOKEN_PREFIX).is_some_and(|rest| {
        rest.split_once('.').is_some_and(|(id, secret)| {
            valid_link_token_id(id)
                && (22..=86).contains(&secret.len())
                && secret
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
        })
    });
    (!ok).then(|| {
        format!(
            "not a link token: one is {LINK_TOKEN_PREFIX}<16 hex digits>.<secret>, as the \
             listener's pages, its Admin API or akit token make it"
        )
    })
}

// ---------------------------------------------------------------------------------------------
// The tunnel.

/// The protocols a tunnel listener or endpoint may take.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum TunnelProtocol {
    /// `tcp`: the frames in the clear.
    #[default]
    Tcp,
    /// `tls`: TCP over TLS.
    Tls,
    /// `ws`: a WebSocket in the clear.
    Ws,
    /// `wss`: a WebSocket over TLS.
    Wss,
    /// `quic`: QUIC.
    Quic,
}

impl TunnelProtocol {
    /// The protocol's name, as the config writes it.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            TunnelProtocol::Tcp => "tcp",
            TunnelProtocol::Tls => "tls",
            TunnelProtocol::Ws => "ws",
            TunnelProtocol::Wss => "wss",
            TunnelProtocol::Quic => "quic",
        }
    }

    fn parse(s: &str) -> Option<TunnelProtocol> {
        Some(match s {
            "tcp" => TunnelProtocol::Tcp,
            "tls" => TunnelProtocol::Tls,
            "ws" => TunnelProtocol::Ws,
            "wss" => TunnelProtocol::Wss,
            "quic" => TunnelProtocol::Quic,
            _ => return None,
        })
    }

    /// Whether the protocol runs TLS: tls, wss and quic.
    #[must_use]
    pub fn takes_tls(self) -> bool {
        matches!(
            self,
            TunnelProtocol::Tls | TunnelProtocol::Wss | TunnelProtocol::Quic
        )
    }

    /// Whether the protocol serves WebSockets: ws and wss.
    #[must_use]
    pub fn is_ws(self) -> bool {
        matches!(self, TunnelProtocol::Ws | TunnelProtocol::Wss)
    }
}

/// The path of a ws or wss listener when left out.
pub const DEFAULT_TUNNEL_WS_PATH: &str = "/tunnel";
/// fit's defaults of the tunnel's roles.
pub const DEFAULT_TUNNEL_POOL_SIZE: usize = 10;
/// The most links a hub keeps open.
pub const DEFAULT_TUNNEL_MAX_LINKS: usize = 1024;
/// The most links open that have not registered.
pub const DEFAULT_TUNNEL_MAX_UNREGISTERED: usize = 256;
/// The most links from one IP address that have not registered.
pub const DEFAULT_TUNNEL_MAX_UNREGISTERED_PER_IP: usize = 32;
/// The most links one peer registers.
pub const DEFAULT_TUNNEL_MAX_LINKS_PER_PEER: usize = 128;
/// The most peers a hub registers.
pub const DEFAULT_TUNNEL_MAX_PEERS: usize = 1024;
/// The most tunnels a hub passes.
pub const DEFAULT_TUNNEL_HUB_TUNNELS: usize = 16384;
/// The most tunnels a node or proxy keeps open.
pub const DEFAULT_TUNNEL_TUNNELS: usize = 4096;
/// The most QUIC handshakes under way at a hub.
pub const DEFAULT_TUNNEL_MAX_HANDSHAKES: usize = 64;
/// The most connections of a console socket or a web proxy.
pub const DEFAULT_TUNNEL_PROXY_CONNS: usize = 32;
/// How long a link may take to register.
pub const DEFAULT_TUNNEL_REGISTER_TIMEOUT: Duration = Duration::from_secs(10);
/// How long a node may take to answer its hub.
pub const DEFAULT_TUNNEL_HUB_CALL: Duration = Duration::from_secs(15);
/// How long a hub may take to answer a node or proxy.
pub const DEFAULT_TUNNEL_CALL_TIMEOUT: Duration = Duration::from_secs(10);
/// How long a dial may take.
pub const DEFAULT_TUNNEL_DIAL_TIMEOUT: Duration = Duration::from_secs(5);
/// The wait before a link dials again.
pub const DEFAULT_TUNNEL_RETRY_INTERVAL: Duration = Duration::from_secs(3);
/// How often the round trip to a hub is measured.
pub const DEFAULT_TUNNEL_PING_INTERVAL: Duration = Duration::from_secs(5);
/// How long a ping may go unanswered.
pub const DEFAULT_TUNNEL_PING_TIMEOUT: Duration = Duration::from_secs(5);
/// How long a tunnel may take to open.
pub const DEFAULT_TUNNEL_OPEN_TIMEOUT: Duration = Duration::from_secs(20);
/// How long a tunnel may carry nothing.
pub const DEFAULT_TUNNEL_IDLE_TIMEOUT: Duration = Duration::from_secs(2 * 60 * 60);
/// How long a QUIC connection may hear nothing.
pub const DEFAULT_TUNNEL_QUIC_IDLE: Duration = Duration::from_secs(30);
/// How often a quiet QUIC connection is pinged.
pub const DEFAULT_TUNNEL_QUIC_KEEP_ALIVE: Duration = Duration::from_secs(10);
/// How long a QUIC handshake may take.
pub const DEFAULT_TUNNEL_QUIC_HANDSHAKE: Duration = Duration::from_secs(10);

/// `tunnel`: the tunnel's roles.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Tunnel {
    /// `hub`: run a hub.
    pub hub: Option<TunnelHub>,
    /// `node`: run a node.
    pub node: Option<TunnelNode>,
    /// `proxy`: run a proxy.
    pub proxy: Option<TunnelProxy>,
}

/// `tunnel.hub`.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct TunnelHub {
    /// `name`; the host's name when empty.
    pub name: String,
    /// `password`: deprecated for tokens.
    pub password: String,
    /// `tokens`: the tokens the hub issued.
    pub tokens: Vec<LinkToken>,
    /// `body_password`: AES-256-GCM on every frame's body.
    pub body_password: String,
    /// `listeners`.
    pub listeners: Vec<TunnelListener>,
    /// `access`.
    pub access: Option<Access>,
    /// `console_socket`.
    pub console_socket: Option<TunnelConsoleSocket>,
    /// `quic`.
    pub quic: Option<TunnelHubQuic>,
    /// `register_timeout`; 0, the default.
    pub register_timeout: Duration,
    /// `call_timeout`; 0, the default.
    pub call_timeout: Duration,
    /// `max_links`; 0, the default.
    pub max_links: usize,
    /// `max_unregistered`; 0, the default.
    pub max_unregistered: usize,
    /// `max_unregistered_per_ip`; 0, the default.
    pub max_unregistered_per_ip: usize,
    /// `max_links_per_peer`; 0, the default.
    pub max_links_per_peer: usize,
    /// `max_peers`; 0, the default.
    pub max_peers: usize,
    /// `max_tunnels`; 0, the default.
    pub max_tunnels: usize,
    /// `allow_proxy_services`.
    pub allow_proxy_services: bool,
    /// `console`: the hub's pages.
    pub console: Option<Console>,
}

/// A hub's listener.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct TunnelListener {
    /// `protocol`.
    pub protocol: TunnelProtocol,
    /// `address`.
    pub address: String,
    /// `path`: a ws or wss listener's; [`DEFAULT_TUNNEL_WS_PATH`] when empty.
    pub path: String,
    /// `cert_file`.
    pub cert_file: String,
    /// `key_file`.
    pub key_file: String,
}

impl TunnelListener {
    /// A ws or wss listener's path, its default when left out.
    #[must_use]
    pub fn ws_path(&self) -> &str {
        if self.path.is_empty() {
            DEFAULT_TUNNEL_WS_PATH
        } else {
            &self.path
        }
    }

    /// What tells listeners apart: two with the same key are the same listener.
    #[must_use]
    pub fn key(&self) -> String {
        let mut k = format!("{} {}", self.protocol.as_str(), self.address);
        if self.protocol.is_ws() {
            k.push(' ');
            k.push_str(self.ws_path());
        }
        k
    }

    /// The address the listener is dialed on, its scheme naming its protocol.
    #[must_use]
    pub fn url(&self) -> String {
        match self.protocol {
            TunnelProtocol::Tcp => self.address.clone(),
            TunnelProtocol::Ws | TunnelProtocol::Wss => {
                format!(
                    "{}://{}{}",
                    self.protocol.as_str(),
                    self.address,
                    self.ws_path()
                )
            }
            p => format!("{}://{}", p.as_str(), self.address),
        }
    }
}

/// A hub's `console_socket`.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct TunnelConsoleSocket {
    /// `address`.
    pub address: String,
    /// `password`; the hub's when empty.
    pub password: String,
    /// `max_conns`; 0, the default.
    pub max_conns: usize,
}

impl TunnelConsoleSocket {
    /// The most WebSockets open at once.
    #[must_use]
    pub fn conns(&self) -> usize {
        or(self.max_conns, DEFAULT_TUNNEL_PROXY_CONNS)
    }
}

/// A hub's `quic`.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct TunnelHubQuic {
    /// `cert`: the hub's own certificate, PEM.
    pub cert: String,
    /// `key`: its key, PEM.
    pub key: String,
    /// `idle_timeout`; 0, the default.
    pub idle_timeout: Duration,
    /// `keep_alive`; 0, the default.
    pub keep_alive: Duration,
    /// `handshake_timeout`; 0, the default.
    pub handshake_timeout: Duration,
    /// `max_handshakes`; 0, the default.
    pub max_handshakes: usize,
}

impl TunnelHubQuic {
    /// How long a QUIC connection may hear nothing.
    #[must_use]
    pub fn idle(q: Option<&TunnelHubQuic>) -> Duration {
        dur_or(q.map(|q| q.idle_timeout), DEFAULT_TUNNEL_QUIC_IDLE)
    }
    /// How often a quiet QUIC connection is pinged.
    #[must_use]
    pub fn ping(q: Option<&TunnelHubQuic>) -> Duration {
        dur_or(q.map(|q| q.keep_alive), DEFAULT_TUNNEL_QUIC_KEEP_ALIVE)
    }
    /// How long a QUIC handshake may take.
    #[must_use]
    pub fn handshake(q: Option<&TunnelHubQuic>) -> Duration {
        dur_or(
            q.map(|q| q.handshake_timeout),
            DEFAULT_TUNNEL_QUIC_HANDSHAKE,
        )
    }
    /// The most QUIC handshakes under way.
    #[must_use]
    pub fn handshakes(q: Option<&TunnelHubQuic>) -> usize {
        or(
            q.map_or(0, |q| q.max_handshakes),
            DEFAULT_TUNNEL_MAX_HANDSHAKES,
        )
    }
}

impl TunnelHub {
    /// The hub's name: its own, or the host's.
    #[must_use]
    pub fn hub_name(&self) -> String {
        name_or_host(&self.name)
    }
    /// `register_timeout`, its default when left out.
    #[must_use]
    pub fn register(&self) -> Duration {
        dur_or(Some(self.register_timeout), DEFAULT_TUNNEL_REGISTER_TIMEOUT)
    }
    /// `call_timeout`, its default when left out.
    #[must_use]
    pub fn call(&self) -> Duration {
        dur_or(Some(self.call_timeout), DEFAULT_TUNNEL_HUB_CALL)
    }
    /// `max_links`, its default when left out.
    #[must_use]
    pub fn links(&self) -> usize {
        or(self.max_links, DEFAULT_TUNNEL_MAX_LINKS)
    }
    /// `max_unregistered`, its default when left out.
    #[must_use]
    pub fn unregistered(&self) -> usize {
        or(self.max_unregistered, DEFAULT_TUNNEL_MAX_UNREGISTERED)
    }
    /// `max_unregistered_per_ip`, its default when left out.
    #[must_use]
    pub fn unregistered_per_ip(&self) -> usize {
        or(
            self.max_unregistered_per_ip,
            DEFAULT_TUNNEL_MAX_UNREGISTERED_PER_IP,
        )
    }
    /// `max_links_per_peer`, its default when left out.
    #[must_use]
    pub fn links_per_peer(&self) -> usize {
        or(self.max_links_per_peer, DEFAULT_TUNNEL_MAX_LINKS_PER_PEER)
    }
    /// `max_peers`, its default when left out.
    #[must_use]
    pub fn peers(&self) -> usize {
        or(self.max_peers, DEFAULT_TUNNEL_MAX_PEERS)
    }
    /// `max_tunnels`, its default when left out.
    #[must_use]
    pub fn tunnels(&self) -> usize {
        or(self.max_tunnels, DEFAULT_TUNNEL_HUB_TUNNELS)
    }
    /// The console socket's password: its own, or the hub's.
    #[must_use]
    pub fn socket_password(&self) -> &str {
        match &self.console_socket {
            Some(s) if !s.password.is_empty() => &s.password,
            _ => &self.password,
        }
    }
}

/// `tunnel.node`.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct TunnelNode {
    /// `name`; the host's name when empty.
    pub name: String,
    /// `meta`: anything else the node tells the hubs.
    pub meta: Vec<(String, String)>,
    /// `hubs`.
    pub hubs: Vec<TunnelUplink>,
    /// `services`.
    pub services: Vec<TunnelService>,
    /// `allow_hub_services`.
    pub allow_hub_services: bool,
    /// `dial_timeout`; 0, the default.
    pub dial_timeout: Duration,
    /// `idle_timeout`; 0, the default.
    pub idle_timeout: Duration,
    /// `max_tunnels`; 0, the default.
    pub max_tunnels: usize,
    /// `console`.
    pub console: Option<Console>,
}

impl TunnelNode {
    /// The node's name: its own, or the host's.
    #[must_use]
    pub fn node_name(&self) -> String {
        name_or_host(&self.name)
    }
    /// `dial_timeout`, its default when left out.
    #[must_use]
    pub fn dial(&self) -> Duration {
        dur_or(Some(self.dial_timeout), DEFAULT_TUNNEL_DIAL_TIMEOUT)
    }
    /// `idle_timeout`, its default when left out.
    #[must_use]
    pub fn idle(&self) -> Duration {
        dur_or(Some(self.idle_timeout), DEFAULT_TUNNEL_IDLE_TIMEOUT)
    }
    /// `max_tunnels`, its default when left out.
    #[must_use]
    pub fn tunnels(&self) -> usize {
        or(self.max_tunnels, DEFAULT_TUNNEL_TUNNELS)
    }
}

/// A service a node reaches.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct TunnelService {
    /// `name`.
    pub name: String,
    /// `address`: host and port.
    pub address: String,
    /// `description`.
    pub description: String,
}

/// `tunnel.proxy`.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct TunnelProxy {
    /// `name`; the host's name when empty.
    pub name: String,
    /// `meta`.
    pub meta: Vec<(String, String)>,
    /// `hubs`.
    pub hubs: Vec<TunnelUplink>,
    /// `forwards`.
    pub forwards: Vec<TunnelForward>,
    /// `web_proxies`.
    pub web_proxies: Vec<TunnelWebProxy>,
    /// `access`.
    pub access: Option<Access>,
    /// `open_timeout`; 0, the default.
    pub open_timeout: Duration,
    /// `idle_timeout`; 0, the default.
    pub idle_timeout: Duration,
    /// `max_tunnels`; 0, the default.
    pub max_tunnels: usize,
    /// `console`.
    pub console: Option<Console>,
}

impl TunnelProxy {
    /// The proxy's name: its own, or the host's.
    #[must_use]
    pub fn proxy_name(&self) -> String {
        name_or_host(&self.name)
    }
    /// `open_timeout`, its default when left out.
    #[must_use]
    pub fn open(&self) -> Duration {
        dur_or(Some(self.open_timeout), DEFAULT_TUNNEL_OPEN_TIMEOUT)
    }
    /// `idle_timeout`, its default when left out.
    #[must_use]
    pub fn idle(&self) -> Duration {
        dur_or(Some(self.idle_timeout), DEFAULT_TUNNEL_IDLE_TIMEOUT)
    }
    /// `max_tunnels`, its default when left out.
    #[must_use]
    pub fn tunnels(&self) -> usize {
        or(self.max_tunnels, DEFAULT_TUNNEL_TUNNELS)
    }
}

/// A proxy's forward.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct TunnelForward {
    /// `listen`.
    pub listen: String,
    /// `node`.
    pub node: String,
    /// `service`.
    pub service: String,
    /// `hub`: the hub's name; every hub that has the node when empty.
    pub hub: String,
}

/// A proxy's web proxy.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct TunnelWebProxy {
    /// `name`.
    pub name: String,
    /// `listen`.
    pub listen: String,
    /// `url`: the hub's console socket, `ws://host:port/`.
    pub url: String,
    /// `password`.
    pub password: String,
    /// `max_conns`; 0, the default.
    pub max_conns: usize,
}

impl TunnelWebProxy {
    /// The most connections open at once.
    #[must_use]
    pub fn conns(&self) -> usize {
        or(self.max_conns, DEFAULT_TUNNEL_PROXY_CONNS)
    }
}

/// A hub a node or proxy links to.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct TunnelUplink {
    /// `name`; the first endpoint's address when empty.
    pub name: String,
    /// `endpoints`.
    pub endpoints: Vec<TunnelEndpoint>,
    /// `token`.
    pub token: String,
    /// `password`: deprecated for `token`.
    pub password: String,
    /// `body_password`.
    pub body_password: String,
    /// `pin` (old name `quic_pin`).
    pub pin: String,
    /// `ca_file`.
    pub ca_file: String,
    /// `quic_insecure`.
    pub quic_insecure: bool,
    /// `pool_size`; 0, the default.
    pub pool_size: usize,
    /// `dial_timeout`; 0, the default.
    pub dial_timeout: Duration,
    /// `call_timeout`; 0, the default.
    pub call_timeout: Duration,
    /// `retry_interval`; 0, the default.
    pub retry_interval: Duration,
    /// `ping_interval`; 0, the default.
    pub ping_interval: Duration,
    /// `ping_timeout`; 0, the default.
    pub ping_timeout: Duration,
}

impl TunnelUplink {
    /// The uplink's name: its own, or its first endpoint's address.
    #[must_use]
    pub fn hub_name(&self) -> &str {
        if !self.name.is_empty() {
            return &self.name;
        }
        self.endpoints.first().map_or("", |e| e.address.as_str())
    }
    /// `pool_size`, its default when left out.
    #[must_use]
    pub fn pool(&self) -> usize {
        or(self.pool_size, DEFAULT_TUNNEL_POOL_SIZE)
    }
    /// `dial_timeout`, its default when left out.
    #[must_use]
    pub fn dial(&self) -> Duration {
        dur_or(Some(self.dial_timeout), DEFAULT_TUNNEL_DIAL_TIMEOUT)
    }
    /// `call_timeout`, its default when left out.
    #[must_use]
    pub fn call(&self) -> Duration {
        dur_or(Some(self.call_timeout), DEFAULT_TUNNEL_CALL_TIMEOUT)
    }
    /// `retry_interval`, its default when left out.
    #[must_use]
    pub fn retry(&self) -> Duration {
        dur_or(Some(self.retry_interval), DEFAULT_TUNNEL_RETRY_INTERVAL)
    }
    /// `ping_interval`, its default when left out.
    #[must_use]
    pub fn ping(&self) -> Duration {
        dur_or(Some(self.ping_interval), DEFAULT_TUNNEL_PING_INTERVAL)
    }
    /// `ping_timeout`, its default when left out.
    #[must_use]
    pub fn ping_wait(&self) -> Duration {
        dur_or(Some(self.ping_timeout), DEFAULT_TUNNEL_PING_TIMEOUT)
    }

    /// Whether an endpoint at `address` would carry the uplink's links unauthenticated past
    /// registration: tcp or ws to a host that is not a loopback one, without `body_password`.
    #[must_use]
    pub fn bare(&self, address: &str) -> bool {
        if !self.body_password.is_empty() {
            return false;
        }
        match endpoint_protocol(address) {
            Some(TunnelProtocol::Tcp | TunnelProtocol::Ws) => {
                parse_endpoint(address).is_none_or(|e| !loopback(&e.host))
            }
            _ => false,
        }
    }
}

/// One address of a hub.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct TunnelEndpoint {
    /// `address`.
    pub address: String,
    /// `disabled`.
    pub disabled: bool,
}

impl TunnelEndpoint {
    /// The protocol of the endpoint's address: its scheme, or tcp for `host:port`; `None` for a
    /// scheme that is none of the five.
    #[must_use]
    pub fn protocol(&self) -> Option<TunnelProtocol> {
        endpoint_protocol(&self.address)
    }
}

/// An endpoint's address, taken apart: what [`check_tunnel_endpoint`] lets through.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EndpointAddr {
    /// The protocol.
    pub protocol: TunnelProtocol,
    /// The host, without brackets.
    pub host: String,
    /// The port; 443 for a wss address without one, 80 for a ws address without one.
    pub port: u16,
    /// A ws or wss address's path, `/` when it has none; empty for the others.
    pub path: String,
}

impl EndpointAddr {
    /// `host:port`, with an IPv6 host in brackets, to dial.
    #[must_use]
    pub fn host_port(&self) -> String {
        if self.host.contains(':') {
            format!("[{}]:{}", self.host, self.port)
        } else {
            format!("{}:{}", self.host, self.port)
        }
    }
}

fn endpoint_protocol(address: &str) -> Option<TunnelProtocol> {
    match address.split_once("://") {
        Some((s, _)) => TunnelProtocol::parse(s),
        None => Some(TunnelProtocol::Tcp),
    }
}

/// Splits `host[:port]` (an IPv6 host in brackets); the port is `None` when left out, and the
/// whole is `None` when it is not one.
fn split_authority(auth: &str) -> Option<(String, Option<String>)> {
    if let Some(rest) = auth.strip_prefix('[') {
        let (h, after) = rest.split_once(']')?;
        let port = match after {
            "" => None,
            p => Some(p.strip_prefix(':')?.to_owned()),
        };
        return Some((h.to_owned(), port));
    }
    match auth.rsplit_once(':') {
        Some((h, p)) if !h.contains(':') => Some((h.to_owned(), Some(p.to_owned()))),
        Some(_) => None,
        None => Some((auth.to_owned(), None)),
    }
}

/// Takes an endpoint's address apart, or `None` when [`check_tunnel_endpoint`] refuses it.
#[must_use]
pub fn parse_endpoint(address: &str) -> Option<EndpointAddr> {
    if address.contains(['?', '#', '@']) || address.chars().any(char::is_whitespace) {
        return None;
    }
    let protocol = endpoint_protocol(address)?;
    if protocol == TunnelProtocol::Tcp {
        let hostport = address.strip_prefix("tcp://").unwrap_or(address);
        if hostport.contains('/') {
            return None;
        }
        let (host, port) = split_authority(hostport)?;
        let port = port?.parse::<u16>().ok().filter(|p| *p != 0)?;
        if host.is_empty() {
            return None;
        }
        return Some(EndpointAddr {
            protocol,
            host,
            port,
            path: String::new(),
        });
    }
    let (_, rest) = address.split_once("://")?;
    let (auth, path) = match rest.find('/') {
        Some(i) => (rest.get(..i)?, rest.get(i..)?),
        None => (rest, ""),
    };
    let (host, port) = split_authority(auth)?;
    if host.is_empty() {
        return None;
    }
    let port = match port.as_deref() {
        Some(p) if !p.is_empty() => Some(p.parse::<u16>().ok()?),
        _ => None,
    };
    let needs_port = matches!(protocol, TunnelProtocol::Quic | TunnelProtocol::Tls);
    let port = match (port, protocol) {
        (Some(p), _) => p,
        (None, TunnelProtocol::Wss) => 443,
        (None, TunnelProtocol::Ws) => 80,
        (None, _) => return None,
    };
    if needs_port && !path.is_empty() && path != "/" {
        return None;
    }
    let path = if protocol.is_ws() {
        if path.is_empty() { "/" } else { path }.to_owned()
    } else {
        String::new()
    };
    Some(EndpointAddr {
        protocol,
        host,
        port,
        path,
    })
}

/// Why an endpoint's address is not one, or `None`, as fit's `CheckTunnelEndpoint`. An address
/// is only where to dial: one with a query, a fragment or a user is refused, as nothing in it
/// may say how the hub's certificate is checked; `pin` and `ca_file` do.
#[must_use]
pub fn check_tunnel_endpoint(address: &str) -> Option<String> {
    if address.contains(['?', '#', '@']) {
        return Some(format!(
            "{address:?} has a query, a fragment or a user ('?', '#' or '@'), which an address \
             does not: the hub's certificate is checked by pin or ca_file"
        ));
    }
    let Some(p) = endpoint_protocol(address) else {
        let scheme = address.split_once("://").map_or("", |(s, _)| s);
        return Some(format!(
            "{address:?} has the scheme {scheme}: use tls://, wss://, quic://, or host:port and \
             ws:// with body_password"
        ));
    };
    if parse_endpoint(address).is_some() {
        return None;
    }
    Some(match p {
        TunnelProtocol::Tcp => {
            format!("{address:?} is not a host and port to dial, as in \"hub.example:7000\"")
        }
        TunnelProtocol::Tls | TunnelProtocol::Quic => {
            // A path where none goes, or no host and port.
            let rest = address.split_once("://").map_or("", |(_, r)| r);
            let has_path = rest.find('/').is_some_and(|i| {
                rest.get(i..).is_some_and(|p| p != "/")
                    && rest
                        .get(..i)
                        .and_then(split_authority)
                        .is_some_and(|(h, port)| !h.is_empty() && port.is_some())
            });
            if has_path {
                format!(
                    "{address:?} has a path, which a {}:// address does not",
                    p.as_str()
                )
            } else {
                format!(
                    "{address:?} is not a {}:// URL with a host and port",
                    p.as_str()
                )
            }
        }
        TunnelProtocol::Ws | TunnelProtocol::Wss => {
            format!(
                "{address:?} is not a {}:// URL with a host and port",
                p.as_str()
            )
        }
    })
}

/// Why `pin` is not a certificate pin, or `None`: the SHA-256 of a certificate in hex, 64
/// digits, colons and case ignored.
#[must_use]
pub fn check_pin(pin: &str) -> Option<String> {
    parse_pin(pin).is_none().then(|| {
        format!(
            "{pin:?} is not a certificate pin: the SHA-256 of the hub's certificate in hex, 64 \
             digits, as its pages show it"
        )
    })
}

/// A certificate pin's 32 bytes, as fit's `certpin.Parse`: 64 hex digits, colons, spaces
/// around it and case ignored.
#[must_use]
pub fn parse_pin(pin: &str) -> Option<[u8; 32]> {
    let hex: Vec<u8> = pin.trim().bytes().filter(|b| *b != b':').collect();
    if hex.len() != 64 {
        return None;
    }
    let mut out = [0u8; 32];
    for (o, pair) in out.iter_mut().zip(hex.chunks(2)) {
        let s = std::str::from_utf8(pair).ok()?;
        *o = u8::from_str_radix(s, 16).ok()?;
    }
    Some(out)
}

/// Whether `host`, a name or an IP address, is a loopback one: `localhost`, `127.0.0.0/8` or
/// `::1` (an IPv4-mapped loopback address too). An empty host, every address, is not.
#[must_use]
pub fn loopback(host: &str) -> bool {
    let h = host.trim_start_matches('[').trim_end_matches(']');
    h.eq_ignore_ascii_case("localhost")
        || h.parse::<IpAddr>()
            .is_ok_and(|ip| crate::model::unmap(ip).is_loopback())
}

/// The host of `host:port`, without brackets; `None` when it is not one.
fn host_of(address: &str) -> Option<String> {
    let (h, p) = split_authority(address)?;
    p?;
    Some(h)
}

/// Why a role's name is not one, or `None`.
fn check_tunnel_name(name: &str) -> Option<String> {
    (!name.is_empty() && name.trim() != name || name.contains(['/', '\n', '\r', '\t'])).then(|| {
        format!("{name:?} is not a name: no spaces around it, and no '/' or control characters")
    })
}

fn name_or_host(name: &str) -> String {
    if name.is_empty() {
        host_name()
    } else {
        name.to_owned()
    }
}

/// The host's name, as fit's `os.Hostname`: `akit` when it cannot be read (fit's `fit`,
/// renamed, decision 037). Read from the environment and `/proc` or `/etc/hostname`, without
/// a system call of akit's own.
#[must_use]
pub fn host_name() -> String {
    for var in ["HOSTNAME", "COMPUTERNAME"] {
        if let Ok(h) = std::env::var(var) {
            let h = h.trim();
            if !h.is_empty() {
                return h.to_owned();
            }
        }
    }
    for path in ["/proc/sys/kernel/hostname", "/etc/hostname"] {
        if let Ok(h) = std::fs::read_to_string(path) {
            let h = h.trim();
            if !h.is_empty() {
                return h.to_owned();
            }
        }
    }
    "akit".to_owned()
}

fn or(v: usize, default: usize) -> usize {
    if v == 0 { default } else { v }
}

fn dur_or(v: Option<Duration>, default: Duration) -> Duration {
    match v {
        Some(d) if !d.is_zero() => d,
        _ => default,
    }
}

// ---------------------------------------------------------------------------------------------
// The LLM feature.

/// The most peers a role dials.
pub const MAX_LLM_PEERS: usize = 64;
/// The most models a node serves.
pub const MAX_LLM_MODELS: usize = 256;
/// The most API keys a proxy keeps.
pub const MAX_LLM_KEYS: usize = 256;
/// The most links a role takes.
pub const DEFAULT_LLM_MAX_CONNS: usize = 1024;
/// The most calls and streams a peer runs on one link.
pub const DEFAULT_LLM_MAX_INFLIGHT: usize = 1024;
/// The most nodes in a hub's table.
pub const DEFAULT_LLM_MAX_NODES: usize = 1024;
/// The most calls a node forwards at once.
pub const DEFAULT_LLM_NODE_MAX_INFLIGHT: usize = 256;
/// The largest request body.
pub const DEFAULT_LLM_MAX_BODY_BYTES: u64 = 32 << 20;
/// The most connections a proxy's API keeps.
pub const DEFAULT_LLM_PROXY_MAX_CONNS: usize = 256;
/// The most calls a proxy's API forwards at once.
pub const DEFAULT_LLM_PROXY_MAX_INFLIGHT: usize = 256;
/// How long a link may hear nothing before it is pinged.
pub const DEFAULT_LLM_PING_INTERVAL: Duration = Duration::from_secs(15);
/// How long a ping may wait for its pong.
pub const DEFAULT_LLM_SLOW_TIMEOUT: Duration = Duration::from_secs(30);
/// The longest wait between dials.
pub const DEFAULT_LLM_RETRY_INTERVAL: Duration = Duration::from_secs(5);
/// How long a stream may carry nothing.
pub const DEFAULT_LLM_STREAM_IDLE: Duration = Duration::from_secs(120);
/// How often a node reads its hardware.
pub const DEFAULT_LLM_REFRESH_INTERVAL: Duration = Duration::from_secs(30);
/// How long a job may run.
pub const DEFAULT_LLM_JOB_TIMEOUT: Duration = Duration::from_secs(60 * 60);
/// How long a connection to an upstream may take.
pub const DEFAULT_LLM_CONNECT_TIMEOUT: Duration = Duration::from_secs(10);
/// How long an upstream may take to begin its response.
pub const DEFAULT_LLM_FIRST_BYTE_TIMEOUT: Duration = Duration::from_secs(5 * 60);
/// How long an upstream may go silent.
pub const DEFAULT_LLM_IDLE_TIMEOUT: Duration = Duration::from_secs(5 * 60);
/// The most one call may take.
pub const DEFAULT_LLM_CALL_TIMEOUT: Duration = Duration::from_secs(30 * 60);
/// The APIs a model's upstream may speak.
pub const LLM_APIS: &[&str] = &[
    "openai",
    "anthropic",
    "gemini",
    "azure_openai",
    "ollama",
    "cohere",
];

/// The API agents call a model of `api` with: `azure_openai`'s is `openai`'s.
#[must_use]
pub fn llm_protocol(api: &str) -> &str {
    if api == "azure_openai" { "openai" } else { api }
}

/// The roles a role of the LLM feature links with.
#[must_use]
pub fn llm_peer_roles(role: &str) -> &'static [&'static str] {
    match role {
        "hub" => &["node", "proxy"],
        "node" => &["hub", "proxy"],
        "proxy" => &["hub", "node"],
        _ => &[],
    }
}

/// `llm`: the LLM feature's roles.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Llm {
    /// `hub`.
    pub hub: Option<LlmHub>,
    /// `node`.
    pub node: Option<LlmNode>,
    /// `proxy`.
    pub proxy: Option<LlmProxy>,
}

/// What every LLM role has.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct LlmRole {
    /// `id`: made at first start when empty.
    pub id: String,
    /// `name`: the host's name when empty.
    pub name: String,
    /// `console`.
    pub console: Option<Console>,
    /// `listen`.
    pub listen: Option<LlmListen>,
    /// `access`.
    pub access: Option<Access>,
    /// `tokens`.
    pub tokens: Vec<LinkToken>,
    /// `peers`.
    pub peers: Vec<LlmPeer>,
    /// `limits`.
    pub limits: Option<LlmLimits>,
}

/// Where a role takes links from its peers.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct LlmListen {
    /// `address`.
    pub address: String,
    /// `password`: deprecated.
    pub password: String,
    /// `node_password`: deprecated.
    pub node_password: String,
    /// `proxy_password`: deprecated.
    pub proxy_password: String,
    /// `hub_password`: deprecated.
    pub hub_password: String,
    /// `cert_file`.
    pub cert_file: String,
    /// `key_file`.
    pub key_file: String,
    /// `self_signed`.
    pub self_signed: bool,
    /// `cert`: the self-signed certificate the role made, PEM.
    pub cert: String,
    /// `key`: its key, PEM.
    pub key: String,
}

impl LlmListen {
    /// Whether the listener takes links over TLS.
    #[must_use]
    pub fn tls(&self) -> bool {
        !self.cert_file.is_empty() || !self.cert.is_empty() || self.self_signed
    }
}

/// A peer a role dials.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct LlmPeer {
    /// `address`.
    pub address: String,
    /// `token`.
    pub token: String,
    /// `password`: deprecated.
    pub password: String,
    /// `role`: `hub`, `node` or `proxy`.
    pub role: String,
    /// `pin`.
    pub pin: String,
    /// `ca_file`.
    pub ca_file: String,
    /// `server_name`.
    pub server_name: String,
}

/// A role's `limits`.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct LlmLimits {
    /// `max_conns`; 0, the default.
    pub max_conns: usize,
    /// `max_inflight`; 0, the default.
    pub max_inflight: usize,
    /// `ping_interval`; 0, the default.
    pub ping_interval: Duration,
    /// `slow_timeout`; 0, the default.
    pub slow_timeout: Duration,
    /// `retry_interval`; 0, the default.
    pub retry_interval: Duration,
    /// `stream_idle`; 0, the default.
    pub stream_idle: Duration,
}

impl LlmLimits {
    /// `max_conns`, its default when left out.
    #[must_use]
    pub fn conns(l: Option<&LlmLimits>) -> usize {
        or(l.map_or(0, |l| l.max_conns), DEFAULT_LLM_MAX_CONNS)
    }
    /// `max_inflight`, its default when left out.
    #[must_use]
    pub fn inflight(l: Option<&LlmLimits>) -> usize {
        or(l.map_or(0, |l| l.max_inflight), DEFAULT_LLM_MAX_INFLIGHT)
    }
    /// `ping_interval`, its default when left out.
    #[must_use]
    pub fn ping(l: Option<&LlmLimits>) -> Duration {
        dur_or(l.map(|l| l.ping_interval), DEFAULT_LLM_PING_INTERVAL)
    }
    /// `slow_timeout`, its default when left out.
    #[must_use]
    pub fn slow(l: Option<&LlmLimits>) -> Duration {
        dur_or(l.map(|l| l.slow_timeout), DEFAULT_LLM_SLOW_TIMEOUT)
    }
    /// `retry_interval`, its default when left out.
    #[must_use]
    pub fn retry(l: Option<&LlmLimits>) -> Duration {
        dur_or(l.map(|l| l.retry_interval), DEFAULT_LLM_RETRY_INTERVAL)
    }
    /// `stream_idle`, its default when left out.
    #[must_use]
    pub fn idle(l: Option<&LlmLimits>) -> Duration {
        dur_or(l.map(|l| l.stream_idle), DEFAULT_LLM_STREAM_IDLE)
    }
}

/// `llm.hub`.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct LlmHub {
    /// What every role has.
    pub role: LlmRole,
    /// `max_nodes`; 0, the default.
    pub max_nodes: usize,
}

impl LlmHub {
    /// `max_nodes`, its default when left out.
    #[must_use]
    pub fn nodes(&self) -> usize {
        or(self.max_nodes, DEFAULT_LLM_MAX_NODES)
    }
}

/// `llm.node`.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct LlmNode {
    /// What every role has.
    pub role: LlmRole,
    /// `description`.
    pub description: String,
    /// `models`.
    pub models: Vec<LlmModel>,
    /// `settings`.
    pub settings: Option<LlmNodeSettings>,
    /// `proxy`.
    pub proxy: Option<LlmNodeProxy>,
}

/// A model a node serves.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct LlmModel {
    /// `name`.
    pub name: String,
    /// `api`: one of [`LLM_APIS`].
    pub api: String,
    /// `base_url`.
    pub base_url: String,
    /// `upstream`: `name` when empty.
    pub upstream: String,
    /// `api_key`.
    pub api_key: String,
    /// `api_version`: an `azure_openai` model's.
    pub api_version: String,
    /// `disabled`.
    pub disabled: bool,
    /// `description`.
    pub description: String,
    /// `max_inflight`; 0, none of its own.
    pub max_inflight: usize,
}

/// A node's `settings`.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct LlmNodeSettings {
    /// `allow_hub_manage`.
    pub allow_hub_manage: bool,
    /// `gpu_sudo`; true when left out.
    pub gpu_sudo: Option<bool>,
    /// `refresh_interval`; 0, the default.
    pub refresh_interval: Duration,
    /// `job_timeout`; 0, the default.
    pub job_timeout: Duration,
    /// `gpu_data_dir`.
    pub gpu_data_dir: String,
}

impl LlmNodeSettings {
    /// `gpu_sudo`, true when left out.
    #[must_use]
    pub fn sudo(s: Option<&LlmNodeSettings>) -> bool {
        s.and_then(|s| s.gpu_sudo).unwrap_or(true)
    }
    /// `refresh_interval`, its default when left out.
    #[must_use]
    pub fn refresh(s: Option<&LlmNodeSettings>) -> Duration {
        dur_or(s.map(|s| s.refresh_interval), DEFAULT_LLM_REFRESH_INTERVAL)
    }
    /// `job_timeout`, its default when left out.
    #[must_use]
    pub fn job(s: Option<&LlmNodeSettings>) -> Duration {
        dur_or(s.map(|s| s.job_timeout), DEFAULT_LLM_JOB_TIMEOUT)
    }
}

/// A node's `proxy`: the bounds of the calls it forwards upstream.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct LlmNodeProxy {
    /// `connect_timeout`; 0, the default.
    pub connect_timeout: Duration,
    /// `first_byte_timeout`; 0, the default.
    pub first_byte_timeout: Duration,
    /// `idle_timeout`; 0, the default.
    pub idle_timeout: Duration,
    /// `timeout`; 0, the default.
    pub timeout: Duration,
    /// `max_inflight`; 0, the default.
    pub max_inflight: usize,
    /// `max_body_bytes`; 0, the default.
    pub max_body_bytes: u64,
}

impl LlmNodeProxy {
    /// `connect_timeout`, its default when left out.
    #[must_use]
    pub fn connect(p: Option<&LlmNodeProxy>) -> Duration {
        dur_or(p.map(|p| p.connect_timeout), DEFAULT_LLM_CONNECT_TIMEOUT)
    }
    /// `first_byte_timeout`, its default when left out.
    #[must_use]
    pub fn first_byte(p: Option<&LlmNodeProxy>) -> Duration {
        dur_or(
            p.map(|p| p.first_byte_timeout),
            DEFAULT_LLM_FIRST_BYTE_TIMEOUT,
        )
    }
    /// `idle_timeout`, its default when left out.
    #[must_use]
    pub fn idle(p: Option<&LlmNodeProxy>) -> Duration {
        dur_or(p.map(|p| p.idle_timeout), DEFAULT_LLM_IDLE_TIMEOUT)
    }
    /// `timeout`, its default when left out.
    #[must_use]
    pub fn total(p: Option<&LlmNodeProxy>) -> Duration {
        dur_or(p.map(|p| p.timeout), DEFAULT_LLM_CALL_TIMEOUT)
    }
    /// `max_inflight`, its default when left out.
    #[must_use]
    pub fn inflight(p: Option<&LlmNodeProxy>) -> usize {
        or(
            p.map_or(0, |p| p.max_inflight),
            DEFAULT_LLM_NODE_MAX_INFLIGHT,
        )
    }
    /// `max_body_bytes`, its default when left out.
    #[must_use]
    pub fn body(p: Option<&LlmNodeProxy>) -> u64 {
        match p.map_or(0, |p| p.max_body_bytes) {
            0 => DEFAULT_LLM_MAX_BODY_BYTES,
            n => n,
        }
    }
}

/// `llm.proxy`.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct LlmProxy {
    /// What every role has.
    pub role: LlmRole,
    /// `api`.
    pub api: Option<LlmProxyApi>,
    /// `api_keys`.
    pub api_keys: Vec<LlmProxyKey>,
}

/// A proxy's `api`.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct LlmProxyApi {
    /// `address`.
    pub address: String,
    /// `cert_file`.
    pub cert_file: String,
    /// `key_file`.
    pub key_file: String,
    /// `max_body_bytes`; 0, the default.
    pub max_body_bytes: u64,
    /// `max_conns`; 0, the default.
    pub max_conns: usize,
    /// `max_inflight`; 0, the default.
    pub max_inflight: usize,
    /// `first_byte_timeout`; 0, the default.
    pub first_byte_timeout: Duration,
    /// `timeout`; 0, the default.
    pub timeout: Duration,
}

impl LlmProxyApi {
    /// `max_body_bytes`, its default when left out.
    #[must_use]
    pub fn body(&self) -> u64 {
        if self.max_body_bytes == 0 {
            DEFAULT_LLM_MAX_BODY_BYTES
        } else {
            self.max_body_bytes
        }
    }
    /// `max_conns`, its default when left out.
    #[must_use]
    pub fn conns(&self) -> usize {
        or(self.max_conns, DEFAULT_LLM_PROXY_MAX_CONNS)
    }
    /// `max_inflight`, its default when left out.
    #[must_use]
    pub fn inflight(&self) -> usize {
        or(self.max_inflight, DEFAULT_LLM_PROXY_MAX_INFLIGHT)
    }
    /// `first_byte_timeout`, its default when left out.
    #[must_use]
    pub fn first_byte(&self) -> Duration {
        dur_or(
            Some(self.first_byte_timeout),
            DEFAULT_LLM_FIRST_BYTE_TIMEOUT,
        )
    }
    /// `timeout`, its default when left out.
    #[must_use]
    pub fn total(&self) -> Duration {
        dur_or(Some(self.timeout), DEFAULT_LLM_CALL_TIMEOUT)
    }
}

/// An API key agents call a proxy with; only its SHA-256 is kept.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct LlmProxyKey {
    /// `id`: 16 hex digits.
    pub id: String,
    /// `name`.
    pub name: String,
    /// `prefix`: the key's first characters.
    pub prefix: String,
    /// `hash`: its SHA-256 in hex.
    pub hash: String,
    /// `created_at`, seconds since 1970.
    pub created_at: i64,
}

// ---------------------------------------------------------------------------------------------
// Decoding.

const TUNNEL_FIELDS: &[&str] = &["hub", "node", "proxy"];
const TUNNEL_HUB_FIELDS: &[&str] = &[
    "name",
    "password",
    "tokens",
    "body_password",
    "listeners",
    "access",
    "console_socket",
    "quic",
    "register_timeout",
    "call_timeout",
    "max_links",
    "max_unregistered",
    "max_unregistered_per_ip",
    "max_links_per_peer",
    "max_peers",
    "max_tunnels",
    "allow_proxy_services",
    "console",
];
const TUNNEL_LISTENER_FIELDS: &[&str] = &["protocol", "address", "path", "cert_file", "key_file"];
const TUNNEL_SOCKET_FIELDS: &[&str] = &["address", "password", "max_conns"];
const TUNNEL_QUIC_FIELDS: &[&str] = &[
    "cert",
    "key",
    "idle_timeout",
    "keep_alive",
    "handshake_timeout",
    "max_handshakes",
];
const TUNNEL_NODE_FIELDS: &[&str] = &[
    "name",
    "meta",
    "hubs",
    "services",
    "allow_hub_services",
    "dial_timeout",
    "idle_timeout",
    "max_tunnels",
    "console",
];
const TUNNEL_SERVICE_FIELDS: &[&str] = &["name", "address", "description"];
const TUNNEL_PROXY_FIELDS: &[&str] = &[
    "name",
    "meta",
    "hubs",
    "forwards",
    "web_proxies",
    "access",
    "open_timeout",
    "idle_timeout",
    "max_tunnels",
    "console",
];
const TUNNEL_FORWARD_FIELDS: &[&str] = &["listen", "node", "service", "hub"];
const TUNNEL_WEB_PROXY_FIELDS: &[&str] = &["name", "listen", "url", "password", "max_conns"];
const TUNNEL_UPLINK_FIELDS: &[&str] = &[
    "name",
    "endpoints",
    "token",
    "password",
    "body_password",
    "pin",
    "quic_pin",
    "ca_file",
    "quic_insecure",
    "pool_size",
    "dial_timeout",
    "call_timeout",
    "retry_interval",
    "ping_interval",
    "ping_timeout",
];
const TUNNEL_ENDPOINT_FIELDS: &[&str] = &["address", "disabled"];
const LINK_TOKEN_FIELDS: &[&str] = &[
    "id",
    "name",
    "hash",
    "created_at",
    "expires_at",
    "note",
    "members",
];
const LLM_FIELDS: &[&str] = &["hub", "node", "proxy"];
// What every role has, then each role's own fields.
const LLM_HUB_FIELDS: &[&str] = &[
    "id",
    "name",
    "console",
    "listen",
    "access",
    "tokens",
    "peers",
    "limits",
    "max_nodes",
];
const LLM_NODE_FIELDS: &[&str] = &[
    "id",
    "name",
    "console",
    "listen",
    "access",
    "tokens",
    "peers",
    "limits",
    "description",
    "models",
    "settings",
    "proxy",
];
const LLM_PROXY_FIELDS: &[&str] = &[
    "id", "name", "console", "listen", "access", "tokens", "peers", "limits", "api", "api_keys",
];
const LLM_LISTEN_FIELDS: &[&str] = &[
    "address",
    "password",
    "node_password",
    "proxy_password",
    "hub_password",
    "cert_file",
    "key_file",
    "self_signed",
    "cert",
    "key",
];
const LLM_PEER_FIELDS: &[&str] = &[
    "address",
    "token",
    "password",
    "role",
    "pin",
    "ca_file",
    "server_name",
];
const LLM_LIMITS_FIELDS: &[&str] = &[
    "max_conns",
    "max_inflight",
    "ping_interval",
    "slow_timeout",
    "retry_interval",
    "stream_idle",
];
const LLM_MODEL_FIELDS: &[&str] = &[
    "name",
    "api",
    "base_url",
    "upstream",
    "api_key",
    "api_version",
    "disabled",
    "description",
    "max_inflight",
];
const LLM_SETTINGS_FIELDS: &[&str] = &[
    "allow_hub_manage",
    "gpu_sudo",
    "refresh_interval",
    "job_timeout",
    "gpu_data_dir",
];
const LLM_NODE_PROXY_FIELDS: &[&str] = &[
    "connect_timeout",
    "first_byte_timeout",
    "idle_timeout",
    "timeout",
    "max_inflight",
    "max_body_bytes",
];
const LLM_API_FIELDS: &[&str] = &[
    "address",
    "cert_file",
    "key_file",
    "max_body_bytes",
    "max_conns",
    "max_inflight",
    "first_byte_timeout",
    "timeout",
];
const LLM_KEY_FIELDS: &[&str] = &["id", "name", "prefix", "hash", "created_at"];

/// A string member; empty when left out.
pub(crate) fn text(d: &mut Decoder<'_>, o: &mut Obj<'_>, key: &str) -> String {
    o.take(key)
        .and_then(|v| d.string(v, &o.at(key)))
        .unwrap_or_default()
}

/// A string member and where its value is; `None` when left out.
pub(crate) fn text_at(d: &mut Decoder<'_>, o: &mut Obj<'_>, key: &str) -> Option<(String, usize)> {
    let v = o.take(key)?;
    d.string(v, &o.at(key)).map(|s| (s, v.off))
}

/// A required string member, and where it is (the object's position when it is missing).
fn required(d: &mut Decoder<'_>, o: &mut Obj<'_>, key: &str) -> (String, usize) {
    match o.require(d, key) {
        Some(v) => (d.string(v, &o.at(key)).unwrap_or_default(), v.off),
        None => (String::new(), o.off),
    }
}

/// A duration member; zero when left out.
pub(crate) fn duration(d: &mut Decoder<'_>, o: &mut Obj<'_>, key: &str) -> Duration {
    o.take(key)
        .and_then(|v| d.duration(v, &o.at(key)))
        .unwrap_or_default()
}

/// A count, at least 0 and at most `max`; 0 when left out.
pub(crate) fn count(d: &mut Decoder<'_>, o: &mut Obj<'_>, key: &str, max: i64) -> usize {
    o.take(key)
        .and_then(|v| d.int(v, &o.at(key), 0, max))
        .and_then(|n| usize::try_from(n).ok())
        .unwrap_or(0)
}

/// A number of bytes, at least 0; 0 when left out.
pub(crate) fn bytes(d: &mut Decoder<'_>, o: &mut Obj<'_>, key: &str) -> u64 {
    o.take(key)
        .and_then(|v| d.int(v, &o.at(key), 0, i64::MAX))
        .and_then(|n| u64::try_from(n).ok())
        .unwrap_or(0)
}

/// A time in seconds since 1970, at least 0; 0 when left out.
fn seconds(d: &mut Decoder<'_>, o: &mut Obj<'_>, key: &str) -> i64 {
    o.take(key)
        .and_then(|v| d.int(v, &o.at(key), 0, i64::MAX))
        .unwrap_or(0)
}

/// A boolean member; false when left out.
pub(crate) fn boolean(d: &mut Decoder<'_>, o: &mut Obj<'_>, key: &str) -> bool {
    bool_opt(d, o, key).unwrap_or(false)
}

pub(crate) fn bool_opt(d: &mut Decoder<'_>, o: &mut Obj<'_>, key: &str) -> Option<bool> {
    let v = o.take(key)?;
    match &v.kind {
        Kind::Bool(b) => Some(*b),
        other => {
            d.error(
                v.off,
                &o.at(key),
                format!("expected a boolean, got {}", other.name()),
            );
            None
        }
    }
}

/// An array member, each element read by `f` with its index's path and position.
pub(crate) fn list<T>(
    d: &mut Decoder<'_>,
    o: &mut Obj<'_>,
    key: &str,
    mut f: impl FnMut(&mut Decoder<'_>, &Node, &str) -> Option<T>,
) -> Vec<(T, usize, String)> {
    let Some(v) = o.take(key) else {
        return Vec::new();
    };
    let p = o.at(key);
    d.array(v, &p, |d, n, p| {
        f(d, n, p).map(|t| (t, n.off, p.to_owned()))
    })
}

/// A role's `console`: `admin.console`'s fields and checks, its `address` required.
pub(crate) fn role_console(d: &mut Decoder<'_>, o: &mut Obj<'_>) -> Option<Console> {
    let v = o.take("console")?;
    let p = o.at("console");
    if let Kind::Object(members) = &v.kind {
        if !members.iter().any(|m| m.key == "address") {
            d.error(v.off, &join_field(&p, "address"), "missing required field");
        }
    }
    crate::model::console(d, v, &p)
}

pub(crate) fn access_of(d: &mut Decoder<'_>, o: &mut Obj<'_>) -> Option<Access> {
    let v = o.take("access")?;
    crate::model::access(d, v, &o.at("access"))
}

/// A listener's address: a host and port, as fit's `checkAddress`.
fn check_listen(d: &mut Decoder<'_>, address: &str, off: usize, path: &str) -> bool {
    match crate::model::check_address(address) {
        Ok(()) => true,
        Err(msg) => {
            d.error(off, path, msg);
            false
        }
    }
}

/// Decodes `tunnel`.
pub(crate) fn tunnel(d: &mut Decoder<'_>, n: &Node, path: &str) -> Option<Tunnel> {
    let mut o = d.object(n, path)?;
    let hub = o
        .take("hub")
        .and_then(|v| tunnel_hub(d, v, &join_field(path, "hub")));
    let node = o
        .take("node")
        .and_then(|v| tunnel_node(d, v, &join_field(path, "node")));
    let proxy = o
        .take("proxy")
        .and_then(|v| tunnel_proxy(d, v, &join_field(path, "proxy")));
    o.finish(d, TUNNEL_FIELDS, &[]);
    Some(Tunnel { hub, node, proxy })
}

fn tunnel_hub(d: &mut Decoder<'_>, n: &Node, path: &str) -> Option<TunnelHub> {
    let mut o = d.object(n, path)?;
    let mut h = TunnelHub::default();
    if let Some((name, off)) = text_at(d, &mut o, "name") {
        if let Some(msg) = check_tunnel_name(&name) {
            d.error(off, &o.at("name"), msg);
        }
        h.name = name;
    }
    h.password = text(d, &mut o, "password");
    h.tokens = link_tokens(d, &mut o);
    h.body_password = text(d, &mut o, "body_password");
    let listeners = list(d, &mut o, "listeners", tunnel_listener);
    h.access = access_of(d, &mut o);
    let socket = o.take("console_socket").and_then(|v| {
        let p = o.at("console_socket");
        tunnel_socket(d, v, &p).map(|s| (s, v.off, p))
    });
    h.quic = o
        .take("quic")
        .and_then(|v| tunnel_quic(d, v, &o.at("quic")));
    h.register_timeout = duration(d, &mut o, "register_timeout");
    h.call_timeout = duration(d, &mut o, "call_timeout");
    h.max_links = count(d, &mut o, "max_links", i64::MAX);
    h.max_unregistered = count(d, &mut o, "max_unregistered", i64::MAX);
    h.max_unregistered_per_ip = count(d, &mut o, "max_unregistered_per_ip", i64::MAX);
    h.max_links_per_peer = count(d, &mut o, "max_links_per_peer", i64::MAX);
    h.max_peers = count(d, &mut o, "max_peers", i64::MAX);
    h.max_tunnels = count(d, &mut o, "max_tunnels", i64::MAX);
    h.allow_proxy_services = boolean(d, &mut o, "allow_proxy_services");
    h.console = role_console(d, &mut o);
    o.finish(d, TUNNEL_HUB_FIELDS, &[]);
    // tcp and ws carry the frames after registration unauthenticated but for body_password:
    // anyone who takes the hub's address could relay a node's registration and then send it
    // what it likes.
    for (l, _, at) in &listeners {
        if matches!(l.protocol, TunnelProtocol::Tcp | TunnelProtocol::Ws)
            && h.body_password.is_empty()
            && host_of(&l.address).is_some_and(|h| !loopback(&h))
        {
            d.error(
                n.off,
                &join_field(at, "protocol"),
                format!(
                    "a {} listener on {:?}, which is not a loopback address, carries the \
                     links in the clear and unauthenticated: use tls or wss, with the hub's \
                     certificate pinned by the nodes and proxies, or quic; or set the hub's \
                     body_password",
                    l.protocol.as_str(),
                    l.address
                ),
            );
        }
    }
    h.listeners = listeners.into_iter().map(|(l, _, _)| l).collect();
    if let Some((s, off, p)) = socket {
        if h.console.is_none() {
            d.error(
                off,
                &p,
                "console_socket carries the console, which this hub lacks",
            );
        }
        if h.password.is_empty() && s.password.is_empty() {
            d.error(
                off,
                &join_field(&p, "password"),
                "a password is needed, its own or the hub's",
            );
        }
        h.console_socket = Some(s);
    }
    Some(h)
}

fn tunnel_listener(d: &mut Decoder<'_>, n: &Node, path: &str) -> Option<TunnelListener> {
    let mut o = d.object(n, path)?;
    let mut l = TunnelListener::default();
    let mut ok = true;
    match o.require(d, "protocol") {
        Some(v) => {
            let p = o.at("protocol");
            if let Some(s) = d.string(v, &p) {
                match TunnelProtocol::parse(&s) {
                    Some(proto) => l.protocol = proto,
                    None => {
                        d.error(
                            v.off,
                            &p,
                            format!("{s:?} is not one of tcp, tls, ws, wss and quic"),
                        );
                        ok = false;
                    }
                }
            }
        }
        None => ok = false,
    }
    let (address, off) = required(d, &mut o, "address");
    if off != o.off {
        d.address_at(&o.at("address"), off);
        check_listen(d, &address, off, &o.at("address"));
    }
    l.address = address;
    if let Some((p, off)) = text_at(d, &mut o, "path") {
        if !p.is_empty() && ok && !l.protocol.is_ws() {
            d.error(off, &o.at("path"), "only a ws or wss listener has a path");
        } else if !p.is_empty() && !p.starts_with('/') {
            d.error(off, &o.at("path"), format!("{p:?} does not start with '/'"));
        }
        l.path = p;
    }
    l.cert_file = text(d, &mut o, "cert_file");
    l.key_file = text(d, &mut o, "key_file");
    if (!l.cert_file.is_empty() || !l.key_file.is_empty()) && ok && !l.protocol.takes_tls() {
        d.error(
            n.off,
            &o.at("cert_file"),
            "only a tls, wss or quic listener has a certificate",
        );
    } else if l.cert_file.is_empty() != l.key_file.is_empty() {
        d.error(n.off, path, "cert_file and key_file go together");
    }
    o.finish(d, TUNNEL_LISTENER_FIELDS, &[]);
    ok.then_some(l)
}

fn tunnel_socket(d: &mut Decoder<'_>, n: &Node, path: &str) -> Option<TunnelConsoleSocket> {
    let mut o = d.object(n, path)?;
    let (address, off) = required(d, &mut o, "address");
    if off != o.off {
        d.address_at(&o.at("address"), off);
        check_listen(d, &address, off, &o.at("address"));
    }
    let password = text(d, &mut o, "password");
    let max_conns = count(d, &mut o, "max_conns", i64::MAX);
    o.finish(d, TUNNEL_SOCKET_FIELDS, &[]);
    Some(TunnelConsoleSocket {
        address,
        password,
        max_conns,
    })
}

fn tunnel_quic(d: &mut Decoder<'_>, n: &Node, path: &str) -> Option<TunnelHubQuic> {
    let mut o = d.object(n, path)?;
    let q = TunnelHubQuic {
        cert: text(d, &mut o, "cert"),
        key: text(d, &mut o, "key"),
        idle_timeout: duration(d, &mut o, "idle_timeout"),
        keep_alive: duration(d, &mut o, "keep_alive"),
        handshake_timeout: duration(d, &mut o, "handshake_timeout"),
        max_handshakes: count(d, &mut o, "max_handshakes", i64::MAX),
    };
    o.finish(d, TUNNEL_QUIC_FIELDS, &[]);
    if q.cert.is_empty() != q.key.is_empty() {
        d.error(n.off, path, "cert and key go together");
    } else if !q.cert.is_empty() {
        if let Some(msg) = key_pair_problem(&q.cert, &q.key) {
            d.error(
                n.off,
                &join_field(path, "cert"),
                format!("the certificate and key do not load: {msg}"),
            );
        }
    }
    Some(q)
}

/// What keeps a PEM certificate and key from loading, or `None`: a certificate block and a
/// private key akio-crypto reads. Whether the key is the certificate's is checked when the
/// role loads them.
fn key_pair_problem(cert: &str, key: &str) -> Option<String> {
    match akio_crypto::pem::decode_label(cert.as_bytes(), "CERTIFICATE") {
        Ok(blocks) if !blocks.is_empty() => {}
        _ => return Some("no certificate in the PEM".to_owned()),
    }
    match akio_crypto::pkey::PrivateKey::from_pem(key.as_bytes()) {
        Ok(_) => None,
        Err(e) => Some(format!("the key: {e}")),
    }
}

fn link_tokens(d: &mut Decoder<'_>, o: &mut Obj<'_>) -> Vec<LinkToken> {
    let Some(v) = o.take("tokens") else {
        return Vec::new();
    };
    let p = o.at("tokens");
    let tokens = d.array(v, &p, |d, n, p| {
        link_token(d, n, p).map(|t| (t, n.off, p.to_owned()))
    });
    if tokens.len() > MAX_LINK_TOKENS {
        d.error(v.off, &p, format!("at most {MAX_LINK_TOKENS} tokens"));
    }
    let mut members: Vec<(String, String)> = Vec::new();
    for (i, (t, off, at)) in tokens.iter().enumerate() {
        if !valid_link_token_id(&t.id) {
            d.error(
                *off,
                &join_field(at, "id"),
                "the ID is 16 lowercase hex digits",
            );
        } else if tokens.iter().take(i).any(|(o, _, _)| o.id == t.id) {
            d.error(
                *off,
                &join_field(at, "id"),
                format!("another token has the ID {}", t.id),
            );
        }
        let chars = t.name.chars().count();
        if chars == 0 || chars > 64 || t.name.chars().any(char::is_control) {
            d.error(
                *off,
                &join_field(at, "name"),
                "the name is 1 to 64 characters, without control characters",
            );
        }
        if t.hash.len() != 64 || !t.hash.bytes().all(|b| b.is_ascii_hexdigit()) {
            d.error(*off, &join_field(at, "hash"), "the hash is 64 hex digits");
        }
        if t.note.len() > 1024 {
            d.error(
                *off,
                &join_field(at, "note"),
                "the note is up to 1024 bytes",
            );
        }
        let mp = join_field(at, "members");
        if t.members.len() > MAX_LINK_TOKEN_MEMBERS {
            d.error(
                *off,
                &mp,
                format!("at most {MAX_LINK_TOKEN_MEMBERS} members"),
            );
        }
        for (j, m) in t.members.iter().enumerate() {
            if m.is_empty() || m.len() > 256 || m.chars().any(char::is_control) {
                d.error(
                    *off,
                    &join_index(&mp, j),
                    "a member is 1 to 256 bytes without control characters",
                );
                continue;
            }
            if let Some((_, other)) = members.iter().find(|(name, _)| name == m) {
                d.error(
                    *off,
                    &join_index(&mp, j),
                    format!("{m} is bound to token {other} already"),
                );
                continue;
            }
            members.push((m.clone(), t.id.clone()));
        }
    }
    tokens.into_iter().map(|(t, _, _)| t).collect()
}

fn link_token(d: &mut Decoder<'_>, n: &Node, path: &str) -> Option<LinkToken> {
    let mut o = d.object(n, path)?;
    let (id, _) = required(d, &mut o, "id");
    let (name, _) = required(d, &mut o, "name");
    let (hash, _) = required(d, &mut o, "hash");
    let created_at = seconds(d, &mut o, "created_at");
    let expires_at = seconds(d, &mut o, "expires_at");
    let note = text(d, &mut o, "note");
    let members = o
        .take("members")
        .map(|v| {
            d.strings(v, &o.at("members"))
                .into_iter()
                .map(|(_, s)| s)
                .collect()
        })
        .unwrap_or_default();
    o.finish(d, LINK_TOKEN_FIELDS, &[]);
    Some(LinkToken {
        id,
        name,
        hash,
        created_at,
        expires_at,
        note,
        members,
    })
}

fn meta(d: &mut Decoder<'_>, o: &mut Obj<'_>) -> Vec<(String, String)> {
    o.take("meta")
        .map(|v| {
            d.string_map(v, &o.at("meta"))
                .into_iter()
                .map(|(_, k, v)| (k, v))
                .collect()
        })
        .unwrap_or_default()
}

fn tunnel_node(d: &mut Decoder<'_>, n: &Node, path: &str) -> Option<TunnelNode> {
    let mut o = d.object(n, path)?;
    let mut t = TunnelNode::default();
    if let Some((name, off)) = text_at(d, &mut o, "name") {
        if let Some(msg) = check_tunnel_name(&name) {
            d.error(off, &o.at("name"), msg);
        }
        t.name = name;
    }
    t.meta = meta(d, &mut o);
    t.hubs = uplinks(d, &mut o);
    let services = list(d, &mut o, "services", tunnel_service);
    for (i, (s, off, at)) in services.iter().enumerate() {
        let np = join_field(at, "name");
        if s.name.is_empty() {
            d.error(*off, &np, "the name is empty");
        } else if let Some(msg) = check_tunnel_name(&s.name) {
            d.error(*off, &np, msg);
        } else if services.iter().take(i).any(|(o, _, _)| o.name == s.name) {
            d.error(
                *off,
                &np,
                format!("another service is already named {:?}", s.name),
            );
        }
        let good = split_authority(&s.address)
            .is_some_and(|(h, p)| !h.is_empty() && p.is_some_and(|p| !p.is_empty() && p != "0"));
        if !good {
            d.error(
                *off,
                &join_field(at, "address"),
                format!(
                    "{:?} is not a host and port to connect to, as in \"10.0.0.5:22\"",
                    s.address
                ),
            );
        }
    }
    t.services = services.into_iter().map(|(s, _, _)| s).collect();
    t.allow_hub_services = boolean(d, &mut o, "allow_hub_services");
    t.dial_timeout = duration(d, &mut o, "dial_timeout");
    t.idle_timeout = duration(d, &mut o, "idle_timeout");
    t.max_tunnels = count(d, &mut o, "max_tunnels", i64::MAX);
    t.console = role_console(d, &mut o);
    o.finish(d, TUNNEL_NODE_FIELDS, &[]);
    Some(t)
}

fn tunnel_service(d: &mut Decoder<'_>, n: &Node, path: &str) -> Option<TunnelService> {
    let mut o = d.object(n, path)?;
    let (name, _) = required(d, &mut o, "name");
    let (address, _) = required(d, &mut o, "address");
    let description = text(d, &mut o, "description");
    o.finish(d, TUNNEL_SERVICE_FIELDS, &[]);
    Some(TunnelService {
        name,
        address,
        description,
    })
}

fn tunnel_proxy(d: &mut Decoder<'_>, n: &Node, path: &str) -> Option<TunnelProxy> {
    let mut o = d.object(n, path)?;
    let mut t = TunnelProxy::default();
    if let Some((name, off)) = text_at(d, &mut o, "name") {
        if let Some(msg) = check_tunnel_name(&name) {
            d.error(off, &o.at("name"), msg);
        }
        t.name = name;
    }
    t.meta = meta(d, &mut o);
    t.hubs = uplinks(d, &mut o);
    let forwards = list(d, &mut o, "forwards", |d, n, p| {
        let mut o = d.object(n, p)?;
        let (listen, off) = required(d, &mut o, "listen");
        if off != o.off {
            d.address_at(&o.at("listen"), off);
            check_listen(d, &listen, off, &o.at("listen"));
        }
        let (node, noff) = required(d, &mut o, "node");
        if node.is_empty() && noff != o.off {
            d.error(noff, &o.at("node"), "the node is empty");
        }
        let (service, soff) = required(d, &mut o, "service");
        if service.is_empty() && soff != o.off {
            d.error(soff, &o.at("service"), "the service is empty");
        }
        let hub = text(d, &mut o, "hub");
        o.finish(d, TUNNEL_FORWARD_FIELDS, &[]);
        Some(TunnelForward {
            listen,
            node,
            service,
            hub,
        })
    });
    for (f, off, at) in &forwards {
        if !f.hub.is_empty() && !t.hubs.iter().any(|u| u.hub_name() == f.hub) {
            d.error(
                *off,
                &join_field(at, "hub"),
                format!("there is no hub named {:?}", f.hub),
            );
        }
    }
    t.forwards = forwards.into_iter().map(|(f, _, _)| f).collect();
    t.web_proxies = list(d, &mut o, "web_proxies", |d, n, p| {
        let mut o = d.object(n, p)?;
        let name = text(d, &mut o, "name");
        let (listen, off) = required(d, &mut o, "listen");
        if off != o.off {
            d.address_at(&o.at("listen"), off);
            check_listen(d, &listen, off, &o.at("listen"));
        }
        let (url, uoff) = required(d, &mut o, "url");
        if uoff != o.off {
            let good = url.strip_prefix("ws://").is_some_and(|rest| {
                let auth = rest.split(['/', '?', '#']).next().unwrap_or("");
                !auth.is_empty()
            });
            if !good {
                d.error(uoff, &o.at("url"), format!("{url:?} is not a ws:// URL"));
            }
        }
        let (password, poff) = required(d, &mut o, "password");
        if password.is_empty() && poff != o.off {
            d.error(poff, &o.at("password"), "the password is empty");
        }
        let max_conns = count(d, &mut o, "max_conns", i64::MAX);
        o.finish(d, TUNNEL_WEB_PROXY_FIELDS, &[]);
        Some(TunnelWebProxy {
            name,
            listen,
            url,
            password,
            max_conns,
        })
    })
    .into_iter()
    .map(|(w, _, _)| w)
    .collect();
    t.access = access_of(d, &mut o);
    t.open_timeout = duration(d, &mut o, "open_timeout");
    t.idle_timeout = duration(d, &mut o, "idle_timeout");
    t.max_tunnels = count(d, &mut o, "max_tunnels", i64::MAX);
    t.console = role_console(d, &mut o);
    o.finish(d, TUNNEL_PROXY_FIELDS, &[]);
    Some(t)
}

fn uplinks(d: &mut Decoder<'_>, o: &mut Obj<'_>) -> Vec<TunnelUplink> {
    let ups = list(d, o, "hubs", uplink);
    for (i, (u, off, at)) in ups.iter().enumerate() {
        if ups
            .iter()
            .take(i)
            .any(|(o, _, _)| o.hub_name() == u.hub_name())
        {
            d.error(
                *off,
                &join_field(at, "name"),
                format!("another hub is already named {:?}", u.hub_name()),
            );
        }
    }
    ups.into_iter().map(|(u, _, _)| u).collect()
}

fn uplink(d: &mut Decoder<'_>, n: &Node, path: &str) -> Option<TunnelUplink> {
    let mut o = d.object(n, path)?;
    let mut u = TunnelUplink::default();
    if let Some((name, off)) = text_at(d, &mut o, "name") {
        if let Some(msg) = check_tunnel_name(&name) {
            d.error(off, &o.at("name"), msg);
        }
        u.name = name;
    }
    let ep = o.at("endpoints");
    match o.require(d, "endpoints") {
        Some(v) => {
            let eps = d.array(v, &ep, |d, n, p| {
                let mut o = d.object(n, p)?;
                let (address, off) = required(d, &mut o, "address");
                let disabled = boolean(d, &mut o, "disabled");
                let at = o.at("address");
                o.finish(d, TUNNEL_ENDPOINT_FIELDS, &[]);
                Some((TunnelEndpoint { address, disabled }, off, at))
            });
            if eps.is_empty() {
                d.error(v.off, &ep, "list at least one endpoint");
            } else if eps.iter().all(|(e, _, _)| e.disabled) {
                d.error(v.off, &ep, "every endpoint is disabled");
            }
            for (j, (e, off, at)) in eps.iter().enumerate() {
                if let Some(msg) = check_tunnel_endpoint(&e.address) {
                    d.error(*off, at, msg);
                }
                if eps.iter().take(j).any(|(o, _, _)| o.address == e.address) {
                    d.error(*off, at, format!("{:?} is listed twice", e.address));
                }
            }
            u.endpoints = eps.iter().map(|(e, _, _)| e.clone()).collect();
            // Kept for the checks below, which need the uplink's other fields.
            o_endpoints_checks(d, &mut o, &mut u, &eps);
        }
        None => {
            o_endpoints_checks(d, &mut o, &mut u, &[]);
        }
    }
    o.finish(d, TUNNEL_UPLINK_FIELDS, &[]);
    Some(u)
}

/// The rest of an uplink, and the checks of its endpoints against it.
fn o_endpoints_checks(
    d: &mut Decoder<'_>,
    o: &mut Obj<'_>,
    u: &mut TunnelUplink,
    eps: &[(TunnelEndpoint, usize, String)],
) {
    let token = text_at(d, o, "token");
    let password = text_at(d, o, "password");
    u.body_password = text(d, o, "body_password");
    // pin, and its old name quic_pin, read with fit's warning.
    let old_off = o.key_off("quic_pin").unwrap_or(o.off);
    let new = text_at(d, o, "pin");
    let was = text_at(d, o, "quic_pin");
    let pin = match (new, was) {
        (Some(_), Some(_)) => {
            d.error(
                old_off,
                &o.at("quic_pin"),
                "\"quic_pin\" is the old name of \"pin\"; give only \"pin\"",
            );
            None
        }
        (None, Some(p)) => {
            d.renamed(old_off, &o.at("quic_pin"), "pin");
            Some(p)
        }
        (p, None) => p,
    };
    let ca_file = text_at(d, o, "ca_file");
    u.quic_insecure = boolean(d, o, "quic_insecure");
    u.pool_size = count(d, o, "pool_size", 100);
    u.dial_timeout = duration(d, o, "dial_timeout");
    u.call_timeout = duration(d, o, "call_timeout");
    u.retry_interval = duration(d, o, "retry_interval");
    u.ping_interval = duration(d, o, "ping_interval");
    u.ping_timeout = duration(d, o, "ping_timeout");
    match (&token, &password) {
        (Some((t, _)), Some((p, poff))) if !t.is_empty() && !p.is_empty() => {
            d.error(
                *poff,
                &o.at("password"),
                "set token or password, not both: the token replaces the password",
            );
        }
        (Some((t, toff)), _) if !t.is_empty() => {
            if let Some(msg) = check_link_token_string(t) {
                d.error(*toff, &o.at("token"), msg);
            }
        }
        (_, Some((p, _))) if !p.is_empty() => {}
        _ => d.error(
            o.off,
            &o.at("token"),
            "set the token the hub issued for this member (or, until the next release, its \
             password)",
        ),
    }
    u.token = token.map(|(t, _)| t).unwrap_or_default();
    u.password = password.map(|(p, _)| p).unwrap_or_default();
    if let Some((p, off)) = &pin {
        if !p.is_empty() {
            if ca_file.as_ref().is_some_and(|(c, _)| !c.is_empty()) {
                d.error(
                    *off,
                    &o.at("ca_file"),
                    "pin and ca_file both check the hub's certificate: set one",
                );
            } else if let Some(msg) = check_pin(p) {
                d.error(*off, &o.at("pin"), msg);
            }
        }
    }
    u.pin = pin.map(|(p, _)| p).unwrap_or_default();
    u.ca_file = ca_file.map(|(c, _)| c).unwrap_or_default();
    // quic_insecure takes any certificate, which leaves a quic link off loopback as
    // unauthenticated as a tcp one: refused alike, unless body_password seals the frames, or
    // pin or ca_file overrides it.
    if u.quic_insecure && u.body_password.is_empty() && u.pin.is_empty() && u.ca_file.is_empty() {
        let off_loopback = eps.iter().find_map(|(e, _, _)| {
            parse_endpoint(&e.address)
                .filter(|a| a.protocol == TunnelProtocol::Quic && !loopback(&a.host))
                .map(|_| e.address.clone())
        });
        if let Some(addr) = off_loopback {
            d.error(
                o.off,
                &o.at("quic_insecure"),
                format!(
                    "quic_insecure takes any certificate, so the links to {addr:?}, whose host is \
                     not a loopback one, would be open to whoever is on the path: set pin (the \
                     fingerprint on the hub's pages) or ca_file instead, or body_password, as the \
                     hub has"
                ),
            );
        }
    }
    for (e, off, at) in eps {
        if check_tunnel_endpoint(&e.address).is_none() && u.bare(&e.address) {
            d.error(
                *off,
                at,
                format!(
                    "{:?}, whose host is not a loopback one, carries the links in the clear and \
                     unauthenticated: use tls:// or wss:// with pin (the fingerprint on the \
                     hub's pages) or ca_file, or quic://; or set body_password, as the hub has",
                    e.address
                ),
            );
        }
    }
}

/// Decodes `llm`.
pub(crate) fn llm(d: &mut Decoder<'_>, n: &Node, path: &str) -> Option<Llm> {
    let mut o = d.object(n, path)?;
    let hub = o.take("hub").and_then(|v| {
        let p = join_field(path, "hub");
        let mut o = d.object(v, &p)?;
        let role = llm_role(d, &mut o, "hub");
        let max_nodes = count(d, &mut o, "max_nodes", i64::MAX);
        o.finish(d, LLM_HUB_FIELDS, &[]);
        Some(LlmHub { role, max_nodes })
    });
    let node = o
        .take("node")
        .and_then(|v| llm_node(d, v, &join_field(path, "node")));
    let proxy = o
        .take("proxy")
        .and_then(|v| llm_proxy(d, v, &join_field(path, "proxy")));
    o.finish(d, LLM_FIELDS, &[]);
    Some(Llm { hub, node, proxy })
}

/// Whether `s` has no space and no control character.
fn no_space(s: &str) -> bool {
    !s.chars().any(|c| c.is_whitespace() || c.is_control())
}

fn llm_role(d: &mut Decoder<'_>, o: &mut Obj<'_>, role: &str) -> LlmRole {
    let mut r = LlmRole::default();
    if let Some((id, off)) = text_at(d, o, "id") {
        if id.chars().count() > 128 || !no_space(&id) {
            d.error(
                off,
                &o.at("id"),
                "the ID is up to 128 characters without spaces",
            );
        }
        r.id = id;
    }
    if let Some((name, off)) = text_at(d, o, "name") {
        if name.chars().count() > 64 || name.trim() != name || name.chars().any(char::is_control) {
            d.error(
                off,
                &o.at("name"),
                "the name is up to 64 characters, without control characters or spaces around \
                 it",
            );
        }
        r.name = name;
    }
    r.console = role_console(d, o);
    r.listen = o
        .take("listen")
        .and_then(|v| llm_listen(d, v, &o.at("listen"), role));
    r.access = access_of(d, o);
    r.tokens = link_tokens(d, o);
    let peers_off = o.key_off("peers").unwrap_or(o.off);
    let peers = list(d, o, "peers", |d, n, p| llm_peer(d, n, p, role));
    if peers.len() > MAX_LLM_PEERS {
        d.error(
            peers_off,
            &o.at("peers"),
            format!("at most {MAX_LLM_PEERS} peers"),
        );
    }
    for (i, (p, off, at)) in peers.iter().enumerate() {
        if !p.address.is_empty() && peers.iter().take(i).any(|(o, _, _)| o.address == p.address) {
            d.error(
                *off,
                &join_field(at, "address"),
                format!("{:?} is listed twice", p.address),
            );
        }
    }
    r.peers = peers.into_iter().map(|(p, _, _)| p).collect();
    r.limits = o.take("limits").and_then(|v| {
        let p = o.at("limits");
        let mut o = d.object(v, &p)?;
        let mut l = LlmLimits {
            max_conns: count(d, &mut o, "max_conns", i64::MAX),
            max_inflight: count(d, &mut o, "max_inflight", i64::MAX),
            ..LlmLimits::default()
        };
        for (key, slot, min) in [
            (
                "ping_interval",
                &mut l.ping_interval,
                Duration::from_millis(100),
            ),
            ("slow_timeout", &mut l.slow_timeout, Duration::ZERO),
            (
                "retry_interval",
                &mut l.retry_interval,
                Duration::from_millis(100),
            ),
            ("stream_idle", &mut l.stream_idle, Duration::from_secs(1)),
        ] {
            if let Some(v) = o.take(key) {
                let at = o.at(key);
                if let Some(t) = d.duration(v, &at) {
                    if !t.is_zero() && t < min {
                        d.error(
                            v.off,
                            &at,
                            format!(
                                "{key} is too short: at least {}",
                                crate::duration::format(min)
                            ),
                        );
                    }
                    *slot = t;
                }
            }
        }
        o.finish(d, LLM_LIMITS_FIELDS, &[]);
        Some(l)
    });
    r
}

fn llm_listen(d: &mut Decoder<'_>, n: &Node, path: &str, role: &str) -> Option<LlmListen> {
    let mut o = d.object(n, path)?;
    let (address, aoff) = required(d, &mut o, "address");
    let mut l = LlmListen {
        address,
        password: text(d, &mut o, "password"),
        node_password: text(d, &mut o, "node_password"),
        proxy_password: text(d, &mut o, "proxy_password"),
        hub_password: text(d, &mut o, "hub_password"),
        cert_file: text(d, &mut o, "cert_file"),
        key_file: text(d, &mut o, "key_file"),
        self_signed: false,
        cert: text(d, &mut o, "cert"),
        key: text(d, &mut o, "key"),
    };
    l.self_signed = boolean(d, &mut o, "self_signed");
    o.finish(d, LLM_LISTEN_FIELDS, &[]);
    if aoff != n.off {
        d.address_at(&join_field(path, "address"), aoff);
    }
    if aoff != n.off
        && check_listen(d, &l.address, aoff, &join_field(path, "address"))
        && host_of(&l.address).is_some_and(|h| !loopback(&h))
        && !l.tls()
    {
        d.error(
            aoff,
            &join_field(path, "address"),
            format!(
                "{:?} is not a loopback address: set cert_file and key_file, or \
                 self_signed, so that links are taken over TLS (wss://) and peers can check \
                 whose listener they reach",
                l.address
            ),
        );
    }
    if l.cert_file.is_empty() != l.key_file.is_empty() {
        d.error(n.off, path, "cert_file and key_file go together");
    }
    if l.cert.is_empty() != l.key.is_empty() {
        d.error(n.off, path, "cert and key go together");
    }
    if !l.cert_file.is_empty() && (l.self_signed || !l.cert.is_empty()) {
        d.error(
            n.off,
            &join_field(path, "cert_file"),
            "cert_file, or self_signed with its cert and key: one of the two",
        );
    }
    if !l.cert.is_empty() && !l.key.is_empty() {
        if let Some(msg) = key_pair_problem(&l.cert, &l.key) {
            d.error(
                n.off,
                &join_field(path, "cert"),
                format!("the certificate and key do not load: {msg}"),
            );
        }
    }
    // A role's own password is only for a role the listener links with.
    let accept = llm_peer_roles(role);
    for (other, v) in [
        ("node", &l.node_password),
        ("proxy", &l.proxy_password),
        ("hub", &l.hub_password),
    ] {
        if !accept.contains(&other) && !v.is_empty() {
            d.error(
                n.off,
                &join_field(path, &format!("{other}_password")),
                format!("a {role} takes no links from {other}s"),
            );
        }
    }
    Some(l)
}

fn llm_peer(d: &mut Decoder<'_>, n: &Node, path: &str, role: &str) -> Option<LlmPeer> {
    let mut o = d.object(n, path)?;
    let (address, aoff) = required(d, &mut o, "address");
    let token = text_at(d, &mut o, "token");
    let password = text_at(d, &mut o, "password");
    let (peer_role, roff) = required(d, &mut o, "role");
    let pin = text(d, &mut o, "pin");
    let ca_file = text(d, &mut o, "ca_file");
    let server_name = text(d, &mut o, "server_name");
    o.finish(d, LLM_PEER_FIELDS, &[]);
    let host = host_of(&address).filter(|h| !h.is_empty());
    let mut good = false;
    if aoff != n.off {
        if crate::model::check_address(&address).is_err() || host.is_none() {
            d.error(
                aoff,
                &join_field(path, "address"),
                format!("{address:?} is not a host and port, as in \"10.0.0.1:9100\""),
            );
        } else {
            good = true;
        }
    }
    let token_s = token.as_ref().map_or("", |(t, _)| t.as_str());
    let password_s = password.as_ref().map_or("", |(p, _)| p.as_str());
    match (token_s.is_empty(), password_s.is_empty()) {
        (false, false) => d.error(
            password.as_ref().map_or(n.off, |(_, o)| *o),
            &join_field(path, "password"),
            "set token or password, not both: the token replaces the password",
        ),
        (false, true) => {
            if let Some(msg) = check_link_token_string(token_s) {
                d.error(
                    token.as_ref().map_or(n.off, |(_, o)| *o),
                    &join_field(path, "token"),
                    msg,
                );
            }
        }
        (true, false) => {}
        (true, true) => d.error(
            n.off,
            &join_field(path, "token"),
            "a peer needs the token its listener issued for this role (or, until the next \
             release, its password)",
        ),
    }
    let accept = llm_peer_roles(role);
    if roff != n.off && !accept.contains(&peer_role.as_str()) {
        d.error(
            roff,
            &join_field(path, "role"),
            format!(
                "a {role} links with {}, not {peer_role:?}",
                accept.join(" and ")
            ),
        );
    }
    if !pin.is_empty() && parse_pin(&pin).is_none() {
        d.error(
            n.off,
            &join_field(path, "pin"),
            format!(
                "bad certificate pin {pin:?}: it is the SHA-256 of the certificate, 64 hex digits"
            ),
        );
    }
    if !pin.is_empty() && !ca_file.is_empty() {
        d.error(
            n.off,
            &join_field(path, "pin"),
            "pin or ca_file, one of the two",
        );
    }
    if !server_name.is_empty() && ca_file.is_empty() {
        d.error(
            n.off,
            &join_field(path, "server_name"),
            "server_name goes with ca_file",
        );
    }
    if good && host.is_some_and(|h| !loopback(&h)) && pin.is_empty() && ca_file.is_empty() {
        d.error(
            aoff,
            &join_field(path, "address"),
            format!(
                "{address:?} is not a loopback address: set pin to the fingerprint of the peer \
                 listener's certificate, as its pages show it, or ca_file, so that the link is \
                 made over TLS (wss://) to that listener alone"
            ),
        );
    }
    Some(LlmPeer {
        address,
        token: token.map(|(t, _)| t).unwrap_or_default(),
        password: password.map(|(p, _)| p).unwrap_or_default(),
        role: peer_role,
        pin,
        ca_file,
        server_name,
    })
}

fn llm_node(d: &mut Decoder<'_>, n: &Node, path: &str) -> Option<LlmNode> {
    let mut o = d.object(n, path)?;
    let role = llm_role(d, &mut o, "node");
    let mut node = LlmNode {
        role,
        ..LlmNode::default()
    };
    if let Some((desc, off)) = text_at(d, &mut o, "description") {
        if desc.len() > 1024 {
            d.error(
                off,
                &o.at("description"),
                "the description is longer than 1024 bytes",
            );
        }
        node.description = desc;
    }
    let models_off = o.key_off("models").unwrap_or(o.off);
    let models = list(d, &mut o, "models", llm_model);
    if models.len() > MAX_LLM_MODELS {
        d.error(
            models_off,
            &o.at("models"),
            format!("at most {MAX_LLM_MODELS} models"),
        );
    }
    for (i, (m, off, at)) in models.iter().enumerate() {
        if let Some((field, msg)) = check_llm_model(m) {
            d.error(*off, &join_field(at, field), msg);
        } else if let Some((first, _, _)) = models
            .iter()
            .take(i)
            .find(|(o, _, _)| o.name == m.name && llm_protocol(&o.api) == llm_protocol(&m.api))
        {
            let msg = if first.api == m.api {
                format!("{} ({}) is listed twice", m.name, m.api)
            } else {
                format!(
                    "{} is listed twice for agents' {} API, as {} and {}",
                    m.name,
                    llm_protocol(&m.api),
                    first.api,
                    m.api
                )
            };
            d.error(*off, &join_field(at, "name"), msg);
        }
    }
    node.models = models.into_iter().map(|(m, _, _)| m).collect();
    node.settings = o.take("settings").and_then(|v| {
        let p = o.at("settings");
        let mut o = d.object(v, &p)?;
        let mut s = LlmNodeSettings {
            allow_hub_manage: boolean(d, &mut o, "allow_hub_manage"),
            gpu_sudo: bool_opt(d, &mut o, "gpu_sudo"),
            ..LlmNodeSettings::default()
        };
        if let Some(v) = o.take("refresh_interval") {
            let at = o.at("refresh_interval");
            if let Some(t) = d.duration(v, &at) {
                if !t.is_zero() && (t < Duration::from_secs(5) || t > Duration::from_secs(3600)) {
                    d.error(v.off, &at, "refresh_interval is 5s to 1h");
                }
                s.refresh_interval = t;
            }
        }
        if let Some(v) = o.take("job_timeout") {
            let at = o.at("job_timeout");
            if let Some(t) = d.duration(v, &at) {
                if !t.is_zero()
                    && (t < Duration::from_secs(10) || t > Duration::from_secs(7 * 24 * 3600))
                {
                    d.error(v.off, &at, "job_timeout is 10s to 168h");
                }
                s.job_timeout = t;
            }
        }
        s.gpu_data_dir = text(d, &mut o, "gpu_data_dir");
        o.finish(d, LLM_SETTINGS_FIELDS, &[]);
        Some(s)
    });
    node.proxy = o.take("proxy").and_then(|v| {
        let p = o.at("proxy");
        let mut o = d.object(v, &p)?;
        let np = LlmNodeProxy {
            connect_timeout: duration(d, &mut o, "connect_timeout"),
            first_byte_timeout: duration(d, &mut o, "first_byte_timeout"),
            idle_timeout: duration(d, &mut o, "idle_timeout"),
            timeout: duration(d, &mut o, "timeout"),
            max_inflight: count(d, &mut o, "max_inflight", i64::MAX),
            max_body_bytes: bytes(d, &mut o, "max_body_bytes"),
        };
        o.finish(d, LLM_NODE_PROXY_FIELDS, &[]);
        Some(np)
    });
    o.finish(d, LLM_NODE_FIELDS, &[]);
    Some(node)
}

fn llm_model(d: &mut Decoder<'_>, n: &Node, path: &str) -> Option<LlmModel> {
    let mut o = d.object(n, path)?;
    let (name, _) = required(d, &mut o, "name");
    let (api, _) = required(d, &mut o, "api");
    let (base_url, _) = required(d, &mut o, "base_url");
    let m = LlmModel {
        name,
        api,
        base_url,
        upstream: text(d, &mut o, "upstream"),
        api_key: text(d, &mut o, "api_key"),
        api_version: text(d, &mut o, "api_version"),
        disabled: boolean(d, &mut o, "disabled"),
        description: text(d, &mut o, "description"),
        max_inflight: count(d, &mut o, "max_inflight", 100_000),
    };
    o.finish(d, LLM_MODEL_FIELDS, &[]);
    Some(m)
}

/// What is wrong with a model, and the field it is about, as fit's `CheckLLMModel`; `None` when
/// nothing is.
#[must_use]
pub fn check_llm_model(m: &LlmModel) -> Option<(&'static str, String)> {
    let bad = |f: &'static str, msg: &str| Some((f, msg.to_owned()));
    if m.name.is_empty() || m.name.len() > 128 || !no_space(&m.name) {
        return bad("name", "the name needs 1 to 128 characters without spaces");
    }
    if !LLM_APIS.contains(&m.api.as_str()) {
        return Some((
            "api",
            format!("api is {}, not {:?}", LLM_APIS.join(", "), m.api),
        ));
    }
    if m.upstream.len() > 256 || !no_space(&m.upstream) {
        return bad(
            "upstream",
            "the upstream model ID is up to 256 characters without spaces",
        );
    }
    if m.api_key.len() > 4096 || !no_space(&m.api_key) {
        return bad(
            "api_key",
            "the API key is up to 4096 characters without spaces",
        );
    }
    if m.description.len() > 1024 {
        return bad("description", "the description is longer than 1024 bytes");
    }
    if m.max_inflight > 100_000 {
        return bad("max_inflight", "max_inflight is 0 to 100000");
    }
    if m.api == "azure_openai" && m.api_version.is_empty() {
        return bad(
            "api_version",
            "an azure_openai model needs its api_version, as 2024-10-21",
        );
    }
    if m.api != "azure_openai" && !m.api_version.is_empty() {
        return bad("api_version", "api_version is only for azure_openai models");
    }
    if m.api_version.len() > 64
        || !m
            .api_version
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'.' | b'_'))
    {
        return bad(
            "api_version",
            "api_version is up to 64 letters, digits, dots, dashes and underscores",
        );
    }
    if !good_base_url(&m.base_url) {
        return bad(
            "base_url",
            "base_url is an http or https URL without a query or a trailing slash, as in \
             http://127.0.0.1:8000/v1",
        );
    }
    None
}

/// An http or https URL with a host, without a query, a fragment or a trailing slash.
fn good_base_url(u: &str) -> bool {
    let rest = u
        .strip_prefix("http://")
        .or_else(|| u.strip_prefix("https://"));
    let Some(rest) = rest else {
        return false;
    };
    let auth = rest.split('/').next().unwrap_or("");
    !auth.is_empty()
        && !auth.contains('@')
        && split_authority(auth).is_some_and(|(h, p)| {
            !h.is_empty() && p.is_none_or(|p| p.is_empty() || p.parse::<u16>().is_ok())
        })
        && !u.contains(['?', '#'])
        && !u.ends_with('/')
        && no_space(u)
}

fn llm_proxy(d: &mut Decoder<'_>, n: &Node, path: &str) -> Option<LlmProxy> {
    let mut o = d.object(n, path)?;
    let role = llm_role(d, &mut o, "proxy");
    let api = o.take("api").and_then(|v| {
        let p = o.at("api");
        let mut o = d.object(v, &p)?;
        let (address, aoff) = required(d, &mut o, "address");
        let a = LlmProxyApi {
            address,
            cert_file: text(d, &mut o, "cert_file"),
            key_file: text(d, &mut o, "key_file"),
            max_body_bytes: bytes(d, &mut o, "max_body_bytes"),
            max_conns: count(d, &mut o, "max_conns", i64::MAX),
            max_inflight: count(d, &mut o, "max_inflight", i64::MAX),
            first_byte_timeout: duration(d, &mut o, "first_byte_timeout"),
            timeout: duration(d, &mut o, "timeout"),
        };
        o.finish(d, LLM_API_FIELDS, &[]);
        let ap = join_field(&p, "address");
        if aoff != v.off {
            d.address_at(&ap, aoff);
        }
        if aoff != v.off
            && check_listen(d, &a.address, aoff, &ap)
            && host_of(&a.address).is_some_and(|h| !loopback(&h))
            && (a.cert_file.is_empty() || a.key_file.is_empty())
        {
            d.error(
                aoff,
                &ap,
                format!(
                    "{:?} is not a loopback address: set cert_file and key_file, so that keys \
                     are not sent in the clear",
                    a.address
                ),
            );
        }
        if a.cert_file.is_empty() != a.key_file.is_empty() {
            d.error(v.off, &p, "cert_file and key_file go together");
        }
        Some(a)
    });
    let keys_off = o.key_off("api_keys").unwrap_or(o.off);
    let keys = list(d, &mut o, "api_keys", |d, n, p| {
        let mut o = d.object(n, p)?;
        let (id, _) = required(d, &mut o, "id");
        let (name, _) = required(d, &mut o, "name");
        let prefix = text(d, &mut o, "prefix");
        let (hash, _) = required(d, &mut o, "hash");
        let created_at = seconds(d, &mut o, "created_at");
        o.finish(d, LLM_KEY_FIELDS, &[]);
        Some(LlmProxyKey {
            id,
            name,
            prefix,
            hash,
            created_at,
        })
    });
    if keys.len() > MAX_LLM_KEYS {
        d.error(
            keys_off,
            &o.at("api_keys"),
            format!("at most {MAX_LLM_KEYS} keys"),
        );
    }
    for (i, (k, off, at)) in keys.iter().enumerate() {
        let hex = |s: &str, n: usize| s.len() == n && s.bytes().all(|b| b.is_ascii_hexdigit());
        if !hex(&k.id, 16) {
            d.error(*off, &join_field(at, "id"), "the ID is 16 hex digits");
        } else if keys.iter().take(i).any(|(o, _, _)| o.id == k.id) {
            d.error(
                *off,
                &join_field(at, "id"),
                format!("{:?} is listed twice", k.id),
            );
        }
        let chars = k.name.trim().chars().count();
        if chars == 0 || chars > 64 {
            d.error(
                *off,
                &join_field(at, "name"),
                "the name needs 1 to 64 characters",
            );
        }
        if !hex(&k.hash, 64) {
            d.error(
                *off,
                &join_field(at, "hash"),
                "the hash is a SHA-256 in hex: 64 hex digits",
            );
        }
        if k.prefix.len() > 16 {
            d.error(
                *off,
                &join_field(at, "prefix"),
                "the prefix is up to 16 characters",
            );
        }
    }
    o.finish(d, LLM_PROXY_FIELDS, &[]);
    Some(LlmProxy {
        role,
        api,
        api_keys: keys.into_iter().map(|(k, _, _)| k).collect(),
    })
}

#[cfg(test)]
mod tests;
