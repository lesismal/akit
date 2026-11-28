//! The read tools (I162, fit's `assistant/tools_read.go`): each reads what akit runs through the
//! Admin API in process ([`crate::Api`]), as the console's pages do, and hands the model what it
//! found, redacted (the chat redacts and bounds it again). `ask_user` is the chat's own.

use std::collections::BTreeSet;

use akit_config::assistant::Level;
use akit_config::value::Value;

use crate::json::{self, Obj, quoted};
use crate::redact::HIDDEN;
use crate::registry::{Call, Kind, Tool};

/// The most samples `metrics_query` gives, and its default.
const MAX_METRIC_LINES: u64 = 1000;
const DEFAULT_METRIC_LINES: u64 = 200;
/// The most changes `versions_diff` lists.
const MAX_DIFF_ENTRIES: usize = 400;
/// The most bytes of text `config_validate` takes.
const MAX_TOOL_TEXT_BYTES: usize = 1 << 20;
/// The most buttons of `ask_user`.
pub const MAX_ASK_OPTIONS: usize = 8;

/// The read tools of the registry, and `ask_user`.
#[must_use]
pub fn read_tools() -> Vec<Tool> {
    let t = |name, group, description, parameters, run| Tool {
        name,
        group,
        level: Level::Read,
        description,
        parameters,
        max_bytes: 0,
        kind: Kind::Pool,
        enabled: None,
        run: Some(run),
    };
    vec![
        t(
            "config_get",
            "config",
            "Read akit's config: the base config (as written and kept) or the effective one (with what providers added), whole or only some top-level members (one it does not set is null), as JSON, akit's .conf or YAML. Secrets are redacted and the assistant's, the console's users and the Admin API's own settings are left out.",
            r#"{"type":"object","properties":{"source":{"type":"string","enum":["base","effective"],"description":"base (default) or effective"},"members":{"type":"array","items":{"type":"string"},"description":"top-level members to read, as routes or upstreams; all when left out"},"format":{"type":"string","enum":["json","conf","yaml"],"description":"json (default), conf or yaml"}}}"#,
            config_get,
        ),
        t(
            "schema_describe",
            "config",
            "Describe a member of akit's config from its schema: what it is, its type, values, default and its members. path is dotted, as routes.handle or admin.console; empty for the top level.",
            r#"{"type":"object","properties":{"path":{"type":"string","description":"dotted path of the member, as listeners.address"},"lang":{"type":"string","enum":["en","zh"],"description":"the descriptions' language: en (default) or zh"}}}"#,
            schema_describe,
        ),
        t(
            "config_validate",
            "config",
            "Check a config as `akit validate` does, without applying it: either text (a whole config in conf, yaml or json), or patch, a JSON object of top-level members put over the running base config. Returns valid, or each error with its path, line and column.",
            r#"{"type":"object","properties":{"text":{"type":"string","description":"a whole config"},"format":{"type":"string","enum":["conf","yaml","json"],"description":"the text's format; json when left out"},"patch":{"type":"object","description":"top-level members that replace the running config's"}}}"#,
            config_validate,
        ),
        t(
            "versions_diff",
            "config",
            "List the config versions akit keeps, or, with from (and to, the current one when left out), what changed between two of them, member by member.",
            r#"{"type":"object","properties":{"from":{"type":"integer","description":"the older version"},"to":{"type":"integer","description":"the newer version; the current one when left out"}}}"#,
            versions_diff,
        ),
        t(
            "status_get",
            "runtime",
            "What akit runs now: the config version and when it was applied, the listeners bound, connections, upstreams in short, the roles running and the uptime.",
            r#"{"type":"object","properties":{}}"#,
            status_get,
        ),
        t(
            "metrics_query",
            "runtime",
            "akit's metrics as they stand now (counters since start, gauges and histograms), filtered: by the metric's name, and the route, upstream or listener label. Histograms' buckets only when asked.",
            r#"{"type":"object","properties":{"name":{"type":"string","description":"text the metric's name holds, as http_requests or upstream"},"route":{"type":"string"},"upstream":{"type":"string"},"listener":{"type":"string"},"buckets":{"type":"boolean","description":"include histogram buckets"},"limit":{"type":"integer","description":"the most samples, 200 when left out"}}}"#,
            metrics_query,
        ),
        t(
            "upstream_health",
            "runtime",
            "The upstream pools and each backend's health: up or down, probed down, requests in flight, succeeded and failed.",
            r#"{"type":"object","properties":{"upstream":{"type":"string","description":"one pool's name; all when left out"}}}"#,
            upstream_health,
        ),
        t(
            "consistency_check",
            "runtime",
            "Compare what akit runs with the config it runs from, as the Consistency page does: each difference with its category and the fix offered.",
            r#"{"type":"object","properties":{}}"#,
            consistency_check,
        ),
        t(
            "certs_list",
            "runtime",
            "The certificates served: listener, names, issuer, validity and days left; with the acme and auto_https settings.",
            r#"{"type":"object","properties":{}}"#,
            certs_list,
        ),
        t(
            "cache_zones",
            "runtime",
            "The response cache's zones: their sizes, entries and hit rates.",
            r#"{"type":"object","properties":{}}"#,
            cache_zones,
        ),
        Tool {
            name: "ask_user",
            group: "agent",
            level: Level::Read,
            description: "Ask the user a question when a change lacks what it needs (an address, a name) or a choice is theirs, rather than guessing. The options are shown as buttons; the one chosen is the result.",
            parameters: r#"{"type":"object","properties":{"question":{"type":"string"},"options":{"type":"array","items":{"type":"string"},"minItems":1,"maxItems":8}},"required":["question","options"]}"#,
            max_bytes: 0,
            kind: Kind::Ask,
            enabled: None,
            run: None,
        },
    ]
}

/// The running config as JSON, the base one or the effective one, redacted by the API.
fn config_json(c: &Call<'_>, source: &str) -> Result<Value, String> {
    if source == "effective" {
        let body = c.a.api_json("GET", "/config/effective", Vec::new())?;
        let v = akit_config::value::parse(&body)
            .map_err(|_| "the effective config is not JSON".to_owned())?;
        return v
            .get("config")
            .cloned()
            .ok_or_else(|| "the effective config is not JSON".to_owned());
    }
    let body = c.a.api_json("GET", "/config", Vec::new())?;
    akit_config::value::parse(&body).map_err(|_| "the config is not JSON".to_owned())
}

fn config_get(c: &Call<'_>) -> Result<String, String> {
    let source = c.str("source")?;
    if !matches!(source.as_str(), "" | "base" | "effective") {
        return Err(format!("source is base or effective, not {source:?}"));
    }
    let names = c.strings("members")?;
    let format = c.str("format")?;
    let v = config_json(c, &source)?;
    let red = c.a.redactor(c.settings).json(&v, HIDDEN);
    let v = akit_config::value::parse(&red).map_err(|_| "the config is not JSON".to_owned())?;
    let Value::Object(ms) = v else {
        return Err("the config is not a JSON object".to_owned());
    };
    let kept: Vec<(String, Value)> = ms
        .into_iter()
        .filter(|(k, _)| names.is_empty() || names.contains(k))
        .collect();
    let absent: Vec<&String> = names
        .iter()
        .filter(|n| !kept.iter().any(|(k, _)| k == *n))
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect();
    let obj = Value::Object(kept);
    match format.as_str() {
        "" | "json" => {
            let mut all = obj;
            if let Value::Object(ms) = &mut all {
                for n in &absent {
                    ms.push(((*n).clone(), Value::Null));
                }
            }
            Ok(json::compact(&all))
        }
        "conf" | "yaml" => {
            let mut text = c.a.api_json(
                "POST",
                &format!("/config/render?format={format}"),
                json::compact(&obj).into_bytes(),
            )?;
            if !absent.is_empty() {
                let list: Vec<&str> = absent.iter().map(|s| s.as_str()).collect();
                text.push_str(&format!("\n# The config sets no {}.\n", list.join(", ")));
            }
            Ok(text)
        }
        f => Err(format!("format is json, conf or yaml, not {f:?}")),
    }
}

/// A member of the schema, its `$ref`s followed.
fn resolve<'v>(root: &'v Value, n: &'v Value) -> Value {
    let mut n = n.clone();
    for _ in 0..8 {
        let Some(Value::String(r)) = n.get("$ref").cloned() else {
            break;
        };
        let name = r.trim_start_matches("#/$defs/");
        let Some(Value::Object(d)) = root.get("$defs").and_then(|d| d.get(name)).cloned() else {
            break;
        };
        let mut merged = d;
        if let Value::Object(own) = &n {
            for (k, v) in own {
                if k != "$ref" {
                    merged.retain(|(mk, _)| mk != k);
                    merged.push((k.clone(), v.clone()));
                }
            }
        }
        n = Value::Object(merged);
    }
    if n.get("properties").is_none() {
        if let Some(items) = n.get("items").cloned() {
            let it = resolve(root, &items);
            if let Some(props) = it.get("properties") {
                let mut o = vec![
                    (
                        "type".to_owned(),
                        n.get("type").cloned().unwrap_or(Value::Null),
                    ),
                    (
                        "description".to_owned(),
                        n.get("description").cloned().unwrap_or(Value::Null),
                    ),
                    (
                        "items_type".to_owned(),
                        it.get("type").cloned().unwrap_or(Value::Null),
                    ),
                    ("properties".to_owned(), props.clone()),
                ];
                if let Some(r) = it.get("required") {
                    o.push(("required".to_owned(), r.clone()));
                }
                return Value::Object(o);
            }
        }
    }
    n
}

fn schema_describe(c: &Call<'_>) -> Result<String, String> {
    let path = c.str("path")?;
    let body = c.a.api_json("GET", "/schema", Vec::new())?;
    let root = akit_config::value::parse(&body).map_err(|_| "the schema is not JSON".to_owned())?;
    let mut n = resolve(&root, &root);
    if !path.is_empty() {
        for part in path.split('.') {
            let next = n
                .get("properties")
                .and_then(|p| p.get(part))
                .cloned()
                .ok_or_else(|| format!("the schema has no member {part:?} at {path:?}"))?;
            n = resolve(&root, &next);
        }
    }
    let mut o = Obj::new().str("path", &path);
    for k in [
        "type",
        "items_type",
        "enum",
        "default",
        "examples",
        "minimum",
        "maximum",
        "required",
        "deprecated",
    ] {
        if let Some(v) = n.get(k).filter(|v| **v != Value::Null) {
            o = o.raw(k, &json::compact(v));
        }
    }
    if let Some(Value::String(d)) = n.get("description") {
        o = o.str("description", d);
    }
    if let Some(Value::Object(props)) = n.get("properties") {
        let mut names: Vec<&(String, Value)> = props.iter().collect();
        names.sort_by(|a, b| a.0.cmp(&b.0));
        let list: Vec<String> = names
            .iter()
            .map(|(k, p)| {
                let p = resolve(&root, p);
                let mut e = Obj::new().str("name", k);
                if let Some(t) = p.get("type") {
                    e = e.raw("type", &json::compact(t));
                }
                if let Some(Value::String(d)) = p.get("description") {
                    e = e.str("description", &crate::prompt::clip(d, 300));
                }
                e.end()
            })
            .collect();
        o = o.raw("members", &format!("[{}]", list.join(",")));
    }
    Ok(o.end())
}

fn errors_json(errs: &akit_config::Errors) -> String {
    let mut list: Vec<String> = errs
        .0
        .iter()
        .take(50)
        .map(|e| {
            let mut o = Obj::new().str_omit("path", &e.path);
            if e.line > 0 {
                o = o.num("line", e.line).num("col", e.col);
            }
            o.str("msg", &e.msg).end()
        })
        .collect();
    if errs.0.len() > 50 {
        list.push(
            Obj::new()
                .str(
                    "msg",
                    &format!("and {} more", errs.0.len().saturating_sub(50)),
                )
                .end(),
        );
    }
    format!("[{}]", list.join(","))
}

fn config_validate(c: &Call<'_>) -> Result<String, String> {
    let text = c.str("text")?;
    let patch = c.args.get("patch").filter(|p| **p != Value::Null);
    if text.is_empty() == patch.is_none() {
        return Err("give either text or patch".to_owned());
    }
    if !text.is_empty() {
        if text.len() > MAX_TOOL_TEXT_BYTES {
            return Err(format!(
                "the config is larger than {MAX_TOOL_TEXT_BYTES} bytes"
            ));
        }
        let r = match c.str("format")?.as_str() {
            "" | "json" => akit_config::from_json(&text, None),
            "yaml" => akit_config::from_yaml(&text, None),
            "conf" => akit_config::from_conf(&text, None),
            f => return Err(format!("format is conf, yaml or json, not {f:?}")),
        };
        return Ok(match r {
            Ok(_) => r#"{"valid":true}"#.to_owned(),
            Err(e) => format!(r#"{{"valid":false,"errors":{}}}"#, errors_json(&e)),
        });
    }
    let Some(Value::Object(patch)) = patch else {
        return Err("patch is not a JSON object".to_owned());
    };
    let base = config_json(c, "base")?;
    let Value::Object(mut cur) = base else {
        return Err("the running config is not a JSON object".to_owned());
    };
    for (k, v) in patch {
        cur.retain(|(ck, _)| ck != k);
        if *v != Value::Null {
            cur.push((k.clone(), v.clone()));
        }
    }
    let data = json::compact(&Value::Object(cur));
    if data.len() > MAX_TOOL_TEXT_BYTES * 4 {
        return Err("the config is too large".to_owned());
    }
    c.a.api_json("POST", "/config/validate", data.into_bytes())
}

/// The changes from `a` to `b` at `path`.
fn diff(path: &str, a: &Value, b: &Value, out: &mut Vec<String>) {
    if out.len() > MAX_DIFF_ENTRIES {
        return;
    }
    let at = |p: &str, k: &str| {
        if p.is_empty() {
            k.to_owned()
        } else {
            format!("{p}.{k}")
        }
    };
    match (a, b) {
        (Value::Object(am), Value::Object(bm)) => {
            let mut keys: BTreeSet<&String> = am.iter().map(|(k, _)| k).collect();
            keys.extend(bm.iter().map(|(k, _)| k));
            for k in keys {
                if out.len() > MAX_DIFF_ENTRIES {
                    return;
                }
                let sub = at(path, k);
                match (a.get(k), b.get(k)) {
                    (None, Some(bv)) => out.push(
                        Obj::new()
                            .str("path", &sub)
                            .str("op", "added")
                            .raw("to", &json::compact(bv))
                            .end(),
                    ),
                    (Some(av), None) => out.push(
                        Obj::new()
                            .str("path", &sub)
                            .str("op", "removed")
                            .raw("from", &json::compact(av))
                            .end(),
                    ),
                    (Some(av), Some(bv)) => diff(&sub, av, bv, out),
                    (None, None) => {}
                }
            }
        }
        (Value::Array(al), Value::Array(bl)) if al.len() == bl.len() => {
            for (i, (x, y)) in al.iter().zip(bl).enumerate() {
                diff(&format!("{path}[{i}]"), x, y, out);
            }
        }
        _ => {
            let (ca, cb) = (json::compact(a), json::compact(b));
            if ca != cb {
                out.push(
                    Obj::new()
                        .str("path", path)
                        .str("op", "changed")
                        .raw("from", &ca)
                        .raw("to", &cb)
                        .end(),
                );
            }
        }
    }
}

fn versions_diff(c: &Call<'_>) -> Result<String, String> {
    let from = c.uint("from")?;
    let mut to = c.uint("to")?;
    let list = c.a.api_json("GET", "/versions", Vec::new())?;
    if from == 0 {
        return Ok(list);
    }
    if to == 0 {
        to = akit_config::value::parse(&list)
            .ok()
            .and_then(|v| match v.get("current") {
                Some(Value::Number(n)) => n.parse().ok(),
                _ => None,
            })
            .unwrap_or(0);
    }
    let red = c.a.redactor(c.settings);
    let get = |n: u64| -> Result<Value, String> {
        let data = c.a.api_json("GET", &format!("/versions/{n}"), Vec::new())?;
        let r = red
            .json_text(&data, HIDDEN)
            .ok_or_else(|| "the version is not JSON".to_owned())?;
        akit_config::value::parse(&r).map_err(|_| "the version is not JSON".to_owned())
    };
    let (a, b) = (get(from)?, get(to)?);
    let mut changes = Vec::new();
    diff("", &a, &b, &mut changes);
    let more = changes.len() > MAX_DIFF_ENTRIES;
    changes.truncate(MAX_DIFF_ENTRIES);
    let mut o = Obj::new()
        .num("from", from)
        .num("to", to)
        .raw("changes", &format!("[{}]", changes.join(",")));
    if more {
        o = o.bool("more", true);
    }
    Ok(o.end())
}

fn status(c: &Call<'_>) -> Result<Value, String> {
    let body = c.a.api_json("GET", "/status", Vec::new())?;
    akit_config::value::parse(&body).map_err(|_| "the status is not JSON".to_owned())
}

/// The config's members that start roles.
const ROLE_MEMBERS: &[&str] = &[
    "llm.hub",
    "llm.node",
    "llm.proxy",
    "tunnel.hub",
    "tunnel.node",
    "tunnel.proxy",
    "webrtc.turn",
    "webrtc.media",
    "webrtc.proxy",
    "streaming",
    "media_library",
    "bittorrent.client",
    "bittorrent.tracker",
];

fn status_get(c: &Call<'_>) -> Result<String, String> {
    let st = status(c)?;
    let Value::Object(mut ms) = st else {
        return Err("the status is not a JSON object".to_owned());
    };
    // Upstreams in short: upstream_health has them whole; certs_list has the certificates.
    for (k, v) in &mut ms {
        if k == "upstreams" {
            if let Value::Array(ups) = v {
                let short: Vec<Value> = ups
                    .iter()
                    .map(|u| {
                        let backends = match u.get("backends") {
                            Some(Value::Array(b)) => b.as_slice(),
                            _ => &[],
                        };
                        let up = backends
                            .iter()
                            .filter(|b| matches!(b.get("up"), Some(Value::Bool(true))))
                            .count();
                        Value::Object(vec![
                            ("name".into(), u.get("name").cloned().unwrap_or(Value::Null)),
                            ("backends".into(), Value::Number(backends.len().to_string())),
                            ("up".into(), Value::Number(up.to_string())),
                        ])
                    })
                    .collect();
                *v = Value::Array(short);
            }
        }
    }
    ms.retain(|(k, _)| k != "certificates");
    let cfg = config_json(c, "effective").unwrap_or(Value::Null);
    let roles: Vec<Value> = ROLE_MEMBERS
        .iter()
        .filter(|m| {
            let mut v = Some(&cfg);
            for p in m.split('.') {
                v = v.and_then(|x| x.get(p));
            }
            v.is_some_and(|x| *x != Value::Null)
        })
        .map(|m| Value::String((*m).to_owned()))
        .collect();
    ms.push(("roles".into(), Value::Array(roles)));
    ms.push((
        "uptime_seconds".into(),
        Value::Number(c.a.0.started.elapsed().as_secs().to_string()),
    ));
    Ok(json::compact(&Value::Object(ms)))
}

/// A sample's name and labels, from a line of Prometheus' text format.
fn sample(line: &str) -> (&str, Vec<(String, String)>) {
    let name_end = line.find(['{', ' ']).unwrap_or(line.len());
    let name = line.get(..name_end).unwrap_or("");
    let mut labels = Vec::new();
    if let Some(rest) = line.get(name_end..).and_then(|r| r.strip_prefix('{')) {
        let mut rest = rest;
        while let Some((k, after)) = rest.split_once("=\"") {
            let mut val = String::new();
            let mut esc = false;
            let mut end = after.len();
            for (i, ch) in after.char_indices() {
                if esc {
                    val.push(match ch {
                        'n' => '\n',
                        c => c,
                    });
                    esc = false;
                } else if ch == '\\' {
                    esc = true;
                } else if ch == '"' {
                    end = i;
                    break;
                } else {
                    val.push(ch);
                }
            }
            labels.push((k.trim_start_matches(',').to_owned(), val));
            rest = after.get(end.saturating_add(1)..).unwrap_or("");
            if rest.starts_with('}') {
                break;
            }
        }
    }
    (name, labels)
}

fn metrics_query(c: &Call<'_>) -> Result<String, String> {
    let (name, route, upstream, listener) = (
        c.str("name")?,
        c.str("route")?,
        c.str("upstream")?,
        c.str("listener")?,
    );
    let buckets = c.flag("buckets");
    let mut limit = c.uint("limit")?;
    if limit == 0 {
        limit = DEFAULT_METRIC_LINES;
    }
    let limit = usize::try_from(limit.min(MAX_METRIC_LINES)).unwrap_or(1000);
    let text = c.a.api_json("GET", "/metrics", Vec::new())?;
    let mut out = Vec::new();
    let mut more = false;
    for line in text.lines() {
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let (n, labels) = sample(line);
        let label = |k: &str| {
            labels
                .iter()
                .find(|(l, _)| l == k)
                .map_or("", |(_, v)| v.as_str())
        };
        if (!name.is_empty() && !n.contains(name.as_str()))
            || (!buckets && n.ends_with("_bucket"))
            || (!route.is_empty() && label("route") != route)
            || (!upstream.is_empty() && label("upstream") != upstream)
            || (!listener.is_empty() && label("listener") != listener)
        {
            continue;
        }
        if out.len() >= limit {
            more = true;
            break;
        }
        out.push(quoted(line));
    }
    let mut o = Obj::new().raw("samples", &format!("[{}]", out.join(",")));
    if more {
        o = o.bool("more", true);
    }
    Ok(o.end())
}

fn upstream_health(c: &Call<'_>) -> Result<String, String> {
    let name = c.str("upstream")?;
    let st = status(c)?;
    let ups: Vec<&Value> = match st.get("upstreams") {
        Some(Value::Array(a)) => a
            .iter()
            .filter(|u| name.is_empty() || json::str_of(u, "name") == name)
            .collect(),
        _ => Vec::new(),
    };
    if !name.is_empty() && ups.is_empty() {
        return Err(format!("no upstream is named {name:?}"));
    }
    let list: Vec<String> = ups.into_iter().map(json::compact).collect();
    Ok(format!("{{\"upstreams\":[{}]}}", list.join(",")))
}

fn consistency_check(c: &Call<'_>) -> Result<String, String> {
    c.a.api_json("GET", "/config/check", Vec::new())
}

fn certs_list(c: &Call<'_>) -> Result<String, String> {
    let st = status(c)?;
    let certs = st
        .get("certificates")
        .map_or_else(|| "[]".to_owned(), json::compact);
    let cfg = config_json(c, "effective").unwrap_or(Value::Null);
    let red = c.a.redactor(c.settings);
    let pick = |k: &str| {
        cfg.get(k)
            .map_or_else(|| "null".to_owned(), |v| red.json(v, &[]))
    };
    Ok(Obj::new()
        .raw("certificates", &certs)
        .raw("acme", &pick("acme"))
        .raw("auto_https", &pick("auto_https"))
        .end())
}

fn cache_zones(c: &Call<'_>) -> Result<String, String> {
    c.a.api_json("GET", "/cache", Vec::new())
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]
    use super::*;

    #[test]
    fn tools_are_well_formed() {
        for t in read_tools() {
            assert!(crate::registry::valid_name(t.name), "{}", t.name);
            let p = akit_config::value::parse(t.parameters).unwrap();
            assert_eq!(json::str_of(&p, "type"), "object", "{}", t.name);
            assert!(crate::registry::GROUPS.contains(&t.group));
            assert_eq!(t.kind == Kind::Pool, t.run.is_some());
        }
    }

    #[test]
    fn samples_parsed() {
        let (n, l) = sample(r#"akit_requests_total{route="a\"b",upstream="u"} 3"#);
        assert_eq!(n, "akit_requests_total");
        assert_eq!(
            l,
            vec![
                ("route".into(), "a\"b".into()),
                ("upstream".into(), "u".into())
            ]
        );
        assert_eq!(sample("up 1").0, "up");
    }

    #[test]
    fn diffs() {
        let a = akit_config::value::parse(r#"{"x": 1, "y": [1, 2], "z": {"k": "a"}}"#).unwrap();
        let b = akit_config::value::parse(r#"{"x": 2, "y": [1, 3], "w": true}"#).unwrap();
        let mut out = Vec::new();
        diff("", &a, &b, &mut out);
        let s = out.join("\n");
        assert!(s.contains(r#""path":"w","op":"added""#), "{s}");
        assert!(
            s.contains(r#""path":"x","op":"changed","from":1,"to":2"#),
            "{s}"
        );
        assert!(s.contains(r#""path":"y[1]""#) && s.contains(r#""path":"z","op":"removed""#));
    }
}
