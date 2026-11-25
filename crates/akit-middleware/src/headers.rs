//! The `headers` middleware, as fit's `middleware/headers`: fields of the request deleted, set and
//! added before the handler sees it, and of the response just before it is sent, whichever
//! handler wrote it.
//!
//! Names are made canonical (`x-a` is `X-A`, as Go writes it) when the config is compiled. The
//! response's changes are made by a response filter (akio's
//! [`Context::add_response_filter`]), so they apply to every response of the request: one the
//! handler answers at once, later (a proxied one) or as a stream. The request's changes are made
//! on the request itself ([`Context::req_mut`], akio ADR-126): the handlers after it, the fields the
//! `proxy` handler sends upstream and what akio reads of the request (the `static` handler's
//! conditional and range fields) see them.

use std::sync::Arc;

use akio_http::{BodyMode, Context, Headers as Fields, Request, Response};
use akit_runtime::{Handler, HandlerParams, Middleware, Provision, ProvisionError, Provisioned};

use crate::url::canonical_key;

/// One side's changes, compiled: deletions first, then fields set, then fields added.
#[derive(Debug, Default)]
pub(crate) struct Ops {
    /// Canonical names.
    delete: Vec<String>,
    /// Prefixes (of deletions ending in `*`), canonical; compared ignoring case.
    prefixes: Vec<String>,
    set: Vec<(String, String)>,
    add: Vec<(String, Vec<String>)>,
}

impl Ops {
    fn compile(o: &akit_config::HeaderOps) -> Ops {
        let mut ops = Ops::default();
        for name in &o.delete {
            match name.strip_suffix('*') {
                Some(prefix) => ops.prefixes.push(canonical_key(prefix)),
                None => ops.delete.push(canonical_key(name)),
            }
        }
        ops.set = o
            .set
            .iter()
            .map(|(n, v)| (canonical_key(n), v.clone()))
            .collect();
        ops.add = o
            .add
            .iter()
            .map(|(n, vs)| (canonical_key(n), vs.clone()))
            .collect();
        ops
    }

    fn is_empty(&self) -> bool {
        self.delete.is_empty()
            && self.prefixes.is_empty()
            && self.set.is_empty()
            && self.add.is_empty()
    }

    /// Whether a field called `name` is deleted.
    fn deletes(&self, name: &str) -> bool {
        self.delete.iter().any(|d| d.eq_ignore_ascii_case(name))
            || self.prefixes.iter().any(|p| {
                name.get(..p.len())
                    .is_some_and(|head| head.eq_ignore_ascii_case(p))
            })
    }

    /// Makes the changes to `h`.
    pub(crate) fn apply(&self, h: &mut Fields) {
        if (!self.delete.is_empty() || !self.prefixes.is_empty())
            && h.iter().any(|(n, _)| self.deletes(n))
        {
            let mut kept = Fields::with_capacity(h.len(), 0);
            for (n, v) in h.iter().filter(|(n, _)| !self.deletes(n)) {
                kept.append(n, v);
            }
            *h = kept;
        }
        for (n, v) in &self.set {
            h.set(n, v);
        }
        for (n, vs) in &self.add {
            for v in vs {
                h.append(n, v);
            }
        }
    }
}

/// The middleware, compiled.
#[derive(Debug, Clone)]
pub(crate) struct Headers {
    request: Arc<Ops>,
    response: Arc<Ops>,
}

pub(crate) fn new(
    p: &Provision<'_, HandlerParams>,
) -> Result<Provisioned<dyn Middleware>, ProvisionError> {
    let HandlerParams::Headers(h) = p.params() else {
        return Err(ProvisionError::new(
            "the headers middleware was given another's params",
        ));
    };
    Ok(Provisioned::new(Arc::new(Headers {
        request: Arc::new(Ops::compile(&h.request)),
        response: Arc::new(Ops::compile(&h.response)),
    }) as Arc<dyn Middleware>))
}

impl Middleware for Headers {
    fn wrap(&self, next: Arc<dyn Handler>) -> Arc<dyn Handler> {
        Arc::new(Changed {
            request: (!self.request.is_empty()).then(|| Arc::clone(&self.request)),
            response: (!self.response.is_empty()).then(|| Arc::clone(&self.response)),
            next,
        })
    }
}

/// A handler behind the changes.
struct Changed {
    request: Option<Arc<Ops>>,
    response: Option<Arc<Ops>>,
    next: Arc<dyn Handler>,
}

impl akio_http::Service for Changed {
    fn call(&self, ctx: &mut Context) {
        if let Some(ops) = &self.request {
            if let Some(req) = ctx.req_mut() {
                ops.apply(&mut req.headers);
            }
        }
        if let Some(ops) = &self.response {
            let ops = Arc::clone(ops);
            ctx.add_response_filter(move |resp: &mut Response, _: &Request| {
                ops.apply(&mut resp.headers);
            });
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

impl Handler for Changed {}

#[cfg(test)]
mod tests {
    use super::*;

    fn fields(pairs: &[(&str, &str)]) -> Fields {
        let mut h = Fields::new();
        for (n, v) in pairs {
            h.append(n, v);
        }
        h
    }

    fn lines(h: &Fields) -> Vec<String> {
        h.iter().map(|(n, v)| format!("{n}: {v}")).collect()
    }

    #[test]
    fn delete_then_set_then_add() {
        let ops = Ops::compile(&akit_config::HeaderOps {
            set: vec![("x-a".into(), "1".into())],
            add: vec![("X-B".into(), vec!["2".into(), "3".into()])],
            delete: vec!["X-C".into(), "x-debug-*".into()],
        });
        let mut h = fields(&[
            ("X-A", "old"),
            ("x-a", "older"),
            ("X-B", "1"),
            ("x-c", "gone"),
            ("X-Debug-A", "1"),
            ("X-Debugger", "3"),
            ("X-D", "kept"),
        ]);
        ops.apply(&mut h);
        assert_eq!(
            lines(&h),
            [
                "X-B: 1",
                "X-Debugger: 3",
                "X-D: kept",
                "X-A: 1",
                "X-B: 2",
                "X-B: 3"
            ]
        );
        let all = Ops::compile(&akit_config::HeaderOps {
            delete: vec!["*".into()],
            ..Default::default()
        });
        all.apply(&mut h);
        assert!(h.is_empty());
    }
}
