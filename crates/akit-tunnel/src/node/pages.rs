//! The node's module of the pages (I109b, fit's `tunnel/node/api.go` routes): what its `console`
//! and `admin.console` serve at `/api/m/tunnel.node/`, and its own console at `/api/` too.
//!
//! ```text
//! GET  /api/status          the node's state: its hubs' links, its services' round trips, its
//!                           tunnels
//! POST /api/hubs            a hub added (fit's HubReq)
//! POST /api/hubs/update     {name, hub, clear_body_password}: one changed
//! POST /api/hubs/delete     {name}: one removed
//! POST /api/services        [{name, address, description}]: the services set
//! POST /api/tunnels/close   {hub, id}: a tunnel closed
//! ```
//!
//! A change of the config is an edit of the node's member, answered once applied
//! ([`crate::pages::edit`]).

// A refusal is the answer itself, made once per request: its size does not matter.
#![allow(clippy::result_large_err)]

use std::sync::Weak;

use akio_http::{Context, Method};
use akit_roles::pages::{Module, decode, fail, json, no_endpoint, ok};

use super::NodeCore;
use crate::pages::{self as common, HubReq};

/// The node's module.
pub(crate) struct NodePages(pub(crate) Weak<NodeCore>);

const ID: &str = "tunnel.node";

fn log(user: &str, what: &str) {
    akit_observe::info!(Tunnel, "tunnel.node: console: {user:?}: {what}");
}

impl Module for NodePages {
    fn id(&self) -> &'static str {
        ID
    }

    fn serve(&self, ctx: &mut Context, user: &str, path: &str) {
        let Some(n) = self.0.upgrade() else {
            return ctx.respond(fail(503, "the node stopped"));
        };
        let method = ctx.method().clone();
        if method == Method::Get && path == "/api/status" {
            return ctx.respond(json(200, n.status()));
        }
        if method != Method::Post {
            return ctx.respond(no_endpoint());
        }
        if !matches!(
            path,
            "/api/hubs"
                | "/api/hubs/update"
                | "/api/hubs/delete"
                | "/api/services"
                | "/api/tunnels/close"
        ) {
            return ctx.respond(no_endpoint());
        }
        let b = match decode(ctx) {
            Ok(b) => b,
            Err(r) => return ctx.respond(r),
        };
        let host = n.host.clone();
        match path {
            "/api/hubs" => {
                let req = match HubReq::of(&b) {
                    Ok(r) => r,
                    Err(r) => return ctx.respond(r),
                };
                log(user, &format!("adds hub {:?}", req.name));
                common::edit(&host, ID, ctx, move |m| common::edit_hub(m, "", req, false));
            }
            "/api/hubs/update" => {
                let read = || -> Result<_, akio_http::Response> {
                    b.object()?;
                    Ok((
                        b.str("name")?,
                        HubReq::of(&b.get("hub")?)?,
                        b.bool("clear_body_password")?,
                    ))
                };
                let (name, req, clear) = match read() {
                    Ok(x) => x,
                    Err(r) => return ctx.respond(r),
                };
                log(user, &format!("changes hub {name:?}"));
                common::edit(&host, ID, ctx, move |m| {
                    common::edit_hub(m, &name, req, clear)
                });
            }
            "/api/hubs/delete" => {
                let name = match b.object().and_then(|_| b.str("name")) {
                    Ok(n) => n,
                    Err(r) => return ctx.respond(r),
                };
                log(user, &format!("removes hub {name:?}"));
                common::edit(&host, ID, ctx, move |m| common::delete_hub(m, &name));
            }
            "/api/services" => {
                let list = match b.list("").and_then(common::services_of) {
                    Ok(l) => l,
                    Err(r) => return ctx.respond(r),
                };
                log(user, "sets the services");
                let v = common::services_value(&list);
                common::edit(&host, ID, ctx, move |m| {
                    akit_roles::pages::set(m, "services", v);
                    Ok(())
                });
            }
            _ => {
                let read = || -> Result<_, akio_http::Response> {
                    b.object()?;
                    Ok((b.str("hub")?, b.str("id")?))
                };
                let (hub, id) = match read() {
                    Ok(x) => x,
                    Err(r) => return ctx.respond(r),
                };
                let Ok(id) = id.parse::<u64>() else {
                    return ctx.respond(fail(400, "the id is not a tunnel's"));
                };
                let ups = n.uplinks();
                let Some(u) = common::uplink(&ups, &hub) else {
                    return ctx.respond(fail(404, "no such hub"));
                };
                match n.table.get(id).filter(|e| u.owns(e.link())) {
                    Some(e) => {
                        e.close("closed from the node's console");
                        ctx.respond(ok());
                    }
                    None => ctx.respond(fail(404, "no such tunnel")),
                }
            }
        }
    }
}
