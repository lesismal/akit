//! The roles' pages (I109, fit's `console.Module` and `tunnel/internal/common`): what a role
//! gives a console to serve its pages' API, and the helpers its endpoints share.
//!
//! A role that has pages gives a [`Module`] ([`Role::module`](crate::host::Role::module)). Its own
//! `console` serves it at `/api/` and at `/api/m/<id>/`, and `admin.console` beside the config's
//! own module at `/api/m/<id>/`, behind the console's logins and CSRF token, as fit's consoles
//! serve a role's module. The console checks the session; the module answers the request.
//!
//! An endpoint that changes the config edits the role's member ([`edit`], fit's `common.Edit`):
//! the change is applied by the control thread as any config, with the role's `apply` among it,
//! and the page is answered once it was, `{"ok": true, "warnings": [...]}`, or 400 with why the
//! config did not load.

// A refusal is the answer itself, made once per request: its size does not matter.
#![allow(clippy::result_large_err)]

use std::fmt;
use std::sync::{Arc, Mutex, PoisonError};

use akio_http::{Context, Method, Response};
use akit_config::value::{self, Value, write_string};

use crate::host::Host;

/// A role's module of the pages: its API, served under `/api/m/<id>/`, and at `/api/` on the
/// role's own console.
pub trait Module: Send + Sync {
    /// The module's ID, the role's member: `tunnel.hub`.
    fn id(&self) -> &'static str;

    /// Answers `ctx`, a request of `user`, whose session and CSRF token the console checked, at
    /// `path`, the endpoint as the module adds it (`/api/status`, without `/api/m/<id>`). On the
    /// console's I/O thread: it must not block; an answer that waits is given through
    /// `ctx.detach()`. 404 for a path it does not serve ([`no_endpoint`]).
    fn serve(&self, ctx: &mut Context, user: &str, path: &str);

    /// Where the role's own console serves the module, as a URL (`http://127.0.0.1:9100/`), once
    /// it started, and `None` once it stopped (fit's `Console.URL`): told by the console server,
    /// which knows a port chosen when it starts. Nothing by default.
    fn console_url(&self, _url: Option<&str>) {}
}

/// A module, shared.
pub type ModuleRef = Arc<dyn Module>;

/// The modules of the running roles, in the order of their IDs, which `admin.console` serves
/// beside the config's own; set by the roles' manager on every config applied, read by the
/// console for each request. Cheap to clone.
#[derive(Clone, Default)]
pub struct Modules(Arc<Mutex<Vec<ModuleRef>>>);

impl fmt::Debug for Modules {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_list().entries(self.ids()).finish()
    }
}

impl Modules {
    /// The modules now.
    #[must_use]
    pub fn list(&self) -> Vec<ModuleRef> {
        self.0
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
    }

    /// Their IDs.
    #[must_use]
    pub fn ids(&self) -> Vec<&'static str> {
        self.list().iter().map(|m| m.id()).collect()
    }

    /// The module `id`, if it runs.
    #[must_use]
    pub fn get(&self, id: &str) -> Option<ModuleRef> {
        self.list().into_iter().find(|m| m.id() == id)
    }

    /// Replaces them.
    pub fn set(&self, ms: Vec<ModuleRef>) {
        *self.0.lock().unwrap_or_else(PoisonError::into_inner) = ms;
    }
}

/// A JSON answer.
#[must_use]
pub fn json(status: u16, body: String) -> Response {
    Response::with_body(status, "application/json", body.into_bytes())
}

/// `{"error": msg}`, fit's `common.Fail`.
#[must_use]
pub fn fail(status: u16, msg: &str) -> Response {
    let mut b = String::from("{\"error\":");
    write_string(&mut b, msg);
    b.push_str("}\n");
    json(status, b)
}

/// `{"ok": true}`, fit's `common.OK`.
#[must_use]
pub fn ok() -> Response {
    json(200, "{\"ok\":true}\n".to_owned())
}

/// What a module answers for a path it does not serve: fit's `no such endpoint`.
#[must_use]
pub fn no_endpoint() -> Response {
    fail(404, "no such endpoint")
}

/// A string as JSON.
#[must_use]
pub fn quoted(s: &str) -> String {
    let mut b = String::new();
    write_string(&mut b, s);
    b
}

/// A request's JSON body, fit's `Request.Decode`: 415 unless it was sent as
/// `application/json`, 400 when it is not JSON.
///
/// # Errors
///
/// The answer that refuses it.
pub fn decode(ctx: &Context) -> Result<Body, Response> {
    let json_body = ctx.header("Content-Type").is_some_and(|t| {
        t.split(';')
            .next()
            .is_some_and(|m| m.trim().eq_ignore_ascii_case("application/json"))
    });
    if !json_body {
        return Err(fail(415, "the body must be sent as application/json"));
    }
    let text = std::str::from_utf8(ctx.body()).map_err(|_| not_json("not UTF-8"))?;
    match value::parse(text) {
        Ok(v) => Ok(Body(v)),
        Err(e) => Err(not_json(
            &e.as_slice()
                .first()
                .map_or_else(String::new, |e| e.msg.clone()),
        )),
    }
}

fn not_json(why: &str) -> Response {
    fail(400, &format!("the body is not the JSON expected: {why}"))
}

/// A request's JSON body, read as fit's handlers decode it into their types: a member left out
/// is its type's zero value, one of another type refuses the request (400).
#[derive(Debug, Clone, PartialEq)]
pub struct Body(pub Value);

impl Body {
    fn member(&self, key: &str) -> Result<Option<&Value>, Response> {
        match &self.0 {
            Value::Object(_) => Ok(self.0.get(key).filter(|v| **v != Value::Null)),
            Value::Null => Ok(None),
            _ => Err(not_json("not an object")),
        }
    }

    /// The object itself, or a refusal when the body is not one.
    ///
    /// # Errors
    ///
    /// The body is not an object.
    pub fn object(&self) -> Result<&Value, Response> {
        self.member("")?;
        Ok(&self.0)
    }

    /// String member `key`, `""` when left out.
    ///
    /// # Errors
    ///
    /// It is not a string.
    pub fn str(&self, key: &str) -> Result<String, Response> {
        str_of(self.member(key)?, key)
    }

    /// Boolean member `key`, false when left out.
    ///
    /// # Errors
    ///
    /// It is not a boolean.
    pub fn bool(&self, key: &str) -> Result<bool, Response> {
        match self.member(key)? {
            None => Ok(false),
            Some(Value::Bool(b)) => Ok(*b),
            Some(_) => Err(not_json(&format!("{key} is not a boolean"))),
        }
    }

    /// Unsigned 32-bit member `key`, 0 when left out.
    ///
    /// # Errors
    ///
    /// It is not such a number.
    pub fn u32(&self, key: &str) -> Result<u32, Response> {
        match self.member(key)? {
            None => Ok(0),
            Some(Value::Number(n)) => n
                .parse()
                .map_err(|_| not_json(&format!("{key} is not an unsigned 32-bit integer"))),
            Some(_) => Err(not_json(&format!("{key} is not a number"))),
        }
    }

    /// Integer member `key`, 0 when left out.
    ///
    /// # Errors
    ///
    /// It is not a 64-bit integer.
    pub fn i64(&self, key: &str) -> Result<i64, Response> {
        match self.member(key)? {
            None => Ok(0),
            Some(Value::Number(n)) => n
                .parse()
                .map_err(|_| not_json(&format!("{key} is not an integer"))),
            Some(_) => Err(not_json(&format!("{key} is not a number"))),
        }
    }

    /// Member `key` as a body of its own (`null` when left out).
    ///
    /// # Errors
    ///
    /// The body is not an object.
    pub fn get(&self, key: &str) -> Result<Body, Response> {
        Ok(Body(self.member(key)?.cloned().unwrap_or(Value::Null)))
    }

    /// Array member `key`, or the body itself with `""`; empty when left out.
    ///
    /// # Errors
    ///
    /// It is not an array.
    pub fn list(&self, key: &str) -> Result<Vec<Body>, Response> {
        let v = if key.is_empty() {
            Some(&self.0).filter(|v| **v != Value::Null)
        } else {
            self.member(key)?
        };
        match v {
            None => Ok(Vec::new()),
            Some(Value::Array(items)) => Ok(items.iter().cloned().map(Body).collect()),
            Some(_) => Err(not_json(&format!(
                "{} is not an array",
                if key.is_empty() { "the body" } else { key }
            ))),
        }
    }
}

fn str_of(v: Option<&Value>, key: &str) -> Result<String, Response> {
    match v {
        None => Ok(String::new()),
        Some(Value::String(s)) => Ok(s.clone()),
        Some(_) => Err(not_json(&format!("{key} is not a string"))),
    }
}

/// Why an edit refused what the page asked, with the status it is answered with: 400 (fit's
/// `common.BadRequest`), or 404 for what is not there.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EditError {
    /// The answer's status.
    pub status: u16,
    /// Why.
    pub msg: String,
}

impl EditError {
    /// A refusal of what the page asked: 400.
    #[must_use]
    pub fn bad(msg: impl Into<String>) -> EditError {
        EditError {
            status: 400,
            msg: msg.into(),
        }
    }

    /// What the page named is not there: 404.
    #[must_use]
    pub fn not_found(msg: impl Into<String>) -> EditError {
        EditError {
            status: 404,
            msg: msg.into(),
        }
    }
}

impl fmt::Display for EditError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.msg)
    }
}

/// A role's member, as an edit changes it: its members by name, as they are written, so that
/// what an edit leaves alone stays as it was (fit's `common.Member`).
pub type Member = Vec<(String, Value)>;

/// Member `key` of `m`.
#[must_use]
pub fn member<'m>(m: &'m Member, key: &str) -> Option<&'m Value> {
    m.iter().find(|(k, _)| k == key).map(|(_, v)| v)
}

/// Sets member `key` of `m` to `v`, where it was, or at the end; a `null`, an empty string,
/// array or object, `false` or `0` leaves it out, as fit's `Member.Set`.
pub fn set(m: &mut Member, key: &str, v: Value) {
    let zero = match &v {
        Value::Null => true,
        Value::Bool(b) => !b,
        Value::Number(n) => n.parse::<f64>().is_ok_and(|f| f == 0.0),
        Value::String(s) => s.is_empty(),
        Value::Array(a) => a.is_empty(),
        Value::Object(o) => o.is_empty(),
    };
    if zero {
        m.retain(|(k, _)| k != key);
        return;
    }
    match m.iter_mut().find(|(k, _)| k == key) {
        Some((_, old)) => *old = v,
        None => m.push((key.to_owned(), v)),
    }
}

/// The message a role's edit is refused with when the control thread has too many waiting:
/// answered 503, as fit's `app.ErrBusy`.
pub const BUSY: &str = "too many config changes are waiting; try again";

/// Changes the role's member with `change` and answers `ctx` once the config with the change was
/// applied, the role's `apply` among it (fit's `common.Edit`): with `then`'s answer, given the
/// warnings, or `{"ok": true, "warnings": [...]}` when `then` is `None`. A change that fails,
/// or a config that does not load, is answered 400 with why; 503 when the control thread is
/// busy. Returns at once.
pub fn edit(
    host: &Host,
    ctx: &mut Context,
    change: impl FnOnce(&mut Member) -> Result<(), EditError> + Send + 'static,
    then: Option<Box<dyn FnOnce(Vec<String>) -> Response + Send>>,
) {
    let Some(d) = ctx.detach() else { return };
    let own: Arc<Mutex<Option<EditError>>> = Arc::default();
    let own2 = Arc::clone(&own);
    host.edit(
        Box::new(move |m: &mut Value| {
            if *m == Value::Null {
                *m = Value::Object(Vec::new());
            }
            let Value::Object(ms) = m else {
                return Err("the member is not an object".to_owned());
            };
            change(ms).map_err(|e| {
                let msg = e.msg.clone();
                *own2.lock().unwrap_or_else(PoisonError::into_inner) = Some(e);
                msg
            })
        }),
        Box::new(move |r| {
            d.respond(match r {
                Ok(warnings) => match then {
                    Some(f) => f(warnings),
                    None => {
                        let mut b = String::from("{\"ok\":true,\"warnings\":[");
                        for (i, w) in warnings.iter().enumerate() {
                            if i > 0 {
                                b.push(',');
                            }
                            write_string(&mut b, w);
                        }
                        b.push_str("]}\n");
                        json(200, b)
                    }
                },
                Err(msg) if msg == BUSY => fail(503, &msg),
                Err(msg) => match own.lock().unwrap_or_else(PoisonError::into_inner).take() {
                    Some(e) => fail(e.status, &e.msg),
                    None => fail(400, &msg),
                },
            });
        }),
    );
}

/// Answers the endpoints of a listener's link tokens on a role's pages, fit's
/// `common.TokenRoutes`, when `path` is one of them; false otherwise:
///
/// ```text
/// GET  /api/tokens          {"tokens": [...]}: the registry's views
/// POST /api/tokens          {name, note, expires_at | ttl}: one issued, answered with the
///                           token, shown this once
/// POST /api/tokens/expiry   {id, expires_at | ttl}: its expiry changed; neither, never
/// POST /api/tokens/revoke   {id}: revoked, its links closed at once
/// ```
///
/// A change is an edit of the member's `tokens` ([`edit`]), logged with `log`.
pub fn tokens(
    host: &Host,
    registry: &crate::linktoken::Registry,
    ctx: &mut Context,
    path: &str,
    log: &(dyn Fn(String) + Sync),
) -> bool {
    use crate::linktoken::{self as lt, ExpiryReq, IssueReq, TokenError};
    let method = ctx.method().clone();
    let get = matches!(method, Method::Get | Method::Head);
    match (get, path) {
        (true, "/api/tokens") => {
            ctx.respond(json(
                200,
                format!("{{\"tokens\":{}}}\n", lt::list_json(&registry.list())),
            ));
            return true;
        }
        (false, "/api/tokens" | "/api/tokens/expiry" | "/api/tokens/revoke")
            if method == Method::Post => {}
        _ => return false,
    }
    let b = match decode(ctx) {
        Ok(b) => b,
        Err(r) => {
            ctx.respond(r);
            return true;
        }
    };
    let read = || -> Result<(String, IssueReq), Response> {
        b.object()?;
        Ok((
            b.str("id")?,
            IssueReq {
                name: b.str("name")?,
                note: b.str("note")?,
                expires_at: b.i64("expires_at")?,
                ttl: b.str("ttl")?,
            },
        ))
    };
    let (id, req) = match read() {
        Ok(r) => r,
        Err(r) => {
            ctx.respond(r);
            return true;
        }
    };
    let refused = |e: &TokenError| match e {
        TokenError::NotFound => EditError::not_found(e.to_string()),
        _ => EditError::bad(e.to_string()),
    };
    let answer: Arc<Mutex<String>> = Arc::default();
    let out = Arc::clone(&answer);
    let now = lt::unix_now();
    let what = path.to_owned();
    match path {
        "/api/tokens" => log(format!("issues a link token named {:?}", req.name)),
        "/api/tokens/expiry" => log(format!("changes the expiry of link token {id}")),
        _ => log(format!("revokes link token {id}")),
    }
    let change = move |m: &mut Member| -> Result<(), EditError> {
        let mut toks = lt::tokens_of(&Value::Object(m.clone()));
        let body = match what.as_str() {
            "/api/tokens" => {
                let is = lt::issue(&mut toks, &req, now).map_err(|e| refused(&e))?;
                let view = lt::record_view(&is.record, now).to_json();
                format!(
                    "{{\"token\":{},{}",
                    quoted(&is.secret.token()),
                    view.get(1..).unwrap_or("}")
                )
            }
            "/api/tokens/expiry" => {
                let er = ExpiryReq {
                    expires_at: req.expires_at,
                    ttl: req.ttl.clone(),
                };
                lt::set_expiry(&mut toks, &id, &er, now).map_err(|e| refused(&e))?;
                toks.iter()
                    .find(|t| t.id == id)
                    .map_or_else(String::new, |t| lt::record_view(t, now).to_json())
            }
            _ => {
                lt::revoke(&mut toks, &id).map_err(|e| refused(&e))?;
                "{\"ok\":true}".to_owned()
            }
        };
        *out.lock().unwrap_or_else(PoisonError::into_inner) = body;
        let v = lt::tokens_value(&toks);
        match m.iter_mut().find(|(k, _)| k == "tokens") {
            Some((_, old)) if !toks.is_empty() => *old = v,
            _ => {
                m.retain(|(k, _)| k != "tokens");
                if !toks.is_empty() {
                    m.push(("tokens".to_owned(), v));
                }
            }
        }
        Ok(())
    };
    let then: Box<dyn FnOnce(Vec<String>) -> Response + Send> = Box::new(move |_| {
        let mut body = std::mem::take(&mut *answer.lock().unwrap_or_else(PoisonError::into_inner));
        body.push('\n');
        json(200, body)
    });
    edit(host, ctx, change, Some(then));
    true
}

/// Serves the roles' own consoles: each started, moved and stopped with its role's member, as
/// fit's `roleconsole.Console.Apply`. The roles' manager calls it on the control thread.
pub trait Consoles: Send {
    /// Moves role `id`'s console to what `config` says, its member's `console` (none when
    /// `config` is `None`, the role stopped), serving `module`: started anew when its settings
    /// changed, its access (the console's, then the global one) set every time. A console that
    /// does not start leaves the one before running, and says why.
    ///
    /// # Errors
    ///
    /// Why the console did not start, or its access did not compile.
    fn apply(
        &mut self,
        id: &'static str,
        config: Option<&akit_config::Config>,
        module: Option<ModuleRef>,
    ) -> Result<(), String>;

    /// Looks at the running consoles' certificate files once (`-watch-certs`, every interval, on
    /// the control thread): files that changed and settled are served to new connections, the
    /// consoles and their sessions kept; files that do not load leave the running certificate.
    /// Nothing by default.
    fn watch_certs(&mut self, _now: std::time::Instant) {}

    /// Reads the running consoles' certificate files now (SIGHUP), as
    /// [`watch_certs`](Consoles::watch_certs) otherwise. Nothing by default.
    fn reload_certs(&mut self) {}

    /// Where role `id`'s console listens, when it runs: the port it bound, when its `address`
    /// says port 0. None by default.
    fn addr(&self, _id: &str) -> Option<std::net::SocketAddr> {
        None
    }
}

/// Role `id`'s `console` in `c`.
#[must_use]
pub fn console_of<'c>(c: &'c akit_config::Config, id: &str) -> Option<&'c akit_config::Console> {
    let t = c.tunnel.as_ref();
    let l = c.llm.as_ref();
    match id {
        "tunnel.hub" => t?.hub.as_ref()?.console.as_ref(),
        "tunnel.node" => t?.node.as_ref()?.console.as_ref(),
        "tunnel.proxy" => t?.proxy.as_ref()?.console.as_ref(),
        "llm.hub" => l?.hub.as_ref()?.role.console.as_ref(),
        "llm.node" => l?.node.as_ref()?.role.console.as_ref(),
        "llm.proxy" => l?.proxy.as_ref()?.role.console.as_ref(),
        "bittorrent.client" => c.bittorrent.as_ref()?.client.as_ref()?.console.as_ref(),
        "bittorrent.tracker" => c.bittorrent.as_ref()?.tracker.as_ref()?.console.as_ref(),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]
    use super::*;

    #[test]
    fn set_leaves_zero_values_out() {
        let mut m: Member = vec![("a".into(), Value::String("x".into()))];
        set(&mut m, "b", Value::Bool(true));
        set(&mut m, "a", Value::String(String::new()));
        set(&mut m, "c", Value::Array(Vec::new()));
        set(&mut m, "d", Value::Number("0".into()));
        assert_eq!(m, vec![("b".to_owned(), Value::Bool(true))]);
        set(&mut m, "b", Value::Number("5".into()));
        assert_eq!(member(&m, "b"), Some(&Value::Number("5".into())));
    }

    #[test]
    fn body_members() {
        let b = Body(value::parse(r#"{"s": "x", "n": 7, "t": true, "l": [1]}"#).unwrap());
        assert_eq!(b.str("s").unwrap(), "x");
        assert_eq!(b.str("missing").unwrap(), "");
        assert_eq!(b.u32("n").unwrap(), 7);
        assert!(b.bool("t").unwrap());
        assert_eq!(b.list("l").unwrap().len(), 1);
        assert!(b.str("n").is_err());
        assert!(b.u32("s").is_err());
        assert!(Body(Value::String("x".into())).str("a").is_err());
        assert_eq!(Body(Value::Null).str("a").unwrap(), "");
    }
}
