//! The handlers `whip`, `whep` and `webrtc_proxy` while their roles do not run (I110): each is
//! provisioned as fit's is (the proxy's upstream looked up), and answers what fit's answer
//! without their role, 503.

use std::sync::Arc;

use akio_http::{Context, Response};
use akit_runtime::{Handler, HandlerParams, Provision, ProvisionError, Provisioned};

/// A handler whose role does not run: every request answered 503 with why.
struct NotRunning(&'static str);

impl akio_http::Service for NotRunning {
    fn call(&self, ctx: &mut Context) {
        let mut resp = Response::new(503);
        resp.headers
            .set("Content-Type", "text/plain; charset=utf-8");
        resp.body = format!("{} is not running\n", self.0).into_bytes();
        ctx.respond(resp);
    }
}

impl Handler for NotRunning {
    fn inline(&self) -> bool {
        true
    }
}

fn not_running(role: &'static str) -> Provisioned<dyn Handler> {
    Provisioned::new(Arc::new(NotRunning(role)) as Arc<dyn Handler>)
}

pub(crate) fn whip(
    p: &Provision<'_, HandlerParams>,
) -> Result<Provisioned<dyn Handler>, ProvisionError> {
    match p.params() {
        HandlerParams::Whip(_) => Ok(not_running("webrtc.media")),
        _ => Err(ProvisionError::new(
            "the whip handler was given another's params",
        )),
    }
}

pub(crate) fn whep(
    p: &Provision<'_, HandlerParams>,
) -> Result<Provisioned<dyn Handler>, ProvisionError> {
    match p.params() {
        HandlerParams::Whep(_) => Ok(not_running("webrtc.media")),
        _ => Err(ProvisionError::new(
            "the whep handler was given another's params",
        )),
    }
}

pub(crate) fn webrtc_proxy(
    p: &Provision<'_, HandlerParams>,
) -> Result<Provisioned<dyn Handler>, ProvisionError> {
    let HandlerParams::WebRtcProxy(params) = p.params() else {
        return Err(ProvisionError::new(
            "the webrtc_proxy handler was given another's params",
        ));
    };
    if p.env().upstream(&params.upstream).is_none() {
        return Err(ProvisionError::field(
            "upstream",
            format!("there is no upstream named {:?}", params.upstream),
        ));
    }
    Ok(not_running("webrtc.proxy"))
}
