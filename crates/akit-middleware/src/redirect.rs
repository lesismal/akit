//! The `redirect` middleware, as fit's `middleware/rewrite` `Redirect`: requests answered with a
//! redirect to a URL made from them; with a `regexp`, only those whose escaped path it matches,
//! the others going on to the handler.
//!
//! The URL's parts are found when the config is compiled (rule 12): per request, the
//! placeholders are filled in, nothing else. With a regexp and a `$` in `to`, the groups of the
//! match (`$1`, `${name}`, Go's `Regexp.Expand` on akio-regex's captures: akio API request A32)
//! are put in first, then the placeholders, in fit's order. A URL that the request's parts would
//! make leave the site, where `to` does not, is answered 400, as fit answers it (an open
//! redirect).

use std::sync::Arc;

use akio_http::{BodyMode, Context, Request, Response};
use akio_regex::Regex;
use akit_runtime::{
    Handler, HandlerParams, Middleware, Provision, ProvisionError, Provisioned, TLS,
};

use crate::url;

/// fit's answer to a request whose parts make the URL an open redirect.
const BAD_TARGET: &str = "400 bad redirect target\n";

/// A part of the URL redirected to.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Part {
    /// Text as `to` writes it.
    Text(Box<str>),
    /// `{scheme}`: `https` over TLS, else `http`.
    Scheme,
    /// `{host}`: the `Host`, with its port, as the client sent it.
    Host,
    /// `{path}`: the escaped path.
    Path,
    /// `{query}`: the query, without the `?`.
    Query,
    /// `{uri}`: the escaped path and, when there is one, `?` and the query.
    Uri,
}

/// The redirect, compiled.
#[derive(Debug, Clone)]
pub(crate) struct Redirect {
    status: u16,
    parts: Arc<[Part]>,
    regexp: Option<Arc<Regex>>,
    /// With a regexp, `to` as written when it holds a `$` (a group, or `$$`): it is expanded per
    /// request, and its placeholders found after.
    template: Option<Arc<str>>,
    /// `to` itself is on another host (starts with `//` or `/\`): the request's parts may
    /// follow it.
    off_site: bool,
    /// `to` starts with `$`: a URL with a scheme is refused.
    dollar: bool,
}

pub(crate) fn new(
    p: &Provision<'_, HandlerParams>,
) -> Result<Provisioned<dyn Middleware>, ProvisionError> {
    let HandlerParams::Redirect(r) = p.params() else {
        return Err(ProvisionError::new(
            "the redirect middleware was given another's params",
        ));
    };
    let regexp = if r.regexp.is_empty() {
        None
    } else {
        Some(Arc::new(
            akit_router::regexp(r.regexp.as_bytes())
                .map_err(|e| ProvisionError::field("regexp", e))?,
        ))
    };
    // With a regexp, fit expands `to` as Go's `Regexp.Expand` does; without a `$` that changes
    // nothing.
    let template = (regexp.is_some() && r.to.contains('$')).then(|| r.to.as_str().into());
    Ok(Provisioned::new(Arc::new(Redirect {
        status: r.status,
        parts: parts(&r.to).into(),
        regexp,
        template,
        off_site: other_host(&r.to),
        dollar: r.to.starts_with('$'),
    }) as Arc<dyn Middleware>))
}

/// `to` split into its text and its placeholders, as Go's `strings.Replacer` finds them, from
/// the left.
fn parts(to: &str) -> Vec<Part> {
    const NAMES: [(&str, Part); 5] = [
        ("{scheme}", Part::Scheme),
        ("{host}", Part::Host),
        ("{path}", Part::Path),
        ("{query}", Part::Query),
        ("{uri}", Part::Uri),
    ];
    let mut parts = Vec::new();
    let mut text = String::new();
    let mut rest = to;
    while let Some(c) = rest.chars().next() {
        if let Some((name, part)) = NAMES.iter().find(|(n, _)| rest.starts_with(n)) {
            if !text.is_empty() {
                parts.push(Part::Text(std::mem::take(&mut text).into()));
            }
            parts.push(part.clone());
            rest = rest.get(name.len()..).unwrap_or("");
        } else {
            text.push(c);
            rest = rest.get(c.len_utf8()..).unwrap_or("");
        }
    }
    if !text.is_empty() {
        parts.push(Part::Text(text.into()));
    }
    parts
}

impl Redirect {
    /// The URL `ctx`'s request is redirected to, its escaped path `path`.
    fn location(&self, ctx: &Context, path: &str) -> String {
        fill(&self.parts, ctx, path)
    }
}

/// `parts` with the request's placeholders filled in, its escaped path `path`.
fn fill(parts: &[Part], ctx: &Context, path: &str) -> String {
    let query = ctx.req().query();
    let mut s = String::new();
    for part in parts {
        match part {
            Part::Text(t) => s.push_str(t),
            Part::Scheme => s.push_str(if ctx.local(TLS).is_some() {
                "https"
            } else {
                "http"
            }),
            Part::Host => s.push_str(ctx.header("host").unwrap_or("")),
            Part::Path => s.push_str(path),
            Part::Query => s.push_str(query.unwrap_or("")),
            Part::Uri => {
                s.push_str(path);
                if let Some(q) = query.filter(|q| !q.is_empty()) {
                    s.push('?');
                    s.push_str(q);
                }
            }
        }
    }
    s
}

/// Whether a browser takes the URL `u` for one on another host with the same scheme: one that
/// starts with two slashes, either of which may be a `\`, which browsers take for `/`, after the
/// spaces and controls they skip (fit's `otherHost`).
pub(crate) fn other_host(u: &str) -> bool {
    let mut n = 0u8;
    for c in u.bytes() {
        if n == 2 {
            break;
        }
        match c {
            b'\t' | b'\n' | b'\r' => {}
            _ if n == 0 && c <= b' ' => {}
            b'/' | b'\\' => n = n.saturating_add(1),
            _ => return false,
        }
    }
    n == 2
}

/// Whether the URL `u` starts with a scheme: a `:` before any `/`, `?` or `#` (fit's
/// `hasScheme`).
fn has_scheme(u: &str) -> bool {
    for c in u.bytes() {
        match c {
            b':' => return true,
            b'/' | b'?' | b'#' => return false,
            _ => {}
        }
    }
    false
}

impl Middleware for Redirect {
    fn wrap(&self, next: Arc<dyn Handler>) -> Arc<dyn Handler> {
        Arc::new(Redirecting {
            rd: self.clone(),
            next,
        })
    }
}

/// A handler behind a redirect.
struct Redirecting {
    rd: Redirect,
    next: Arc<dyn Handler>,
}

impl akio_http::Service for Redirecting {
    fn call(&self, ctx: &mut Context) {
        let location = {
            let path = url::escaped_path(url::current_path(ctx));
            match (&self.rd.regexp, &self.rd.template) {
                // The groups are taken from the path as escaped, so that what the client wrote
                // encoded, as `%5C` or `%2F`, stays so in the URL.
                (Some(re), Some(template)) => {
                    let Some(caps) = re.captures(&path) else {
                        self.next.call(ctx);
                        return;
                    };
                    let mut to = String::new();
                    re.expand(&caps, template, &path, &mut to);
                    if to.contains('{') {
                        fill(&parts(&to), ctx, &path)
                    } else {
                        to
                    }
                }
                (Some(re), None) if !re.is_match(&path) => {
                    self.next.call(ctx);
                    return;
                }
                _ => self.rd.location(ctx, &path),
            }
        };
        if !self.rd.off_site && (other_host(&location) || self.rd.dollar && has_scheme(&location)) {
            // The request's parts made a URL on another host: an open redirect.
            ctx.respond(Response::text(400, BAD_TARGET));
            return;
        }
        let mut resp = Response::new(self.rd.status);
        resp.headers.set("Location", location);
        ctx.respond(resp);
    }

    fn body_mode(&self, req: &Request) -> BodyMode {
        self.next.body_mode(req)
    }

    fn streams_bodies(&self) -> bool {
        self.next.streams_bodies()
    }
}

impl Handler for Redirecting {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn other_hosts_as_fit_finds_them() {
        for (u, want) in [
            ("//a", true),
            ("/\\a", true),
            ("\\\\a", true),
            ("\\/a", true),
            (" //a", true),
            ("/\t/a", true),
            ("\x01/\n/a", true),
            ("/a", false),
            ("/", false),
            ("", false),
            ("a//b", false),
            ("/ /a", false),
            ("https://a", false),
        ] {
            assert_eq!(other_host(u), want, "{u:?}");
        }
    }

    #[test]
    fn schemes() {
        assert!(has_scheme("https:/evil.com"));
        assert!(has_scheme("a:b"));
        assert!(!has_scheme("/a:b"));
        assert!(!has_scheme("page"));
        assert!(!has_scheme("?x:y"));
    }

    #[test]
    fn placeholders_found_once() {
        assert_eq!(
            parts("https://{host}{uri}"),
            [Part::Text("https://".into()), Part::Host, Part::Uri]
        );
        assert_eq!(
            parts("{scheme}://new.example{path}#{query}"),
            [
                Part::Scheme,
                Part::Text("://new.example".into()),
                Part::Path,
                Part::Text("#".into()),
                Part::Query
            ]
        );
        assert_eq!(parts("/x{hostx}{"), [Part::Text("/x{hostx}{".into())]);
        assert_eq!(parts("é{path}"), [Part::Text("é".into()), Part::Path]);
        assert!(parts("").is_empty());
    }
}
