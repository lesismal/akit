//! `GET /cache` and `POST /cache/purge` (I69), as fit's `admin/cache.go`: what each cache zone
//! keeps, and purges of what a zone (or every zone) keeps by URL, URL prefix, tag, or all of it.
//!
//! Both run on the API's I/O thread. Neither blocks it: a purge takes the cache's locks for as
//! long as finding and dropping its keys takes, and the files it removes from a disk tier go on
//! that cache's disk pool.

use std::sync::Arc;

use akio_http::{Context, Response};
use akit_cache::{PurgeError, Selection};
use akit_config::value::Value;

use crate::server::{Shared, error, json, read_body};

/// `GET /cache`: what each cache zone keeps, `[{"zone", "keys", "bytes", "disk_entries",
/// "disk_bytes", "disk_path"}]`, by name.
pub(crate) fn zones() -> Response {
    let mut b = akit_cache::zones_json();
    b.push('\n');
    json(200, b)
}

/// `POST /cache/purge`: `{"zone", "urls", "prefixes", "tags", "all"}`, JSON within
/// `max_config_bytes`; `{"purged": n}`, the keys removed; 404 for a zone no cache uses, 400 for a
/// body that names nothing to purge or a URL that is not one.
pub(crate) fn purge(shared: &Arc<Shared>, ctx: &mut Context) {
    read_body(shared, ctx, &["application/json"], |ctx, data| {
        ctx.respond(answer(&data));
    });
}

pub(crate) fn answer(data: &[u8]) -> Response {
    let (zone, sel) = match request(data) {
        Ok(r) => r,
        Err(msg) => return error(400, &format!("the body: {msg}")),
    };
    if sel.is_empty() {
        return error(400, "name urls, prefixes, tags, or all");
    }
    match akit_cache::purge(&zone, &sel) {
        Ok(n) => json(200, format!("{{\"purged\":{n}}}\n")),
        Err(e @ PurgeError::NoZone(_)) => error(404, &e.to_string()),
        Err(e) => error(400, &e.to_string()),
    }
}

/// The zone and the selection a purge's body names. Members it does not know are ignored and
/// `null` is a member left out, as Go's `json.Unmarshal` reads fit's `purgeRequest`.
fn request(data: &[u8]) -> Result<(String, Selection), String> {
    let text = std::str::from_utf8(data).map_err(|_| "not UTF-8".to_owned())?;
    let v = akit_config::value::parse(text).map_err(|errs| {
        errs.as_slice()
            .first()
            .map_or_else(|| "not JSON".to_owned(), |e| e.msg.clone())
    })?;
    let Value::Object(members) = v else {
        return Err("not a JSON object".to_owned());
    };
    let mut zone = String::new();
    let mut sel = Selection::default();
    for (name, v) in members {
        match name.as_str() {
            "zone" => zone = string(&name, v)?.unwrap_or_default(),
            "urls" => sel.urls = strings(&name, v)?,
            "prefixes" => sel.prefixes = strings(&name, v)?,
            "tags" => sel.tags = strings(&name, v)?,
            "all" => match v {
                Value::Bool(b) => sel.all = b,
                Value::Null => {}
                _ => return Err("all is true or false".to_owned()),
            },
            _ => {}
        }
    }
    Ok((zone, sel))
}

fn string(name: &str, v: Value) -> Result<Option<String>, String> {
    match v {
        Value::String(s) => Ok(Some(s)),
        Value::Null => Ok(None),
        _ => Err(format!("{name} is a string")),
    }
}

fn strings(name: &str, v: Value) -> Result<Vec<String>, String> {
    match v {
        Value::Array(items) => items
            .into_iter()
            .map(|i| match i {
                Value::String(s) => Ok(s),
                _ => Err(format!("{name} is an array of strings")),
            })
            .collect(),
        Value::Null => Ok(Vec::new()),
        _ => Err(format!("{name} is an array of strings")),
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]

    use super::*;

    #[test]
    fn requests() {
        let (zone, sel) = request(
            br#"{"zone": "z", "urls": ["http://h/a"], "prefixes": null, "tags": ["t"], "all": false, "other": 1}"#,
        )
        .unwrap();
        assert_eq!(zone, "z");
        assert_eq!(sel.urls, ["http://h/a"]);
        assert!(sel.prefixes.is_empty() && !sel.all);
        assert_eq!(sel.tags, ["t"]);
        for bad in [
            &b"not json"[..],
            b"[]",
            br#"{"zone": 1}"#,
            br#"{"urls": "http://h/"}"#,
            br#"{"tags": [1]}"#,
            br#"{"all": "yes"}"#,
            b"\xff",
        ] {
            assert!(request(bad).is_err(), "{}", String::from_utf8_lossy(bad));
        }
    }
}
