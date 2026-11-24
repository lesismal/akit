//! The `rewrite` middleware, as fit's `middleware/rewrite` `Rewrite`: the request's path changed
//! before the handler sees it, the route having been picked by the path as it came.
//!
//! The path is changed where akit's handlers read it, [`CANONICAL_PATH`] (akio's request target
//! cannot be changed: akio API request A33): the `proxy` handler sends the new path upstream, the
//! `static` handler looks it up. As fit's, the rewrite works on the decoded path and writes the
//! new one escaped afresh (`/api/a%2Fb` stripped of `/api` is `/a/b`). fit's `regexp` and
//! `replacement` are refused as not supported yet when the config is read: akio-regex does not
//! find where a pattern matches yet (akio API request A32).

use std::sync::Arc;

use akio_http::{BodyMode, Context, Request};
use akit_runtime::{
    CANONICAL_PATH, Handler, HandlerParams, Middleware, Provision, ProvisionError, Provisioned,
};

use crate::url;

/// The rewrite, compiled.
#[derive(Debug, Clone)]
pub(crate) struct Rewrite {
    /// The prefixes to strip, each without a trailing `/`, in the order written.
    strip: Arc<[Box<[u8]>]>,
    /// The prefix to add; empty for none.
    add: Arc<[u8]>,
}

pub(crate) fn new(
    p: &Provision<'_, HandlerParams>,
) -> Result<Provisioned<dyn Middleware>, ProvisionError> {
    let HandlerParams::Rewrite(r) = p.params() else {
        return Err(ProvisionError::new(
            "the rewrite middleware was given another's params",
        ));
    };
    Ok(Provisioned::new(Arc::new(Rewrite {
        strip: r
            .strip_prefix
            .iter()
            .map(|s| s.strip_suffix('/').unwrap_or(s).as_bytes().into())
            .collect(),
        add: r.add_prefix.as_bytes().into(),
    }) as Arc<dyn Middleware>))
}

impl Rewrite {
    /// `p`, a decoded path, rewritten; `None` when it is left as it is.
    fn path(&self, p: &[u8]) -> Option<Vec<u8>> {
        let mut rest = p;
        for prefix in self.strip.iter() {
            if let Some(r) = cut_segments(p, prefix) {
                rest = r;
                break;
            }
        }
        let stripped = rest.len() != p.len();
        if !stripped && self.add.is_empty() {
            return None;
        }
        let mut out =
            Vec::with_capacity(self.add.len().saturating_add(rest.len()).saturating_add(1));
        out.extend_from_slice(&self.add);
        if rest.is_empty() {
            out.push(b'/');
        } else {
            out.extend_from_slice(rest);
        }
        (out != p).then_some(out)
    }
}

/// What is left of `p` without `prefix` (which has no trailing `/`), when `p` is `prefix` or goes
/// on with a `/` after it: fit's `cutSegments`. An empty rest stands for `/`.
fn cut_segments<'p>(p: &'p [u8], prefix: &[u8]) -> Option<&'p [u8]> {
    let rest = p.strip_prefix(prefix)?;
    (rest.is_empty() || rest.first() == Some(&b'/')).then_some(rest)
}

impl Middleware for Rewrite {
    fn wrap(&self, next: Arc<dyn Handler>) -> Arc<dyn Handler> {
        Arc::new(Rewritten {
            rw: self.clone(),
            next,
        })
    }
}

/// A handler behind a rewrite.
struct Rewritten {
    rw: Rewrite,
    next: Arc<dyn Handler>,
}

impl akio_http::Service for Rewritten {
    fn call(&self, ctx: &mut Context) {
        // The path the routes matched is canonical (akit-listener answers 400 to one that has
        // none), so decoding it again cannot fail; were it to, the path is left alone.
        let new = url::decode(url::current_path(ctx).as_bytes())
            .and_then(|p| self.rw.path(&p))
            .map(|p| url::escape_path(&p));
        if let Some(p) = new {
            ctx.set_local(CANONICAL_PATH, p);
        }
        self.next.call(ctx);
    }

    fn body_mode(&self, req: &Request) -> BodyMode {
        self.next.body_mode(req)
    }

    fn streams_bodies(&self) -> bool {
        self.next.streams_bodies()
    }
}

impl Handler for Rewritten {}

#[cfg(test)]
mod tests {
    use super::*;

    fn rw(strip: &[&str], add: &str) -> Rewrite {
        Rewrite {
            strip: strip
                .iter()
                .map(|s| s.strip_suffix('/').unwrap_or(s).as_bytes().into())
                .collect(),
            add: add.as_bytes().into(),
        }
    }

    fn apply(r: &Rewrite, p: &str) -> String {
        String::from_utf8(
            r.path(p.as_bytes())
                .unwrap_or_else(|| p.as_bytes().to_vec()),
        )
        .unwrap_or_default()
    }

    #[test]
    fn paths_as_fit_rewrites_them() {
        let r = rw(&["/api/v1", "/api"], "");
        for (p, want) in [
            ("/api/users", "/users"),
            ("/api/v1/users", "/users"),
            ("/api", "/"),
            ("/api/", "/"),
            ("/apix", "/apix"),
            ("/other/api", "/other/api"),
            ("/api/a/b", "/a/b"),
        ] {
            assert_eq!(apply(&r, p), want, "{p}");
        }
        let r = rw(&["/static/"], "");
        assert_eq!(apply(&r, "/static/css/a.css"), "/css/a.css");
        assert_eq!(apply(&r, "/static"), "/");
        let r = rw(&[], "/v2");
        assert_eq!(apply(&r, "/users"), "/v2/users");
        assert_eq!(apply(&r, "/"), "/v2/");
        let r = rw(&["/api"], "/internal");
        assert_eq!(apply(&r, "/api/v1/users"), "/internal/v1/users");
        assert_eq!(apply(&r, "/api"), "/internal/");
        // Unchanged: no new path.
        assert!(rw(&["/api"], "").path(b"/x").is_none());
        assert!(rw(&["/"], "").path(b"/x").is_none());
    }
}
