//! HTTPS by default (I33), as fit's `config/autohttps.go`: a config that writes no listeners is
//! served over HTTPS, with certificates obtained with ACME for its routes' host names, and over
//! HTTP only to be redirected (and to answer HTTP-01); a listener whose `tls` is empty gets
//! certificates for the host names of the routes it serves. Listeners written out are served as
//! written. `auto_https` turns it off, turns the redirects off, or moves the ports.
//!
//! The listeners it makes are worked out once, when the config is read, and are what
//! [`Config::listeners`](crate::Config::listeners) holds: the runtime binds, diffs and serves
//! them as any other (rule 12).

use std::net::IpAddr;

use crate::decode::Decoder;
use crate::json::{Kind, Node};
use crate::model::{
    DEFAULT_MAX_BODY_BYTES, DEFAULT_PROTOCOLS, DEFAULT_TLS_PROTOCOLS, Listener, Route, Tls,
    TlsVersion,
};

/// fit's default `auto_https.http_port`.
pub const DEFAULT_HTTP_PORT: u16 = 80;
/// fit's default `auto_https.https_port`.
pub const DEFAULT_HTTPS_PORT: u16 = 443;
/// The name of the listener in the clear HTTPS by default makes, which redirects to HTTPS.
pub const AUTO_HTTP_LISTENER: &str = "http";
/// The name of the HTTPS listener HTTPS by default makes.
pub const AUTO_HTTPS_LISTENER: &str = "https";

/// `auto_https`, as fit's `config.AutoHTTPS`: on, with redirects, on ports 80 and 443, when left
/// out.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AutoHttps {
    /// `disable`: no listeners are made for a config that writes none, and an empty `tls` gets
    /// no certificates.
    pub disable: bool,
    /// `disable_redirects`: no listener in the clear is made to redirect to HTTPS; HTTP-01 is then
    /// answered by none, and certificates come through TLS-ALPN-01.
    pub disable_redirects: bool,
    /// `http_port`: the port of the listener in the clear; [`DEFAULT_HTTP_PORT`] when left out
    /// or 0.
    pub http_port: u16,
    /// `https_port`: the port of the HTTPS listener, and the one redirects name when it is not
    /// 443; [`DEFAULT_HTTPS_PORT`] when left out or 0.
    pub https_port: u16,
}

impl Default for AutoHttps {
    fn default() -> AutoHttps {
        AutoHttps {
            disable: false,
            disable_redirects: false,
            http_port: DEFAULT_HTTP_PORT,
            https_port: DEFAULT_HTTPS_PORT,
        }
    }
}

impl AutoHttps {
    fn on(&self) -> bool {
        !self.disable
    }

    fn redirects(&self) -> bool {
        !self.disable_redirects
    }
}

/// The names, and the parents of names, that only a private network resolves, which no public CA
/// gives a certificate for (fit's `privateSuffixes`).
const PRIVATE_SUFFIXES: &[&str] = &["localhost", "local", "internal", "home.arpa", "invalid"];

/// Whether HTTPS by default obtains a certificate for `host` (fit's `AutoHTTPSName`): a host name
/// of two labels or more, and not a wildcard (`*.` or `**.`), an IP address or a name only a
/// private network resolves.
#[must_use]
pub fn auto_https_name(host: &str) -> bool {
    let name = host.strip_suffix('.').unwrap_or(host).to_ascii_lowercase();
    if name.starts_with('*')
        || name.trim_matches(['[', ']']).parse::<IpAddr>().is_ok()
        || !name.contains('.')
    {
        return false;
    }
    !PRIVATE_SUFFIXES
        .iter()
        .any(|s| name == *s || name.strip_suffix(s).is_some_and(|rest| rest.ends_with('.')))
}

/// The host names HTTPS by default obtains certificates for among the routes `listener` serves,
/// or every route's when `listener` is `None`: in lower case, each once, in the order written
/// (fit's `autoNames`).
pub(crate) fn auto_names<'r>(
    routes: impl IntoIterator<Item = &'r Route>,
    listener: Option<&str>,
) -> Vec<String> {
    let mut names: Vec<String> = Vec::new();
    for r in routes {
        let Some(m) = &r.matcher else { continue };
        if let Some(l) = listener {
            if !r.listeners.is_empty() && !r.listeners.iter().any(|x| x == l) {
                continue;
            }
        }
        for h in &m.host {
            let h = h.strip_suffix('.').unwrap_or(h).to_ascii_lowercase();
            if auto_https_name(&h) && !names.contains(&h) {
                names.push(h);
            }
        }
    }
    names
}

/// `auto_https`, whose fields are `fields`, with where a problem with its ports is reported:
/// `http_port`'s value when written, the object's otherwise.
pub(crate) fn decode(
    d: &mut Decoder<'_>,
    n: &Node,
    path: &str,
    fields: &[&str],
) -> Option<(AutoHttps, usize)> {
    let mut o = d.object(n, path)?;
    let mut a = AutoHttps::default();
    for (key, slot) in [
        ("disable", &mut a.disable),
        ("disable_redirects", &mut a.disable_redirects),
    ] {
        match o.take(key).map(|v| (v.off, &v.kind)) {
            None => {}
            Some((_, Kind::Bool(b))) => *slot = *b,
            Some((off, other)) => d.error(
                off,
                &o.at(key),
                format!("expected a boolean, got {}", other.name()),
            ),
        }
    }
    let mut off = n.off;
    for (key, slot, default) in [
        ("http_port", &mut a.http_port, DEFAULT_HTTP_PORT),
        ("https_port", &mut a.https_port, DEFAULT_HTTPS_PORT),
    ] {
        if let Some(v) = o.take(key) {
            if key == "http_port" {
                off = v.off;
            }
            if let Some(p) = d
                .int(v, &o.at(key), 0, i64::from(u16::MAX))
                .and_then(|p| u16::try_from(p).ok())
            {
                *slot = if p == 0 { default } else { p };
            }
        }
    }
    o.finish(d, fields, &[]);
    Some((a, off))
}

/// The position of a listener's `tls` when it asks for nothing (fit's `emptyTLS`): no
/// certificates, no names in `acme`, and `on_demand` not set.
pub(crate) fn empty_tls(listener: &Node) -> Option<usize> {
    let Kind::Object(members) = &listener.kind else {
        return None;
    };
    let tls = members.iter().find(|m| m.key == "tls")?;
    let Kind::Object(fields) = &tls.value.kind else {
        return None;
    };
    let empty = fields.iter().all(|m| match m.key.as_str() {
        "certificates" | "acme" => matches!(&m.value.kind, Kind::Array(a) if a.is_empty()),
        "on_demand" => !matches!(&m.value.kind, Kind::Bool(true)),
        _ => true,
    });
    empty.then_some(tls.value.off)
}

/// What [`effective`] makes of a config's listeners.
pub(crate) enum Effective {
    /// The listeners written, those with an empty `tls` given their routes' names; for each
    /// listener with an empty `tls` that gets none, its index and the problem.
    Written(Vec<Listener>, Vec<(usize, &'static str)>),
    /// Those HTTPS by default makes for a config that writes none.
    Made(Vec<Listener>),
    /// None written and none made: why.
    None(&'static str),
    /// None written, and the HTTP port is the HTTPS one: the port.
    OnePort(u16),
}

/// The listeners a config is served on (fit's `Config.Effective`). `written` are the listeners
/// written, each with whether its `tls` is empty ([`empty_tls`]).
pub(crate) fn effective(
    auto: &AutoHttps,
    written: Vec<(Listener, bool)>,
    routes: &[&Route],
) -> Effective {
    if !written.is_empty() {
        let mut problems = Vec::new();
        let listeners = written
            .into_iter()
            .enumerate()
            .map(|(i, (mut l, empty))| {
                if let (true, Some(t)) = (empty, l.tls.as_mut()) {
                    if !auto.on() {
                        problems.push((i, "a certificate, a name for acme or on_demand is needed"));
                    } else {
                        t.acme = auto_names(routes.iter().copied(), Some(&l.name));
                        if t.acme.is_empty() {
                            problems.push((
                                i,
                                "a certificate, a name for acme or on_demand is needed, or a route on this listener that matches a host name for HTTPS by default",
                            ));
                        }
                    }
                }
                l
            })
            .collect();
        return Effective::Written(listeners, problems);
    }
    if !auto.on() {
        return Effective::None("at least one listener is needed");
    }
    let names = auto_names(routes.iter().copied(), None);
    if names.is_empty() {
        return Effective::None(
            "at least one listener is needed, or a route that matches a host name for HTTPS by default",
        );
    }
    if auto.redirects() && auto.http_port == auto.https_port {
        return Effective::OnePort(auto.http_port);
    }
    let mut listeners = vec![Listener {
        name: AUTO_HTTPS_LISTENER.to_owned(),
        address: format!(":{}", auto.https_port),
        protocols: DEFAULT_TLS_PROTOCOLS.to_vec(),
        tls: Some(Tls {
            certificates: Vec::new(),
            client_auth: None,
            max_version: TlsVersion::Tls13,
            acme: names,
            on_demand: false,
        }),
        max_body_bytes: DEFAULT_MAX_BODY_BYTES,
        redirect_https: 0,
        proxy_protocol: None,
        socks: None,
    }];
    if auto.redirects() {
        listeners.push(Listener {
            name: AUTO_HTTP_LISTENER.to_owned(),
            address: format!(":{}", auto.http_port),
            protocols: DEFAULT_PROTOCOLS.to_vec(),
            tls: None,
            max_body_bytes: DEFAULT_MAX_BODY_BYTES,
            redirect_https: auto.https_port,
            proxy_protocol: None,
            socks: None,
        });
    }
    Effective::Made(listeners)
}

#[cfg(test)]
mod tests {
    use super::auto_https_name;

    /// fit's `TestAutoHTTPSName`.
    #[test]
    fn names() {
        for (name, want) in [
            ("a.example", true),
            ("WWW.Example.COM.", true),
            ("x.fit.test", true),
            ("localhost", false),
            ("a.localhost", false),
            ("printer.local", false),
            ("db.internal", false),
            ("nas.home.arpa", false),
            ("x.invalid", false),
            ("intranet", false),
            ("*.a.example", false),
            ("**.a.example", false),
            ("10.0.0.1", false),
            ("[::1]", false),
            ("notlocal.example", true),
            ("xlocalhost.example", true),
        ] {
            assert_eq!(auto_https_name(name), want, "{name}");
        }
    }
}
