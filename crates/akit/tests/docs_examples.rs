//! Every config example in akit's docs is a config akit takes (I78): each fenced block of a
//! config, in a Markdown file of the repository, is written to a file and run through
//! `akit validate`, as a reader would run it; nginx configs and Caddyfiles (the migration guides')
//! are converted with `akit adapt -from nginx` or `-from caddyfile` first, and what that writes must
//! validate.
//!
//! A block is a config when its info string's first word is `fit` (fit's `.conf` format), `yaml`,
//! `json`, `nginx` or `caddyfile`; a block that is not a config (an Admin API answer, a Kubernetes
//! manifest) says so with `not-config` after its language, as in ```` ```json not-config ````.
//!
//! The examples name files relative to the directory akit runs in; the test runs them in a
//! directory that holds them ([`sandbox`]): `public/`, `certs/example.com.pem`,
//! `certs/example.com.key`, `certs/ca.pem`, `admin.token`, and `plugins/headers.wasm` and
//! `plugins/http_headers.wasm` (a plugin of fit's ABI and a Proxy-Wasm one).
//!
//! A document and its Chinese twin show the same examples, in the same order: that is checked
//! too, so that an example changed in one language is changed in the other.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing,
    clippy::arithmetic_side_effects
)]

mod support;

use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};
use std::time::{Duration, Instant};

/// The languages of the blocks that are configs, and the extension each is validated with.
const CONFIGS: &[(&str, &str)] = &[
    ("fit", "conf"),
    ("yaml", "yaml"),
    ("json", "json"),
    ("nginx", "nginx"),
    ("caddyfile", "caddyfile"),
];

/// How long one `akit validate` or `akit adapt` may take before the test fails (rules.md 31).
const COMMAND_TIMEOUT: Duration = Duration::from_secs(60);

/// A config example: where it is, its language and its text.
struct Example {
    file: PathBuf,
    line: usize,
    lang: &'static str,
    text: String,
}

fn repo() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..")
}

/// The Markdown files of the repository that are akit's documents: not build output, and not the
/// copies of other projects' files or the reports benchmark runs write.
fn documents() -> Vec<PathBuf> {
    fn walk(dir: &Path, out: &mut Vec<PathBuf>) {
        let mut entries: Vec<_> = fs::read_dir(dir)
            .unwrap()
            .map(|e| e.unwrap().path())
            .collect();
        entries.sort();
        for p in entries {
            let name = p.file_name().unwrap().to_string_lossy().into_owned();
            if p.is_dir() {
                if !matches!(
                    name.as_str(),
                    "target" | ".git" | ".claude" | "fixtures" | "results" | "node_modules"
                ) {
                    walk(&p, out);
                }
            } else if name.ends_with(".md") {
                out.push(p);
            }
        }
    }
    let mut out = Vec::new();
    walk(&repo(), &mut out);
    out
}

/// The config examples of one document, in order.
fn examples(file: &Path) -> Vec<Example> {
    let text = fs::read_to_string(file).unwrap();
    let mut out = Vec::new();
    let mut lines = text.lines().enumerate();
    while let Some((n, line)) = lines.next() {
        let Some(info) = line.strip_prefix("```") else {
            continue;
        };
        let mut words = info.split_whitespace();
        let lang = words.next().unwrap_or("");
        let not_config = words.any(|w| w == "not-config");
        let mut body = String::new();
        let mut closed = false;
        for (_, l) in lines.by_ref() {
            if l.starts_with("```") {
                closed = true;
                break;
            }
            body.push_str(l);
            body.push('\n');
        }
        assert!(
            closed,
            "{}:{}: a block that is not closed",
            file.display(),
            n + 1
        );
        if not_config {
            continue;
        }
        if let Some((lang, _)) = CONFIGS.iter().find(|(l, _)| *l == lang) {
            out.push(Example {
                file: file.to_owned(),
                line: n + 1,
                lang,
                text: body,
            });
        }
    }
    out
}

/// The directory the examples run in, with the files they name.
fn sandbox() -> PathBuf {
    let d = support::certs::dir("docs-examples");
    fs::create_dir_all(d.join("public")).unwrap();
    fs::write(d.join("public/index.html"), "hello from akit\n").unwrap();
    fs::create_dir_all(d.join("certs")).unwrap();
    let ca = support::certs::Ca::new("akit docs CA");
    ca.write(&d.join("certs"), "ca.pem");
    ca.leaf(
        &d.join("certs"),
        "example.com",
        &["example.com", "www.example.com"],
    );
    fs::write(d.join("admin.token"), "an-admin-token\n").unwrap();
    // fit's `headers` plugin (fit's ABI), and the Proxy-Wasm Go SDK's `http_headers`.
    fs::create_dir_all(d.join("plugins")).unwrap();
    for (guest, name) in [
        ("headers-go.wasm.gz", "headers.wasm"),
        ("pw-http_headers.wasm.gz", "http_headers.wasm"),
    ] {
        let gz = fs::read(
            PathBuf::from(env!("CARGO_MANIFEST_DIR"))
                .join("../akit-plugin/tests/guests")
                .join(guest),
        )
        .unwrap();
        let wasm = akio_deflate::decompress(akio_deflate::Format::Gzip, &gz, 64 << 20).unwrap();
        fs::write(d.join("plugins").join(name), wasm).unwrap();
    }
    d
}

/// Runs akit with `args` in `dir`, failing the test if it takes longer than [`COMMAND_TIMEOUT`].
fn akit(dir: &Path, args: &[&str]) -> Output {
    let mut child = Command::new(env!("CARGO_BIN_EXE_akit"))
        .args(args)
        .current_dir(dir)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let start = Instant::now();
    while child.try_wait().unwrap().is_none() {
        if start.elapsed() > COMMAND_TIMEOUT {
            let _ = child.kill();
            panic!("akit {} took more than {COMMAND_TIMEOUT:?}", args.join(" "));
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    child.wait_with_output().unwrap()
}

/// Why `ex` is not a config akit takes, or `None` when it is.
fn check(dir: &Path, ex: &Example) -> Option<String> {
    let ext = CONFIGS.iter().find(|(l, _)| *l == ex.lang).unwrap().1;
    let path = format!("example.{ext}");
    fs::write(dir.join(&path), &ex.text).unwrap();
    let config = if matches!(ex.lang, "nginx" | "caddyfile") {
        let out = akit(
            dir,
            &["adapt", "-from", ex.lang, "-c", &path, "-o", "adapted.json"],
        );
        if !out.status.success() {
            return Some(format!(
                "akit adapt -from {}: {}",
                ex.lang,
                String::from_utf8_lossy(&out.stderr)
            ));
        }
        "adapted.json".to_owned()
    } else {
        path
    };
    let out = akit(dir, &["validate", "-c", &config]);
    (!out.status.success()).then(|| {
        format!(
            "akit validate: {}{}",
            String::from_utf8_lossy(&out.stdout),
            String::from_utf8_lossy(&out.stderr)
        )
    })
}

#[test]
fn every_config_example_in_the_docs_is_valid() {
    let dir = sandbox();
    let mut failed = Vec::new();
    let mut n = 0;
    for doc in documents() {
        for ex in examples(&doc) {
            n += 1;
            if let Some(why) = check(&dir, &ex) {
                let at = ex.file.strip_prefix(repo()).unwrap_or(&ex.file);
                failed.push(format!("{}:{}: {}", at.display(), ex.line, why.trim_end()));
            }
        }
    }
    let _ = fs::remove_dir_all(&dir);
    // The reference alone has dozens: finding few means the blocks are no longer recognized.
    assert!(n >= 60, "only {n} config examples found in the docs");
    assert!(
        failed.is_empty(),
        "{} of {n} examples fail:\n{}",
        failed.len(),
        failed.join("\n")
    );
}

#[test]
fn twins_show_the_same_examples() {
    for doc in documents() {
        let name = doc.to_string_lossy().into_owned();
        let Some(base) = name.strip_suffix(".md") else {
            continue;
        };
        if base.ends_with(".zh-CN") {
            continue;
        }
        let twin = PathBuf::from(format!("{base}.zh-CN.md"));
        if !twin.exists() {
            continue;
        }
        let a: Vec<(&str, String)> = examples(&doc)
            .into_iter()
            .map(|e| (e.lang, e.text))
            .collect();
        let b: Vec<(&str, String)> = examples(&twin)
            .into_iter()
            .map(|e| (e.lang, e.text))
            .collect();
        assert!(
            a == b,
            "{} and its Chinese twin show different config examples ({} and {})",
            doc.display(),
            a.len(),
            b.len()
        );
    }
}

#[test]
fn examples_are_found() {
    let file = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("docs-examples-probe.md");
    fs::write(
        &file,
        "# t\n\n```fit\nlistener web :8080\n```\n\n```json not-config\n{}\n```\n\n```bash\nakit run\n```\n\n```yaml\na: 1\n```\n",
    )
    .unwrap();
    let found = examples(&file);
    let _ = fs::remove_file(&file);
    let langs: Vec<&str> = found.iter().map(|e| e.lang).collect();
    assert_eq!(langs, ["fit", "yaml"]);
    assert_eq!(found[0].line, 3);
    assert_eq!(found[0].text, "listener web :8080\n");
}
