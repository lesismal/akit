//! The proxy's module of the pages (I109b, fit's `tunnel/proxy/api.go` routes): what its
//! `console` and `admin.console` serve at `/api/m/tunnel.proxy/`, and its own console at `/api/`
//! too.
//!
//! ```text
//! GET  /api/status            the proxy's state: each hub with its nodes and their shares, its
//!                             forwards, web proxies and tunnels
//! POST /api/hubs              a hub added (fit's HubReq)
//! POST /api/hubs/update       {name, hub, clear_body_password}: one changed
//! POST /api/hubs/delete       {name}: one removed
//! POST /api/nodes/refresh     {hub}: the hub's nodes asked for again
//! POST /api/nodes/services    {hub, node, services}: a node's services set through its hub
//! POST /api/meta              {key: value, ...}: the meta set
//! POST /api/forwards          {listen, node, service, hub}: a forward added
//! POST /api/forwards/delete   {listen}: one removed
//! POST /api/tunnels/close     {hub, id}: a tunnel closed
//! ```
//!
//! A change of the config is an edit of the proxy's member, answered once applied
//! ([`crate::pages::edit`]); a call on a hub is answered when the hub answers, within its
//! `call_timeout`.

// A refusal is the answer itself, made once per request: its size does not matter.
#![allow(clippy::result_large_err)]

use std::sync::Weak;

use akio_http::{Context, Method, Response};
use akit_config::value::Value;
use akit_roles::pages::{EditError, Module, decode, fail, json, member, no_endpoint, ok};

use super::ProxyCore;
use crate::msg;
use crate::pages::{self as common, HubReq};

/// The proxy's module.
pub(crate) struct ProxyPages(pub(crate) Weak<ProxyCore>);

const ID: &str = "tunnel.proxy";

fn log(user: &str, what: &str) {
    akit_observe::info!(Tunnel, "tunnel.proxy: console: {user:?}: {what}");
}

/// The `listen` of a forward as the member writes it.
fn listen_of(v: &Value) -> String {
    match v.get("listen") {
        Some(Value::String(s)) => s.clone(),
        _ => String::new(),
    }
}

/// The member's forwards, refused when they are not a list.
fn forwards_of(m: &akit_roles::pages::Member) -> Result<Vec<Value>, EditError> {
    match member(m, "forwards") {
        None | Some(Value::Null) => Ok(Vec::new()),
        Some(Value::Array(fs)) => Ok(fs.clone()),
        Some(_) => Err(EditError::bad(
            "forwards: the member's forwards are not a list of forwards",
        )),
    }
}

impl Module for ProxyPages {
    fn id(&self) -> &'static str {
        ID
    }

    #[allow(clippy::too_many_lines)]
    fn serve(&self, ctx: &mut Context, user: &str, path: &str) {
        let Some(c) = self.0.upgrade() else {
            return ctx.respond(fail(503, "the proxy stopped"));
        };
        let method = ctx.method().clone();
        if method == Method::Get && path == "/api/status" {
            return ctx.respond(json(200, c.status()));
        }
        if method != Method::Post {
            return ctx.respond(no_endpoint());
        }
        if !matches!(
            path,
            "/api/hubs"
                | "/api/hubs/update"
                | "/api/hubs/delete"
                | "/api/nodes/refresh"
                | "/api/nodes/services"
                | "/api/meta"
                | "/api/forwards"
                | "/api/forwards/delete"
                | "/api/tunnels/close"
        ) {
            return ctx.respond(no_endpoint());
        }
        let b = match decode(ctx) {
            Ok(b) => b,
            Err(r) => return ctx.respond(r),
        };
        let host = c.host.clone();
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
                let read = || -> Result<_, Response> {
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
            "/api/nodes/refresh" => {
                let hub = match b.object().and_then(|_| b.str("hub")) {
                    Ok(h) => h,
                    Err(r) => return ctx.respond(r),
                };
                let ups = c.uplinks();
                let Some(u) = common::uplink(&ups, &hub) else {
                    return ctx.respond(fail(404, "no such hub"));
                };
                let Some(d) = ctx.detach() else { return };
                c.refresh_nodes(
                    u,
                    Box::new(move |r| {
                        d.respond(match r {
                            Ok(list) => {
                                let nodes = msg::array(&list.nodes, |p| p.view(0.0));
                                json(
                                    200,
                                    msg::Obj::new()
                                        .str("hub", &list.hub)
                                        .raw("nodes", &nodes)
                                        .end(),
                                )
                            }
                            Err(e) => fail(502, &e),
                        });
                    }),
                );
            }
            "/api/nodes/services" => {
                let read = || -> Result<_, Response> {
                    b.object()?;
                    Ok((
                        b.str("hub")?,
                        b.str("node")?,
                        common::services_of(b.list("services")?)?,
                    ))
                };
                let (hub, node, services) = match read() {
                    Ok(x) => x,
                    Err(r) => return ctx.respond(r),
                };
                let ups = c.uplinks();
                let Some(u) = common::uplink(&ups, &hub) else {
                    return ctx.respond(fail(404, "no such hub"));
                };
                log(
                    user,
                    &format!("sets the services of node {node:?} through hub {hub:?}"),
                );
                let Some(d) = ctx.detach() else { return };
                c.set_node_services(
                    u,
                    &node,
                    &services,
                    Box::new(move |r| {
                        d.respond(match r {
                            Ok(()) => ok(),
                            Err(e) => fail(502, &e),
                        });
                    }),
                );
            }
            "/api/meta" => {
                let meta = match b.object().cloned() {
                    Ok(Value::Object(ms)) => ms,
                    Ok(Value::Null) => Vec::new(),
                    Ok(_) => return ctx.respond(fail(400, "the body is not the JSON expected")),
                    Err(r) => return ctx.respond(r),
                };
                if meta.iter().any(|(_, v)| !matches!(v, Value::String(_))) {
                    return ctx.respond(fail(
                        400,
                        "the body is not the JSON expected: the meta's values are strings",
                    ));
                }
                log(user, "changes the meta");
                common::edit(&host, ID, ctx, move |m| {
                    akit_roles::pages::set(m, "meta", Value::Object(meta));
                    Ok(())
                });
            }
            "/api/forwards" => {
                let read = || -> Result<_, Response> {
                    b.object()?;
                    Ok((
                        b.str("listen")?.trim().to_owned(),
                        b.str("node")?.trim().to_owned(),
                        b.str("service")?.trim().to_owned(),
                        b.str("hub")?.trim().to_owned(),
                    ))
                };
                let (listen, node, service, hub) = match read() {
                    Ok(x) => x,
                    Err(r) => return ctx.respond(r),
                };
                log(
                    user,
                    &format!("forwards {listen} to node {node:?} service {service:?}"),
                );
                let mut f = vec![
                    ("listen".to_owned(), Value::String(listen.clone())),
                    ("node".to_owned(), Value::String(node)),
                    ("service".to_owned(), Value::String(service)),
                ];
                if !hub.is_empty() {
                    f.push(("hub".to_owned(), Value::String(hub)));
                }
                common::edit(&host, ID, ctx, move |m| {
                    let mut fs = forwards_of(m)?;
                    if fs.iter().any(|x| listen_of(x) == listen) {
                        return Err(EditError::bad(format!("{listen} is forwarded already")));
                    }
                    fs.push(Value::Object(f));
                    akit_roles::pages::set(m, "forwards", Value::Array(fs));
                    Ok(())
                });
            }
            "/api/forwards/delete" => {
                let listen = match b.object().and_then(|_| b.str("listen")) {
                    Ok(l) => l,
                    Err(r) => return ctx.respond(r),
                };
                log(user, &format!("stops forwarding {listen}"));
                common::edit(&host, ID, ctx, move |m| {
                    let mut fs = forwards_of(m)?;
                    let Some(i) = fs.iter().position(|x| listen_of(x) == listen) else {
                        return Err(EditError::bad(format!("{listen} is not forwarded")));
                    };
                    fs.remove(i);
                    akit_roles::pages::set(m, "forwards", Value::Array(fs));
                    Ok(())
                });
            }
            _ => {
                let read = || -> Result<_, Response> {
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
                let ups = c.uplinks();
                let Some(u) = common::uplink(&ups, &hub) else {
                    return ctx.respond(fail(404, "no such hub"));
                };
                match c.table.get(id).filter(|e| u.owns(e.link())) {
                    Some(e) => {
                        e.close("closed from the proxy's console");
                        ctx.respond(ok());
                    }
                    None => ctx.respond(fail(404, "no such tunnel")),
                }
            }
        }
    }
}
