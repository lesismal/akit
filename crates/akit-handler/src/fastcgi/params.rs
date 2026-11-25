//! A request's CGI/1.1 parameters, as PHP and other FastCGI applications read them (fit's
//! `params.go`): what the request is, the script it runs and its path info under the root, who
//! sent it and to where, and each of its fields as `HTTP_` and its name, but `Proxy` (httpoxy),
//! the ones CGI has names of its own for, and those with `_` in their names; then the config's
//! `env`.
//!
//! A field with `_` is dropped, as nginx drops it by default (`underscores_in_headers off`): CGI
//! names fold `-` into `_`, so a client's `X_User` would reach the application as `HTTP_X_USER`,
//! the very parameter of the `X-User` an auth middleware set.
//!
//! They are encoded in two parts when the request arrives, the request's own and its fields with
//! `env`; the script's four (`SCRIPT_NAME`, `SCRIPT_FILENAME`, `PATH_INFO`, `PATH_TRANSLATED`) go
//! between them once the script is known ([`Params::encode`]), after `try_files` when there are
//! any.

use std::net::SocketAddr;

use akio_http::{Headers, Method, Version};

use super::record::append_param;

/// What a request's parameters are made of, but its script.
#[derive(Debug)]
pub(crate) struct Params {
    /// The request's own parameters, encoded.
    head: Vec<u8>,
    /// Its fields as `HTTP_` parameters, then `env`, encoded.
    tail: Vec<u8>,
}

/// The request, as the parameters read it.
pub(crate) struct Request<'a> {
    pub(crate) method: &'a Method,
    /// The target as sent, in origin form.
    pub(crate) target: &'a str,
    /// The path, unescaped.
    pub(crate) path: &'a str,
    pub(crate) version: Version,
    pub(crate) headers: &'a Headers,
    pub(crate) peer: SocketAddr,
    /// The address the request came to: unspecified, port 0, where there is none (a
    /// Unix-domain socket).
    pub(crate) local: SocketAddr,
    pub(crate) tls: bool,
    /// The body's length when known: declared, or the whole body's.
    pub(crate) length: Option<u64>,
}

fn add(b: &mut Vec<u8>, name: &str, value: &str) {
    append_param(b, name.as_bytes(), value.as_bytes());
}

/// `host` without its port, and the port, if it has one (an IPv6 address in brackets).
fn split_host(host: &str) -> (&str, Option<&str>) {
    if let Some(rest) = host.strip_prefix('[') {
        if let Some((h, after)) = rest.split_once(']') {
            return (h, after.strip_prefix(':'));
        }
        return (host, None);
    }
    match host.rsplit_once(':') {
        Some((h, p)) if !h.contains(':') => (h, Some(p)),
        _ => (host, None),
    }
}

impl Params {
    /// The request's parameters but its script's, with `root` and `env` from the config.
    pub(crate) fn new(r: &Request<'_>, root: &str, env: &[(String, String)]) -> Params {
        let mut head = Vec::with_capacity(512);
        let (scheme, default_port) = if r.tls {
            ("https", "443")
        } else {
            ("http", "80")
        };
        let host = r.headers.get("host").unwrap_or("");
        let (server_name, host_port) = split_host(host);
        let query = r.target.split_once('?').map_or("", |(_, q)| q);
        let protocol = match r.version {
            Version::Http10 => "HTTP/1.0",
            Version::Http2 => "HTTP/2.0",
            Version::Http3 => "HTTP/3.0",
            _ => "HTTP/1.1",
        };
        let method = r.method.as_str();
        add(&mut head, "GATEWAY_INTERFACE", "CGI/1.1");
        add(&mut head, "SERVER_SOFTWARE", "akit");
        add(&mut head, "SERVER_PROTOCOL", protocol);
        add(&mut head, "SERVER_NAME", server_name);
        // The address the request came to, as fit's from its TCP connection's; where there is
        // none (a Unix-domain socket), empty, and the port the Host field names, as fit's.
        if r.local.port() == 0 {
            add(&mut head, "SERVER_ADDR", "");
            add(&mut head, "SERVER_PORT", host_port.unwrap_or(default_port));
        } else {
            add(
                &mut head,
                "SERVER_ADDR",
                &r.local.ip().to_canonical().to_string(),
            );
            add(&mut head, "SERVER_PORT", &r.local.port().to_string());
        }
        add(&mut head, "REQUEST_SCHEME", scheme);
        if r.tls {
            add(&mut head, "HTTPS", "on");
        }
        add(&mut head, "REMOTE_ADDR", &r.peer.ip().to_string());
        add(&mut head, "REMOTE_PORT", &r.peer.port().to_string());
        add(&mut head, "REQUEST_METHOD", method);
        add(&mut head, "REQUEST_URI", r.target);
        add(&mut head, "QUERY_STRING", query);
        add(&mut head, "DOCUMENT_ROOT", root);
        add(&mut head, "DOCUMENT_URI", r.path);
        add(
            &mut head,
            "CONTENT_TYPE",
            r.headers.get("content-type").unwrap_or(""),
        );
        let length = match r.length {
            Some(n) if n > 0 || !matches!(r.method, Method::Get | Method::Head) => n.to_string(),
            _ => String::new(),
        };
        add(&mut head, "CONTENT_LENGTH", &length);
        // PHP built with force-cgi-redirect runs only for a request a server passed on.
        add(&mut head, "REDIRECT_STATUS", "200");

        let mut tail = Vec::with_capacity(512);
        // A field's values together, as Go's server gives them: in the order of the fields'
        // first appearance.
        let mut seen: Vec<(String, String)> = Vec::new();
        for (name, value) in r.headers.iter() {
            if name.starts_with(':')
                || name.contains('_')
                || name.eq_ignore_ascii_case("proxy")
                || name.eq_ignore_ascii_case("content-type")
                || name.eq_ignore_ascii_case("content-length")
                || name.eq_ignore_ascii_case("host")
            {
                continue;
            }
            let mut key = String::with_capacity(name.len().saturating_add(5));
            key.push_str("HTTP_");
            key.extend(name.chars().map(|c| {
                if c == '-' {
                    '_'
                } else {
                    c.to_ascii_uppercase()
                }
            }));
            match seen.iter_mut().find(|(k, _)| *k == key) {
                Some((_, v)) => {
                    v.push_str(", ");
                    v.push_str(value);
                }
                None => seen.push((key, value.to_owned())),
            }
        }
        for (k, v) in &seen {
            add(&mut tail, k, v);
        }
        if !host.is_empty() {
            add(&mut tail, "HTTP_HOST", host);
        }
        for (k, v) in env {
            add(&mut tail, k, v);
        }
        Params { head, tail }
    }

    /// The parameters with the script's, `script` and its `path_info` under `root`.
    pub(crate) fn encode(&self, root: &str, script: &str, path_info: &str) -> Vec<u8> {
        let mut b = Vec::with_capacity(
            self.head
                .len()
                .saturating_add(self.tail.len())
                .saturating_add(256),
        );
        b.extend_from_slice(&self.head);
        let script = clean(script);
        add(&mut b, "SCRIPT_NAME", &script);
        add(&mut b, "SCRIPT_FILENAME", &join(root, &script));
        add(&mut b, "PATH_INFO", path_info);
        if !path_info.is_empty() {
            add(&mut b, "PATH_TRANSLATED", &join(root, path_info));
        }
        b.extend_from_slice(&self.tail);
        b
    }
}

/// `p` as Go's `path.Clean` gives it: rooted, no empty, `.` or `..` segments.
pub(crate) fn clean(p: &str) -> String {
    let mut out: Vec<&str> = Vec::new();
    for s in p.split('/') {
        match s {
            "" | "." => {}
            ".." => {
                out.pop();
            }
            s => out.push(s),
        }
    }
    let mut s = String::with_capacity(p.len().saturating_add(1));
    for seg in &out {
        s.push('/');
        s.push_str(seg);
    }
    if s.is_empty() {
        s.push('/');
    }
    s
}

/// `root` and `p` joined and cleaned, as Go's `filepath.Join`.
pub(crate) fn join(root: &str, p: &str) -> String {
    let joined = clean(&format!("{root}/{p}"));
    if root.starts_with('/') {
        joined
    } else {
        joined.trim_start_matches('/').to_owned()
    }
}

#[cfg(test)]
mod tests {
    #![allow(
        clippy::unwrap_used,
        clippy::indexing_slicing,
        clippy::arithmetic_side_effects
    )]

    use super::*;

    /// Decodes parameters as an application reads them.
    pub(crate) fn decode(mut b: &[u8]) -> Vec<(String, String)> {
        let mut out = Vec::new();
        let len = |b: &mut &[u8]| -> usize {
            if b[0] & 0x80 == 0 {
                let n = usize::from(b[0]);
                *b = &b[1..];
                n
            } else {
                let n = u32::from_be_bytes([b[0] & 0x7f, b[1], b[2], b[3]]) as usize;
                *b = &b[4..];
                n
            }
        };
        while !b.is_empty() {
            let n = len(&mut b);
            let v = len(&mut b);
            out.push((
                String::from_utf8(b[..n].to_vec()).unwrap(),
                String::from_utf8(b[n..n + v].to_vec()).unwrap(),
            ));
            b = &b[n + v..];
        }
        out
    }

    fn get<'a>(m: &'a [(String, String)], k: &str) -> Option<&'a str> {
        m.iter()
            .rev()
            .find(|(n, _)| n == k)
            .map(|(_, v)| v.as_str())
    }

    /// fit's `TestParamsTLS`: a request over TLS, of a declared length.
    #[test]
    fn over_tls() {
        let mut h = Headers::new();
        h.append("Host", "example.com");
        h.append("Content-Length", "3");
        let r = Request {
            method: &Method::Post,
            target: "/x.php/p?q=1",
            path: "/x.php/p",
            version: Version::Http11,
            headers: &h,
            peer: "10.0.0.1:5555".parse().unwrap(),
            local: "0.0.0.0:0".parse().unwrap(),
            tls: true,
            length: Some(3),
        };
        let p = Params::new(&r, "/srv", &[("A".into(), "b".into())]);
        let m = decode(&p.encode("/srv", "/x.php", "/p"));
        for (k, want) in [
            ("HTTPS", "on"),
            ("REQUEST_SCHEME", "https"),
            ("SERVER_PORT", "443"),
            ("REQUEST_URI", "/x.php/p?q=1"),
            ("CONTENT_LENGTH", "3"),
            ("A", "b"),
            ("PATH_TRANSLATED", "/srv/p"),
            ("SCRIPT_FILENAME", "/srv/x.php"),
            ("QUERY_STRING", "q=1"),
            ("REMOTE_ADDR", "10.0.0.1"),
            ("REMOTE_PORT", "5555"),
            ("HTTP_HOST", "example.com"),
            ("SERVER_NAME", "example.com"),
        ] {
            assert_eq!(get(&m, k), Some(want), "{k}");
        }
        assert_eq!(get(&m, "HTTP_CONTENT_LENGTH"), None);
    }

    /// Fields: `_` in a name and `Proxy` dropped, repeated ones joined.
    #[test]
    fn fields() {
        let mut h = Headers::new();
        h.append("Host", "a:8080");
        h.append("X-Test", "1");
        h.append("x-test", "2");
        h.append("X_Test", "forged");
        h.append("Proxy", "http://evil");
        let r = Request {
            method: &Method::Get,
            target: "/",
            path: "/",
            version: Version::Http2,
            headers: &h,
            peer: "[::1]:1".parse().unwrap(),
            local: "0.0.0.0:0".parse().unwrap(),
            tls: false,
            length: Some(0),
        };
        let m = decode(&Params::new(&r, "/srv", &[]).encode("/srv", "/index.php", ""));
        assert_eq!(get(&m, "HTTP_X_TEST"), Some("1, 2"));
        assert_eq!(get(&m, "HTTP_PROXY"), None);
        assert_eq!(get(&m, "SERVER_PORT"), Some("8080"));
        assert_eq!(get(&m, "SERVER_PROTOCOL"), Some("HTTP/2.0"));
        assert_eq!(get(&m, "CONTENT_LENGTH"), Some(""));
        assert_eq!(get(&m, "PATH_TRANSLATED"), None);
        assert_eq!(get(&m, "REMOTE_ADDR"), Some("::1"));
    }

    /// The address the request came to, as fit's from its TCP connection's: `SERVER_PORT` is
    /// its port, not the `Host` field's; an IPv4 address mapped into IPv6 written as IPv4. With
    /// none (unspecified, port 0, as over a Unix-domain socket), empty and the `Host` field's
    /// port, as fit's.
    #[test]
    fn server_address() {
        let mut h = Headers::new();
        h.append("Host", "example.com:9999");
        for (local, addr, port) in [
            ("192.0.2.7:8443", "192.0.2.7", "8443"),
            ("[::ffff:192.0.2.7]:80", "192.0.2.7", "80"),
            ("[2001:db8::1]:443", "2001:db8::1", "443"),
            ("0.0.0.0:0", "", "9999"),
        ] {
            let r = Request {
                method: &Method::Get,
                target: "/",
                path: "/",
                version: Version::Http11,
                headers: &h,
                peer: "10.0.0.1:5555".parse().unwrap(),
                local: local.parse().unwrap(),
                tls: false,
                length: None,
            };
            let m = decode(&Params::new(&r, "/srv", &[]).encode("/srv", "/index.php", ""));
            assert_eq!(get(&m, "SERVER_ADDR"), Some(addr), "{local}");
            assert_eq!(get(&m, "SERVER_PORT"), Some(port), "{local}");
            assert_eq!(get(&m, "SERVER_NAME"), Some("example.com"), "{local}");
        }
    }

    #[test]
    fn cleans() {
        assert_eq!(clean("/a//b/./c/../d"), "/a/b/d");
        assert_eq!(clean(""), "/");
        assert_eq!(join("/srv/www", "/extra/path"), "/srv/www/extra/path");
        assert_eq!(join("/srv", "/../etc"), "/etc");
    }
}
