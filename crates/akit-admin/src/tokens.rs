//! The Admin API's endpoints for the link tokens the tunnel's hub and the LLM roles' listeners
//! issue (I100, fit's `admin/tokens.go`, fit's decision 035), which `akit token` calls:
//!
//! ```text
//! GET    /tokens                 every issuing member's tokens: {"roles": {"tunnel.hub": [...]}}
//! GET    /tokens/{member}        one member's: {"tokens": [...]}
//! POST   /tokens/{member}        issue one: {name, note, expires_at | ttl}, answered with the
//!                                token, shown this once
//! PATCH  /tokens/{member}/{id}   change its expiry: {expires_at | ttl}; neither, never
//! DELETE /tokens/{member}/{id}   revoke it: its links close at once
//! ```
//!
//! A change is an edit of the member's `tokens` in the running config, applied on the control
//! thread as any config and logged to the `admin` logger. Where fit answers 404 unless the role
//! runs, akit's read the member the running config sets, with the running role's last uses and
//! links when it runs ([`TokenViews`]), so that a token can be issued before its role starts.

use std::sync::Arc;

use akio_http::{Context, Method, Response};
use akit_config::value::{Value, write_string};
use akit_observe::logger::{Level, Logger};
use akit_roles::linktoken::{
    self, ExpiryReq, IssueReq, TokenError, View, list_json, record_view, tokens_of, tokens_value,
};

use crate::configs::{Applied, ApplyError, Configs, Mode, Outcome, Refused};
use crate::server::{Shared, error, read_body, submit};

/// The members whose listeners issue link tokens, in the order `GET /tokens` lists them.
pub const ISSUERS: [&str; 4] = ["llm.hub", "llm.node", "llm.proxy", "tunnel.hub"];

/// The running role's tokens, with their last uses and links, for a member (`tunnel.hub`), or
/// `None` when it does not run: called on the API's I/O thread, it must not block.
pub type TokenViews = Arc<dyn Fn(&str) -> Option<Vec<View>> + Send + Sync>;

/// A change of a member's tokens, made on the control thread.
pub(crate) enum TokenOp {
    Issue(String, IssueReq),
    Expiry(String, String, ExpiryReq),
    Revoke(String, String),
}

impl TokenOp {
    pub(crate) fn name(&self) -> &'static str {
        match self {
            TokenOp::Issue(..) => "issue",
            TokenOp::Expiry(..) => "expiry",
            TokenOp::Revoke(..) => "revoke",
        }
    }
}

/// Whether `path` is one of the tokens' that takes a JSON body: `POST /tokens/{member}`,
/// `PATCH /tokens/{member}/{id}`.
pub(crate) fn takes_body(method: &Method, path: &str) -> bool {
    let n = segments(path).map_or(0, |s| s.len());
    matches!((method, n), (Method::Post, 1) | (Method::Patch, 2))
}

/// The segments after `/tokens/`: `None` for a path that is not one of them.
fn segments(path: &str) -> Option<Vec<&str>> {
    let rest = path.strip_prefix("/tokens/")?;
    let segs: Vec<&str> = rest.split('/').collect();
    segs.iter().all(|s| !s.is_empty()).then_some(segs)
}

/// The path of `member`'s object in the config, for one that issues tokens.
fn member_path(member: &str) -> Option<[&str; 2]> {
    ISSUERS
        .contains(&member)
        .then(|| member.split_once('.'))
        .flatten()
        .map(|(a, b)| [a, b])
}

fn member_of<'v>(root: &'v Value, member: &str) -> Option<&'v Value> {
    let [a, b] = member_path(member)?;
    root.get(a)?
        .get(b)
        .filter(|v| matches!(v, Value::Object(_)))
}

fn member_of_mut<'v>(root: &'v mut Value, member: &str) -> Option<&'v mut Vec<(String, Value)>> {
    let [a, b] = member_path(member)?;
    let Value::Object(top) = root else {
        return None;
    };
    let (_, role) = top.iter_mut().find(|(k, _)| k == a)?;
    let Value::Object(role) = role else {
        return None;
    };
    let (_, m) = role.iter_mut().find(|(k, _)| k == b)?;
    match m {
        Value::Object(ms) => Some(ms),
        _ => None,
    }
}

/// A member's views: the running role's, or else from the config.
fn views(shared: &Shared, root: &Value, member: &str) -> Option<Vec<View>> {
    let m = member_of(root, member)?;
    if let Some(v) = shared.tokens.as_ref().and_then(|f| f(member)) {
        return Some(v);
    }
    let now = linktoken::unix_now();
    let mut out: Vec<View> = tokens_of(m).iter().map(|t| record_view(t, now)).collect();
    out.sort_by(|a, b| (&a.name, &a.id).cmp(&(&b.name, &b.id)));
    Some(out)
}

fn json(status: u16, body: String) -> Response {
    Response::with_body(status, "application/json", body.into_bytes())
}

fn running(shared: &Shared) -> Option<Value> {
    akit_config::value::parse(shared.configs.current().data()).ok()
}

/// Answers a request of `/tokens` and below.
pub(crate) fn serve(shared: &Arc<Shared>, ctx: &mut Context, path: &str) {
    let method = ctx.method().clone();
    let read = matches!(method, Method::Get | Method::Head);
    if path == "/tokens" {
        if !read {
            return ctx.respond(crate::server::not_allowed("GET, HEAD"));
        }
        let Some(root) = running(shared) else {
            return ctx.respond(error(500, "the running config is not JSON"));
        };
        let mut body = String::from("{\"roles\":{");
        let mut first = true;
        for m in ISSUERS {
            if let Some(vs) = views(shared, &root, m) {
                if !first {
                    body.push(',');
                }
                first = false;
                write_string(&mut body, m);
                body.push(':');
                body.push_str(&list_json(&vs));
            }
        }
        body.push_str("}}");
        return ctx.respond(json(200, body));
    }
    let Some(segs) = segments(path) else {
        return ctx.respond(error(404, "there is no such endpoint"));
    };
    match (segs.as_slice(), &method) {
        ([member], _) if read => {
            let Some(root) = running(shared) else {
                return ctx.respond(error(500, "the running config is not JSON"));
            };
            match views(shared, &root, member) {
                Some(vs) => ctx.respond(json(200, format!("{{\"tokens\":{}}}", list_json(&vs)))),
                None => ctx.respond(no_member(member)),
            }
        }
        ([member], Method::Post) => {
            let member = (*member).to_owned();
            let shared2 = Arc::clone(shared);
            read_body(
                shared,
                ctx,
                &["application/json"],
                move |ctx, data| match issue_req(&data) {
                    Ok(req) => submit(
                        &shared2,
                        ctx,
                        crate::server::Op::Tokens(TokenOp::Issue(member, req)),
                        Mode::Apply,
                    ),
                    Err(msg) => ctx.respond(error(400, &msg)),
                },
            );
        }
        ([_], _) => ctx.respond(crate::server::not_allowed("GET, HEAD, POST")),
        ([member, id], Method::Patch) => {
            let (member, id) = ((*member).to_owned(), (*id).to_owned());
            let shared2 = Arc::clone(shared);
            read_body(
                shared,
                ctx,
                &["application/json"],
                move |ctx, data| match expiry_req(&data) {
                    Ok(req) => submit(
                        &shared2,
                        ctx,
                        crate::server::Op::Tokens(TokenOp::Expiry(member, id, req)),
                        Mode::Apply,
                    ),
                    Err(msg) => ctx.respond(error(400, &msg)),
                },
            );
        }
        ([member, id], Method::Delete) => submit(
            shared,
            ctx,
            crate::server::Op::Tokens(TokenOp::Revoke((*member).to_owned(), (*id).to_owned())),
            Mode::Apply,
        ),
        ([_, _], _) => ctx.respond(crate::server::not_allowed("PATCH, DELETE")),
        _ => ctx.respond(error(404, "there is no such endpoint")),
    }
}

fn no_member(member: &str) -> Response {
    if ISSUERS.contains(&member) {
        error(404, &format!("the running config sets no {member}"))
    } else {
        error(
            404,
            &format!("{member:?} issues no link tokens: only tunnel.hub and the llm roles do"),
        )
    }
}

/// A body's JSON object, or why it is not one.
fn body_object(data: &[u8]) -> Result<Value, String> {
    let text = std::str::from_utf8(data).map_err(|_| "the body is not UTF-8".to_owned())?;
    let text = if text.trim().is_empty() { "{}" } else { text };
    match akit_config::value::parse(text) {
        Ok(v @ Value::Object(_)) => Ok(v),
        Ok(_) => Err("the body: not a JSON object".to_owned()),
        Err(e) => Err(format!(
            "the body: {}",
            e.as_slice().first().map_or("", |e| e.msg.as_str())
        )),
    }
}

/// A member of a request's object: a string, or a number of seconds.
fn field_str(v: &Value, key: &str) -> Result<String, String> {
    match v.get(key) {
        None | Some(Value::Null) => Ok(String::new()),
        Some(Value::String(s)) => Ok(s.clone()),
        Some(_) => Err(format!("the body: {key} is not a string")),
    }
}

fn field_int(v: &Value, key: &str) -> Result<i64, String> {
    match v.get(key) {
        None | Some(Value::Null) => Ok(0),
        Some(Value::Number(n)) => n
            .parse()
            .map_err(|_| format!("the body: {key} is not an integer")),
        Some(_) => Err(format!("the body: {key} is not a number")),
    }
}

fn issue_req(data: &[u8]) -> Result<IssueReq, String> {
    let v = body_object(data)?;
    Ok(IssueReq {
        name: field_str(&v, "name")?,
        note: field_str(&v, "note")?,
        expires_at: field_int(&v, "expires_at")?,
        ttl: field_str(&v, "ttl")?,
    })
}

fn expiry_req(data: &[u8]) -> Result<ExpiryReq, String> {
    let v = body_object(data)?;
    Ok(ExpiryReq {
        expires_at: field_int(&v, "expires_at")?,
        ttl: field_str(&v, "ttl")?,
    })
}

fn token_refused(e: &TokenError) -> Refused {
    let status = match e {
        TokenError::NotFound => 404,
        _ => 400,
    };
    Refused::Request {
        status,
        msg: e.to_string(),
    }
}

fn log(msg: &str) {
    akit_observe::logger::log(Logger::Admin, Level::Info, format_args!("{msg}"));
}

/// Makes `op`'s change from the running config in `configs` and applies it with `apply`, on the
/// control thread; the answer, and what was applied.
pub(crate) fn run(
    op: TokenOp,
    configs: &Configs,
    source: &str,
    apply: impl FnOnce(&str, Mode) -> Result<Applied, ApplyError>,
) -> (Response, Option<Outcome>) {
    let now = linktoken::unix_now();
    let mut answer: Option<String> = None;
    let mut logged: Option<String> = None;
    let r = configs.change(
        source,
        None,
        Mode::Apply,
        |cur| {
            let mut root = akit_config::value::parse(cur.data()).map_err(|_| Refused::Request {
                status: 500,
                msg: "the running config is not JSON".into(),
            })?;
            let member = match &op {
                TokenOp::Issue(m, _) | TokenOp::Expiry(m, _, _) | TokenOp::Revoke(m, _) => {
                    m.as_str()
                }
            };
            let Some(m) = member_of_mut(&mut root, member) else {
                return Err(Refused::Request {
                    status: 404,
                    msg: if ISSUERS.contains(&member) {
                        format!("the running config sets no {member}")
                    } else {
                        format!(
                            "{member:?} issues no link tokens: only tunnel.hub and the llm roles do"
                        )
                    },
                });
            };
            let obj = Value::Object(m.clone());
            let mut tokens = tokens_of(&obj);
            match &op {
                TokenOp::Issue(_, req) => {
                    let is =
                        linktoken::issue(&mut tokens, req, now).map_err(|e| token_refused(&e))?;
                    let view = record_view(&is.record, now).to_json();
                    let mut body = String::from("{\"token\":");
                    write_string(&mut body, &is.secret.token());
                    body.push(',');
                    body.push_str(view.get(1..).unwrap_or("}"));
                    answer = Some(body);
                    logged = Some(format!(
                        "{member}: issued link token {} ({:?}), {}",
                        is.record.id,
                        is.record.name,
                        linktoken::expires_text(is.record.expires_at)
                    ));
                }
                TokenOp::Expiry(_, id, req) => {
                    let exp = linktoken::set_expiry(&mut tokens, id, req, now)
                        .map_err(|e| token_refused(&e))?;
                    if let Some(t) = tokens.iter().find(|t| &t.id == id) {
                        answer = Some(record_view(t, now).to_json());
                    }
                    logged = Some(format!(
                        "{member}: link token {id} now {}",
                        linktoken::expires_text(exp)
                    ));
                }
                TokenOp::Revoke(_, id) => {
                    linktoken::revoke(&mut tokens, id).map_err(|e| token_refused(&e))?;
                    answer = Some("{\"ok\":true}".to_owned());
                    logged = Some(format!("{member}: revoked link token {id}"));
                }
            }
            let value = tokens_value(&tokens);
            m.retain(|(k, _)| k != "tokens");
            if !tokens.is_empty() {
                // After the member's id and name, as fit writes it; at its end otherwise.
                m.push(("tokens".to_owned(), value));
            }
            Ok(root.to_json())
        },
        apply,
    );
    match r {
        Ok(o) => {
            if let Some(l) = logged {
                log(&l);
            }
            let body = answer.unwrap_or_else(|| "{\"ok\":true}".to_owned());
            (json(200, body), Some(o))
        }
        Err(e) => (crate::server::refused_response(&e), None),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use akit_config::roles::LinkToken;

    #[test]
    fn tokens_round_trip_through_the_config() {
        let t = LinkToken {
            id: "0123456789abcdef".into(),
            name: "lab".into(),
            hash: "ab".repeat(32),
            created_at: 5,
            note: "n".into(),
            members: vec!["node/lab".into()],
            ..LinkToken::default()
        };
        let v = Value::Object(vec![(
            "tokens".into(),
            tokens_value(std::slice::from_ref(&t)),
        )]);
        assert_eq!(tokens_of(&v), vec![t]);
        assert_eq!(member_path("tunnel.hub"), Some(["tunnel", "hub"]));
        assert_eq!(member_path("tunnel.node"), None);
        assert!(takes_body(&Method::Post, "/tokens/tunnel.hub"));
        assert!(takes_body(
            &Method::Patch,
            "/tokens/tunnel.hub/0123456789abcdef"
        ));
        assert!(!takes_body(&Method::Post, "/tokens"));
        assert!(!takes_body(&Method::Delete, "/tokens/tunnel.hub/x"));
    }
}
