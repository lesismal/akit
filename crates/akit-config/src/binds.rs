//! One address, one server (I100, fit's I162 and `config/bindings.go`): every address the config
//! has akit listen on, the listeners', the Admin API's, the console's and the roles', checked
//! once, a clash reported at the later of two with the earlier named.
//!
//! akio's engines listen with `SO_REUSEPORT`, so that each I/O thread accepts on a socket of its
//! own; two bindings on one address would both listen without an error, and the kernel would split
//! the connections between two servers.

use std::collections::HashMap;
use std::net::IpAddr;

use crate::decode::Decoder;
use crate::model::{Admin, Listener, Protocol};
use crate::roles::{Llm, Tunnel, TunnelProtocol};
use crate::webrtc::WebRtc;

/// An address the config has akit listen on.
struct Bind {
    /// Whose it is, in an error: `listener "web"`, `tunnel.proxy.web_proxies[0]`.
    name: String,
    /// The path of its address, where a clash is reported.
    path: String,
    /// `tcp` or `udp`.
    network: &'static str,
    address: String,
}

/// The networks a listener binds: TCP, and UDP for h3; UDP alone for `udp`.
fn networks(l: &Listener) -> Vec<&'static str> {
    let udp = l.protocols.contains(&Protocol::Udp);
    let h3 = l.protocols.contains(&Protocol::H3);
    match (udp, h3) {
        (true, _) => vec!["udp"],
        (false, true) => vec!["tcp", "udp"],
        _ => vec!["tcp"],
    }
}

/// Every address the config listens on, in the order fit's `binds` lists them, so that a clash
/// is reported at the later of the two. A Unix socket is left out: it shares no port.
fn binds(
    listeners: &[(Listener, String)],
    admin: Option<&Admin>,
    tunnel: Option<&Tunnel>,
    llm: Option<&Llm>,
    webrtc: Option<&WebRtc>,
) -> Vec<Bind> {
    let mut out = Vec::new();
    let mut add = |name: String, path: String, network: &'static str, address: &str| {
        out.push(Bind {
            name,
            path,
            network,
            address: address.to_owned(),
        });
    };
    for (l, path) in listeners {
        for n in networks(l) {
            add(
                format!("listener {:?}", l.name),
                path.clone(),
                n,
                &l.address,
            );
        }
    }
    if let Some(a) = admin {
        if !a.address.starts_with("unix/") {
            add("admin".into(), "admin.address".into(), "tcp", &a.address);
        }
        if let Some(c) = &a.console {
            add(
                "admin.console".into(),
                "admin.console.address".into(),
                "tcp",
                &c.address,
            );
        }
    }
    let console = |add: &mut dyn FnMut(String, String, &'static str, &str),
                   name: &str,
                   c: Option<&crate::model::Console>| {
        if let Some(c) = c {
            add(
                name.to_owned(),
                format!("{name}.address"),
                "tcp",
                &c.address,
            );
        }
    };
    if let Some(l) = llm {
        for (role, r) in [
            ("hub", l.hub.as_ref().map(|h| &h.role)),
            ("node", l.node.as_ref().map(|n| &n.role)),
            ("proxy", l.proxy.as_ref().map(|p| &p.role)),
        ] {
            let Some(r) = r else { continue };
            console(&mut add, &format!("llm.{role}.console"), r.console.as_ref());
            if let Some(listen) = &r.listen {
                add(
                    format!("llm.{role}.listen"),
                    format!("llm.{role}.listen.address"),
                    "tcp",
                    &listen.address,
                );
            }
            if role == "proxy" {
                if let Some(api) = l.proxy.as_ref().and_then(|p| p.api.as_ref()) {
                    add(
                        "llm.proxy.api".into(),
                        "llm.proxy.api.address".into(),
                        "tcp",
                        &api.address,
                    );
                }
            }
        }
    }
    if let Some(t) = tunnel {
        if let Some(h) = &t.hub {
            console(&mut add, "tunnel.hub.console", h.console.as_ref());
            for (i, l) in h.listeners.iter().enumerate() {
                let network = if l.protocol == TunnelProtocol::Quic {
                    "udp"
                } else {
                    "tcp"
                };
                add(
                    format!("tunnel.hub.listeners[{i}]"),
                    format!("tunnel.hub.listeners[{i}].address"),
                    network,
                    &l.address,
                );
            }
            if let Some(cs) = &h.console_socket {
                add(
                    "tunnel.hub.console_socket".into(),
                    "tunnel.hub.console_socket.address".into(),
                    "tcp",
                    &cs.address,
                );
            }
        }
        if let Some(n) = &t.node {
            console(&mut add, "tunnel.node.console", n.console.as_ref());
        }
        if let Some(p) = &t.proxy {
            console(&mut add, "tunnel.proxy.console", p.console.as_ref());
            for (i, f) in p.forwards.iter().enumerate() {
                add(
                    format!("tunnel.proxy.forwards[{i}]"),
                    format!("tunnel.proxy.forwards[{i}].listen"),
                    "tcp",
                    &f.listen,
                );
            }
            for (i, w) in p.web_proxies.iter().enumerate() {
                add(
                    format!("tunnel.proxy.web_proxies[{i}]"),
                    format!("tunnel.proxy.web_proxies[{i}].listen"),
                    "tcp",
                    &w.listen,
                );
            }
        }
    }
    if let Some(w) = webrtc {
        if let Some(t) = &w.turn {
            for (i, a) in t.listen.iter().enumerate() {
                add(
                    format!("webrtc.turn.listen[{i}]"),
                    format!("webrtc.turn.listen[{i}]"),
                    "udp",
                    a,
                );
            }
            for (i, a) in t.listen_tcp.iter().enumerate() {
                add(
                    format!("webrtc.turn.listen_tcp[{i}]"),
                    format!("webrtc.turn.listen_tcp[{i}]"),
                    "tcp",
                    a,
                );
            }
            for (i, a) in t.listen_tls.iter().enumerate() {
                add(
                    format!("webrtc.turn.listen_tls[{i}]"),
                    format!("webrtc.turn.listen_tls[{i}]"),
                    "tcp",
                    a,
                );
            }
        }
        if let Some(m) = &w.media {
            add(
                "webrtc.media".into(),
                "webrtc.media.listen".into(),
                "udp",
                &m.listen,
            );
        }
        if let Some(p) = &w.proxy {
            add(
                "webrtc.proxy".into(),
                "webrtc.proxy.listen".into(),
                "udp",
                &p.listen,
            );
        }
    }
    out
}

/// The host of every address: an empty host, `0.0.0.0` or `::`.
const ANY_HOST: &str = "*";

/// The host and port `address` binds, the host normalized: [`ANY_HOST`] for every address,
/// `127.0.0.1` for `localhost`, an IP address in its shortest form, a host name in lower case.
/// `None` for an address that is not a host and port.
pub(crate) fn bind_key(address: &str) -> Option<(String, u16)> {
    let (h, p) = if let Some(rest) = address.strip_prefix('[') {
        let (h, p) = rest.split_once(']')?;
        (h, p.strip_prefix(':')?)
    } else {
        let (h, p) = address.rsplit_once(':')?;
        if h.contains(':') {
            return None;
        }
        (h, p)
    };
    let port = p.parse::<u16>().ok()?;
    if h.is_empty() {
        return Some((ANY_HOST.to_owned(), port));
    }
    if let Ok(ip) = h.parse::<IpAddr>() {
        let ip = crate::model::unmap(ip);
        if ip.is_unspecified() {
            return Some((ANY_HOST.to_owned(), port));
        }
        return Some((ip.to_string(), port));
    }
    let h = h.to_ascii_lowercase();
    if h == "localhost" {
        return Some(("127.0.0.1".to_owned(), port));
    }
    Some((h, port))
}

/// Whether two normalized hosts take some of the same connections.
fn hosts_overlap(a: &str, b: &str) -> bool {
    a == b || a == ANY_HOST || b == ANY_HOST
}

/// How two addresses written differently are one.
fn overlap_note(first: &str, first_host: &str, second: &str, second_host: &str) -> String {
    if first == second {
        String::new()
    } else if first_host == second_host {
        format!(", the same address as {second}")
    } else if first_host == ANY_HOST {
        format!(", every address, {second} among them")
    } else {
        format!(", which {second}, every address, takes in")
    }
}

/// The clashes among the config's addresses: each the path of the later address and its message.
pub(crate) fn clashes(
    listeners: &[(Listener, String)],
    admin: Option<&Admin>,
    tunnel: Option<&Tunnel>,
    llm: Option<&Llm>,
    webrtc: Option<&WebRtc>,
) -> Vec<(String, String)> {
    let mut kept: Vec<(Bind, String, u16)> = Vec::new();
    let mut out = Vec::new();
    for b in binds(listeners, admin, tunnel, llm, webrtc) {
        // Port 0 picks a free port, so two of them do not clash.
        let Some((host, port)) = bind_key(&b.address).filter(|(_, p)| *p != 0) else {
            continue;
        };
        if let Some((k, khost, _)) = kept.iter().find(|(k, khost, kport)| {
            k.network == b.network && *kport == port && hosts_overlap(khost, &host)
        }) {
            out.push((
                b.path.clone(),
                format!(
                    "{} listens on {} {} already{}; two servers on one address would share its \
                     connections, so give one of them another port",
                    k.name,
                    b.network.to_ascii_uppercase(),
                    k.address,
                    overlap_note(&k.address, khost, &b.address, &host)
                ),
            ));
        }
        kept.push((b, host, port));
    }
    out
}

/// Reports the clashes in the decoders: each at the position its decoder recorded for its path
/// ([`Decoder::addresses`]), or at the top of the first file when none did (a listener HTTPS by
/// default made).
pub(crate) fn report(ds: &mut [Decoder<'_>], clashes: Vec<(String, String)>) {
    let mut at: HashMap<String, (usize, usize)> = HashMap::new();
    for (file, d) in ds.iter().enumerate() {
        for (path, off) in &d.addresses {
            at.entry(path.clone()).or_insert((file, *off));
        }
    }
    for (path, msg) in clashes {
        let (file, off) = at.get(&path).copied().unwrap_or((0, 0));
        if let Some(d) = ds.get_mut(file) {
            d.error(off, &path, msg);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::from_json;

    fn errors(doc: &str) -> Vec<String> {
        match from_json(doc, None) {
            Ok(_) => Vec::new(),
            Err(e) => {
                e.0.iter()
                    .map(|e| format!("{}: {}", e.path, e.msg))
                    .collect()
            }
        }
    }

    /// Every kind of address akit listens on, each as a config member with `{a}` its address.
    fn kinds() -> Vec<(&'static str, &'static str, &'static str)> {
        vec![
            (
                r#"listener "tcp""#,
                "tcp",
                r#""listeners": [{"name": "tcp", "address": "{a}"}], "routes": [{"handler": {"module": "respond"}}]"#,
            ),
            (
                r#"listener "udp""#,
                "udp",
                r#""listeners": [{"name": "udp", "address": "{a}", "protocols": ["udp"]}], "services": [{"name": "s", "listeners": ["udp"], "upstream": "u"}], "upstreams": [{"name": "u", "addresses": ["127.0.0.1:9"]}]"#,
            ),
            ("admin", "tcp", r#""admin": {"address": "{a}"}"#),
            (
                "llm.hub.listen",
                "tcp",
                r#""llm": {"hub": {"listen": {"address": "{a}"}}}"#,
            ),
            (
                "llm.proxy.api",
                "tcp",
                r#""llm": {"proxy": {"api": {"address": "{a}"}}}"#,
            ),
            (
                "tunnel.hub.listeners[0]",
                "tcp",
                r#""tunnel": {"hub": {"listeners": [{"protocol": "tls", "address": "{a}"}]}}"#,
            ),
            (
                "tunnel.hub.listeners[0]",
                "udp",
                r#""tunnel": {"hub": {"listeners": [{"protocol": "quic", "address": "{a}"}]}}"#,
            ),
            (
                "tunnel.proxy.forwards[0]",
                "tcp",
                r#""tunnel": {"proxy": {"forwards": [{"listen": "{a}", "node": "n", "service": "s"}]}}"#,
            ),
            // I110: fit's bindings_test.go's WebRTC kinds.
            (
                "webrtc.turn.listen[0]",
                "udp",
                r#""webrtc": {"turn": {"listen": ["{a}"], "realm": "r", "secret": "s", "relay_ip": "10.0.0.5"}}"#,
            ),
            (
                "webrtc.turn.listen_tcp[0]",
                "tcp",
                r#""webrtc": {"turn": {"listen": ["127.0.0.1:0"], "listen_tcp": ["{a}"], "realm": "r", "secret": "s", "relay_ip": "10.0.0.5"}}"#,
            ),
            (
                "webrtc.media",
                "udp",
                r#""webrtc": {"media": {"listen": "{a}"}}"#,
            ),
            (
                "webrtc.proxy",
                "udp",
                r#""webrtc": {"proxy": {"listen": "{a}"}}"#,
            ),
        ]
    }

    /// fit's `TestBindsEveryKindClashes`: two kinds that share a transport clash on one address,
    /// with an error naming the first; a TCP one and a UDP one do not.
    #[test]
    fn every_kind_clashes() {
        let ks = kinds();
        for (i, (an, anet, a)) in ks.iter().enumerate() {
            for (bn, bnet, b) in ks.iter().skip(i + 1) {
                // Two members of one object (two llm roles) are written as one.
                if a.split(':').next() == b.split(':').next() {
                    continue;
                }
                let doc = format!(
                    "{{{}, {}}}",
                    a.replace("{a}", "127.0.0.1:7000"),
                    b.replace("{a}", "127.0.0.1:7000")
                );
                let errs = errors(&doc);
                let clash = errs.iter().any(|e| e.contains(&format!("{an} listens on")));
                assert_eq!(clash, anet == bnet, "{an} and {bn}: {errs:?}");
            }
        }
    }

    #[test]
    fn normalized() {
        for (first, second, note) in [
            (
                ":7000",
                "127.0.0.1:7000",
                ", every address, 127.0.0.1:7000 among them",
            ),
            (
                "127.0.0.1:7000",
                "0.0.0.0:7000",
                ", which 0.0.0.0:7000, every address, takes in",
            ),
            (
                "localhost:7000",
                "127.0.0.1:7000",
                ", the same address as 127.0.0.1:7000",
            ),
            ("[::]:7000", "[::1]:7000", ""),
        ] {
            let doc = format!(
                r#"{{"admin": {{"address": "{first}", "token_file": "t", "cert_file": "c", "key_file": "k"}}, "tunnel": {{"proxy": {{"forwards": [{{"listen": "{second}", "node": "n", "service": "s"}}]}}}}}}"#
            );
            let errs = errors(&doc);
            assert!(
                errs.iter().any(|e| e
                    .starts_with("tunnel.proxy.forwards[0].listen: admin listens on TCP")
                    && (note.is_empty() || e.contains(note))),
                "{first} {second}: {errs:?}"
            );
        }
        // Port 0, and other ports, do not clash.
        assert!(errors(
            r#"{"tunnel": {"proxy": {"forwards": [{"listen": "127.0.0.1:0", "node": "n", "service": "s"}, {"listen": "127.0.0.1:0", "node": "n", "service": "s"}]}}}"#
        )
        .is_empty());
        assert!(errors(
            r#"{"tunnel": {"proxy": {"forwards": [{"listen": "127.0.0.1:1", "node": "n", "service": "s"}, {"listen": "127.0.0.2:1", "node": "n", "service": "s"}]}}}"#
        )
        .is_empty());
    }

    /// fit's `tunnel/config_test.go` clashes.
    #[test]
    fn within_a_role() {
        let errs = errors(
            r#"{"tunnel": {"hub": {"listeners": [{"protocol": "tcp", "address": "127.0.0.1:7000"}, {"protocol": "ws", "address": "127.0.0.1:7000"}]}}}"#,
        );
        assert!(
            errs.iter().any(
                |e| e.contains("tunnel.hub.listeners[0] listens on TCP 127.0.0.1:7000 already")
            ),
            "{errs:?}"
        );
        let errs = errors(
            r#"{"tunnel": {"proxy": {"forwards": [{"listen": "127.0.0.1:1", "node": "n", "service": "s"}, {"listen": "127.0.0.1:1", "node": "n", "service": "t"}]}}}"#,
        );
        assert!(
            errs.iter()
                .any(|e| e.contains("tunnel.proxy.forwards[0] listens on TCP 127.0.0.1:1 already")),
            "{errs:?}"
        );
        assert_eq!(bind_key("Example.COM:1"), Some(("example.com".into(), 1)));
        assert_eq!(bind_key("[::ffff:0.0.0.0]:1"), Some(("*".into(), 1)));
        assert_eq!(bind_key("nope"), None);
    }
}
