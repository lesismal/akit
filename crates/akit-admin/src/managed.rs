//! What a manager of many owns on this akit (I136, fit's `admin/managed.go` and
//! `internal/managed`): the record a manager's templates write with `PUT /node/managed`, kept in
//! the store's section `managed` (fit's) or, without a store, in memory; and `GET /node/managed`,
//! which says, of each part, whether the config has it and whether it changed since the manager
//! last saw it in sync, so that the pages mark the fields a manager owns.
//!
//! The hashes are of the parts as this akit's API answers them, its secrets redacted (decision
//! 035), which is what a manager reads and compares.

use std::sync::{Mutex, PoisonError};

use akio_http::{Context, Method, Response};
use akit_config::cluster::{
    MAX_MANAGED_NAME, check_managed_part, managed_display, managed_hash, managed_lookup,
};
use akit_config::value::{Value, write_string};

use crate::configs::Configs;
use crate::server::{error, json};

/// The capability `GET /node` lists for it: fit's.
pub const CAP_MANAGED: &str = "node.managed";
/// The most parts a record names.
pub const MAX_PATHS: usize = 4096;
/// The largest record.
pub const MAX_RECORD_BYTES: usize = 1 << 20;

/// A part a manager owns: fit's `managed.Path`.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Path {
    /// The top-level member.
    pub member: String,
    /// The element's name, for a named list.
    pub name: String,
    /// The template that owns it.
    pub template: String,
    /// The SHA-256 of its value when the manager last saw it in sync, or empty.
    pub hash: String,
}

/// What a manager owns here: fit's `managed.Record`.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Record {
    /// The manager's node ID.
    pub manager: String,
    /// Its host's name.
    pub manager_host: String,
    /// When it last told this akit, as written.
    pub updated: String,
    /// The parts, sorted by member and name.
    pub paths: Vec<Path>,
}

impl Record {
    /// Reads a record as fit's `PUT /node/managed` takes it: unknown members refused.
    ///
    /// # Errors
    ///
    /// Why it is not one.
    pub fn from_value(v: &Value) -> Result<Record, String> {
        let Value::Object(ms) = v else {
            return Err("not an object".to_owned());
        };
        let s = |x: &Value, k: &str| match x {
            Value::String(s) => Ok(s.clone()),
            Value::Null => Ok(String::new()),
            _ => Err(format!("{k} is not a string")),
        };
        let mut r = Record::default();
        for (k, x) in ms {
            match k.as_str() {
                "manager" => r.manager = s(x, k)?,
                "manager_host" => r.manager_host = s(x, k)?,
                "updated" => r.updated = s(x, k)?,
                "paths" => match x {
                    Value::Array(items) => {
                        for it in items {
                            let Value::Object(pm) = it else {
                                return Err("paths holds what is not an object".to_owned());
                            };
                            let mut p = Path::default();
                            for (pk, px) in pm {
                                match pk.as_str() {
                                    "member" => p.member = s(px, pk)?,
                                    "name" => p.name = s(px, pk)?,
                                    "template" => p.template = s(px, pk)?,
                                    "hash" => p.hash = s(px, pk)?,
                                    other => return Err(format!("unknown field {other:?}")),
                                }
                            }
                            r.paths.push(p);
                        }
                    }
                    Value::Null => {}
                    _ => return Err("paths is not a list".to_owned()),
                },
                other => return Err(format!("unknown field {other:?}")),
            }
        }
        Ok(r)
    }

    /// Checks it as fit's `Record.Check`.
    ///
    /// # Errors
    ///
    /// Why it is not one.
    pub fn check(&self) -> Result<(), String> {
        if self.manager.len() > MAX_MANAGED_NAME {
            return Err(format!(
                "manager: at most {MAX_MANAGED_NAME} bytes of UTF-8"
            ));
        }
        if self.manager_host.len() > MAX_MANAGED_NAME {
            return Err(format!(
                "manager_host: at most {MAX_MANAGED_NAME} bytes of UTF-8"
            ));
        }
        if self.paths.len() > MAX_PATHS {
            return Err(format!(
                "paths: {} parts, more than {MAX_PATHS}",
                self.paths.len()
            ));
        }
        for (i, p) in self.paths.iter().enumerate() {
            if let Some(e) = check_managed_part(&p.member, &p.name) {
                return Err(format!("paths[{i}]: {e}"));
            }
            if p.template.len() > MAX_MANAGED_NAME {
                return Err(format!(
                    "paths[{i}].template: at most {MAX_MANAGED_NAME} bytes of UTF-8"
                ));
            }
            if !p.hash.is_empty()
                && !(p.hash.len() == 64
                    && p.hash
                        .bytes()
                        .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b)))
            {
                return Err(format!(
                    "paths[{i}].hash: 64 lower-case hex digits, or none"
                ));
            }
            if self.paths.get(..i).is_some_and(|before| {
                before
                    .iter()
                    .any(|q| q.member == p.member && q.name == p.name)
            }) {
                return Err(format!(
                    "paths[{i}]: {} is listed twice",
                    managed_display(&p.member, &p.name)
                ));
            }
        }
        Ok(())
    }

    /// Sorts the parts by member and name, fit's `Normalize`.
    pub fn normalize(&mut self) {
        self.paths
            .sort_by(|a, b| a.member.cmp(&b.member).then_with(|| a.name.cmp(&b.name)));
    }

    /// The record as fit writes it, `updated` left out when empty.
    #[must_use]
    pub fn to_json(&self, with_updated: bool) -> String {
        let mut b = String::from("{\"manager\":");
        write_string(&mut b, &self.manager);
        if !self.manager_host.is_empty() {
            b.push_str(",\"manager_host\":");
            write_string(&mut b, &self.manager_host);
        }
        if with_updated && !self.updated.is_empty() {
            b.push_str(",\"updated\":");
            write_string(&mut b, &self.updated);
        }
        b.push_str(",\"paths\":[");
        for (i, p) in self.paths.iter().enumerate() {
            if i > 0 {
                b.push(',');
            }
            path_fields(&mut b, p);
            b.push('}');
        }
        b.push_str("]}");
        b
    }

    /// Its ETag: of the record without `updated`, normalized, fit's form (`"m"` and 16 hex).
    #[must_use]
    pub fn etag(&self) -> String {
        let mut c = self.clone();
        c.normalize();
        let h = managed_hash(&akit_config::value::parse(&c.to_json(false)).unwrap_or(Value::Null));
        format!("\"m{}\"", h.get(..16).unwrap_or(""))
    }
}

fn path_fields(b: &mut String, p: &Path) {
    b.push_str("{\"member\":");
    write_string(b, &p.member);
    for (k, v) in [
        ("name", &p.name),
        ("template", &p.template),
        ("hash", &p.hash),
    ] {
        if !v.is_empty() {
            b.push_str(&format!(",\"{k}\":"));
            write_string(b, v);
        }
    }
}

/// Keeps a record: in the store's section `managed`.
pub type SaveManaged = Box<dyn Fn(&[u8]) -> Result<(), String> + Send + Sync>;

/// What `Configs` keeps of it.
#[derive(Default)]
pub(crate) struct State {
    rec: Record,
    save: Option<SaveManaged>,
}

impl std::fmt::Debug for State {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("State")
            .field("paths", &self.rec.paths.len())
            .field("stored", &self.save.is_some())
            .finish()
    }
}

fn lock<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap_or_else(PoisonError::into_inner)
}

impl Configs {
    /// Keeps what a manager owns here in the store: `initial` is the section as the store holds
    /// it, `save` writes it there. Without it, the record lives in memory, as fit's without a
    /// store.
    pub fn use_managed(&self, initial: Option<&[u8]>, save: SaveManaged) {
        let rec = initial
            .and_then(|d| std::str::from_utf8(d).ok())
            .and_then(|t| akit_config::value::parse(t).ok())
            .and_then(|v| Record::from_value(&v).ok())
            .filter(|r| r.check().is_ok())
            .map(|mut r| {
                r.normalize();
                r
            })
            .unwrap_or_default();
        let mut st = lock(self.managed_state());
        st.rec = rec;
        st.save = Some(save);
    }

    /// The record, and where it is kept: `store` or `memory`.
    #[must_use]
    pub fn managed(&self) -> (Record, &'static str) {
        let st = lock(self.managed_state());
        (
            st.rec.clone(),
            if st.save.is_some() { "store" } else { "memory" },
        )
    }

    /// Keeps `rec`, checked: written to the store when there is one. Writes a file: not on an I/O
    /// thread.
    ///
    /// # Errors
    ///
    /// The record does not check, or could not be written.
    pub fn set_managed(&self, mut rec: Record) -> Result<(), String> {
        rec.check()?;
        rec.normalize();
        let mut st = lock(self.managed_state());
        if let Some(save) = &st.save {
            save(rec.to_json(true).as_bytes())?;
        }
        st.rec = rec;
        Ok(())
    }
}

/// `GET /node/managed`'s answer: the record, where it is kept, and each part's presence and
/// whether it changed since the manager last saw it in sync, against the config as the API
/// answers it.
fn view(configs: &Configs) -> Response {
    let (rec, kept) = configs.managed();
    let data = crate::redact::config(configs.current().data())
        .unwrap_or_else(|| configs.current().data().to_owned());
    let cfg = akit_config::value::parse(&data).unwrap_or(Value::Null);
    let mut b = String::from("{\"manager\":");
    write_string(&mut b, &rec.manager);
    if !rec.manager_host.is_empty() {
        b.push_str(",\"manager_host\":");
        write_string(&mut b, &rec.manager_host);
    }
    if !rec.updated.is_empty() {
        b.push_str(",\"updated\":");
        write_string(&mut b, &rec.updated);
    }
    b.push_str(",\"kept\":");
    write_string(&mut b, kept);
    b.push_str(",\"paths\":[");
    for (i, p) in rec.paths.iter().enumerate() {
        if i > 0 {
            b.push(',');
        }
        let v = managed_lookup(&cfg, &p.member, &p.name);
        let changed = !p.hash.is_empty() && v.is_none_or(|v| managed_hash(v) != p.hash);
        path_fields(&mut b, p);
        b.push_str(",\"path\":");
        write_string(&mut b, &managed_display(&p.member, &p.name));
        b.push_str(&format!(
            ",\"present\":{},\"changed\":{changed}}}",
            v.is_some()
        ));
    }
    b.push_str("]}\n");
    json(200, b).header("ETag", rec.etag())
}

/// Answers `/node/managed`: `GET`, `PUT` (a record, `If-Match` its ETag) and `DELETE` (none
/// owned), the writes on the pool.
pub(crate) fn serve(configs: &Configs, ctx: &mut Context) {
    let method = ctx.method().clone();
    match method {
        Method::Get | Method::Head => ctx.respond(view(configs)),
        Method::Put | Method::Delete => {
            let rec = if method == Method::Put {
                if ctx.body().len() > MAX_RECORD_BYTES {
                    return ctx.respond(error(
                        413,
                        "a record of what a manager owns is 1 MiB at most",
                    ));
                }
                let parsed = std::str::from_utf8(ctx.body())
                    .ok()
                    .and_then(|t| akit_config::value::parse(t).ok());
                match parsed.as_ref().map(Record::from_value) {
                    Some(Ok(r)) => r,
                    Some(Err(e)) => {
                        return ctx.respond(error(
                            400,
                            &format!("the body is not a record of what a manager owns: {e}"),
                        ));
                    }
                    None => {
                        return ctx.respond(error(
                            400,
                            "the body is not a record of what a manager owns: not JSON",
                        ));
                    }
                }
            } else {
                Record::default()
            };
            if let Err(e) = rec.check() {
                return ctx.respond(error(400, &e));
            }
            let want = ctx.header("If-Match").map(str::to_owned);
            let configs = configs.clone();
            ctx.detach_to_pool(move |d| {
                let (cur, _) = configs.managed();
                if let Some(w) = want.filter(|w| w != "*") {
                    if w != cur.etag() {
                        return d.respond(error(
                            412,
                            "what a manager owns here changed since it was read; read it again",
                        ));
                    }
                }
                d.respond(match configs.set_managed(rec) {
                    Ok(()) => view(&configs),
                    Err(e) => error(500, &format!("keeping what a manager owns: {e}")),
                });
            });
        }
        _ => ctx.respond(
            error(405, "the method is not allowed").header("Allow", "GET, HEAD, PUT, DELETE"),
        ),
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]
    use super::*;

    #[test]
    fn records_checked() {
        let v = akit_config::value::parse(
            r#"{"manager": "m1", "manager_host": "h", "updated": "2026-11-27T00:00:00Z",
                "paths": [{"member": "routes", "name": "api", "template": "t", "hash": ""},
                          {"member": "log", "hash": "00000000000000000000000000000000000000000000000000000000000000ab"}]}"#,
        )
        .unwrap();
        let mut r = Record::from_value(&v).unwrap();
        r.check().unwrap();
        r.normalize();
        assert_eq!(r.paths[0].member, "log");
        assert!(
            r.to_json(true)
                .contains(r#""updated":"2026-11-27T00:00:00Z""#)
        );
        assert_eq!(r.etag().len(), 19);
        let mut same = r.clone();
        same.updated = "later".into();
        assert_eq!(same.etag(), r.etag());
        for bad in [
            r#"{"paths": [{"member": "admin"}]}"#,
            r#"{"paths": [{"member": "routes"}]}"#,
            r#"{"paths": [{"member": "log", "hash": "XY"}]}"#,
            r#"{"paths": [{"member": "log"}, {"member": "log"}]}"#,
        ] {
            let r = Record::from_value(&akit_config::value::parse(bad).unwrap()).unwrap();
            assert!(r.check().is_err(), "{bad}");
        }
        assert!(Record::from_value(&akit_config::value::parse(r#"{"x": 1}"#).unwrap()).is_err());
    }
}
