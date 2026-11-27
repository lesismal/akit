//! The hub's module of the pages (I109, fit's `tunnel/hub/api.go` routes): what its `console` and
//! `admin.console` serve at `/api/m/tunnel.hub/`, and its own console at `/api/` too.
//!
//! ```text
//! GET  /api/status               the hub's state (HubApi::status)
//! *    /api/tokens...            its link tokens (akit_roles::pages::tokens)
//! POST /api/listeners            {protocol, address, path}: a listener added
//! POST /api/listeners/update     {old, new}: one changed, its certificate files kept
//! POST /api/listeners/delete     {protocol, address, path}: one removed
//! POST /api/settings             {password | clear_password, body_password |
//!                                clear_body_password, socket_address, socket_password |
//!                                clear_socket_password}
//! POST /api/peers/refresh        {id}: a node asked for its info
//! POST /api/peers/services       {id, services}: a node's services set
//! POST /api/peers/kick           {id}: a peer told why, and its links closed
//! POST /api/tunnels/close        {id}: a tunnel closed at both ends
//! ```
//!
//! A change of the config is an edit of the hub's member, answered once applied
//! ([`akit_roles::pages::edit`]); a call on a node is answered when the node answers, within
//! `call_timeout`.

// A refusal is the answer itself, made once per request: its size does not matter.
#![allow(clippy::result_large_err)]

use std::sync::{Arc, Weak};

use akio_http::{Context, Method, Response};
use akit_config::value::Value;
use akit_roles::pages::{
    self, Body, EditError, Member, Module, decode, fail, json, member, no_endpoint, ok,
};

use super::api::info_view;
use super::{HubApi, HubCore};
use crate::msg::{self, Service};

/// The hub's module.
pub(crate) struct HubPages(pub(crate) Weak<HubCore>);

fn log(user: &str, what: &str) {
    akit_observe::info!(Tunnel, "tunnel.hub: console: {user:?}: {what}");
}

/// A listener as the pages send it (fit's `listenerReq.listener`): trimmed, a ws or wss one's
/// default path left out, a path without its leading slash given one.
#[derive(Debug, Clone, PartialEq, Eq)]
struct ListenerReq {
    protocol: String,
    address: String,
    path: String,
}

fn is_ws(protocol: &str) -> bool {
    protocol == "ws" || protocol == "wss"
}

fn takes_tls(protocol: &str) -> bool {
    matches!(protocol, "tls" | "wss" | "quic")
}

impl ListenerReq {
    fn of(b: &Body) -> Result<ListenerReq, Response> {
        b.object()?;
        let protocol = b.str("protocol")?.trim().to_owned();
        let address = b.str("address")?.trim().to_owned();
        let mut path = b.str("path")?.trim().to_owned();
        if !is_ws(&protocol) || path == akit_config::roles::DEFAULT_TUNNEL_WS_PATH {
            path.clear();
        }
        if !path.is_empty() && !path.starts_with('/') {
            path.insert(0, '/');
        }
        Ok(ListenerReq {
            protocol,
            address,
            path,
        })
    }

    /// A listener of the member as written.
    fn from_value(v: &Value) -> ListenerReq {
        let s = |k: &str| match v.get(k) {
            Some(Value::String(s)) => s.clone(),
            _ => String::new(),
        };
        ListenerReq {
            protocol: s("protocol"),
            address: s("address"),
            path: s("path"),
        }
    }

    fn ws_path(&self) -> &str {
        if self.path.is_empty() {
            akit_config::roles::DEFAULT_TUNNEL_WS_PATH
        } else {
            &self.path
        }
    }

    /// What tells listeners apart (fit's `TunnelListener.Key`).
    fn key(&self) -> String {
        let mut k = format!("{} {}", self.protocol, self.address);
        if is_ws(&self.protocol) {
            k.push(' ');
            k.push_str(self.ws_path());
        }
        k
    }

    /// The listener as a URL (fit's `TunnelListener.URL`).
    fn url(&self) -> String {
        match self.protocol.as_str() {
            "tcp" => self.address.clone(),
            p if is_ws(p) => format!("{p}://{}{}", self.address, self.ws_path()),
            p => format!("{p}://{}", self.address),
        }
    }

    /// As the member writes it: `path` only when set, `cert_file` and `key_file` as given.
    fn value(&self, cert: Option<(Value, Value)>) -> Value {
        let mut o = vec![
            ("protocol".to_owned(), Value::String(self.protocol.clone())),
            ("address".to_owned(), Value::String(self.address.clone())),
        ];
        if !self.path.is_empty() {
            o.push(("path".to_owned(), Value::String(self.path.clone())));
        }
        if let Some((c, k)) = cert {
            for (name, v) in [("cert_file", c), ("key_file", k)] {
                if v != Value::Null && v != Value::String(String::new()) {
                    o.push((name.to_owned(), v));
                }
            }
        }
        Value::Object(o)
    }
}

/// The member's listeners, refused when they are not a list (fit's `m.Get` failing).
fn listeners_of(m: &Member) -> Result<Vec<Value>, EditError> {
    match member(m, "listeners") {
        None | Some(Value::Null) => Ok(Vec::new()),
        Some(Value::Array(ls)) if ls.iter().all(|l| matches!(l, Value::Object(_))) => {
            Ok(ls.clone())
        }
        Some(_) => Err(EditError::bad(
            "listeners: the member's listeners are not a list of listeners",
        )),
    }
}

/// Changes the member's listeners with `change` (fit's `editListeners`).
fn edit_listeners(
    m: &mut Member,
    change: impl FnOnce(Vec<Value>) -> Result<Vec<Value>, EditError>,
) -> Result<(), EditError> {
    let ls = change(listeners_of(m)?)?;
    pages::set(m, "listeners", Value::Array(ls));
    Ok(())
}

fn position(ls: &[Value], key: &str) -> Option<usize> {
    ls.iter()
        .position(|l| ListenerReq::from_value(l).key() == key)
}

/// What the settings form sent (fit's `settingsReq`).
#[derive(Debug, Clone, Default)]
struct SettingsReq {
    password: String,
    clear_password: bool,
    body_password: String,
    clear_body_password: bool,
    socket_password: String,
    clear_socket_password: bool,
    socket_address: String,
}

impl SettingsReq {
    fn of(b: &Body) -> Result<SettingsReq, Response> {
        b.object()?;
        Ok(SettingsReq {
            password: b.str("password")?,
            clear_password: b.bool("clear_password")?,
            body_password: b.str("body_password")?,
            clear_body_password: b.bool("clear_body_password")?,
            socket_password: b.str("socket_password")?,
            clear_socket_password: b.bool("clear_socket_password")?,
            socket_address: b.str("socket_address")?,
        })
    }
}

/// Applies the settings form to the member (fit's `editSettings`): a password left empty stays
/// as it was, unless its clear says to remove it; the console socket goes with an empty address.
fn edit_settings(m: &mut Member, req: SettingsReq) -> Result<(), EditError> {
    if !req.password.is_empty() {
        pages::set(m, "password", Value::String(req.password));
    } else if req.clear_password {
        m.retain(|(k, _)| k != "password");
    }
    if !req.body_password.is_empty() {
        pages::set(m, "body_password", Value::String(req.body_password));
    } else if req.clear_body_password {
        m.retain(|(k, _)| k != "body_password");
    }
    let addr = req.socket_address.trim();
    if addr.is_empty() {
        m.retain(|(k, _)| k != "console_socket");
        return Ok(());
    }
    let mut cs: Member = match member(m, "console_socket") {
        None | Some(Value::Null) => Vec::new(),
        Some(Value::Object(o)) => o.clone(),
        Some(_) => {
            return Err(EditError::bad(
                "console_socket: the member's console socket is not an object",
            ));
        }
    };
    pages::set(&mut cs, "address", Value::String(addr.to_owned()));
    if !req.socket_password.is_empty() {
        pages::set(&mut cs, "password", Value::String(req.socket_password));
    } else if req.clear_socket_password {
        cs.retain(|(k, _)| k != "password");
    }
    match m.iter_mut().find(|(k, _)| k == "console_socket") {
        Some((_, v)) => *v = Value::Object(cs),
        None => m.push(("console_socket".to_owned(), Value::Object(cs))),
    }
    Ok(())
}

impl HubPages {
    fn api(&self) -> Option<HubApi> {
        self.0.upgrade().map(HubApi)
    }

    /// The node `{"id"}` names (fit's `nodeParam`): 404 when there is none.
    fn node_param(api: &HubApi, b: &Body) -> Result<u32, Response> {
        b.object()?;
        let id = b.u32("id")?;
        if api.peer_kind(id).as_deref() == Some(msg::TYPE_NODE) {
            Ok(id)
        } else {
            Err(fail(404, "no such node"))
        }
    }

    fn post(&self, api: &HubApi, ctx: &mut Context, user: &str, path: &str) {
        let b = match decode(ctx) {
            Ok(b) => b,
            Err(r) => return ctx.respond(r),
        };
        let host = api.0.host.clone();
        match path {
            "/api/listeners" => {
                let l = match ListenerReq::of(&b) {
                    Ok(l) => l,
                    Err(r) => return ctx.respond(r),
                };
                log(user, &format!("adds listener {}", l.url()));
                pages::edit(
                    &host,
                    ctx,
                    move |m| {
                        edit_listeners(m, |mut ls| {
                            if position(&ls, &l.key()).is_some() {
                                return Err(EditError::bad(format!(
                                    "a listener on {} exists",
                                    l.url()
                                )));
                            }
                            ls.push(l.value(None));
                            Ok(ls)
                        })
                    },
                    None,
                );
            }
            "/api/listeners/update" => {
                let both = b
                    .object()
                    .and_then(|_| Ok((b.get("old")?, b.get("new")?)))
                    .and_then(|(o, n)| Ok((ListenerReq::of(&o)?, ListenerReq::of(&n)?)));
                let (old, l) = match both {
                    Ok(x) => x,
                    Err(r) => return ctx.respond(r),
                };
                log(
                    user,
                    &format!("changes listener {} to {}", old.url(), l.url()),
                );
                pages::edit(
                    &host,
                    ctx,
                    move |m| {
                        edit_listeners(m, |mut ls| {
                            let Some(i) = position(&ls, &old.key()) else {
                                return Err(EditError::bad(format!(
                                    "no listener on {}",
                                    old.url()
                                )));
                            };
                            if old.key() != l.key() && position(&ls, &l.key()).is_some() {
                                return Err(EditError::bad(format!(
                                    "a listener on {} exists",
                                    l.url()
                                )));
                            }
                            // The pages do not show a listener's certificate files: an edit
                            // keeps them where the protocol still takes them.
                            let cert = takes_tls(&l.protocol).then(|| {
                                let have = ls.get(i).cloned().unwrap_or(Value::Null);
                                (
                                    have.get("cert_file").cloned().unwrap_or(Value::Null),
                                    have.get("key_file").cloned().unwrap_or(Value::Null),
                                )
                            });
                            if let Some(slot) = ls.get_mut(i) {
                                *slot = l.value(cert);
                            }
                            Ok(ls)
                        })
                    },
                    None,
                );
            }
            "/api/listeners/delete" => {
                let l = match ListenerReq::of(&b) {
                    Ok(l) => l,
                    Err(r) => return ctx.respond(r),
                };
                log(user, &format!("removes listener {}", l.url()));
                pages::edit(
                    &host,
                    ctx,
                    move |m| {
                        edit_listeners(m, |mut ls| {
                            let Some(i) = position(&ls, &l.key()) else {
                                return Err(EditError::bad(format!("no listener on {}", l.url())));
                            };
                            ls.remove(i);
                            Ok(ls)
                        })
                    },
                    None,
                );
            }
            "/api/settings" => {
                let req = match SettingsReq::of(&b) {
                    Ok(r) => r,
                    Err(r) => return ctx.respond(r),
                };
                log(user, "changes the settings");
                pages::edit(&host, ctx, move |m| edit_settings(m, req), None);
            }
            "/api/peers/refresh" => {
                let id = match Self::node_param(api, &b) {
                    Ok(id) => id,
                    Err(r) => return ctx.respond(r),
                };
                let Some(d) = ctx.detach() else { return };
                api.refresh_node(id, Box::new(move |r| d.respond(info_answer(r))));
            }
            "/api/peers/services" => {
                let read = || -> Result<(u32, Vec<Service>), Response> {
                    b.object()?;
                    let id = b.u32("id")?;
                    let mut services = Vec::new();
                    for s in b.list("services")? {
                        s.object()?;
                        services.push(Service {
                            name: s.str("name")?.trim().to_owned(),
                            addr: s.str("address")?.trim().to_owned(),
                            desc: s.str("description")?.trim().to_owned(),
                        });
                    }
                    Ok((id, services))
                };
                let (id, services) = match read() {
                    Ok(x) => x,
                    Err(r) => return ctx.respond(r),
                };
                if api.peer_kind(id).as_deref() != Some(msg::TYPE_NODE) {
                    return ctx.respond(fail(404, "no such node"));
                }
                log(
                    user,
                    &format!(
                        "sets the services of node {:?}",
                        api.peer_name(id).unwrap_or_default()
                    ),
                );
                let Some(d) = ctx.detach() else { return };
                api.set_services(id, &services, Box::new(move |r| d.respond(info_answer(r))));
            }
            "/api/peers/kick" => {
                let id = match b.object().and_then(|_| b.u32("id")) {
                    Ok(id) => id,
                    Err(r) => return ctx.respond(r),
                };
                let (Some(kind), Some(name)) = (api.peer_kind(id), api.peer_name(id)) else {
                    return ctx.respond(fail(404, "no such peer"));
                };
                log(user, &format!("kicks {kind} {name:?}"));
                if api.kick(id, "kicked from the hub's console") {
                    ctx.respond(ok());
                } else {
                    ctx.respond(fail(404, "no such peer"));
                }
            }
            "/api/tunnels/close" => {
                let id = match b.object().and_then(|_| b.str("id")) {
                    Ok(id) => id,
                    Err(r) => return ctx.respond(r),
                };
                let Ok(id) = id.parse::<u64>() else {
                    return ctx.respond(fail(400, "the id is not a tunnel's"));
                };
                if api.close_tunnel(id, "closed from the hub's console") {
                    ctx.respond(ok());
                } else {
                    ctx.respond(fail(404, "no such tunnel"));
                }
            }
            _ => ctx.respond(no_endpoint()),
        }
    }
}

/// A node's info as the pages show it, or 502 with why the call failed.
fn info_answer(r: Result<msg::Info, String>) -> Response {
    match r {
        Ok(info) => json(200, info_view(&info)),
        Err(e) if e == "no such node" => fail(404, &e),
        Err(e) => fail(502, &e),
    }
}

impl Module for HubPages {
    fn id(&self) -> &'static str {
        "tunnel.hub"
    }

    fn serve(&self, ctx: &mut Context, user: &str, path: &str) {
        let Some(api) = self.api() else {
            return ctx.respond(fail(503, "the hub stopped"));
        };
        let method = ctx.method().clone();
        if method == Method::Get && path == "/api/status" {
            return ctx.respond(json(200, api.status()));
        }
        let c = Arc::clone(&api.0);
        let logged = user.to_owned();
        if pages::tokens(&c.host, &c.tokens, ctx, path, &move |what: String| {
            log(&logged, &what);
        }) {
            return;
        }
        if method == Method::Post {
            return self.post(&api, ctx, user, path);
        }
        ctx.respond(no_endpoint());
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]
    use super::*;

    fn req(p: &str, a: &str, path: &str) -> ListenerReq {
        let b = Body(
            akit_config::value::parse(&format!(
                r#"{{"protocol": {p:?}, "address": {a:?}, "path": {path:?}}}"#
            ))
            .unwrap(),
        );
        ListenerReq::of(&b).unwrap()
    }

    #[test]
    fn listener_keys_and_paths() {
        assert_eq!(req("ws", "h:1", "x").path, "/x");
        assert_eq!(req("ws", "h:1", "/tunnel").path, "");
        assert_eq!(req("tcp", "h:1", "/x").path, "");
        assert_eq!(req("ws", "h:1", "").key(), "ws h:1 /tunnel");
        assert_eq!(req("wss", "h:1", "").url(), "wss://h:1/tunnel");
        assert_eq!(req(" tcp ", " h:1 ", "").url(), "h:1");
        assert_eq!(req("quic", "h:1", "").url(), "quic://h:1");
    }

    #[test]
    fn settings_edits() {
        let mut m: Member = vec![("password".into(), Value::String("p".into()))];
        edit_settings(
            &mut m,
            SettingsReq {
                body_password: "b".into(),
                socket_address: "127.0.0.1:0".into(),
                socket_password: "sp".into(),
                ..SettingsReq::default()
            },
        )
        .unwrap();
        assert_eq!(
            member(&m, "body_password"),
            Some(&Value::String("b".into()))
        );
        let cs = member(&m, "console_socket").unwrap();
        assert_eq!(cs.get("password"), Some(&Value::String("sp".into())));
        edit_settings(
            &mut m,
            SettingsReq {
                clear_password: true,
                socket_address: "127.0.0.1:0".into(),
                clear_socket_password: true,
                ..SettingsReq::default()
            },
        )
        .unwrap();
        assert!(member(&m, "password").is_none());
        assert!(
            member(&m, "console_socket")
                .unwrap()
                .get("password")
                .is_none()
        );
        edit_settings(&mut m, SettingsReq::default()).unwrap();
        assert!(member(&m, "console_socket").is_none());
        let mut bad: Member = vec![("console_socket".into(), Value::Number("5".into()))];
        assert!(
            edit_settings(
                &mut bad,
                SettingsReq {
                    socket_address: "x:1".into(),
                    ..SettingsReq::default()
                }
            )
            .is_err()
        );
    }
}
