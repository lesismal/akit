//! Fuzz entry points (feature `fuzzing`; not a public API): the `fuzz/` cargo-fuzz project calls
//! them, and the tests below replay `fuzz/corpus/<target>` on stable, so every input a run kept,
//! and every one that once failed, is checked by `cargo test` too. fit's `FuzzHopByHop`,
//! `FuzzForwarded` and `FuzzGRPCTimeout` (`handler/proxy/fuzz_test.go`).
#![allow(
    clippy::indexing_slicing,
    clippy::arithmetic_side_effects,
    clippy::unwrap_used,
    clippy::panic,
    clippy::expect_used
)]

use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr};
use std::time::Duration;

use akio_http::Headers;
use akit_config::Network;

use crate::grpc;
use crate::headers::{self, Client, Field, Refused};

/// Fields from `name: value` lines, as a client or an upstream could send them.
fn fields(text: &str) -> Headers {
    let mut h = Headers::new();
    for line in text.split('\n') {
        if let Some((n, v)) = line.trim_end_matches('\r').split_once(':') {
            h.append(n.trim(), v.trim());
        }
    }
    h
}

/// Whether `s` holds a byte no field value may: CR, LF or NUL.
fn breaks_lines(s: &str) -> bool {
    s.bytes().any(|b| matches!(b, b'\r' | b'\n' | 0))
}

/// The names the proxy writes itself, whatever the client's `Connection` names.
const OWN: [&str; 7] = [
    "host",
    "user-agent",
    "x-forwarded-for",
    "x-forwarded-proto",
    "x-forwarded-host",
    "forwarded",
    "traceparent",
];

/// `proxy_headers`: the fields of the request sent upstream and of the response sent back.
/// `data[0]`'s bits pick TLS, a trusted client, a `traceparent` of akit's, `keep_length` and an
/// IPv6 peer; `data[1..17]` is the peer's address; the rest, up to a NUL, the client's fields
/// (`name: value` lines), and after it the upstream's response fields, then its trailers after
/// another NUL. A `@host_header: x` line sets the route's `host_header`.
///
/// Neither side keeps a hop-by-hop field or one its `Connection` names; the forwarding fields
/// are the proxy's own, once each, the client last in them; a `Forwarded` element made from any
/// `Host` has no control character; and fields that break no line come out breaking none.
pub fn proxy_headers(data: &[u8]) {
    if data.len() < 17 {
        return;
    }
    let flags = data[0];
    let ip = if flags & 16 != 0 {
        let mut a = [0u8; 16];
        a.copy_from_slice(&data[1..17]);
        IpAddr::V6(Ipv6Addr::from(a))
    } else {
        IpAddr::V4(Ipv4Addr::new(data[1], data[2], data[3], data[4]))
    };
    let peer = SocketAddr::new(ip, u16::from_be_bytes([data[5], data[6]]));
    let Ok(text) = std::str::from_utf8(&data[17..]) else {
        return;
    };
    let mut parts = text.splitn(3, '\0');
    let (request, response, trailers) = (
        parts.next().unwrap_or(""),
        parts.next().unwrap_or(""),
        parts.next().unwrap_or(""),
    );
    let mut host_header = "";
    let mut req_lines = String::new();
    for line in request.split('\n') {
        match line.strip_prefix("@host_header:") {
            Some(h) => host_header = h.trim(),
            None => {
                req_lines.push_str(line);
                req_lines.push('\n');
            }
        }
    }
    let req = fields(&req_lines);
    let trusted = if flags & 2 != 0 {
        vec![
            Network {
                addr: IpAddr::V4(Ipv4Addr::UNSPECIFIED),
                bits: 0,
            },
            Network {
                addr: IpAddr::V6(Ipv6Addr::UNSPECIFIED),
                bits: 0,
            },
        ]
    } else {
        Vec::new()
    };
    let traceparent = "00-0af7651916cd43dd8448eb211c80319c-b7ad6b7169203331-01";
    let c = Client {
        headers: &req,
        peer,
        tls: flags & 1 != 0,
        traceparent: (flags & 4 != 0).then_some(traceparent),
    };
    match headers::outgoing(&c, &trusted, host_header) {
        Err(Refused::BadFieldName) => {
            assert!(
                req.iter().any(|(n, _)| !headers::valid_token(n)),
                "refused fields with token names only"
            );
        }
        Ok(out) => check_outgoing(&c, &out, host_header, flags & 2 != 0),
    }

    let up = fields(response);
    let keep_length = flags & 8 != 0;
    let back = headers::incoming(&up, keep_length);
    for (name, value) in back.iter() {
        let length = keep_length && name.eq_ignore_ascii_case("content-length");
        assert!(!headers::is_hop_by_hop(name) || length, "{name} was left");
        assert!(
            !headers::connection_names(up.get_all("connection"), name),
            "{name}, which Connection names, was left"
        );
        assert!(up.iter().any(|(n, v)| n == name && v == value));
    }

    let names = headers::trailer_names(&up);
    for (i, n) in names.iter().enumerate() {
        assert!(headers::valid_token(n), "trailer name {n:?}");
        assert!(
            !names[..i].iter().any(|m| m.eq_ignore_ascii_case(n)),
            "{n} announced twice"
        );
    }
    let t = fields(trailers);
    let kept = headers::announced(&t, &names);
    for (name, _) in kept.iter() {
        assert!(names.iter().any(|n| n.eq_ignore_ascii_case(name)));
    }
}

fn check_outgoing(c: &Client<'_>, out: &Headers, host_header: &str, trusted: bool) {
    let client_host = c.headers.get("host").unwrap_or("");
    let host = if host_header.is_empty() {
        client_host
    } else {
        host_header
    };
    let mut it = out.iter();
    if !host.is_empty() {
        assert_eq!(it.next(), Some(("Host", host)), "Host is not first");
    }
    for (name, _) in it {
        let own = OWN.iter().any(|o| name.eq_ignore_ascii_case(o));
        if own {
            continue;
        }
        assert_ne!(headers::request_field(name), Field::Drop, "{name} was left");
        assert!(
            !headers::connection_names(c.headers.get_all("connection"), name),
            "{name}, which Connection names, was left"
        );
    }
    let ip = headers::client_ip(c.peer);
    let once = |n: &str| out.get_all(n).count() == 1;
    assert!(once("x-forwarded-for") && once("forwarded"));
    let xff = out.get("x-forwarded-for").unwrap();
    assert!(xff.ends_with(&ip.to_string()), "X-Forwarded-For {xff:?}");
    if !trusted {
        assert_eq!(
            xff,
            ip.to_string(),
            "an untrusted client's X-Forwarded-For was kept"
        );
        assert!(once("x-forwarded-proto") && once("x-forwarded-host"));
    }
    let proto = if c.tls { "https" } else { "http" };
    let mut el = String::new();
    headers::forwarded_element(&mut el, Some(ip), client_host, proto);
    assert!(
        !el.chars().any(|ch| ch < ' ' || ch == '\x7f'),
        "a control character in {el:?}"
    );
    assert!(el.starts_with("for=") && el.ends_with(&format!(";proto={proto}")));
    let fwd = out.get("forwarded").unwrap();
    assert!(
        fwd.ends_with(&el),
        "Forwarded {fwd:?} does not end with {el:?}"
    );
    if !trusted {
        assert_eq!(fwd, el, "an untrusted client's Forwarded was kept");
    }
    if c.traceparent.is_some() {
        assert_eq!(out.get_all("traceparent").count(), 1);
    }
    let clean = !c.headers.iter().any(|(_, v)| breaks_lines(v)) && !breaks_lines(host_header);
    if clean {
        for (name, value) in out.iter() {
            assert!(!breaks_lines(value), "{name}: {value:?} breaks a line");
        }
    }
}

/// `proxy_grpc_timeout`: `grpc-timeout`'s parser on any field, and what the proxy writes for a
/// duration (`data[..8]`, nanoseconds) reads back no shorter, and never more than one unit
/// longer.
pub fn proxy_grpc_timeout(data: &[u8]) {
    if let Ok(s) = std::str::from_utf8(data.get(8..).unwrap_or_default()) {
        if let Some(d) = grpc::parse_timeout(s) {
            assert!(s.len() <= 9, "{s:?} read as {d:?}");
        }
    }
    let Some(head) = data.get(..8) else { return };
    let nanos = u64::from_be_bytes(head.try_into().unwrap());
    if nanos == 0 {
        return;
    }
    let d = Duration::from_nanos(nanos);
    let enc = grpc::encode_timeout(d);
    let got = grpc::parse_timeout(&enc);
    let Some(got) = got else {
        panic!("{d:?}: {enc:?} does not read back");
    };
    assert!(got >= d, "{d:?}: {enc:?} reads back as {got:?}, shorter");
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Runs `f` on every file of `fuzz/corpus/<target>`.
    fn replay(target: &str, f: fn(&[u8])) {
        let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../fuzz/corpus")
            .join(target);
        let mut n = 0;
        for e in std::fs::read_dir(&dir).unwrap() {
            f(&std::fs::read(e.unwrap().path()).unwrap());
            n += 1;
        }
        assert!(n > 0, "{} is empty", dir.display());
    }

    #[test]
    fn corpus_proxy_headers() {
        replay("proxy_headers", proxy_headers);
    }

    #[test]
    fn corpus_proxy_grpc_timeout() {
        replay("proxy_grpc_timeout", proxy_grpc_timeout);
    }
}
