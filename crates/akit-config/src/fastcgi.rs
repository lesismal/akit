//! The `fastcgi` handler's params (I75), with fit's checks and messages (its `Provision`), and
//! the path rules the handler and its config share: how a path splits into a script and its path
//! info ([`FastCgi::split`]), and what a script's path may hold ([`scan`]).

use std::time::Duration;

use crate::decode::{Decoder, Obj};
use crate::json::Node;
use crate::model::{Dotfiles, FASTCGI_FIELDS};

/// fit's default `max_conns` of a `fastcgi` handler.
pub const DEFAULT_FASTCGI_MAX_CONNS: usize = 64;
/// fit's default `max_idle_conns` of a `fastcgi` handler.
pub const DEFAULT_FASTCGI_MAX_IDLE_CONNS: usize = 16;
/// fit's default `max_pending` of a `fastcgi` handler.
pub const DEFAULT_FASTCGI_MAX_PENDING: usize = 1024;
/// fit's default `max_header_bytes` of a `fastcgi` handler: 64 KiB.
pub const DEFAULT_FASTCGI_MAX_HEADER_BYTES: usize = 64 << 10;
/// fit's default `connect_timeout` of a `fastcgi` handler.
pub const DEFAULT_FASTCGI_CONNECT_TIMEOUT: Duration = Duration::from_secs(5);
/// fit's default `response_header_timeout` and `response_idle_timeout` of a `fastcgi` handler.
pub const DEFAULT_FASTCGI_RESPONSE_TIMEOUT: Duration = Duration::from_secs(60);
/// fit's default `timeout` of a `fastcgi` handler: the whole exchange.
pub const DEFAULT_FASTCGI_TIMEOUT: Duration = Duration::from_secs(600);
/// fit's default `idle_timeout` of a `fastcgi` handler: how long an idle connection is kept.
pub const DEFAULT_FASTCGI_IDLE_TIMEOUT: Duration = Duration::from_secs(60);

/// Where a FastCGI application listens.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FastCgiAddress {
    /// A host (a name or an IP address) and a port, as `"127.0.0.1:9000"` or `"php:9000"`.
    Tcp {
        /// The host, without the brackets of an IPv6 address.
        host: String,
        /// The port.
        port: u16,
    },
    /// A Unix-domain socket's path, written `"unix/"` and the path.
    Unix(String),
}

/// The `fastcgi` handler's params, as fit's `handler/fastcgi`: requests answered by a FastCGI
/// application, as PHP-FPM is.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FastCgi {
    /// `address`, required: where the application listens.
    pub address: FastCgiAddress,
    /// `address` as written, for messages and the application's log lines.
    pub address_text: String,
    /// `root`, required: the document root as the application sees it, an absolute path.
    pub root: String,
    /// `split_path`: what ends a script's name in a path, in lower case; `[".php"]` when left
    /// out.
    pub split_path: Vec<String>,
    /// `index`: the script a path ending in `/` runs; `index.php` when left out.
    pub index: String,
    /// `try_files`: paths tried in turn for the script to run, `{path}` standing for the
    /// request's path; empty when left out (the path runs as it is).
    pub try_files: Vec<String>,
    /// `dotfiles`: whether scripts with a segment starting with a dot run.
    pub dotfiles: Dotfiles,
    /// `env`: more parameters for the application, set after akit's own, sorted by name.
    pub env: Vec<(String, String)>,
    /// `max_conns`: the most connections to the application.
    pub max_conns: usize,
    /// `max_idle_conns`: the most idle connections kept.
    pub max_idle_conns: usize,
    /// `max_pending`: the most requests waiting for a connection.
    pub max_pending: usize,
    /// `connect_timeout`.
    pub connect_timeout: Duration,
    /// `response_header_timeout`: how long the application may take to begin its response.
    pub response_header_timeout: Duration,
    /// `response_idle_timeout`: how long the application may go silent mid-response.
    pub response_idle_timeout: Duration,
    /// `timeout`: the most a whole exchange may take.
    pub timeout: Duration,
    /// `idle_timeout`: how long an idle connection is kept.
    pub idle_timeout: Duration,
    /// `max_header_bytes`: the most bytes the response's CGI header may take.
    pub max_header_bytes: usize,
}

impl FastCgi {
    /// Splits a path into the script it names and its path info: the script ends with the first
    /// of `split_path` (in any case) that ends a segment, as `/index.php/a/b` runs `/index.php`
    /// with path info `/a/b`; a path ending in `/` runs the index; any other runs as it is.
    #[must_use]
    pub fn split<'p>(&self, p: &'p str) -> (std::borrow::Cow<'p, str>, &'p str) {
        let lower = p.to_ascii_lowercase();
        for ext in &self.split_path {
            let mut from = 0;
            while let Some(i) = lower.get(from..).and_then(|rest| rest.find(ext.as_str())) {
                let end = from.saturating_add(i).saturating_add(ext.len());
                if end == p.len() || p.as_bytes().get(end) == Some(&b'/') {
                    if let (Some(script), Some(info)) = (p.get(..end), p.get(end..)) {
                        return (std::borrow::Cow::Borrowed(script), info);
                    }
                }
                from = end;
            }
        }
        if p.ends_with('/') {
            return (std::borrow::Cow::Owned(format!("{p}{}", self.index)), "");
        }
        (std::borrow::Cow::Borrowed(p), "")
    }

    /// The status a script is refused with, or 0 when it may run: 400 for one with a `..`
    /// segment, a NUL or a backslash (what could name a file outside the root); 404 for a
    /// dotfile under `dotfiles` `deny`.
    #[must_use]
    pub fn check(&self, script: &str) -> u16 {
        match scan(script) {
            Scan::Unsafe => 400,
            Scan::Hidden if self.dotfiles == Dotfiles::Deny => 404,
            Scan::Hidden | Scan::Plain => 0,
        }
    }
}

/// What [`scan`] finds in a path.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Scan {
    /// Nothing to refuse.
    Plain,
    /// A segment starts with a dot (but `.` and a first `.well-known`).
    Hidden,
    /// A `..` segment, a NUL or a backslash, which Windows takes as a separator.
    Unsafe,
}

/// Looks at the segments of `p`, a slash-separated path, in one pass, as fit's `dotfile.Scan`:
/// unsafe wins over hidden.
#[must_use]
pub fn scan(p: &str) -> Scan {
    let mut hidden = false;
    for (i, segment) in p
        .split('/')
        .filter(|s| !s.is_empty() && *s != ".")
        .enumerate()
    {
        if segment.contains(['\0', '\\']) || segment == ".." {
            return Scan::Unsafe;
        }
        if segment.starts_with('.') && !(i == 0 && segment == ".well-known") {
            hidden = true;
        }
    }
    if hidden { Scan::Hidden } else { Scan::Plain }
}

/// `address` read: a host and port, or `unix/` and a path. `None` when it is neither.
fn address(s: &str) -> Option<FastCgiAddress> {
    if let Some(path) = s.strip_prefix("unix/") {
        return (!path.is_empty()).then(|| FastCgiAddress::Unix(path.to_owned()));
    }
    let (host, port) = s.rsplit_once(':')?;
    let host = match host.strip_prefix('[') {
        Some(h) => h.strip_suffix(']')?,
        // An IPv6 address needs its brackets.
        None if host.contains(':') => return None,
        None => host,
    };
    if host.is_empty() || host.contains(['[', ']', '/']) {
        return None;
    }
    let port = port.parse().ok()?;
    Some(FastCgiAddress::Tcp {
        host: host.to_owned(),
        port,
    })
}

/// A count: fit's `min:"0"`, the default for 0 or left out.
fn count(d: &mut Decoder<'_>, o: &mut Obj<'_>, key: &str, default: usize) -> usize {
    o.take(key)
        .and_then(|v| d.int(v, &o.at(key), 0, i64::from(i32::MAX)))
        .and_then(|n| usize::try_from(n).ok())
        .filter(|n| *n != 0)
        .unwrap_or(default)
}

/// A duration: the default for 0 or left out, as fit's `dur`.
fn duration(d: &mut Decoder<'_>, o: &mut Obj<'_>, key: &str, default: Duration) -> Duration {
    o.take(key)
        .and_then(|v| d.duration(v, &o.at(key)))
        .filter(|t| !t.is_zero())
        .unwrap_or(default)
}

/// Reads the `fastcgi` handler's params, with fit's checks. `module_off` is where the module is
/// named, for a handler with no params.
pub(crate) fn fastcgi(
    d: &mut Decoder<'_>,
    params: Option<&Node>,
    module_off: usize,
    path: &str,
) -> Option<FastCgi> {
    let Some(n) = params else {
        d.error(
            module_off,
            &crate::error::join_field(path, "address"),
            "missing required field",
        );
        return None;
    };
    let mut o: Obj<'_> = d.object(n, path)?;
    let addr = o.require(d, "address").and_then(|v| {
        let p = o.at("address");
        let s = d.string(v, &p)?;
        match address(&s) {
            Some(a) => Some((a, s)),
            None => {
                d.error(
                    v.off,
                    &p,
                    format!("{s:?} is neither a host and port nor unix/ and a path"),
                );
                None
            }
        }
    });
    let root = o.require(d, "root").and_then(|v| {
        let p = o.at("root");
        let s = d.string(v, &p)?;
        if !s.starts_with('/') {
            d.error(v.off, &p, format!("{s:?} is not an absolute path"));
            return None;
        }
        Some(s)
    });
    let mut split_path: Vec<String> = match o.take("split_path") {
        None => Vec::new(),
        Some(v) => d
            .strings(v, &o.at("split_path"))
            .into_iter()
            .map(|(_, s)| s.to_ascii_lowercase())
            .collect(),
    };
    if split_path.is_empty() {
        split_path.push(".php".to_owned());
    }
    let dotfiles = match o.take("dotfiles") {
        None => Dotfiles::Deny,
        Some(v) => {
            let p = o.at("dotfiles");
            match d.string(v, &p).as_deref() {
                Some("deny") | None => Dotfiles::Deny,
                Some("allow") => Dotfiles::Allow,
                Some(other) => {
                    d.error(
                        v.off,
                        &p,
                        format!("{other:?} is not one of \"deny\", \"allow\""),
                    );
                    Dotfiles::Deny
                }
            }
        }
    };
    let mut index = String::from("index.php");
    if let Some(v) = o.take("index") {
        let p = o.at("index");
        if let Some(s) = d.string(v, &p) {
            if dotfiles == Dotfiles::Deny && s.starts_with('.') {
                d.error(
                    v.off,
                    &p,
                    format!(
                        "{s:?} starts with a dot, which dotfiles deny hides; set dotfiles to allow to run it"
                    ),
                );
            } else if !s.is_empty() {
                index = s;
            }
        }
    }
    let try_files_raw: Vec<(usize, String)> = match o.take("try_files") {
        None => Vec::new(),
        Some(v) => d.strings(v, &o.at("try_files")),
    };
    let mut env: Vec<(String, String)> = match o.take("env") {
        None => Vec::new(),
        Some(v) => d
            .string_map(v, &o.at("env"))
            .into_iter()
            .map(|(_, k, v)| (k, v))
            .collect(),
    };
    env.sort();
    let max_conns = count(d, &mut o, "max_conns", DEFAULT_FASTCGI_MAX_CONNS);
    let max_idle_conns = count(d, &mut o, "max_idle_conns", DEFAULT_FASTCGI_MAX_IDLE_CONNS);
    let max_pending = count(d, &mut o, "max_pending", DEFAULT_FASTCGI_MAX_PENDING);
    let max_header_bytes = count(
        d,
        &mut o,
        "max_header_bytes",
        DEFAULT_FASTCGI_MAX_HEADER_BYTES,
    );
    let connect_timeout = duration(
        d,
        &mut o,
        "connect_timeout",
        DEFAULT_FASTCGI_CONNECT_TIMEOUT,
    );
    let response_header_timeout = duration(
        d,
        &mut o,
        "response_header_timeout",
        DEFAULT_FASTCGI_RESPONSE_TIMEOUT,
    );
    let response_idle_timeout = duration(
        d,
        &mut o,
        "response_idle_timeout",
        DEFAULT_FASTCGI_RESPONSE_TIMEOUT,
    );
    let timeout = duration(d, &mut o, "timeout", DEFAULT_FASTCGI_TIMEOUT);
    let idle_timeout = duration(d, &mut o, "idle_timeout", DEFAULT_FASTCGI_IDLE_TIMEOUT);
    let try_path = o.at("try_files");
    o.finish(d, FASTCGI_FIELDS, &[]);
    let (address, address_text) = addr?;
    let mut f = FastCgi {
        address,
        address_text,
        root: root?,
        split_path,
        index,
        try_files: Vec::new(),
        dotfiles,
        env,
        max_conns,
        max_idle_conns,
        max_pending,
        connect_timeout,
        response_header_timeout,
        response_idle_timeout,
        timeout,
        idle_timeout,
        max_header_bytes,
    };
    let mut bad = false;
    for (i, (off, t)) in try_files_raw.iter().enumerate() {
        let at = crate::error::join_index(&try_path, i);
        if !t.starts_with('/') && !t.starts_with("{path}") {
            d.error(
                *off,
                &at,
                format!("{t:?} starts with neither / nor {{path}}"),
            );
            bad = true;
            continue;
        }
        let candidate = t.replace("{path}", "/x");
        let (script, _) = f.split(&candidate);
        match f.check(&script) {
            400 => {
                d.error(
                    *off,
                    &at,
                    format!(
                        "{t:?} names a script with a \"..\" segment, a NUL or a backslash, which are refused"
                    ),
                );
                bad = true;
            }
            404 => {
                d.error(
                    *off,
                    &at,
                    format!(
                        "{t:?} names a script with a segment starting with a dot, which dotfiles deny hides; set dotfiles to allow to run it"
                    ),
                );
                bad = true;
            }
            _ => {}
        }
    }
    if bad {
        return None;
    }
    f.try_files = try_files_raw.into_iter().map(|(_, t)| t).collect();
    Some(f)
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]

    use super::*;

    fn handler() -> FastCgi {
        FastCgi {
            address: FastCgiAddress::Unix("/x".into()),
            address_text: "unix//x".into(),
            root: "/srv".into(),
            split_path: vec![".php".into()],
            index: "index.php".into(),
            try_files: Vec::new(),
            dotfiles: Dotfiles::Deny,
            env: Vec::new(),
            max_conns: 1,
            max_idle_conns: 1,
            max_pending: 1,
            connect_timeout: DEFAULT_FASTCGI_CONNECT_TIMEOUT,
            response_header_timeout: DEFAULT_FASTCGI_RESPONSE_TIMEOUT,
            response_idle_timeout: DEFAULT_FASTCGI_RESPONSE_TIMEOUT,
            timeout: DEFAULT_FASTCGI_TIMEOUT,
            idle_timeout: DEFAULT_FASTCGI_IDLE_TIMEOUT,
            max_header_bytes: DEFAULT_FASTCGI_MAX_HEADER_BYTES,
        }
    }

    /// fit's `TestSplit`.
    #[test]
    fn split() {
        let h = handler();
        for (input, script, info) in [
            ("/index.php", "/index.php", ""),
            ("/index.php/a/b", "/index.php", "/a/b"),
            ("/x.phpx/y.php/z", "/x.phpx/y.php", "/z"),
            ("/A.PHP/b", "/A.PHP", "/b"),
            ("/dir/", "/dir/index.php", ""),
            ("/plain", "/plain", ""),
        ] {
            let (s, i) = h.split(input);
            assert_eq!((s.as_ref(), i), (script, info), "{input}");
        }
    }

    #[test]
    fn scans() {
        assert_eq!(scan("/a/b.php"), Scan::Plain);
        assert_eq!(scan("/.well-known/x.php"), Scan::Plain);
        assert_eq!(scan("/a/.well-known/x.php"), Scan::Hidden);
        assert_eq!(scan("/.git/x.php"), Scan::Hidden);
        assert_eq!(scan("/.git/../x.php"), Scan::Unsafe);
        assert_eq!(scan("/a\\b.php"), Scan::Unsafe);
        assert_eq!(scan("/a\0b.php"), Scan::Unsafe);
        assert_eq!(scan("/./a.php"), Scan::Plain);
    }

    #[test]
    fn addresses() {
        assert_eq!(
            address("127.0.0.1:9000"),
            Some(FastCgiAddress::Tcp {
                host: "127.0.0.1".into(),
                port: 9000
            })
        );
        assert_eq!(
            address("[::1]:9000"),
            Some(FastCgiAddress::Tcp {
                host: "::1".into(),
                port: 9000
            })
        );
        assert_eq!(
            address("unix//run/php.sock"),
            Some(FastCgiAddress::Unix("/run/php.sock".into()))
        );
        for bad in ["nowhere", "::1:9000", ":9000", "a:b", "unix/", "a:70000"] {
            assert_eq!(address(bad), None, "{bad}");
        }
    }
}
