//! The handlers `whip` and `whep` (I113, fit's `webrtc/media/handler.go`), on the media server
//! of the process's engine (`webrtc.media`).
//!
//! A POST of an offer (`application/sdp`, at most `max_sdp_bytes`, read whole) publishes or plays the stream
//! named by `stream` or by the last segment of its path, answered `201` with the answer, the
//! session's URL in `Location` (the POST's path followed by the session's ID) and the ICE servers
//! in `Link` headers. The session's URL takes DELETE, without credentials: its ID, 128 random
//! bits, is the capability. PATCH (trickle ICE, ICE restarts) is answered 405, as fit's: akit is
//! an ICE lite agent whose answer holds all of its candidates.

use std::sync::Arc;

use akio_http::{Context, Method, Response};
use akit_runtime::{Handler, HandlerParams, Provision, ProvisionError, Provisioned};
use akit_upstream::EngineSlot;

use crate::media::{Kind, Refusal};

/// The `whip` or `whep` handler.
struct WhipWhep {
    kind: Kind,
    stream: String,
    engine: EngineSlot,
}

/// Whether `s` is a session's ID: 32 lowercase hex digits.
fn session_id(s: &str) -> bool {
    s.len() == 32
        && s.bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}

/// Answers with `status` and a plain text message.
fn respond(ctx: &mut Context, status: u16, msg: &str, headers: &[(&str, &str)]) {
    let mut resp = Response::new(status);
    for (n, v) in headers {
        resp.headers.set(n, v);
    }
    if !msg.is_empty() && status != 204 {
        resp.headers
            .set("Content-Type", "text/plain; charset=utf-8");
        resp.body = format!("{msg}\n").into_bytes();
    }
    ctx.respond(resp);
}

fn refused(ctx: &mut Context, e: &Refusal) {
    let retry = e.retry_after.to_string();
    if e.retry_after > 0 {
        respond(ctx, e.status, &e.msg, &[("Retry-After", &retry)]);
    } else {
        respond(ctx, e.status, &e.msg, &[]);
    }
}

const ALLOW_SESSION: (&str, &str) = ("Allow", "DELETE, OPTIONS");
const ALLOW_POST: (&str, &str) = ("Allow", "POST, OPTIONS");
const ACCEPT_POST: (&str, &str) = ("Accept-Post", "application/sdp");

impl akio_http::Service for WhipWhep {
    fn call(&self, ctx: &mut Context) {
        let path = ctx.path().trim_end_matches('/').to_owned();
        let last = path.rsplit('/').next().unwrap_or("").to_owned();
        let srv = self.engine.get().and_then(crate::media::lookup);
        let method = ctx.method().clone();
        if session_id(&last) && srv.as_ref().is_none_or(|s| s.has(self.kind, &last)) {
            match method {
                Method::Delete => match srv {
                    Some(s) if s.end_id(self.kind, &last) => respond(ctx, 200, "", &[]),
                    _ => respond(ctx, 404, "no such session", &[]),
                },
                Method::Options => respond(ctx, 204, "", &[ALLOW_SESSION]),
                _ => respond(
                    ctx,
                    405,
                    "a session's URL takes DELETE; PATCH (trickle ICE, ICE restarts) is not \
                     supported",
                    &[ALLOW_SESSION],
                ),
            }
            return;
        }
        let name = if self.stream.is_empty() {
            last.clone()
        } else {
            self.stream.clone()
        };
        match method {
            Method::Post => {}
            Method::Options => return respond(ctx, 204, "", &[ALLOW_POST, ACCEPT_POST]),
            _ if session_id(&last) => return respond(ctx, 404, "no such session", &[]),
            _ => return respond(ctx, 405, "the endpoint takes POST", &[ALLOW_POST]),
        }
        if !akit_config::webrtc::stream_name(&name) {
            return respond(ctx, 404, "no stream name in the path", &[]);
        }
        let sdp_type = ctx.header("Content-Type").is_some_and(|t| {
            t.split(';')
                .next()
                .is_some_and(|m| m.trim().eq_ignore_ascii_case("application/sdp"))
        });
        if !sdp_type {
            return respond(
                ctx,
                415,
                "the offer must be application/sdp",
                &[ACCEPT_POST],
            );
        }
        let Some(srv) = srv else {
            return respond(ctx, 503, "webrtc.media is not running", &[]);
        };
        let limit = srv.sdp_bytes();
        let declared = ctx
            .header("Content-Length")
            .and_then(|l| l.trim().parse::<u64>().ok());
        if declared.is_some_and(|n| n > u64::try_from(limit).unwrap_or(u64::MAX)) {
            return respond(ctx, 413, "the offer is too large", &[]);
        }
        // The offer is read whole (the route's buffered body, within the listener's bounds),
        // and refused past max_sdp_bytes.
        if ctx.req().body.len() > limit {
            return respond(ctx, 413, "the offer is too large", &[]);
        }
        let offer = String::from_utf8_lossy(&ctx.req().body).into_owned();
        let r = match self.kind {
            Kind::Publisher => srv.publish(&name, &offer),
            Kind::Viewer => srv.view(&name, &offer),
        };
        match r {
            Err(e) => refused(ctx, &e),
            Ok(a) => {
                let mut resp = Response::new(201);
                resp.headers.set("Content-Type", "application/sdp");
                resp.headers.set("Location", format!("{path}/{}", a.id));
                for l in srv.links(&a.id) {
                    resp.headers.append("Link", &l);
                }
                resp.body = a.sdp.into_bytes();
                ctx.respond(resp);
            }
        }
    }
}

impl Handler for WhipWhep {
    fn inline(&self) -> bool {
        true
    }
}

pub(crate) fn whip(
    p: &Provision<'_, HandlerParams>,
) -> Result<Provisioned<dyn Handler>, ProvisionError> {
    let HandlerParams::Whip(params) = p.params() else {
        return Err(ProvisionError::new(
            "the whip handler was given another's params",
        ));
    };
    Ok(Provisioned::new(Arc::new(WhipWhep {
        kind: Kind::Publisher,
        stream: params.stream.clone(),
        engine: p.env().engine().clone(),
    }) as Arc<dyn Handler>))
}

pub(crate) fn whep(
    p: &Provision<'_, HandlerParams>,
) -> Result<Provisioned<dyn Handler>, ProvisionError> {
    let HandlerParams::Whep(params) = p.params() else {
        return Err(ProvisionError::new(
            "the whep handler was given another's params",
        ));
    };
    Ok(Provisioned::new(Arc::new(WhipWhep {
        kind: Kind::Viewer,
        stream: params.stream.clone(),
        engine: p.env().engine().clone(),
    }) as Arc<dyn Handler>))
}
