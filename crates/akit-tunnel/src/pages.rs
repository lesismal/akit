//! What the node's and the proxy's pages share (I109b, fit's `tunnel/internal/common/hubs.go`):
//! the hubs they link to, added, changed and removed as edits of their members, and the edits'
//! answers.

// A refusal is the answer itself, made once per request: its size does not matter.
#![allow(clippy::result_large_err)]

use std::sync::Arc;

use akio_http::{Context, Response};
use akit_config::roles::{DEFAULT_TUNNEL_WS_PATH, check_tunnel_endpoint};
use akit_config::value::{Value, write_string};
use akit_roles::host::Host;
use akit_roles::pages::{self, Body, EditError, Member, fail, json, member};

use crate::msg::{self, Service};
use crate::uplink::Uplink;

/// One of a hub's addresses, as the pages send it (fit's `EndpointReq`).
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub(crate) struct EndpointReq {
    pub protocol: String,
    pub address: String,
    pub enabled: bool,
}

impl EndpointReq {
    /// The endpoint's address with its scheme (`host:port` for tcp), `""` for none; one the
    /// config would refuse is refused here (fit's `EndpointReq.address`): one with a query, a
    /// fragment or a user among them, as nothing in an address says how the hub's certificate is
    /// checked.
    pub(crate) fn address(&self) -> Result<String, EditError> {
        let mut a = self.address.trim().to_owned();
        if a.is_empty() {
            return Ok(a);
        }
        if !a.contains("://") {
            match self.protocol.as_str() {
                "ws" | "wss" => {
                    if !a.contains('/') {
                        a.push_str(DEFAULT_TUNNEL_WS_PATH);
                    }
                    a = format!("{}://{a}", self.protocol);
                }
                "quic" | "tls" => a = format!("{}://{a}", self.protocol),
                _ => {}
            }
        }
        match check_tunnel_endpoint(&a) {
            Some(msg) => Err(EditError::bad(msg)),
            None => Ok(a),
        }
    }
}

/// A hub a node or a proxy links to, as its pages send it to add or change one (fit's
/// `HubReq`). A token or password left empty stays as it was; one given replaces the other.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub(crate) struct HubReq {
    pub name: String,
    pub endpoints: Vec<EndpointReq>,
    pub token: String,
    pub password: String,
    pub body_password: String,
    pub pool_size: u32,
    pub pin: String,
    pub quic_insecure: bool,
    /// `pin`'s old name, taken when `pin` is empty.
    pub quic_pin: String,
}

impl HubReq {
    pub(crate) fn of(b: &Body) -> Result<HubReq, Response> {
        b.object()?;
        let mut endpoints = Vec::new();
        for e in b.list("endpoints")? {
            e.object()?;
            endpoints.push(EndpointReq {
                protocol: e.str("protocol")?,
                address: e.str("address")?,
                enabled: e.bool("enabled")?,
            });
        }
        Ok(HubReq {
            name: b.str("name")?,
            endpoints,
            token: b.str("token")?,
            password: b.str("password")?,
            body_password: b.str("body_password")?,
            pool_size: b.u32("pool_size")?,
            pin: b.str("pin")?,
            quic_insecure: b.bool("quic_insecure")?,
            quic_pin: b.str("quic_pin")?,
        })
    }
}

/// A hub's name as the config has it (fit's `TunnelUplink.HubName`): its own, or its first
/// endpoint's address.
pub(crate) fn uplink_name(v: &Value) -> String {
    if let Some(Value::String(n)) = v.get("name") {
        if !n.is_empty() {
            return n.clone();
        }
    }
    match v.get("endpoints") {
        Some(Value::Array(eps)) => match eps.first().and_then(|e| e.get("address")) {
            Some(Value::String(a)) => a.clone(),
            _ => String::new(),
        },
        _ => String::new(),
    }
}

/// The member's hubs, refused when they are not a list (fit's `m.Get` failing).
fn hubs_of(m: &Member) -> Result<Vec<Value>, EditError> {
    match member(m, "hubs") {
        None | Some(Value::Null) => Ok(Vec::new()),
        Some(Value::Array(hs)) => Ok(hs.clone()),
        Some(_) => Err(EditError::bad(
            "hubs: the member's hubs are not a list of hubs",
        )),
    }
}

/// Changes the member's hubs (fit's `EditHub`): adds `req` when `name` is `""`, and otherwise
/// changes the one named `name` to `req`, keeping what `req` leaves out: its passwords when empty,
/// unless `clear_body`, and every setting the pages do not show.
pub(crate) fn edit_hub(
    m: &mut Member,
    name: &str,
    req: HubReq,
    clear_body: bool,
) -> Result<(), EditError> {
    let mut hubs = hubs_of(m)?;
    let i = if name.is_empty() {
        None
    } else {
        Some(
            hubs.iter()
                .position(|h| uplink_name(h) == name)
                .ok_or_else(|| EditError::bad(format!("no hub named {name}")))?,
        )
    };
    let mut u: Member = match i.and_then(|i| hubs.get(i)) {
        Some(Value::Object(o)) => o.clone(),
        Some(Value::Null) | None => Vec::new(),
        Some(_) => return Err(EditError::bad(format!("hub {name}: not an object"))),
    };
    let mut eps = Vec::new();
    for e in &req.endpoints {
        let a = e.address()?;
        if a.is_empty() {
            continue;
        }
        let mut ep = vec![("address".to_owned(), Value::String(a))];
        if !e.enabled {
            ep.push(("disabled".to_owned(), Value::Bool(true)));
        }
        eps.push(Value::Object(ep));
    }
    if eps.is_empty() {
        return Err(EditError::bad("a hub needs an address"));
    }
    pages::set(&mut u, "name", Value::String(req.name.trim().to_owned()));
    pages::set(&mut u, "endpoints", Value::Array(eps));
    // A token replaces the password, and the password a token.
    if !req.token.is_empty() {
        pages::set(&mut u, "token", Value::String(req.token.trim().to_owned()));
        u.retain(|(k, _)| k != "password");
    } else if !req.password.is_empty() {
        pages::set(&mut u, "password", Value::String(req.password));
        u.retain(|(k, _)| k != "token");
    }
    if !req.body_password.is_empty() {
        pages::set(&mut u, "body_password", Value::String(req.body_password));
    } else if clear_body {
        u.retain(|(k, _)| k != "body_password");
    }
    pages::set(
        &mut u,
        "pool_size",
        Value::Number(req.pool_size.to_string()),
    );
    // pin was quic_pin.
    u.retain(|(k, _)| k != "quic_pin");
    let pin = if req.pin.is_empty() {
        req.quic_pin
    } else {
        req.pin
    };
    pages::set(&mut u, "pin", Value::String(pin.trim().to_owned()));
    pages::set(&mut u, "quic_insecure", Value::Bool(req.quic_insecure));
    if member(&u, "token").is_none() && member(&u, "password").is_none() {
        return Err(EditError::bad(
            "a hub needs the token it issued for this member, or its password",
        ));
    }
    let u = Value::Object(u);
    let new_name = uplink_name(&u);
    if hubs
        .iter()
        .enumerate()
        .any(|(j, h)| Some(j) != i && uplink_name(h) == new_name)
    {
        return Err(EditError::bad(format!("a hub named {new_name} exists")));
    }
    match i.and_then(|i| hubs.get_mut(i)) {
        Some(slot) => *slot = u,
        None => hubs.push(u),
    }
    pages::set(m, "hubs", Value::Array(hubs));
    Ok(())
}

/// Removes the hub named `name` from the member's hubs (fit's `DeleteHub`).
pub(crate) fn delete_hub(m: &mut Member, name: &str) -> Result<(), EditError> {
    let mut hubs = hubs_of(m)?;
    let Some(i) = hubs.iter().position(|h| uplink_name(h) == name) else {
        return Err(EditError::bad(format!("no hub named {name}")));
    };
    hubs.remove(i);
    pages::set(m, "hubs", Value::Array(hubs));
    Ok(())
}

/// The services the pages send (fit's `ServiceView`s), trimmed.
pub(crate) fn services_of(list: Vec<Body>) -> Result<Vec<Service>, Response> {
    let mut out = Vec::with_capacity(list.len());
    for s in list {
        s.object()?;
        out.push(Service {
            name: s.str("name")?.trim().to_owned(),
            addr: s.str("address")?.trim().to_owned(),
            desc: s.str("description")?.trim().to_owned(),
        });
    }
    Ok(out)
}

/// Services as a member writes them (`name`, `address`, `description` when set).
pub(crate) fn services_value(list: &[Service]) -> Value {
    Value::Array(
        list.iter()
            .map(|s| {
                let mut o = vec![
                    ("name".to_owned(), Value::String(s.name.clone())),
                    ("address".to_owned(), Value::String(s.addr.clone())),
                ];
                if !s.desc.is_empty() {
                    o.push(("description".to_owned(), Value::String(s.desc.clone())));
                }
                Value::Object(o)
            })
            .collect(),
    )
}

/// A service as the pages show it (fit's `ServiceView`).
pub(crate) fn service_view(s: &Service) -> String {
    msg::Obj::new()
        .str("name", &s.name)
        .str("address", &s.addr)
        .str_omit("description", &s.desc)
        .end()
}

/// Changes role `id`'s member with `change`, as [`pages::edit`], and answers once applied:
/// `{"ok": true, "warnings": [...]}`, or 400 with why when the role's own apply failed (fit's
/// `common.Edit`, whose edit fails with the role's `Apply`), the config written all the same.
pub(crate) fn edit(
    host: &Host,
    id: &'static str,
    ctx: &mut Context,
    change: impl FnOnce(&mut Member) -> Result<(), EditError> + Send + 'static,
) {
    let prefix = format!("{id}: ");
    pages::edit(
        host,
        ctx,
        change,
        Some(Box::new(move |warnings: Vec<String>| {
            let own: Vec<&str> = warnings
                .iter()
                .filter_map(|w| w.strip_prefix(&prefix))
                .collect();
            if !own.is_empty() {
                return fail(400, &own.join("; "));
            }
            let mut b = String::from("{\"ok\":true,\"warnings\":[");
            for (i, w) in warnings.iter().enumerate() {
                if i > 0 {
                    b.push(',');
                }
                write_string(&mut b, w);
            }
            b.push_str("]}\n");
            json(200, b)
        })),
    );
}

/// The uplink named `name` among `ups`.
pub(crate) fn uplink<'u>(ups: &'u [Arc<Uplink>], name: &str) -> Option<&'u Arc<Uplink>> {
    ups.iter().find(|u| u.name == name)
}

/// The hub's name an end's link belongs to, among `ups`; `""` for none.
pub(crate) fn hub_of(ups: &[Arc<Uplink>], e: &crate::end::End) -> String {
    ups.iter()
        .find(|u| u.owns(e.link()))
        .map_or_else(String::new, |u| u.name.clone())
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::indexing_slicing)]
    use super::*;
    use akit_config::value::parse;

    fn ep(p: &str, a: &str) -> EndpointReq {
        EndpointReq {
            protocol: p.into(),
            address: a.into(),
            enabled: true,
        }
    }

    /// fit's `TestEndpointAddress`.
    #[test]
    fn endpoint_address() {
        for (e, want) in [
            (ep("ws", " "), ""),
            (ep("ws", "wss://h:1/x"), "wss://h:1/x"),
            (ep("ws", "h:1"), "ws://h:1/tunnel"),
            (ep("ws", "h:1/tun"), "ws://h:1/tun"),
            (ep("quic", "h:1"), "quic://h:1"),
            (ep("tls", "h:1"), "tls://h:1"),
            (ep("wss", "h:1"), "wss://h:1/tunnel"),
            (ep("wss", "h:1/tun"), "wss://h:1/tun"),
            (ep("tcp", " h:1 "), "h:1"),
        ] {
            assert_eq!(e.address().unwrap(), want, "{e:?}");
        }
        let pin = "0".repeat(64);
        for e in [
            ep("tls", &format!("tls://hub:7000#?pin={pin}&")),
            ep("tls", "hub:443#?x="),
            ep("quic", "hub:7000#?insecure=1&"),
            ep("wss", "hub:443/tunnel#?ca=/tmp/evil.pem&"),
            ep("wss", &format!("wss://hub:443/tunnel?pin={pin}")),
            ep("tls", "user@hub:7000"),
            ep("tcp", "hub:7000#x"),
            ep("tcp", "hub"),
        ] {
            let err = e.address().unwrap_err();
            assert_eq!(err.status, 400);
            assert!(
                err.msg.contains("is not") || err.msg.contains("pin or ca_file"),
                "{e:?}: {err}"
            );
        }
    }

    /// fit's `TestEditHubRefusesFragment`.
    #[test]
    fn edit_hub_refuses_a_fragment() {
        let mut m = Member::new();
        let req = HubReq {
            endpoints: vec![ep("tls", &format!("hub:7000#?pin={}&", "0".repeat(64)))],
            password: "pw".into(),
            pin: "1".repeat(64),
            ..HubReq::default()
        };
        let err = edit_hub(&mut m, "", req, false).unwrap_err();
        assert!(err.msg.contains("fragment"), "{err}");
        assert!(member(&m, "hubs").is_none());
    }

    fn hubs(m: &Member) -> Vec<Value> {
        match member(m, "hubs") {
            Some(Value::Array(h)) => h.clone(),
            _ => Vec::new(),
        }
    }

    fn s(v: &Value, k: &str) -> Option<String> {
        match v.get(k) {
            Some(Value::String(s)) => Some(s.clone()),
            Some(other) => Some(other.to_json().trim().to_owned()),
            None => None,
        }
    }

    /// fit's `TestEditHub`.
    #[test]
    fn edit_hubs() {
        let mut m = Member::new();
        let mut quic = ep("quic", "h:2");
        quic.enabled = false;
        let eps = vec![ep("tcp", "h:1"), quic];
        edit_hub(
            &mut m,
            "",
            HubReq {
                name: " a ".into(),
                endpoints: eps.clone(),
                password: "pw".into(),
                body_password: "bp".into(),
                pool_size: 2,
                pin: " pin ".into(),
                quic_insecure: true,
                ..HubReq::default()
            },
            false,
        )
        .unwrap();
        // A hub named by its first address, beside it.
        edit_hub(
            &mut m,
            "",
            HubReq {
                endpoints: vec![ep("tcp", "h:9")],
                password: "pw".into(),
                ..HubReq::default()
            },
            false,
        )
        .unwrap();
        let got = hubs(&m);
        assert_eq!(got.len(), 2);
        assert_eq!(s(&got[0], "name").as_deref(), Some("a"));
        assert_eq!(s(&got[0], "body_password").as_deref(), Some("bp"));
        assert_eq!(s(&got[0], "pin").as_deref(), Some("pin"));
        assert_eq!(s(&got[0], "quic_insecure").as_deref(), Some("true"));
        assert_eq!(s(&got[0], "pool_size").as_deref(), Some("2"));
        let Some(Value::Array(e)) = got[0].get("endpoints") else {
            panic!()
        };
        assert_eq!(e.len(), 2);
        assert_eq!(s(&e[1], "disabled").as_deref(), Some("true"));
        assert_eq!(s(&e[1], "address").as_deref(), Some("quic://h:2"));

        // A change keeps the passwords left empty and what the pages do not show; clear_body
        // drops the body password. A quic_pin, pin's old name, gives way to pin.
        let mut first = match &got[0] {
            Value::Object(o) => o.clone(),
            _ => panic!(),
        };
        first.push(("dial_timeout".into(), Value::String("3s".into())));
        first.push(("ca_file".into(), Value::String("/etc/ca.pem".into())));
        first.push(("quic_pin".into(), Value::String("old".into())));
        let mut all = got.clone();
        all[0] = Value::Object(first);
        pages::set(&mut m, "hubs", Value::Array(all));
        let one = vec![ep("tcp", "h:1")];
        edit_hub(
            &mut m,
            "a",
            HubReq {
                name: "a".into(),
                endpoints: one.clone(),
                ..HubReq::default()
            },
            false,
        )
        .unwrap();
        let got = hubs(&m);
        assert_eq!(s(&got[0], "password").as_deref(), Some("pw"));
        assert_eq!(s(&got[0], "body_password").as_deref(), Some("bp"));
        assert_eq!(s(&got[0], "dial_timeout").as_deref(), Some("3s"));
        assert_eq!(s(&got[0], "pool_size"), None);
        assert_eq!(s(&got[0], "ca_file").as_deref(), Some("/etc/ca.pem"));
        assert_eq!(s(&got[0], "quic_pin"), None);
        // The pages of an earlier release send the pin as quic_pin.
        edit_hub(
            &mut m,
            "a",
            HubReq {
                name: "a".into(),
                endpoints: one.clone(),
                quic_pin: "old".into(),
                ..HubReq::default()
            },
            false,
        )
        .unwrap();
        assert_eq!(s(&hubs(&m)[0], "pin").as_deref(), Some("old"));
        edit_hub(
            &mut m,
            "a",
            HubReq {
                name: "b".into(),
                endpoints: one.clone(),
                ..HubReq::default()
            },
            true,
        )
        .unwrap();
        let got = hubs(&m);
        assert_eq!(s(&got[0], "name").as_deref(), Some("b"));
        assert_eq!(s(&got[0], "body_password"), None);
        // A token replaces the password, and the password the token.
        edit_hub(
            &mut m,
            "b",
            HubReq {
                name: "b".into(),
                endpoints: one.clone(),
                token: " tok ".into(),
                ..HubReq::default()
            },
            false,
        )
        .unwrap();
        let got = hubs(&m);
        assert_eq!(s(&got[0], "token").as_deref(), Some("tok"));
        assert_eq!(s(&got[0], "password"), None);
        edit_hub(
            &mut m,
            "b",
            HubReq {
                name: "b".into(),
                endpoints: one,
                password: "pw2".into(),
                ..HubReq::default()
            },
            false,
        )
        .unwrap();
        let got = hubs(&m);
        assert_eq!(s(&got[0], "token"), None);
        assert_eq!(s(&got[0], "password").as_deref(), Some("pw2"));

        for (name, req, want) in [
            (
                "nope",
                HubReq {
                    endpoints: eps.clone(),
                    password: "pw".into(),
                    ..HubReq::default()
                },
                "no hub named nope",
            ),
            (
                "",
                HubReq {
                    endpoints: vec![ep("", " ")],
                    password: "pw".into(),
                    ..HubReq::default()
                },
                "needs an address",
            ),
            (
                "",
                HubReq {
                    endpoints: eps.clone(),
                    ..HubReq::default()
                },
                "needs the token it issued for this member, or its password",
            ),
            (
                "",
                HubReq {
                    name: "b".into(),
                    endpoints: eps.clone(),
                    password: "pw".into(),
                    ..HubReq::default()
                },
                "a hub named b exists",
            ),
            (
                "b",
                HubReq {
                    endpoints: vec![ep("", "h:9")],
                    ..HubReq::default()
                },
                "a hub named h:9 exists",
            ),
        ] {
            let err = edit_hub(&mut m, name, req.clone(), false).unwrap_err();
            assert_eq!(err.status, 400);
            assert!(err.msg.contains(want), "{name:?} {req:?}: {err}");
        }
        let mut bad: Member = vec![("hubs".into(), parse("{}").unwrap())];
        assert!(edit_hub(&mut bad, "", HubReq::default(), false).is_err());
    }

    /// fit's `TestDeleteHub`.
    #[test]
    fn delete_hubs() {
        let mut m: Member = vec![(
            "hubs".into(),
            parse(r#"[{"name":"a"},{"endpoints":[{"address":"h:1"}]}]"#).unwrap(),
        )];
        assert_eq!(delete_hub(&mut m, "nope").unwrap_err().status, 400);
        delete_hub(&mut m, "h:1").unwrap();
        let got = hubs(&m);
        assert_eq!(got.len(), 1);
        assert_eq!(s(&got[0], "name").as_deref(), Some("a"));
        delete_hub(&mut m, "a").unwrap();
        assert!(member(&m, "hubs").is_none(), "an empty list was kept");
        let mut bad: Member = vec![("hubs".into(), Value::Number("1".into()))];
        assert!(delete_hub(&mut bad, "a").is_err());
    }
}
