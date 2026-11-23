//! The compatibility gate (docs/compatibility.md): the pinned fixtures and the field inventory
//! are sound, the harness judges outcomes as decision 002 says, and, with the `compat-gate`
//! feature (I07b), every fixture passes through `akit_config::from_json_file`.

use std::collections::BTreeSet;
use std::fs;
use std::path::Path;

use akit_compat::{
    EXAMPLE_SECRETS, Finding, Kind, NO_VALIDATE, Outcome, TEMPLATE_FORMATS, fields, fixtures,
    fixtures_dir, path_matches, pins, run_gate, supported,
};

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
    let text = fs::read_to_string(akit_compat::compat_dir().join("fit-fields.txt")).unwrap();
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

/// The gate itself, through akit-config (I07b). Run it with `make compat-gate`, which sets the
/// examples' secrets to stand-ins.
#[cfg(feature = "compat-gate")]
#[test]
fn gate_through_akit_config() {
    for name in EXAMPLE_SECRETS {
        assert!(
            std::env::var_os(name).is_some(),
            "{name} is not set: run make compat-gate"
        );
    }
    let report = run_gate(
        &fixtures().unwrap(),
        &fields().unwrap(),
        validate_with_akit_config,
    );
    println!("{report}");
    let failures = report.failures();
    assert!(
        failures.is_empty(),
        "{} fixtures fail the gate:\n{report}",
        failures.len()
    );
}

/// The one hook into akit-config (I02's API): read the config, and turn each error into a
/// finding, "not supported yet" by `Error::is_not_supported_yet` (message prefix
/// `akit_config::NOT_SUPPORTED_YET`). I07b extends it to directories and, as I23 and I24 land,
/// to .conf and YAML; until then those fixtures are findings the gate reports.
#[cfg(feature = "compat-gate")]
fn validate_with_akit_config(path: &Path) -> Result<(), Vec<Finding>> {
    let is_json = path.extension().is_some_and(|e| e == "json");
    if !is_json {
        return Err(vec![Finding::Error(format!(
            "{}: akit-config reads JSON files only so far (I07b, I23, I24)",
            path.display()
        ))]);
    }
    akit_config::from_json_file(path)
        .map(|_| ())
        .map_err(|errs| {
            errs.as_slice()
                .iter()
                .map(|e| {
                    let msg = e.to_string();
                    if e.is_not_supported_yet() {
                        Finding::from_message(&msg)
                    } else {
                        Finding::Error(msg)
                    }
                })
                .collect()
        })
}

#[cfg(not(feature = "compat-gate"))]
#[test]
#[ignore = "I07b: needs akit_config::from_json_file (I02); run with --features compat-gate"]
fn gate_through_akit_config() {
    assert_eq!(EXAMPLE_SECRETS.len(), 15);
}
