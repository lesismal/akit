//! `GET /logs`, `POST /logs/rotate` and `POST /logs/cleanup` (I83), as fit's `admin/logs.go`:
//! every file output of akit's own log and of the access log, with its loggers, format, zone,
//! rotation and the files on disk; one output rotated now; what its retention removes, removed or
//! only reported.
//!
//! Each reads a directory or waits for a log file's writer, so each runs on the API engine's
//! logical pool, never on its I/O thread (rule 1). Only the paths the running config writes are
//! listed, rotated or cleaned up, and only the names their rotations made are ever removed.

use std::fmt::Write as _;
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use akio_http::{Context, Response};
use akit_config::value::{Value, write_string};
use akit_config::{AccessLog, Config, Log, LogFormat, Rotate};
use akit_observe::Files;
use akit_observe::files::Info;

use crate::server::{Shared, error, json, read_body};

/// The most files listed for one output; past it the answer says `"truncated": true`.
pub(crate) const MAX_LISTED: usize = 1000;

/// One file output of the running config.
struct Output {
    path: String,
    loggers: Vec<&'static str>,
    access_log: bool,
    format: &'static str,
    zone: String,
    rotate: Option<Rotate>,
}

fn format_name(f: LogFormat) -> &'static str {
    match f {
        LogFormat::Line => "line",
        LogFormat::Text => "text",
        LogFormat::Json => "json",
    }
}

/// The file outputs of `c`: the log's (fit's `FileOutputs`), then the access log's.
fn outputs(log: Option<&Log>, access: Option<&AccessLog>) -> Vec<Output> {
    let mut outs = Vec::new();
    if let Some(l) = log {
        for o in l.file_outputs() {
            let mut format = "";
            for name in &o.loggers {
                let f = format_name(l.format_of(name));
                if format.is_empty() {
                    format = f;
                } else if format != f {
                    format = "mixed";
                }
            }
            outs.push(Output {
                path: o.path.clone(),
                loggers: o.loggers.clone(),
                access_log: false,
                format,
                zone: zone_label(&l.time_zone),
                rotate: o.rotate.clone(),
            });
        }
    }
    if let Some(a) = access.filter(|a| akit_config::is_file_output(&a.output)) {
        outs.push(Output {
            path: a.output.clone(),
            loggers: vec!["access_log"],
            access_log: true,
            format: format_name(a.format),
            zone: zone_label(&a.time_zone),
            rotate: a.rotate.clone(),
        });
    }
    outs
}

/// The name a `time_zone` is shown by, as fit's `TimeZoneName`: `local` when left out.
fn zone_label(tz: &str) -> String {
    match tz.trim() {
        "" => "local".to_owned(),
        t => t.to_owned(),
    }
}

/// The running config, as it runs.
fn running(shared: &Shared) -> Option<Config> {
    akit_config::from_json(shared.configs.current().effective(), None).ok()
}

/// `GET /logs`: `{"outputs": [...]}`, on the pool.
pub(crate) fn list(shared: &Arc<Shared>, ctx: &mut Context) {
    let shared = Arc::clone(shared);
    ctx.detach_to_pool(move |d| d.respond(answer_list(&shared)));
}

fn answer_list(shared: &Shared) -> Response {
    let Some(c) = running(shared) else {
        return json(200, "{\"outputs\":[]}\n".to_owned());
    };
    let infos: Vec<Info> = shared.logs.as_ref().map(Files::list).unwrap_or_default();
    let mut b = String::from("{\"outputs\":[");
    for (i, o) in outputs(c.log.as_ref(), c.access_log.as_ref())
        .iter()
        .enumerate()
    {
        if i > 0 {
            b.push(',');
        }
        let key = akit_config::output_path(&o.path);
        let info = infos.iter().find(|i| i.path == key);
        output_json(&mut b, o, info);
    }
    b.push_str("]}\n");
    json(200, b)
}

fn output_json(b: &mut String, o: &Output, info: Option<&Info>) {
    b.push_str("{\"path\":");
    write_string(b, &o.path);
    b.push_str(",\"loggers\":[");
    for (i, l) in o.loggers.iter().enumerate() {
        if i > 0 {
            b.push(',');
        }
        write_string(b, l);
    }
    b.push(']');
    if o.access_log {
        b.push_str(",\"access_log\":true");
    }
    b.push_str(",\"format\":");
    write_string(b, o.format);
    b.push_str(",\"time_zone\":");
    write_string(b, &o.zone);
    b.push_str(",\"rotate\":");
    match akit_config::rotation(o.rotate.as_ref()) {
        None => b.push_str("null"),
        Some(r) => {
            let _ = write!(
                b,
                "{{\"max_bytes\":{},\"interval_seconds\":{},\"max_age_seconds\":{},\"max_files\":{},\"max_total_bytes\":{},\"compress\":{}}}",
                r.max_bytes,
                r.interval.as_secs(),
                r.max_age.as_secs(),
                r.max_files,
                r.max_total_bytes,
                r.compress
            );
        }
    }
    let (current, size, dropped) = match info {
        Some(i) => (i.path.as_str(), i.current_size, i.dropped),
        None => ("", 0, 0),
    };
    b.push_str(",\"current\":");
    write_string(b, current);
    let _ = write!(b, ",\"current_size\":{size},\"dropped\":{dropped}");
    if let Some(e) = info
        .map(|i| i.last_error.as_str())
        .filter(|e| !e.is_empty())
    {
        b.push_str(",\"last_error\":");
        write_string(b, e);
    }
    b.push_str(",\"files\":[");
    let files = info.map(|i| i.files.as_slice()).unwrap_or_default();
    let mut total: u64 = 0;
    for (n, f) in files.iter().take(MAX_LISTED).enumerate() {
        if n > 0 {
            b.push(',');
        }
        total = total.saturating_add(f.size);
        b.push_str("{\"name\":");
        write_string(b, &f.name);
        let _ = write!(b, ",\"size\":{},\"modified\":", f.size);
        time_json(b, f.modified);
        let _ = write!(b, ",\"current\":{}}}", f.current);
    }
    b.push(']');
    if files.len() > MAX_LISTED {
        b.push_str(",\"truncated\":true");
        for f in files.iter().skip(MAX_LISTED) {
            total = total.saturating_add(f.size);
        }
    }
    let _ = write!(b, ",\"total_bytes\":{total}}}");
}

/// A time as RFC 3339 in UTC with nanoseconds, as Go's `time.Time` marshals; `null` for none.
fn time_json(b: &mut String, t: Option<SystemTime>) {
    let Some(t) = t else {
        b.push_str("null");
        return;
    };
    let mut v = Vec::with_capacity(32);
    match t.duration_since(UNIX_EPOCH) {
        Ok(d) => akit_observe::time::rfc3339_nano(
            &mut v,
            i64::try_from(d.as_secs()).unwrap_or(i64::MAX),
            d.subsec_nanos(),
            0,
        ),
        Err(_) => akit_observe::time::rfc3339_nano(&mut v, 0, 0, 0),
    }
    b.push('"');
    b.push_str(&String::from_utf8_lossy(&v));
    b.push('"');
}

/// The body both operations take: `{"path", "dry_run"}`.
fn request(data: &[u8]) -> Result<(String, bool), String> {
    let text = std::str::from_utf8(data).map_err(|_| "not UTF-8".to_owned())?;
    let v = akit_config::value::parse(text).map_err(|errs| {
        errs.as_slice()
            .first()
            .map_or_else(|| "not JSON".to_owned(), |e| e.msg.clone())
    })?;
    let Value::Object(members) = v else {
        return Err("not a JSON object".to_owned());
    };
    let mut path = String::new();
    let mut dry_run = false;
    for (name, v) in members {
        match (name.as_str(), v) {
            ("path", Value::String(s)) => path = s,
            ("dry_run", Value::Bool(b)) => dry_run = b,
            ("path" | "dry_run", Value::Null) => {}
            ("path", _) => return Err("path is a string".to_owned()),
            ("dry_run", _) => return Err("dry_run is true or false".to_owned()),
            _ => {}
        }
    }
    Ok((path, dry_run))
}

/// The sink of `path`, when it is one of the running config's file outputs: no other file is
/// rotated or cleaned up (fit's `logFiles`); otherwise why not.
fn checked(shared: &Shared, path: &str) -> Result<akit_observe::LogFile, String> {
    const NONE: &str = "akit runs no log file sinks";
    let files = shared.logs.as_ref().ok_or_else(|| NONE.to_owned())?;
    let c = running(shared).ok_or_else(|| NONE.to_owned())?;
    let key = akit_config::output_path(path);
    let ours = outputs(c.log.as_ref(), c.access_log.as_ref())
        .iter()
        .any(|o| akit_config::output_path(&o.path) == key);
    if !ours {
        return Err(format!(
            "{path} is not a log file of the running config: only the paths it writes are listed or removed"
        ));
    }
    files
        .get(path)
        .ok_or_else(|| format!("{path} is not open: a new log file takes a restart"))
}

/// `POST /logs/rotate` (`rotate` true) and `POST /logs/cleanup`: the body read, the work on the
/// pool.
pub(crate) fn operate(shared: &Arc<Shared>, ctx: &mut Context, rotate: bool) {
    let shared2 = Arc::clone(shared);
    read_body(shared, ctx, &["application/json"], move |ctx, data| {
        let (path, dry_run) = match request(&data) {
            Ok(r) => r,
            Err(msg) => return ctx.respond(error(400, &format!("the body: {msg}"))),
        };
        if path.is_empty() {
            return ctx.respond(error(400, "name the log file's path"));
        }
        ctx.detach_to_pool(move |d| {
            let resp = match checked(&shared2, &path) {
                Err(msg) => error(400, &msg),
                Ok(file) if rotate => match file.rotate_now() {
                    Ok(()) => json(200, "{}\n".to_owned()),
                    Err(e) => error(500, &e),
                },
                Ok(file) => match file.cleanup_now(dry_run) {
                    Ok(removed) => {
                        let mut b = String::from("{\"removed\":[");
                        for (i, r) in removed.iter().enumerate() {
                            if i > 0 {
                                b.push(',');
                            }
                            b.push_str("{\"name\":");
                            write_string(&mut b, &r.name);
                            let _ = write!(b, ",\"size\":{},\"modified\":", r.size);
                            time_json(&mut b, Some(r.modified));
                            b.push('}');
                        }
                        b.push_str("]}\n");
                        json(200, b)
                    }
                    Err(e) => error(500, &e),
                },
            };
            d.respond(resp);
        });
    });
}

/// The most `GET /logs/recent` answers at once: the ring's size, fit's `MaxRecentLogs`.
pub(crate) const MAX_RECENT_LOGS: usize = akit_observe::logger::RING_SIZE;

/// A query's parameters, `+` and percent escapes decoded, as Go's `url.Query` reads them: the
/// first value of each name.
pub(crate) fn query_params(query: &str) -> Vec<(String, String)> {
    let mut out: Vec<(String, String)> = Vec::new();
    for kv in query.split('&').filter(|s| !s.is_empty()) {
        let (k, v) = kv.split_once('=').unwrap_or((kv, ""));
        let dec = |s: &str| akio_http::message::percent_decode(&s.replace('+', " "));
        if let (Some(k), Some(v)) = (dec(k), dec(v)) {
            if !out.iter().any(|(n, _)| *n == k) {
                out.push((k, v));
            }
        }
    }
    out
}

/// `GET /logs/recent?logger=&level=&contains=&limit=&after=` (I139c), fit's `recentLogs`: the
/// latest messages of akit's own log the filter lets through, oldest first, at most `limit` (100
/// when not given, [`MAX_RECENT_LOGS`] at most), those numbered after `after` when it is given;
/// with the number of the latest the ring holds, which the next request asks after, and the
/// ring's size. akit's log keeps them in memory, a fixed ring: nothing is read from a file.
pub(crate) fn recent(query: &str) -> Response {
    let q = query_params(query);
    let get = |name: &str| {
        q.iter()
            .find(|(k, _)| k == name)
            .map_or("", |(_, v)| v.as_str())
    };
    let mut f = akit_observe::logger::Filter {
        logger: get("logger").to_owned(),
        contains: get("contains").to_owned(),
        ..akit_observe::logger::Filter::default()
    };
    match get("level") {
        "" => {}
        l => match akit_observe::logger::Level::parse(l) {
            Some(level) => f.level = Some(level),
            None => {
                let mut quoted = String::new();
                write_string(&mut quoted, l);
                return error(
                    400,
                    &format!("level is debug, info, warn or error, not {quoted}"),
                );
            }
        },
    }
    if f.contains.len() > 256 || f.logger.len() > 64 {
        return error(400, "contains is 256 bytes at most, and logger 64");
    }
    for (name, max) in [
        ("limit", MAX_RECENT_LOGS as u64),
        ("after", (1u64 << 63) - 1),
    ] {
        let v = get(name);
        if v.is_empty() {
            continue;
        }
        match v.parse::<u64>() {
            Ok(n) if n <= max && v.bytes().all(|c| c.is_ascii_digit()) => {
                if name == "limit" {
                    f.limit = usize::try_from(n).unwrap_or(MAX_RECENT_LOGS);
                } else {
                    f.after = n;
                }
            }
            _ => return error(400, &format!("{name} is a whole number, at most {max}")),
        }
    }
    let mut b = String::from("{\"records\":[");
    let mut last = 0;
    if let Some(root) = akit_observe::logger::installed() {
        last = root.last();
        for (i, r) in root.recent(&f).iter().enumerate() {
            if i > 0 {
                b.push(',');
            }
            b.push_str(&r.json());
        }
    }
    let _ = writeln!(
        b,
        "],\"last\":{last},\"ring_size\":{}}}",
        akit_observe::logger::RING_SIZE
    );
    json(200, b)
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]

    use akit_observe::files::{FileInfo, Info};

    use super::{MAX_LISTED, Output, output_json, request};

    #[test]
    fn a_long_listing_is_cut_and_says_so() {
        let o = Output {
            path: "/l/a.log".into(),
            loggers: vec!["server", "tls"],
            access_log: false,
            format: "mixed",
            zone: "UTC".into(),
            rotate: None,
        };
        let info = Info {
            path: "/l/a.log".into(),
            current_size: 5,
            dropped: 2,
            last_error: "write: disk full".into(),
            files: (0..=MAX_LISTED)
                .map(|i| FileInfo {
                    name: format!("a.log.{i}"),
                    size: 10,
                    modified: None,
                    current: i == 0,
                })
                .collect(),
        };
        let mut b = String::new();
        output_json(&mut b, &o, Some(&info));
        assert!(b.starts_with(r#"{"path":"/l/a.log","loggers":["server","tls"],"format":"mixed","time_zone":"UTC","rotate":null,"current":"/l/a.log","current_size":5,"dropped":2,"last_error":"write: disk full","files":[{"name":"a.log.0","size":10,"modified":null,"current":true}"#), "{b}");
        assert_eq!(b.matches("\"name\":").count(), MAX_LISTED);
        assert!(
            b.ends_with(r#""truncated":true,"total_bytes":10010}"#),
            "{b}"
        );
    }

    #[test]
    fn requests() {
        assert_eq!(
            request(br#"{"path": "/l/a.log", "dry_run": true, "x": 1}"#).unwrap(),
            ("/l/a.log".to_owned(), true)
        );
        assert_eq!(
            request(br#"{"path": null}"#).unwrap(),
            (String::new(), false)
        );
        for bad in [
            &b"[]"[..],
            br#"{"path": 1}"#,
            br#"{"dry_run": "yes"}"#,
            b"nope",
        ] {
            assert!(request(bad).is_err(), "{}", String::from_utf8_lossy(bad));
        }
    }
}
