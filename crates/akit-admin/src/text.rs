//! The config as text (I91), the endpoints the console's raw editors and its transfer page call,
//! as fit's `admin/check.go` and `admin/transfer.go`:
//!
//! ```text
//! POST /config/render?format=     a config fragment, a JSON object, written as .conf, YAML or
//!                                 JSON (format conf, yaml or json)
//! POST /config/parse?format=      the JSON of such a text
//! PUT  /config/text?format=&members=a,b
//!                                 the text applied as the whole config, or as those top-level
//!                                 members only, the rest kept; dry_run and If-Match as PUT
//!                                 /config
//! POST /config/export             {"format", "members"}: the config, or those members, as a
//!                                 file, without admin
//! POST /config/import?format=&members=a,b
//!                                 a file applied as the config, admin kept, or as those members
//! ```
//!
//! fit's `store` format (I93b) exports a `fitstore` file sealed with the password the request
//! gives, or with the running store's key, and imports one opened with the password in
//! `Akit-Store-Password` (fit's `Fit-Store-Password`, renamed: decision 037), or with the running
//! store's: the file's key is derived on the API's pool, one at a time in the process (the others
//! answered 503), and a file asking for more than twice the cost akit writes with is refused
//! before (`store::unseal`). A whole config's export holds the console users whose passwords
//! changed, so that a restore logs in with them; an import takes none. A text sends the secrets
//! the reads answer as `[redacted]` back as they came, and they are put back from the running config
//! as `PUT /config` puts them (decision 035); an export redacts them as the reads do. Converting
//! a text parses it whole on the API's I/O thread, as a JSON body is, within `max_config_bytes`;
//! the change goes to the control thread as every change does.

// A refusal is the answer itself, on the control plane: its size does not matter.
#![allow(clippy::result_large_err)]

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use akio_http::{Context, Detached, Response};
use akit_config::adapt::{Target, document};
use akit_config::value::{Value, standardize};
use akit_config::{Errors, Format, Source};

use crate::configs::{Mode, Refused, Version};
use crate::server::{Op, Shared, VERSION_HEADER, error, json, read_body, submit, submit_detached};
use crate::store;

/// The header carrying the password of a `fitstore` file imported with `POST
/// /config/import?format=store` that was sealed with one of its own: fit's `Fit-Store-Password`,
/// renamed (decision 037).
pub const STORE_PASSWORD_HEADER: &str = "Akit-Store-Password";

/// A text format: its `format` name, how it is read, and the content types its text is sent as.
struct TextFormat {
    format: Format,
    target: Target,
    types: &'static [&'static str],
}

fn format_of(name: Option<&str>) -> Option<TextFormat> {
    Some(match name? {
        "conf" => TextFormat {
            format: Format::Conf,
            target: Target::Conf,
            types: &["text/plain"],
        },
        "yaml" => TextFormat {
            format: Format::Yaml,
            target: Target::Yaml,
            types: &[
                "text/plain",
                "application/yaml",
                "text/yaml",
                "application/x-yaml",
            ],
        },
        "json" => TextFormat {
            format: Format::Json,
            target: Target::Json,
            types: &["text/plain", "application/json"],
        },
        _ => return None,
    })
}

fn bad_format(export: bool) -> Response {
    if export {
        error(400, "the format must be conf, yaml, json or store")
    } else {
        error(400, "the format must be conf, yaml or json")
    }
}

/// Holds [`Shared::deriving`] while a key is derived for a `fitstore` file, and lets it go when
/// dropped.
struct Deriving(Arc<AtomicBool>);

impl Deriving {
    /// The hold, or `None` while another file's key is derived.
    fn take(flag: &Arc<AtomicBool>) -> Option<Deriving> {
        flag.compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
            .ok()
            .map(|_| Deriving(Arc::clone(flag)))
    }
}

impl Drop for Deriving {
    fn drop(&mut self) {
        self.0.store(false, Ordering::Release);
    }
}

/// 503 while another `fitstore` file's key is derived, as fit's.
fn busy_deriving() -> Response {
    error(503, "another store file is being opened; try again").header("Retry-After", "1")
}

/// What a store's error says to the client: fit's words for a file that does not open.
fn store_error(e: &store::Error) -> String {
    match e {
        store::Error::Open(_) => {
            "the file does not open with the password: it is wrong, or the file was changed"
                .to_owned()
        }
        e => e.to_string(),
    }
}

/// `{"error", "errors"}` of a text that does not parse, as fit's `writeTextParseError`.
fn not_parsed(errs: &Errors) -> Response {
    let mut b = String::from("{\"error\":\"the text does not parse\",\"errors\":[");
    for (i, e) in errs.as_slice().iter().enumerate() {
        if i > 0 {
            b.push(',');
        }
        b.push('{');
        if e.line > 0 {
            b.push_str(&format!("\"line\":{},\"column\":{},", e.line, e.col));
        }
        b.push_str("\"message\":");
        akit_config::value::write_string(&mut b, &e.msg);
        b.push('}');
    }
    b.push_str("]}\n");
    json(400, b)
}

/// The JSON of `text`, a config in `f`, as an object: an empty text (or YAML's null) is the
/// empty config.
fn adapt(f: &TextFormat, text: &str) -> Result<Value, Response> {
    let doc = document(&[Source {
        name: None,
        format: f.format,
        src: text,
    }])
    .map_err(|e| not_parsed(&e))?;
    let json = doc
        .write(Target::Json)
        .map_err(|e| error(400, &format!("the text does not parse: {e}")))?;
    match akit_config::value::parse(&json) {
        Ok(v @ Value::Object(_)) => Ok(v),
        Ok(Value::Null) => Ok(Value::Object(Vec::new())),
        _ => Err(error(400, "the text is not a config object")),
    }
}

/// `members=a,b`: the top-level members a page edits, `admin` refused, as fit's `queryMembers`.
fn members_of(ctx: &Context) -> Result<Vec<String>, Response> {
    let Some(q) = ctx.req().query_param("members") else {
        return Ok(Vec::new());
    };
    let q = akio_http::message::percent_decode(q).unwrap_or_default();
    clean(q.split(','))
}

fn clean<'a>(names: impl Iterator<Item = &'a str>) -> Result<Vec<String>, Response> {
    let mut out: Vec<String> = Vec::new();
    for n in names.map(str::trim).filter(|n| !n.is_empty()) {
        if n == "admin" {
            return Err(error(
                400,
                "admin cannot be edited this way: it comes from the config akit started with",
            ));
        }
        if !out.iter().any(|o| o == n) {
            out.push(n.to_owned());
        }
    }
    Ok(out)
}

fn utf8(data: Vec<u8>) -> Result<String, Response> {
    String::from_utf8(data).map_err(|_| error(400, "the body is not UTF-8"))
}

/// `POST /config/render`.
pub(crate) fn render(shared: &Arc<Shared>, ctx: &mut Context) {
    let format = ctx.req().query_param("format").map(str::to_owned);
    if format_of(format.as_deref()).is_none() {
        return ctx.respond(bad_format(false));
    }
    read_body(shared, ctx, &["application/json"], move |ctx, data| {
        ctx.respond(render_body(format.as_deref(), data));
    });
}

/// `POST /config/render?format=` of `data`, the body: the config in that format, or why not
/// (also the assistant's in-process call, I162).
pub(crate) fn render_body(format: Option<&str>, data: Vec<u8>) -> Response {
    let Some(f) = format_of(format) else {
        return bad_format(false);
    };
    let r = utf8(data).and_then(|text| {
        let std = standardize(&text)
            .map_err(|e| error(400, &format!("the body is not a config: {e}")))?;
        if !matches!(akit_config::value::parse(&std), Ok(Value::Object(_))) {
            return Err(error(400, "a config is a JSON object"));
        }
        let out = document(&[Source {
            name: None,
            format: Format::Json,
            src: &std,
        }])
        .map_err(|e| error(400, &format!("rendering the config: {e}")))?
        .write(f.target)
        .map_err(|e| error(400, &format!("rendering the config: {e}")))?;
        Ok(Response::with_body(
            200,
            "text/plain; charset=utf-8",
            out.into_bytes(),
        ))
    });
    r.unwrap_or_else(|e| e)
}

/// `POST /config/parse`.
pub(crate) fn parse(shared: &Arc<Shared>, ctx: &mut Context) {
    let Some(f) = format_of(ctx.req().query_param("format")) else {
        return ctx.respond(bad_format(false));
    };
    let types = f.types;
    read_body(shared, ctx, types, move |ctx, data| {
        let r = utf8(data)
            .and_then(|text| adapt(&f, &text))
            .map(|v| json(200, v.to_json()));
        ctx.respond(r.unwrap_or_else(|e| e));
    });
}

/// `PUT /config/text`, and `POST /config/import` with `import`: a text applied as the config, or
/// as `members` of it.
pub(crate) fn put(shared: &Arc<Shared>, ctx: &mut Context, import: bool) {
    let name = ctx.req().query_param("format").map(str::to_owned);
    let members = match members_of(ctx) {
        Ok(m) => m,
        Err(r) => return ctx.respond(r),
    };
    let mode = match crate::server::mode_of(ctx) {
        Ok(m) => m,
        Err(p) => return ctx.respond(error(p.status, &p.msg)),
    };
    if import && name.as_deref() == Some("store") {
        return import_store(shared, ctx, members, mode);
    }
    let Some(f) = format_of(name.as_deref()) else {
        return ctx.respond(bad_format(false));
    };
    let shared2 = Arc::clone(shared);
    let types = f.types;
    read_body(shared, ctx, types, move |ctx, data| {
        let text = match utf8(data) {
            Ok(t) => t,
            Err(r) => return ctx.respond(r),
        };
        let frag = match adapt(&f, &text) {
            Ok(v) => v,
            Err(r) => return ctx.respond(r),
        };
        if !import && !members.is_empty() {
            if let Value::Object(ms) = &frag {
                if let Some((k, _)) = ms.iter().find(|(k, _)| !members.contains(k)) {
                    return ctx.respond(error(
                        400,
                        &format!("the text sets {k:?}, which this page does not edit"),
                    ));
                }
            }
        }
        submit(
            &shared2,
            ctx,
            Op::Text {
                frag,
                members,
                import,
                text,
                format: f.format,
            },
            mode,
        );
    });
}

/// `POST /config/import?format=store`: a `fitstore` file, opened on the pool with the password in
/// [`STORE_PASSWORD_HEADER`] or the running store's, applied as the config or as `members` of it,
/// the running `admin` kept; the users it may hold are not imported. fit's `importConfig`.
fn import_store(shared: &Arc<Shared>, ctx: &mut Context, members: Vec<String>, mode: Mode) {
    let password = ctx
        .header(STORE_PASSWORD_HEADER)
        .filter(|p| !p.is_empty())
        .map(str::to_owned);
    let if_match = ctx.header("If-Match").map(str::to_owned);
    let shared2 = Arc::clone(shared);
    read_body(
        shared,
        ctx,
        &["application/octet-stream"],
        move |ctx, file| {
            // Deriving a file's key takes Argon2id's time and memory (up to twice what akit
            // writes with): one at a time, the others refused rather than kept waiting.
            let Some(hold) = Deriving::take(&shared2.deriving) else {
                return ctx.respond(busy_deriving());
            };
            let sealer = shared2.configs.sealer();
            if password.is_none() && sealer.is_none() {
                return ctx.respond(error(
                    400,
                    &format!(
                        "akit runs without a store: give the file's password in {STORE_PASSWORD_HEADER}"
                    ),
                ));
            }
            let shared3 = Arc::clone(&shared2);
            let queued = ctx.detach_to_pool(move |d: Detached| {
                let opened = match (&password, &sealer) {
                    (Some(p), _) => store::unseal(&file, p),
                    (None, Some(s)) => s.unseal(&file),
                    (None, None) => return,
                };
                drop(hold);
                let data = match opened {
                    Ok((data, _users)) => data,
                    Err(e) => return d.respond(error(400, &store_error(&e))),
                };
                let frag = match std::str::from_utf8(&data)
                    .ok()
                    .and_then(|t| akit_config::value::parse(t).ok())
                {
                    Some(v @ Value::Object(_)) => v,
                    _ => return d.respond(error(400, "the file is not a config object")),
                };
                submit_detached(&shared3, d, if_match, Op::Sealed { frag, members }, mode);
            });
            if !queued {
                ctx.respond(error(503, "the Admin API is busy; try again"));
            }
        },
    );
}

/// `e`, a change refused, its config's errors placed in `text`, the text they came from in
/// `format`, as fit's adapter maps them: each at the value its path names, or the deepest part
/// of it the text has.
pub(crate) fn placed(e: Refused, text: &str, format: Format) -> Refused {
    let Refused::Apply(crate::configs::ApplyError::Config(mut errs)) = e else {
        return e;
    };
    let src = Source {
        name: None,
        format,
        src: text,
    };
    for err in &mut errs.0 {
        match akit_config::position_of(&src, &err.path) {
            Some((line, col)) => {
                err.line = line;
                err.col = col;
            }
            None => {
                err.line = 0;
                err.col = 0;
            }
        }
        err.file = None;
    }
    Refused::Apply(crate::configs::ApplyError::Config(errs))
}

/// The config a text makes of the running one, `cur`: `frag` whole (with `import`, the running
/// `admin` kept and every other member `frag`'s), or `members` of it put into the running config
/// (a member `frag` leaves out removed); the secrets it sends as `[redacted]` put back.
pub(crate) fn edited(
    cur: &Version,
    mut frag: Value,
    members: &[String],
    import: bool,
) -> Result<String, Refused> {
    let bad = |msg: String| Refused::Request { status: 400, msg };
    let old = akit_config::value::parse(cur.data())
        .map_err(|_| bad("the running config is not a JSON object".into()))?;
    let Value::Object(old_members) = &old else {
        return Err(bad("the running config is not a JSON object".into()));
    };
    crate::redact::restore(&mut frag, Some(&old), "").map_err(bad)?;
    let Value::Object(frag_members) = frag else {
        return Err(bad("a config is a JSON object".into()));
    };
    if members.is_empty() && !import {
        return Ok(Value::Object(frag_members).to_json());
    }
    let take: Vec<String> = if members.is_empty() {
        frag_members
            .iter()
            .map(|(k, _)| k.clone())
            .filter(|k| k != "admin")
            .collect()
    } else {
        members.to_vec()
    };
    let mut base: Vec<(String, Value)> = if members.is_empty() {
        // An import of the whole config: admin is kept, the rest is the file's.
        old_members
            .iter()
            .filter(|(k, _)| k == "admin")
            .cloned()
            .collect()
    } else {
        old_members.clone()
    };
    for m in &take {
        let new = frag_members.iter().find(|(k, _)| k == m).map(|(_, v)| v);
        match (base.iter().position(|(k, _)| k == m), new) {
            (Some(i), Some(v)) => {
                if let Some(slot) = base.get_mut(i) {
                    slot.1 = v.clone();
                }
            }
            (None, Some(v)) => base.push((m.clone(), v.clone())),
            (Some(i), None) => {
                base.remove(i);
            }
            (None, None) => {}
        }
    }
    Ok(Value::Object(base).to_json())
}

/// `POST /config/export`: `{"format", "members"}`, the running config, or those members of it,
/// written in the format, without `admin` and with its secrets redacted, as a file.
pub(crate) fn export(shared: &Arc<Shared>, ctx: &mut Context) {
    let configs = shared.configs.clone();
    let deriving = Arc::clone(&shared.deriving);
    read_body(shared, ctx, &["application/json"], move |ctx, data| {
        let r = (|| -> Result<Response, Exported> {
            let text = utf8(data)?;
            let body = akit_config::value::parse(&text).map_err(|_| {
                error(
                    400,
                    "the body must be {\"format\", \"members\", \"password\"}",
                )
            })?;
            let format = match body.get("format") {
                Some(Value::String(s)) => s.clone(),
                _ => String::new(),
            };
            let names: Vec<String> = match body.get("members") {
                Some(Value::Array(a)) => a
                    .iter()
                    .filter_map(|v| match v {
                        Value::String(s) => Some(s.clone()),
                        _ => None,
                    })
                    .collect(),
                _ => Vec::new(),
            };
            let members = clean(names.iter().map(String::as_str)).map_err(|_| {
                error(
                    400,
                    "admin is not exported: it comes from the config akit started with",
                )
            })?;
            let password = match body.get("password") {
                Some(Value::String(s)) if !s.is_empty() => Some(s.clone()),
                _ => None,
            };
            let target = match format_of(Some(format.as_str())) {
                Some(f) => Some(f.target),
                None if format == "store" => None,
                None => return Err(bad_format(true).into()),
            };
            let v = configs.current();
            let redacted = crate::redact::config(v.data())
                .ok_or_else(|| error(500, "the running config is not a JSON object"))?;
            let Ok(Value::Object(mut root)) = akit_config::value::parse(&redacted) else {
                return Err(error(500, "the running config is not a JSON object").into());
            };
            root.retain(|(k, _)| k != "admin" && (members.is_empty() || members.contains(k)));
            let std = Value::Object(root).to_json();
            let name = if members.is_empty() {
                "config".to_owned()
            } else {
                members.join("-")
            };
            let headers = move |resp: Response| {
                resp.header(
                    "Content-Disposition",
                    format!(
                        "attachment; filename=\"akit-{name}-v{}.{format}\"",
                        v.number
                    ),
                )
                .header("ETag", &v.etag)
                .header(VERSION_HEADER, v.number.to_string())
            };
            let Some(target) = target else {
                return Err(Sealing {
                    std,
                    whole: members.is_empty(),
                    password,
                    headers: Box::new(headers),
                }
                .into());
            };
            let out = document(&[Source {
                name: None,
                format: Format::Json,
                src: &std,
            }])
            .map_err(|e| error(500, &format!("exporting the config: {e}")))?
            .write(target)
            .map_err(|e| error(500, &format!("exporting the config: {e}")))?;
            Ok(headers(Response::with_body(
                200,
                "text/plain; charset=utf-8",
                out.into_bytes(),
            )))
        })();
        match r {
            Ok(resp) | Err(Exported::Answer(resp)) => ctx.respond(resp),
            Err(Exported::Seal(s)) => seal_export(&configs, ctx, &deriving, *s),
        }
    });
}

/// A `fitstore` export to seal off the I/O thread.
struct Sealing {
    /// The config, standard JSON.
    std: String,
    /// Whether it is the whole config, which takes the console users with it.
    whole: bool,
    /// The export's own password, or `None` for the store's key.
    password: Option<String>,
    headers: Box<dyn FnOnce(Response) -> Response + Send>,
}

/// What an export comes to before it is answered.
enum Exported {
    Answer(Response),
    Seal(Box<Sealing>),
}

impl From<Response> for Exported {
    fn from(r: Response) -> Exported {
        Exported::Answer(r)
    }
}

impl From<Sealing> for Exported {
    fn from(s: Sealing) -> Exported {
        Exported::Seal(Box::new(s))
    }
}

/// Seals an export on the pool: with its own password (a key derived, one at a time in the
/// process), or with the running store's key; 400 when akit runs without a store and none is
/// given. As fit's `exportConfig`.
fn seal_export(
    configs: &crate::Configs,
    ctx: &mut Context,
    deriving: &Arc<AtomicBool>,
    s: Sealing,
) {
    let sealer = configs.sealer();
    if s.password.is_none() && sealer.is_none() {
        return ctx.respond(error(
            400,
            "akit runs without a store: give the password to seal the export with",
        ));
    }
    let hold = match &s.password {
        Some(_) => match Deriving::take(deriving) {
            Some(h) => Some(h),
            None => return ctx.respond(busy_deriving()),
        },
        None => None,
    };
    let configs = configs.clone();
    let queued = ctx.detach_to_pool(move |d: Detached| {
        let users = if s.whole {
            configs.console_users().0
        } else {
            Vec::new()
        };
        let sealed = match (&s.password, &sealer) {
            (Some(p), _) => store::seal(p, s.std.as_bytes(), &users),
            (None, Some(st)) => st.seal(s.std.as_bytes(), &users),
            (None, None) => return,
        };
        drop(hold);
        match sealed {
            Ok(out) => d.respond((s.headers)(Response::with_body(
                200,
                "application/octet-stream",
                out,
            ))),
            Err(e) => d.respond(error(500, &format!("exporting the config: {e}"))),
        }
    });
    if !queued {
        ctx.respond(error(503, "the Admin API is busy; try again"));
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]
    use super::*;

    fn version(json: &str) -> Arc<Version> {
        crate::Configs::new(json.to_owned(), "file", 2).current()
    }

    fn obj(s: &str) -> Value {
        akit_config::value::parse(s).unwrap()
    }

    #[test]
    fn texts_edit_the_running_config() {
        let cur = version(
            r#"{"admin": {"address": "127.0.0.1:2019"}, "routes": [], "log": {"level": "info"}}"#,
        );
        // Whole.
        assert_eq!(
            edited(&cur, obj(r#"{"routes": [1]}"#), &[], false).unwrap(),
            Value::Object(vec![("routes".into(), obj("[1]"))]).to_json()
        );
        // Some members: the rest kept, a member left out removed.
        let out = edited(
            &cur,
            obj(r#"{"routes": [2]}"#),
            &["routes".into(), "log".into()],
            false,
        )
        .unwrap();
        let v = obj(&out);
        assert_eq!(v.get("routes"), Some(&obj("[2]")));
        assert_eq!(v.get("log"), None);
        assert!(v.get("admin").is_some());
        // An import of the whole file: admin kept, the rest the file's.
        let out = edited(&cur, obj(r#"{"admin": {"x": 1}, "limits": {}}"#), &[], true).unwrap();
        let v = obj(&out);
        assert_eq!(
            v.get("admin"),
            obj(r#"{"admin": {"address": "127.0.0.1:2019"}}"#).get("admin")
        );
        assert!(v.get("limits").is_some() && v.get("routes").is_none());
    }
}
