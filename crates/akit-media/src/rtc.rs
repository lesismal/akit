//! The handlers `stream_whip` and `stream_whep` (I124), as fit's `rtcHandler`: WHIP (RFC 9725)
//! publishes into the streaming role's hub and WHEP plays from it, through akio-media's
//! `WhipServer` on the role's WebRTC port (`streaming.webrtc`, [`crate::streaming::Rtc`]).
//!
//! A POST of an offer is checked by `auth` ([`crate::http::guard`]), publish or play on the
//! stream's path, then handed to the server under its own path (`/whip/<stream>`,
//! `/whep/<stream>`); its answer's `Location`, the server's `/rtc/session/<id>`, is written back
//! as the POST's path followed by the ID, which the route must match too. A session's URL takes
//! PATCH (trickle ICE), DELETE and OPTIONS without credentials, its random ID of 32 hex digits
//! being the capability (RFC 9725). An OPTIONS of the endpoint reaches the server, which answers
//! the preflight. An offer that is empty or larger than `max_offer_bytes` is answered 400, as a
//! PATCH body larger than it. The handlers answer 503 while the role does not run, or runs
//! without `streaming.webrtc`.

use std::sync::{Arc, OnceLock};

use akio_http::{Context, Method, Service};
use akit_config::media::MediaAction as Action;
use akit_runtime::{
    Handler, HandlerParams, Provision, ProvisionError, Provisioned, set_request_path,
};

use crate::auth::registry;
use crate::handlers::valid_stream;
use crate::http::{Access, HandlerAuth, redact};
use crate::streaming::{RTC_RESOURCE, RTC_WHEP, RTC_WHIP, Slot, lookup};

/// What a handler finds once the engine is there.
struct Found {
    role: Arc<Slot>,
}

/// `stream_whip` or `stream_whep`.
struct RtcHandler {
    prefix: String,
    stream: String,
    action: Action,
    /// The server's path the POSTs go to: `/whip/` or `/whep/`.
    endpoint: &'static str,
    auth: HandlerAuth,
    found: Arc<OnceLock<Found>>,
}

/// Whether `id` is a session's: 32 hex digits, as the server makes them.
fn session_id(id: &str) -> bool {
    id.len() == 32 && id.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f'))
}

fn text(ctx: &mut Context, status: u16, msg: &str) {
    redact(ctx);
    ctx.respond(akio_http::Response::with_body(
        status,
        "text/plain; charset=utf-8",
        format!("{msg}\n").into_bytes(),
    ));
}

pub(crate) fn new_whip(
    p: &Provision<'_, HandlerParams>,
) -> Result<Provisioned<dyn Handler>, ProvisionError> {
    let HandlerParams::StreamWhip(h) = p.params() else {
        return Err(ProvisionError::new(
            "the stream_whip handler's params are not stream_whip's",
        ));
    };
    RtcHandler::provision(p, h, Action::Publish, RTC_WHIP)
}

pub(crate) fn new_whep(
    p: &Provision<'_, HandlerParams>,
) -> Result<Provisioned<dyn Handler>, ProvisionError> {
    let HandlerParams::StreamWhep(h) = p.params() else {
        return Err(ProvisionError::new(
            "the stream_whep handler's params are not stream_whep's",
        ));
    };
    RtcHandler::provision(p, h, Action::Play, RTC_WHEP)
}

impl RtcHandler {
    fn provision(
        p: &Provision<'_, HandlerParams>,
        h: &akit_config::media::StreamRtc,
        action: Action,
        endpoint: &'static str,
    ) -> Result<Provisioned<dyn Handler>, ProvisionError> {
        if !h.stream.is_empty() && !valid_stream(&h.stream) {
            return Err(ProvisionError::new(format!(
                "stream: {:?} is not a stream's path: segments of letters, digits, '.', '_', '~' and '-' joined by /",
                h.stream
            )));
        }
        let prefix = if h.prefix.is_empty() {
            endpoint.to_owned()
        } else {
            h.prefix.clone()
        };
        if !prefix.starts_with('/') {
            return Err(ProvisionError::new(format!(
                "prefix: {prefix:?} does not start with /"
            )));
        }
        let found = Arc::new(OnceLock::new());
        let f = Arc::clone(&found);
        let auth = HandlerAuth::new(p.env().engine(), &h.auth, h.policy.as_ref());
        p.env().engine().when_set(move |engine| {
            let _ = f.set(Found {
                role: lookup(registry::key(engine)),
            });
        });
        Ok(Provisioned::new(Arc::new(RtcHandler {
            prefix,
            stream: h.stream.clone(),
            action,
            endpoint,
            auth,
            found,
        }) as Arc<dyn Handler>))
    }
}

impl Service for RtcHandler {
    fn call(&self, ctx: &mut Context) {
        let p = ctx.path().to_owned();
        let rest = match p.strip_prefix(self.prefix.as_str()) {
            Some(r) => r.to_owned(),
            None if format!("{p}/") == self.prefix => String::new(),
            None => return text(ctx, 404, "not found"),
        };
        let Some(found) = self.found.get() else {
            return text(ctx, 503, "the streaming role is not running");
        };
        let Some(running) = found.role.with(|c| c.map(|c| c.rtc.clone())) else {
            return text(ctx, 503, "the streaming role is not running");
        };
        let Some(rtc) = running else {
            return text(ctx, 503, "streaming.webrtc is not set");
        };
        let rest = rest.trim_matches('/');
        let post = *ctx.method() == Method::Post;
        if !post {
            let id = rest.rsplit('/').next().unwrap_or("");
            if session_id(id) && (*ctx.method() != Method::Options || rest != self.stream) {
                if ctx.body().len() > rtc.max_offer_bytes {
                    return text(ctx, 400, "oversized fragment");
                }
                set_request_path(ctx, &format!("{RTC_RESOURCE}{id}"));
                return rtc.router.call(ctx);
            }
        }
        let stream = if self.stream.is_empty() {
            rest.to_owned()
        } else {
            self.stream.clone()
        };
        if !valid_stream(&stream) {
            return text(ctx, 404, "no stream in the path");
        }
        let target = format!("{}{stream}", self.endpoint);
        if !post {
            set_request_path(ctx, &target);
            return rtc.router.call(ctx);
        }
        let base = p.trim_end_matches('/').to_owned();
        self.auth
            .guard(ctx, self.action, &stream, "", move |ctx, _: Access| {
                let sdp = ctx
                    .header("content-type")
                    .is_some_and(|t| t.to_ascii_lowercase().starts_with("application/sdp"));
                let n = ctx.body().len();
                if sdp && (n == 0 || n > rtc.max_offer_bytes) {
                    return text(ctx, 400, "missing or oversized offer");
                }
                ctx.add_response_filter(move |resp, _| {
                    let loc = resp
                        .headers
                        .get("location")
                        .and_then(|l| l.strip_prefix(RTC_RESOURCE))
                        .map(|id| format!("{base}/{id}"));
                    if let Some(l) = loc {
                        resp.headers.set("Location", l);
                    }
                });
                set_request_path(ctx, &target);
                rtc.router.call(ctx);
            });
    }
}

impl Handler for RtcHandler {}
