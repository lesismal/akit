//! `akit adapt` and `akit fmt` (I73), end to end: fit's templates and examples converted to each
//! format byte for byte as `fit adapt` converts them (golden files fit wrote, with
//! `tools/adapt-golden/run.sh`), round trips through every format, and fit's own `adapt` tests.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]

use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};

/// The stand-ins for the secrets fit's examples read from the environment, as the compatibility
/// gate sets them.
const SECRETS: &[&str] = &[
    "ES_PASSWORD",
    "KAFKA_PASSWORD",
    "MYSQL_PASSWORD",
    "PG_PASSWORD",
    "RABBITMQ_PASSWORD",
    "NATS_PASSWORD",
    "PULSAR_TOKEN",
    "MQTT_PASSWORD",
    "INFLUX_TOKEN",
    "TD_PASSWORD",
    "MSSQL_PASSWORD",
    "DORIS_PASSWORD",
    "OPENAI_API_KEY",
    "QDRANT_API_KEY",
    "MILVUS_TOKEN",
];

/// akit's root, where the cases' paths start.
fn root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}

fn dir(name: &str) -> PathBuf {
    let d = PathBuf::from(env!("CARGO_TARGET_TMPDIR"))
        .join("adapt")
        .join(name);
    let _ = fs::remove_dir_all(&d);
    fs::create_dir_all(&d).unwrap();
    d
}

fn write(d: &Path, name: &str, text: &str) -> PathBuf {
    let p = d.join(name);
    fs::create_dir_all(p.parent().unwrap()).unwrap();
    fs::write(&p, text).unwrap();
    p
}

fn akit_in(cwd: &Path, args: &[&str], stdin: Option<&str>) -> Output {
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_akit"));
    cmd.current_dir(cwd).args(args);
    for s in SECRETS {
        cmd.env(s, "example-password");
    }
    cmd.stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let mut child = cmd.spawn().unwrap();
    let mut input = child.stdin.take().unwrap();
    input
        .write_all(stdin.unwrap_or_default().as_bytes())
        .unwrap();
    drop(input);
    child.wait_with_output().unwrap()
}

fn akit(args: &[&str]) -> Output {
    akit_in(&root(), args, None)
}

fn text(b: &[u8]) -> String {
    String::from_utf8_lossy(b).into_owned()
}

/// A JSON document as a value, its numbers read as numbers and its members in any order, as fit's
/// tests compare documents (Go maps).
fn same_json(a: &str, b: &str) -> bool {
    fn norm(v: akit_config::value::Value) -> akit_config::value::Value {
        use akit_config::value::Value;
        match v {
            Value::Number(n) => Value::Number(format!("{}", n.parse::<f64>().unwrap())),
            Value::Array(a) => Value::Array(a.into_iter().map(norm).collect()),
            Value::Object(ms) => {
                let mut ms: Vec<_> = ms.into_iter().map(|(k, v)| (k, norm(v))).collect();
                ms.sort_by(|a, b| a.0.cmp(&b.0));
                Value::Object(ms)
            }
            v => v,
        }
    }
    match (akit_config::value::parse(a), akit_config::value::parse(b)) {
        (Ok(x), Ok(y)) => norm(x) == norm(y),
        _ => false,
    }
}

/// Each case of `testdata/adapt/cases.txt`: an input, a target, fit's exit code and its output.
fn cases() -> Vec<(String, String, String, Option<String>)> {
    let base = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/testdata/adapt");
    fs::read_to_string(base.join("cases.txt"))
        .unwrap()
        .lines()
        .map(|l| {
            let c: Vec<&str> = l.split('\t').collect();
            let golden = (c[3] != "-").then(|| fs::read_to_string(base.join(c[3])).unwrap());
            (c[0].to_owned(), c[1].to_owned(), c[2].to_owned(), golden)
        })
        .collect()
}

#[test]
fn fits_templates_and_examples_convert_as_fit_converts_them() {
    let cases = cases();
    assert!(cases.len() > 240, "only {} cases", cases.len());
    let mut wrong = Vec::new();
    for (input, to, code, golden) in &cases {
        let out = akit(&["adapt", "-c", input, "-to", to]);
        let got = out.status.code().unwrap().to_string();
        let same = match golden {
            Some(g) => got == *code && text(&out.stdout) == *g,
            None => got == *code,
        };
        if !same {
            wrong.push(format!(
                "{input} -to {to}: exit {got} (fit: {code})\n{}\n--- akit\n{}--- fit\n{}",
                text(&out.stderr),
                text(&out.stdout),
                golden.as_deref().unwrap_or_default()
            ));
        }
    }
    assert!(
        wrong.is_empty(),
        "{} of {} cases differ from fit:\n{}",
        wrong.len(),
        cases.len(),
        wrong.iter().take(5).cloned().collect::<String>()
    );
}

/// fit's nginx configs and Caddyfiles (its converters' samples, its assistant's migration cases
/// and its benchmark's configs, copied to `testdata/adapt/convert`) converted with `akit adapt
/// -from nginx` and `-from caddyfile` to each format as `fit adapt` converts them: the exit code,
/// the output and the warnings byte for byte (I74; golden files fit wrote, with
/// `tools/adapt-golden/run.sh`).
#[test]
fn nginx_configs_and_caddyfiles_convert_as_fit_converts_them() {
    let base = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/testdata/adapt/convert");
    let cases = fs::read_to_string(base.join("../convert-cases.txt")).unwrap();
    let mut wrong = Vec::new();
    let mut n = 0;
    for l in cases.lines() {
        let c: Vec<&str> = l.split('\t').collect();
        let (input, from, to, code, golden) = (c[0], c[1], c[2], c[3], c[4]);
        n += 1;
        // The environment fit's golden files were written with, for a Caddyfile's {$NAME}.
        let out = Command::new(env!("CARGO_BIN_EXE_akit"))
            .current_dir(root())
            .args(["adapt", "-from", from, "-c", input, "-to", to])
            .env_clear()
            .env("FIT_TEST_PORT", "9000")
            .output()
            .unwrap();
        let want_out = fs::read_to_string(base.join(golden)).unwrap();
        let want_err = fs::read_to_string(base.join(format!("{golden}.stderr"))).unwrap();
        let got = out.status.code().unwrap().to_string();
        // akit's warnings of the fields it does not run yet (a converted error_log is fit's log)
        // come first; the rest is fit's.
        let stderr = text(&out.stderr);
        let (akit_warns, rest): (Vec<&str>, Vec<&str>) = stderr
            .lines()
            .partition(|l| l.starts_with("akit adapt: warning: "));
        let rest: String = rest.iter().map(|l| format!("{l}\n")).collect();
        let only_unsupported = akit_warns
            .iter()
            .all(|w| w.ends_with(": not supported yet; converted as written"));
        if got != code || text(&out.stdout) != want_out || rest != want_err || !only_unsupported {
            wrong.push(format!(
                "{input} -from {from} -to {to}: exit {got} (fit: {code})\n--- akit\n{}{}--- fit\n{want_err}{want_out}",
                text(&out.stderr),
                text(&out.stdout),
            ));
        }
    }
    assert!(n >= 30, "only {n} cases");
    assert!(
        wrong.is_empty(),
        "{} of {n} cases differ from fit:\n{}",
        wrong.len(),
        wrong.iter().take(3).cloned().collect::<String>()
    );
}

/// A converted config's problems are placed at the lines of the nginx config they came from,
/// as fit places them; the converter's warnings follow, each naming the file.
#[test]
fn converted_configs_place_their_problems_at_the_sources_lines() {
    let d = dir("convert-errors");
    let bad = write(
        &d,
        "bad.conf",
        "http {\n  server {\n    listen 80;\n    location /a {\n      return 204;\n    }\n    location b {\n      gzip_comp_level 0;\n      return 204;\n    }\n  }\n}\n",
    );
    let out = akit(&["adapt", "-from", "nginx", "-c", bad.to_str().unwrap()]);
    let err = text(&out.stderr);
    assert_eq!(out.status.code(), Some(1), "{err}");
    assert_eq!(
        err,
        format!(
            "{}:7:1: routes[1].match.path[0]: \"b*\" does not start with '/'\n",
            bad.display()
        )
    );

    // Standard input, written as fit's .conf.
    let src = "http { server { listen 8080; server_name a.test; ssl_protocols TLSv1.3; location / { return 200 \"hi\"; } } }\n";
    let out = akit_in(
        &root(),
        &["adapt", "-from", "nginx", "-c", "-", "-to", "fit"],
        Some(src),
    );
    assert_eq!(out.status.code(), Some(0), "{}", text(&out.stderr));
    assert_eq!(
        text(&out.stderr),
        "-: warning: line 1: ssl_protocols: skipped: fit picks TLS versions and ciphers, and staples OCSP, itself\n"
    );
    assert!(
        text(&out.stdout).contains("a.test"),
        "{}",
        text(&out.stdout)
    );
    let o = d.join("out.yaml");
    let out = akit_in(
        &root(),
        &[
            "adapt",
            "-from",
            "nginx",
            "-c",
            "-",
            "-to",
            "yaml",
            "-o",
            o.to_str().unwrap(),
        ],
        Some(src),
    );
    assert_eq!(out.status.code(), Some(0), "{}", text(&out.stderr));
    assert!(fs::read_to_string(&o).unwrap().contains("listeners:"));
    let _ = fs::remove_dir_all(&d);
}

#[test]
fn every_template_and_example_round_trips_through_every_format() {
    let d = dir("round-trips");
    let mut n = 0;
    for (input, to, _, golden) in cases() {
        let Some(json) = golden.filter(|_| to == "json") else {
            continue;
        };
        for (to, ext, from) in [("yaml", "yaml", "yaml"), ("fit", "conf", "fit")] {
            let out = akit(&["adapt", "-c", &input, "-to", to]);
            assert!(
                out.status.success(),
                "{input} -to {to}: {}",
                text(&out.stderr)
            );
            let f = write(&d, &format!("{n}.{ext}"), &text(&out.stdout));
            let back = akit(&["adapt", "-from", from, "-c", f.to_str().unwrap()]);
            assert!(
                back.status.success(),
                "{input} -to {to} and back: {}\n{}",
                text(&back.stderr),
                text(&out.stdout)
            );
            assert!(
                same_json(&text(&back.stdout), &json),
                "{input} -to {to} and back changed it:\n{}\n{}",
                text(&out.stdout),
                text(&back.stdout)
            );
            // Written again, it is the same text: the writers are stable.
            let again = akit(&["adapt", "-c", f.to_str().unwrap(), "-to", to]);
            assert_eq!(text(&again.stdout), text(&out.stdout), "{input} -to {to}");
            n += 1;
        }
    }
    assert!(n > 160, "only {n} round trips");
}

/// fit's TestAdapt: to YAML and back.
#[test]
fn to_yaml_and_back() {
    let out = akit(&[
        "adapt",
        "-c",
        "crates/akit/tests/testdata/adapt/fit/proxy.json",
        "-to",
        "yaml",
    ]);
    let yaml = text(&out.stdout);
    assert!(
        out.status.success() && yaml.contains("upstreams:\n  - name: app"),
        "{yaml} {}",
        text(&out.stderr)
    );
    let d = dir("to-yaml");
    let f = write(&d, "proxy.yaml", &yaml);
    let o = d.join("proxy.json");
    let back = akit(&[
        "adapt",
        "-c",
        f.to_str().unwrap(),
        "-o",
        o.to_str().unwrap(),
    ]);
    assert!(back.status.success(), "{}", text(&back.stderr));
    // akit validate reads it as it reads the JSON it came from: here, refusing the fields akit
    // does not run yet, and nothing else.
    let v = akit(&["validate", "-c", o.to_str().unwrap()]);
    let errs = text(&v.stderr);
    assert!(
        v.status.success() || errs.lines().all(|l| l.contains("not supported yet")),
        "{errs}"
    );
    let first = akit(&[
        "validate",
        "-c",
        "crates/akit/tests/testdata/adapt/fit/proxy.json",
    ]);
    assert_eq!(v.status.code(), first.status.code());
    for args in [
        &["-to", "toml"][..],
        &[
            "-from",
            "xml",
            "-c",
            "crates/akit/tests/testdata/adapt/fit/proxy.json",
        ],
    ] {
        let mut a = vec!["adapt"];
        a.extend(args);
        assert!(!akit(&a).status.success(), "{args:?}");
    }
}

/// fit's TestAdaptFit: to a `.conf` file and back.
#[test]
fn to_conf_and_back() {
    let out = akit(&[
        "adapt",
        "-c",
        "crates/akit/tests/testdata/adapt/fit/proxy.json",
        "-to",
        "fit",
    ]);
    let conf = text(&out.stdout);
    assert!(
        out.status.success()
            && conf.contains("upstream app 127.0.0.1:9000 127.0.0.1:9001 {\n\tmax_conns 256\n"),
        "{conf} {}",
        text(&out.stderr)
    );
    let d = dir("to-conf");
    let f = write(&d, "proxy.conf", &conf);
    let back = akit(&["adapt", "-c", f.to_str().unwrap()]);
    let want = akit(&[
        "adapt",
        "-c",
        "crates/akit/tests/testdata/adapt/fit/proxy.json",
    ]);
    assert!(same_json(&text(&back.stdout), &text(&want.stdout)));
}

/// fit's TestAdapt's errors, and -from and standard input.
#[test]
fn flags_and_their_errors() {
    let d = dir("flags");
    let hello = "crates/akit/tests/testdata/adapt/fit/hello.json";
    let missing_yaml = d.join("missing.yaml");
    let missing_out = d.join("missing").join("out.json");
    for (args, code, want) in [
        (vec!["-bogus"], 2, "Usage: akit adapt"),
        (
            vec!["-c", hello, "extra"],
            2,
            "unexpected argument \"extra\"",
        ),
        (
            vec!["-c", "-", "-c", hello],
            2,
            "reads standard input alone",
        ),
        (
            vec!["-from", "yaml", "-c", hello, "-c", hello],
            2,
            "-from is for a single file",
        ),
        (
            vec!["-from", "yaml", "-c", missing_yaml.to_str().unwrap()],
            1,
            "missing.yaml",
        ),
        (
            vec!["-c", hello, "-o", missing_out.to_str().unwrap()],
            1,
            "akit adapt: ",
        ),
        (
            vec!["-c", hello, "-to", "toml"],
            2,
            "-to \"toml\": json, yaml or fit",
        ),
        (
            vec!["-c", hello, "-from", "nginx"],
            1,
            "fit/hello.json: line 5: name does not end with \";\"",
        ),
        (
            vec!["-c", hello, "-from", "caddyfile"],
            1,
            "fit/hello.json: line 3: a block that does not end",
        ),
        (
            vec!["-c", hello, "-c", hello, "-from", "nginx"],
            2,
            "-from is for a single file",
        ),
        (
            vec!["-c", hello, "-from", "xml"],
            1,
            "there is no adapter \"xml\" (there are caddyfile, fit, nginx, yaml)",
        ),
    ] {
        let mut a = vec!["adapt"];
        a.extend(&args);
        let out = akit(&a);
        assert_eq!(
            out.status.code(),
            Some(code),
            "{args:?}: {}",
            text(&out.stderr)
        );
        assert!(
            text(&out.stderr).contains(want),
            "{args:?}: {}",
            text(&out.stderr)
        );
    }

    // -from names the format of a file whose extension does not.
    let yml = write(
        &d,
        "config.txt",
        "listeners:\n  - name: web\n    address: \":8080\"\nroutes:\n  - handler: {module: respond}\n",
    );
    let out = akit(&[
        "adapt",
        "-from",
        "yaml",
        "-c",
        yml.to_str().unwrap(),
        "-to",
        "fit",
    ]);
    assert!(out.status.success(), "{}", text(&out.stderr));
    assert_eq!(
        text(&out.stdout),
        "listener web :8080\n\nroute { handler respond }\n"
    );

    // -c - reads standard input, JSON when -from does not say.
    let json = r#"{"listeners": [{"name": "web", "address": ":8080"}], "routes": [{"handler": {"module": "respond"}}]}"#;
    let out = akit_in(&root(), &["adapt", "-c", "-"], Some(json));
    assert!(out.status.success() && text(&out.stdout).contains("\"listeners\""));
    let out = akit_in(
        &root(),
        &["adapt", "-c", "-", "-from", "yaml"],
        Some("listeners: [\n"),
    );
    assert_eq!(out.status.code(), Some(1), "{}", text(&out.stderr));
    assert!(text(&out.stderr).starts_with("-:"), "{}", text(&out.stderr));
}

/// fit's TestSeveralFiles: a directory of files in several formats, merged.
#[test]
fn several_files_are_merged() {
    let d = dir("several");
    write(&d, "base.conf", "listener web {\n\taddress :8080\n}\n");
    write(
        &d,
        "sites/a.conf",
        "site a.example {\n\troute { handler respond { body a } }\n}\n",
    );
    write(
        &d,
        "sites/b.yaml",
        "routes:\n  - handler: {module: respond, params: {status: 204}}\n",
    );
    let out = akit(&["adapt", "-c", d.to_str().unwrap()]);
    let s = text(&out.stdout);
    assert!(
        out.status.success()
            && s.matches("\"module\": \"respond\"").count() == 2
            && s.contains("\"a.example\""),
        "{s} {}",
        text(&out.stderr)
    );
    let base = d.join("base.conf");
    let sites = d.join("sites");
    let out = akit(&[
        "adapt",
        "-c",
        base.to_str().unwrap(),
        "-c",
        sites.to_str().unwrap(),
        "-from",
        "fit",
    ]);
    assert_eq!(out.status.code(), Some(2));
    // A problem in the config fails the conversion, at its place.
    write(
        &d,
        "sites/b.yaml",
        "routes:\n  - handler: {module: respond, params: {status: 99}}\n",
    );
    let out = akit(&["adapt", "-c", d.to_str().unwrap()]);
    let want = format!(
        "{}:2:49: routes[0].handler.params.status",
        sites.join("b.yaml").display()
    );
    assert!(
        out.status.code() == Some(1) && text(&out.stderr).contains(&want),
        "{}",
        text(&out.stderr)
    );
}

/// fit's TestAdaptToFitRefused, with a field of no known type: a list of one value, which a
/// `.conf` file would read back as the value alone.
#[test]
fn a_config_a_conf_file_cannot_hold_is_refused() {
    let d = dir("refused");
    let f = write(
        &d,
        "fit.json",
        r#"{"listeners": [{"name": "web", "address": ":8080"}],
  "routes": [{"handler": {"module": "respond"}}], "x-extra": [1]}"#,
    );
    let out = akit(&["adapt", "-c", f.to_str().unwrap(), "-to", "fit"]);
    // An unknown field fails the check first, as in fit.
    assert_eq!(out.status.code(), Some(1));
    assert!(out.stdout.is_empty());
    let err = akit_config::adapt::conf_from_json(
        r#"{"routes": [{"handler": {"module": "elsewhere", "params": {"x": [1]}}}]}"#,
    )
    .unwrap_err();
    assert!(err.contains("fewer than two values"), "{err}");
}

/// A field akit does not run yet is converted as written, with a warning.
#[test]
fn fields_akit_does_not_run_yet_are_converted_with_a_warning() {
    let out = akit(&[
        "adapt",
        "-c",
        "tests/compat/fixtures/fit-examples/bittorrent/fit.conf",
        "-to",
        "yaml",
    ]);
    assert!(out.status.success(), "{}", text(&out.stderr));
    let err = text(&out.stderr);
    assert!(
        err.contains("akit adapt: warning: ") && err.contains("not supported yet"),
        "{err}"
    );
    assert!(text(&out.stdout).contains("bittorrent:"));
}

#[test]
fn fmt_writes_each_file_in_its_own_format() {
    let d = dir("fmt");
    let conf = write(
        &d,
        "a.conf",
        "listener web {\n  address :8080\n  protocols http/1.1 h2c\n}\n",
    );
    let yaml = write(
        &d,
        "b.yaml",
        "routes:\n- listeners: [web]\n  handler: {module: respond, params: {body: \"hi\"}}\n",
    );
    let json = write(
        &d,
        "c.json",
        "{\"admin\": {\"address\": \"127.0.0.1:2019\"}}",
    );
    let (conf_s, yaml_s, json_s) = (
        conf.to_str().unwrap(),
        yaml.to_str().unwrap(),
        json.to_str().unwrap(),
    );
    let want_conf = "listener web :8080 http/1.1 h2c\n";
    let want_yaml = "routes:\n  - listeners:\n      - web\n    handler:\n      module: respond\n      params:\n        body: hi\n";
    let want_json = "{\n  \"admin\": {\n    \"address\": \"127.0.0.1:2019\"\n  }\n}\n";

    // To standard output, file by file, the files checked together.
    let out = akit(&["fmt", "-c", d.to_str().unwrap()]);
    assert!(out.status.success(), "{}", text(&out.stderr));
    assert_eq!(
        text(&out.stdout),
        format!("{want_conf}{want_yaml}{want_json}")
    );

    // -l lists the files whose formatting differs.
    let out = akit(&["fmt", "-l", "-c", d.to_str().unwrap()]);
    assert_eq!(text(&out.stdout), format!("{conf_s}\n{yaml_s}\n{json_s}\n"));

    // -w writes them in place; then there is nothing left to do.
    let out = akit(&["fmt", "-w", "-c", d.to_str().unwrap()]);
    assert!(out.status.success(), "{}", text(&out.stderr));
    assert_eq!(fs::read_to_string(&conf).unwrap(), want_conf);
    assert_eq!(fs::read_to_string(&yaml).unwrap(), want_yaml);
    assert_eq!(fs::read_to_string(&json).unwrap(), want_json);
    let out = akit(&["fmt", "-l", "-c", d.to_str().unwrap()]);
    assert!(out.status.success() && out.stdout.is_empty());

    // A file with comments is left as it is with -w.
    let commented =
        "# the listener\nlistener web {\n  address :8080\n  protocols http/1.1 h2c\n}\n";
    fs::write(&conf, commented).unwrap();
    let out = akit(&["fmt", "-w", "-c", d.to_str().unwrap()]);
    assert_eq!(out.status.code(), Some(1));
    assert!(
        text(&out.stderr).contains("holds comments"),
        "{}",
        text(&out.stderr)
    );
    assert_eq!(fs::read_to_string(&conf).unwrap(), commented);

    // A broken config is not written.
    fs::write(
        &yaml,
        "routes:\n- listeners: [nowhere]\n  handler: {module: respond}\n",
    )
    .unwrap();
    let out = akit(&["fmt", "-c", d.to_str().unwrap()]);
    assert_eq!(out.status.code(), Some(1));
    assert!(out.stdout.is_empty());
    let out = akit(&["fmt", "-bogus"]);
    assert_eq!(out.status.code(), Some(2));
    assert!(text(&out.stderr).contains("Usage: akit fmt"));
}
