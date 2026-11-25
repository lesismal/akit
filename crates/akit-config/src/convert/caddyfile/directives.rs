//! A Caddyfile's directives converted: those that change the routes they cover (modifiers) and
//! those that answer (handlers), with the upstreams reverse_proxy and forward_auth reach.

use std::collections::BTreeMap;

use super::parse::Directive;
use super::routes::{Handler, Modifier, Mw, Set, order};
use super::{Converter, OutHealth, OutRef, OutUpstream, Site, matcher, params};
use crate::convert::gostd::{
    self, J, Params, atoi, atoi_ok, format_float, join_host_port, parse_float, quote, quote_meta,
    split_host_port,
};
use crate::convert::{goregexp, nginx::go_lower};

/// Caddy's placeholders, `{name}` and `{name[x]}`, found in `s`, in order.
fn placeholders(s: &str) -> Vec<&str> {
    let b = s.as_bytes();
    let mut out = Vec::new();
    let mut i = 0usize;
    while i < b.len() {
        if let Some(end) = placeholder_at(b, i) {
            out.push(s.get(i..end).unwrap_or_default());
            i = end;
        } else {
            i += 1;
        }
    }
    out
}

/// `\{[A-Za-z_][A-Za-z0-9_.\-]*(\[[^\]]*\])?\}` at `i`: its end.
fn placeholder_at(b: &[u8], i: usize) -> Option<usize> {
    if b.get(i) != Some(&b'{')
        || !b
            .get(i + 1)
            .is_some_and(|c| c.is_ascii_alphabetic() || *c == b'_')
    {
        return None;
    }
    let mut j = i + 2;
    while b
        .get(j)
        .is_some_and(|c| c.is_ascii_alphanumeric() || matches!(c, b'_' | b'.' | b'-'))
    {
        j += 1;
    }
    match b.get(j) {
        Some(b'}') => Some(j + 1),
        Some(b'[') => {
            let close = b.get(j + 1..)?.iter().position(|&c| c == b']')?;
            let k = j + 1 + close + 1;
            (b.get(k) == Some(&b'}')).then_some(k + 1)
        }
        _ => None,
    }
}

fn has_placeholder(s: &str) -> bool {
    !placeholders(s).is_empty()
}

/// A URL of Caddy's as one of fit's redirect, or `None` when it has placeholders fit has not.
fn redirect_target(to: &str) -> Option<String> {
    let to = gostd::replace(
        to,
        &[
            ("{http.request.uri}", "{uri}"),
            ("{http.request.uri.path}", "{path}"),
            ("{http.request.uri.query}", "{query}"),
            ("{http.request.host}", "{host}"),
            ("{http.request.hostport}", "{host}"),
            ("{hostport}", "{host}"),
            ("{http.request.scheme}", "{scheme}"),
        ],
    );
    placeholders(&to)
        .iter()
        .all(|p| matches!(*p, "{uri}" | "{path}" | "{query}" | "{host}" | "{scheme}"))
        .then_some(to)
}

/// Warnings a callback gives, flushed in order by its caller.
type Warns = Vec<(i64, String)>;

fn skip_msg(e: &Directive, why: &str) -> (i64, String) {
    (e.line, format!("{}: skipped: {why}", e.name))
}

impl Converter {
    /// Converts a directive that changes the routes it covers.
    #[allow(clippy::too_many_lines)]
    pub(super) fn modifier(&mut self, s: &Site, d: &Directive) -> Option<Modifier> {
        let single = matches!(d.name.as_str(), "root" | "redir" | "rewrite");
        let (matched, args, spec) = self.matcher(s, d, single)?;
        let mut m = Modifier {
            name: d.name.clone(),
            line: d.line,
            matcher: matched,
            spec,
            order: order(&d.name),
            add: None,
            set: None,
        };
        let add = |mut m: Modifier, module: &str, p: Params| {
            m.add = Some(Mw {
                r: OutRef::new(module, p),
                line: d.line,
            });
            Some(m)
        };
        match d.name.as_str() {
            "root" => {
                let [root] = args.as_slice() else {
                    self.skip(d, "root takes a path");
                    return None;
                };
                if has_placeholder(root) {
                    self.skip(d, "a root with placeholders is not converted");
                    return None;
                }
                m.set = Some(Set::Root(root.clone()));
                Some(m)
            }
            "try_files" => {
                let files = self.try_files(d, &args)?;
                m.set = Some(Set::TryFiles(files, d.line));
                Some(m)
            }
            "redir" => {
                if args.is_empty() || args.len() > 2 {
                    self.skip(d, "redir takes a URL and a status");
                    return None;
                }
                let Some(to) =
                    redirect_target(args.first().map(String::as_str).unwrap_or_default())
                else {
                    self.skip(d, "its URL has placeholders fit has no counterpart for");
                    return None;
                };
                let mut status = 302;
                if let Some(st) = args.get(1) {
                    match st.as_str() {
                        "permanent" => status = 301,
                        "temporary" => {}
                        "html" => self.warn(d.line, "redir html: written as a 302 redirect"),
                        _ => match atoi_ok(st).filter(|n| (300..=399).contains(n)) {
                            Some(n) => status = n,
                            None => {
                                self.skip(d, "a status fit's redirect does not take");
                                return None;
                            }
                        },
                    }
                }
                add(
                    m,
                    "redirect",
                    params([("to", J::Str(to)), ("status", J::Int(status))]),
                )
            }
            "rewrite" => {
                let [to] = args.as_slice() else {
                    self.skip(d, "rewrite takes a URI");
                    return None;
                };
                let p = self.rewrite(d, to)?;
                self.warn(d.line, "rewrite: fit picks a request's route before rewriting it: the rewritten path does not choose another route");
                add(m, "rewrite", p)
            }
            "uri" => {
                let p = self.uri(d, &args)?;
                add(m, "rewrite", p)
            }
            "header" | "request_header" => {
                let ops = self.header_ops(d, &args, &d.block)?;
                let side = if d.name == "request_header" {
                    "request"
                } else {
                    "response"
                };
                add(m, "headers", params([(side, J::Map(ops))]))
            }
            "encode" => {
                let p = self.encode(d, &args)?;
                add(m, "compress", p)
            }
            "basicauth" | "basic_auth" => {
                let p = self.basic_auth(d, &args)?;
                add(m, "basic_auth", p)
            }
            "forward_auth" => {
                let p = self.forward_auth(d, &args)?;
                add(m, "forward_auth", p)
            }
            "request_body" => {
                for e in &d.block {
                    if e.name == "max_size"
                        && let [s] = e.args.as_slice()
                        && let Some(n) = size(s)
                    {
                        return add(m, "body_limit", params([("max_bytes", J::Int(n))]));
                    }
                }
                self.skip(d, "fit limits a request body's size, with max_size");
                None
            }
            _ => None,
        }
    }

    /// try_files's files as fit's, `{path}` kept, and the last =404 dropped, which is what fit
    /// answers anyway.
    pub(super) fn try_files(&mut self, d: &Directive, args: &[String]) -> Option<Vec<String>> {
        let mut out = Vec::new();
        for (i, a) in args.iter().enumerate() {
            if a.starts_with('=') {
                if i + 1 != args.len() {
                    self.skip(d, "=status is not converted but last");
                    return None;
                }
                if a != "=404" {
                    self.warn(
                        d.line,
                        format!("try_files: the last {a} answers 404 in fit"),
                    );
                }
                continue;
            }
            let mut a = a.replace("{http.request.uri.path}", "{path}");
            if a.contains('?') || a.replace("{path}", "").contains('{') {
                self.skip(
                    d,
                    "a file with a query or with placeholders other than {path} is not converted",
                );
                return None;
            }
            if !a.starts_with('/') && !a.starts_with("{path}") {
                a = format!("/{a}");
            }
            out.push(a);
        }
        for e in &d.block {
            self.skip(e, "fit tries files in order, the first that exists served");
        }
        if out.is_empty() {
            self.skip(d, "nothing to try");
            return None;
        }
        Some(out)
    }

    /// rewrite's URI as the rewrite middleware's params.
    fn rewrite(&mut self, d: &Directive, to: &str) -> Option<Params> {
        let to = to
            .replace("{http.request.uri.path}", "{path}")
            .replace("{http.request.uri}", "{uri}");
        if to.contains('?') {
            self.skip(d, "a rewrite of the query is not converted");
            return None;
        }
        if !to.starts_with('/') {
            self.skip(d, "fit rewrites to a path starting with /");
            return None;
        }
        // The query is kept, so {uri} is the path's.
        let to = to.replace("{uri}", "{path}");
        if let Some(prefix) = to.strip_suffix("{path}")
            && !prefix.contains('{')
        {
            return Some(params([("add_prefix", J::str(prefix))]));
        }
        if to.contains('{') {
            self.skip(
                d,
                "a URI with placeholders other than {path} is not converted",
            );
            return None;
        }
        Some(params([
            ("regexp", J::str("^.*$")),
            ("replacement", J::Str(to.replace('$', "$$"))),
        ]))
    }

    /// uri's operations as the rewrite middleware's params.
    fn uri(&mut self, d: &Directive, args: &[String]) -> Option<Params> {
        if args.len() < 2 {
            self.skip(d, "uri takes an operation and its arguments");
            return None;
        }
        let op = args.first().map(String::as_str).unwrap_or_default();
        let a1 = args.get(1).cloned().unwrap_or_default();
        match (op, args.len()) {
            ("strip_prefix", 2) => {
                let p = if a1.starts_with('/') {
                    a1
                } else {
                    format!("/{a1}")
                };
                let p = p.strip_suffix('/').unwrap_or(&p);
                return Some(params([("strip_prefix", J::strs(&[p]))]));
            }
            ("strip_suffix", 2) => {
                return Some(params([
                    ("regexp", J::Str(format!("{}$", quote_meta(&a1)))),
                    ("replacement", J::str("")),
                ]));
            }
            ("replace", 3 | 4) => {
                if args.len() == 4 {
                    self.warn(
                        d.line,
                        "uri replace: its limit is not converted: fit replaces every match",
                    );
                }
                let a2 = args.get(2).cloned().unwrap_or_default();
                return Some(params([
                    ("regexp", J::Str(quote_meta(&a1))),
                    ("replacement", J::Str(a2.replace('$', "$$"))),
                ]));
            }
            ("path_regexp", 3) => {
                if goregexp::check(&a1).is_err() {
                    self.skip(
                        d,
                        &format!("Go's regular expressions do not take {}", quote(&a1)),
                    );
                    return None;
                }
                let a2 = args.get(2).cloned().unwrap_or_default();
                return Some(params([
                    ("regexp", J::Str(a1)),
                    ("replacement", J::Str(a2)),
                ]));
            }
            _ => {}
        }
        self.skip(d, &format!("uri {op} is not converted"));
        None
    }

    /// The field operations of header or request_header, on their line or in their block:
    /// set, `+`add, `-`delete.
    fn header_ops(
        &mut self,
        d: &Directive,
        args: &[String],
        block: &[Directive],
    ) -> Option<Params> {
        let mut set: BTreeMap<String, String> = BTreeMap::new();
        let mut add: BTreeMap<String, Vec<String>> = BTreeMap::new();
        let mut del: Vec<String> = Vec::new();
        let mut op = |c: &mut Converter, line: i64, a: &[String]| {
            let Some(f) = a.first() else { return };
            if let Some(name) = f.strip_prefix('-') {
                if a.len() != 1 {
                    c.warn(
                        line,
                        format!("{} {f}: skipped: a deletion takes no value", d.name),
                    );
                    return;
                }
                del.push(name.into());
                return;
            }
            if a.len() == 1 {
                c.warn(
                    line,
                    format!("{} {f}: skipped: a field with no value", d.name),
                );
                return;
            }
            if a.len() > 2 {
                c.warn(
                    line,
                    format!(
                        "{} {f}: skipped: replacing within a value is not converted",
                        d.name
                    ),
                );
                return;
            }
            let v = a.get(1).cloned().unwrap_or_default();
            if has_placeholder(&v) {
                c.warn(
                    line,
                    format!(
                        "{} {f}: skipped: its value has placeholders, which fit does not expand",
                        d.name
                    ),
                );
                return;
            }
            if let Some(n) = f.strip_prefix('+') {
                add.entry(n.into()).or_default().push(v);
            } else if let Some(n) = f.strip_prefix('?') {
                c.warn(
                    line,
                    format!(
                        "{} {f}: written as set: fit sets the field whether or not it is there",
                        d.name
                    ),
                );
                set.insert(n.into(), v);
            } else if let Some(n) = f.strip_prefix('>') {
                set.insert(n.into(), v);
            } else {
                set.insert(f.clone(), v);
            }
        };
        op(self, d.line, args);
        for e in block {
            if e.name == "defer" {
                continue;
            }
            let mut a = vec![e.name.clone()];
            a.extend(e.args.iter().cloned());
            op(self, e.line, &a);
        }
        let mut ops = Params::new();
        if !set.is_empty() {
            ops.insert(
                "set".into(),
                J::Map(set.into_iter().map(|(k, v)| (k, J::Str(v))).collect()),
            );
        }
        if !add.is_empty() {
            ops.insert(
                "add".into(),
                J::Map(add.into_iter().map(|(k, v)| (k, J::strs(&v))).collect()),
            );
        }
        if !del.is_empty() {
            ops.insert("delete".into(), J::strs(&del));
        }
        if ops.is_empty() {
            self.skip(d, "no field it changes converted");
            return None;
        }
        Some(ops)
    }

    fn encode(&mut self, d: &Directive, args: &[String]) -> Option<Params> {
        let mut encs: Vec<String> = Vec::new();
        let mut p = Params::new();
        let mut add_enc = |c: &mut Converter, line: i64, e: &str| match e {
            "zstd" | "gzip" | "br" => {
                if !encs.iter().any(|x| x == e) {
                    encs.push(e.into());
                }
            }
            _ => c.warn(
                line,
                format!("encode {e}: skipped: fit compresses with zstd, br and gzip"),
            ),
        };
        for a in args {
            add_enc(self, d.line, a);
        }
        for e in &d.block {
            match e.name.as_str() {
                "zstd" | "gzip" | "br" => {
                    add_enc(self, e.line, &e.name);
                    if let Some(level) = e.args.first() {
                        if let Some((field, n)) = encode_level(&e.name, level) {
                            p.insert(field.into(), J::Int(n));
                        } else {
                            self.warn(e.line, format!("encode {} {level}: the level is not converted: fit's compress takes gzip 1 to 9, br 1 to 11, and zstd 1 to 22 or fastest, default, better and best", e.name));
                        }
                    }
                }
                "minimum_length" => {
                    if let [n] = e.args.as_slice()
                        && let Some(n) = atoi_ok(n).filter(|n| *n >= 0)
                    {
                        p.insert("min_bytes".into(), J::Int(n));
                        continue;
                    }
                    self.skip(e, "minimum_length takes a number of bytes");
                }
                "match" => self.skip(
                    e,
                    "fit compresses what compresses well, by its content type",
                ),
                _ => self.skip(e, "not converted"),
            }
        }
        if !args.is_empty() || !d.block.is_empty() {
            if encs.is_empty() {
                self.skip(d, "none of its encodings converted");
                return None;
            }
            p.insert("encodings".into(), J::strs(&encs));
        } else {
            // Caddy's encode with nothing named offers zstd and gzip.
            p.insert("encodings".into(), J::strs(&["zstd", "gzip"]));
        }
        Some(p)
    }

    fn basic_auth(&mut self, d: &Directive, args: &[String]) -> Option<Params> {
        let mut p = Params::new();
        if args.first().is_some_and(|a| a != "bcrypt") {
            self.skip(d, "fit checks bcrypt hashes");
            return None;
        }
        if let [_, realm] = args {
            p.insert("realm".into(), J::str(realm));
        }
        let mut users = Vec::new();
        for e in &d.block {
            let [hash] = e.args.as_slice() else {
                self.warn(
                    e.line,
                    format!(
                        "{}: skipped: a user takes a name and a password hash",
                        e.name
                    ),
                );
                continue;
            };
            let mut hash = hash.clone();
            if !hash.starts_with("$2") {
                // Caddy before 2.8 wrote hashes in base64.
                match gostd::base64_std(&hash) {
                    Some(b) if b.starts_with(b"$2") => hash = gostd::lossy(&b),
                    _ => {
                        self.warn(e.line, format!("{}: skipped: not a bcrypt hash", e.name));
                        continue;
                    }
                }
            }
            users.push(J::Map(params([
                ("username", J::str(&e.name)),
                ("password_hash", J::Str(hash)),
            ])));
        }
        if users.is_empty() {
            self.skip(d, "no user converted");
            return None;
        }
        p.insert("users".into(), J::Arr(users));
        Some(p)
    }

    fn forward_auth(&mut self, d: &Directive, args: &[String]) -> Option<Params> {
        let mut addrs = args.to_vec();
        let mut rest = Vec::new();
        for e in &d.block {
            match e.name.as_str() {
                "to" => addrs.extend(e.args.iter().cloned()),
                "uri" | "copy_headers" | "header_up" | "trusted_proxies" => rest.push(e),
                _ => {}
            }
        }
        let mut other = |_: &OutUpstream, e: &Directive, _: &mut Warns| {
            matches!(
                e.name.as_str(),
                "uri" | "copy_headers" | "header_up" | "trusted_proxies"
            )
        };
        let name = self.upstream(d, &addrs, &mut other)?;
        let mut p = params([("upstream", J::Str(name))]);
        let mut copied: Vec<String> = Vec::new();
        for e in rest {
            match e.name.as_str() {
                "uri" => {
                    if let [u] = e.args.as_slice()
                        && !has_placeholder(u)
                    {
                        p.insert("uri".into(), J::str(u));
                        continue;
                    }
                    self.skip(e, "a URI with placeholders is not converted");
                }
                "copy_headers" => {
                    let mut all = e.args.clone();
                    all.extend(e.block.iter().map(|x| x.name.clone()));
                    for h in all {
                        if h.contains('>') {
                            self.warn(
                                e.line,
                                format!("copy_headers {h}: skipped: renaming is not converted"),
                            );
                            continue;
                        }
                        copied.push(h);
                    }
                }
                "header_up" => self.skip(e, "fit sends the request's fields to the auth service"),
                _ => self.skip(e, "not converted"),
            }
        }
        if !copied.is_empty() {
            p.insert("copy_headers".into(), J::strs(&copied));
        }
        Some(p)
    }

    /// Converts a directive that answers: its handler, and middleware that goes with it, as a
    /// proxy's header_up.
    pub(super) fn handler(
        &mut self,
        _s: &Site,
        d: &Directive,
        args: &[String],
    ) -> Option<(Handler, Vec<Mw>)> {
        match d.name.as_str() {
            "respond" => self.respond(d, args),
            "error" => {
                let mut status = 500;
                for a in args {
                    if let Some(n) = atoi_ok(a).filter(|n| (400..600).contains(n)) {
                        status = n;
                    }
                }
                self.warn(d.line, format!("error: answered {status} with no body: Caddy's error routes are not converted"));
                Some((
                    Handler {
                        r: OutRef::new("respond", params([("status", J::Int(status))])),
                        ..Handler::default()
                    },
                    Vec::new(),
                ))
            }
            "abort" => {
                self.skip(d, "fit answers every request it takes");
                None
            }
            "file_server" => Some(self.file_server(d, args)),
            "php_fastcgi" => self.php_fastcgi(d, args),
            "reverse_proxy" => self.reverse_proxy(d, args),
            _ => None,
        }
    }

    fn respond(&mut self, d: &Directive, args: &[String]) -> Option<(Handler, Vec<Mw>)> {
        let mut p = Params::new();
        let mut status = None;
        let mut body = String::new();
        match args {
            [] => {}
            [a] => match atoi_ok(a) {
                Some(n) if a.len() == 3 => status = Some(n),
                _ => body.clone_from(a),
            },
            [b, st] => {
                body.clone_from(b);
                let Some(n) = atoi_ok(st) else {
                    self.skip(d, "respond takes a body and a status");
                    return None;
                };
                status = Some(n);
            }
            _ => {
                self.skip(d, "respond takes a body and a status");
                return None;
            }
        }
        for e in &d.block {
            if e.name == "body"
                && let [b] = e.args.as_slice()
            {
                body.clone_from(b);
            } else if e.name == "close" {
                self.skip(e, "fit keeps the connection");
            } else {
                self.skip(e, "not converted");
            }
        }
        if let Some(n) = status {
            if !(100..=599).contains(&n) {
                self.skip(d, "a status out of range");
                return None;
            }
            p.insert("status".into(), J::Int(n));
        }
        if !body.is_empty() {
            if has_placeholder(&body) {
                self.warn(d.line, "respond: its body's placeholders are sent as they are written: fit does not expand them");
            }
            p.insert("body".into(), J::Str(body));
        }
        Some((
            Handler {
                r: OutRef::new("respond", p),
                ..Handler::default()
            },
            Vec::new(),
        ))
    }

    fn file_server(&mut self, d: &Directive, args: &[String]) -> (Handler, Vec<Mw>) {
        let mut p = Params::new();
        let mut h = Handler::default();
        for a in args {
            if a == "browse" {
                p.insert("browse".into(), J::Bool(true));
            } else {
                self.warn(d.line, format!("file_server {a}: skipped: not converted"));
            }
        }
        for e in &d.block {
            match e.name.as_str() {
                "root" => {
                    if let [root] = e.args.as_slice() {
                        h.root.clone_from(root);
                        h.root_set = true;
                        continue;
                    }
                    self.skip(e, "root takes a path");
                }
                "browse" => {
                    p.insert("browse".into(), J::Bool(true));
                    if !e.args.is_empty() || !e.block.is_empty() {
                        self.warn(e.line, "browse: its template and options are not converted");
                    }
                }
                "index" => {
                    p.insert("index".into(), J::strs(&e.args));
                }
                "precompressed" => {
                    let mut encs = Vec::new();
                    for a in &e.args {
                        match a.as_str() {
                            "br" | "zstd" | "gzip" => encs.push(a.clone()),
                            _ => self.warn(e.line, format!("precompressed {a}: skipped: fit serves br, zstd and gzip twins")),
                        }
                    }
                    if e.args.is_empty() {
                        encs = vec!["br".into(), "zstd".into(), "gzip".into()];
                    }
                    if !encs.is_empty() {
                        p.insert("precompressed".into(), J::strs(&encs));
                    }
                }
                "hide" => self.skip(e, "fit hides no files but those whose names start with a dot, which the static handler's dotfiles deny answers 404"),
                _ => self.skip(e, "not converted"),
            }
        }
        h.r = OutRef::new("static", p);
        (h, Vec::new())
    }

    fn php_fastcgi(&mut self, d: &Directive, args: &[String]) -> Option<(Handler, Vec<Mw>)> {
        let Some(first) = args.first() else {
            self.skip(d, "php_fastcgi needs the application's address");
            return None;
        };
        if args.len() > 1 {
            self.warn(
                d.line,
                format!("php_fastcgi: fit sends requests to the first address, {first}"),
            );
        }
        let mut addr = first.clone();
        if addr.starts_with("unix/") {
        } else if addr.starts_with(':') {
            addr = format!("localhost{addr}");
        } else if split_host_port(&addr).is_none() {
            addr = join_host_port(&addr, "9000");
        }
        let mut p = params([
            ("address", J::Str(addr)),
            (
                "try_files",
                J::strs(&["{path}", "{path}/index.php", "/index.php"]),
            ),
        ]);
        let mut env: BTreeMap<String, J> = BTreeMap::new();
        for e in &d.block {
            let n = e.args.len();
            let arg0 = e.args.first().cloned().unwrap_or_default();
            match e.name.as_str() {
                "root" if n == 1 => {
                    p.insert("root".into(), J::Str(arg0));
                }
                "split" if n > 0 => {
                    p.insert("split_path".into(), J::strs(&e.args));
                }
                "index" if n == 1 => {
                    if arg0 == "off" {
                        p.remove("try_files");
                        continue;
                    }
                    p.insert(
                        "try_files".into(),
                        J::strs(&[
                            "{path}".to_owned(),
                            format!("{{path}}/{arg0}"),
                            format!("/{arg0}"),
                        ]),
                    );
                    p.insert("index".into(), J::Str(arg0));
                }
                "try_files" if n > 0 => {
                    if let Some(files) = self.try_files(e, &e.args) {
                        p.insert("try_files".into(), J::strs(&files));
                    }
                }
                "env" if n == 2 => {
                    env.insert(
                        arg0,
                        J::str(e.args.get(1).map(String::as_str).unwrap_or_default()),
                    );
                    p.insert("env".into(), J::Map(env.clone()));
                }
                "dial_timeout" if n == 1 => {
                    if let Some(t) = duration(&arg0) {
                        p.insert("connect_timeout".into(), J::Str(t));
                    }
                }
                "read_timeout" if n == 1 => {
                    if let Some(t) = duration(&arg0) {
                        p.insert("response_idle_timeout".into(), J::Str(t));
                    }
                }
                _ => self.skip(e, "not converted"),
            }
        }
        Some((
            Handler {
                r: OutRef::new("fastcgi", p),
                ..Handler::default()
            },
            Vec::new(),
        ))
    }

    #[allow(clippy::too_many_lines)]
    fn reverse_proxy(&mut self, d: &Directive, args: &[String]) -> Option<(Handler, Vec<Mw>)> {
        let mut addrs = args.to_vec();
        for e in &d.block {
            if e.name == "to" {
                addrs.extend(e.args.iter().cloned());
            }
        }
        let mut p = Params::new();
        let mut req_ops: Vec<Vec<String>> = Vec::new();
        let mut resp_ops: Vec<Vec<String>> = Vec::new();
        let mut req_lines: Vec<i64> = Vec::new();
        let mut resp_lines: Vec<i64> = Vec::new();
        let mut other = |u: &OutUpstream, e: &Directive, w: &mut Warns| -> bool {
            let one = e.args.len() == 1;
            let arg0 = e.args.first().cloned().unwrap_or_default();
            match e.name.as_str() {
                "header_up" => {
                    if !e.args.is_empty() {
                        let f = go_lower(arg0.trim_start_matches('+'));
                        let v = e.args.get(1).cloned().unwrap_or_default();
                        if f == "host" && e.args.len() == 2 {
                            match v.as_str() {
                                "{host}"
                                | "{http.request.host}"
                                | "{hostport}"
                                | "{http.request.hostport}" => {
                                    // fit sends the client's Host.
                                }
                                "{upstream_hostport}"
                                | "{http.reverse_proxy.upstream.hostport}" => {
                                    if let [a] = u.addresses.as_slice() {
                                        p.insert("host_header".into(), J::str(a));
                                    } else {
                                        w.push((e.line, format!("header_up Host {v}: skipped: fit sends one Host to every address")));
                                    }
                                }
                                _ => {
                                    if has_placeholder(&v) {
                                        w.push((
                                            e.line,
                                            format!("header_up Host {v}: skipped: not converted"),
                                        ));
                                    } else {
                                        p.insert("host_header".into(), J::Str(v));
                                    }
                                }
                            }
                        } else if f.starts_with("x-forwarded-")
                            || f == "x-real-ip" && e.args.len() == 2 && v == "{remote_host}"
                        {
                            w.push((e.line, format!("header_up {arg0}: skipped: fit's proxy sets X-Forwarded-For, -Proto and -Host itself")));
                        } else {
                            req_ops.push(e.args.clone());
                            req_lines.push(e.line);
                        }
                    }
                    true
                }
                "header_down" => {
                    resp_ops.push(e.args.clone());
                    resp_lines.push(e.line);
                    true
                }
                "trusted_proxies" => {
                    p.insert("trusted_proxies".into(), J::strs(&matcher::ranges(&e.args)));
                    true
                }
                "lb_retries" => {
                    if one && let Some(n) = atoi_ok(&arg0).filter(|n| *n > 0) {
                        p.insert(
                            "retry".into(),
                            J::Map(params([("attempts", J::Int(n.min(10)))])),
                        );
                        return true;
                    }
                    false
                }
                "lb_try_duration" => {
                    p.entry("retry".into())
                        .or_insert_with(|| J::Map(Params::new()));
                    w.push((e.line, "lb_try_duration: written as fit's retry, which tries again at once, twice by default, within a budget".into()));
                    true
                }
                "flush_interval" | "buffer_requests" | "buffer_responses" | "request_buffers"
                | "response_buffers" | "max_buffer_size" => {
                    w.push(skip_msg(e, "fit streams bodies both ways, as they arrive"));
                    true
                }
                "stream_timeout" => {
                    if one && let Some(t) = duration(&arg0) {
                        p.insert("tunnel_idle_timeout".into(), J::Str(t));
                        return true;
                    }
                    false
                }
                _ => false,
            }
        };
        let name = self.upstream(d, &addrs, &mut other)?;
        p.insert("upstream".into(), J::Str(name));
        let mut extra = Vec::new();
        let mut req_set = Params::new();
        if !req_ops.is_empty()
            && let Some(ops) = self.proxy_headers("header_up", &req_ops, &req_lines)
        {
            req_set.insert("request".into(), J::Map(ops));
        }
        if !resp_ops.is_empty()
            && let Some(ops) = self.proxy_headers("header_down", &resp_ops, &resp_lines)
        {
            req_set.insert("response".into(), J::Map(ops));
        }
        if !req_set.is_empty() {
            extra.push(Mw {
                r: OutRef::new("headers", req_set),
                line: d.line,
            });
        }
        Some((
            Handler {
                r: OutRef::new("proxy", p),
                ..Handler::default()
            },
            extra,
        ))
    }

    /// header_up or header_down lines converted as header_ops does.
    fn proxy_headers(&mut self, name: &str, ops: &[Vec<String>], lines: &[i64]) -> Option<Params> {
        let block: Vec<Directive> = ops
            .iter()
            .zip(lines)
            .filter_map(|(o, l)| {
                o.first().map(|n| Directive {
                    name: n.clone(),
                    args: o.get(1..).unwrap_or_default().to_vec(),
                    line: *l,
                    ..Directive::default()
                })
            })
            .collect();
        let d = Directive {
            name: name.into(),
            line: lines.first().copied().unwrap_or_default(),
            ..Directive::default()
        };
        self.header_ops(&d, &[], &block)
    }

    /// Makes the upstream of reverse_proxy's or forward_auth's addresses and subdirectives,
    /// or reuses one made alike, and returns its name. `other` takes the subdirectives that
    /// are not the upstream's, and says whether it did.
    #[allow(clippy::too_many_lines)]
    fn upstream(
        &mut self,
        d: &Directive,
        addrs: &[String],
        other: &mut dyn FnMut(&OutUpstream, &Directive, &mut Warns) -> bool,
    ) -> Option<String> {
        if addrs.is_empty() {
            self.skip(d, "no upstream address");
            return None;
        }
        let mut u = OutUpstream {
            line: d.line,
            ..OutUpstream::default()
        };
        for a in addrs {
            let (hosts, proto) = match upstream_addr(a) {
                Ok(x) => x,
                Err(why) => {
                    self.warn(d.line, format!("{} {a}: skipped: {why}", d.name));
                    continue;
                }
            };
            if !u.addresses.is_empty() && proto != u.protocol {
                self.warn(
                    d.line,
                    format!(
                        "{} {a}: skipped: fit reaches an upstream's addresses alike",
                        d.name
                    ),
                );
                continue;
            }
            u.protocol = proto.into();
            for h in hosts {
                if !u.addresses.contains(&h) {
                    u.addresses.push(h);
                }
            }
        }
        if u.addresses.is_empty() {
            self.skip(d, "none of its addresses converted");
            return None;
        }
        let mut health = OutHealth::default();
        for e in &d.block {
            if e.name == "to" {
                continue;
            }
            let one = e.args.len() == 1;
            let arg = if one {
                e.args.first().cloned().unwrap_or_default()
            } else {
                String::new()
            };
            let dur = |c: &mut Converter, set: &mut String| match duration(&arg) {
                Some(t) if one => *set = t,
                _ => c.skip(e, "it takes a duration"),
            };
            let num = |c: &mut Converter| match atoi_ok(&arg) {
                Some(n) if one && n >= 0 => Some(n),
                _ => {
                    c.skip(e, "it takes a number");
                    None
                }
            };
            match e.name.as_str() {
                "lb_policy" => self.lb_policy(&mut u, e),
                "health_uri" | "health_path" => {
                    if one {
                        health.path.clone_from(&arg);
                    }
                }
                "health_port" => {
                    if let Some(n) = num(self).filter(|n| *n > 0 && *n < 65536) {
                        health.port = n;
                    }
                }
                "health_interval" => dur(self, &mut health.interval),
                "health_timeout" => dur(self, &mut health.timeout),
                "health_status" => {
                    if let Some(n) = atoi_ok(&arg).filter(|n| one && (100..600).contains(n)) {
                        health.expect_status = vec![n];
                    } else if arg != "2xx" {
                        self.warn(e.line, format!("health_status {arg}: skipped: fit passes any status from 200 to 399, or those listed"));
                    }
                }
                "health_passes" => {
                    if let Some(n) = num(self) {
                        health.passes = n;
                    }
                }
                "health_fails" => {
                    if let Some(n) = num(self) {
                        health.fails = n;
                    }
                }
                "health_headers" => {
                    for f in &e.block {
                        if f.name.eq_ignore_ascii_case("host") && f.args.len() == 1 {
                            health.host = f.args.first().cloned().unwrap_or_default();
                        } else {
                            self.skip(
                                f,
                                "fit's probes send a Host of their own and no other field",
                            );
                        }
                    }
                }
                "fail_duration" => dur(self, &mut u.fail_timeout),
                "max_fails" => {
                    if let Some(n) = num(self) {
                        u.max_fails = Some(n);
                    }
                }
                "unhealthy_status"
                | "unhealthy_latency"
                | "unhealthy_request_count"
                | "health_body"
                | "health_method"
                | "health_request_body"
                | "health_follow_redirects"
                | "passive_health" => self.skip(
                    e,
                    "not converted: fit counts failed connections and responses cut short",
                ),
                "transport" => self.transport(&mut u, e),
                _ => {
                    let mut w = Warns::new();
                    let took = other(&u, e, &mut w);
                    for (line, msg) in w {
                        self.warn(line, msg);
                    }
                    if !took {
                        self.skip(e, "not converted");
                    }
                }
            }
        }
        if health != OutHealth::default() {
            if health.path.is_empty() {
                health.path = "/".into();
            }
            u.health_check = Some(health);
        }
        Some(self.add_upstream(u))
    }

    /// Adds `u`, or finds the upstream made alike before it, and returns its name.
    fn add_upstream(&mut self, mut u: OutUpstream) -> String {
        let sig = u.json().marshal().unwrap_or_default();
        if let Some(&i) = self.upstream_by_sig.get(&sig)
            && let Some(have) = self.upstreams.get(i)
        {
            return have.name.clone();
        }
        let base = format!(
            "proxy_{}",
            gostd::addr_name(
                u.addresses.first().map(String::as_str).unwrap_or_default(),
                true
            )
        );
        let mut name = base.clone();
        let mut i = 2u64;
        while self.upstreams.iter().any(|o| o.name == name) {
            name = format!("{base}-{i}");
            i = i.saturating_add(1);
        }
        u.name.clone_from(&name);
        self.upstream_by_sig.insert(sig, self.upstreams.len());
        self.upstreams.push(u);
        name
    }

    fn lb_policy(&mut self, u: &mut OutUpstream, e: &Directive) {
        let Some(policy) = e.args.first() else {
            self.skip(e, "lb_policy names a policy");
            return;
        };
        match policy.as_str() {
            "round_robin" | "least_conn" | "random" | "ip_hash" | "uri_hash" => {
                u.load_balancing.clone_from(policy);
            }
            "client_ip_hash" => u.load_balancing = "ip_hash".into(),
            "random_choose" => u.load_balancing = "p2c".into(),
            "header" => {
                if let [_, field] = e.args.as_slice() {
                    u.load_balancing = "header_hash".into();
                    u.hash_key.clone_from(field);
                    return;
                }
                self.skip(e, "lb_policy header takes a field");
            }
            "cookie" => {
                let name = e.args.get(1).cloned().unwrap_or_else(|| "lb".into());
                u.sticky_cookie = Some(name);
                if e.args.len() == 3 {
                    self.warn(e.line, "lb_policy cookie: its secret is not converted: fit's cookie names the address");
                }
            }
            "weighted_round_robin" => {
                let mut ws = Vec::new();
                for a in e.args.iter().skip(1) {
                    let Some(n) = atoi_ok(a).filter(|n| *n >= 1) else {
                        self.skip(e, "weights are numbers from 1");
                        return;
                    };
                    ws.push(n.min(1000));
                }
                if ws.len() != u.addresses.len() {
                    self.skip(e, "it needs a weight for each address");
                    return;
                }
                u.load_balancing.clone_from(policy);
                u.weights = ws;
            }
            "first" => self.warn(e.line, "lb_policy first: written as round_robin: fit's proxy fails over to another upstream, named as failover, written by hand"),
            _ => self.skip(e, "fit balances by round robin, weights, least connections, random, P2C, or a hash of the client's address, the URI, a header or a cookie"),
        }
    }

    fn transport(&mut self, u: &mut OutUpstream, e: &Directive) {
        if e.args.first().is_none_or(|a| a != "http") {
            self.skip(e, "fit's proxy speaks HTTP; write php_fastcgi for FastCGI");
            return;
        }
        for f in &e.block {
            let one = f.args.len() == 1;
            let arg = if one {
                f.args.first().cloned().unwrap_or_default()
            } else {
                String::new()
            };
            let dur = |c: &mut Converter, set: &mut String| match duration(&arg) {
                Some(t) if one => *set = t,
                _ => c.skip(f, "it takes a duration"),
            };
            match f.name.as_str() {
                "versions" => {
                    if f.args.iter().any(|a| a == "h2c") {
                        u.protocol = "h2c".into();
                    } else if f.args.iter().any(|a| a == "2") && u.protocol != "h2" {
                        self.warn(f.line, "versions 2: fit speaks HTTP/2 to TLS upstreams, and h2c in the clear");
                    }
                }
                "tls" => u.protocol = "h2".into(),
                "tls_server_name" => {
                    if one {
                        u.tls_server_name = arg;
                    }
                }
                "tls_trusted_ca_certs" => {
                    if one {
                        u.tls_ca_file = arg;
                    }
                }
                "tls_trust_pool" => {
                    if let [kind, file] = f.args.as_slice()
                        && kind == "file"
                    {
                        u.tls_ca_file.clone_from(file);
                    } else {
                        self.skip(f, "fit takes one CA file");
                    }
                }
                "tls_client_auth" => {
                    if let [c, k] = f.args.as_slice() {
                        u.tls_cert_file.clone_from(c);
                        u.tls_key_file.clone_from(k);
                    }
                }
                "dial_timeout" => dur(self, &mut u.connect_timeout),
                "response_header_timeout" => dur(self, &mut u.response_header_timeout),
                "read_timeout" => dur(self, &mut u.response_idle_timeout),
                "keepalive" => {
                    if arg == "off" {
                        u.max_idle_conns = Some(0);
                    } else {
                        dur(self, &mut u.idle_timeout);
                    }
                }
                "keepalive_idle_conns" | "keepalive_idle_conns_per_host" => {
                    if let Some(n) = atoi_ok(&arg).filter(|n| one && *n >= 0) {
                        u.max_idle_conns = Some(n);
                    }
                }
                "max_conns_per_host" => {
                    if let Some(n) = atoi_ok(&arg).filter(|n| one && *n >= 0) {
                        u.max_conns = n;
                    }
                }
                "tls_insecure_skip_verify" => self.skip(f, "fit verifies upstreams' certificates; give it their CA with tls_trusted_ca_certs"),
                _ => self.skip(f, "not converted"),
            }
        }
    }
}

/// An upstream address of Caddy's, `[scheme://]host[:port]`, and a port range as
/// `host:8001-8003`, as fit's addresses and the protocol they are reached with; or why not.
fn upstream_addr(a: &str) -> Result<(Vec<String>, &'static str), &'static str> {
    if a.contains('{') {
        return Err("placeholders are not converted");
    }
    let (mut proto, mut port) = ("", "80");
    let mut a = a;
    if let Some((scheme, rest)) = a.split_once("://") {
        match scheme {
            "http" => {}
            "https" => {
                proto = "h2";
                port = "443";
            }
            "h2c" => proto = "h2c",
            _ => return Err("fit reaches upstreams over http://, https:// and h2c://"),
        }
        a = rest;
    }
    if a.starts_with("unix/") {
        return Err("fit reaches upstreams over TCP");
    }
    if a.contains('/') {
        return Err("an upstream address has no path");
    }
    let (mut host, ports) = if let Some((h, p)) = split_host_port(a) {
        (h, p)
    } else if let Some(p) = a.strip_prefix(':') {
        ("", p)
    } else {
        (a, port)
    };
    if host.is_empty() {
        host = "localhost";
    }
    let (from, to) = ports.split_once('-').unwrap_or((ports, ports));
    let range = match (atoi(from), atoi(to)) {
        (Ok(lo), Ok(hi)) if lo >= 1 && hi <= 65535 && lo <= hi && hi - lo <= 100 => lo..=hi,
        _ => return Err("not a port, or a range of 100 at most"),
    };
    Ok((
        range
            .map(|p| join_host_port(host, &p.to_string()))
            .collect(),
        proto,
    ))
}

/// The compress param, and its value, that an encode subdirective's level for `encoding`
/// comes to: gzip and br by number, and zstd by number or by Caddy's names for its four speeds.
pub(super) fn encode_level(encoding: &str, level: &str) -> Option<(&'static str, i64)> {
    let (field, hi) = match encoding {
        "br" => ("brotli_level", 11),
        "zstd" => {
            let named = match level {
                "fastest" => Some(1),
                "default" => Some(3),
                "better" => Some(6),
                "best" => Some(10),
                _ => None,
            };
            if let Some(n) = named {
                return Some(("zstd_level", n));
            }
            ("zstd_level", 22)
        }
        _ => ("gzip_level", 9),
    };
    let n = atoi_ok(level)?;
    (1..=hi).contains(&n).then_some((field, n))
}

/// A duration of Caddy's, Go's with days, as fit's.
pub(super) fn duration(s: &str) -> Option<String> {
    // (\d+(?:\.\d+)?)d, each replaced with its hours.
    let b = s.as_bytes();
    let mut out = String::with_capacity(s.len());
    let (mut i, mut copied) = (0usize, 0usize);
    while i < b.len() {
        if let Some((end, num)) = days_at(s, i) {
            out.push_str(s.get(copied..i).unwrap_or_default());
            let n = parse_float(num).unwrap_or(0.0);
            out.push_str(&format_float(24.0 * n));
            out.push('h');
            i = end;
            copied = end;
        } else {
            i += 1;
        }
    }
    out.push_str(s.get(copied..).unwrap_or_default());
    match crate::duration::parse(&out) {
        Some(Ok(_)) => Some(out),
        _ => None,
    }
}

/// `\d+(?:\.\d+)?d` at `i`: its end and its number.
fn days_at(s: &str, i: usize) -> Option<(usize, &str)> {
    let b = s.as_bytes();
    let digits = |from: usize| {
        let mut j = from;
        while b.get(j).is_some_and(u8::is_ascii_digit) {
            j += 1;
        }
        j
    };
    let j = digits(i);
    if j == i {
        return None;
    }
    if b.get(j) == Some(&b'.') {
        let k = digits(j + 1);
        if k > j + 1 && b.get(k) == Some(&b'd') {
            return Some((k + 1, s.get(i..k)?));
        }
    }
    (b.get(j) == Some(&b'd')).then(|| (j + 1, s.get(i..j).unwrap_or_default()))
}

/// A size of Caddy's, as 10MB or 1GiB, in bytes.
pub(super) fn size(s: &str) -> Option<i64> {
    // ^(\d+(?:\.\d+)?)\s*([kKmMgGtT]?)(i?)[bB]?$
    let b = s.as_bytes();
    let mut j = 0usize;
    while b.get(j).is_some_and(u8::is_ascii_digit) {
        j += 1;
    }
    if j == 0 {
        return None;
    }
    if b.get(j) == Some(&b'.') {
        let k = j + 1;
        let mut e = k;
        while b.get(e).is_some_and(u8::is_ascii_digit) {
            e += 1;
        }
        if e == k {
            return None;
        }
        j = e;
    }
    let num = s.get(..j)?;
    while b
        .get(j)
        .is_some_and(|c| matches!(c, b'\t' | b'\n' | b'\x0c' | b'\r' | b' '))
    {
        j += 1;
    }
    let mut exp = 0;
    if let Some(u) = b
        .get(j)
        .and_then(|c| "kmgt".find(char::from(c.to_ascii_lowercase())))
    {
        exp = u + 1;
        j += 1;
    }
    let mut binary = false;
    if b.get(j) == Some(&b'i') {
        binary = true;
        j += 1;
    }
    if matches!(b.get(j), Some(b'b' | b'B')) {
        j += 1;
    }
    if j != b.len() {
        return None;
    }
    let n = parse_float(num)?;
    let base: f64 = if binary { 1024.0 } else { 1000.0 };
    let v = n * base.powi(i32::try_from(exp).unwrap_or(0));
    #[allow(clippy::cast_precision_loss)]
    let limit = (i64::MAX / 2) as f64;
    if v > limit {
        return None;
    }
    #[allow(clippy::cast_possible_truncation)]
    Some(v as i64)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn units_as_fit() {
        assert_eq!(duration("1d").as_deref(), Some("24h"));
        assert_eq!(duration("1.5d").as_deref(), Some("36h"));
        assert_eq!(duration("1d2h").as_deref(), Some("24h2h"));
        assert_eq!(duration("-1s"), None);
        assert_eq!(duration("soon"), None);
        assert_eq!(size("10MB"), Some(10_000_000));
        assert_eq!(size("1GiB"), Some(1 << 30));
        assert_eq!(size("1.5k"), Some(1500));
        assert_eq!(size("5"), Some(5));
        assert_eq!(size("5.b"), None);
        assert_eq!(size("99999999999999999999T"), None);
        assert_eq!(encode_level("zstd", "better"), Some(("zstd_level", 6)));
        assert_eq!(encode_level("br", "12"), None);
        assert_eq!(encode_level("gzip", "9"), Some(("gzip_level", 9)));
    }
}
