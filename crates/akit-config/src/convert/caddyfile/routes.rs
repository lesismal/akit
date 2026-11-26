//! A site's routes: a route for each handler and each handle block's routes, the directives
//! that change them applied in Caddy's order, a route split where a directive covers it in part.

use std::collections::HashMap;

use super::matcher::{OutMatch, intersect, same_match, set_paths};
use super::parse::Directive;
use super::{Converter, Group, OutRef, OutRoute, Site, params};
use crate::convert::gostd::J;

/// A route being made: its match, the middleware of the blocks around it still to be put
/// before its own, and its handler, whose root and try_files are settled once every block has
/// had its say.
#[derive(Debug, Clone, Default)]
pub(super) struct Route {
    pub(super) matcher: OutMatch,
    pub(super) pre: Vec<Mw>,
    pub(super) mw: Vec<Mw>,
    pub(super) h: Handler,
    pub(super) line: i64,
    pub(super) hint: String,
    /// The exclusive directives this route took, by name, with the scope that gave them: only
    /// the most specific of a scope applies.
    pub(super) got: HashMap<String, i64>,
}

#[derive(Debug, Clone, Default)]
pub(super) struct Mw {
    pub(super) r: OutRef,
    pub(super) line: i64,
}

/// A route's handler: what answers, and for static files and FastCGI, the root and try_files
/// the blocks give it.
#[derive(Debug, Clone, Default)]
pub(super) struct Handler {
    pub(super) r: OutRef,
    pub(super) line: i64,
    pub(super) root: String,
    pub(super) root_set: bool,
    pub(super) try_files: Vec<String>,
    pub(super) try_set: bool,
    pub(super) try_line: i64,
}

/// What a modifier sets on the handlers it covers.
#[derive(Debug, Clone)]
pub(super) enum Set {
    Root(String),
    TryFiles(Vec<String>, i64),
}

/// A directive that changes the routes it covers: it adds middleware, or sets the handler's
/// root or try_files.
#[derive(Debug, Clone)]
pub(super) struct Modifier {
    pub(super) name: String,
    pub(super) line: i64,
    pub(super) matcher: Option<OutMatch>,
    pub(super) spec: i64,
    pub(super) order: i64,
    pub(super) add: Option<Mw>,
    pub(super) set: Option<Set>,
}

impl Set {
    /// Sets it on `h`, unless `h` has it already.
    fn apply(&self, h: &mut Handler) -> bool {
        match self {
            Set::Root(root) => {
                if h.root_set {
                    return false;
                }
                h.root.clone_from(root);
                h.root_set = true;
            }
            Set::TryFiles(files, line) => {
                if h.try_set {
                    return false;
                }
                h.try_files.clone_from(files);
                h.try_set = true;
                h.try_line = *line;
            }
        }
        true
    }
}

/// Caddy's order of the directives this converter knows.
pub(super) fn order(name: &str) -> i64 {
    match name {
        "root" => 1,
        "header" => 3,
        "request_body" => 4,
        "redir" => 5,
        "rewrite" => 6,
        "uri" => 7,
        "try_files" => 8,
        "basicauth" | "basic_auth" => 9,
        "forward_auth" => 10,
        "request_header" => 11,
        "encode" => 12,
        "handle" => 13,
        "handle_path" => 14,
        "route" => 15,
        "abort" => 18,
        "error" => 19,
        "respond" => 20,
        "reverse_proxy" => 21,
        "php_fastcgi" => 22,
        "file_server" => 23,
        _ => 0,
    }
}

/// The directives of which, in one block, only the one with the most specific matcher applies
/// to a request, as in Caddy.
fn exclusive(name: &str) -> bool {
    matches!(name, "root" | "rewrite" | "uri" | "try_files" | "redir")
}

/// The bound on the routes one site may make.
pub(super) const MAX_SITE_ROUTES: usize = 2000;

impl Converter {
    /// Makes a site's routes for a group of its addresses.
    pub(super) fn site_routes(&mut self, s: &Site, g: &Group) {
        let mut base = OutMatch::default();
        for h in &g.hosts {
            if h.is_empty() {
                base.host.clear();
                break;
            }
            if !base.host.contains(h) {
                base.host.push(h.clone());
            }
        }
        if let Some(p) = g.paths.first()
            && !p.is_empty()
        {
            if g.paths.iter().any(|q| q != p) {
                self.warn(
                    s.block.line,
                    format!(
                        "site {}: its addresses have different paths: matched by none",
                        s.block.keys.join(", ")
                    ),
                );
            } else {
                base.path = vec![p.clone()];
            }
        }
        let rs = self.scope(s, &s.block.dirs, &base, false, 0);
        if rs.len() > MAX_SITE_ROUTES {
            self.warn(
                s.block.line,
                format!(
                    "site {}: skipped: it makes more than {MAX_SITE_ROUTES} routes",
                    s.block.keys.join(", ")
                ),
            );
            return;
        }
        for r in rs {
            self.finish(s, g, r);
        }
    }

    /// Settles a route's handler and adds it to the config.
    fn finish(&mut self, s: &Site, g: &Group, r: Route) {
        let mut h = r.h.r.clone();
        match h.module.as_str() {
            "static" => {
                let mut root = r.h.root.clone();
                if !r.h.root_set {
                    root = ".".into();
                    self.warn(r.h.line, "file_server with no root: Caddy serves the current directory, written as \".\"");
                }
                h.params.insert("root".into(), J::Str(root));
                if !r.h.try_files.is_empty() {
                    h.params.insert("try_files".into(), J::strs(&r.h.try_files));
                }
            }
            "fastcgi" => {
                if !h.params.contains_key("root") {
                    if !r.h.root_set {
                        self.warn(r.h.line, "php_fastcgi: skipped: it needs a root, which akit takes as an absolute path; write root");
                        return;
                    }
                    h.params.insert("root".into(), J::str(&r.h.root));
                }
                if !h.params.contains_key("try_files") && !r.h.try_files.is_empty() {
                    h.params.insert("try_files".into(), J::strs(&r.h.try_files));
                }
            }
            _ => {
                if r.h.try_set && r.hint != "default" {
                    self.warn(
                        r.h.try_line,
                        format!(
                            "try_files: applies to static files and FastCGI only, not to the {} routes it covers",
                            h.module
                        ),
                    );
                }
            }
        }
        let name = self.route_name(&s.name, &r.hint);
        self.routes.push(OutRoute {
            name,
            listeners: g.listeners.clone(),
            matcher: (!r.matcher.empty()).then(|| r.matcher.clone()),
            middleware: r.mw.iter().map(|m| m.r.clone()).collect(),
            handler: h,
            line: r.line,
            mw_lines: r.mw.iter().map(|m| m.line).collect(),
            h_line: r.h.line,
        });
    }

    /// The routes of a block's directives, for the requests `base` matches: a route for each
    /// handler and each handle block's routes, and for what none of them takes, a route
    /// answering 200 with no body, as Caddy does. Then the block's other directives apply to
    /// the routes they cover, splitting a route they cover in part. In a route block,
    /// directives keep the order they are written in.
    #[allow(clippy::too_many_lines)]
    pub(super) fn scope(
        &mut self,
        s: &Site,
        dirs: &[Directive],
        base: &OutMatch,
        ordered: bool,
        depth: usize,
    ) -> Vec<Route> {
        self.scopes += 1;
        let id = self.scopes;
        let mut rs: Vec<Route> = Vec::new();
        let mut mods: Vec<Modifier> = Vec::new();
        let mut terms: Vec<(&Directive, i64)> = Vec::new();
        for d in dirs {
            if d.name.starts_with('@') {
                continue;
            }
            match d.name.as_str() {
                "tls" | "bind" | "log" => {
                    if depth > 0 {
                        self.skip(d, "it belongs in the site's block");
                    }
                }
                "handle" | "handle_path" | "route" | "respond" | "reverse_proxy"
                | "php_fastcgi" | "file_server" | "error" | "abort" => {
                    terms.push((d, order(&d.name)));
                }
                "root" | "header" | "request_body" | "redir" | "rewrite" | "uri" | "try_files"
                | "basicauth" | "basic_auth" | "forward_auth" | "request_header" | "encode" => {
                    if let Some(m) = self.modifier(s, d) {
                        mods.push(m);
                    }
                }
                "metrics" => {
                    self.metrics = true;
                    self.warn(
                        d.line,
                        "metrics: akit serves them on the Admin API at /metrics, not on the site",
                    );
                }
                "handle_errors" => self.skip(d, "error pages are not converted"),
                "import" => self.skip(d, "not converted"),
                "encode_gzip" | "gzip" => self.skip(d, "write encode"),
                _ => self.skip(d, "not converted"),
            }
        }
        if !ordered {
            terms.sort_by_key(|t| t.1);
        }
        for (d, _) in terms {
            let more = self.terminal(s, d, base, depth);
            rs.extend(more);
        }
        self.php_with_files(&mut rs);
        if !rs.iter().any(|r| same_match(&r.matcher, base)) {
            rs.push(Route {
                matcher: base.clone(),
                h: Handler {
                    r: OutRef::new("respond", crate::convert::gostd::Params::new()),
                    ..Handler::default()
                },
                hint: "default".into(),
                ..Route::default()
            });
        }
        if !ordered {
            mods.sort_by(|a, b| a.order.cmp(&b.order).then(b.spec.cmp(&a.spec)));
        }
        for m in &mods {
            let mut i = 0usize;
            while let Some(r) = rs.get(i) {
                if !ordered && exclusive(&m.name) && r.got.get(&m.name) == Some(&id) {
                    i += 1;
                    continue;
                }
                let Some(both) = intersect(Some(&r.matcher), m.matcher.as_ref()) else {
                    i += 1;
                    continue;
                };
                let target = i;
                let mut next = i + 1;
                if !same_match(&both, &r.matcher) {
                    if rs.len() >= MAX_SITE_ROUTES {
                        self.warn(
                            m.line,
                            format!("{}: not applied: the site makes too many routes", m.name),
                        );
                        break;
                    }
                    // The part the modifier covers is split off before the route, which it
                    // passes over.
                    let mut t = r.clone();
                    if let Some(p) = both.path.first() {
                        t.hint.clone_from(p);
                    }
                    t.matcher = both;
                    rs.insert(i, t);
                    next = i + 2;
                }
                i = next;
                let Some(t) = rs.get_mut(target) else { break };
                if let Some(add) = &m.add {
                    t.pre.push(add.clone());
                }
                if let Some(set) = &m.set
                    && !set.apply(&mut t.h)
                {
                    continue;
                }
                t.got.insert(m.name.clone(), id);
            }
        }
        for r in &mut rs {
            let mut mw = std::mem::take(&mut r.pre);
            mw.append(&mut r.mw);
            r.mw = mw;
        }
        rs
    }

    /// A directive's matcher, taken off its arguments: `*`, a path, or a named matcher's
    /// name. With `single`, a lone argument is never a matcher, as Caddy reads root, redir,
    /// rewrite and respond. `None` when the directive's matcher is one that was not converted.
    pub(super) fn matcher(
        &mut self,
        s: &Site,
        d: &Directive,
        single: bool,
    ) -> Option<(Option<OutMatch>, Vec<String>, i64)> {
        let args = &d.args;
        let Some(a) = args.first() else {
            return Some((None, Vec::new(), -1));
        };
        if single && args.len() == 1 {
            return Some((None, args.clone(), -1));
        }
        let rest = args.get(1..).unwrap_or_default().to_vec();
        if a == "*" {
            return Some((None, rest, -1));
        }
        if a.starts_with('/') {
            let mut m = OutMatch::default();
            set_paths(&mut m, std::slice::from_ref(a));
            let mut spec = 2 * i64::try_from(a.len()).unwrap_or(i64::MAX / 4);
            if !a.ends_with('*') {
                spec += 1;
            }
            return Some((Some(m), rest, spec));
        }
        if a.starts_with('@') {
            if let Some(m) = s.matchers.get(a) {
                return Some((Some(m.clone()), rest, 1 << 20));
            }
            if !s.bad_match.contains_key(a) {
                self.warn(
                    d.line,
                    format!("{}: skipped: no matcher {a} is defined", d.name),
                );
            }
            return None;
        }
        Some((None, args.clone(), -1))
    }

    /// The routes of a directive that answers, or of a block.
    fn terminal(&mut self, s: &Site, d: &Directive, base: &OutMatch, depth: usize) -> Vec<Route> {
        let single = d.name == "respond";
        let Some((m, args, _)) = self.matcher(s, d, single) else {
            return Vec::new();
        };
        let Some(matched) = intersect(Some(base), m.as_ref()) else {
            self.warn(
                d.line,
                format!(
                    "{}: skipped: its matcher matches nothing its site or block does",
                    d.name
                ),
            );
            return Vec::new();
        };
        let hint = matched.path.first().cloned().unwrap_or_default();
        match d.name.as_str() {
            "handle" | "route" => {
                if !args.is_empty() {
                    self.warn(
                        d.line,
                        format!("{}: its arguments are not converted", d.name),
                    );
                }
                return self.scope(s, &d.block, &matched, d.name == "route", depth + 1);
            }
            "handle_path" => {
                let Some(m) = m.filter(|m| m.path.len() == 1) else {
                    self.skip(d, "handle_path takes one path");
                    return Vec::new();
                };
                let mut rs = self.scope(s, &d.block, &matched, false, depth + 1);
                let p0 = m.path.first().cloned().unwrap_or_default();
                let p0 = p0.strip_suffix('*').unwrap_or(&p0);
                let prefix = p0.strip_suffix('/').unwrap_or(p0);
                if !prefix.is_empty() {
                    let strip = Mw {
                        r: OutRef::new("rewrite", params([("strip_prefix", J::strs(&[prefix]))])),
                        line: d.line,
                    };
                    for r in &mut rs {
                        r.mw.insert(0, strip.clone());
                    }
                }
                return rs;
            }
            _ => {}
        }
        let Some((mut h, extra)) = self.handler(s, d, &args) else {
            return Vec::new();
        };
        h.line = d.line;
        vec![Route {
            matcher: matched,
            h,
            mw: extra,
            line: d.line,
            hint,
            ..Route::default()
        }]
    }

    /// Makes php_fastcgi share a block with file_server: fit's FastCGI handler takes the paths
    /// of scripts and directories, and file_server the rest.
    fn php_with_files(&mut self, rs: &mut [Route]) {
        for pi in 0..rs.len() {
            if rs.get(pi).is_none_or(|p| p.h.r.module != "fastcgi") {
                continue;
            }
            for fi in 0..rs.len() {
                let (Some(p), Some(f)) = (rs.get(pi), rs.get(fi)) else {
                    continue;
                };
                if f.h.r.module != "static" || !same_match(&f.matcher, &p.matcher) {
                    continue;
                }
                let php = OutMatch {
                    path_regexp: vec![r"(?i)\.php(/|$)".into(), "/$".into()],
                    ..OutMatch::default()
                };
                let matched = intersect(Some(&p.matcher), Some(&php));
                let line = p.h.line;
                if let Some(p) = rs.get_mut(pi) {
                    // Go's intersect never fails on a path regexp: it is the match's or an
                    // expression's.
                    p.matcher = matched.unwrap_or_default();
                    p.h.r.params.remove("try_files");
                    p.h.r
                        .params
                        .insert("try_files".into(), J::strs(&["{path}"]));
                }
                self.warn(line, "php_fastcgi with file_server: PHP takes the paths of scripts and directories, file_server the rest; a path that is neither a file nor a script is not sent to index.php");
                break;
            }
        }
    }
}
