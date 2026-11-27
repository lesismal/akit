//! `bt_tracker` (I151, fit's `bittorrent/tracker/handler.go`): a route's handler that serves the
//! `bittorrent.tracker` role's HTTP announces and scrapes. It has no params: the route's match
//! decides where the tracker lives (`/announce`, `/tracker/*`). It finds the role of the engine it
//! serves on per request, through the role's board, and answers 503 while none runs there.

use std::sync::Arc;

use akio_http::{Context, Response};
use akit_runtime::upstream::EngineSlot;
use akit_runtime::{Handler, HandlerParams, Provision, ProvisionError, Provisioned};

use super::{key_of, with_snap};

/// The handler: the engine its route serves on.
struct BtTracker {
    engine: EngineSlot,
}

impl akio_http::Service for BtTracker {
    fn call(&self, ctx: &mut Context) {
        let key = self.engine.get().map(key_of);
        let peer = ctx.peer();
        let answer = key.and_then(|k| with_snap(k, |s| s.map(|s| s.http(peer, &ctx.req().target))));
        match answer {
            Some((status, ty, body)) => {
                let mut r = Response::new(status);
                r.headers.set("Content-Type", ty);
                r.body = body;
                ctx.respond(r);
            }
            None => {
                let mut r = Response::new(503);
                r.headers.set("Content-Type", "text/plain; charset=utf-8");
                r.body = b"the bittorrent.tracker role is not running\n".to_vec();
                ctx.respond(r);
            }
        }
    }
}

impl Handler for BtTracker {
    fn inline(&self) -> bool {
        true
    }
}

pub(crate) fn new(
    p: &Provision<'_, HandlerParams>,
) -> Result<Provisioned<dyn Handler>, ProvisionError> {
    if let HandlerParams::Module(m) = p.params() {
        if m.params.as_ref().is_some_and(|v| {
            !matches!(v, akit_config::value::Value::Null)
                && *v != akit_config::value::Value::Object(Vec::new())
        }) {
            return Err(ProvisionError::new("bt_tracker takes no params"));
        }
    }
    Ok(Provisioned::new(Arc::new(BtTracker {
        engine: p.env().engine().clone(),
    }) as Arc<dyn Handler>))
}

/// The handler on `engine`, as an akio-http service, for the tests' listener.
#[cfg(test)]
pub(crate) fn tests_handler(engine: EngineSlot) -> akit_runtime::HandlerService {
    akit_runtime::HandlerService(Arc::new(BtTracker { engine }))
}
