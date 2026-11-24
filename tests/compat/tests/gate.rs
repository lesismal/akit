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
use akit_config::{Config, Handler, Protocol};

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
/// `the "<module>" middleware's <field>: ...`, at the field of `<ref><<module>>.params`; any other
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
    if let Some((m, _)) = quoted.and_then(|r| r.split_once("\" middleware's ")) {
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
    let upstream = |fields: &str| {
        format!(r#"{{"upstreams": [{{"name": "app", "addresses": ["127.0.0.1:9000"]{fields}}}]}}"#)
    };
    let proxy = |params: &str| {
        format!(
            r#"{{{L}, "upstreams": [{{"name": "app", "addresses": ["127.0.0.1:9000"]}}], "routes": [{{"handler": {{"module": "proxy", "params": {{"upstream": "app"{params}}}}}}}]}}"#
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
        ("limits", r#"{"limits": {}}"#.into(), |c| {
            c.limits == akit_config::Limits::default()
        }),
        (
            "limits.max_body_bytes",
            format!(r#"{{{L}, "limits": {{"max_body_bytes": 5}}}}"#),
            |c| c.limits.max_body_bytes == 5 && c.listeners[0].max_body_bytes == 5,
        ),
        ("listeners[].name", format!("{{{L}}}"), |c| {
            c.listeners[0].name == "web"
        }),
        ("listeners[].address", format!("{{{L}}}"), |c| {
            c.listeners[0].address == ":8080"
        }),
        (
            "listeners[].protocols",
            r#"{"listeners": [{"name": "w", "address": ":1", "protocols": ["h2c"]}]}"#.into(),
            |c| c.listeners[0].protocols == [Protocol::H2c],
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
        // 1.3, the default; 1.2 is refused as not supported yet (akio API request A9).
        (
            "listeners[].tls.max_version",
            tls(r#", "max_version": "1.3""#),
            |c| c.listeners[0].tls.as_ref().unwrap().max_version == akit_config::TlsVersion::Tls13,
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
            static_of(c).index == "index.html"
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
            |c| static_of(c).index == "default.htm",
        ),
        (
            "routes[].handler<static>.params.max_file_bytes",
            statics(r#", "max_file_bytes": 4096"#),
            |c| static_of(c).max_file_bytes == 4096,
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
            basic_of(c).realm == "fit" && basic_of(c).user_header.is_empty()
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
                == Some("fit_sticky")
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
    ];
    let marked = supported().unwrap();
    let sampled: BTreeSet<String> = samples.iter().map(|(p, ..)| (*p).to_owned()).collect();
    assert_eq!(
        sampled, marked,
        "supported.txt and this test's samples must name the same fields"
    );
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

fn proxy_of(c: &Config) -> &akit_config::Proxy {
    match &c.routes.first().unwrap().handler {
        Handler::Proxy(p) => Some(p),
        _ => None,
    }
    .unwrap()
}

/// akit-config's lists of what it reads and of what fit reads that it refuses as not supported
/// yet (`crates/akit-config/src/model.rs`), read from the source: each entry is a field (or
/// handler module, or protocol) of the inventory, together they cover every field fit has at each
/// object akit-config reads (so no field fit accepts is refused as unknown), and what akit-config
/// reads is what `supported.txt` marks.
#[test]
fn akit_config_lists_match_the_inventory() {
    let src =
        fs::read_to_string(compat_dir().join("../../crates/akit-config/src/model.rs")).unwrap();
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
    let objects: [(&str, &str, Option<&str>); 30] = [
        ("", "CONFIG_FIELDS", Some("CONFIG_FIT_ONLY")),
        ("admin", "ADMIN_FIELDS", Some("ADMIN_FIT_ONLY")),
        ("acme", "ACME_FIELDS", Some("ACME_FIT_ONLY")),
        ("limits", "LIMITS_FIELDS", Some("LIMITS_FIT_ONLY")),
        (
            "access_log",
            "ACCESS_LOG_FIELDS",
            Some("ACCESS_LOG_FIT_ONLY"),
        ),
        ("listeners[]", "LISTENER_FIELDS", Some("LISTENER_FIT_ONLY")),
        ("listeners[].tls", "TLS_FIELDS", Some("TLS_FIT_ONLY")),
        ("listeners[].tls.certificates[]", "CERTIFICATE_FIELDS", None),
        ("listeners[].tls.client_auth", "CLIENT_AUTH_FIELDS", None),
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
        ("upstreams[]", "UPSTREAM_FIELDS", Some("UPSTREAM_FIT_ONLY")),
        ("upstreams[].health_check", "HEALTH_CHECK_FIELDS", None),
        ("upstreams[].sticky_cookie", "STICKY_COOKIE_FIELDS", None),
        ("routes[].middleware[]", "MODULE_FIELDS", None),
        (
            "routes[].middleware[]<rewrite>.params",
            "REWRITE_FIELDS",
            Some("REWRITE_FIT_ONLY"),
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
    ];
    let mapped: BTreeSet<&str> = objects
        .iter()
        .flat_map(|(_, r, n)| [Some(*r), *n])
        .flatten()
        .chain(["FIT_HANDLERS", "FIT_MIDDLEWARE", "FIT_PROTOCOLS"])
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
    // Handler modules: respond and proxy, which akit reads, and FIT_HANDLERS, which it refuses.
    let mut handlers: BTreeSet<String> = list("FIT_HANDLERS").iter().cloned().collect();
    for h in &handlers {
        assert!(
            known.contains(format!("routes[].handler<{h}>").as_str()),
            "model.rs FIT_HANDLERS: {h:?} is not a handler fit has"
        );
    }
    for h in ["respond", "static", "proxy"] {
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
    // Protocols: the four akit serves and FIT_PROTOCOLS.
    let mut protocols: BTreeSet<String> = list("FIT_PROTOCOLS").iter().cloned().collect();
    for p in [Protocol::Http11, Protocol::H2c, Protocol::H2, Protocol::H3] {
        assert!(
            protocols.insert(p.as_str().to_owned()),
            "FIT_PROTOCOLS lists {p:?}"
        );
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
        r#"{"listeners": [{"name": "w", "address": ":1", "protocols": ["udp"], "mptcp": true}],
            "routes": [{"handler": {"module": "fastcgi"}}], "acme": {"shared_storage": {}}}"#,
        None,
    )
    .unwrap_err();
    let got: BTreeSet<String> = errs
        .as_slice()
        .iter()
        .map(|e| finding(e).to_string())
        .collect();
    let want: BTreeSet<String> = [
        "listeners[0].protocols[0]: the \"udp\" protocol",
        "listeners[0].mptcp",
        "routes[0].handler<fastcgi>",
        "acme.shared_storage",
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
