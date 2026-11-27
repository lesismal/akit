//! The compatibility gate (docs/compatibility.md): the pinned fixtures and the field inventory
//! are sound, the harness judges outcomes as decision 002 says, akit-config reads every field
//! `supported.txt` marks and refuses by inventory names the rest, and every fixture goes
//! through akit-config with the outcomes the checked-in report (`gate-report.txt`) records.

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::Path;

use akit_compat::{
    Field, Finding, Kind, NO_VALIDATE, Outcome, REPORT, Report, TEMPLATE_FORMATS, compat_dir,
    fields, fixtures, fixtures_dir, inventory_path, path_matches, pins, run_gate, supported,
};
use akit_config::{Config, Destination, Handler, Protocol};

// The samples of the roles' fields (I98).
#[path = "support/roles.rs"]
mod role_samples;

#[test]
fn fixtures_are_pinned_and_complete() {
    let pins = pins().unwrap();
    for repo in ["fit", "fit-examples"] {
        let hash = pins
            .get(repo)
            .unwrap_or_else(|| panic!("PINS has no {repo}"));
        assert!(
            hash.len() == 40 && hash.bytes().all(|b| b.is_ascii_hexdigit()),
            "{repo}={hash}"
        );
    }
    let all = fixtures().unwrap();
    let templates = all.iter().filter(|f| f.kind != Kind::Example).count();
    let examples = all.iter().filter(|f| f.kind == Kind::Example).count();
    assert_eq!(templates, 2 * TEMPLATE_FORMATS.len());
    // fit-examples' own test refuses fewer than 50.
    assert!(examples >= 50, "only {examples} examples");
    for (name, _) in NO_VALIDATE {
        assert!(
            all.iter().any(|f| f.name == format!("examples/{name}")),
            "no example {name}"
        );
    }
    for licence in ["fit/LICENSE", "fit-examples/LICENSE"] {
        let text = fs::read_to_string(fixtures_dir().join(licence)).unwrap();
        assert!(text.contains("Apache License"), "{licence}");
    }
}

#[test]
fn template_formats_have_the_same_files() {
    let stems = |format: &str| -> BTreeSet<String> {
        let dir = fixtures_dir()
            .join("fit/templates")
            .join(format)
            .join("conf.d");
        let mut out = BTreeSet::new();
        walk(&dir, &dir, &mut out);
        out
    };
    let conf = stems("conf");
    assert!(conf.len() >= 8, "{conf:?}");
    assert_eq!(conf, stems("json"));
    assert_eq!(conf, stems("yaml"));
}

fn walk(root: &Path, dir: &Path, out: &mut BTreeSet<String>) {
    for e in fs::read_dir(dir).unwrap() {
        let p = e.unwrap().path();
        if p.is_dir() {
            walk(root, &p, out);
        } else {
            let rel = p.strip_prefix(root).unwrap().with_extension("");
            out.insert(rel.to_string_lossy().into_owned());
        }
    }
}

#[test]
fn examples_hold_only_configs() {
    let dir = fixtures_dir().join("fit-examples");
    for e in fs::read_dir(&dir).unwrap() {
        let p = e.unwrap().path();
        let name = p.file_name().unwrap().to_string_lossy().into_owned();
        if p.is_dir() {
            let files: Vec<_> = fs::read_dir(&p)
                .unwrap()
                .map(|e| e.unwrap().file_name())
                .collect();
            assert_eq!(files, ["fit.conf"], "{name}");
        } else {
            assert_eq!(name, "LICENSE");
        }
    }
}

#[test]
fn inventory_is_sound() {
    let fields = fields().unwrap();
    assert!(fields.len() > 1000, "only {} fields", fields.len());
    let mut seen = BTreeSet::new();
    for f in &fields {
        assert!(seen.insert(f.path.as_str()), "duplicate {}", f.path);
        assert!(
            !f.section.is_empty() && !f.ty.is_empty() && !f.flags.is_empty(),
            "{f:?}"
        );
    }
    for top in ["listeners", "routes", "upstreams", "admin", "log"] {
        assert!(seen.contains(top), "no {top}");
    }
    assert!(seen.contains("routes[].handler<respond>.params.status"));
    // The header names the fit commit the fixtures are pinned at.
    let text = fs::read_to_string(compat_dir().join("fit-fields.txt")).unwrap();
    let pin = pins().unwrap()["fit"].clone();
    assert!(
        text.lines().take(3).any(|l| l.contains(&pin[..8])),
        "fit-fields.txt is not from fit {pin}"
    );
    for s in supported().unwrap() {
        assert!(
            seen.contains(s.as_str()),
            "supported.txt: {s} is not in fit-fields.txt"
        );
    }
}

#[test]
fn paths_match_the_inventory_notation() {
    let inv = "routes[].handler<respond>.params.status";
    assert!(path_matches(inv, inv));
    assert!(path_matches(inv, "routes[0].handler.params.status"));
    assert!(path_matches(
        inv,
        "routes[12].handler<respond>.params.status"
    ));
    assert!(!path_matches(inv, "routes[0].handler.params"));
    assert!(!path_matches(inv, "routes[0].handler.params.body"));
    assert!(path_matches(
        "log.loggers.*.level",
        "log.loggers.engine.level"
    ));
    assert!(!path_matches("log.loggers.*.level", "log.loggers.engine"));
}

#[test]
fn findings_read_from_messages() {
    assert_eq!(
        Finding::from_message("not supported yet: routes[0].handler.params.status"),
        Finding::Unsupported("routes[0].handler.params.status".to_owned())
    );
    assert_eq!(
        Finding::from_message("fit.conf:12: not supported yet: `acme.email`"),
        Finding::Unsupported("acme.email".to_owned())
    );
    assert_eq!(
        Finding::from_message("unknown field x"),
        Finding::Error("unknown field x".to_owned())
    );
}

/// The harness on the real fixtures with stand-in validators: what passes and what fails.
#[test]
fn gate_judges_outcomes() {
    let fixtures = fixtures().unwrap();
    let inventory = fields().unwrap();

    // Refusing everything as not supported, by a field fit accepts: the gate passes.
    let report = run_gate(&fixtures, &inventory, |_| {
        Err(vec![Finding::Unsupported("acme.email".into())])
    });
    assert!(report.failures().is_empty(), "{report}");
    assert_eq!(
        report.unsupported().into_iter().collect::<Vec<_>>(),
        ["acme.email"]
    );
    assert!(matches!(
        report.outcomes["examples/https"],
        Outcome::Skipped(_)
    ));
    assert_eq!(report.outcomes.len(), fixtures.len());

    // Valid everywhere: passes.
    let report = run_gate(&fixtures, &inventory, |_| Ok(()));
    assert!(report.failures().is_empty());

    // "not supported yet" for a field fit does not have: fails.
    let report = run_gate(&fixtures, &inventory, |_| {
        Err(vec![Finding::Unsupported("routes[0].nope".into())])
    });
    assert_eq!(report.failures().len(), fixtures.len() - NO_VALIDATE.len());

    // Any other error, or a refusal with nothing said: fails.
    let report = run_gate(&fixtures, &inventory, |_| {
        Err(vec![Finding::Error("unexpected token".into())])
    });
    assert_eq!(report.failures().len(), fixtures.len() - NO_VALIDATE.len());
    let report = run_gate(&fixtures, &inventory, |_| Err(vec![]));
    assert_eq!(report.failures().len(), fixtures.len() - NO_VALIDATE.len());
}

/// The gate itself: every fixture through akit-config, judged, and the report compared line for
/// line with the checked-in one. `make compat-bless` (`AKIT_COMPAT_BLESS=1`) rewrites the
/// checked-in report when the gate passes, for the commit that changes an outcome.
#[test]
fn gate_through_akit_config() {
    let inventory = fields().unwrap();
    let report = run_gate(&fixtures().unwrap(), &inventory, validate_with_akit_config);
    let mut problems: Vec<String> = report
        .failures()
        .iter()
        .flat_map(|(name, why)| why.iter().map(move |w| format!("{name}: {w}")))
        .collect();
    // A field marked supported is read: refusing it as not supported yet contradicts the mark.
    let supported = supported().unwrap();
    for path in report.unsupported() {
        if supported.contains(path) {
            problems.push(format!(
                "{path} is refused as not supported yet but supported.txt marks it supported"
            ));
        }
    }
    let text = report_file(&report);
    let path = compat_dir().join(REPORT);
    if std::env::var_os("AKIT_COMPAT_BLESS").is_some() {
        assert!(
            problems.is_empty(),
            "not writing a report with failures:\n{}",
            problems.join("\n")
        );
        fs::write(&path, &text).unwrap();
        println!("wrote {}", path.display());
        return;
    }
    let checked_in = fs::read_to_string(&path).unwrap_or_default();
    if checked_in != text {
        problems.push(format!(
            "the outcomes differ from tests/compat/{REPORT} (- checked in, + now); if the \
             change is meant, run make compat-bless and commit the report:\n{}",
            report.diff(&checked_in).join("\n")
        ));
        if report.diff(&checked_in).is_empty() {
            problems.push("only the report's header or order differs".to_owned());
        }
    }
    println!("{}", summary(&report));
    assert!(problems.is_empty(), "{}", problems.join("\n"));
}

fn summary(report: &Report) -> String {
    let [valid, unsupported, no_reader, skipped, failed] = report.counts();
    format!(
        "{} fixtures: {valid} valid, {unsupported} refused only as not supported yet (fields refused: {}), \
         {no_reader} in a format not supported yet, {skipped} skipped, {failed} failed",
        report.outcomes.len(),
        report.unsupported().len()
    )
}

/// The checked-in report: a header, then [`Report::render`]'s lines.
fn report_file(report: &Report) -> String {
    let pins = pins().unwrap();
    let mut out = format!(
        "# The compatibility gate's report (docs/compatibility.md), written by make compat-bless;\n\
         # do not edit. The gate fails when a run's outcomes differ from these lines.\n\
         # fit {}, fit-examples {}.\n# {}.\n",
        pins.get("fit").and_then(|p| p.get(..8)).unwrap(),
        pins.get("fit-examples").and_then(|p| p.get(..8)).unwrap(),
        summary(report)
    );
    for line in report.render() {
        out.push_str(&line);
        out.push('\n');
    }
    out
}

/// The one hook into akit-config: read the config with the reader of its format, and turn each
/// problem into a finding. Every format fit reads has a reader since I23 and I24;
/// [`Finding::NoReader`] stays for a format a later fit adds.
///
/// What validation opens (I23): the config's own files only. The files a config names (TLS
/// certificates, the examples' WebAssembly plugins, static roots) are named by fields akit-config
/// refuses as not supported yet, so it opens none of them, and the examples are read from where
/// they are pinned, not from fit-examples' root; the stand-in secrets `make compat-gate` sets are
/// read by no field akit supports either. When a field naming a file is supported, its check
/// decides whether validation opens the file, relative to the directory fit-examples runs from.
fn validate_with_akit_config(path: &Path) -> Result<(), Vec<Finding>> {
    if path.is_dir() {
        // A directory's files, merged as fit merges them (I06), each in its format: JSON, .conf
        // (I23) and YAML (I24) are all read. A directory with no config file is a problem.
        akit_config::config_files(&[path])
            .map_err(|errs| errs.as_slice().iter().map(finding).collect::<Vec<_>>())?;
    } else if !matches!(
        path.extension().and_then(|e| e.to_str()),
        Some("json" | "conf" | "yaml" | "yml")
    ) {
        return Err(vec![Finding::Error(format!(
            "{}: not a config file fit reads",
            path.display()
        ))]);
    }
    akit_config::from_files(&[path])
        .map(|_| ())
        .map_err(|errs| errs.as_slice().iter().map(finding).collect())
}

/// An akit-config problem as a finding. "not supported yet" names its field by the problem's
/// path; a handler or middleware module akit does not have is refused at `<handler>.module` as
/// `the "<module>" handler` (or `middleware`), which the inventory writes `<handler><<module>>`;
/// a field of a module's params that another module akit reads also has is refused as
/// `the "<module>" middleware's <field>: ...` (or `handler's`, I110), at the field of
/// `<ref><<module>>.params`; any other
/// "not supported yet: <what>" is a value of the field at the path (a protocol), or what its
/// absence asks for, reported with `what`.
fn finding(e: &akit_config::Error) -> Finding {
    if !e.is_not_supported_yet() {
        return Finding::Error(e.to_string());
    }
    let quoted = e
        .msg
        .strip_prefix(akit_config::NOT_SUPPORTED_YET)
        .and_then(|r| r.strip_prefix(": the \""));
    let module = quoted.and_then(|r| {
        r.strip_suffix("\" handler")
            .or_else(|| r.strip_suffix("\" middleware"))
    });
    // A field of a module's params: the path with the module's tag.
    if let Some((m, _)) = quoted.and_then(|r| {
        r.split_once("\" middleware's ")
            .or_else(|| r.split_once("\" handler's "))
    }) {
        if let Some((at, field)) = e.path.split_once(".params.") {
            return Finding::Unsupported(format!("{at}<{m}>.params.{field}"));
        }
    }
    let what = e
        .msg
        .strip_prefix(akit_config::NOT_SUPPORTED_YET)
        .and_then(|r| r.strip_prefix(": "))
        .filter(|w| !w.is_empty());
    match (e.path.strip_suffix(".module"), module, what) {
        (Some(handler), Some(m), _) => Finding::Unsupported(format!("{handler}<{m}>")),
        // A value of a field akit reads, or what the field's absence asks of fit.
        (_, _, Some(what)) => Finding::UnsupportedValue {
            path: e.path.clone(),
            what: what.to_owned(),
        },
        _ => Finding::Unsupported(e.path.clone()),
    }
}

/// Every field `supported.txt` marks, in a config akit-config reads, and what it reads from it:
/// a field marked supported that akit refuses, or reads and drops, fails here.
#[test]
fn supported_fields_are_read() {
    type Check = fn(&Config) -> bool;
    const L: &str = r#""listeners": [{"name": "web", "address": ":8080"}]"#;
    let respond = |params: &str| {
        format!(
            r#"{{{L}, "routes": [{{"handler": {{"module": "respond", "params": {{{params}}}}}}}]}}"#
        )
    };
    let statics = |params: &str| {
        format!(
            r#"{{{L}, "routes": [{{"handler": {{"module": "static", "params": {{"root": "/srv/www"{params}}}}}}}]}}"#
        )
    };
    let fastcgi = |params: &str| {
        format!(
            r#"{{{L}, "routes": [{{"handler": {{"module": "fastcgi", "params": {{"address": "127.0.0.1:9000", "root": "/srv/www"{params}}}}}}}]}}"#
        )
    };
    let upstream = |fields: &str| {
        format!(r#"{{"upstreams": [{{"name": "app", "addresses": ["127.0.0.1:9000"]{fields}}}]}}"#)
    };
    let proxy = |params: &str| {
        format!(
            r#"{{{L}, "upstreams": [{{"name": "app", "addresses": ["127.0.0.1:9000"]}}], "routes": [{{"handler": {{"module": "proxy", "params": {{"upstream": "app"{params}}}}}}}]}}"#
        )
    };
    // A proxy with two upstreams, app and b, and only `params` (I46).
    let split = |params: &str| {
        format!(
            r#"{{{L}, "upstreams": [{{"name": "app", "addresses": ["127.0.0.1:9000"]}}, {{"name": "b", "addresses": ["127.0.0.1:9001"]}}], "routes": [{{"handler": {{"module": "proxy", "params": {{{params}}}}}}}]}}"#
        )
    };
    let route = |fields: &str| {
        format!(r#"{{{L}, "routes": [{{{fields}, "handler": {{"module": "respond"}}}}]}}"#)
    };
    let tls = |more: &str| {
        format!(
            r#"{{"listeners": [{{"name": "s", "address": ":443", "tls": {{"certificates": [{{"cert_file": "a.pem", "key_file": "a.key"}}]{more}}}}}]}}"#
        )
    };
    let access = |fields: &str| format!("{{\"access_log\": {{{fields}}}}}");
    // A kubernetes provider serving Ingresses, with `fields` (I59).
    let k8s = |fields: &str| {
        format!(r#"{{"providers": [{{"kubernetes": {{"ingress": {{}}{fields}}}}}]}}"#)
    };
    // No listeners: HTTPS by default makes them, or with `listeners`, written ones.
    let auto_https = |fields: &str| {
        format!(
            r#"{{"auto_https": {{{fields}}}, "routes": [{{"match": {{"host": ["a.example"]}}, "handler": {{"module": "respond"}}}}]}}"#
        )
    };
    let health = |fields: &str| upstream(&format!(r#", "health_check": {{{fields}}}"#));
    let sticky = |fields: &str| upstream(&format!(r#", "sticky_cookie": {{{fields}}}"#));
    let mw = |module: &str, params: &str| {
        format!(
            r#"{{{L}, "routes": [{{"middleware": [{{"module": "{module}", "params": {{{params}}}}}], "handler": {{"module": "respond"}}}}]}}"#
        )
    };
    let rewrite = |params: &str| mw("rewrite", params);
    let redirect = |params: &str| mw("redirect", params);
    let headers = |params: &str| mw("headers", params);
    let rate = |params: &str| mw("rate_limit", params);
    let concurrency = |params: &str| mw("concurrency_limit", params);
    let body = |params: &str| mw("body_limit", params);
    const H: &str = "$2a$04$5xt5UjNNUVWTqOgf9FDZm.bfWVyXg.U2as/rOFV.Ny5IQstc/jC26";
    let basic = |more: &str| {
        mw(
            "basic_auth",
            &format!(r#""users": [{{"username": "alice", "password_hash": "{H}"}}]{more}"#),
        )
    };
    let cert = |params: &str| mw("client_cert", params);
    let forward = |more: &str| mw("forward_auth", &format!(r#""upstream": "auth"{more}"#));
    let breaker = |params: &str| mw("circuit_breaker", params);
    let compress = |params: &str| mw("compress", params);
    let cache = |params: &str| mw("cache", params);
    let wasm = |more: &str| mw("wasm", &format!(r#""path": "p.wasm"{more}"#));
    let jwt = |more: &str| mw("jwt", &format!(r#""hmac_secret_file": "s"{more}"#));
    let service = |fields: &str| {
        format!(
            r#"{{"listeners": [{{"name": "t", "address": ":7000", "protocols": ["tcp"]}}], "upstreams": [{{"name": "app", "addresses": ["127.0.0.1:9000"]}}], "services": [{{"name": "svc", "listeners": ["t"], "upstream": "app"{fields}}}]}}"#
        )
    };
    // I55: a listener's socks, and the forward_proxy handler's params, holding `fields`.
    let socks = |fields: &str| {
        format!(
            r#"{{"listeners": [{{"name": "s", "address": ":1080", "protocols": ["socks"], "socks": {{"clients": ["10.0.0.0/8"]{fields}}}}}]}}"#
        )
    };
    let fproxy = |fields: &str| {
        format!(
            r#"{{{L}, "routes": [{{"handler": {{"module": "forward_proxy", "params": {{"clients": ["10.0.0.0/8"]{fields}}}}}}}]}}"#
        )
    };
    let samples: Vec<(&str, String, Check)> = vec![
        ("schema_version", r#"{"schema_version": 1}"#.into(), |c| {
            c.schema_version == 1
        }),
        ("access_log", access(""), |c| c.access_log.is_some()),
        ("access_log.output", access(r#""output": "stderr""#), |c| {
            c.access_log.as_ref().unwrap().output == "stderr"
        }),
        (
            "access_log.flush_interval",
            access(r#""flush_interval": "250ms""#),
            |c| {
                c.access_log.as_ref().unwrap().flush_interval
                    == std::time::Duration::from_millis(250)
            },
        ),
        (
            "access_log.buffer_bytes",
            access(r#""buffer_bytes": 1024"#),
            |c| c.access_log.as_ref().unwrap().buffer_bytes == 1024,
        ),
        ("access_log.level", access(r#""level": "warn""#), |c| {
            c.access_log.as_ref().unwrap().min_status() == 400
        }),
        ("access_log.format", access(r#""format": "line""#), |c| {
            c.access_log.as_ref().unwrap().format == akit_config::LogFormat::Line
        }),
        (
            "access_log.time_zone",
            access(r#""time_zone": "utc""#),
            |c| c.access_log.as_ref().unwrap().time_zone == "utc",
        ),
        // I81: the file sink's rotation and retention.
        (
            "access_log.rotate",
            access(r#""rotate": {}"#),
            |c| c.access_log.as_ref().unwrap().rotate.is_some(),
        ),
        (
            "access_log.rotate.max_bytes",
            access(r#""rotate": {"max_bytes": 7}"#),
            |c| rotate(c).max_bytes == Some(7),
        ),
        (
            "access_log.rotate.interval",
            access(r#""rotate": {"interval": "1h"}"#),
            |c| rotate(c).interval == std::time::Duration::from_secs(3600),
        ),
        (
            "access_log.rotate.max_age",
            access(r#""rotate": {"max_age": "1h"}"#),
            |c| rotate(c).max_age == Some(std::time::Duration::from_secs(3600)),
        ),
        (
            "access_log.rotate.max_files",
            access(r#""rotate": {"max_files": 3}"#),
            |c| rotate(c).max_files == Some(3),
        ),
        (
            "access_log.rotate.max_total_bytes",
            access(r#""rotate": {"max_total_bytes": 9}"#),
            |c| rotate(c).max_total_bytes == Some(9),
        ),
        (
            "access_log.rotate.compress",
            access(r#""rotate": {"compress": false}"#),
            |c| rotate(c).compress == Some(false),
        ),
        // I82: akit's own log.
        (
            "log",
            r#"{"log": {}}"#.into(),
            |c| c.log.is_some(),
        ),
        (
            "log.level",
            r#"{"log": {"level": "warn"}}"#.into(),
            |c| log(c).level == Some(akit_config::LogLevel::Warn),
        ),
        (
            "log.levels",
            r#"{"log": {"levels": {"tls": "debug"}}}"#.into(),
            |c| log(c).level_of("tls") == akit_config::LogLevel::Debug,
        ),
        (
            "log.output",
            r#"{"log": {"output": "stderr"}}"#.into(),
            |c| log(c).output == "stderr",
        ),
        (
            "log.format",
            r#"{"log": {"format": "json"}}"#.into(),
            |c| log(c).format == Some(akit_config::LogFormat::Json),
        ),
        (
            "log.time_zone",
            r#"{"log": {"time_zone": "utc"}}"#.into(),
            |c| log(c).time_zone == "utc",
        ),
        (
            "log.loggers",
            r#"{"log": {"loggers": {"tls": {}}}}"#.into(),
            |c| log(c).loggers.len() == 1,
        ),
        (
            "log.loggers.*.level",
            r#"{"log": {"loggers": {"tls": {"level": "error"}}}}"#.into(),
            |c| log(c).level_of("tls") == akit_config::LogLevel::Error,
        ),
        (
            "log.loggers.*.output",
            r#"{"log": {"loggers": {"tls": {"output": "stdout"}}}}"#.into(),
            |c| log(c).output_of("tls") == "stdout",
        ),
        (
            "log.loggers.*.format",
            r#"{"log": {"loggers": {"tls": {"format": "text"}}}}"#.into(),
            |c| log(c).format_of("tls") == akit_config::LogFormat::Text,
        ),
        (
            "log.rotate",
            r#"{"log": {"rotate": {}}}"#.into(),
            |c| log(c).rotate.is_some(),
        ),
        (
            "log.rotate.max_bytes",
            r#"{"log": {"rotate": {"max_bytes": 7}}}"#.into(),
            |c| log(c).rotate.as_ref().unwrap().max_bytes == Some(7),
        ),
        (
            "log.rotate.interval",
            r#"{"log": {"rotate": {"interval": "1h"}}}"#.into(),
            |c| log(c).rotate.as_ref().unwrap().interval == std::time::Duration::from_secs(3600),
        ),
        (
            "log.rotate.max_age",
            r#"{"log": {"rotate": {"max_age": "1h"}}}"#.into(),
            |c| log(c).rotate.as_ref().unwrap().max_age == Some(std::time::Duration::from_secs(3600)),
        ),
        (
            "log.rotate.max_files",
            r#"{"log": {"rotate": {"max_files": 3}}}"#.into(),
            |c| log(c).rotate.as_ref().unwrap().max_files == Some(3),
        ),
        (
            "log.rotate.max_total_bytes",
            r#"{"log": {"rotate": {"max_total_bytes": 9}}}"#.into(),
            |c| log(c).rotate.as_ref().unwrap().max_total_bytes == Some(9),
        ),
        (
            "log.rotate.compress",
            r#"{"log": {"rotate": {"compress": false}}}"#.into(),
            |c| log(c).rotate.as_ref().unwrap().compress == Some(false),
        ),
        (
            "log.loggers.*.rotate",
            r#"{"log": {"loggers": {"tls": {"output": "/l/t.log", "rotate": {}}}}}"#.into(),
            |c| log(c).loggers[0].1.rotate.is_some(),
        ),
        (
            "log.loggers.*.rotate.max_bytes",
            r#"{"log": {"loggers": {"tls": {"output": "/l/t.log", "rotate": {"max_bytes": 7}}}}}"#.into(),
            |c| log(c).loggers[0].1.rotate.as_ref().unwrap().max_bytes == Some(7),
        ),
        (
            "log.loggers.*.rotate.interval",
            r#"{"log": {"loggers": {"tls": {"output": "/l/t.log", "rotate": {"interval": "1h"}}}}}"#.into(),
            |c| log(c).loggers[0].1.rotate.as_ref().unwrap().interval == std::time::Duration::from_secs(3600),
        ),
        (
            "log.loggers.*.rotate.max_age",
            r#"{"log": {"loggers": {"tls": {"output": "/l/t.log", "rotate": {"max_age": "1h"}}}}}"#.into(),
            |c| log(c).loggers[0].1.rotate.as_ref().unwrap().max_age == Some(std::time::Duration::from_secs(3600)),
        ),
        (
            "log.loggers.*.rotate.max_files",
            r#"{"log": {"loggers": {"tls": {"output": "/l/t.log", "rotate": {"max_files": 3}}}}}"#.into(),
            |c| log(c).loggers[0].1.rotate.as_ref().unwrap().max_files == Some(3),
        ),
        (
            "log.loggers.*.rotate.max_total_bytes",
            r#"{"log": {"loggers": {"tls": {"output": "/l/t.log", "rotate": {"max_total_bytes": 9}}}}}"#.into(),
            |c| log(c).loggers[0].1.rotate.as_ref().unwrap().max_total_bytes == Some(9),
        ),
        (
            "log.loggers.*.rotate.compress",
            r#"{"log": {"loggers": {"tls": {"output": "/l/t.log", "rotate": {"compress": false}}}}}"#.into(),
            |c| log(c).loggers[0].1.rotate.as_ref().unwrap().compress == Some(false),
        ),
        // It needs admin, as in fit: refused without it, with fit's message.
        ("metrics", r#"{"admin": {}, "metrics": {}}"#.into(), |c| {
            c.metrics.is_some()
                && akit_config::from_json(r#"{"metrics": {}}"#, None)
                    .is_err_and(|e| e.to_string().contains("set admin too"))
        }),
        ("listeners", format!("{{{L}}}"), |c| c.listeners.len() == 1),
        ("admin", r#"{"admin": {}}"#.into(), |c| {
            c.admin == Some(akit_config::Admin::default())
        }),
        (
            "admin.address",
            r#"{"admin": {"address": "localhost:2020"}}"#.into(),
            |c| c.admin.as_ref().unwrap().address == "localhost:2020",
        ),
        (
            "admin.history",
            r#"{"admin": {"history": 3}}"#.into(),
            |c| c.admin.as_ref().unwrap().history == 3,
        ),
        (
            "admin.max_config_bytes",
            r#"{"admin": {"max_config_bytes": 1024}}"#.into(),
            |c| c.admin.as_ref().unwrap().max_config_bytes == 1024,
        ),
        (
            "admin.max_conns",
            r#"{"admin": {"max_conns": 2}}"#.into(),
            |c| c.admin.as_ref().unwrap().max_conns == 2,
        ),
        (
            "admin.token_file",
            r#"{"admin": {"token_file": "/etc/akit/admin.token"}}"#.into(),
            |c| c.admin.as_ref().unwrap().token_file.as_deref() == Some("/etc/akit/admin.token"),
        ),
        (
            "admin.access",
            r#"{"admin": {"access": {"allow": ["127.0.0.1"]}}}"#.into(),
            |c| c.admin.as_ref().unwrap().access.is_some(),
        ),
        (
            "admin.access.allow",
            r#"{"admin": {"access": {"allow": ["127.0.0.1"]}}}"#.into(),
            |c| c.admin.as_ref().unwrap().access.as_ref().unwrap().allow == ["127.0.0.1"],
        ),
        (
            "admin.access.deny",
            r#"{"admin": {"access": {"deny": ["192.0.2.1"]}}}"#.into(),
            |c| c.admin.as_ref().unwrap().access.as_ref().unwrap().deny == ["192.0.2.1"],
        ),
        (
            "admin.access.allow_files",
            r#"{"admin": {"access": {"allow_files": ["a.txt"]}}}"#.into(),
            |c| c.admin.as_ref().unwrap().access.as_ref().unwrap().allow_files == ["a.txt"],
        ),
        (
            "admin.access.deny_files",
            r#"{"admin": {"access": {"deny_files": ["d.txt"]}}}"#.into(),
            |c| c.admin.as_ref().unwrap().access.as_ref().unwrap().deny_files == ["d.txt"],
        ),
        (
            "admin.disable_dashboard",
            r#"{"admin": {"disable_dashboard": true}}"#.into(),
            |c| c.admin.as_ref().unwrap().disable_dashboard,
        ),
        (
            "admin.cert_file",
            r#"{"admin": {"cert_file": "c.pem", "key_file": "k.pem"}}"#.into(),
            |c| c.admin.as_ref().unwrap().cert_file.as_deref() == Some("c.pem"),
        ),
        (
            "admin.key_file",
            r#"{"admin": {"cert_file": "c.pem", "key_file": "k.pem"}}"#.into(),
            |c| c.admin.as_ref().unwrap().key_file.as_deref() == Some("k.pem"),
        ),
        (
            "admin.client_ca_file",
            r#"{"admin": {"cert_file": "c.pem", "key_file": "k.pem", "client_ca_file": "ca.pem"}}"#
                .into(),
            |c| c.admin.as_ref().unwrap().client_ca_file.as_deref() == Some("ca.pem"),
        ),
        (
            "admin.store",
            r#"{"admin": {"store": {"path": "s.store", "password": "p"}}}"#.into(),
            |c| c.admin.as_ref().unwrap().store.is_some(),
        ),
        (
            "admin.store.path",
            r#"{"admin": {"store": {"path": "s.store", "password": "p"}}}"#.into(),
            |c| c.admin.as_ref().unwrap().store.as_ref().unwrap().path == "s.store",
        ),
        (
            "admin.store.password",
            r#"{"admin": {"store": {"path": "s.store", "password": "p"}}}"#.into(),
            |c| {
                c.admin.as_ref().unwrap().store.as_ref().unwrap().password.as_deref() == Some("p")
            },
        ),
        (
            "admin.store.password_file",
            r#"{"admin": {"store": {"path": "s.store", "password_file": "p.txt"}}}"#.into(),
            |c| {
                c.admin.as_ref().unwrap().store.as_ref().unwrap().password_file.as_deref()
                    == Some("p.txt")
            },
        ),
        (
            "admin.store.password_env",
            r#"{"admin": {"store": {"path": "s.store", "password_env": "P"}}}"#.into(),
            |c| {
                c.admin.as_ref().unwrap().store.as_ref().unwrap().password_env.as_deref()
                    == Some("P")
            },
        ),
        // I130: cluster management.
        (
            "admin.node_id",
            r#"{"admin": {"node_id": "edge-1"}}"#.into(),
            |c| c.admin.as_ref().unwrap().node_id.as_deref() == Some("edge-1"),
        ),
        ("admin.cluster", cluster_sample(""), |c| cluster_of(c).is_some()),
        ("admin.cluster.poll_interval", cluster_sample(r#""poll_interval": "5s""#), |c| {
            cluster_of(c).unwrap().poll_interval == std::time::Duration::from_secs(5)
        }),
        ("admin.cluster.timeout", cluster_sample(r#""timeout": "3s""#), |c| {
            cluster_of(c).unwrap().timeout == std::time::Duration::from_secs(3)
        }),
        ("admin.cluster.max_nodes", cluster_sample(r#""max_nodes": 5"#), |c| {
            cluster_of(c).unwrap().max_nodes == 5
        }),
        ("admin.cluster.max_concurrency", cluster_sample(r#""max_concurrency": 2"#), |c| {
            cluster_of(c).unwrap().max_concurrency == 2
        }),
        ("admin.cluster.max_response_bytes", cluster_sample(r#""max_response_bytes": 65536"#), |c| {
            cluster_of(c).unwrap().max_response_bytes == 65536
        }),
        ("admin.cluster.link_address", cluster_sample(r#""link_address": ":7444""#), |c| {
            cluster_of(c).unwrap().link_address == ":7444"
        }),
        (
            "admin.cluster.link_url",
            cluster_sample(r#""link_address": ":7444", "link_url": "wss://m:7444/cluster/link""#),
            |c| cluster_of(c).unwrap().link_url == "wss://m:7444/cluster/link",
        ),
        ("admin.cluster.admins", cluster_sample(r#""admins": ["a"]"#), |c| {
            cluster_of(c).unwrap().admins == ["a"]
        }),
        ("admin.cluster_member", member_sample(r#""token_file": "/t""#), |c| {
            member_of(c).is_some()
        }),
        ("admin.cluster_member.manager", member_sample(r#""token_file": "/t""#), |c| {
            member_of(c).unwrap().manager == "wss://m:7444/cluster/link"
        }),
        ("admin.cluster_member.fingerprint", member_sample(r#""token_file": "/t""#), |c| {
            member_of(c).unwrap().fingerprint.len() == 64
        }),
        ("admin.cluster_member.token_file", member_sample(r#""token_file": "/t""#), |c| {
            member_of(c).unwrap().token_file == "/t"
        }),
        (
            "admin.cluster_member.token",
            member_sample(&format!(r#""token": "akite1.0123456789abcdef.{}""#, "A".repeat(43))),
            |c| member_of(c).unwrap().token.starts_with("akite1."),
        ),
        ("admin.console", console_sample(""), |c| console_of(c).users.len() == 1),
        ("admin.console.users", console_sample(""), |c| console_of(c).users.len() == 1),
        ("admin.console.users[].username", console_sample(""), |c| {
            console_of(c).users[0].username == "a"
        }),
        ("admin.console.users[].password_hash", console_sample(""), |c| {
            console_of(c).users[0].password_hash == "$2b$04$x"
        }),
        (
            "admin.console.address",
            console_sample(r#", "address": "127.0.0.1:9""#),
            |c| console_of(c).address == "127.0.0.1:9",
        ),
        (
            "admin.console.cert_file",
            console_sample(r#", "cert_file": "c.pem", "key_file": "k.pem""#),
            |c| console_of(c).cert_file.as_deref() == Some("c.pem"),
        ),
        (
            "admin.console.key_file",
            console_sample(r#", "cert_file": "c.pem", "key_file": "k.pem""#),
            |c| console_of(c).key_file.as_deref() == Some("k.pem"),
        ),
        (
            "admin.console.session_timeout",
            console_sample(r#", "session_timeout": "1m""#),
            |c| console_of(c).session_timeout == std::time::Duration::from_secs(60),
        ),
        (
            "admin.console.session_lifetime",
            console_sample(r#", "session_lifetime": "1h""#),
            |c| console_of(c).session_lifetime == std::time::Duration::from_secs(3600),
        ),
        (
            "admin.console.max_sessions",
            console_sample(r#", "max_sessions": 3"#),
            |c| console_of(c).max_sessions == 3,
        ),
        (
            "admin.console.max_conns",
            console_sample(r#", "max_conns": 3"#),
            |c| console_of(c).max_conns == 3,
        ),
        (
            "admin.console.access",
            console_sample(r#", "access": {"allow": ["127.0.0.1"]}"#),
            |c| console_of(c).access.is_some(),
        ),
        (
            "admin.console.access.allow",
            console_sample(r#", "access": {"allow": ["127.0.0.1"]}"#),
            |c| console_of(c).access.as_ref().unwrap().allow == ["127.0.0.1"],
        ),
        (
            "admin.console.access.deny",
            console_sample(r#", "access": {"deny": ["192.0.2.1"]}"#),
            |c| console_of(c).access.as_ref().unwrap().deny == ["192.0.2.1"],
        ),
        (
            "admin.console.access.allow_files",
            console_sample(r#", "access": {"allow_files": ["a.txt"]}"#),
            |c| console_of(c).access.as_ref().unwrap().allow_files == ["a.txt"],
        ),
        (
            "admin.console.access.deny_files",
            console_sample(r#", "access": {"deny_files": ["d.txt"]}"#),
            |c| console_of(c).access.as_ref().unwrap().deny_files == ["d.txt"],
        ),
        ("limits", r#"{"limits": {}}"#.into(), |c| {
            c.limits == akit_config::Limits::default()
        }),
        (
            "limits.max_body_bytes",
            format!(r#"{{{L}, "limits": {{"max_body_bytes": 5}}}}"#),
            |c| c.limits.max_body_bytes == 5 && c.listeners[0].max_body_bytes == 5,
        ),
        (
            "limits.write_stall_timeout",
            format!(r#"{{{L}, "limits": {{"write_stall_timeout": "5s"}}}}"#),
            |c| c.limits.write_stall_timeout == std::time::Duration::from_secs(5),
        ),
        (
            "limits.max_header_bytes",
            format!(r#"{{{L}, "limits": {{"max_header_bytes": 5}}}}"#),
            |c| c.limits.max_header_bytes == 5 && c.listeners[0].max_header_bytes == 5,
        ),
        (
            "limits.max_conns",
            format!(r#"{{{L}, "limits": {{"max_conns": 5}}}}"#),
            |c| c.limits.max_conns == 5,
        ),
        (
            "access",
            format!(r#"{{{L}, "access": {{"allow": ["10.0.0.0/8"]}}}}"#),
            |c| c.access.is_some(),
        ),
        (
            "access.allow",
            format!(r#"{{{L}, "access": {{"allow": ["10.0.0.0/8"]}}}}"#),
            |c| c.access.as_ref().unwrap().allow == ["10.0.0.0/8"],
        ),
        (
            "access.deny",
            format!(r#"{{{L}, "access": {{"deny": [" 192.0.2.1 "]}}}}"#),
            |c| c.access.as_ref().unwrap().deny == [" 192.0.2.1 "],
        ),
        (
            "access.allow_files",
            format!(r#"{{{L}, "access": {{"allow_files": ["a.txt"]}}}}"#),
            |c| c.access.as_ref().unwrap().allow_files == ["a.txt"],
        ),
        (
            "access.deny_files",
            format!(r#"{{{L}, "access": {{"deny_files": ["d.txt"]}}}}"#),
            |c| c.access.as_ref().unwrap().deny_files == ["d.txt"],
        ),
        (
            "listeners[].access",
            r#"{"listeners": [{"name": "web", "address": ":8080", "access": {"allow": ["10.0.0.0/8"]}}]}"#.into(),
            |c| c.listeners[0].access.is_some(),
        ),
        (
            "listeners[].access.allow",
            r#"{"listeners": [{"name": "web", "address": ":8080", "access": {"allow": ["10.0.0.0/8"]}}]}"#.into(),
            |c| c.listeners[0].access.as_ref().unwrap().allow == ["10.0.0.0/8"],
        ),
        (
            "listeners[].access.deny",
            r#"{"listeners": [{"name": "web", "address": ":8080", "access": {"deny": [" 192.0.2.1 "]}}]}"#.into(),
            |c| c.listeners[0].access.as_ref().unwrap().deny == [" 192.0.2.1 "],
        ),
        (
            "listeners[].access.allow_files",
            r#"{"listeners": [{"name": "web", "address": ":8080", "access": {"allow_files": ["a.txt"]}}]}"#.into(),
            |c| c.listeners[0].access.as_ref().unwrap().allow_files == ["a.txt"],
        ),
        (
            "listeners[].access.deny_files",
            r#"{"listeners": [{"name": "web", "address": ":8080", "access": {"deny_files": ["d.txt"]}}]}"#.into(),
            |c| c.listeners[0].access.as_ref().unwrap().deny_files == ["d.txt"],
        ),
        (
            "routes[].access",
            route(r#""access": {"allow": ["10.0.0.0/8"]}"#),
            |c| c.routes[0].access.is_some(),
        ),
        (
            "routes[].access.allow",
            route(r#""access": {"allow": ["10.0.0.0/8"]}"#),
            |c| c.routes[0].access.as_ref().unwrap().allow == ["10.0.0.0/8"],
        ),
        (
            "routes[].access.deny",
            route(r#""access": {"deny": [" 192.0.2.1 "]}"#),
            |c| c.routes[0].access.as_ref().unwrap().deny == [" 192.0.2.1 "],
        ),
        (
            "routes[].access.allow_files",
            route(r#""access": {"allow_files": ["a.txt"]}"#),
            |c| c.routes[0].access.as_ref().unwrap().allow_files == ["a.txt"],
        ),
        (
            "routes[].access.deny_files",
            route(r#""access": {"deny_files": ["d.txt"]}"#),
            |c| c.routes[0].access.as_ref().unwrap().deny_files == ["d.txt"],
        ),
        (
            "services[].access",
            r#"{"listeners": [{"name": "t", "address": ":7000", "protocols": ["tcp"]}], "upstreams": [{"name": "app", "addresses": ["127.0.0.1:9000"]}], "services": [{"name": "a", "listeners": ["t"], "upstream": "app", "access": {"allow": ["10.0.0.0/8"]}}]}"#.into(),
            |c| c.services[0].access.is_some(),
        ),
        (
            "services[].access.allow",
            r#"{"listeners": [{"name": "t", "address": ":7000", "protocols": ["tcp"]}], "upstreams": [{"name": "app", "addresses": ["127.0.0.1:9000"]}], "services": [{"name": "a", "listeners": ["t"], "upstream": "app", "access": {"allow": ["10.0.0.0/8"]}}]}"#.into(),
            |c| c.services[0].access.as_ref().unwrap().allow == ["10.0.0.0/8"],
        ),
        (
            "services[].access.deny",
            r#"{"listeners": [{"name": "t", "address": ":7000", "protocols": ["tcp"]}], "upstreams": [{"name": "app", "addresses": ["127.0.0.1:9000"]}], "services": [{"name": "a", "listeners": ["t"], "upstream": "app", "access": {"deny": ["192.0.2.1"]}}]}"#.into(),
            |c| c.services[0].access.as_ref().unwrap().deny == ["192.0.2.1"],
        ),
        (
            "services[].access.allow_files",
            r#"{"listeners": [{"name": "t", "address": ":7000", "protocols": ["tcp"]}], "upstreams": [{"name": "app", "addresses": ["127.0.0.1:9000"]}], "services": [{"name": "a", "listeners": ["t"], "upstream": "app", "access": {"allow_files": ["a.txt"]}}]}"#.into(),
            |c| c.services[0].access.as_ref().unwrap().allow_files == ["a.txt"],
        ),
        (
            "services[].access.deny_files",
            r#"{"listeners": [{"name": "t", "address": ":7000", "protocols": ["tcp"]}], "upstreams": [{"name": "app", "addresses": ["127.0.0.1:9000"]}], "services": [{"name": "a", "listeners": ["t"], "upstream": "app", "access": {"deny_files": ["d.txt"]}}]}"#.into(),
            |c| c.services[0].access.as_ref().unwrap().deny_files == ["d.txt"],
        ),
        (
            "limits.max_pending_bytes",
            format!(r#"{{{L}, "limits": {{"max_pending_bytes": "25%"}}}}"#),
            |c| c.limits.max_pending_bytes == akit_config::Memory::Share(2500),
        ),
        (
            "limits.max_write_queue_bytes",
            format!(r#"{{{L}, "limits": {{"max_write_queue_bytes": 5}}}}"#),
            |c| c.limits.max_write_queue_bytes == 5,
        ),
        ("engine", r#"{"engine": {}}"#.into(), |c| {
            c.engine == akit_config::Engine::default()
        }),
        (
            "engine.event_loops",
            r#"{"engine": {"event_loops": 3}}"#.into(),
            |c| c.engine.event_loops == 3,
        ),
        (
            "engine.workers",
            r#"{"engine": {"workers": 3}}"#.into(),
            |c| c.engine.workers == 3,
        ),
        (
            "engine.min_workers",
            r#"{"engine": {"min_workers": 3}}"#.into(),
            |c| c.engine.min_workers == 3,
        ),
        (
            "engine.task_queue",
            r#"{"engine": {"task_queue": 3}}"#.into(),
            |c| c.engine.task_queue == 3,
        ),
        (
            "engine.read_buffer_size",
            r#"{"engine": {"read_buffer_size": 4096}}"#.into(),
            |c| c.engine.read_buffer_size == 4096,
        ),
        (
            "engine.write_watermark",
            r#"{"engine": {"write_watermark": 3}}"#.into(),
            |c| c.engine.write_watermark == 3,
        ),
        (
            "engine.backlog",
            r#"{"engine": {"backlog": 3}}"#.into(),
            |c| c.engine.backlog == 3,
        ),
        ("listeners[].name", format!("{{{L}}}"), |c| {
            c.listeners[0].name == "web"
        }),
        ("listeners[].address", format!("{{{L}}}"), |c| {
            c.listeners[0].address == ":8080"
        }),
        (
            "listeners[].protocols",
            r#"{"listeners": [{"name": "w", "address": ":1", "protocols": ["http/1.1", "h2c"]}]}"#.into(),
            |c| c.listeners[0].protocols == [Protocol::Http11, Protocol::H2c],
        ),
        ("listeners[].tls", tls(""), |c| c.listeners[0].tls.is_some()),
        ("listeners[].tls.certificates", tls(""), |c| {
            c.listeners[0].tls.as_ref().unwrap().certificates.len() == 1
        }),
        ("listeners[].tls.certificates[].cert_file", tls(""), |c| {
            c.listeners[0].tls.as_ref().unwrap().certificates[0].cert_file == "a.pem"
        }),
        ("listeners[].tls.certificates[].key_file", tls(""), |c| {
            c.listeners[0].tls.as_ref().unwrap().certificates[0].key_file == "a.key"
        }),
        (
            "listeners[].tls.client_auth",
            tls(r#", "client_auth": {"ca_files": ["ca.pem"]}"#),
            |c| c.listeners[0].tls.as_ref().unwrap().client_auth.is_some(),
        ),
        (
            "listeners[].tls.client_auth.ca_files",
            tls(r#", "client_auth": {"ca_files": ["ca.pem"]}"#),
            |c| {
                c.listeners[0]
                    .tls
                    .as_ref()
                    .unwrap()
                    .client_auth
                    .as_ref()
                    .unwrap()
                    .ca_files
                    == ["ca.pem"]
            },
        ),
        (
            "listeners[].tls.client_auth.mode",
            tls(r#", "client_auth": {"ca_files": ["ca.pem"], "mode": "verify_if_given"}"#),
            |c| {
                c.listeners[0]
                    .tls
                    .as_ref()
                    .unwrap()
                    .client_auth
                    .as_ref()
                    .unwrap()
                    .mode
                    == akit_config::ClientAuthMode::VerifyIfGiven
            },
        ),
        (
            "listeners[].tls.acme",
            tls(r#", "acme": ["a.example"]"#),
            |c| c.listeners[0].tls.as_ref().unwrap().acme == ["a.example"],
        ),
        ("acme", r#"{"acme": {}}"#.into(), |c| {
            c.acme == Some(akit_config::Acme::default())
        }),
        (
            "acme.directory",
            r#"{"acme": {"directory": "https://ca.test/dir"}}"#.into(),
            |c| c.acme.as_ref().unwrap().directory == "https://ca.test/dir",
        ),
        (
            "acme.email",
            r#"{"acme": {"email": "ops@example.com"}}"#.into(),
            |c| c.acme.as_ref().unwrap().email == "ops@example.com",
        ),
        (
            "acme.ca_file",
            r#"{"acme": {"ca_file": "ca.pem"}}"#.into(),
            |c| c.acme.as_ref().unwrap().ca_file == "ca.pem",
        ),
        (
            "acme.storage",
            r#"{"acme": {"storage": "/var/lib/akit"}}"#.into(),
            |c| c.acme.as_ref().unwrap().storage == "/var/lib/akit",
        ),
        (
            "acme.max_concurrent",
            r#"{"acme": {"max_concurrent": 3}}"#.into(),
            |c| c.acme.as_ref().unwrap().max_concurrent == 3,
        ),
        (
            "acme.timeout",
            r#"{"acme": {"timeout": "1m"}}"#.into(),
            |c| c.acme.as_ref().unwrap().timeout == std::time::Duration::from_secs(60),
        ),
        (
            "acme.request_timeout",
            r#"{"acme": {"request_timeout": "5s"}}"#.into(),
            |c| c.acme.as_ref().unwrap().request_timeout == std::time::Duration::from_secs(5),
        ),
        // Both, in the order given (I32).
        (
            "acme.challenges",
            r#"{"acme": {"challenges": ["tls-alpn-01", "http-01"]}}"#.into(),
            |c| {
                c.acme.as_ref().unwrap().challenges
                    == [
                        akit_config::Challenge::TlsAlpn01,
                        akit_config::Challenge::Http01,
                    ]
            },
        ),
        // Shared storage (I62).
        (
            "acme.shared_storage",
            r#"{"acme": {"shared_storage": {"redis": {}}}}"#.into(),
            |c| shared_storage(c).kind.name() == "redis",
        ),
        (
            "acme.shared_storage.redis",
            r#"{"acme": {"shared_storage": {"redis": {}}}}"#.into(),
            |c| redis_storage(c).address == akit_config::DEFAULT_REDIS_ADDRESS,
        ),
        (
            "acme.shared_storage.redis.address",
            r#"{"acme": {"shared_storage": {"redis": {"address": "r:6380"}}}}"#.into(),
            |c| redis_storage(c).address == "r:6380",
        ),
        (
            "acme.shared_storage.redis.username",
            r#"{"acme": {"shared_storage": {"redis": {"username": "u", "password_file": "/p"}}}}"#.into(),
            |c| redis_storage(c).username == "u",
        ),
        (
            "acme.shared_storage.redis.password_file",
            r#"{"acme": {"shared_storage": {"redis": {"password_file": "/p"}}}}"#.into(),
            |c| redis_storage(c).password_file == "/p",
        ),
        (
            "acme.shared_storage.redis.db",
            r#"{"acme": {"shared_storage": {"redis": {"db": 2}}}}"#.into(),
            |c| redis_storage(c).db == 2,
        ),
        (
            "acme.shared_storage.redis.tls",
            r#"{"acme": {"shared_storage": {"redis": {"tls": true}}}}"#.into(),
            |c| redis_storage(c).tls,
        ),
        (
            "acme.shared_storage.redis.ca_file",
            r#"{"acme": {"shared_storage": {"redis": {"tls": true, "ca_file": "/ca"}}}}"#.into(),
            |c| redis_storage(c).tls_files.ca_file == "/ca",
        ),
        (
            "acme.shared_storage.redis.cert_file",
            r#"{"acme": {"shared_storage": {"redis": {"tls": true, "cert_file": "/c", "key_file": "/k"}}}}"#.into(),
            |c| redis_storage(c).tls_files.cert_file == "/c",
        ),
        (
            "acme.shared_storage.redis.key_file",
            r#"{"acme": {"shared_storage": {"redis": {"tls": true, "cert_file": "/c", "key_file": "/k"}}}}"#.into(),
            |c| redis_storage(c).tls_files.key_file == "/k",
        ),
        (
            "acme.shared_storage.etcd",
            r#"{"acme": {"shared_storage": {"etcd": {}}}}"#.into(),
            |c| shared_storage(c).kind.name() == "etcd",
        ),
        (
            "acme.shared_storage.etcd.endpoints",
            r#"{"acme": {"shared_storage": {"etcd": {"endpoints": ["http://e:2379"]}}}}"#.into(),
            |c| etcd_storage(c).endpoints == ["http://e:2379"],
        ),
        (
            "acme.shared_storage.etcd.username",
            r#"{"acme": {"shared_storage": {"etcd": {"username": "u", "password_file": "/p"}}}}"#.into(),
            |c| etcd_storage(c).username == "u",
        ),
        (
            "acme.shared_storage.etcd.password_file",
            r#"{"acme": {"shared_storage": {"etcd": {"username": "u", "password_file": "/p"}}}}"#.into(),
            |c| etcd_storage(c).password_file == "/p",
        ),
        (
            "acme.shared_storage.etcd.ca_file",
            r#"{"acme": {"shared_storage": {"etcd": {"ca_file": "/ca"}}}}"#.into(),
            |c| etcd_storage(c).tls.ca_file == "/ca",
        ),
        (
            "acme.shared_storage.etcd.cert_file",
            r#"{"acme": {"shared_storage": {"etcd": {"cert_file": "/c", "key_file": "/k"}}}}"#.into(),
            |c| etcd_storage(c).tls.cert_file == "/c",
        ),
        (
            "acme.shared_storage.etcd.key_file",
            r#"{"acme": {"shared_storage": {"etcd": {"cert_file": "/c", "key_file": "/k"}}}}"#.into(),
            |c| etcd_storage(c).tls.key_file == "/k",
        ),
        (
            "acme.shared_storage.consul",
            r#"{"acme": {"shared_storage": {"consul": {}}}}"#.into(),
            |c| shared_storage(c).kind.name() == "consul",
        ),
        (
            "acme.shared_storage.consul.endpoint",
            r#"{"acme": {"shared_storage": {"consul": {"endpoint": "http://c:8500"}}}}"#.into(),
            |c| consul_storage(c).endpoint == "http://c:8500",
        ),
        (
            "acme.shared_storage.consul.datacenter",
            r#"{"acme": {"shared_storage": {"consul": {"datacenter": "dc2"}}}}"#.into(),
            |c| consul_storage(c).datacenter == "dc2",
        ),
        (
            "acme.shared_storage.consul.token_file",
            r#"{"acme": {"shared_storage": {"consul": {"token_file": "/t"}}}}"#.into(),
            |c| consul_storage(c).token_file == "/t",
        ),
        (
            "acme.shared_storage.consul.ca_file",
            r#"{"acme": {"shared_storage": {"consul": {"ca_file": "/ca"}}}}"#.into(),
            |c| consul_storage(c).tls.ca_file == "/ca",
        ),
        (
            "acme.shared_storage.consul.cert_file",
            r#"{"acme": {"shared_storage": {"consul": {"cert_file": "/c", "key_file": "/k"}}}}"#.into(),
            |c| consul_storage(c).tls.cert_file == "/c",
        ),
        (
            "acme.shared_storage.consul.key_file",
            r#"{"acme": {"shared_storage": {"consul": {"cert_file": "/c", "key_file": "/k"}}}}"#.into(),
            |c| consul_storage(c).tls.key_file == "/k",
        ),
        (
            "acme.shared_storage.prefix",
            r#"{"acme": {"shared_storage": {"redis": {}, "prefix": "akit/"}}}"#.into(),
            |c| shared_storage(c).prefix == "akit/",
        ),
        (
            "acme.shared_storage.lock_ttl",
            r#"{"acme": {"shared_storage": {"redis": {}, "lock_ttl": "1m"}}}"#.into(),
            |c| shared_storage(c).lock_ttl == std::time::Duration::from_secs(60),
        ),
        (
            "acme.shared_storage.request_timeout",
            r#"{"acme": {"shared_storage": {"redis": {}, "request_timeout": "3s"}}}"#.into(),
            |c| shared_storage(c).request_timeout == std::time::Duration::from_secs(3),
        ),
        // On-demand TLS (I34).
        (
            "acme.on_demand",
            r#"{"acme": {"on_demand": {"ask": "http://127.0.0.1:5555/"}}}"#.into(),
            |c| {
                c.acme.as_ref().unwrap().on_demand
                    == Some(akit_config::OnDemand::new("http://127.0.0.1:5555/"))
            },
        ),
        (
            "acme.on_demand.ask",
            r#"{"acme": {"on_demand": {"ask": "https://ask.test/?k=1"}}}"#.into(),
            |c| c.acme.as_ref().unwrap().on_demand.as_ref().unwrap().ask == "https://ask.test/?k=1",
        ),
        (
            "acme.on_demand.rate",
            r#"{"acme": {"on_demand": {"ask": "http://a/", "rate": 3}}}"#.into(),
            |c| c.acme.as_ref().unwrap().on_demand.as_ref().unwrap().rate == 3,
        ),
        (
            "acme.on_demand.interval",
            r#"{"acme": {"on_demand": {"ask": "http://a/", "interval": "1h"}}}"#.into(),
            |c| {
                c.acme.as_ref().unwrap().on_demand.as_ref().unwrap().interval
                    == std::time::Duration::from_secs(3600)
            },
        ),
        (
            "acme.on_demand.max_names",
            r#"{"acme": {"on_demand": {"ask": "http://a/", "max_names": 7}}}"#.into(),
            |c| c.acme.as_ref().unwrap().on_demand.as_ref().unwrap().max_names == 7,
        ),
        (
            "acme.on_demand.timeout",
            r#"{"acme": {"on_demand": {"ask": "http://a/", "timeout": "30s"}}}"#.into(),
            |c| {
                c.acme.as_ref().unwrap().on_demand.as_ref().unwrap().timeout
                    == std::time::Duration::from_secs(30)
            },
        ),
        (
            "acme.on_demand.idle_timeout",
            r#"{"acme": {"on_demand": {"ask": "http://a/", "idle_timeout": "24h"}}}"#.into(),
            |c| {
                c.acme.as_ref().unwrap().on_demand.as_ref().unwrap().idle_timeout
                    == std::time::Duration::from_secs(86_400)
            },
        ),
        (
            "listeners[].tls.on_demand",
            tls(r#", "on_demand": true"#).replacen(
                '{',
                r#"{"acme": {"on_demand": {"ask": "http://a/"}}, "#,
                1,
            ),
            |c| c.listeners[0].tls.as_ref().unwrap().on_demand,
        ),
        // HTTPS by default (I33): its listeners are made from the routes' host names.
        ("auto_https", auto_https(""), |c| {
            c.auto_https == Some(akit_config::AutoHttps::default()) && c.listeners.len() == 2
        }),
        // Off: listeners must be written.
        (
            "auto_https.disable",
            auto_https(r#""disable": true"#).replacen('{', &format!("{{{L}, "), 1),
            |c| c.auto_https.unwrap().disable && c.listeners.len() == 1,
        ),
        (
            "auto_https.disable_redirects",
            auto_https(r#""disable_redirects": true"#),
            |c| c.listeners.len() == 1 && c.listeners[0].name == "https",
        ),
        (
            "auto_https.http_port",
            auto_https(r#""http_port": 8080"#),
            |c| c.listeners[1].address == ":8080",
        ),
        (
            "auto_https.https_port",
            auto_https(r#""https_port": 8443"#),
            |c| c.listeners[0].address == ":8443" && c.listeners[1].redirect_https == 8443,
        ),
        // 1.2: TLS 1.2 alone over TCP (akio API request A9); 1.3 is the default.
        (
            "listeners[].tls.max_version",
            tls(r#", "max_version": "1.2""#),
            |c| c.listeners[0].tls.as_ref().unwrap().max_version == akit_config::TlsVersion::Tls12,
        ),
        ("routes", respond(""), |c| c.routes.len() == 1),
        ("routes[].name", route(r#""name": "r1""#), |c| {
            c.routes[0].name.as_deref() == Some("r1")
        }),
        (
            "routes[].listeners",
            route(r#""listeners": ["web"]"#),
            |c| c.routes[0].listeners == ["web"],
        ),
        ("routes[].priority", route(r#""priority": 7"#), |c| {
            c.routes[0].priority == 7
        }),
        ("routes[].match", route(r#""match": {}"#), |c| {
            c.routes[0].matcher.is_some()
        }),
        (
            "routes[].match.host",
            route(r#""match": {"host": ["a.example"]}"#),
            |c| c.routes[0].matcher.as_ref().unwrap().host == ["a.example"],
        ),
        (
            "routes[].match.path",
            route(r#""match": {"path": ["/x*"]}"#),
            |c| c.routes[0].matcher.as_ref().unwrap().path == ["/x*"],
        ),
        (
            "routes[].match.path_regexp",
            route(r#""match": {"path_regexp": ["^/a$"]}"#),
            |c| c.routes[0].matcher.as_ref().unwrap().path_regexp == ["^/a$"],
        ),
        (
            "routes[].match.method",
            route(r#""match": {"method": ["GET"]}"#),
            |c| c.routes[0].matcher.as_ref().unwrap().method == ["GET"],
        ),
        (
            "routes[].match.header",
            route(r#""match": {"header": {"X-A": ["1"]}}"#),
            |c| {
                c.routes[0].matcher.as_ref().unwrap().header
                    == [("X-A".to_owned(), vec!["1".to_owned()])]
            },
        ),
        (
            "routes[].match.query",
            route(r#""match": {"query": {"v": []}}"#),
            |c| c.routes[0].matcher.as_ref().unwrap().query == [("v".to_owned(), vec![])],
        ),
        (
            "routes[].match.remote_ip",
            route(r#""match": {"remote_ip": ["10.0.0.0/8"]}"#),
            |c| c.routes[0].matcher.as_ref().unwrap().remote_ip == ["10.0.0.0/8"],
        ),
        (
            "routes[].match.expr",
            route(r#""match": {"expr": "Method(\"GET\")"}"#),
            |c| c.routes[0].matcher.as_ref().unwrap().expr == r#"Method("GET")"#,
        ),
        ("routes[].handler", respond(""), |c| {
            matches!(c.routes[0].handler, Handler::Respond(_))
        }),
        ("routes[].handler.module", respond(""), |c| {
            matches!(c.routes[0].handler, Handler::Respond(_))
        }),
        (
            "routes[].handler.params",
            respond(r#""status": 201"#),
            |c| respond_of(c).status == 201,
        ),
        ("routes[].handler<respond>", respond(""), |c| {
            respond_of(c).status == 200
        }),
        (
            "routes[].handler<respond>.params",
            respond(r#""body": "b""#),
            |c| respond_of(c).body == "b",
        ),
        (
            "routes[].handler<respond>.params.status",
            respond(r#""status": 404"#),
            |c| respond_of(c).status == 404,
        ),
        (
            "routes[].handler<respond>.params.headers",
            respond(r#""headers": {"X-A": "1"}"#),
            |c| respond_of(c).headers == [("X-A".to_owned(), "1".to_owned())],
        ),
        (
            "routes[].handler<respond>.params.body",
            respond(r#""body": "hi""#),
            |c| respond_of(c).body == "hi",
        ),
        ("routes[].handler<static>", statics(""), |c| {
            static_of(c).index == ["index.html"]
        }),
        ("routes[].handler<static>.params", statics(""), |c| {
            static_of(c).root == "/srv/www"
        }),
        ("routes[].handler<static>.params.root", statics(""), |c| {
            static_of(c).root == "/srv/www"
        }),
        (
            "routes[].handler<static>.params.index",
            statics(r#", "index": ["default.htm"]"#),
            |c| static_of(c).index == ["default.htm"],
        ),
        (
            "routes[].handler<static>.params.max_file_bytes",
            statics(r#", "max_file_bytes": 4096"#),
            |c| static_of(c).max_file_bytes == 4096,
        ),
        (
            "routes[].handler<static>.params.max_cache_bytes",
            statics(r#", "max_cache_bytes": 8192"#),
            |c| static_of(c).max_cache_bytes == 8192,
        ),
        (
            "routes[].handler<static>.params.default_type",
            statics(r#", "default_type": "application/octet-stream""#),
            |c| static_of(c).default_type.as_deref() == Some("application/octet-stream"),
        ),
        (
            "routes[].handler<static>.params.dotfiles",
            statics(r#", "dotfiles": "allow""#),
            |c| static_of(c).dotfiles == akit_config::Dotfiles::Allow,
        ),
        // Its one value akit takes is the default, so it reads it by refusing the other.
        (
            "routes[].handler<static>.params.disk_io",
            statics(r#", "disk_io": "pool""#),
            |_| {
                akit_config::from_json(
                    r#"{"listeners": [{"name": "web", "address": ":8080"}], "routes": [{"handler": {"module": "static", "params": {"root": "/", "disk_io": "auto"}}}]}"#,
                    None,
                )
                .is_err_and(|e| e.only_not_supported_yet())
            },
        ),
        (
            "routes[].handler<static>.params.check_interval",
            statics(r#", "check_interval": "0s""#),
            |c| static_of(c).check_interval.is_zero(),
        ),
        (
            "routes[].handler<static>.params.precompressed",
            statics(r#", "precompressed": ["zstd", "gzip"]"#),
            |c| {
                static_of(c).precompressed
                    == [akit_config::Encoding::Zstd, akit_config::Encoding::Gzip]
            },
        ),
        (
            "routes[].handler<static>.params.compress",
            statics(r#", "compress": ["br", "gzip"]"#),
            |c| static_of(c).compress == [akit_config::Encoding::Br, akit_config::Encoding::Gzip],
        ),
        (
            "routes[].handler<static>.params.browse",
            statics(r#", "browse": true"#),
            |c| static_of(c).browse,
        ),
        (
            "routes[].handler<static>.params.browse_max_entries",
            statics(r#", "browse_max_entries": 5"#),
            |c| static_of(c).browse_max_entries == 5,
        ),
        (
            "routes[].handler<static>.params.strip_prefix",
            statics(r#", "strip_prefix": "/assets""#),
            |c| static_of(c).strip_prefix == "/assets",
        ),
        (
            "routes[].handler<static>.params.try_files",
            statics(r#", "try_files": ["{path}", "/index.html"]"#),
            |c| {
                static_of(c).try_files.as_deref()
                    == Some(&["{path}".to_owned(), "/index.html".to_owned()][..])
            },
        ),
        ("routes[].handler<fastcgi>", fastcgi(""), |c| {
            fastcgi_of(c).index == "index.php"
        }),
        ("routes[].handler<fastcgi>.params", fastcgi(""), |c| {
            fastcgi_of(c).split_path == [".php"]
        }),
        ("routes[].handler<fastcgi>.params.address", fastcgi(""), |c| {
            fastcgi_of(c).address
                == akit_config::FastCgiAddress::Tcp {
                    host: "127.0.0.1".to_owned(),
                    port: 9000,
                }
        }),
        ("routes[].handler<fastcgi>.params.root", fastcgi(""), |c| {
            fastcgi_of(c).root == "/srv/www"
        }),
        (
            "routes[].handler<fastcgi>.params.split_path",
            fastcgi(r#", "split_path": [".PHP", ".phtml"]"#),
            |c| fastcgi_of(c).split_path == [".php", ".phtml"],
        ),
        (
            "routes[].handler<fastcgi>.params.index",
            fastcgi(r#", "index": "app.php""#),
            |c| fastcgi_of(c).index == "app.php",
        ),
        (
            "routes[].handler<fastcgi>.params.try_files",
            fastcgi(r#", "try_files": ["{path}", "/index.php"]"#),
            |c| fastcgi_of(c).try_files == ["{path}", "/index.php"],
        ),
        (
            "routes[].handler<fastcgi>.params.dotfiles",
            fastcgi(r#", "dotfiles": "allow""#),
            |c| fastcgi_of(c).dotfiles == akit_config::Dotfiles::Allow,
        ),
        (
            "routes[].handler<fastcgi>.params.env",
            fastcgi(r#", "env": {"APP_ENV": "prod"}"#),
            |c| fastcgi_of(c).env == [("APP_ENV".to_owned(), "prod".to_owned())],
        ),
        (
            "routes[].handler<fastcgi>.params.max_conns",
            fastcgi(r#", "max_conns": 8"#),
            |c| fastcgi_of(c).max_conns == 8,
        ),
        (
            "routes[].handler<fastcgi>.params.max_idle_conns",
            fastcgi(r#", "max_idle_conns": 4"#),
            |c| fastcgi_of(c).max_idle_conns == 4,
        ),
        (
            "routes[].handler<fastcgi>.params.max_pending",
            fastcgi(r#", "max_pending": 9"#),
            |c| fastcgi_of(c).max_pending == 9,
        ),
        (
            "routes[].handler<fastcgi>.params.connect_timeout",
            fastcgi(r#", "connect_timeout": "2s""#),
            |c| fastcgi_of(c).connect_timeout == std::time::Duration::from_secs(2),
        ),
        (
            "routes[].handler<fastcgi>.params.response_header_timeout",
            fastcgi(r#", "response_header_timeout": "3s""#),
            |c| fastcgi_of(c).response_header_timeout == std::time::Duration::from_secs(3),
        ),
        (
            "routes[].handler<fastcgi>.params.response_idle_timeout",
            fastcgi(r#", "response_idle_timeout": "4s""#),
            |c| fastcgi_of(c).response_idle_timeout == std::time::Duration::from_secs(4),
        ),
        (
            "routes[].handler<fastcgi>.params.timeout",
            fastcgi(r#", "timeout": "5s""#),
            |c| fastcgi_of(c).timeout == std::time::Duration::from_secs(5),
        ),
        (
            "routes[].handler<fastcgi>.params.idle_timeout",
            fastcgi(r#", "idle_timeout": "6s""#),
            |c| fastcgi_of(c).idle_timeout == std::time::Duration::from_secs(6),
        ),
        (
            "routes[].handler<fastcgi>.params.max_header_bytes",
            fastcgi(r#", "max_header_bytes": 4096"#),
            |c| fastcgi_of(c).max_header_bytes == 4096,
        ),
        ("routes[].handler<proxy>", proxy(""), |c| {
            proxy_of(c).upstream == "app"
        }),
        ("routes[].handler<proxy>.params", proxy(""), |c| {
            proxy_of(c).trusted_proxies.is_empty()
        }),
        ("routes[].handler<proxy>.params.upstream", proxy(""), |c| {
            proxy_of(c).upstream == "app"
        }),
        (
            "routes[].handler<proxy>.params.trusted_proxies",
            proxy(r#", "trusted_proxies": ["10.0.0.0/8", "192.0.2.1"]"#),
            |c| {
                proxy_of(c)
                    .trusted_proxies
                    .iter()
                    .map(ToString::to_string)
                    .collect::<Vec<_>>()
                    == ["10.0.0.0/8", "192.0.2.1/32"]
            },
        ),
        (
            "routes[].handler<proxy>.params.host_header",
            proxy(r#", "host_header": "internal.svc""#),
            |c| proxy_of(c).host_header == "internal.svc",
        ),
        (
            "routes[].handler<proxy>.params.timeout",
            proxy(r#", "timeout": "45s""#),
            |c| proxy_of(c).timeout == Some(std::time::Duration::from_secs(45)),
        ),
        (
            "routes[].handler<proxy>.params.retry",
            proxy(r#", "retry": {}"#),
            |c| proxy_of(c).retry == Some(akit_config::Retry::default()),
        ),
        (
            "routes[].handler<proxy>.params.retry.attempts",
            proxy(r#", "retry": {"attempts": 4}"#),
            |c| proxy_of(c).retry.as_ref().unwrap().attempts == 4,
        ),
        (
            "routes[].handler<proxy>.params.retry.statuses",
            proxy(r#", "retry": {"statuses": [502, 503]}"#),
            |c| proxy_of(c).retry.as_ref().unwrap().statuses == [502, 503],
        ),
        (
            "routes[].handler<proxy>.params.retry.budget_ratio",
            proxy(r#", "retry": {"budget_ratio": 0.5}"#),
            |c| proxy_of(c).retry.as_ref().unwrap().budget_ratio == 0.5,
        ),
        (
            "routes[].handler<proxy>.params.retry.budget_min_per_second",
            proxy(r#", "retry": {"budget_min_per_second": 3}"#),
            |c| proxy_of(c).retry.as_ref().unwrap().budget_min_per_second == 3.0,
        ),
        (
            "routes[].handler<proxy>.params.upstreams",
            split(
                r#""upstreams": [{"upstream": "app", "weight": 3}, {"upstream": "b", "weight": 1}]"#,
            ),
            |c| proxy_of(c).upstreams.len() == 2,
        ),
        (
            "routes[].handler<proxy>.params.upstreams[].upstream",
            split(r#""upstreams": [{"upstream": "b", "weight": 1}]"#),
            |c| proxy_of(c).upstreams[0].upstream == "b",
        ),
        (
            "routes[].handler<proxy>.params.upstreams[].weight",
            split(r#""upstreams": [{"upstream": "b", "weight": 7}]"#),
            |c| proxy_of(c).upstreams[0].weight == 7,
        ),
        (
            "routes[].handler<proxy>.params.failover",
            split(r#""upstream": "app", "failover": "b""#),
            |c| proxy_of(c).failover.as_deref() == Some("b"),
        ),
        (
            "routes[].handler<proxy>.params.mirror",
            split(r#""upstream": "app", "mirror": {"upstream": "b"}"#),
            |c| proxy_of(c).mirror.as_ref().unwrap().percent == 100.0,
        ),
        (
            "routes[].handler<proxy>.params.mirror.upstream",
            split(r#""upstream": "app", "mirror": {"upstream": "b"}"#),
            |c| proxy_of(c).mirror.as_ref().unwrap().upstream == "b",
        ),
        (
            "routes[].handler<proxy>.params.mirror.percent",
            split(r#""upstream": "app", "mirror": {"upstream": "b", "percent": 10}"#),
            |c| proxy_of(c).mirror.as_ref().unwrap().percent == 10.0,
        ),
        (
            "routes[].handler<proxy>.params.tunnel_idle_timeout",
            proxy(r#", "tunnel_idle_timeout": "90s""#),
            |c| proxy_of(c).tunnel_idle_timeout == std::time::Duration::from_secs(90),
        ),
        (
            "routes[].middleware",
            rewrite(r#""add_prefix": "/v2""#),
            |c| c.routes[0].middleware.len() == 1,
        ),
        (
            "routes[].middleware[].module",
            redirect(r#""to": "/x""#),
            |c| c.routes[0].middleware[0].module() == "redirect",
        ),
        (
            "routes[].middleware[].params",
            rewrite(r#""add_prefix": "/v2""#),
            |c| rewrite_of(c).add_prefix == "/v2",
        ),
        (
            "routes[].middleware[]<rewrite>",
            rewrite(r#""add_prefix": "/v2""#),
            |c| rewrite_of(c).strip_prefix.is_empty(),
        ),
        (
            "routes[].middleware[]<rewrite>.params",
            rewrite(r#""add_prefix": "/v2""#),
            |c| rewrite_of(c).add_prefix == "/v2",
        ),
        (
            "routes[].middleware[]<rewrite>.params.strip_prefix",
            rewrite(r#""strip_prefix": ["/api", "/b/"]"#),
            |c| rewrite_of(c).strip_prefix == ["/api", "/b/"],
        ),
        (
            "routes[].middleware[]<rewrite>.params.regexp",
            rewrite(r#""regexp": "^/old/(.*)$", "replacement": "/new/$1""#),
            |c| rewrite_of(c).regexp == "^/old/(.*)$",
        ),
        (
            "routes[].middleware[]<rewrite>.params.replacement",
            rewrite(r#""regexp": "^/old/(.*)$", "replacement": "/new/$1""#),
            |c| rewrite_of(c).replacement == "/new/$1",
        ),
        (
            "routes[].middleware[]<rewrite>.params.add_prefix",
            rewrite(r#""add_prefix": "/v2""#),
            |c| rewrite_of(c).add_prefix == "/v2",
        ),
        (
            "routes[].middleware[]<redirect>",
            redirect(r#""to": "/x""#),
            |c| redirect_of(c).status == 302,
        ),
        (
            "routes[].middleware[]<redirect>.params",
            redirect(r#""to": "/x""#),
            |c| redirect_of(c).regexp.is_empty(),
        ),
        (
            "routes[].middleware[]<redirect>.params.to",
            redirect(r#""to": "https://{host}{uri}""#),
            |c| redirect_of(c).to == "https://{host}{uri}",
        ),
        (
            "routes[].middleware[]<redirect>.params.status",
            redirect(r#""to": "/x", "status": 308"#),
            |c| redirect_of(c).status == 308,
        ),
        (
            "routes[].middleware[]<redirect>.params.regexp",
            redirect(r#""to": "/x", "regexp": "^/old/""#),
            |c| redirect_of(c).regexp == "^/old/",
        ),
        (
            "routes[].middleware[]<headers>",
            headers(r#""request": {"delete": ["X-A"]}"#),
            |c| headers_of(c).response.is_empty(),
        ),
        (
            "routes[].middleware[]<headers>.params",
            headers(r#""request": {"delete": ["X-A"]}"#),
            |c| !headers_of(c).request.is_empty(),
        ),
        (
            "routes[].middleware[]<headers>.params.request",
            headers(r#""request": {"delete": ["X-A"]}"#),
            |c| headers_of(c).request.delete == ["X-A"],
        ),
        (
            "routes[].middleware[]<headers>.params.request.set",
            headers(r#""request": {"set": {"X-B": "1", "X-A": "2"}}"#),
            |c| {
                headers_of(c).request.set
                    == [
                        ("X-A".to_owned(), "2".to_owned()),
                        ("X-B".to_owned(), "1".to_owned()),
                    ]
            },
        ),
        (
            "routes[].middleware[]<headers>.params.request.add",
            headers(r#""request": {"add": {"X-A": ["1", "2"]}}"#),
            |c| {
                headers_of(c).request.add
                    == [("X-A".to_owned(), vec!["1".to_owned(), "2".to_owned()])]
            },
        ),
        (
            "routes[].middleware[]<headers>.params.request.delete",
            headers(r#""request": {"delete": ["X-A", "X-Debug-*"]}"#),
            |c| headers_of(c).request.delete == ["X-A", "X-Debug-*"],
        ),
        (
            "routes[].middleware[]<headers>.params.response",
            headers(r#""response": {"delete": ["Server"]}"#),
            |c| headers_of(c).request.is_empty() && !headers_of(c).response.is_empty(),
        ),
        (
            "routes[].middleware[]<headers>.params.response.set",
            headers(r#""response": {"set": {"Server": "akit"}}"#),
            |c| headers_of(c).response.set == [("Server".to_owned(), "akit".to_owned())],
        ),
        (
            "routes[].middleware[]<headers>.params.response.add",
            headers(r#""response": {"add": {"Vary": ["Accept"]}}"#),
            |c| headers_of(c).response.add == [("Vary".to_owned(), vec!["Accept".to_owned()])],
        ),
        (
            "routes[].middleware[]<headers>.params.response.delete",
            headers(r#""response": {"delete": ["*"]}"#),
            |c| headers_of(c).response.delete == ["*"],
        ),
        (
            "routes[].middleware[]<rate_limit>",
            rate(r#""rate": 1"#),
            |c| rate_of(c).burst == 1,
        ),
        (
            "routes[].middleware[]<rate_limit>.params",
            rate(r#""rate": 2.5"#),
            |c| rate_of(c).burst == 3 && rate_of(c).status == 429,
        ),
        (
            "routes[].middleware[]<rate_limit>.params.rate",
            rate(r#""rate": 0.5"#),
            |c| rate_of(c).rate.to_bits() == 0.5f64.to_bits(),
        ),
        (
            "routes[].middleware[]<rate_limit>.params.burst",
            rate(r#""rate": 1, "burst": 7"#),
            |c| rate_of(c).burst == 7,
        ),
        (
            "routes[].middleware[]<rate_limit>.params.key",
            rate(r#""rate": 1, "key": "header:X-Api-Key""#),
            |c| rate_of(c).key == akit_config::RateKey::Header("X-Api-Key".into()),
        ),
        (
            "routes[].middleware[]<rate_limit>.params.trusted_proxies",
            rate(r#""rate": 1, "trusted_proxies": ["10.0.0.0/8"]"#),
            |c| rate_of(c).trusted_proxies.len() == 1,
        ),
        (
            "routes[].middleware[]<rate_limit>.params.ipv6_prefix",
            rate(r#""rate": 1, "ipv6_prefix": 48"#),
            |c| rate_of(c).ipv6_prefix == 48,
        ),
        (
            "routes[].middleware[]<rate_limit>.params.max_keys",
            rate(r#""rate": 1, "max_keys": 640"#),
            |c| rate_of(c).max_keys == 640,
        ),
        (
            "routes[].middleware[]<rate_limit>.params.status",
            rate(r#""rate": 1, "status": 503"#),
            |c| rate_of(c).status == 503,
        ),
        (
            "routes[].middleware[]<concurrency_limit>",
            concurrency(r#""max": 1"#),
            |c| matches!(c.routes[0].middleware[0], Handler::ConcurrencyLimit(_)),
        ),
        (
            "routes[].middleware[]<concurrency_limit>.params",
            concurrency(r#""max": 1"#),
            |c| concurrency_of(c).status == 429,
        ),
        (
            "routes[].middleware[]<concurrency_limit>.params.max",
            concurrency(r#""max": 64"#),
            |c| concurrency_of(c).max == 64,
        ),
        (
            "routes[].middleware[]<concurrency_limit>.params.status",
            concurrency(r#""max": 1, "status": 503"#),
            |c| concurrency_of(c).status == 503,
        ),
        (
            "routes[].middleware[]<body_limit>",
            body(r#""max_bytes": 0"#),
            |c| matches!(c.routes[0].middleware[0], Handler::BodyLimit(_)),
        ),
        (
            "routes[].middleware[]<body_limit>.params",
            body(r#""max_bytes": 5"#),
            |c| matches!(&c.routes[0].middleware[0], Handler::BodyLimit(b) if b.max_bytes == 5),
        ),
        (
            "routes[].middleware[]<body_limit>.params.max_bytes",
            body(r#""max_bytes": 1048576"#),
            |c| matches!(&c.routes[0].middleware[0], Handler::BodyLimit(b) if b.max_bytes == 1 << 20),
        ),
        ("routes[].middleware[]<basic_auth>", basic(""), |c| {
            matches!(c.routes[0].middleware[0], Handler::BasicAuth(_))
        }),
        ("routes[].middleware[]<basic_auth>.params", basic(""), |c| {
            basic_of(c).realm == "akit" && basic_of(c).user_header.is_empty()
        }),
        (
            "routes[].middleware[]<basic_auth>.params.realm",
            basic(r#", "realm": "staff""#),
            |c| basic_of(c).realm == "staff",
        ),
        (
            "routes[].middleware[]<basic_auth>.params.users",
            basic(""),
            |c| basic_of(c).users.len() == 1,
        ),
        (
            "routes[].middleware[]<basic_auth>.params.users[].username",
            basic(""),
            |c| basic_of(c).users[0].username == "alice",
        ),
        (
            "routes[].middleware[]<basic_auth>.params.users[].password_hash",
            basic(""),
            |c| basic_of(c).users[0].password_hash.starts_with("$2a$04$"),
        ),
        (
            "routes[].middleware[]<basic_auth>.params.user_header",
            basic(r#", "user_header": "X-User""#),
            |c| basic_of(c).user_header == "X-User",
        ),
        (
            "routes[].middleware[]<basic_auth>.params.forward_authorization",
            basic(r#", "forward_authorization": true"#),
            |c| basic_of(c).forward_authorization,
        ),
        ("routes[].middleware[]<client_cert>", cert(""), |c| {
            matches!(c.routes[0].middleware[0], Handler::ClientCert(_))
        }),
        ("routes[].middleware[]<client_cert>.params", cert(""), |c| {
            *cert_of(c) == akit_config::ClientCert::default()
        }),
        (
            "routes[].middleware[]<client_cert>.params.optional",
            cert(r#""optional": true"#),
            |c| cert_of(c).optional,
        ),
        (
            "routes[].middleware[]<client_cert>.params.subject_header",
            cert(r#""subject_header": "X-Subject""#),
            |c| cert_of(c).subject_header == "X-Subject",
        ),
        ("routes[].middleware[]<forward_auth>", forward(""), |c| {
            matches!(c.routes[0].middleware[0], Handler::ForwardAuth(_))
        }),
        (
            "routes[].middleware[]<forward_auth>.params",
            forward(""),
            |c| {
                forward_of(c).uri == "/"
                    && forward_of(c).timeout == std::time::Duration::from_secs(10)
            },
        ),
        (
            "routes[].middleware[]<forward_auth>.params.upstream",
            forward(""),
            |c| forward_of(c).upstream == "auth",
        ),
        (
            "routes[].middleware[]<forward_auth>.params.uri",
            forward(r#", "uri": "/api/verify?app=1""#),
            |c| forward_of(c).uri == "/api/verify?app=1",
        ),
        (
            "routes[].middleware[]<forward_auth>.params.request_headers",
            forward(r#", "request_headers": ["Cookie", "Authorization"]"#),
            |c| forward_of(c).request_headers == ["Cookie", "Authorization"],
        ),
        (
            "routes[].middleware[]<forward_auth>.params.copy_headers",
            forward(r#", "copy_headers": ["Remote-User"]"#),
            |c| forward_of(c).copy_headers == ["Remote-User"],
        ),
        (
            "routes[].middleware[]<forward_auth>.params.timeout",
            forward(r#", "timeout": "5s""#),
            |c| forward_of(c).timeout == std::time::Duration::from_secs(5),
        ),
        ("routes[].middleware[]<circuit_breaker>", breaker(""), |c| {
            matches!(c.routes[0].middleware[0], Handler::CircuitBreaker(_))
        }),
        (
            "routes[].middleware[]<circuit_breaker>.params",
            breaker(""),
            |c| *breaker_of(c) == akit_config::CircuitBreaker::default(),
        ),
        (
            "routes[].middleware[]<circuit_breaker>.params.window",
            breaker(r#""window": "20s""#),
            |c| breaker_of(c).window == std::time::Duration::from_secs(20),
        ),
        (
            "routes[].middleware[]<circuit_breaker>.params.min_requests",
            breaker(r#""min_requests": 7"#),
            |c| breaker_of(c).min_requests == 7,
        ),
        (
            "routes[].middleware[]<circuit_breaker>.params.error_ratio",
            breaker(r#""error_ratio": 0.25"#),
            |c| breaker_of(c).error_ratio == 0.25,
        ),
        (
            "routes[].middleware[]<circuit_breaker>.params.statuses",
            breaker(r#""statuses": [429, 503]"#),
            |c| breaker_of(c).statuses == [429, 503],
        ),
        (
            "routes[].middleware[]<circuit_breaker>.params.slow_threshold",
            breaker(r#""slow_threshold": "2s""#),
            |c| breaker_of(c).slow_threshold == Some(std::time::Duration::from_secs(2)),
        ),
        (
            "routes[].middleware[]<circuit_breaker>.params.slow_ratio",
            breaker(r#""slow_ratio": 0.75"#),
            |c| breaker_of(c).slow_ratio == 0.75,
        ),
        (
            "routes[].middleware[]<circuit_breaker>.params.open_duration",
            breaker(r#""open_duration": "1m""#),
            |c| breaker_of(c).open_duration == std::time::Duration::from_secs(60),
        ),
        (
            "routes[].middleware[]<circuit_breaker>.params.half_open_requests",
            breaker(r#""half_open_requests": 2"#),
            |c| breaker_of(c).half_open_requests == 2,
        ),
        ("routes[].middleware[]<compress>", compress(""), |c| {
            matches!(c.routes[0].middleware[0], Handler::Compress(_))
        }),
        (
            "routes[].middleware[]<compress>.params",
            compress(""),
            |c| compress_of(c).min_bytes == 1400 && compress_of(c).max_in_flight == 1024,
        ),
        (
            "routes[].middleware[]<compress>.params.encodings",
            compress(r#""encodings": ["br", "gzip"]"#),
            |c| {
                compress_of(c).encodings == [akit_config::Encoding::Br, akit_config::Encoding::Gzip]
            },
        ),
        (
            "routes[].middleware[]<compress>.params.level",
            compress(r#""level": "better""#),
            |c| compress_of(c).gzip_level == 7 && compress_of(c).zstd_level == 6,
        ),
        (
            "routes[].middleware[]<compress>.params.gzip_level",
            compress(r#""gzip_level": 9"#),
            |c| compress_of(c).gzip_level == 9,
        ),
        (
            "routes[].middleware[]<compress>.params.brotli_level",
            compress(r#""brotli_level": 11"#),
            |c| compress_of(c).brotli_level == 11,
        ),
        (
            "routes[].middleware[]<compress>.params.zstd_level",
            compress(r#""zstd_level": 19"#),
            |c| compress_of(c).zstd_level == 19,
        ),
        (
            "routes[].middleware[]<compress>.params.min_bytes",
            compress(r#""min_bytes": 512"#),
            |c| compress_of(c).min_bytes == 512,
        ),
        (
            "routes[].middleware[]<compress>.params.min_length",
            compress(r#""min_length": 256"#),
            |c| compress_of(c).min_bytes == 256,
        ),
        (
            "routes[].middleware[]<compress>.params.max_in_flight",
            compress(r#""max_in_flight": 8"#),
            |c| compress_of(c).max_in_flight == 8,
        ),
        ("routes[].middleware[]<wasm>", wasm(""), |c| {
            matches!(c.routes[0].middleware[0], Handler::Wasm(_))
        }),
        ("routes[].middleware[]<wasm>.params", wasm(""), |c| {
            let w = wasm_of(c);
            w.instances == 0
                && w.queue == 1024
                && w.timeout == std::time::Duration::from_millis(100)
                && w.wait_timeout == std::time::Duration::from_secs(1)
                && w.max_memory_bytes == 128 << 20
                && w.on_error == akit_config::WasmOnError::Fail
                && w.response == akit_config::WasmResponse::All
                && w.max_streams == 1024
                && w.max_body_bytes == 1 << 20
                && w.vm_config.is_empty()
                && w.properties.is_empty()
        }),
        ("routes[].middleware[]<wasm>.params.path", wasm(""), |c| {
            wasm_of(c).path == "p.wasm"
        }),
        (
            "routes[].middleware[]<wasm>.params.config",
            wasm(r#", "config": "c""#),
            |c| wasm_of(c).config == "c",
        ),
        (
            "routes[].middleware[]<wasm>.params.instances",
            wasm(r#", "instances": 3"#),
            |c| wasm_of(c).instances == 3,
        ),
        (
            "routes[].middleware[]<wasm>.params.queue",
            wasm(r#", "queue": 8"#),
            |c| wasm_of(c).queue == 8,
        ),
        (
            "routes[].middleware[]<wasm>.params.timeout",
            wasm(r#", "timeout": "50ms""#),
            |c| wasm_of(c).timeout == std::time::Duration::from_millis(50),
        ),
        (
            "routes[].middleware[]<wasm>.params.wait_timeout",
            wasm(r#", "wait_timeout": "2s""#),
            |c| wasm_of(c).wait_timeout == std::time::Duration::from_secs(2),
        ),
        (
            "routes[].middleware[]<wasm>.params.max_memory_bytes",
            wasm(r#", "max_memory_bytes": 67108864"#),
            |c| wasm_of(c).max_memory_bytes == 64 << 20,
        ),
        (
            "routes[].middleware[]<wasm>.params.max_memory",
            wasm(r#", "max_memory": 33554432"#),
            |c| wasm_of(c).max_memory_bytes == 32 << 20,
        ),
        (
            "routes[].middleware[]<wasm>.params.on_error",
            wasm(r#", "on_error": "skip""#),
            |c| wasm_of(c).on_error == akit_config::WasmOnError::Skip,
        ),
        (
            "routes[].middleware[]<wasm>.params.vm_config",
            wasm(r#", "vm_config": "v""#),
            |c| wasm_of(c).vm_config == "v",
        ),
        (
            "routes[].middleware[]<wasm>.params.properties",
            wasm(r#", "properties": [{"path": ["a", "b"], "value": "v"}]"#),
            |c| wasm_of(c).properties.len() == 1,
        ),
        (
            "routes[].middleware[]<wasm>.params.properties[].path",
            wasm(r#", "properties": [{"path": ["a", "b"]}]"#),
            |c| wasm_of(c).properties[0].path == ["a", "b"],
        ),
        (
            "routes[].middleware[]<wasm>.params.properties[].value",
            wasm(r#", "properties": [{"path": ["a"], "value": "v"}]"#),
            |c| wasm_of(c).properties[0].value == "v",
        ),
        (
            "routes[].middleware[]<wasm>.params.response",
            wasm(r#", "response": "headers""#),
            |c| wasm_of(c).response == akit_config::WasmResponse::Headers,
        ),
        (
            "routes[].middleware[]<wasm>.params.max_streams",
            wasm(r#", "max_streams": 8"#),
            |c| wasm_of(c).max_streams == 8,
        ),
        (
            "routes[].middleware[]<wasm>.params.max_body_bytes",
            wasm(r#", "max_body_bytes": 4096"#),
            |c| wasm_of(c).max_body_bytes == 4096,
        ),
        ("routes[].middleware[]<cache>", cache(""), |c| {
            matches!(c.routes[0].middleware[0], Handler::Cache(_))
        }),
        ("routes[].middleware[]<cache>.params", cache(""), |c| {
            cache_of(c).zone == "default" && cache_of(c).max_entry_bytes == 1 << 20
        }),
        (
            "routes[].middleware[]<cache>.params.zone",
            cache(r#""zone": "z""#),
            |c| cache_of(c).zone == "z",
        ),
        (
            "routes[].middleware[]<cache>.params.max_bytes",
            cache(r#""max_bytes": 1048576"#),
            |c| cache_of(c).max_bytes == 1 << 20 && cache_of(c).max_entry_bytes == 1 << 16,
        ),
        (
            "routes[].middleware[]<cache>.params.max_size",
            cache(r#""max_size": 1048576"#),
            |c| cache_of(c).max_bytes == 1 << 20,
        ),
        (
            "routes[].middleware[]<cache>.params.max_entry_bytes",
            cache(r#""max_entry_bytes": 4096"#),
            |c| cache_of(c).max_entry_bytes == 4096,
        ),
        (
            "routes[].middleware[]<cache>.params.max_entry_size",
            cache(r#""max_entry_size": 2048"#),
            |c| cache_of(c).max_entry_bytes == 2048,
        ),
        (
            "routes[].middleware[]<cache>.params.default_ttl",
            cache(r#""default_ttl": "1m""#),
            |c| cache_of(c).default_ttl == Some(std::time::Duration::from_secs(60)),
        ),
        (
            "routes[].middleware[]<cache>.params.disable_lock",
            cache(r#""disable_lock": true"#),
            |c| cache_of(c).disable_lock,
        ),
        (
            "routes[].middleware[]<cache>.params.lock_timeout",
            cache(r#""lock_timeout": "2s""#),
            |c| cache_of(c).lock_timeout == std::time::Duration::from_secs(2),
        ),
        (
            "routes[].middleware[]<cache>.params.stale_while_revalidate",
            cache(r#""stale_while_revalidate": "30s""#),
            |c| cache_of(c).stale_while_revalidate == Some(std::time::Duration::from_secs(30)),
        ),
        (
            "routes[].middleware[]<cache>.params.stale_if_error",
            cache(r#""stale_if_error": "1m""#),
            |c| cache_of(c).stale_if_error == Some(std::time::Duration::from_secs(60)),
        ),
        (
            "routes[].middleware[]<cache>.params.disk",
            cache(r#""disk": {"path": "/var/cache/akit"}"#),
            |c| {
                cache_of(c).disk.as_ref().is_some_and(|d| {
                    d.path == "/var/cache/akit" && d.max_bytes == 1 << 30 && d.max_entries == 100_000
                })
            },
        ),
        (
            "routes[].middleware[]<cache>.params.disk.path",
            cache(r#""disk": {"path": "d"}"#),
            |c| cache_of(c).disk.as_ref().is_some_and(|d| d.path == "d"),
        ),
        (
            "routes[].middleware[]<cache>.params.disk.max_bytes",
            cache(r#""disk": {"path": "d", "max_bytes": 4096}"#),
            |c| cache_of(c).disk.as_ref().is_some_and(|d| d.max_bytes == 4096),
        ),
        (
            "routes[].middleware[]<cache>.params.disk.max_size",
            cache(r#""disk": {"path": "d", "max_size": 2048}"#),
            |c| cache_of(c).disk.as_ref().is_some_and(|d| d.max_bytes == 2048),
        ),
        (
            "routes[].middleware[]<cache>.params.disk.max_entries",
            cache(r#""disk": {"path": "d", "max_entries": 7}"#),
            |c| cache_of(c).disk.as_ref().is_some_and(|d| d.max_entries == 7),
        ),
        ("routes[].middleware[]<jwt>", jwt(""), |c| {
            matches!(c.routes[0].middleware[0], Handler::Jwt(_))
        }),
        ("routes[].middleware[]<jwt>.params", jwt(""), |c| {
            jwt_of(c).jwks_refresh_interval == std::time::Duration::from_secs(3600)
                && jwt_of(c).token_from == akit_config::TokenFrom::Authorization
        }),
        (
            "routes[].middleware[]<jwt>.params.jwks_url",
            jwt(r#", "jwks_url": "https://idp.example/keys""#),
            |c| jwt_of(c).jwks_url == "https://idp.example/keys",
        ),
        (
            "routes[].middleware[]<jwt>.params.jwks_refresh_interval",
            jwt(r#", "jwks_refresh_interval": "10m""#),
            |c| jwt_of(c).jwks_refresh_interval == std::time::Duration::from_secs(600),
        ),
        (
            "routes[].middleware[]<jwt>.params.jwks_refresh",
            jwt(r#", "jwks_refresh": "10m""#),
            |c| jwt_of(c).jwks_refresh_interval == std::time::Duration::from_secs(600),
        ),
        (
            "routes[].middleware[]<jwt>.params.public_key_files",
            jwt(r#", "public_key_files": ["k.pem"]"#),
            |c| jwt_of(c).public_key_files == ["k.pem"],
        ),
        (
            "routes[].middleware[]<jwt>.params.hmac_secret_file",
            jwt(""),
            |c| jwt_of(c).hmac_secret_file == "s",
        ),
        (
            "routes[].middleware[]<jwt>.params.algorithms",
            jwt(r#", "algorithms": ["RS256", "EdDSA"]"#),
            |c| jwt_of(c).algorithms == ["RS256", "EdDSA"],
        ),
        (
            "routes[].middleware[]<jwt>.params.issuer",
            jwt(r#", "issuer": "https://idp.example""#),
            |c| jwt_of(c).issuer == "https://idp.example",
        ),
        (
            "routes[].middleware[]<jwt>.params.audience",
            jwt(r#", "audience": ["api"]"#),
            |c| jwt_of(c).audience == ["api"],
        ),
        (
            "routes[].middleware[]<jwt>.params.leeway",
            jwt(r#", "leeway": "30s""#),
            |c| jwt_of(c).leeway == std::time::Duration::from_secs(30),
        ),
        (
            "routes[].middleware[]<jwt>.params.token_from",
            jwt(r#", "token_from": "query:access_token""#),
            |c| jwt_of(c).token_from == akit_config::TokenFrom::Query("access_token".into()),
        ),
        (
            "routes[].middleware[]<jwt>.params.claims_headers",
            jwt(r#", "claims_headers": {"X-User": "sub"}"#),
            |c| jwt_of(c).claims_headers == [("X-User".to_owned(), "sub".to_owned())],
        ),
        ("upstreams", upstream(""), |c| c.upstreams.len() == 1),
        ("upstreams[].name", upstream(""), |c| {
            c.upstreams[0].name == "app"
        }),
        ("upstreams[].addresses", upstream(""), |c| {
            c.upstreams[0].addresses == ["127.0.0.1:9000"]
        }),
        (
            "upstreams[].protocol",
            upstream(r#", "protocol": "h2c""#),
            |c| c.upstreams[0].protocol == akit_config::UpstreamProtocol::H2c,
        ),
        (
            "upstreams[].tls_server_name",
            upstream(r#", "protocol": "h2", "tls_server_name": "app.internal""#),
            |c| c.upstreams[0].tls_server_name == "app.internal",
        ),
        (
            "upstreams[].tls_ca_file",
            upstream(r#", "protocol": "h2", "tls_ca_file": "/etc/akit/upstream-ca.pem""#),
            |c| c.upstreams[0].tls_ca_file == "/etc/akit/upstream-ca.pem",
        ),
        (
            "upstreams[].tls_cert_file",
            upstream(r#", "protocol": "h2", "tls_cert_file": "c.pem", "tls_key_file": "k.pem""#),
            |c| c.upstreams[0].tls_cert_file == "c.pem",
        ),
        (
            "upstreams[].tls_key_file",
            upstream(r#", "protocol": "h2", "tls_cert_file": "c.pem", "tls_key_file": "k.pem""#),
            |c| c.upstreams[0].tls_key_file == "k.pem",
        ),
        (
            "upstreams[].max_conns",
            upstream(r#", "max_conns": 7"#),
            |c| c.upstreams[0].max_conns == 7,
        ),
        (
            "upstreams[].max_pending",
            upstream(r#", "max_pending": 8"#),
            |c| c.upstreams[0].max_pending == 8,
        ),
        (
            "upstreams[].max_idle_conns",
            upstream(r#", "max_idle_conns": 0"#),
            |c| c.upstreams[0].max_idle_conns == 0,
        ),
        (
            "upstreams[].idle_timeout",
            upstream(r#", "idle_timeout": "5s""#),
            |c| c.upstreams[0].idle_timeout == std::time::Duration::from_secs(5),
        ),
        (
            "upstreams[].connect_timeout",
            upstream(r#", "connect_timeout": "1500ms""#),
            |c| c.upstreams[0].connect_timeout == std::time::Duration::from_millis(1500),
        ),
        (
            "upstreams[].weights",
            upstream(r#", "weights": [3]"#),
            |c| {
                c.upstreams[0].weights == [3]
                    && c.upstreams[0].load_balancing
                        == akit_config::LoadBalancing::WeightedRoundRobin
            },
        ),
        (
            "upstreams[].load_balancing",
            upstream(r#", "load_balancing": "least_conn""#),
            |c| c.upstreams[0].load_balancing == akit_config::LoadBalancing::LeastConn,
        ),
        (
            "upstreams[].max_fails",
            upstream(r#", "max_fails": 3"#),
            |c| c.upstreams[0].max_fails == 3,
        ),
        (
            "upstreams[].fail_timeout",
            upstream(r#", "fail_timeout": "30s""#),
            |c| c.upstreams[0].fail_timeout == std::time::Duration::from_secs(30),
        ),
        (
            "upstreams[].hash_key",
            upstream(r#", "load_balancing": "header_hash", "hash_key": "x-user""#),
            |c| {
                c.upstreams[0].load_balancing == akit_config::LoadBalancing::HeaderHash
                    && c.upstreams[0].hash_key == "x-user"
            },
        ),
        ("upstreams[].sticky_cookie", sticky(""), |c| {
            c.upstreams[0]
                .sticky_cookie
                .as_ref()
                .map(|s| s.name.as_str())
                == Some("akit_sticky")
        }),
        (
            "upstreams[].sticky_cookie.name",
            sticky(r#""name": "s""#),
            |c| {
                c.upstreams[0]
                    .sticky_cookie
                    .as_ref()
                    .map(|s| s.name.as_str())
                    == Some("s")
            },
        ),
        (
            "upstreams[].sticky_cookie.path",
            sticky(r#""path": "/app""#),
            |c| {
                c.upstreams[0]
                    .sticky_cookie
                    .as_ref()
                    .map(|s| s.path.as_str())
                    == Some("/app")
            },
        ),
        (
            "upstreams[].sticky_cookie.max_age",
            sticky(r#""max_age": "1h""#),
            |c| {
                c.upstreams[0]
                    .sticky_cookie
                    .as_ref()
                    .and_then(|s| s.max_age)
                    == Some(std::time::Duration::from_secs(3600))
            },
        ),
        (
            "upstreams[].sticky_cookie.secure",
            sticky(r#""secure": true"#),
            |c| {
                c.upstreams[0]
                    .sticky_cookie
                    .as_ref()
                    .is_some_and(|s| s.secure)
            },
        ),
        ("upstreams[].health_check", health(""), |c| {
            c.upstreams[0].health_check.is_some()
        }),
        (
            "upstreams[].health_check.type",
            health(r#""type": "tcp""#),
            |c| {
                c.upstreams[0].health_check.as_ref().unwrap().kind
                    == akit_config::HealthCheckType::Tcp
            },
        ),
        (
            "upstreams[].health_check.path",
            health(r#""path": "/h""#),
            |c| c.upstreams[0].health_check.as_ref().unwrap().path == "/h",
        ),
        (
            "upstreams[].health_check.host",
            health(r#""host": "app.internal""#),
            |c| {
                c.upstreams[0]
                    .health_check
                    .as_ref()
                    .unwrap()
                    .host
                    .as_deref()
                    == Some("app.internal")
            },
        ),
        (
            "upstreams[].health_check.port",
            health(r#""port": 8081"#),
            |c| c.upstreams[0].health_check.as_ref().unwrap().port == Some(8081),
        ),
        (
            "upstreams[].health_check.expect_status",
            health(r#""expect_status": [204]"#),
            |c| c.upstreams[0].health_check.as_ref().unwrap().expect_status == [204],
        ),
        (
            "upstreams[].health_check.interval",
            health(r#""interval": "7s""#),
            |c| {
                c.upstreams[0].health_check.as_ref().unwrap().interval
                    == std::time::Duration::from_secs(7)
            },
        ),
        (
            "upstreams[].health_check.timeout",
            health(r#""timeout": "1s""#),
            |c| {
                c.upstreams[0].health_check.as_ref().unwrap().timeout
                    == std::time::Duration::from_secs(1)
            },
        ),
        (
            "upstreams[].health_check.jitter",
            health(r#""jitter": "3ms""#),
            |c| {
                c.upstreams[0].health_check.as_ref().unwrap().jitter
                    == std::time::Duration::from_millis(3)
            },
        ),
        (
            "upstreams[].health_check.fails",
            health(r#""fails": 4"#),
            |c| c.upstreams[0].health_check.as_ref().unwrap().fails == 4,
        ),
        (
            "upstreams[].health_check.passes",
            health(r#""passes": 5"#),
            |c| c.upstreams[0].health_check.as_ref().unwrap().passes == 5,
        ),
        (
            "upstreams[].response_timeout",
            upstream(r#", "response_timeout": "2m""#),
            |c| c.upstreams[0].response_timeout == std::time::Duration::from_secs(120),
        ),
        (
            "upstreams[].response_header_timeout",
            upstream(r#", "response_header_timeout": "3s""#),
            |c| c.upstreams[0].response_header_timeout == std::time::Duration::from_secs(3),
        ),
        (
            "upstreams[].response_idle_timeout",
            upstream(r#", "response_idle_timeout": "4s""#),
            |c| c.upstreams[0].response_idle_timeout == std::time::Duration::from_secs(4),
        ),
        ("services", service(""), |c| c.services.len() == 1),
        ("services[].name", service(""), |c| {
            c.services[0].name == "svc"
        }),
        ("services[].listeners", service(""), |c| {
            c.services[0].listeners == ["t"]
        }),
        ("services[].upstream", service(""), |c| {
            c.services[0].upstream == "app"
        }),
        (
            "services[].idle_timeout",
            service(r#", "idle_timeout": "45s""#),
            |c| c.services[0].idle_timeout == Some(std::time::Duration::from_secs(45)),
        ),
        (
            "services[].max_conns",
            service(r#", "max_conns": 7"#),
            |c| c.services[0].max_conns == 7,
        ),
        (
            "services[].server_names",
            service(r#", "server_names": ["A.example.com", "*.w.example.com"]"#),
            |c| c.services[0].server_names == ["a.example.com", "*.w.example.com"],
        ),
        (
            "services[].proxy_protocol",
            service(r#", "proxy_protocol": "v2""#),
            |c| c.services[0].proxy_protocol == Some(akit_config::ProxyVersion::V2),
        ),
        (
            "listeners[].proxy_protocol",
            r#"{"listeners": [{"name": "web", "address": ":80", "proxy_protocol": {"trusted": ["10.0.0.1"]}}]}"#.into(),
            |c| c.listeners[0].proxy_protocol.is_some(),
        ),
        (
            "listeners[].proxy_protocol.trusted",
            r#"{"listeners": [{"name": "web", "address": ":80", "proxy_protocol": {"trusted": ["10.0.0.0/8"]}}]}"#.into(),
            |c| {
                c.listeners[0].proxy_protocol.as_ref().unwrap().trusted
                    == [akit_config::Network::parse("10.0.0.0/8").unwrap()]
            },
        ),
        ("listeners[].socks", socks(""), |c| {
            c.listeners[0].protocols == [Protocol::Socks] && c.listeners[0].socks.is_some()
        }),
        ("listeners[].socks.clients", socks(""), |c| {
            socks_of(c).clients == [akit_config::Network::parse("10.0.0.0/8").unwrap()]
        }),
        (
            "listeners[].socks.allow",
            socks(r#", "allow": ["*.Example.com"]"#),
            |c| socks_of(c).allow == [Destination::Suffix(".example.com".into())],
        ),
        (
            "listeners[].socks.deny",
            socks(r#", "deny": ["bad.example.com"]"#),
            |c| socks_of(c).deny == [Destination::Name("bad.example.com".into())],
        ),
        (
            "listeners[].socks.ports",
            socks(r#", "ports": [443]"#),
            |c| socks_of(c).ports == [443],
        ),
        (
            "listeners[].socks.connect_timeout",
            socks(r#", "connect_timeout": "3s""#),
            |c| socks_of(c).connect_timeout == std::time::Duration::from_secs(3),
        ),
        (
            "listeners[].socks.idle_timeout",
            socks(r#", "idle_timeout": "1m""#),
            |c| socks_of(c).idle_timeout == std::time::Duration::from_secs(60),
        ),
        // I151: bt_tracker takes no params; the registry finds it (akit-bittorrent).
        (
            "routes[].handler<bt_tracker>",
            format!(r#"{{{L}, "routes": [{{"handler": {{"module": "bt_tracker"}}}}]}}"#),
            |c| matches!(&c.routes.first().unwrap().handler, Handler::Module(m) if m.name == "bt_tracker"),
        ),
        (
            "routes[].handler<bt_tracker>.params",
            format!(r#"{{{L}, "routes": [{{"handler": {{"module": "bt_tracker", "params": {{}}}}}}]}}"#),
            |c| matches!(&c.routes.first().unwrap().handler, Handler::Module(m) if m.params.is_some()),
        ),
        ("routes[].handler<forward_proxy>", fproxy(""), |c| {
            fproxy_of(c).ports.is_empty()
        }),
        ("routes[].handler<forward_proxy>.params", fproxy(""), |c| {
            fproxy_of(c).connect_timeout == akit_config::DEFAULT_FORWARD_CONNECT_TIMEOUT
        }),
        (
            "routes[].handler<forward_proxy>.params.clients",
            fproxy(""),
            |c| fproxy_of(c).clients == [akit_config::Network::parse("10.0.0.0/8").unwrap()],
        ),
        (
            "routes[].handler<forward_proxy>.params.allow",
            fproxy(r#", "allow": ["192.0.2.0/24"]"#),
            |c| {
                fproxy_of(c).allow
                    == [Destination::Network(
                        akit_config::Network::parse("192.0.2.0/24").unwrap(),
                    )]
            },
        ),
        (
            "routes[].handler<forward_proxy>.params.deny",
            fproxy(r#", "deny": ["169.254.169.254"]"#),
            |c| fproxy_of(c).deny.len() == 1,
        ),
        (
            "routes[].handler<forward_proxy>.params.ports",
            fproxy(r#", "ports": [443, 8443]"#),
            |c| fproxy_of(c).ports == [443, 8443],
        ),
        (
            "routes[].handler<forward_proxy>.params.connect_timeout",
            fproxy(r#", "connect_timeout": "2s""#),
            |c| fproxy_of(c).connect_timeout == std::time::Duration::from_secs(2),
        ),
        (
            "routes[].handler<forward_proxy>.params.idle_timeout",
            fproxy(r#", "idle_timeout": "2m""#),
            |c| fproxy_of(c).idle_timeout == std::time::Duration::from_secs(120),
        ),
        (
            "telemetry",
            r#"{"telemetry": {"traces": {}}}"#.into(),
            |c| c.telemetry.is_some(),
        ),
        (
            "telemetry.endpoint",
            r#"{"telemetry": {"endpoint": "https://otel.example:4318", "traces": {}}}"#.into(),
            |c| c.telemetry.as_ref().unwrap().endpoint == "https://otel.example:4318",
        ),
        (
            "telemetry.headers_file",
            r#"{"telemetry": {"headers_file": "h", "traces": {}}}"#.into(),
            |c| c.telemetry.as_ref().unwrap().headers_file == "h",
        ),
        (
            "telemetry.ca_file",
            r#"{"telemetry": {"ca_file": "ca.pem", "traces": {}}}"#.into(),
            |c| c.telemetry.as_ref().unwrap().ca_file == "ca.pem",
        ),
        (
            "telemetry.cert_file",
            r#"{"telemetry": {"cert_file": "c.pem", "key_file": "k.pem", "traces": {}}}"#.into(),
            |c| c.telemetry.as_ref().unwrap().cert_file == "c.pem",
        ),
        (
            "telemetry.key_file",
            r#"{"telemetry": {"cert_file": "c.pem", "key_file": "k.pem", "traces": {}}}"#.into(),
            |c| c.telemetry.as_ref().unwrap().key_file == "k.pem",
        ),
        (
            "telemetry.service_name",
            r#"{"telemetry": {"service_name": "edge", "traces": {}}}"#.into(),
            |c| c.telemetry.as_ref().unwrap().service_name == "edge",
        ),
        (
            "telemetry.timeout",
            r#"{"telemetry": {"timeout": "3s", "traces": {}}}"#.into(),
            |c| c.telemetry.as_ref().unwrap().timeout == std::time::Duration::from_secs(3),
        ),
        (
            "telemetry.traces",
            r#"{"telemetry": {"traces": {}}}"#.into(),
            |c| c.telemetry.as_ref().unwrap().traces.is_some(),
        ),
        (
            "telemetry.traces.sample_ratio",
            r#"{"telemetry": {"traces": {"sample_ratio": 0.25}}}"#.into(),
            |c| c.telemetry.as_ref().unwrap().traces.as_ref().unwrap().sample_ratio == 0.25,
        ),
        (
            "telemetry.traces.max_queue_bytes",
            r#"{"telemetry": {"traces": {"max_queue_bytes": 1024}}}"#.into(),
            |c| c.telemetry.as_ref().unwrap().traces.as_ref().unwrap().max_queue_bytes == 1024,
        ),
        (
            "telemetry.traces.interval",
            r#"{"telemetry": {"traces": {"interval": "250ms"}}}"#.into(),
            |c| c.telemetry.as_ref().unwrap().traces.as_ref().unwrap().interval == std::time::Duration::from_millis(250),
        ),
        (
            "telemetry.metrics",
            r#"{"telemetry": {"metrics": {}}}"#.into(),
            |c| c.telemetry.as_ref().unwrap().metrics.is_some(),
        ),
        (
            "telemetry.metrics.interval",
            r#"{"telemetry": {"metrics": {"interval": "1m"}}}"#.into(),
            |c| c.telemetry.as_ref().unwrap().metrics.as_ref().unwrap().interval == std::time::Duration::from_secs(60),
        ),
        // I58: the Docker provider.
        (
            "providers",
            r#"{"providers": [{"docker": {}}]}"#.into(),
            |c| c.providers.len() == 1,
        ),
        (
            "providers[].name",
            r#"{"providers": [{"name": "dk", "docker": {}}]}"#.into(),
            |c| c.providers.first().unwrap().name == "dk",
        ),
        (
            "providers[].docker",
            r#"{"providers": [{"docker": {}}]}"#.into(),
            |c| c.providers.first().unwrap().name == "docker" && docker(c).label_prefix == "akit",
        ),
        (
            "providers[].docker.endpoint",
            r#"{"providers": [{"docker": {"endpoint": "tcp://proxy:2375"}}]}"#.into(),
            |c| docker(c).endpoint == akit_config::DockerEndpoint::Tcp("proxy:2375".into()),
        ),
        (
            "providers[].docker.expose_all",
            r#"{"providers": [{"docker": {"expose_all": true}}]}"#.into(),
            |c| docker(c).expose_all,
        ),
        (
            "providers[].docker.network",
            r#"{"providers": [{"docker": {"network": "web"}}]}"#.into(),
            |c| docker(c).network == "web",
        ),
        (
            "providers[].docker.use_published_ports",
            r#"{"providers": [{"docker": {"use_published_ports": true}}]}"#.into(),
            |c| docker(c).use_published_ports,
        ),
        (
            "providers[].docker.label_prefix",
            r#"{"providers": [{"docker": {"label_prefix": "app"}}]}"#.into(),
            |c| docker(c).label_prefix == "app",
        ),
        (
            "providers[].docker.max_containers",
            r#"{"providers": [{"docker": {"max_containers": 5}}]}"#.into(),
            |c| docker(c).max_containers == 5,
        ),
        (
            "providers[].docker.refresh_interval",
            r#"{"providers": [{"docker": {"refresh_interval": "5s"}}]}"#.into(),
            |c| docker(c).refresh_interval == std::time::Duration::from_secs(5),
        ),
        (
            "providers[].docker.request_timeout",
            r#"{"providers": [{"docker": {"request_timeout": "2s"}}]}"#.into(),
            |c| docker(c).request_timeout == std::time::Duration::from_secs(2),
        ),
        (
            "providers[].kubernetes",
            k8s(""),
            |c| c.providers[0].kind.name() == "kubernetes",
        ),
        (
            "providers[].kubernetes.kubeconfig",
            k8s(r#", "kubeconfig": "/k""#),
            |c| kube(c).kubeconfig == "/k",
        ),
        (
            "providers[].kubernetes.context",
            k8s(r#", "context": "prod""#),
            |c| kube(c).context == "prod",
        ),
        (
            "providers[].kubernetes.controller_name",
            k8s(r#", "controller_name": "example.net/gw""#),
            |c| kube(c).controller_name == "example.net/gw",
        ),
        (
            "providers[].kubernetes.namespaces",
            k8s(r#", "namespaces": ["a"]"#),
            |c| kube(c).namespaces == ["a"],
        ),
        (
            "providers[].kubernetes.listen_host",
            k8s(r#", "listen_host": "10.0.0.1""#),
            |c| kube(c).listen_host == "10.0.0.1",
        ),
        (
            "providers[].kubernetes.addresses",
            r#"{"providers": [{"kubernetes": {"addresses": ["192.0.2.1", "lb.example.com"]}}]}"#
                .into(),
            |c| kube(c).addresses == ["192.0.2.1", "lb.example.com"] && kube(c).ingress.is_none(),
        ),
        (
            "providers[].kubernetes.backends",
            k8s(r#", "backends": "cluster_ip""#),
            |c| kube(c).backends == akit_config::KubernetesBackends::ClusterIp,
        ),
        (
            "providers[].kubernetes.max_objects",
            k8s(r#", "max_objects": 7"#),
            |c| kube(c).max_objects == 7,
        ),
        (
            "providers[].kubernetes.request_timeout",
            k8s(r#", "request_timeout": "3s""#),
            |c| kube(c).request_timeout == std::time::Duration::from_secs(3),
        ),
        (
            "providers[].kubernetes.ingress",
            k8s(""),
            |c| kube(c).ingress.is_some(),
        ),
        (
            "providers[].kubernetes.ingress.http_port",
            r#"{"providers": [{"kubernetes": {"ingress": {"http_port": 8080}}}]}"#.into(),
            |c| kube(c).ingress.unwrap().http_port == 8080,
        ),
        (
            "providers[].kubernetes.ingress.https_port",
            r#"{"providers": [{"kubernetes": {"ingress": {"https_port": 8443}}}]}"#.into(),
            |c| kube(c).ingress.unwrap().https_port == 8443,
        ),
        (
            "providers[].kubernetes.certificates_dir",
            k8s(r#", "certificates_dir": "/d""#),
            |c| kube(c).certificates_dir == "/d",
        ),
        // I61: the Consul, Consul KV and etcd providers.
        (
            "providers[].consul",
            r#"{"providers": [{"consul": {}}]}"#.into(),
            |c| c.providers[0].kind.name() == "consul",
        ),
        (
            "providers[].consul.endpoint",
            r#"{"providers": [{"consul": {"endpoint": "https://c.example:8501"}}]}"#.into(),
            |c| consul(c).conn.endpoint == "https://c.example:8501",
        ),
        (
            "providers[].consul.datacenter",
            r#"{"providers": [{"consul": {"datacenter": "dc2"}}]}"#.into(),
            |c| consul(c).conn.datacenter == "dc2",
        ),
        (
            "providers[].consul.token_file",
            r#"{"providers": [{"consul": {"token_file": "t"}}]}"#.into(),
            |c| consul(c).conn.token_file == "t",
        ),
        (
            "providers[].consul.ca_file",
            r#"{"providers": [{"consul": {"ca_file": "ca.pem"}}]}"#.into(),
            |c| consul(c).conn.tls.ca_file == "ca.pem",
        ),
        (
            "providers[].consul.cert_file",
            r#"{"providers": [{"consul": {"cert_file": "c.pem", "key_file": "k.pem"}}]}"#.into(),
            |c| consul(c).conn.tls.cert_file == "c.pem",
        ),
        (
            "providers[].consul.key_file",
            r#"{"providers": [{"consul": {"cert_file": "c.pem", "key_file": "k.pem"}}]}"#.into(),
            |c| consul(c).conn.tls.key_file == "k.pem",
        ),
        (
            "providers[].consul.expose_all",
            r#"{"providers": [{"consul": {"expose_all": true}}]}"#.into(),
            |c| consul(c).expose_all,
        ),
        (
            "providers[].consul.tag_prefix",
            r#"{"providers": [{"consul": {"tag_prefix": "app"}}]}"#.into(),
            |c| consul(c).tag_prefix == "app",
        ),
        (
            "providers[].consul.max_services",
            r#"{"providers": [{"consul": {"max_services": 5}}]}"#.into(),
            |c| consul(c).max_services == 5,
        ),
        (
            "providers[].consul.refresh_interval",
            r#"{"providers": [{"consul": {"refresh_interval": "1m"}}]}"#.into(),
            |c| consul(c).conn.refresh_interval == std::time::Duration::from_secs(60),
        ),
        (
            "providers[].consul.request_timeout",
            r#"{"providers": [{"consul": {"request_timeout": "3s"}}]}"#.into(),
            |c| consul(c).conn.request_timeout == std::time::Duration::from_secs(3),
        ),
        (
            "providers[].consul_kv",
            r#"{"providers": [{"consul_kv": {}}]}"#.into(),
            |c| c.providers[0].kind.name() == "consul_kv",
        ),
        (
            "providers[].consul_kv.endpoint",
            r#"{"providers": [{"consul_kv": {"endpoint": "https://c.example:8501"}}]}"#.into(),
            |c| consul_kv(c).conn.endpoint == "https://c.example:8501",
        ),
        (
            "providers[].consul_kv.datacenter",
            r#"{"providers": [{"consul_kv": {"datacenter": "dc2"}}]}"#.into(),
            |c| consul_kv(c).conn.datacenter == "dc2",
        ),
        (
            "providers[].consul_kv.token_file",
            r#"{"providers": [{"consul_kv": {"token_file": "t"}}]}"#.into(),
            |c| consul_kv(c).conn.token_file == "t",
        ),
        (
            "providers[].consul_kv.ca_file",
            r#"{"providers": [{"consul_kv": {"ca_file": "ca.pem"}}]}"#.into(),
            |c| consul_kv(c).conn.tls.ca_file == "ca.pem",
        ),
        (
            "providers[].consul_kv.cert_file",
            r#"{"providers": [{"consul_kv": {"cert_file": "c.pem", "key_file": "k.pem"}}]}"#.into(),
            |c| consul_kv(c).conn.tls.cert_file == "c.pem",
        ),
        (
            "providers[].consul_kv.key_file",
            r#"{"providers": [{"consul_kv": {"cert_file": "c.pem", "key_file": "k.pem"}}]}"#.into(),
            |c| consul_kv(c).conn.tls.key_file == "k.pem",
        ),
        (
            "providers[].consul_kv.prefix",
            r#"{"providers": [{"consul_kv": {"prefix": "edge/"}}]}"#.into(),
            |c| consul_kv(c).prefix == "edge/",
        ),
        (
            "providers[].consul_kv.max_keys",
            r#"{"providers": [{"consul_kv": {"max_keys": 7}}]}"#.into(),
            |c| consul_kv(c).max_keys == 7,
        ),
        (
            "providers[].consul_kv.refresh_interval",
            r#"{"providers": [{"consul_kv": {"refresh_interval": "1m"}}]}"#.into(),
            |c| consul_kv(c).conn.refresh_interval == std::time::Duration::from_secs(60),
        ),
        (
            "providers[].consul_kv.request_timeout",
            r#"{"providers": [{"consul_kv": {"request_timeout": "3s"}}]}"#.into(),
            |c| consul_kv(c).conn.request_timeout == std::time::Duration::from_secs(3),
        ),
        (
            "providers[].etcd",
            r#"{"providers": [{"etcd": {}}]}"#.into(),
            |c| c.providers[0].kind.name() == "etcd",
        ),
        (
            "providers[].etcd.endpoints",
            r#"{"providers": [{"etcd": {"endpoints": ["http://10.0.0.71:2379"]}}]}"#.into(),
            |c| etcd(c).endpoints == ["http://10.0.0.71:2379"],
        ),
        (
            "providers[].etcd.username",
            r#"{"providers": [{"etcd": {"username": "u", "password_file": "p"}}]}"#.into(),
            |c| etcd(c).username == "u",
        ),
        (
            "providers[].etcd.password_file",
            r#"{"providers": [{"etcd": {"password_file": "p", "username": "u"}}]}"#.into(),
            |c| etcd(c).password_file == "p",
        ),
        (
            "providers[].etcd.ca_file",
            r#"{"providers": [{"etcd": {"ca_file": "ca.pem"}}]}"#.into(),
            |c| etcd(c).tls.ca_file == "ca.pem",
        ),
        (
            "providers[].etcd.cert_file",
            r#"{"providers": [{"etcd": {"cert_file": "c.pem", "key_file": "k.pem"}}]}"#.into(),
            |c| etcd(c).tls.cert_file == "c.pem",
        ),
        (
            "providers[].etcd.key_file",
            r#"{"providers": [{"etcd": {"cert_file": "c.pem", "key_file": "k.pem"}}]}"#.into(),
            |c| etcd(c).tls.key_file == "k.pem",
        ),
        (
            "providers[].etcd.prefix",
            r#"{"providers": [{"etcd": {"prefix": "edge/"}}]}"#.into(),
            |c| etcd(c).prefix == "edge/",
        ),
        (
            "providers[].etcd.max_keys",
            r#"{"providers": [{"etcd": {"max_keys": 7}}]}"#.into(),
            |c| etcd(c).max_keys == 7,
        ),
        (
            "providers[].etcd.refresh_interval",
            r#"{"providers": [{"etcd": {"refresh_interval": "1m"}}]}"#.into(),
            |c| etcd(c).refresh_interval == std::time::Duration::from_secs(60),
        ),
        (
            "providers[].etcd.request_timeout",
            r#"{"providers": [{"etcd": {"request_timeout": "3s"}}]}"#.into(),
            |c| etcd(c).request_timeout == std::time::Duration::from_secs(3),
        ),
    ];
    let marked = supported().unwrap();
    // The roles' fields (I98), sampled in support/roles.rs.
    let roles = role_samples::samples();
    let mut sampled: BTreeSet<String> = samples.iter().map(|(p, ..)| (*p).to_owned()).collect();
    for (p, ..) in &roles {
        assert!(sampled.insert(p.clone()), "{p} is sampled twice");
    }
    assert_eq!(
        sampled, marked,
        "supported.txt and this test's samples must name the same fields"
    );
    for (path, a, read) in &roles {
        let first = akit_config::from_json(a, None);
        match read {
            role_samples::Read::Differs(b) => {
                let first =
                    first.unwrap_or_else(|e| panic!("{path}: akit-config refuses {a}:\n{e}"));
                let second = akit_config::from_json(b, None)
                    .unwrap_or_else(|e| panic!("{path}: akit-config refuses {b}:\n{e}"));
                assert!(
                    first != second,
                    "{path}: akit-config does not read it: {a} and {b} read the same"
                );
            }
            // A field fit refuses in this role: refused by its check, not as unknown.
            role_samples::Read::Refused(msg) => match first {
                Ok(_) => panic!("{path}: {a} loads, want {msg:?}"),
                Err(e) => assert!(e.to_string().contains(msg), "{path}: {e}, want {msg:?}"),
            },
        }
    }
    for (path, json, check) in &samples {
        let config = akit_config::from_json(json, None)
            .unwrap_or_else(|e| panic!("{path}: akit-config refuses {json}:\n{e}"));
        assert!(
            check(&config),
            "{path}: akit-config does not read it from {json}"
        );
    }
}

#[allow(clippy::panic)]
fn socks_of(c: &Config) -> &akit_config::ForwardProxy {
    c.listeners.first().unwrap().socks.as_ref().unwrap()
}

fn fproxy_of(c: &Config) -> &akit_config::ForwardProxy {
    match &c.routes.first().unwrap().handler {
        Handler::ForwardProxy(f) => Some(f),
        _ => None,
    }
    .unwrap()
}

fn respond_of(c: &Config) -> &akit_config::Respond {
    match &c.routes.first().unwrap().handler {
        Handler::Respond(r) => Some(r),
        _ => None,
    }
    .unwrap()
}

fn static_of(c: &Config) -> &akit_config::Static {
    match &c.routes.first().unwrap().handler {
        Handler::Static(s) => Some(s),
        _ => None,
    }
    .unwrap()
}

fn rewrite_of(c: &Config) -> &akit_config::Rewrite {
    match c.routes.first().unwrap().middleware.first().unwrap() {
        Handler::Rewrite(r) => Some(r),
        _ => None,
    }
    .unwrap()
}

fn redirect_of(c: &Config) -> &akit_config::Redirect {
    match c.routes.first().unwrap().middleware.first().unwrap() {
        Handler::Redirect(r) => Some(r),
        _ => None,
    }
    .unwrap()
}

fn headers_of(c: &Config) -> &akit_config::Headers {
    match c.routes.first().unwrap().middleware.first().unwrap() {
        Handler::Headers(h) => Some(h),
        _ => None,
    }
    .unwrap()
}

fn basic_of(c: &Config) -> &akit_config::BasicAuth {
    match c.routes.first().unwrap().middleware.first().unwrap() {
        Handler::BasicAuth(b) => Some(b),
        _ => None,
    }
    .unwrap()
}

fn jwt_of(c: &Config) -> &akit_config::Jwt {
    match c.routes.first().unwrap().middleware.first().unwrap() {
        Handler::Jwt(j) => Some(j),
        _ => None,
    }
    .unwrap()
}

fn forward_of(c: &Config) -> &akit_config::ForwardAuth {
    match c.routes.first().unwrap().middleware.first().unwrap() {
        Handler::ForwardAuth(f) => Some(f),
        _ => None,
    }
    .unwrap()
}

fn breaker_of(c: &Config) -> &akit_config::CircuitBreaker {
    match c.routes.first().unwrap().middleware.first().unwrap() {
        Handler::CircuitBreaker(b) => Some(b),
        _ => None,
    }
    .unwrap()
}

fn wasm_of(c: &Config) -> &akit_config::Wasm {
    match c.routes.first().unwrap().middleware.first().unwrap() {
        Handler::Wasm(w) => Some(w),
        _ => None,
    }
    .unwrap()
}

fn cache_of(c: &Config) -> &akit_config::Cache {
    match c.routes.first().unwrap().middleware.first().unwrap() {
        Handler::Cache(cm) => Some(cm),
        _ => None,
    }
    .unwrap()
}

fn compress_of(c: &Config) -> &akit_config::Compress {
    match c.routes.first().unwrap().middleware.first().unwrap() {
        Handler::Compress(cm) => Some(cm),
        _ => None,
    }
    .unwrap()
}

fn cert_of(c: &Config) -> &akit_config::ClientCert {
    match c.routes.first().unwrap().middleware.first().unwrap() {
        Handler::ClientCert(cc) => Some(cc),
        _ => None,
    }
    .unwrap()
}

fn rate_of(c: &Config) -> &akit_config::RateLimit {
    match c.routes.first().unwrap().middleware.first().unwrap() {
        Handler::RateLimit(r) => Some(r),
        _ => None,
    }
    .unwrap()
}

fn concurrency_of(c: &Config) -> &akit_config::ConcurrencyLimit {
    match c.routes.first().unwrap().middleware.first().unwrap() {
        Handler::ConcurrencyLimit(l) => Some(l),
        _ => None,
    }
    .unwrap()
}

fn fastcgi_of(c: &Config) -> &akit_config::FastCgi {
    match &c.routes.first().unwrap().handler {
        Handler::FastCgi(f) => Some(f),
        _ => None,
    }
    .unwrap()
}

fn proxy_of(c: &Config) -> &akit_config::Proxy {
    match &c.routes.first().unwrap().handler {
        Handler::Proxy(p) => Some(p),
        _ => None,
    }
    .unwrap()
}

/// The first provider's Docker settings.
#[allow(clippy::panic)]
fn docker(c: &Config) -> &akit_config::DockerProvider {
    match &c.providers.first().unwrap().kind {
        akit_config::ProviderKind::Docker(d) => d,
        _ => panic!("not docker"),
    }
}

/// The first provider's Kubernetes settings.
#[allow(clippy::panic)]
fn kube(c: &Config) -> &akit_config::KubernetesProvider {
    match &c.providers.first().unwrap().kind {
        akit_config::ProviderKind::Kubernetes(k) => k,
        _ => panic!("not kubernetes"),
    }
}

/// akit-config's lists of what it reads and of what fit reads that it refuses as not supported
/// yet (`crates/akit-config/src/model.rs`), read from the source: each entry is a field (or
/// handler module, or protocol) of the inventory, together they cover every field fit has at each
/// object akit-config reads (so no field fit accepts is refused as unknown), and what akit-config
/// reads is what `supported.txt` marks.
/// The lists of akit's extensions in `model.rs`, each with the inventory path of its object:
/// fields fit does not have, which akit-config reads (decision 010).
const AKIT_ONLY: [(&str, &str); 1] = [("listeners[]", "LISTENER_AKIT_ONLY")];

#[test]
fn akit_config_lists_match_the_inventory() {
    // The model, and the providers' and shared storage's lists, in modules of their own (I58,
    // I61, I62).
    let src = [
        "model.rs",
        "providers.rs",
        "providers_kv.rs",
        "shared_storage.rs",
        "log.rs",
        // I98: the tunnel's and the LLM feature's roles.
        "roles.rs",
        // I110: the WebRTC roles and handlers.
        "webrtc.rs",
        // I150: the BitTorrent roles.
        "bittorrent.rs",
        // I130: cluster management.
        "cluster.rs",
        // I120: media auth and the streaming role.
        "media.rs",
        // I140: the media libraries.
        "medialib.rs",
    ]
    .iter()
    .map(|f| fs::read_to_string(compat_dir().join("../../crates/akit-config/src").join(f)).unwrap())
    .collect::<Vec<_>>()
    .join("\n");
    let lists = str_lists(&src);
    let inventory = fields().unwrap();
    let known: BTreeSet<&str> = inventory.iter().map(|f| f.path.as_str()).collect();
    let list = |name: &str| -> &Vec<String> {
        lists
            .get(name)
            .unwrap_or_else(|| panic!("model.rs has no list {name}"))
    };
    // Each object akit-config reads: its inventory path, the list of fields it reads, and the
    // list of fields it refuses as not supported yet.
    let objects: [(&str, &str, Option<&str>); 185] = [
        ("", "CONFIG_FIELDS", Some("CONFIG_FIT_ONLY")),
        (
            "providers[].kubernetes",
            "KUBERNETES_FIELDS",
            Some("KUBERNETES_FIT_ONLY"),
        ),
        (
            "providers[].kubernetes.ingress",
            "KUBERNETES_INGRESS_FIELDS",
            None,
        ),
        ("admin", "ADMIN_FIELDS", Some("ADMIN_FIT_ONLY")),
        ("admin.console", "CONSOLE_FIELDS", Some("CONSOLE_FIT_ONLY")),
        ("admin.console.users[]", "CONSOLE_USER_FIELDS", None),
        ("admin.console.access", "ACCESS_FIELDS", None),
        ("admin.store", "STORE_FIELDS", None),
        ("admin.cluster", "CLUSTER_FIELDS", None),
        ("admin.cluster_member", "CLUSTER_MEMBER_FIELDS", None),
        ("acme", "ACME_FIELDS", Some("ACME_FIT_ONLY")),
        ("acme.on_demand", "ON_DEMAND_FIELDS", None),
        ("acme.shared_storage", "SHARED_STORAGE_FIELDS", None),
        ("acme.shared_storage.redis", "REDIS_STORAGE_FIELDS", None),
        ("acme.shared_storage.etcd", "ETCD_STORAGE_FIELDS", None),
        ("acme.shared_storage.consul", "CONSUL_STORAGE_FIELDS", None),
        ("limits", "LIMITS_FIELDS", Some("LIMITS_FIT_ONLY")),
        ("access", "ACCESS_FIELDS", None),
        ("listeners[].access", "ACCESS_FIELDS", None),
        ("routes[].access", "ACCESS_FIELDS", None),
        ("services[].access", "ACCESS_FIELDS", None),
        ("admin.access", "ACCESS_FIELDS", None),
        ("engine", "ENGINE_FIELDS", Some("ENGINE_FIT_ONLY")),
        (
            "access_log",
            "ACCESS_LOG_FIELDS",
            Some("ACCESS_LOG_FIT_ONLY"),
        ),
        ("access_log.rotate", "ROTATE_FIELDS", None),
        ("log", "LOG_FIELDS", None),
        ("log.rotate", "ROTATE_FIELDS", None),
        ("log.loggers.*", "LOGGER_FIELDS", None),
        ("log.loggers.*.rotate", "ROTATE_FIELDS", None),
        ("auto_https", "AUTO_HTTPS_FIELDS", None),
        ("telemetry", "TELEMETRY_FIELDS", None),
        ("telemetry.traces", "TRACES_EXPORT_FIELDS", None),
        ("telemetry.metrics", "METRICS_EXPORT_FIELDS", None),
        ("providers[]", "PROVIDER_FIELDS", Some("PROVIDER_FIT_ONLY")),
        ("providers[].consul", "CONSUL_PROVIDER_FIELDS", None),
        ("providers[].consul_kv", "CONSUL_KV_PROVIDER_FIELDS", None),
        ("providers[].etcd", "ETCD_PROVIDER_FIELDS", None),
        ("providers[].docker", "DOCKER_FIELDS", None),
        ("listeners[]", "LISTENER_FIELDS", Some("LISTENER_FIT_ONLY")),
        ("listeners[].tls", "TLS_FIELDS", Some("TLS_FIT_ONLY")),
        ("listeners[].tls.certificates[]", "CERTIFICATE_FIELDS", None),
        ("listeners[].tls.client_auth", "CLIENT_AUTH_FIELDS", None),
        (
            "listeners[].proxy_protocol",
            "LISTENER_PROXY_PROTOCOL_FIELDS",
            None,
        ),
        ("listeners[].socks", "FORWARD_PROXY_FIELDS", None),
        (
            "routes[].handler<forward_proxy>.params",
            "FORWARD_PROXY_FIELDS",
            None,
        ),
        ("routes[]", "ROUTE_FIELDS", Some("ROUTE_FIT_ONLY")),
        ("routes[].match", "MATCH_FIELDS", Some("MATCH_FIT_ONLY")),
        ("routes[].handler", "MODULE_FIELDS", None),
        ("routes[].handler<respond>.params", "RESPOND_FIELDS", None),
        (
            "routes[].handler<static>.params",
            "STATIC_FIELDS",
            Some("STATIC_FIT_ONLY"),
        ),
        (
            "routes[].handler<proxy>.params",
            "PROXY_FIELDS",
            Some("PROXY_FIT_ONLY"),
        ),
        ("routes[].handler<fastcgi>.params", "FASTCGI_FIELDS", None),
        ("upstreams[]", "UPSTREAM_FIELDS", Some("UPSTREAM_FIT_ONLY")),
        ("services[]", "SERVICE_FIELDS", Some("SERVICE_FIT_ONLY")),
        ("upstreams[].health_check", "HEALTH_CHECK_FIELDS", None),
        ("upstreams[].sticky_cookie", "STICKY_COOKIE_FIELDS", None),
        ("routes[].middleware[]", "MODULE_FIELDS", None),
        (
            "routes[].middleware[]<rewrite>.params",
            "REWRITE_FIELDS",
            None,
        ),
        (
            "routes[].middleware[]<redirect>.params",
            "REDIRECT_FIELDS",
            None,
        ),
        (
            "routes[].middleware[]<headers>.params",
            "HEADERS_FIELDS",
            None,
        ),
        (
            "routes[].middleware[]<headers>.params.request",
            "HEADER_OPS_FIELDS",
            None,
        ),
        (
            "routes[].middleware[]<headers>.params.response",
            "HEADER_OPS_FIELDS",
            None,
        ),
        (
            "routes[].middleware[]<rate_limit>.params",
            "RATE_LIMIT_FIELDS",
            None,
        ),
        (
            "routes[].middleware[]<concurrency_limit>.params",
            "CONCURRENCY_LIMIT_FIELDS",
            None,
        ),
        (
            "routes[].middleware[]<body_limit>.params",
            "BODY_LIMIT_FIELDS",
            None,
        ),
        (
            "routes[].middleware[]<basic_auth>.params",
            "BASIC_AUTH_FIELDS",
            None,
        ),
        (
            "routes[].middleware[]<basic_auth>.params.users[]",
            "BASIC_USER_FIELDS",
            None,
        ),
        (
            "routes[].middleware[]<client_cert>.params",
            "CLIENT_CERT_FIELDS",
            None,
        ),
        (
            "routes[].middleware[]<forward_auth>.params",
            "FORWARD_AUTH_FIELDS",
            None,
        ),
        (
            "routes[].middleware[]<circuit_breaker>.params",
            "CIRCUIT_BREAKER_FIELDS",
            None,
        ),
        ("routes[].handler<proxy>.params.retry", "RETRY_FIELDS", None),
        (
            "routes[].handler<proxy>.params.upstreams[]",
            "SPLIT_FIELDS",
            None,
        ),
        (
            "routes[].handler<proxy>.params.mirror",
            "MIRROR_FIELDS",
            None,
        ),
        (
            "routes[].middleware[]<compress>.params",
            "COMPRESS_FIELDS",
            None,
        ),
        ("routes[].middleware[]<jwt>.params", "JWT_FIELDS", None),
        ("routes[].middleware[]<cache>.params", "CACHE_FIELDS", None),
        ("routes[].middleware[]<wasm>.params", "WASM_FIELDS", None),
        (
            "routes[].middleware[]<wasm>.params.properties[]",
            "WASM_PROPERTY_FIELDS",
            None,
        ),
        (
            "routes[].middleware[]<cache>.params.disk",
            "CACHE_DISK_FIELDS",
            None,
        ),
        // I98: the roles (crate::roles).
        ("tunnel", "TUNNEL_FIELDS", None),
        ("tunnel.hub", "TUNNEL_HUB_FIELDS", None),
        ("tunnel.hub.listeners[]", "TUNNEL_LISTENER_FIELDS", None),
        ("tunnel.hub.console_socket", "TUNNEL_SOCKET_FIELDS", None),
        ("tunnel.hub.quic", "TUNNEL_QUIC_FIELDS", None),
        ("tunnel.hub.tokens[]", "LINK_TOKEN_FIELDS", None),
        ("tunnel.hub.access", "ACCESS_FIELDS", None),
        (
            "tunnel.hub.console",
            "CONSOLE_FIELDS",
            Some("CONSOLE_FIT_ONLY"),
        ),
        ("tunnel.hub.console.users[]", "CONSOLE_USER_FIELDS", None),
        ("tunnel.hub.console.access", "ACCESS_FIELDS", None),
        ("tunnel.node", "TUNNEL_NODE_FIELDS", None),
        ("tunnel.node.hubs[]", "TUNNEL_UPLINK_FIELDS", None),
        (
            "tunnel.node.hubs[].endpoints[]",
            "TUNNEL_ENDPOINT_FIELDS",
            None,
        ),
        ("tunnel.node.services[]", "TUNNEL_SERVICE_FIELDS", None),
        (
            "tunnel.node.console",
            "CONSOLE_FIELDS",
            Some("CONSOLE_FIT_ONLY"),
        ),
        ("tunnel.node.console.users[]", "CONSOLE_USER_FIELDS", None),
        ("tunnel.node.console.access", "ACCESS_FIELDS", None),
        ("tunnel.proxy", "TUNNEL_PROXY_FIELDS", None),
        ("tunnel.proxy.hubs[]", "TUNNEL_UPLINK_FIELDS", None),
        (
            "tunnel.proxy.hubs[].endpoints[]",
            "TUNNEL_ENDPOINT_FIELDS",
            None,
        ),
        ("tunnel.proxy.forwards[]", "TUNNEL_FORWARD_FIELDS", None),
        (
            "tunnel.proxy.web_proxies[]",
            "TUNNEL_WEB_PROXY_FIELDS",
            None,
        ),
        ("tunnel.proxy.access", "ACCESS_FIELDS", None),
        (
            "tunnel.proxy.console",
            "CONSOLE_FIELDS",
            Some("CONSOLE_FIT_ONLY"),
        ),
        ("tunnel.proxy.console.users[]", "CONSOLE_USER_FIELDS", None),
        ("tunnel.proxy.console.access", "ACCESS_FIELDS", None),
        ("llm", "LLM_FIELDS", None),
        ("llm.hub", "LLM_HUB_FIELDS", None),
        ("llm.hub.listen", "LLM_LISTEN_FIELDS", None),
        ("llm.hub.peers[]", "LLM_PEER_FIELDS", None),
        ("llm.hub.limits", "LLM_LIMITS_FIELDS", None),
        ("llm.hub.tokens[]", "LINK_TOKEN_FIELDS", None),
        ("llm.hub.access", "ACCESS_FIELDS", None),
        (
            "llm.hub.console",
            "CONSOLE_FIELDS",
            Some("CONSOLE_FIT_ONLY"),
        ),
        ("llm.hub.console.users[]", "CONSOLE_USER_FIELDS", None),
        ("llm.hub.console.access", "ACCESS_FIELDS", None),
        ("llm.node", "LLM_NODE_FIELDS", None),
        ("llm.node.listen", "LLM_LISTEN_FIELDS", None),
        ("llm.node.peers[]", "LLM_PEER_FIELDS", None),
        ("llm.node.limits", "LLM_LIMITS_FIELDS", None),
        ("llm.node.tokens[]", "LINK_TOKEN_FIELDS", None),
        ("llm.node.access", "ACCESS_FIELDS", None),
        (
            "llm.node.console",
            "CONSOLE_FIELDS",
            Some("CONSOLE_FIT_ONLY"),
        ),
        ("llm.node.console.users[]", "CONSOLE_USER_FIELDS", None),
        ("llm.node.console.access", "ACCESS_FIELDS", None),
        ("llm.proxy", "LLM_PROXY_FIELDS", None),
        ("llm.proxy.listen", "LLM_LISTEN_FIELDS", None),
        ("llm.proxy.peers[]", "LLM_PEER_FIELDS", None),
        ("llm.proxy.limits", "LLM_LIMITS_FIELDS", None),
        ("llm.proxy.tokens[]", "LINK_TOKEN_FIELDS", None),
        ("llm.proxy.access", "ACCESS_FIELDS", None),
        (
            "llm.proxy.console",
            "CONSOLE_FIELDS",
            Some("CONSOLE_FIT_ONLY"),
        ),
        ("llm.proxy.console.users[]", "CONSOLE_USER_FIELDS", None),
        ("llm.proxy.console.access", "ACCESS_FIELDS", None),
        ("llm.node.models[]", "LLM_MODEL_FIELDS", None),
        ("llm.node.settings", "LLM_SETTINGS_FIELDS", None),
        ("llm.node.proxy", "LLM_NODE_PROXY_FIELDS", None),
        ("llm.proxy.api", "LLM_API_FIELDS", None),
        ("llm.proxy.api_keys[]", "LLM_KEY_FIELDS", None),
        // I110: the WebRTC roles (crate::webrtc) and handlers.
        ("webrtc", "WEBRTC_FIELDS", None),
        ("webrtc.turn", "TURN_FIELDS", None),
        ("webrtc.turn.tls", "TURN_TLS_FIELDS", None),
        ("webrtc.turn.tls.certificates[]", "CERTIFICATE_FIELDS", None),
        ("webrtc.turn.users[]", "TURN_USER_FIELDS", None),
        ("webrtc.turn.access", "ACCESS_FIELDS", None),
        ("webrtc.media", "MEDIA_FIELDS", None),
        ("webrtc.media.access", "ACCESS_FIELDS", None),
        ("webrtc.media.ice_servers[]", "ICE_SERVER_FIELDS", None),
        ("webrtc.proxy", "WEBRTC_PROXY_ROLE_FIELDS", None),
        ("webrtc.proxy.access", "ACCESS_FIELDS", None),
        (
            "routes[].handler<whip>.params",
            "WHIP_FIELDS",
            Some("WHIP_FIT_ONLY"),
        ),
        (
            "routes[].handler<whep>.params",
            "WHIP_FIELDS",
            Some("WHIP_FIT_ONLY"),
        ),
        (
            "routes[].handler<webrtc_proxy>.params",
            "WEBRTC_PROXY_FIELDS",
            Some("WEBRTC_PROXY_FIT_ONLY"),
        ),
        // I150: the BitTorrent roles (crate::bittorrent).
        ("bittorrent", "BITTORRENT_FIELDS", None),
        (
            "bittorrent.client",
            "BT_CLIENT_FIELDS",
            Some("BT_CLIENT_FIT_ONLY"),
        ),
        (
            "bittorrent.client.limits",
            "BT_LIMITS_FIELDS",
            Some("BT_LIMITS_FIT_ONLY"),
        ),
        (
            "bittorrent.client.categories[]",
            "BT_CATEGORY_FIELDS",
            Some("BT_CATEGORY_FIT_ONLY"),
        ),
        (
            "bittorrent.client.categories[].limits",
            "BT_TORRENT_LIMITS_FIELDS",
            Some("BT_TORRENT_LIMITS_FIT_ONLY"),
        ),
        ("bittorrent.client.alt_speed", "BT_ALT_SPEED_FIELDS", None),
        (
            "bittorrent.client.alt_speed.schedules[]",
            "BT_SCHEDULE_FIELDS",
            None,
        ),
        (
            "bittorrent.client.console",
            "CONSOLE_FIELDS",
            Some("CONSOLE_FIT_ONLY"),
        ),
        (
            "bittorrent.client.console.users[]",
            "CONSOLE_USER_FIELDS",
            None,
        ),
        ("bittorrent.client.console.access", "ACCESS_FIELDS", None),
        (
            "bittorrent.tracker",
            "BT_TRACKER_FIELDS",
            Some("BT_TRACKER_FIT_ONLY"),
        ),
        ("bittorrent.tracker.access", "ACCESS_FIELDS", None),
        (
            "bittorrent.tracker.console",
            "CONSOLE_FIELDS",
            Some("CONSOLE_FIT_ONLY"),
        ),
        (
            "bittorrent.tracker.console.users[]",
            "CONSOLE_USER_FIELDS",
            None,
        ),
        ("bittorrent.tracker.console.access", "ACCESS_FIELDS", None),
        // I120: media auth and the streaming role (crate::media).
        (
            "media_auth[]",
            "MEDIA_AUTH_FIELDS",
            Some("MEDIA_AUTH_FIT_ONLY"),
        ),
        ("media_auth[].tokens[]", "MEDIA_TOKEN_FIELDS", None),
        ("media_auth[].users[]", "MEDIA_USER_FIELDS", None),
        (
            "media_library[]",
            "MEDIA_LIBRARY_FIELDS",
            Some("MEDIA_LIBRARY_FIT_ONLY"),
        ),
        ("streaming", "STREAMING_FIELDS", Some("STREAMING_FIT_ONLY")),
        (
            "streaming.rtmp",
            "STREAM_RTMP_FIELDS",
            Some("STREAM_RTMP_FIT_ONLY"),
        ),
        (
            "streaming.rtsp",
            "STREAM_RTSP_FIELDS",
            Some("STREAM_RTSP_FIT_ONLY"),
        ),
        ("streaming.srt", "STREAM_SRT_FIELDS", None),
        ("streaming.webrtc", "STREAM_WEBRTC_FIELDS", None),
        (
            "streaming.webrtc.ice_servers[]",
            "STREAM_ICE_SERVER_FIELDS",
            None,
        ),
        ("streaming.access", "ACCESS_FIELDS", None),
        // I123: the streaming role's handlers.
        ("routes[].handler<httpflv>.params", "HTTPFLV_FIELDS", None),
        ("routes[].handler<hls>.params", "HLS_FIELDS", None),
        // I124: WHIP and WHEP on the role's WebRTC port.
        (
            "routes[].handler<stream_whip>.params",
            "STREAM_RTC_FIELDS",
            None,
        ),
        (
            "routes[].handler<stream_whep>.params",
            "STREAM_RTC_FIELDS",
            None,
        ),
        // I125: the media pages.
        (
            "routes[].handler<media_pages>.params",
            "MEDIA_PAGES_FIELDS",
            Some("MEDIA_PAGES_FIT_ONLY"),
        ),
        (
            "routes[].handler<media_pages>.params.links[]",
            "PAGE_LINK_FIELDS",
            None,
        ),
        (
            "routes[].handler<media_pages>.params.ice_servers[]",
            "STREAM_ICE_SERVER_FIELDS",
            None,
        ),
    ];
    let mapped: BTreeSet<&str> = objects
        .iter()
        .flat_map(|(_, r, n)| [Some(*r), *n])
        .flatten()
        // fit's logger names (config.Loggers), not fields: the keys log.levels and log.loggers take;
        // and the APIs an LLM model's `api` takes (I98), values too.
        // The codecs `webrtc.media.codecs` takes (I110), values too. The members a cluster's
        // templates own by element and those no manager owns (I136), config members, not fields.
        .chain([
            "MANAGED_LISTS",
            "UNOWNABLE",
            "FIT_HANDLERS",
            "FIT_MIDDLEWARE",
            "LOGGERS",
            "LLM_APIS",
            "MEDIA_CODECS",
        ])
        .chain(AKIT_ONLY.iter().map(|(_, l)| *l))
        .collect();
    for name in lists.keys() {
        assert!(
            mapped.contains(name.as_str()),
            "model.rs list {name} is not mapped to the inventory by this test: map it"
        );
    }
    let join = |prefix: &str, key: &str| {
        if prefix.is_empty() {
            key.to_owned()
        } else {
            format!("{prefix}.{key}")
        }
    };
    let mut read = BTreeSet::new();
    for (prefix, reads, refuses) in objects {
        let mut covered = BTreeSet::new();
        for (list_name, is_read) in [(Some(reads), true), (refuses, false)] {
            let Some(list_name) = list_name else { continue };
            for key in list(list_name) {
                let path = join(prefix, key);
                assert!(
                    known.contains(path.as_str()),
                    "model.rs {list_name}: {key:?} ({path}) is not a field fit accepts"
                );
                assert!(covered.insert(key.clone()), "model.rs lists {path} twice");
                if is_read {
                    read.insert(path);
                }
            }
        }
        // The inventory's fields of this object (module rows aside: handler<respond> is a
        // module, not a field of `handler`).
        let fit: BTreeSet<String> = children(&inventory, prefix);
        assert_eq!(
            covered, fit,
            "at {prefix:?}, model.rs's lists and fit's fields differ"
        );
    }
    // akit's extensions (decision 010): fields fit does not have, read by akit-config. One fit
    // comes to have moves to the object's list of fields read.
    for (prefix, list_name) in AKIT_ONLY {
        for key in list(list_name) {
            let path = join(prefix, key);
            assert!(
                !known.contains(path.as_str()),
                "model.rs {list_name}: {path} is a field fit has now: list it with fit's"
            );
        }
    }
    // Handler modules: respond and proxy, which akit reads, and FIT_HANDLERS, which it refuses.
    let mut handlers: BTreeSet<String> = list("FIT_HANDLERS").iter().cloned().collect();
    for h in &handlers {
        assert!(
            known.contains(format!("routes[].handler<{h}>").as_str()),
            "model.rs FIT_HANDLERS: {h:?} is not a handler fit has"
        );
    }
    // I151: bt_tracker, a module of akit-bittorrent's, which akit-config hands the registry.
    for h in [
        "respond",
        "static",
        "proxy",
        "fastcgi",
        "forward_proxy",
        "whip",
        "whep",
        "webrtc_proxy",
        "bt_tracker",
        "httpflv",
        "hls",
        "stream_whip",
        "stream_whep",
        "media_pages",
    ] {
        assert!(handlers.insert(h.to_owned()), "FIT_HANDLERS lists {h}");
    }
    let fit_handlers: BTreeSet<String> = inventory
        .iter()
        .filter_map(|f| f.path.strip_prefix("routes[].handler<")?.strip_suffix('>'))
        .map(str::to_owned)
        .collect();
    assert_eq!(
        handlers, fit_handlers,
        "model.rs's handlers and fit's differ"
    );
    read.insert("routes[].handler<respond>".to_owned());
    read.insert("routes[].handler<respond>.params".to_owned());
    read.insert("routes[].handler<static>".to_owned());
    read.insert("routes[].handler<static>.params".to_owned());
    read.insert("routes[].handler<proxy>".to_owned());
    read.insert("routes[].handler<proxy>.params".to_owned());
    read.insert("routes[].handler<fastcgi>".to_owned());
    read.insert("routes[].handler<fastcgi>.params".to_owned());
    read.insert("routes[].handler<forward_proxy>".to_owned());
    read.insert("routes[].handler<forward_proxy>.params".to_owned());
    // I110: the WebRTC handlers; I123, I124: the streaming role's.
    for h in [
        "whip",
        "whep",
        "webrtc_proxy",
        "httpflv",
        "hls",
        "stream_whip",
        "stream_whep",
        "media_pages",
    ] {
        read.insert(format!("routes[].handler<{h}>"));
        read.insert(format!("routes[].handler<{h}>.params"));
    }
    read.insert("routes[].handler<bt_tracker>".to_owned());
    read.insert("routes[].handler<bt_tracker>.params".to_owned());
    // Middleware modules: rewrite, redirect, headers, the limits and auth, which akit reads, and
    // FIT_MIDDLEWARE, which it refuses.
    let mut middleware: BTreeSet<String> = list("FIT_MIDDLEWARE").iter().cloned().collect();
    for m in &middleware {
        assert!(
            known.contains(format!("routes[].middleware[]<{m}>").as_str()),
            "model.rs FIT_MIDDLEWARE: {m:?} is not a middleware fit has"
        );
    }
    for m in [
        "rewrite",
        "redirect",
        "headers",
        "rate_limit",
        "concurrency_limit",
        "body_limit",
        "basic_auth",
        "client_cert",
        "forward_auth",
        "circuit_breaker",
        "compress",
        "jwt",
        "cache",
        "wasm",
    ] {
        assert!(middleware.insert(m.to_owned()), "FIT_MIDDLEWARE lists {m}");
        read.insert(format!("routes[].middleware[]<{m}>"));
        read.insert(format!("routes[].middleware[]<{m}>.params"));
    }
    let fit_middleware: BTreeSet<String> = inventory
        .iter()
        .filter_map(|f| {
            f.path
                .strip_prefix("routes[].middleware[]<")?
                .strip_suffix('>')
        })
        .map(str::to_owned)
        .collect();
    assert_eq!(
        middleware, fit_middleware,
        "model.rs's middleware and fit's differ"
    );
    // Protocols: the seven akit serves, every one of fit's since I55.
    let mut protocols: BTreeSet<String> = BTreeSet::new();
    for p in [
        Protocol::Http11,
        Protocol::H2c,
        Protocol::H2,
        Protocol::H3,
        Protocol::Tcp,
        Protocol::Udp,
        Protocol::Socks,
    ] {
        assert!(protocols.insert(p.as_str().to_owned()), "{p:?} twice");
    }
    let ty = &inventory
        .iter()
        .find(|f| f.path == "listeners[].protocols")
        .unwrap()
        .ty;
    let fit_protocols: BTreeSet<String> = ty
        .strip_prefix("array<enum(")
        .and_then(|t| t.strip_suffix(")>"))
        .unwrap_or_else(|| panic!("listeners[].protocols is {ty}"))
        .split('|')
        .map(str::to_owned)
        .collect();
    assert_eq!(
        protocols, fit_protocols,
        "model.rs's protocols and fit's differ"
    );
    // What akit-config reads is what supported.txt marks.
    assert_eq!(
        read,
        supported().unwrap(),
        "the fields model.rs reads and those supported.txt marks differ"
    );
}

/// The keys of the inventory's fields directly under `prefix` (top-level keys for `""`), module
/// rows and module params aside.
fn children(inventory: &[Field], prefix: &str) -> BTreeSet<String> {
    inventory
        .iter()
        .filter_map(|f| {
            let rest = if prefix.is_empty() {
                f.path.as_str()
            } else {
                f.path.strip_prefix(prefix)?.strip_prefix('.')?
            };
            (!rest.contains(['.', '[', '<'])).then(|| rest.to_owned())
        })
        .collect()
}

/// The `const NAME: &[&str] = &[...];` lists of a Rust source, by name.
fn str_lists(src: &str) -> BTreeMap<String, Vec<String>> {
    let mut out = BTreeMap::new();
    let mut rest = src;
    while let Some(i) = rest.find("const ") {
        rest = rest.get(i..).unwrap().strip_prefix("const ").unwrap();
        let Some((name, after)) = rest.split_once(": &[&str] = &[") else {
            continue;
        };
        if !name.bytes().all(|b| b.is_ascii_uppercase() || b == b'_') {
            continue;
        }
        let (body, _) = after.split_once("];").unwrap();
        let items = body
            .split('"')
            .skip(1)
            .step_by(2)
            .map(str::to_owned)
            .collect();
        out.insert(name.to_owned(), items);
        rest = after;
    }
    out
}

#[test]
fn inventory_paths_for_akit_config_paths() {
    let inv = fields().unwrap();
    assert_eq!(
        inventory_path(&inv, "routes[3].handler<proxy>"),
        Some("routes[].handler<proxy>")
    );
    assert_eq!(
        inventory_path(&inv, "routes[0].handler"),
        Some("routes[].handler")
    );
    assert_eq!(
        inventory_path(&inv, "listeners[0].protocols[1]"),
        Some("listeners[].protocols")
    );
    assert_eq!(inventory_path(&inv, "upstreams"), Some("upstreams"));
    assert_eq!(inventory_path(&inv, "routes[0].nope"), None);
    assert_eq!(inventory_path(&inv, "routes[0].handler[1]"), None);
    assert_eq!(
        inventory_path(&inv, "routes[0].listeners[1]"),
        Some("routes[].listeners")
    );
}

#[test]
fn findings_from_akit_config_errors() {
    let errs = akit_config::from_json(
        r#"{"listeners": [{"name": "w", "address": ":1", "mptcp": true}],
            "routes": [{"handler": {"module": "vod"}}], "engine": {"loop_wait": "1ms"}}"#,
        None,
    )
    .unwrap_err();
    let got: BTreeSet<String> = errs
        .as_slice()
        .iter()
        .map(|e| finding(e).to_string())
        .collect();
    let want: BTreeSet<String> = [
        "listeners[0].mptcp",
        "routes[0].handler<vod>",
        "engine.loop_wait",
    ]
    .iter()
    .map(|p| format!("not supported yet: {p}"))
    .collect();
    assert_eq!(got, want);
    // Anything else is an error.
    let errs = akit_config::from_json(r#"{"listener": []}"#, None).unwrap_err();
    assert!(matches!(finding(&errs.as_slice()[0]), Finding::Error(_)));
    // A value refused is reported with the value, under its field's inventory path, and is not
    // the field refused: listeners[].protocols stays supported.
    let fixtures = fixtures().unwrap();
    let report = run_gate(&fixtures[..1], &fields().unwrap(), |_| {
        Err(vec![Finding::UnsupportedValue {
            path: "listeners[1].protocols[2]".into(),
            what: "the \"h3\" protocol".into(),
        }])
    });
    let line = format!(
        "{}: not supported yet: listeners[].protocols: the \"h3\" protocol",
        fixtures[0].name
    );
    assert_eq!(report.render(), std::slice::from_ref(&line));
    assert!(!report.unsupported().contains("listeners[].protocols"));
    // At a path that is not fit's, it fails.
    let report = run_gate(&fixtures[..1], &fields().unwrap(), |_| {
        Err(vec![Finding::UnsupportedValue {
            path: "nope".into(),
            what: "x".into(),
        }])
    });
    assert_eq!(report.failures().len(), 1);
}

#[test]
fn formats_without_a_reader_are_their_own_outcome() {
    let fixtures = fixtures().unwrap();
    let inventory = fields().unwrap();
    let report = run_gate(&fixtures, &inventory, |p| {
        Err(vec![Finding::NoReader(format!("{}", p.display()))])
    });
    assert!(report.failures().is_empty());
    assert!(matches!(
        report.outcomes["templates/conf/fit.conf"],
        Outcome::NoReader(_)
    ));
    // Beside other findings, it fails: a reader that is half there is not "no reader".
    let report = run_gate(&fixtures, &inventory, |_| {
        Err(vec![
            Finding::NoReader("x".into()),
            Finding::Unsupported("acme".into()),
        ])
    });
    assert_eq!(report.failures().len(), fixtures.len() - NO_VALIDATE.len());
    // A changed outcome is a changed line of the report, either way.
    let report = run_gate(&fixtures[..1], &inventory, |_| {
        Err(vec![Finding::Unsupported("routes[2].middleware".into())])
    });
    let name = &fixtures[0].name;
    let line = format!("{name}: not supported yet: routes[].middleware");
    assert_eq!(report.render(), std::slice::from_ref(&line));
    assert!(report.diff(&format!("# header\n{line}\n")).is_empty());
    assert_eq!(
        report.diff(&format!("{line}\n{name}: not supported yet: acme\n")),
        [format!("- {name}: not supported yet: acme")]
    );
    assert_eq!(report.diff(""), [format!("+ {line}")]);
}

/// The settings of the config's first provider, of the kind each names (I61).
fn consul(c: &Config) -> &akit_config::ConsulProvider {
    match c.providers.first().map(|p| &p.kind) {
        Some(akit_config::ProviderKind::Consul(p)) => p,
        other => unreachable!("{other:?}"),
    }
}

fn consul_kv(c: &Config) -> &akit_config::ConsulKvProvider {
    match c.providers.first().map(|p| &p.kind) {
        Some(akit_config::ProviderKind::ConsulKv(p)) => p,
        other => unreachable!("{other:?}"),
    }
}

fn log(c: &Config) -> &akit_config::Log {
    c.log.as_ref().unwrap()
}

fn rotate(c: &Config) -> &akit_config::Rotate {
    c.access_log
        .as_ref()
        .and_then(|a| a.rotate.as_ref())
        .unwrap()
}

fn etcd(c: &Config) -> &akit_config::EtcdProvider {
    match c.providers.first().map(|p| &p.kind) {
        Some(akit_config::ProviderKind::Etcd(p)) => p,
        other => unreachable!("{other:?}"),
    }
}

fn shared_storage(c: &Config) -> &akit_config::SharedStorage {
    c.acme
        .as_ref()
        .and_then(|a| a.shared_storage.as_ref())
        .unwrap()
}

fn redis_storage(c: &Config) -> &akit_config::RedisStorage {
    match &shared_storage(c).kind {
        akit_config::SharedStorageKind::Redis(r) => r,
        other => unreachable!("{other:?}"),
    }
}

fn etcd_storage(c: &Config) -> &akit_config::EtcdStorage {
    match &shared_storage(c).kind {
        akit_config::SharedStorageKind::Etcd(e) => e,
        other => unreachable!("{other:?}"),
    }
}

fn consul_storage(c: &Config) -> &akit_config::ConsulStorage {
    match &shared_storage(c).kind {
        akit_config::SharedStorageKind::Consul(c) => c,
        other => unreachable!("{other:?}"),
    }
}

/// The config reference (docs/reference/config.md and its Chinese twin, I78), whose tables
/// `tools/config-reference.py` generates, against akit-config: every object akit reads has its
/// table, each table lists every field fit's object has, a field marked read exactly when
/// `supported.txt` (held to `model.rs` by `akit_config_lists_match_the_inventory`) marks it, every
/// handler and middleware module is listed as `model.rs` reads or refuses it, and the extensions
/// shown are `model.rs`'s `*_AKIT_ONLY` lists. CI's `config-reference.py --check` holds the
/// tables to the generator; this holds them to the model.
#[test]
fn reference_matches_akit_config() {
    let inventory = fields().unwrap();
    let supported = supported().unwrap();
    let src =
        fs::read_to_string(compat_dir().join("../../crates/akit-config/src/model.rs")).unwrap();
    let lists = str_lists(&src);
    let mut extensions = BTreeSet::new();
    for (prefix, list_name) in AKIT_ONLY {
        for key in &lists[list_name] {
            extensions.insert(format!("{prefix}.{key}"));
        }
    }
    // The objects akit reads, each with a table: the top level, and every field read that has
    // fields of its own (an array's items, `listeners[]`, a map's values, `log.loggers.*`, and a
    // module's params among them).
    let mut objects = BTreeSet::from([String::new()]);
    for p in &supported {
        if p.ends_with('>') {
            continue;
        }
        for obj in [p.clone(), format!("{p}[]"), format!("{p}.*")] {
            if !children(&inventory, &obj).is_empty() {
                objects.insert(obj);
            }
        }
    }
    let join = |obj: &str, key: &str| {
        if obj.is_empty() {
            key.to_owned()
        } else {
            format!("{obj}.{key}")
        }
    };
    for (doc, not_yet, extension) in [
        ("config.md", "**Not supported yet**", "**akit's extension**"),
        ("config.zh-CN.md", "**尚不支持**", "**akit 扩展**"),
    ] {
        let text = fs::read_to_string(compat_dir().join("../../docs/reference").join(doc)).unwrap();
        let mut tables = BTreeSet::new();
        let mut read = BTreeSet::new();
        let mut refused = BTreeSet::new();
        let mut extended = BTreeSet::new();
        let mut rest = text.as_str();
        while let Some(i) = rest.find("<!-- BEGIN GENERATED: ") {
            let after = &rest[i + "<!-- BEGIN GENERATED: ".len()..];
            let (key, after) = after.split_once(" -->").unwrap();
            let (block, after) = after.split_once("<!-- END GENERATED -->").unwrap();
            rest = after;
            let rows = block.lines().filter_map(|l| {
                let l = l.strip_prefix("| `")?;
                let (name, cells) = l.split_once("` |")?;
                Some((name.to_owned(), cells.to_owned()))
            });
            if let Some(at) = key.strip_prefix("modules:") {
                for (m, status) in rows {
                    let path = format!("{at}<{m}>");
                    assert!(
                        inventory.iter().any(|f| f.path == path),
                        "{doc}: module {path} is not one of fit's"
                    );
                    let is_read =
                        !status.contains("**not supported yet**") && !status.contains(not_yet);
                    assert_eq!(is_read, supported.contains(&path), "{doc}: module {path}");
                    tables.insert(format!("modules:{at}"));
                }
                continue;
            }
            if key == "summary" || key == "extensions" {
                continue;
            }
            let obj = if key == "config" { "" } else { key };
            assert!(tables.insert(obj.to_owned()), "{doc}: two tables for {key}");
            let mut keys = BTreeSet::new();
            for (name, cells) in rows {
                let path = join(obj, &name);
                if cells.contains(extension) {
                    extended.insert(path);
                    continue;
                }
                keys.insert(name);
                if cells.contains(not_yet) {
                    refused.insert(path);
                } else {
                    read.insert(path);
                }
            }
            assert_eq!(
                keys,
                children(&inventory, obj),
                "{doc}: the table of {key} and fit's fields of it differ"
            );
        }
        let shown: BTreeSet<String> = tables
            .iter()
            .filter(|t| !t.starts_with("modules:"))
            .cloned()
            .collect();
        assert_eq!(
            shown, objects,
            "{doc}: the tables and the objects akit reads differ"
        );
        let fields_read: BTreeSet<String> = supported
            .iter()
            .filter(|p| !p.ends_with('>') && !p.ends_with(">.params"))
            .cloned()
            .collect();
        assert_eq!(
            read, fields_read,
            "{doc}: the fields shown read and those akit reads differ"
        );
        for p in &refused {
            assert!(
                !supported.contains(p),
                "{doc}: {p} is shown refused, and akit reads it"
            );
        }
        assert_eq!(
            extended, extensions,
            "{doc}: the extensions shown and model.rs's differ"
        );
        for at in ["routes[].handler", "routes[].middleware[]"] {
            assert!(
                tables.contains(&format!("modules:{at}")),
                "{doc}: no list of the modules at {at}"
            );
        }
    }
}

/// A config whose `admin.console` has one user and `extra` members (I89).
fn console_sample(extra: &str) -> String {
    format!(
        r#"{{"admin": {{"console": {{"users": [{{"username": "a", "password_hash": "$2b$04$x"}}]{extra}}}}}}}"#
    )
}

/// A config with `admin.console` and `admin.cluster` with `extra` members (I130).
fn cluster_sample(extra: &str) -> String {
    format!(
        r#"{{"admin": {{"console": {{"users": [{{"username": "a", "password_hash": "$2b$04$x"}}]}}, "cluster": {{{extra}}}}}}}"#
    )
}

/// The `admin.cluster` of `c`.
fn cluster_of(c: &Config) -> Option<&akit_config::cluster::Cluster> {
    c.admin.as_ref().unwrap().cluster.as_ref()
}

/// A config with `admin.cluster_member`, its manager and pin, and `extra` members (I130).
fn member_sample(extra: &str) -> String {
    format!(
        r#"{{"admin": {{"cluster_member": {{"manager": "wss://m:7444/cluster/link", "fingerprint": "{}", {extra}}}}}}}"#,
        "0a".repeat(32)
    )
}

/// The `admin.cluster_member` of `c`.
fn member_of(c: &Config) -> Option<&akit_config::cluster::ClusterMember> {
    c.admin.as_ref().unwrap().cluster_member.as_ref()
}

/// The `admin.console` of `c`.
fn console_of(c: &Config) -> &akit_config::Console {
    c.admin.as_ref().unwrap().console.as_ref().unwrap()
}
