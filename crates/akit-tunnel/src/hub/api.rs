//! The hub's calls and status (I103, fit's `tunnel/hub/api.go` and the calls of `hub.go` it
//! makes): what the hub's pages show and do, through [`HubApi`], the handle the role gives
//! (`Role::api`). Each call returns at once: one that asks a node is answered through a callback,
//! on the link's I/O thread, within `call_timeout`.

use std::sync::Arc;

use akit_roles::linktoken::rfc3339;

use super::{HubCore, HubState, Route, kind_name, lock};
use crate::link::Link;
use crate::msg::{self, Info, Obj, Service, method};

/// What a call on a node ends in: the node's info as it answered, or why not.
pub type InfoDone = Box<dyn FnOnce(Result<Info, String>) + Send>;

/// The hub's handle for its pages.
#[derive(Clone)]
pub struct HubApi(pub(super) Arc<HubCore>);

impl std::fmt::Debug for HubApi {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("HubApi").finish_non_exhaustive()
    }
}

/// A peer's info as the pages show it (fit's `InfoView`): its services always a list, with
/// `address` and `description`.
fn info_view(i: &Info) -> String {
    let services = msg::array(&i.services, |s| {
        Obj::new()
            .str("name", &s.name)
            .str("address", &s.addr)
            .str_omit("description", &s.desc)
            .end()
    });
    let mut o = Obj::new()
        .str("name", &i.name)
        .str("type", &i.kind)
        .str_omit("version", &i.version)
        .str_omit("hostname", &i.hostname)
        .str_omit("os", &i.os)
        .raw("services", &services);
    if !i.meta.is_empty() {
        let mut m = Obj::new();
        for (k, v) in &i.meta {
            m = m.str(k, v);
        }
        o = o.raw("meta", &m.end());
    }
    o.end()
}

/// A listener's URL as peers dial it, an unspecified host the loopback (fit's `Dialable`).
fn listener_url(
    protocol: akit_config::roles::TunnelProtocol,
    at: std::net::SocketAddr,
    path: &str,
) -> String {
    use akit_config::roles::TunnelProtocol as P;
    let mut at = at;
    if at.ip().is_unspecified() {
        at.set_ip(if at.is_ipv6() {
            std::net::Ipv6Addr::LOCALHOST.into()
        } else {
            std::net::Ipv4Addr::LOCALHOST.into()
        });
    }
    match protocol {
        P::Tcp => at.to_string(),
        P::Ws | P::Wss => format!("{}://{at}{path}", protocol.as_str()),
        P::Tls | P::Quic => format!("{}://{at}", protocol.as_str()),
    }
}

impl HubCore {
    /// A registered peer's links the hub may send on, by their places, lowest first.
    fn peer_links(st: &HubState, peer: u32) -> Vec<Arc<Link>> {
        st.peers.get(&peer).map_or_else(Vec::new, |p| {
            p.links
                .values()
                .filter_map(|lid| {
                    st.links
                        .get(lid)
                        .filter(|r| r.reg.as_ref().is_some_and(|g| g.ready))
                        .map(|r| Arc::clone(&r.link))
                })
                .collect()
        })
    }

    /// A node's info answered to the hub's call: taken, the proxies told, unless it names another
    /// node.
    fn accept_node_info(&self, peer: u32, info: &Info) -> Result<(), String> {
        {
            let mut st = lock(&self.st);
            let Some(p) = st.peers.get_mut(&peer) else {
                return Err("tunnel: the node is gone".to_owned());
            };
            if info.name != p.info.name || info.kind != p.info.kind {
                return Err("tunnel: the node answered with another name or type".to_owned());
            }
            p.info = info.clone();
        }
        self.broadcast_nodes();
        Ok(())
    }

    /// Calls `m` with `body` on node `id`, over its link in the lowest place; its answer, the
    /// node's info, is taken.
    fn node_call(self: &Arc<Self>, id: u32, m: u8, body: &[u8], done: InfoDone) {
        let link = {
            let st = lock(&self.st);
            match st.peers.get(&id) {
                Some(p) if p.info.kind == msg::TYPE_NODE => {
                    Self::peer_links(&st, id).into_iter().next()
                }
                _ => return done(Err("no such node".to_owned())),
            }
        };
        let Some(link) = link else {
            return done(Err("tunnel: the peer has no link".to_owned()));
        };
        let me = Arc::clone(self);
        let within = lock(&self.cfg).call();
        link.call(
            m,
            body,
            within,
            None,
            Box::new(move |r| {
                let r = r.map_err(|e| e.to_string()).and_then(|b| {
                    let info = msg::parse(&b)
                        .map(|v| Info::from(&v))
                        .ok_or("tunnel: the node's answer is not JSON")?;
                    me.accept_node_info(id, &info)?;
                    Ok(info)
                });
                done(r);
            }),
        );
    }

    fn close_route_now(&self, id: u64, reason: &str) -> bool {
        let Some(r) = lock(&self.st).routes.remove(&id) else {
            return false;
        };
        r.proxy_link
            .send_tunnel(method::TUNNEL_CLOSE, id, reason.as_bytes());
        r.node_link
            .send_tunnel(method::TUNNEL_CLOSE, id, reason.as_bytes());
        true
    }

    fn route_view(id: u64, r: &Route, st: &HubState) -> String {
        let name = |p: u32| {
            st.peers
                .get(&p)
                .map(|p| p.info.name.clone())
                .unwrap_or_default()
        };
        Obj::new()
            .str("id", &id.to_string())
            .str("proxy", &name(r.proxy))
            .str("node", &name(r.node))
            .str("service", &r.service)
            .str("addr", &r.addr)
            .str("created", &rfc3339(r.created))
            .num("up", r.up)
            .num("down", r.down)
            .end()
    }
}

impl HubApi {
    /// The hub's state, as fit's pages read it (`GET /api/status`): its name, version, listeners,
    /// certificate, peers (their links' places, tokens and round trips), tunnels and the peers that
    /// registered with the shared password.
    #[must_use]
    pub fn status(&self) -> String {
        let c = &self.0;
        let cfg = lock(&c.cfg).clone();
        let fp = lock(&c.fingerprint).clone();
        let engine = &c.host.engine;
        let tcp: Vec<akio::ListenerInfo> = engine.listeners();
        let mut listen = Vec::new();
        let mut listeners = Vec::new();
        for (l, b) in lock(&c.listeners).iter() {
            let at = match b {
                super::Bound::Tcp(id) => tcp.iter().find(|i| i.id == *id).map(|i| i.local_addr),
                super::Bound::Udp(u) => Some(u.local_addr()),
            };
            let path = if l.protocol.is_ws() { l.ws_path() } else { "" };
            let url = at
                .map(|a| listener_url(l.protocol, a, path))
                .unwrap_or_default();
            if !url.is_empty() {
                listen.push(url.clone());
            }
            let own = !l.cert_file.is_empty();
            listeners.push(
                Obj::new()
                    .str("protocol", l.protocol.as_str())
                    .str("address", &l.address)
                    .str_omit("path", if l.protocol.is_ws() { &l.path } else { "" })
                    .str_omit("url", &url)
                    .str_omit(
                        "fingerprint",
                        if l.protocol.takes_tls() && !own {
                            &fp
                        } else {
                            ""
                        },
                    )
                    .bool("own_cert", own)
                    .end(),
            );
        }
        let st = lock(&c.st);
        let mut ids: Vec<u64> = st.routes.keys().copied().collect();
        ids.sort_unstable();
        let tunnels: Vec<String> = ids
            .iter()
            .filter_map(|id| st.routes.get(id).map(|r| HubCore::route_view(*id, r, &st)))
            .collect();
        let mut peers: Vec<&super::Peer> = st.peers.values().collect();
        peers.sort_by_key(|p| p.id);
        let mut password_peers = Vec::new();
        let peer_views: Vec<String> = peers
            .iter()
            .map(|p| {
                let mut tokens: Vec<String> = Vec::new();
                let mut password = false;
                for lid in p.links.values() {
                    match st.links.get(lid).map(|r| r.token_use.as_ref()) {
                        Some(Some(u)) => {
                            if !tokens.iter().any(|t| t == u.token()) {
                                tokens.push(u.token().to_owned());
                            }
                        }
                        Some(None) => password = true,
                        None => {}
                    }
                }
                tokens.sort();
                if password {
                    password_peers.push(kind_name(&p.info));
                }
                let n = st
                    .routes
                    .values()
                    .filter(|r| r.proxy == p.id || r.node == p.id)
                    .count();
                let places: Vec<i64> = p.links.keys().copied().collect();
                let mut o = Obj::new()
                    .num("id", p.id)
                    .str("addr", &p.addr)
                    .str("since", &rfc3339(p.since))
                    .raw("info", &info_view(&p.info))
                    .num("tunnels", n)
                    .num("conns", places.len())
                    .raw("places", &msg::array(&places, ToString::to_string))
                    .raw("rtt", &p.rtt.json());
                if !p.service_rtt.is_empty() {
                    o = o.raw("service_rtt", &p.service_rtt);
                }
                o = o.raw("tokens", &msg::array(&tokens, |t| msg::json_string(t)));
                if password {
                    o = o.bool("password", true);
                }
                o.end()
            })
            .collect();
        let links = st.links.len();
        drop(st);
        let mut settings = Obj::new()
            .bool("has_password", !cfg.password.is_empty())
            .bool("has_body_password", !cfg.body_password.is_empty());
        let socket = cfg.console_socket.as_ref();
        let socket_at = c.socket_addr().map(|a| a.to_string()).unwrap_or_default();
        settings = settings
            .bool(
                "has_socket_password",
                socket.is_some_and(|s| !s.password.is_empty()),
            )
            .str("socket_address", socket.map_or("", |s| s.address.as_str()));
        Obj::new()
            .str("name", &cfg.hub_name())
            .str("version", &msg::version())
            .raw("listen", &msg::array(&listen, |u| msg::json_string(u)))
            .raw("listeners", &msg::array(&listeners, Clone::clone))
            .raw("settings", &settings.end())
            .str_omit("socket", &socket_at)
            .str("fingerprint", &fp)
            .str("quic_fingerprint", &fp)
            .bool("body_encrypted", !cfg.body_password.is_empty())
            .str("started", &rfc3339(c.started))
            .num("links", links)
            .raw("peers", &msg::array(&peer_views, Clone::clone))
            .raw("tunnels", &msg::array(&tunnels, Clone::clone))
            .raw(
                "password_peers",
                &msg::array(&password_peers, |p| msg::json_string(p)),
            )
            .end()
    }

    /// The peers, as `type/name`, that registered a link with the shared password, which the
    /// pages warn of.
    #[must_use]
    pub fn password_peers(&self) -> Vec<String> {
        let mut v = self.0.password_peers();
        v.sort();
        v
    }

    /// Where the console socket listens (I103b, fit's `SocketAddr`), or `None` without one.
    #[must_use]
    pub fn socket_addr(&self) -> Option<std::net::SocketAddr> {
        self.0.socket_addr()
    }

    /// The ID of the registered peer `kind/name`.
    #[must_use]
    pub fn peer_id(&self, kind: &str, name: &str) -> Option<u32> {
        lock(&self.0.st)
            .by_name
            .get(&format!("{kind}/{name}"))
            .copied()
    }

    /// Asks node `id` for its info, and takes it (fit's `POST /api/peers/refresh`).
    pub fn refresh_node(&self, id: u32, done: InfoDone) {
        self.0.node_call(id, method::NODE_INFO, b"null", done);
    }

    /// Has node `id` reach `services` (fit's `POST /api/peers/services`): the node takes them only
    /// with its `allow_hub_services`.
    pub fn set_services(&self, id: u32, services: &[Service], done: InfoDone) {
        let body = msg::array(services, Service::json);
        self.0
            .node_call(id, method::NODE_SET_SERVICES, body.as_bytes(), done);
    }

    /// Tells peer `id` why, and closes its links once that is sent, those whose registration is
    /// still being answered too (fit's `POST /api/peers/kick`). False for no such peer.
    pub fn kick(&self, id: u32, reason: &str) -> bool {
        let links: Vec<Arc<Link>> = {
            let st = lock(&self.0.st);
            let Some(p) = st.peers.get(&id) else {
                return false;
            };
            p.links
                .values()
                .filter_map(|lid| st.links.get(lid).map(|r| Arc::clone(&r.link)))
                .collect()
        };
        let body = msg::json_string(reason);
        for l in links {
            l.notify(method::KICK, body.as_bytes());
            l.close_after_send();
        }
        true
    }

    /// Closes tunnel `id` at both ends (fit's `CloseRoute`). False for no such tunnel.
    pub fn close_tunnel(&self, id: u64, reason: &str) -> bool {
        self.0.close_route_now(id, reason)
    }
}
