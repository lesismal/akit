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
/// problem into a finding. A format akit has no reader for yet is [`Finding::NoReader`], with
/// the iteration that brings the reader.
fn validate_with_akit_config(path: &Path) -> Result<(), Vec<Finding>> {
    if path.is_dir() {
        // A directory's files, merged as fit merges them (I06), when akit reads their format.
        let files = akit_config::config_files(&[path])
            .map_err(|errs| errs.as_slice().iter().map(finding).collect::<Vec<_>>())?;
        let ext = |f: &Path| f.extension().and_then(|e| e.to_str()).map(str::to_owned);
        if files.iter().any(|f| ext(f).as_deref() == Some("conf")) {
            return Err(vec![Finding::NoReader(".conf (I23)".into())]);
        }
        if files
            .iter()
            .any(|f| matches!(ext(f).as_deref(), Some("yaml" | "yml")))
        {
            return Err(vec![Finding::NoReader("YAML (I24)".into())]);
        }
        return akit_config::from_files(&[path])
            .map(|_| ())
            .map_err(|errs| errs.as_slice().iter().map(finding).collect());
    }
    match path.extension().and_then(|e| e.to_str()) {
        Some("json") => akit_config::from_json_file(path)
            .map(|_| ())
            .map_err(|errs| errs.as_slice().iter().map(finding).collect()),
        Some("conf") => Err(vec![Finding::NoReader(".conf (I23)".into())]),
        Some("yaml" | "yml") => Err(vec![Finding::NoReader("YAML (I24)".into())]),
        _ => Err(vec![Finding::Error(format!(
            "{}: not a config file fit reads",
            path.display()
        ))]),
    }
}

/// An akit-config problem as a finding. "not supported yet" names its field by the problem's
/// path; a handler module akit does not have is refused at `<handler>.module` as
/// `the "<module>" handler`, which the inventory writes `<handler><<module>>`.
fn finding(e: &akit_config::Error) -> Finding {
    if !e.is_not_supported_yet() {
        return Finding::Error(e.to_string());
    }
    let module = e
        .msg
        .strip_prefix(akit_config::NOT_SUPPORTED_YET)
        .and_then(|r| r.strip_prefix(": the \""))
        .and_then(|r| r.strip_suffix("\" handler"));
    match (e.path.strip_suffix(".module"), module) {
        (Some(handler), Some(m)) => Finding::Unsupported(format!("{handler}<{m}>")),
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
    let route = |fields: &str| {
        format!(r#"{{{L}, "routes": [{{{fields}, "handler": {{"module": "respond"}}}}]}}"#)
    };
    let samples: Vec<(&str, String, Check)> = vec![
        ("schema_version", r#"{"schema_version": 1}"#.into(), |c| {
            c.schema_version == 1
        }),
        ("listeners", format!("{{{L}}}"), |c| c.listeners.len() == 1),
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

fn respond_of(c: &Config) -> &akit_config::Respond {
    match &c.routes.first().unwrap().handler {
        Handler::Respond(r) => r,
    }
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
    let objects: [(&str, &str, Option<&str>); 6] = [
        ("", "CONFIG_FIELDS", Some("CONFIG_FIT_ONLY")),
        ("listeners[]", "LISTENER_FIELDS", Some("LISTENER_FIT_ONLY")),
        ("routes[]", "ROUTE_FIELDS", Some("ROUTE_FIT_ONLY")),
        ("routes[].match", "MATCH_FIELDS", Some("MATCH_FIT_ONLY")),
        ("routes[].handler", "MODULE_FIELDS", None),
        ("routes[].handler<respond>.params", "RESPOND_FIELDS", None),
    ];
    let mapped: BTreeSet<&str> = objects
        .iter()
        .flat_map(|(_, r, n)| [Some(*r), *n])
        .flatten()
        .chain(["FIT_HANDLERS", "FIT_PROTOCOLS"])
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
    // Handler modules: respond, which akit reads, and FIT_HANDLERS, which it refuses.
    let mut handlers: BTreeSet<String> = list("FIT_HANDLERS").iter().cloned().collect();
    for h in &handlers {
        assert!(
            known.contains(format!("routes[].handler<{h}>").as_str()),
            "model.rs FIT_HANDLERS: {h:?} is not a handler fit has"
        );
    }
    assert!(
        handlers.insert("respond".to_owned()),
        "FIT_HANDLERS lists respond"
    );
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
    // Protocols: the two akit serves and FIT_PROTOCOLS.
    let mut protocols: BTreeSet<String> = list("FIT_PROTOCOLS").iter().cloned().collect();
    for p in [Protocol::Http11, Protocol::H2c] {
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
        r#"{"listeners": [{"name": "w", "address": ":1", "protocols": ["h3"], "mptcp": true}],
            "routes": [{"handler": {"module": "proxy"}}], "acme": {}}"#,
        None,
    )
    .unwrap_err();
    let got: BTreeSet<String> = errs
        .as_slice()
        .iter()
        .map(|e| finding(e).to_string())
        .collect();
    let want: BTreeSet<String> = [
        "listeners[0].protocols[0]",
        "listeners[0].mptcp",
        "routes[0].handler<proxy>",
        "acme",
    ]
    .iter()
    .map(|p| format!("not supported yet: {p}"))
    .collect();
    assert_eq!(got, want);
    // Anything else is an error.
    let errs = akit_config::from_json(r#"{"listener": []}"#, None).unwrap_err();
    assert!(matches!(finding(&errs.as_slice()[0]), Finding::Error(_)));
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
