//! Building akit with the modules of other crates besides its own, as `akit build` does (I70) and
//! fit's `plugin/build` does for `fit build`, the way xcaddy builds Caddy.
//!
//! A build generates a Cargo package of its own: a `main.rs` that calls [`crate::main`] with the
//! `MODULES` of akit's own module crates ([`STANDARD`], less those left out) and of the crates
//! asked for, and a `Cargo.toml` that depends on akit (a directory of its source, or a revision of
//! its git repository) and on those crates (from crates.io, a directory or a git repository). It
//! builds the package with the `cargo` found on `PATH` (or named by `CARGO`, as cargo names
//! itself to what it runs) and copies the binary to the output. Rust has no `init`: a module
//! crate exports `pub static MODULES: &[Info<HandlerParams>]` at its root (decision 003), and the
//! generated `main` lists it. `docs/modules.md` says how to write one.
//!
//! A build from a directory of akit builds with that checkout's `Cargo.lock`, and replaces akit's
//! git repository with the directory (`[patch]`) for a module crate that depends on akit by git,
//! as `fit build` replaces fit's module with a directory: a module and the akit it is built into
//! always share one `akit-runtime`. Cargo's own settings apply: `CARGO_TARGET_DIR` to share
//! compiled crates between builds, `CARGO_BUILD_TARGET` to build for another system.
//!
//! This runs on the command line, never on an akio thread: it waits for cargo.

#![warn(clippy::unwrap_used)]

use std::fmt;
use std::fs;
use std::io::{self, BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{SystemTime, UNIX_EPOCH};

/// akit's git repository: what a revision of akit is fetched from, and what a build from a
/// directory replaces.
pub const AKIT_GIT: &str = "https://github.com/lesismal/akit";

/// akit's own module crates, which a build registers unless told to leave them out: every handler
/// and middleware akit ships with. The akit crate re-exports each.
pub const STANDARD: &[&str] = &[
    "akit-handler",
    "akit-middleware",
    "akit-cache",
    "akit-proxy",
];

/// The name of the package a build generates, and of its binary.
const PACKAGE: &str = "akit-build";

/// Where a crate comes from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Source {
    /// crates.io, at a version requirement as `Cargo.toml` writes one (`1.2`, `=1.2.3`); empty
    /// or `latest` for the newest.
    Registry(String),
    /// A directory holding its source.
    Dir(PathBuf),
    /// A git repository, at a reference: a commit (hexadecimal), a tag (`v` and a digit) or a
    /// branch; its default branch when `None`.
    Git {
        /// The repository's URL.
        url: String,
        /// The commit, tag or branch.
        reference: Option<String>,
    },
}

/// A crate to build in: its package name, whose library's root exports `MODULES`, and where it
/// comes from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Module {
    /// The package's name, as `Cargo.toml` names it; the library is the name with `_` for `-`.
    pub name: String,
    /// Where it comes from.
    pub source: Source,
}

/// Why a build failed, or what it was given that it cannot build.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Error(String);

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for Error {}

fn err(msg: impl Into<String>) -> Error {
    Error(msg.into())
}

/// Whether `name` is a package name cargo takes: ASCII letters, digits, `-` and `_`, not starting
/// with a digit or `-`.
fn is_package_name(name: &str) -> bool {
    let mut chars = name.chars();
    chars
        .next()
        .is_some_and(|c| c.is_ascii_alphabetic() || c == '_')
        && chars.all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
}

/// Parses a crate as `akit build`'s `-with` takes it: `name`, `name@version` (crates.io),
/// `name=dir`, for one whose source is in `dir`, or `name=git+url` with `#reference` or not,
/// for one in a git repository.
pub fn parse_module(s: &str) -> Result<Module, Error> {
    let (head, from) = match s.split_once('=') {
        Some((h, f)) => (h, Some(f)),
        None => (s, None),
    };
    let (name, version) = match head.split_once('@') {
        Some((n, v)) => (n, Some(v)),
        None => (head, None),
    };
    if name.is_empty() {
        return Err(err(format!("-with {s:?}: no crate name")));
    }
    if !is_package_name(name) {
        return Err(err(format!("-with {s:?}: {name:?} is not a crate name")));
    }
    if name == "akit" {
        return Err(err(
            "-with akit: akit is what is built on; name it with -akit",
        ));
    }
    let source = match (from, version) {
        (Some(_), Some(_)) => {
            return Err(err(format!("-with {name}: a version and a directory both")));
        }
        (Some(""), None) => return Err(err(format!("-with {name}=: no directory"))),
        (Some(f), None) => match f.strip_prefix("git+") {
            Some(git) => git_source(name, git)?,
            None => Source::Dir(PathBuf::from(f)),
        },
        (None, v) => Source::Registry(v.unwrap_or_default().to_owned()),
    };
    Ok(Module {
        name: name.to_owned(),
        source,
    })
}

/// `url` and `#reference` or not, as a git source.
fn git_source(name: &str, git: &str) -> Result<Source, Error> {
    let (url, reference) = match git.split_once('#') {
        Some((u, r)) => (u, Some(r)),
        None => (git, None),
    };
    if url.is_empty() || reference == Some("") {
        return Err(err(format!(
            "-with {name}=git+{git}: no repository, or an empty reference"
        )));
    }
    Ok(Source::Git {
        url: url.to_owned(),
        reference: reference.map(str::to_owned),
    })
}

/// Parses akit as `-akit` names it: a revision of akit's git repository (a commit, a tag `v…`),
/// `git+url#reference` for another repository of it, or a directory holding its source (anything
/// with a path separator, or not a revision).
#[must_use]
pub fn parse_akit(v: &str) -> Source {
    if let Some(git) = v.strip_prefix("git+") {
        let (url, reference) = match git.split_once('#') {
            Some((u, r)) => (u, Some(r.to_owned())),
            None => (git, None),
        };
        return Source::Git {
            url: url.to_owned(),
            reference,
        };
    }
    if !v.contains(['/', '\\']) && (is_commit(v) || is_tag(v)) {
        return Source::Git {
            url: AKIT_GIT.to_owned(),
            reference: Some(v.to_owned()),
        };
    }
    Source::Dir(PathBuf::from(v))
}

/// Whether `r` is a commit: 7 to 40 hexadecimal digits.
fn is_commit(r: &str) -> bool {
    (7..=40).contains(&r.len()) && r.chars().all(|c| c.is_ascii_hexdigit())
}

/// Whether `r` is a version tag: `v` and a digit.
fn is_tag(r: &str) -> bool {
    r.strip_prefix('v')
        .is_some_and(|rest| rest.starts_with(|c: char| c.is_ascii_digit()))
}

/// What a build makes.
#[derive(Debug, Clone)]
pub struct Options {
    /// The binary to write.
    pub output: PathBuf,
    /// The akit to build on: a directory of its source or a revision of its repository; a build
    /// needs one.
    pub akit: Option<Source>,
    /// The crates to build in besides akit's own.
    pub with: Vec<Module>,
    /// Crates of [`STANDARD`] to leave out.
    pub without: Vec<String>,
    /// What the binary's `akit version` says; akit's own version when empty.
    pub version: String,
    /// When set, the directory the package is generated in, and kept; otherwise a temporary one
    /// is, and removed.
    pub keep: Option<PathBuf>,
    /// The cargo to run: `CARGO` when set (cargo sets it for what it runs), else `cargo` on
    /// `PATH`.
    pub cargo: PathBuf,
    /// A release build (cargo's `--release`, with akit's release profile); a debug one when
    /// false, which is quicker to build.
    pub release: bool,
}

impl Options {
    /// A release build of akit `akit` into `output`, with akit's own modules alone.
    #[must_use]
    pub fn new(output: impl Into<PathBuf>, akit: Source) -> Options {
        Options {
            output: output.into(),
            akit: Some(akit),
            with: Vec::new(),
            without: Vec::new(),
            version: String::new(),
            keep: None,
            cargo: std::env::var_os("CARGO").map_or_else(|| PathBuf::from("cargo"), PathBuf::from),
            release: true,
        }
    }
}

/// The crates whose `MODULES` the generated `main` registers: [`STANDARD`] less `without`, then
/// the crates of `with`, each once.
pub fn crates(with: &[Module], without: &[String]) -> Result<Vec<String>, Error> {
    for w in without {
        if !STANDARD.contains(&w.as_str()) {
            return Err(err(format!(
                "-without {w}: it is none of {}",
                STANDARD.join(", ")
            )));
        }
    }
    let mut out: Vec<String> = STANDARD
        .iter()
        .filter(|s| !without.iter().any(|w| w == *s))
        .map(|s| (*s).to_owned())
        .collect();
    for m in with {
        if !out.contains(&m.name) {
            out.push(m.name.clone());
        }
    }
    Ok(out)
}

/// The source of the generated package's `main.rs`: akit's command line with the `MODULES` of
/// `crates`, as an akit of version `version`.
#[must_use]
pub fn main_source(version: &str, crates: &[String]) -> String {
    let mut s = String::from(
        "// Code generated by akit build. DO NOT EDIT.\n\
         \n\
         //! akit, built with the modules of the crates listed below.\n\
         #![forbid(unsafe_code)]\n\
         \n\
         fn main() -> std::process::ExitCode {\n",
    );
    s.push_str(&format!(
        "    akit::main(\n        {version:?},\n        &[\n"
    ));
    for c in crates {
        let ident = c.replace('-', "_");
        if STANDARD.contains(&c.as_str()) {
            s.push_str(&format!("            akit::{ident}::MODULES,\n"));
        } else {
            s.push_str(&format!("            {ident}::MODULES,\n"));
        }
    }
    s.push_str("        ],\n    )\n}\n");
    s
}

/// `s` as a TOML basic string.
fn toml_str(s: &str) -> String {
    let mut out = String::with_capacity(s.len().saturating_add(2));
    out.push('"');
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            c if c.is_control() => out.push_str(&format!("\\u{:04X}", u32::from(c))),
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

/// A git reference as `Cargo.toml` writes it: `rev`, `tag` or `branch`.
fn git_reference(reference: Option<&str>) -> String {
    match reference {
        None => String::new(),
        Some(r) if is_commit(r) => format!(", rev = {}", toml_str(r)),
        Some(r) if is_tag(r) => format!(", tag = {}", toml_str(r)),
        Some(r) => format!(", branch = {}", toml_str(r)),
    }
}

/// A dependency's table, for a crate from `source`; directories are absolute already.
fn dependency(source: &Source) -> String {
    match source {
        Source::Registry(v) if v.is_empty() || v == "latest" => "{ version = \"*\" }".to_owned(),
        Source::Registry(v) => format!("{{ version = {} }}", toml_str(v)),
        Source::Dir(d) => format!("{{ path = {} }}", toml_str(&d.to_string_lossy())),
        Source::Git { url, reference } => format!(
            "{{ git = {}{} }}",
            toml_str(url),
            git_reference(reference.as_deref())
        ),
    }
}

/// akit from a directory: the akit crate's directory, and the workspace's root.
fn akit_dirs(dir: &Path) -> (PathBuf, PathBuf) {
    if dir.join("crates").join("akit").join("Cargo.toml").is_file() {
        (dir.join("crates").join("akit"), dir.to_path_buf())
    } else {
        // The akit crate's own directory: the workspace is two up.
        let root = dir
            .parent()
            .and_then(Path::parent)
            .map_or_else(|| dir.to_path_buf(), Path::to_path_buf);
        (dir.to_path_buf(), root)
    }
}

/// The crates of the akit workspace at `root`: each directory of `crates` with a `Cargo.toml`,
/// whose name is the package's, sorted.
fn workspace_crates(root: &Path) -> Vec<(String, PathBuf)> {
    let mut out: Vec<(String, PathBuf)> = fs::read_dir(root.join("crates"))
        .into_iter()
        .flatten()
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.join("Cargo.toml").is_file())
        .filter_map(|p| Some((p.file_name()?.to_str()?.to_owned(), p)))
        .collect();
    out.sort();
    out
}

/// Whether a crate from `source` may depend on akit by git, so that a build from a directory of
/// akit must replace akit's repository with it: one from a directory whose `Cargo.toml` names
/// akit's repository, or one from anywhere else.
fn may_use_akit_git(source: &Source) -> bool {
    match source {
        Source::Dir(d) => fs::read_to_string(d.join("Cargo.toml"))
            .map(|t| t.contains("github.com/lesismal/akit"))
            .unwrap_or(false),
        Source::Registry(_) | Source::Git { .. } => true,
    }
}

/// The generated package's `Cargo.toml`: akit from `akit`, the crates of `with`, and, from a
/// directory of akit whose workspace is at `root`, `patch` crates of it replacing akit's git
/// repository. Directories are absolute already.
fn manifest(akit: &Source, with: &[Module], patch: &[(String, PathBuf)], release: bool) -> String {
    let mut s = format!(
        "# Generated by akit build. DO NOT EDIT.\n\
         [package]\n\
         name = \"{PACKAGE}\"\n\
         version = \"0.0.0\"\n\
         edition = \"2024\"\n\
         publish = false\n\
         \n\
         [[bin]]\n\
         name = \"{PACKAGE}\"\n\
         path = \"main.rs\"\n\
         \n\
         [dependencies]\n\
         akit = {}\n",
        dependency(akit)
    );
    for m in with {
        if STANDARD.contains(&m.name.as_str()) {
            continue;
        }
        s.push_str(&format!("{} = {}\n", m.name, dependency(&m.source)));
    }
    if !patch.is_empty() {
        s.push_str(&format!("\n[patch.{}]\n", toml_str(AKIT_GIT)));
        for (name, dir) in patch {
            s.push_str(&format!(
                "{name} = {{ path = {} }}\n",
                toml_str(&dir.to_string_lossy())
            ));
        }
    }
    if release {
        // akit's release profile: a dependency's own profiles are not used.
        s.push_str("\n[profile.release]\nlto = \"thin\"\ncodegen-units = 1\npanic = \"unwind\"\n");
    }
    // The package is a workspace of its own, wherever it is generated.
    s.push_str("\n[workspace]\n");
    s
}

/// `p` made absolute against the working directory.
fn absolute(what: &str, p: &Path) -> Result<PathBuf, Error> {
    std::path::absolute(p).map_err(|e| err(format!("{what} {}: {e}", p.display())))
}

/// `source` with its directory made absolute.
fn absolute_source(what: &str, source: &Source) -> Result<Source, Error> {
    Ok(match source {
        Source::Dir(d) => Source::Dir(absolute(what, d)?),
        s => s.clone(),
    })
}

/// A directory removed when dropped: the temporary package.
struct Temp(PathBuf);

impl Drop for Temp {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

/// Generates the package `o` describes and builds it into `o.output` with cargo, whose messages
/// go to `log` as they come.
pub fn build(o: &Options, log: &mut dyn Write) -> Result<(), Error> {
    if o.output.as_os_str().is_empty() {
        return Err(err("no output"));
    }
    let Some(akit) = &o.akit else {
        return Err(err(
            "no akit to build on: name a revision of akit or a directory of its source",
        ));
    };
    let crates = crates(&o.with, &o.without)?;
    let output = absolute("output", &o.output)?;
    let akit = absolute_source("akit's directory", akit)?;
    let mut with = Vec::with_capacity(o.with.len());
    for m in &o.with {
        with.push(Module {
            name: m.name.clone(),
            source: absolute_source(&format!("{}'s directory", m.name), &m.source)?,
        });
    }

    let (_temp, dir) = match &o.keep {
        Some(k) => {
            fs::create_dir_all(k).map_err(|e| err(format!("{}: {e}", k.display())))?;
            (None, k.clone())
        }
        None => {
            let nanos = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .map_or(0, |d| d.subsec_nanos());
            let d = std::env::temp_dir().join(format!("akit-build-{}-{nanos}", std::process::id()));
            fs::create_dir(&d).map_err(|e| err(format!("{}: {e}", d.display())))?;
            (Some(Temp(d.clone())), d)
        }
    };

    let (akit_dep, patch, lock) = match &akit {
        Source::Dir(d) => {
            let (krate, root) = akit_dirs(d);
            let patch = if with.iter().any(|m| may_use_akit_git(&m.source)) {
                workspace_crates(&root)
            } else {
                Vec::new()
            };
            (Source::Dir(krate), patch, Some(root.join("Cargo.lock")))
        }
        s => (s.clone(), Vec::new(), None),
    };
    write(&dir.join("main.rs"), &main_source(&version(o), &crates))?;
    write(
        &dir.join("Cargo.toml"),
        &manifest(&akit_dep, &with, &patch, o.release),
    )?;
    let lock_file = dir.join("Cargo.lock");
    let _ = fs::remove_file(&lock_file);
    if let Some(lock) = lock.filter(|l| l.is_file()) {
        fs::copy(&lock, &lock_file).map_err(|e| err(format!("{}: {e}", lock.display())))?;
    }

    let exe = cargo_build(o, &dir, log)?;
    fs::copy(&exe, &output).map_err(|e| err(format!("writing {}: {e}", output.display())))?;
    Ok(())
}

/// The version the binary says: `o.version`, or akit's own.
fn version(o: &Options) -> String {
    if o.version.is_empty() {
        crate::VERSION.to_owned()
    } else {
        o.version.clone()
    }
}

fn write(path: &Path, text: &str) -> Result<(), Error> {
    fs::write(path, text).map_err(|e| err(format!("writing {}: {e}", path.display())))
}

/// Runs `cargo build` in `dir`, its messages to `log`, and returns the binary it built.
fn cargo_build(o: &Options, dir: &Path, log: &mut dyn Write) -> Result<PathBuf, Error> {
    let mut cmd = Command::new(&o.cargo);
    cmd.arg("build")
        .arg("--message-format=json-render-diagnostics");
    if o.release {
        cmd.arg("--release");
    }
    let mut child = cmd
        .current_dir(dir)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| err(format!("cargo build: {}: {e}", o.cargo.display())))?;
    // Cargo's messages on stdout, read meanwhile: only the binary's path is kept.
    let stdout = child.stdout.take();
    let reader = std::thread::spawn(move || stdout.map(|s| executable(BufReader::new(s))));
    if let Some(mut stderr) = child.stderr.take() {
        let _ = io::copy(&mut stderr, log);
    }
    let status = child.wait().map_err(|e| err(format!("cargo build: {e}")))?;
    let exe = reader.join().ok().flatten().flatten();
    if !status.success() {
        return Err(err(format!("cargo build: {status}")));
    }
    exe.ok_or_else(|| err("cargo build: it named no binary"))
}

/// The binary of the generated package that cargo's JSON messages on `r` name.
fn executable(r: impl BufRead) -> Option<PathBuf> {
    let mut exe = None;
    for line in r.lines() {
        let Ok(line) = line else { break };
        // Only an artifact of the package with a binary is parsed.
        if !line.contains("\"executable\":\"") || !line.contains(PACKAGE) {
            continue;
        }
        let Ok(v) = akit_config::value::parse(&line) else {
            continue;
        };
        use akit_config::value::Value;
        let is = |key: &str, want: &str| v.get(key) == Some(&Value::String(want.to_owned()));
        let name = v.get("target").and_then(|t| t.get("name"));
        if is("reason", "compiler-artifact") && name == Some(&Value::String(PACKAGE.to_owned())) {
            if let Some(Value::String(p)) = v.get("executable") {
                exe = Some(PathBuf::from(p));
            }
        }
    }
    exe
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::panic, clippy::indexing_slicing)]
mod tests {
    use super::*;

    fn registry(name: &str, v: &str) -> Module {
        Module {
            name: name.into(),
            source: Source::Registry(v.into()),
        }
    }

    #[test]
    fn modules_parse_as_with_takes_them() {
        for (input, want) in [
            ("akit-hello", registry("akit-hello", "")),
            ("akit-hello@1.2.3", registry("akit-hello", "1.2.3")),
            ("akit-hello@latest", registry("akit-hello", "latest")),
            (
                "akit_hello=../src/m",
                Module {
                    name: "akit_hello".into(),
                    source: Source::Dir("../src/m".into()),
                },
            ),
            (
                "m=C:\\src\\m",
                Module {
                    name: "m".into(),
                    source: Source::Dir("C:\\src\\m".into()),
                },
            ),
            (
                "m=git+https://example.com/m.git#v1.0.0",
                Module {
                    name: "m".into(),
                    source: Source::Git {
                        url: "https://example.com/m.git".into(),
                        reference: Some("v1.0.0".into()),
                    },
                },
            ),
            (
                "m=git+https://example.com/m.git",
                Module {
                    name: "m".into(),
                    source: Source::Git {
                        url: "https://example.com/m.git".into(),
                        reference: None,
                    },
                },
            ),
        ] {
            assert_eq!(parse_module(input), Ok(want), "{input}");
        }
        for (input, want) in [
            ("", "no crate name"),
            ("@v1", "no crate name"),
            ("=dir", "no crate name"),
            ("m@v1=dir", "a version and a directory both"),
            ("a b", "is not a crate name"),
            ("1m", "is not a crate name"),
            ("m=", "no directory"),
            ("m=git+", "no repository"),
            ("m=git+https://example.com/m#", "an empty reference"),
            ("akit=../akit", "name it with -akit"),
        ] {
            let e = parse_module(input).unwrap_err().to_string();
            assert!(e.contains(want), "{input:?}: {e}");
        }
    }

    #[test]
    fn akit_is_a_revision_or_a_directory() {
        let git = |r: &str| Source::Git {
            url: AKIT_GIT.into(),
            reference: Some(r.into()),
        };
        assert_eq!(parse_akit("v0.1.0"), git("v0.1.0"));
        assert_eq!(parse_akit("4f952de"), git("4f952de"));
        assert_eq!(parse_akit("../akit"), Source::Dir("../akit".into()));
        assert_eq!(parse_akit("akit"), Source::Dir("akit".into()));
        assert_eq!(parse_akit("vendor"), Source::Dir("vendor".into()));
        assert_eq!(parse_akit("./v1"), Source::Dir("./v1".into()));
        assert_eq!(
            parse_akit("git+https://example.com/akit#main"),
            Source::Git {
                url: "https://example.com/akit".into(),
                reference: Some("main".into()),
            }
        );
    }

    #[test]
    fn crates_and_the_generated_main() {
        let got = crates(
            &[registry("akit-a", ""), registry("akit-a", "1")],
            &["akit-cache".into()],
        )
        .unwrap();
        assert_eq!(
            got,
            ["akit-handler", "akit-middleware", "akit-proxy", "akit-a"]
        );
        let e = crates(&[], &["example".into()]).unwrap_err().to_string();
        assert!(
            e.contains("-without example: it is none of akit-handler"),
            "{e}"
        );
        // A crate of akit's own named with -with is built in once.
        assert_eq!(
            crates(&[registry("akit-cache", "")], &[]).unwrap(),
            STANDARD
        );

        let src = main_source("0.0.1+build", &got);
        for want in [
            "fn main() -> std::process::ExitCode",
            "#![forbid(unsafe_code)]",
            "akit::main(\n        \"0.0.1+build\",",
            "akit::akit_handler::MODULES,",
            "akit_a::MODULES,",
        ] {
            assert!(src.contains(want), "no {want:?} in\n{src}");
        }
        assert!(!src.contains("akit_cache"), "{src}");
    }

    #[test]
    fn the_generated_manifest() {
        let with = [
            Module {
                name: "akit-hello".into(),
                source: Source::Dir("/src/hello \"x\"".into()),
            },
            registry("akit-a", ""),
            registry("akit-b", "=1.2.3"),
            Module {
                name: "akit-c".into(),
                source: Source::Git {
                    url: "https://example.com/c".into(),
                    reference: Some("0123456789abcdef".into()),
                },
            },
            registry("akit-cache", ""),
        ];
        let m = manifest(
            &Source::Dir("/src/akit/crates/akit".into()),
            &with,
            &[(
                "akit-runtime".into(),
                "/src/akit/crates/akit-runtime".into(),
            )],
            true,
        );
        for want in [
            "name = \"akit-build\"",
            "akit = { path = \"/src/akit/crates/akit\" }",
            "akit-hello = { path = \"/src/hello \\\"x\\\"\" }",
            "akit-a = { version = \"*\" }",
            "akit-b = { version = \"=1.2.3\" }",
            "akit-c = { git = \"https://example.com/c\", rev = \"0123456789abcdef\" }",
            "[patch.\"https://github.com/lesismal/akit\"]\nakit-runtime = { path = \"/src/akit/crates/akit-runtime\" }",
            "[profile.release]\nlto = \"thin\"",
            "\n[workspace]\n",
        ] {
            assert!(m.contains(want), "no {want:?} in\n{m}");
        }
        assert!(!m.contains("akit-cache ="), "{m}");
        let m = manifest(
            &Source::Git {
                url: AKIT_GIT.into(),
                reference: Some("v0.1.0".into()),
            },
            &[],
            &[],
            false,
        );
        assert!(
            m.contains("akit = { git = \"https://github.com/lesismal/akit\", tag = \"v0.1.0\" }"),
            "{m}"
        );
        assert!(!m.contains("[patch") && !m.contains("[profile"), "{m}");
        assert_eq!(git_reference(Some("main")), ", branch = \"main\"");
        assert_eq!(toml_str("a\u{1}"), "\"a\\u0001\"");
    }

    #[test]
    fn the_binary_is_read_from_cargos_messages() {
        let messages = r#"{"reason":"compiler-artifact","target":{"name":"akit"},"executable":null}
{"reason":"compiler-artifact","target":{"name":"akit-build","kind":["bin"]},"executable":"/t/release/akit-build"}
not json "executable":"akit-build"
{"reason":"build-finished","success":true}
"#;
        assert_eq!(
            executable(messages.as_bytes()),
            Some(PathBuf::from("/t/release/akit-build"))
        );
        assert_eq!(executable(&b"{\"reason\":\"build-finished\"}\n"[..]), None);
    }

    /// The crates named here are those whose lists the library's `STANDARD` holds, in order.
    #[test]
    fn standard_crates_are_the_librarys() {
        let lists = [
            akit_handler::MODULES,
            akit_middleware::MODULES,
            akit_cache::MODULES,
            akit_proxy::MODULES,
        ];
        assert_eq!(crate::STANDARD.len(), STANDARD.len());
        for (got, want) in crate::STANDARD.iter().zip(lists) {
            let ids = |l: &[akit_runtime::Info<akit_runtime::HandlerParams>]| {
                l.iter().map(|i| i.id).collect::<Vec<_>>()
            };
            assert_eq!(ids(got), ids(want));
        }
    }

    #[test]
    fn akit_from_a_directory() {
        let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..");
        let (krate, ws) = akit_dirs(&root);
        assert!(
            krate.ends_with("crates/akit") && ws == root,
            "{krate:?} {ws:?}"
        );
        let (krate, ws) = akit_dirs(&root.join("crates/akit"));
        assert!(
            krate.ends_with("crates/akit") && ws == root,
            "{krate:?} {ws:?}"
        );
        let names: Vec<String> = workspace_crates(&root)
            .into_iter()
            .map(|(n, _)| n)
            .collect();
        assert!(
            names.contains(&"akit".to_owned()) && names.contains(&"akit-runtime".to_owned()),
            "{names:?}"
        );
        assert!(may_use_akit_git(&Source::Registry(String::new())));
        assert!(!may_use_akit_git(&Source::Dir(root.join("nothing"))));
    }
}
