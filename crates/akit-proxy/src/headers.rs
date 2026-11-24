//! The header work of the proxy, as fit's `handler/proxy/proxy.go` does it: which fields of a
//! request go on to the upstream and which of a response come back, and the fields that tell the
//! upstream who the client is (`X-Forwarded-For`, `-Proto`, `-Host` and `Forwarded`, RFC 7239).
//!
//! A request's fields go on in the order and case they arrived, as fit's `outgoingFramed` passes
//! them, without the hop-by-hop ones, those its `Connection` names, its framing and the
//! forwarding fields, which the proxy's own replace (after the client's), unless the client is a
//! trusted proxy: then what it forwarded is kept and the client added to it.

use std::fmt::Write as _;
use std::net::{IpAddr, SocketAddr};

use akio_http::Headers;
use akit_config::Network;

/// What [`outgoing`] does with a field of the client's request, by name.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Field {
    /// Passed on as it came.
    Keep,
    /// Dropped: hop-by-hop, the request's framing, or replaced by the proxy (`Host`).
    Drop,
    /// `X-Forwarded-For`: the client is added to it when it is a trusted proxy.
    ForwardedFor,
    /// `X-Forwarded-Proto` and `X-Forwarded-Host`: kept from a trusted proxy, else replaced.
    ForwardedKept,
    /// `Forwarded`: the client's element is added to it when it is a trusted proxy.
    Forwarded,
}

/// What [`outgoing`] does with the field `name`, in any case: fit's `framedRequestField`, by
/// length first, as most names are none of these.
pub(crate) fn request_field(name: &str) -> Field {
    let is = |n: &str| name.eq_ignore_ascii_case(n);
    match name.len() {
        2 if is("te") => Field::Drop,
        4 if is("host") => Field::Drop,
        // The client's own server answers 100 Continue, having read the body.
        6 if is("expect") => Field::Drop,
        7 if is("upgrade") || is("trailer") => Field::Drop,
        9 if is("forwarded") => Field::Forwarded,
        10 if is("connection") || is("keep-alive") => Field::Drop,
        14 if is("content-length") => Field::Drop,
        15 if is("x-forwarded-for") => Field::ForwardedFor,
        16 if is("proxy-connection") => Field::Drop,
        16 if is("x-forwarded-host") => Field::ForwardedKept,
        17 if is("transfer-encoding") => Field::Drop,
        17 if is("x-forwarded-proto") => Field::ForwardedKept,
        18 if is("proxy-authenticate") => Field::Drop,
        19 if is("proxy-authorization") => Field::Drop,
        _ if name.starts_with(':') => Field::Drop,
        _ => Field::Keep,
    }
}

/// Whether `name`, in any case, is a field that concerns one connection only (RFC 9110 section
/// 7.6.1), or `Keep-Alive` or `Proxy-Connection`, which older peers send; or `Content-Length`,
/// which a response is framed by afresh. fit's `isHopByHopFold`.
pub(crate) fn is_hop_by_hop(name: &str) -> bool {
    let is = |n: &str| name.eq_ignore_ascii_case(n);
    match name.len() {
        2 => is("te"),
        7 => is("trailer") || is("upgrade"),
        10 => is("connection") || is("keep-alive"),
        14 => is("content-length"),
        16 => is("proxy-connection"),
        17 => is("transfer-encoding"),
        18 => is("proxy-authenticate"),
        19 => is("proxy-authorization"),
        _ => false,
    }
}

/// Whether one of `connection`'s values, a `Connection` field's, names the field `name`.
pub(crate) fn connection_names<'a>(
    connection: impl IntoIterator<Item = &'a str>,
    name: &str,
) -> bool {
    connection.into_iter().any(|v| {
        v.split(',')
            .any(|t| t.trim_matches([' ', '\t']).eq_ignore_ascii_case(name))
    })
}

/// Whether `s` is an HTTP token, as a field name must be.
pub(crate) fn valid_token(s: &str) -> bool {
    !s.is_empty()
        && s.bytes().all(|c| {
            c.is_ascii_alphanumeric()
                || matches!(
                    c,
                    b'!' | b'#'
                        | b'$'
                        | b'%'
                        | b'&'
                        | b'\''
                        | b'*'
                        | b'+'
                        | b'-'
                        | b'.'
                        | b'^'
                        | b'_'
                        | b'`'
                        | b'|'
                        | b'~'
                )
        })
}

/// The client's address as the forwarding fields write it: an IPv4-mapped address as IPv4, an
/// IPv6 one without its zone (std writes none), in Go's canonical form. fit's `clientAddr`.
pub(crate) fn client_ip(peer: SocketAddr) -> IpAddr {
    match peer.ip() {
        IpAddr::V6(v6) => v6.to_ipv4_mapped().map_or(IpAddr::V6(v6), IpAddr::V4),
        ip @ IpAddr::V4(_) => ip,
    }
}

/// Whether `ip` is one of `trusted`'s.
pub(crate) fn is_trusted(trusted: &[Network], ip: IpAddr) -> bool {
    trusted.iter().any(|n| n.contains(ip))
}

/// Appends to `dst` a `Forwarded` element (RFC 7239) for the client at `ip` (`None` when its
/// address is not an IP one), asking for `host` over `proto`. fit's `appendForwardedElement`.
pub(crate) fn forwarded_element(dst: &mut String, ip: Option<IpAddr>, host: &str, proto: &str) {
    dst.push_str("for=");
    match ip {
        None => dst.push_str("unknown"),
        Some(IpAddr::V6(v6)) => {
            let _ = write!(dst, "\"[{v6}]\"");
        }
        Some(IpAddr::V4(v4)) => {
            let _ = write!(dst, "{v4}");
        }
    }
    if !host.is_empty() {
        dst.push_str(";host=");
        forwarded_value(dst, host);
    }
    dst.push_str(";proto=");
    dst.push_str(proto);
}

/// Appends `v` as a `Forwarded` parameter value: a token as it is, anything else quoted, as Go's
/// `strconv.Quote` would.
fn forwarded_value(dst: &mut String, v: &str) {
    if valid_token(v) {
        dst.push_str(v);
        return;
    }
    dst.push('"');
    for c in v.chars() {
        match c {
            '"' => dst.push_str("\\\""),
            '\\' => dst.push_str("\\\\"),
            ' '..='~' => dst.push(c),
            '\x07' => dst.push_str("\\a"),
            '\x08' => dst.push_str("\\b"),
            '\x0c' => dst.push_str("\\f"),
            '\n' => dst.push_str("\\n"),
            '\r' => dst.push_str("\\r"),
            '\t' => dst.push_str("\\t"),
            '\x0b' => dst.push_str("\\v"),
            c if (c as u32) < 0x20 || c == '\x7f' => {
                let _ = write!(dst, "\\x{:02x}", c as u32);
            }
            // Printable beyond ASCII, as strconv.Quote keeps it.
            c => dst.push(c),
        }
    }
    dst.push('"');
}

/// What the proxy knows of a request, for [`outgoing`].
pub(crate) struct Client<'a> {
    /// The request's fields.
    pub headers: &'a Headers,
    /// Its peer.
    pub peer: SocketAddr,
    /// Whether it came over TLS.
    pub tls: bool,
    /// The `traceparent` naming akit's span of a request it traces (I64), which goes on in place
    /// of the client's, after the forwarding fields, as fit's; `None` for one it does not, whose
    /// own goes on as it came.
    pub traceparent: Option<&'a str>,
}

/// Why a request is not forwarded.
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum Refused {
    /// A field name that is not a token: RFC 9112 asks a server to refuse it, and an upstream
    /// could read it differently (fit answers 400).
    BadFieldName,
}

/// The fields of the upstream's request for the client's `c`: `Host` first, the client's own
/// (`host_header` replacing it when set), then the client's fields in the order and case they
/// came but those [`request_field`] drops and those its `Connection` names, then the forwarding
/// fields, then akit's `traceparent` for a request it traces; a `User-Agent` the client did not
/// send is not added either (an empty one, which akio's
/// client sends as none).
pub(crate) fn outgoing(
    c: &Client<'_>,
    trusted: &[Network],
    host_header: &str,
) -> Result<Headers, Refused> {
    let client_host = c.headers.get("host").unwrap_or("");
    let ip = client_ip(c.peer);
    let trusted = is_trusted(trusted, ip);
    let proto = if c.tls { "https" } else { "http" };

    let mut h = Headers::with_capacity(c.headers.len().saturating_add(5), 256);
    let host = if host_header.is_empty() {
        client_host
    } else {
        host_header
    };
    if !host.is_empty() {
        h.append("Host", host);
    }
    // What a trusted proxy forwarded: added to, rather than replaced.
    let mut prior_for = String::new();
    let mut prior_forwarded = String::new();
    let (mut kept_proto, mut kept_host) = (false, false);
    let mut user_agent = false;
    let connection = || c.headers.get_all("connection");
    for (name, value) in c.headers.iter() {
        if !valid_token(name) {
            return Err(Refused::BadFieldName);
        }
        let field = request_field(name);
        if field == Field::Drop
            || connection_names(connection(), name)
            || (c.traceparent.is_some() && name.eq_ignore_ascii_case("traceparent"))
        {
            continue;
        }
        match field {
            Field::Keep => {
                user_agent |= name.eq_ignore_ascii_case("user-agent");
                h.append(name, value);
            }
            Field::ForwardedFor if trusted => join(&mut prior_for, value),
            Field::Forwarded if trusted => join(&mut prior_forwarded, value),
            Field::ForwardedKept if trusted => {
                if name.len() == "x-forwarded-proto".len() {
                    kept_proto = true;
                } else {
                    kept_host = true;
                }
                h.append(name, value);
            }
            _ => {}
        }
    }
    if !user_agent {
        h.append("User-Agent", "");
    }
    let mut v = String::with_capacity(64);
    if !prior_for.is_empty() {
        v.push_str(&prior_for);
        v.push_str(", ");
    }
    let _ = write!(v, "{ip}");
    h.append("X-Forwarded-For", &v);
    if !kept_proto {
        h.append("X-Forwarded-Proto", proto);
    }
    if !kept_host {
        h.append("X-Forwarded-Host", client_host);
    }
    v.clear();
    if !prior_forwarded.is_empty() {
        v.push_str(&prior_forwarded);
        v.push_str(", ");
    }
    forwarded_element(&mut v, Some(ip), client_host, proto);
    h.append("Forwarded", &v);
    if let Some(t) = c.traceparent {
        h.append("Traceparent", t);
    }
    Ok(h)
}

/// Appends `value` to `list`, a field's values joined by `", "`.
fn join(list: &mut String, value: &str) {
    if !list.is_empty() {
        list.push_str(", ");
    }
    list.push_str(value);
}

/// The trailer fields `h` announces in its `Trailer` fields, in the order and case given, each
/// once: fit's `trailerNames`. Names that are no token are passed over.
pub(crate) fn trailer_names(h: &Headers) -> Vec<String> {
    let mut names: Vec<String> = Vec::new();
    for value in h.get_all("trailer") {
        for name in value.split(',').map(|n| n.trim_matches([' ', '\t'])) {
            if valid_token(name) && !names.iter().any(|n| n.eq_ignore_ascii_case(name)) {
                names.push(name.to_owned());
            }
        }
    }
    names
}

/// The fields of `trailers` that `names` announced: fit's `announcedTrailer`.
pub(crate) fn announced(trailers: &Headers, names: &[String]) -> Headers {
    let mut h = Headers::new();
    for (name, value) in trailers.iter() {
        if names.iter().any(|n| n.eq_ignore_ascii_case(name)) {
            h.append(name, value);
        }
    }
    h
}

/// The fields of the client's response for the upstream's `upstream`: in the upstream's order
/// and case, without the hop-by-hop ones and those its `Connection` names, and without
/// `Content-Length`, which the response's own framing gives, unless `keep_length` (a response to
/// `HEAD`, which has no body to frame it). fit's `responseBlock`.
pub(crate) fn incoming(upstream: &Headers, keep_length: bool) -> Headers {
    let mut h = Headers::with_capacity(upstream.len(), 256);
    for (name, value) in upstream.iter() {
        let length = keep_length && name.eq_ignore_ascii_case("content-length");
        if (is_hop_by_hop(name) && !length)
            || connection_names(upstream.get_all("connection"), name)
        {
            continue;
        }
        h.append(name, value);
    }
    h
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]

    use super::*;

    /// fit's `TestFramedFieldNames`: what each name the proxy looks for gets, in any case, and
    /// names of the same lengths it keeps.
    #[test]
    fn field_names() {
        let request = [
            ("te", Field::Drop),
            ("HOST", Field::Drop),
            ("expect", Field::Drop),
            ("UPGRADE", Field::Drop),
            ("trailer", Field::Drop),
            ("forwarded", Field::Forwarded),
            ("connection", Field::Drop),
            ("KEEP-ALIVE", Field::Drop),
            ("content-length", Field::Drop),
            ("x-forwarded-for", Field::ForwardedFor),
            ("proxy-connection", Field::Drop),
            ("X-FORWARDED-HOST", Field::ForwardedKept),
            ("transfer-encoding", Field::Drop),
            ("x-forwarded-proto", Field::ForwardedKept),
            ("proxy-authenticate", Field::Drop),
            ("proxy-authorization", Field::Drop),
            (":protocol", Field::Drop),
            ("ab", Field::Keep),
            ("abcd", Field::Keep),
            ("abcdef", Field::Keep),
            ("abcdefg", Field::Keep),
            ("abcdefghi", Field::Keep),
            ("abcdefghij", Field::Keep),
            ("abcdefghijklmn", Field::Keep),
            ("abcdefghijklmno", Field::Keep),
            ("abcdefghijklmnop", Field::Keep),
            ("abcdefghijklmnopq", Field::Keep),
            ("abcdefghijklmnopqr", Field::Keep),
            ("abcdefghijklmnopqrs", Field::Keep),
            ("x", Field::Keep),
        ];
        for (name, want) in request {
            assert_eq!(request_field(name), want, "{name}");
        }
        for name in [
            "te",
            "TRAILER",
            "upgrade",
            "connection",
            "keep-alive",
            "content-length",
            "proxy-connection",
            "TRANSFER-ENCODING",
            "proxy-authenticate",
            "proxy-authorization",
        ] {
            assert!(is_hop_by_hop(name), "{name}");
        }
        for name in [
            "ab",
            "abcdefg",
            "abcdefghij",
            "abcdefghijklmn",
            "abcdefghijklmnop",
            "abcdefghijklmnopq",
            "abcdefghijklmnopqr",
            "abcdefghijklmnopqrs",
            "x",
        ] {
            assert!(!is_hop_by_hop(name), "{name}");
        }
    }

    /// fit's `TestForwardedElement`.
    #[test]
    fn forwarded_elements() {
        let cases: [(Option<&str>, &str, &str, &str); 4] = [
            (
                Some("192.0.2.1"),
                "a.example",
                "http",
                "for=192.0.2.1;host=a.example;proto=http",
            ),
            (
                Some("2001:db8::1"),
                "a.example:8443",
                "https",
                r#"for="[2001:db8::1]";host="a.example:8443";proto=https"#,
            ),
            (None, "", "http", "for=unknown;proto=http"),
            (
                Some("192.0.2.1"),
                "a\"b\\c d",
                "http",
                r#"for=192.0.2.1;host="a\"b\\c d";proto=http"#,
            ),
        ];
        for (ip, host, proto, want) in cases {
            let mut s = String::new();
            forwarded_element(&mut s, ip.map(|i| i.parse().unwrap()), host, proto);
            assert_eq!(s, want);
        }
        for (peer, want) in [
            ("192.0.2.1:80", "192.0.2.1"),
            ("[::ffff:192.0.2.1]:80", "192.0.2.1"),
            ("[2001:DB8::1]:80", "2001:db8::1"),
            ("[fe80::1%1]:80", "fe80::1"),
        ] {
            let peer: SocketAddr = peer.parse().unwrap();
            assert_eq!(client_ip(peer).to_string(), want, "{peer}");
        }
    }

    fn headers(fields: &[(&str, &str)]) -> Headers {
        let mut h = Headers::new();
        for (n, v) in fields {
            h.append(n, v);
        }
        h
    }

    fn lines(h: &Headers) -> String {
        h.iter().map(|(n, v)| format!("{n}: {v}\n")).collect()
    }

    /// A traced request's upstream gets akit's `traceparent` in place of the client's, last, as
    /// fit's `outgoingFramed` sends it; `tracestate` goes on as it came, and an untraced request's
    /// `traceparent` too.
    #[test]
    fn traceparent_replaced_when_traced() {
        let h = headers(&[
            ("Host", "a"),
            (
                "traceparent",
                "00-4bf92f3577b34da6a3ce929d0e0e4736-00f067aa0ba902b7-01",
            ),
            ("tracestate", "k=v"),
        ]);
        let mine = "00-4bf92f3577b34da6a3ce929d0e0e4736-0102030405060708-01";
        let mut c = Client {
            headers: &h,
            peer: "127.0.0.1:1".parse().unwrap(),
            tls: false,
            traceparent: Some(mine),
        };
        let out = outgoing(&c, &[], "").unwrap();
        assert_eq!(out.get_all("traceparent").collect::<Vec<_>>(), [mine]);
        assert_eq!(out.iter().last().map(|(n, _)| n), Some("Traceparent"));
        assert_eq!(out.get("tracestate"), Some("k=v"));
        c.traceparent = None;
        let out = outgoing(&c, &[], "").unwrap();
        assert_eq!(
            out.get("traceparent"),
            Some("00-4bf92f3577b34da6a3ce929d0e0e4736-00f067aa0ba902b7-01")
        );
    }

    /// fit's `TestRefusedHeaderName`: a field name that is not a token is refused.
    #[test]
    fn refused_field_names() {
        let h = headers(&[("Host", "a"), ("X A", "1")]);
        let c = Client {
            headers: &h,
            peer: "127.0.0.1:1".parse().unwrap(),
            tls: false,
            traceparent: None,
        };
        assert_eq!(outgoing(&c, &[], "").err(), Some(Refused::BadFieldName));
        assert!(!valid_token(""));
    }

    /// fit's `TestFramedRequestHeader`, on the fields alone.
    #[test]
    fn request_fields_in_order() {
        let h = headers(&[
            ("Host", "app.example"),
            ("zz-first", "1"),
            ("connection", "x-hop, keep-alive"),
            ("x-hop", "1"),
            ("keep-alive", "5"),
            ("TE", "trailers"),
            ("proxy-authorization", "x"),
            ("x-forwarded-for", "203.0.113.9"),
            ("Forwarded", "for=203.0.113.9"),
            ("X-Forwarded-Host", "evil"),
            ("x-multi", "1"),
            ("Aa-Last", "2"),
            ("x-multi", "2"),
            ("content-length", "3"),
            ("User-Agent", "ua"),
        ]);
        let c = Client {
            headers: &h,
            peer: "127.0.0.1:5".parse().unwrap(),
            tls: false,
            traceparent: None,
        };
        assert_eq!(
            lines(&outgoing(&c, &[], "").unwrap()),
            "Host: app.example\nzz-first: 1\nx-multi: 1\nAa-Last: 2\nx-multi: 2\nUser-Agent: ua\n\
             X-Forwarded-For: 127.0.0.1\nX-Forwarded-Proto: http\nX-Forwarded-Host: app.example\n\
             Forwarded: for=127.0.0.1;host=app.example;proto=http\n"
        );
        // Over TLS, with host_header, and no User-Agent: an empty one, which sends none.
        let h = headers(&[("Host", "app.example")]);
        let c = Client {
            headers: &h,
            peer: "[2001:db8::7]:5".parse().unwrap(),
            tls: true,
            traceparent: None,
        };
        assert_eq!(
            lines(&outgoing(&c, &[], "internal.svc").unwrap()),
            "Host: internal.svc\nUser-Agent: \nX-Forwarded-For: 2001:db8::7\n\
             X-Forwarded-Proto: https\nX-Forwarded-Host: app.example\n\
             Forwarded: for=\"[2001:db8::7]\";host=app.example;proto=https\n"
        );
    }

    /// fit's `TestForwardedTrusted`, on the fields alone, with fields given twice.
    #[test]
    fn trusted_forwarding() {
        let trusted = [Network::parse("10.0.0.0/8").unwrap()];
        let h = headers(&[
            ("Host", "site.example"),
            ("X-Forwarded-For", "203.0.113.9"),
            ("x-forwarded-for", "198.51.100.1"),
            ("X-Forwarded-Proto", "https"),
            ("Forwarded", "for=203.0.113.9;proto=https"),
        ]);
        let c = Client {
            headers: &h,
            peer: "[::ffff:10.1.2.3]:5".parse().unwrap(),
            tls: false,
            traceparent: None,
        };
        let out = outgoing(&c, &trusted, "").unwrap();
        assert_eq!(
            out.get("x-forwarded-for"),
            Some("203.0.113.9, 198.51.100.1, 10.1.2.3")
        );
        assert_eq!(out.get("x-forwarded-proto"), Some("https"));
        assert_eq!(out.get("x-forwarded-host"), Some("site.example"));
        assert_eq!(
            out.get("forwarded"),
            Some("for=203.0.113.9;proto=https, for=10.1.2.3;host=site.example;proto=http")
        );
        // The same fields from a client that is not trusted are replaced.
        let c = Client {
            peer: "192.0.2.1:5".parse().unwrap(),
            ..c
        };
        let out = outgoing(&c, &trusted, "").unwrap();
        assert_eq!(out.get_all("x-forwarded-for").count(), 1);
        assert_eq!(out.get("x-forwarded-for"), Some("192.0.2.1"));
        assert_eq!(out.get("x-forwarded-proto"), Some("http"));
    }

    /// fit's `TestFramedResponseHeader` and `TestFramedResponseConnectionFields`, on the fields
    /// alone.
    #[test]
    fn response_fields() {
        let h = headers(&[
            ("zz", "1"),
            ("Date", "Mon, 02 Jan 2006 15:04:05 GMT"),
            ("Connection", "x-hop, keep-alive"),
            ("x-hop", "1"),
            ("Keep-Alive", "timeout=5"),
            ("Content-Length", "2"),
            ("Aa", "2"),
        ]);
        assert_eq!(
            lines(&incoming(&h, false)),
            "zz: 1\nDate: Mon, 02 Jan 2006 15:04:05 GMT\nAa: 2\n"
        );
        assert!(lines(&incoming(&h, true)).contains("Content-Length: 2\n"));
        let h = headers(&[
            ("Connection", "x-a"),
            ("Connection", "X-B"),
            ("Connection", "x-c, close"),
            ("x-a", "1"),
            ("x-b", "2"),
            ("X-C", "3"),
            ("kept", "4"),
            ("Proxy-Authenticate", "Basic"),
            ("Trailer", "X-T"),
            ("Upgrade", "foo"),
            ("Transfer-Encoding", "chunked"),
        ]);
        assert_eq!(lines(&incoming(&h, false)), "kept: 4\n");
    }

    /// fit's `trailerNames` and `announcedTrailer`: the names announced, once each, in any case,
    /// and the trailers of those names alone.
    #[test]
    fn trailers() {
        let h = headers(&[
            ("Trailer", "X-Sum, x-check"),
            ("X-Other", "1"),
            ("trailer", " X-SUM ,,X-Late\t, bad name"),
        ]);
        let names = trailer_names(&h);
        assert_eq!(names, ["X-Sum", "x-check", "X-Late"]);
        assert!(trailer_names(&headers(&[("X-Other", "1")])).is_empty());
        let t = headers(&[("x-sum", "3"), ("X-Unannounced", "no"), ("X-CHECK", "ok")]);
        assert_eq!(lines(&announced(&t, &names)), "x-sum: 3\nX-CHECK: ok\n");
        assert!(announced(&t, &[]).is_empty());
    }
}
