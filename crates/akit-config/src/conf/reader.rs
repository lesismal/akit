//! What a `.conf` file's directives say before they are fields: `def` and `use`, read in place,
//! `include` and `import`, refused as not supported yet, and `site`, expanded into its routes; as
//! fit's `adapter/fitconf` (`include.go`, `site.go`).

use std::collections::HashMap;

use super::{Directive, MAX_TOKENS, Tok, err};
use crate::error::NOT_SUPPORTED_YET;
use crate::json::ParseError;

/// How deep uses nest, a def using another and so on, as fit's bound on includes and uses.
const MAX_USE_DEPTH: usize = 16;

/// Reads a file's defs and uses.
pub(super) struct Reader {
    /// The defs being read, each using the next; the file itself first.
    stack: Vec<String>,
    defs: HashMap<String, Vec<Directive>>,
    /// The words read, the defs' uses counted.
    words: usize,
}

impl Reader {
    pub(super) fn new(words: usize) -> Reader {
        Reader {
            stack: vec![String::new()],
            defs: HashMap::new(),
            words,
        }
    }

    /// `ds` with each `use` replaced with the directives of the defs it names, each def's
    /// definition taken out; `top` says they are at the top of the file.
    ///
    /// ```text
    /// def <name> { ... }            at the top: directives to use by name
    /// use <defs...>                 anywhere: those directives, read here
    /// include <paths or globs...>   anywhere: other files' directives (not supported yet)
    /// import <paths or globs...>    at the top: other config files (not supported yet)
    /// ```
    pub(super) fn splice(
        &mut self,
        ds: Vec<Directive>,
        top: bool,
    ) -> Result<Vec<Directive>, ParseError> {
        let mut out = Vec::with_capacity(ds.len());
        for mut d in ds {
            if !d.name.quoted {
                match d.name.text.as_str() {
                    "import" => {
                        if !top || self.stack.len() > 1 {
                            return Err(err(
                                d.name.off,
                                "import is given at the top of a config file only, not in a block or a file included: it imports whole config files",
                            ));
                        }
                        if d.args.is_empty() || d.has_block {
                            return Err(err(
                                d.name.off,
                                "import takes the config files to import, as in import sites/*.conf",
                            ));
                        }
                        return Err(err(
                            d.name.off,
                            format!(
                                "{NOT_SUPPORTED_YET}: import, other config files named by a .conf file (name them on the command line instead)"
                            ),
                        ));
                    }
                    "def" => {
                        self.define(d, top)?;
                        continue;
                    }
                    "use" => {
                        if d.args.is_empty() || d.has_block {
                            return Err(err(
                                d.name.off,
                                "use takes the defs to read, as in use tls-common",
                            ));
                        }
                        out.extend(self.use_all(&d, top)?);
                        continue;
                    }
                    "include" => {
                        if d.args.is_empty() || d.has_block {
                            return Err(err(
                                d.name.off,
                                "include takes the files to include, as in include routes/*.conf",
                            ));
                        }
                        return Err(err(
                            d.name.off,
                            format!(
                                "{NOT_SUPPORTED_YET}: include, other files' directives read in a .conf file (use def and use within the file)"
                            ),
                        ));
                    }
                    _ => {}
                }
            }
            if d.has_block {
                let block = std::mem::take(&mut d.block);
                d.block = self.splice(block, false)?;
            }
            out.push(d);
        }
        Ok(out)
    }

    fn define(&mut self, d: Directive, top: bool) -> Result<(), ParseError> {
        if !top {
            return Err(err(
                d.name.off,
                "def is given at the top of a file only, not in a block",
            ));
        }
        let [name] = d.args.as_slice() else {
            return Err(err(
                d.name.off,
                "def takes its name and a block, as in def pool { max_conns 1024 }",
            ));
        };
        if !d.has_block {
            return Err(err(
                d.name.off,
                "def takes its name and a block, as in def pool { max_conns 1024 }",
            ));
        }
        if name.text.is_empty() || name.text.contains(['*', '?', '[', '/', '\\']) {
            return Err(err(
                name.off,
                format!(
                    "{:?} cannot name a def: a name holds no /, \\, *, ? or [",
                    name.text
                ),
            ));
        }
        if self.defs.contains_key(&name.text) {
            return Err(err(name.off, format!("def {} is defined twice", name.text)));
        }
        self.defs.insert(name.text.clone(), d.block);
        Ok(())
    }

    fn use_all(&mut self, d: &Directive, top: bool) -> Result<Vec<Directive>, ParseError> {
        if self.stack.len() > MAX_USE_DEPTH {
            return Err(err(
                d.name.off,
                format!("uses and includes nested more than {MAX_USE_DEPTH} deep"),
            ));
        }
        let mut out = Vec::new();
        for a in &d.args {
            let Some(block) = self.defs.get(&a.text) else {
                return Err(err(
                    a.off,
                    format!(
                        "use {}: there is no def {} defined before it",
                        a.text, a.text
                    ),
                ));
            };
            let name = format!("def {}", a.text);
            if self.stack.contains(&name) {
                return Err(err(a.off, format!("{name} uses itself")));
            }
            let ds = block.clone();
            self.words = self.words.saturating_add(count(&ds));
            if self.words > MAX_TOKENS {
                return Err(err(
                    a.off,
                    format!("the config holds more than {MAX_TOKENS} words, with the defs it uses"),
                ));
            }
            self.stack.push(name);
            let inner = self.splice(ds, top);
            self.stack.pop();
            out.extend(inner?);
        }
        Ok(out)
    }
}

/// The words `ds` holds.
fn count(ds: &[Directive]) -> usize {
    ds.iter().fold(0usize, |n, d| {
        n.saturating_add(1)
            .saturating_add(d.args.len())
            .saturating_add(count(&d.block))
    })
}

/// The directives a site shares with its routes.
const SITE_SHARED: &[&str] = &["listeners", "priority", "match", "middleware"];

/// The top-level directives with each `site` replaced with its routes, where it was written.
///
/// ```text
/// site <hosts...> {      or site * { for any host
///     listeners ...      the routes' listeners, where a route sets none
///     priority <n>       their priority, where a route sets none
///     match { ... }      what every route matches too, but host and path;
///                        a route's match setting the same field wins
///     middleware ...     middleware every route runs, before its own
///     route [<paths...>] { ... }
/// }
/// ```
pub(super) fn expand_sites(ds: Vec<Directive>) -> Result<Vec<Directive>, ParseError> {
    let mut out = Vec::with_capacity(ds.len());
    for d in ds {
        if d.name.text == "site" {
            out.extend(site_routes(d)?);
        } else {
            out.push(d);
        }
    }
    Ok(out)
}

fn named(name: &str, at: &Tok) -> Tok {
    Tok::made(name, at.off)
}

#[allow(clippy::too_many_lines)]
fn site_routes(site: Directive) -> Result<Vec<Directive>, ParseError> {
    if site.args.is_empty() {
        return Err(err(
            site.name.off,
            "a site needs its host names, as in site example.com www.example.com { ... }",
        ));
    }
    if !site.has_block {
        return Err(err(site.name.off, "a site needs a block with its routes"));
    }
    let mut any_host = false;
    for a in &site.args {
        if a.text == "*" && !a.quoted {
            if site.args.len() > 1 {
                return Err(err(
                    a.off,
                    "* is a site of any host, given alone: site * { ... }",
                ));
            }
            any_host = true;
        }
    }
    let mut shared: Vec<Directive> = Vec::new();
    let mut middleware = Vec::new();
    let mut routes = Vec::new();
    for d in site.block {
        let name = d.name.text.as_str();
        if name == "route" {
            routes.push(d);
        } else if name == "middleware" {
            middleware.push(d);
        } else if SITE_SHARED.contains(&name) {
            if shared.iter().any(|s| s.name.text == name) {
                return Err(err(d.name.off, format!("a site's {name} is given twice")));
            }
            if name == "match" {
                if let Some(m) = d
                    .block
                    .iter()
                    .find(|m| m.name.text == "host" || m.name.text == "path")
                {
                    return Err(err(
                        m.name.off,
                        "a site's routes take their hosts from the site and their paths from their own arguments",
                    ));
                }
            }
            shared.push(d);
        } else {
            return Err(err(
                d.name.off,
                format!(
                    "a site holds listeners, priority, match, middleware and its routes, not {name}"
                ),
            ));
        }
    }
    if routes.is_empty() {
        return Err(err(
            site.name.off,
            "a site needs a route, as in route /* { handler ... }",
        ));
    }
    let shared_of = |name: &str| shared.iter().find(|s| s.name.text == name);
    let mut out = Vec::with_capacity(routes.len());
    for r in routes {
        if !r.has_block {
            return Err(err(r.name.off, "a route needs a block with its handler"));
        }
        let mut route = Directive {
            name: named("route", &r.name),
            args: Vec::new(),
            block: Vec::new(),
            has_block: true,
        };
        let mut matcher = Directive {
            name: named("match", &r.name),
            args: Vec::new(),
            block: Vec::new(),
            has_block: true,
        };
        if !any_host {
            matcher.block.push(Directive {
                name: named("host", &site.name),
                args: site.args.clone(),
                block: Vec::new(),
                has_block: false,
            });
        }
        if !r.args.is_empty() {
            matcher.block.push(Directive {
                name: named("path", &r.name),
                args: r.args.clone(),
                block: Vec::new(),
                has_block: false,
            });
        }
        let mut own: Option<Directive> = None;
        let mut body = Vec::new();
        let mut own_middleware = Vec::new();
        for d in r.block {
            match d.name.text.as_str() {
                "match" => {
                    if own.is_some() {
                        return Err(err(d.name.off, "a route's match is given twice"));
                    }
                    own = Some(d);
                }
                "middleware" => own_middleware.push(d),
                _ => body.push(d),
            }
        }
        let set = |name: &str| body.iter().any(|d: &Directive| d.name.text == name);
        if let Some(own) = &own {
            for m in &own.block {
                match m.name.text.as_str() {
                    "host" => {
                        return Err(err(
                            m.name.off,
                            "a site's route takes its hosts from the site",
                        ));
                    }
                    "path" if !r.args.is_empty() => {
                        return Err(err(
                            m.name.off,
                            "the route's paths are given twice, as its arguments and in its match",
                        ));
                    }
                    _ => {}
                }
            }
            matcher.block.extend(own.block.iter().cloned());
        }
        if let Some(sm) = shared_of("match") {
            for m in &sm.block {
                let overridden = own
                    .as_ref()
                    .is_some_and(|o| o.block.iter().any(|x| x.name.text == m.name.text));
                if !overridden {
                    matcher.block.push(m.clone());
                }
            }
        }
        if !matcher.block.is_empty() {
            route.block.push(matcher);
        }
        for name in ["listeners", "priority"] {
            if let Some(d) = shared_of(name) {
                if !set(name) {
                    route.block.push(d.clone());
                }
            }
        }
        route.block.extend(middleware.iter().cloned());
        route.block.extend(own_middleware);
        route.block.extend(body);
        out.push(route);
    }
    Ok(out)
}
