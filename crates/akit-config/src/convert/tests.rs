#![allow(
    clippy::unwrap_used,
    clippy::panic,
    clippy::indexing_slicing,
    clippy::arithmetic_side_effects
)]

use std::path::Path;

/// The environment fit's golden files were written with.
fn env(name: &str) -> Option<String> {
    (name == "FIT_TEST_PORT").then(|| "9000".to_owned())
}

/// A hex-encoded text; `None` when it is not UTF-8, which akit refuses before converting.
fn unhex_utf8(s: &str) -> Option<String> {
    let b: Vec<u8> = (0..s.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&s[i..i + 2], 16).unwrap())
        .collect();
    String::from_utf8(b).ok()
}

fn unhex(s: &str) -> String {
    unhex_utf8(s).unwrap()
}

/// Each converter on fit's corpus gives what fit's gives, byte for byte: the JSON and the
/// warnings, or the error.
fn golden(name: &str, convert: &dyn Fn(&str) -> Result<super::Converted, String>) {
    let file = Path::new(env!("CARGO_MANIFEST_DIR")).join(format!("testdata/convert/{name}.txt"));
    let text = std::fs::read_to_string(file).unwrap();
    let mut wrong = Vec::new();
    let mut n = 0;
    for line in text.lines() {
        let c: Vec<&str> = line.split('\t').collect();
        let Some(input) = unhex_utf8(c[0]) else {
            continue;
        };
        n += 1;
        let got = match convert(&input) {
            Ok(r) => format!("OK\n{}\n{}", r.json(), r.warnings().join("\n")),
            Err(e) => format!("ERR\n{e}"),
        };
        let want = if c[1] == "OK" {
            format!("OK\n{}\n{}", unhex(c[2]), unhex(c[3]))
        } else {
            format!("ERR\n{}", unhex(c[2]))
        };
        if got != want {
            wrong.push(format!(
                "--- input\n{input}\n--- akit\n{got}\n--- fit\n{want}\n"
            ));
        }
    }
    assert!(n > 100, "only {n} cases");
    assert!(
        wrong.is_empty(),
        "{} of {n} cases differ from fit:\n{}",
        wrong.len(),
        wrong.iter().take(4).cloned().collect::<String>()
    );
}

#[test]
fn nginx_converts_as_fit() {
    golden("nginx", &|s| super::nginx(s));
}

#[test]
fn caddyfiles_convert_as_fit() {
    golden("caddyfile", &|s| super::caddyfile(s, &env));
}

/// Where akit's config checks knowingly differ from fit's: a converted config fit loads that akit
/// refuses (akit checks a bcrypt hash's shape, and refuses header values with line breaks), and
/// one fit refuses that akit loads (fit refuses an Admin API address a listener takes; akit does
/// not check it yet).
const KNOWN: &[&str] = &[
    "is not a bcrypt hash",
    "at least one user is needed",
    "holds a line break or NUL",
    "listens on TCP",
];

fn known(msg: &str) -> bool {
    msg.lines().all(|l| KNOWN.iter().any(|k| l.contains(k)))
}

/// Each converted config akit's config loader takes as fit's takes it: one fit loads, akit
/// loads, or refuses only for fields it does not support yet; one fit refuses, akit refuses.
fn loads_as_fit(name: &str, convert: &dyn Fn(&str) -> Result<super::Converted, String>) {
    let file = Path::new(env!("CARGO_MANIFEST_DIR")).join(format!("testdata/convert/{name}.txt"));
    let text = std::fs::read_to_string(file).unwrap();
    let (mut loads, mut refused, mut unsupported) = (0, 0, 0);
    let mut wrong = Vec::new();
    for line in text.lines() {
        let c: Vec<&str> = line.split('\t').collect();
        let Some(input) = unhex_utf8(c[0]) else {
            continue;
        };
        if c[1] != "OK" {
            continue;
        }
        let r = convert(&input).unwrap();
        let fit = unhex(c[4]);
        let akit = crate::from_json(r.json(), None);
        let same = match &akit {
            Ok(_) => fit.is_empty() || known(&fit),
            Err(e) if e.only_not_supported_yet() => {
                unsupported += 1;
                fit.is_empty()
            }
            Err(e) => !fit.is_empty() || known(&e.to_string()),
        };
        if fit.is_empty() {
            loads += 1;
        } else {
            refused += 1;
        }
        if !same {
            wrong.push(format!(
                "--- {}\n--- akit: {:?}\n--- fit: {fit}\n",
                r.json(),
                akit.err().map(|e| e.to_string())
            ));
        }
    }
    assert!(loads > 100, "only {loads} configs fit loads");
    assert!(
        wrong.is_empty(),
        "{} of {} differ from fit ({unsupported} not supported yet):\n{}",
        wrong.len(),
        loads + refused,
        wrong.iter().take(6).cloned().collect::<String>()
    );
}

#[test]
fn nginx_output_loads_as_in_fit() {
    loads_as_fit("nginx", &|s| super::nginx(s));
}

#[test]
fn caddyfile_output_loads_as_in_fit() {
    loads_as_fit("caddyfile", &|s| super::caddyfile(s, &env));
}

/// fit's nginx `TestPosition`: routes, upstreams and listeners are placed at their lines, and
/// nothing else is.
#[test]
fn nginx_positions() {
    let r = super::nginx(
        "http {\n upstream u { server x; }\n server {\n  listen 80;\n  location / { proxy_pass http://u; }\n }\n}\n",
    )
    .unwrap();
    for (path, want) in [("listeners[0]", 4), ("upstreams[0]", 2), ("routes[0]", 5)] {
        assert_eq!(r.line(path), Some(want), "{path}");
    }
    assert_eq!(r.line("routes[1]"), None);
    // An error about a route's part is placed at the route's line.
    let errs = crate::Errors(vec![crate::Error {
        file: None,
        path: "routes[0].handler.params.upstream".into(),
        line: 1,
        col: 200,
        msg: "x".into(),
    }]);
    let mapped = r.map_errors(errs);
    assert_eq!((mapped.0[0].line, mapped.0[0].col), (5, 1));
}

/// fit's nginx `TestParse` and `TestParseMore`.
#[test]
fn nginx_parse() {
    use super::nginx_parse::{MAX_DIRECTIVES, parse};
    let ds = parse(
        "# a comment\na \"quoted; with # and \\\"escapes\\\"\" 'single' plain;\nb ${var}x {  c 1; d { e; } }\nf \"line\ntwo\";\n",
    )
    .unwrap();
    assert_eq!(ds.len(), 3);
    assert_eq!(ds[0].args[0], r#"quoted; with # and "escapes""#);
    assert_eq!(ds[0].args[1], "single");
    assert_eq!(ds[0].line, 2);
    assert_eq!(ds[1].args[0], "${var}x");
    assert_eq!(ds[1].block.len(), 2);
    assert_eq!(ds[1].block[1].block[0].name, "e");
    assert_eq!(ds[2].line, 4);
    assert_eq!(ds[2].args[0], "line\ntwo");
    for (src, want) in [
        ("a b".to_owned(), "does not end"),
        ("a { b; ".into(), "does not end"),
        ("a; }".into(), "unexpected"),
        ("a \"open;".into(), "does not end"),
        ("; a;".into(), "no directive"),
        ("a { b }".into(), "does not end with"),
        ("http { a ".into(), "does not end"),
        (
            format!("{}{}", "a {".repeat(100), "}".repeat(100)),
            "nested too deep",
        ),
        ("a;".repeat(MAX_DIRECTIVES + 1), "directives"),
    ] {
        let err = parse(&src).err().unwrap_or_default();
        assert!(err.contains(want), "{err}, want {want}");
    }
    let ds = parse("a \"x\\ny\\tz\\q\";").unwrap();
    assert_eq!(ds[0].args[0], "x\ny\tz\\q");
}

/// fit's nginx `TestConversions` "a long route name", whose input is made.
#[test]
fn nginx_long_route_name() {
    let src = format!(
        "http {{\n server {{\nlisten 80; server_name {}.test; location /x {{ return 204; }}\n }}\n}}\n",
        "a".repeat(70)
    );
    let r = super::nginx(&src).unwrap();
    let v = crate::value::parse(r.json()).unwrap();
    let Some(crate::value::Value::Array(routes)) = v.get("routes") else {
        panic!("no routes")
    };
    let Some(crate::value::Value::String(name)) = routes[0].get("name") else {
        panic!("no name")
    };
    assert_eq!(name.len(), 60);
}

/// fit's `FuzzConvert` property on the corpus: every warning names its line.
#[test]
fn warnings_name_their_lines() {
    for name in ["nginx", "caddyfile"] {
        let file =
            Path::new(env!("CARGO_MANIFEST_DIR")).join(format!("testdata/convert/{name}.txt"));
        for line in std::fs::read_to_string(file).unwrap().lines() {
            let c: Vec<&str> = line.split('\t').collect();
            let Some(input) = unhex_utf8(c[0]) else {
                continue;
            };
            let r = if name == "nginx" {
                super::nginx(&input)
            } else {
                super::caddyfile(&input, &env)
            };
            if let Ok(r) = r {
                assert!(crate::value::parse(r.json()).is_ok());
                for w in r.warnings() {
                    assert!(w.starts_with("line "), "{w}");
                }
            }
        }
    }
}

/// No config, however broken, makes a converter panic (fit's `FuzzConvert`): every input of the
/// corpus cut short at every fifth character, and with each brace and quote taken out in turn.
#[test]
fn broken_configs_do_not_panic() {
    let mut n = 0usize;
    for name in ["nginx", "caddyfile"] {
        let file =
            Path::new(env!("CARGO_MANIFEST_DIR")).join(format!("testdata/convert/{name}.txt"));
        for line in std::fs::read_to_string(file).unwrap().lines() {
            let Some(input) = unhex_utf8(line.split('\t').next().unwrap()) else {
                continue;
            };
            let mut variants: Vec<String> = input
                .char_indices()
                .step_by(5)
                .map(|(i, _)| input[..i].to_owned())
                .collect();
            for (i, c) in input.char_indices() {
                if matches!(c, '{' | '}' | '"' | '\'' | '`' | ';') && variants.len() < 400 {
                    variants.push(format!("{}{}", &input[..i], &input[i + 1..]));
                }
            }
            for v in variants {
                n += 1;
                let r = if name == "nginx" {
                    super::nginx(&v)
                } else {
                    super::caddyfile(&v, &env)
                };
                if let Ok(r) = r {
                    assert!(crate::value::parse(r.json()).is_ok(), "{v}");
                }
            }
        }
    }
    assert!(n > 10_000, "only {n} inputs");
}

/// The paths, by index, of every directive of `ds` that has a block, each before its children's.
fn blocks_of(ds: &[super::nginx_parse::Directive], at: &mut Vec<usize>, out: &mut Vec<Vec<usize>>) {
    for (i, d) in ds.iter().enumerate() {
        at.push(i);
        if !d.block.is_empty() {
            out.push(at.clone());
            blocks_of(&d.block, at, out);
        }
        at.pop();
    }
}

/// M5's exit criterion (I76): the nginx converter handles fit's sample configs and lists every
/// directive it skips. Each sample (fit's `adapter/nginx/testdata` and the configs of fit's docs,
/// guides and benchmark that the CLI's tests convert) converts, every warning names its line, and
/// a directive no converter knows, put first in every block and at the top, is reported at its own
/// line, or within a block reported skipped as a whole.
#[test]
fn nginx_samples_list_every_skipped_directive() {
    use super::nginx::convert_parsed;
    use super::nginx_parse::{Directive, parse};
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("../akit/tests/testdata/adapt/convert");
    let probe = |line| Directive {
        name: "akit_probe".into(),
        args: vec!["on".into()],
        line,
        block: Vec::new(),
    };
    let mut probed_blocks = 0;
    for name in [
        "full.conf",
        "proxy.conf",
        "static.conf",
        "bench-nginx.conf",
        "nginx-docs.conf",
        "nginx-shop.conf",
    ] {
        let mut src = std::fs::read_to_string(dir.join(name)).unwrap();
        // The benchmark's config as fitbench fills it in.
        for (k, v) in [
            ("{{WORKERS}}", "7"),
            ("{{BACKLOG}}", "2048"),
            ("{{WORKER_CONNECTIONS}}", "65536"),
            ("{{BACKEND_CONNS}}", "16384"),
            ("{{BACKEND_IDLE_PER_WORKER}}", "4096"),
            ("{{BACKEND}}", "127.0.0.1:9000"),
            ("{{WSBACKEND}}", "127.0.0.1:9001"),
            ("{{CACHEBACKEND}}", "127.0.0.1:9003"),
        ] {
            src = src.replace(k, v);
        }
        let ds = parse(&src).unwrap_or_else(|e| panic!("{name}: {e}"));
        let base = convert_parsed(&ds).unwrap_or_else(|e| panic!("{name}: {e}"));
        for w in base.warnings() {
            let n = w
                .strip_prefix("line ")
                .and_then(|r| r.split_once(':'))
                .map(|(n, _)| n);
            assert!(
                n.is_some_and(|n| n.parse::<u32>().is_ok_and(|n| n > 0)),
                "{name}: a warning without its line: {w}"
            );
        }
        let mut paths = vec![Vec::new()];
        blocks_of(&ds, &mut Vec::new(), &mut paths);
        for path in paths {
            let line = 1_000_000;
            let mut probed = ds.clone();
            let mut ancestors = Vec::new();
            let mut block = &mut probed;
            for &i in &path {
                ancestors.push(block[i].line);
                block = &mut block[i].block;
            }
            block.insert(0, probe(line));
            let got = convert_parsed(&probed)
                .unwrap_or_else(|e| panic!("{name}: a probe in the block at {ancestors:?}: {e}"));
            let at = |l: i64| format!("line {l}: ");
            let listed = got
                .warnings()
                .iter()
                .any(|w| w.starts_with(&at(line)) && w.contains("akit_probe"))
                || got.warnings().iter().any(|w| {
                    ancestors.iter().any(|&a| w.starts_with(&at(a)))
                        && (w.contains("skipped") || w.contains("not converted"))
                });
            assert!(
                listed,
                "{name}: a directive in the block at lines {ancestors:?} is not listed: {:?}",
                got.warnings()
            );
            assert_eq!(
                got.json(),
                base.json(),
                "{name}: a probe at {ancestors:?} changed the output"
            );
            probed_blocks += 1;
        }
    }
    assert!(probed_blocks > 50, "{probed_blocks} blocks probed");
}
