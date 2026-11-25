//! `akit build` (I70), as fit's `plugin/build` and `cli` tests test `fit build`: its flags, each
//! step that can fail, and a build of akit from this checkout with the example module in
//! `testdata/hello-module`, whose binary serves a route through the module.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::fs;
use std::io::{BufRead, BufReader, Read, Write};
use std::net::{SocketAddr, TcpStream};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Output, Stdio};
use std::time::Duration;

use akit::build::{self, Module, Options, Source};

/// This checkout of akit.
fn root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..")
}

fn dir(name: &str) -> PathBuf {
    let d = PathBuf::from(env!("CARGO_TARGET_TMPDIR"))
        .join("build")
        .join(name);
    let _ = fs::remove_dir_all(&d);
    fs::create_dir_all(&d).unwrap();
    d
}

fn akit(args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_akit"))
        .args(args)
        .stdin(Stdio::null())
        .output()
        .unwrap()
}

fn text(b: &[u8]) -> String {
    String::from_utf8_lossy(b).into_owned()
}

/// fit's `TestBuildFlags`: what the command refuses before it builds (2), and what fails once it
/// builds (1), saying so.
#[test]
fn build_flags() {
    let d = dir("flags");
    let file = d.join("file");
    fs::write(&file, "").unwrap();
    // -keep under a file fails before cargo runs.
    let keep = file.join("keep");
    let keep = keep.to_str().unwrap();
    let out = d.join("akit");
    let out = out.to_str().unwrap();
    let d_str = d.to_str().unwrap();
    for (args, code, want) in [
        (&["-bogus"][..], 2, "Usage of akit build"),
        (&["-h"][..], 2, "-without"),
        (
            &["extra"][..],
            2,
            "akit build: unexpected argument \"extra\"",
        ),
        (&["-with", "@v1"][..], 2, "no crate name"),
        (
            &["-with", "m@v1=dir"][..],
            2,
            "a version and a directory both",
        ),
        (
            &["-akit", "v1.0.0", "-without", "example-none"][..],
            2,
            "-without example-none",
        ),
        (
            &["-akit", "v1.0.0", "-keep", keep, "-o", out][..],
            1,
            "akit build: ",
        ),
        (
            &[
                "-akit", d_str, "-with", "m", "-with", "m", "-keep", keep, "-o", out,
            ][..],
            1,
            "akit build: ",
        ),
    ] {
        let o = akit(&[&["build"][..], args].concat());
        let (stdout, stderr) = (text(&o.stdout), text(&o.stderr));
        assert!(
            o.status.code() == Some(code) && stderr.contains(want),
            "{args:?}: {:?} {stdout:?} {stderr:?}, want {code} and {want:?}",
            o.status.code()
        );
        if code == 1 {
            assert!(
                stdout.contains("akit build: building "),
                "{args:?}: {stdout}"
            );
        }
    }
    // With no -akit, this akit's own source is built on: it fails at -keep.
    let o = akit(&["build", "-keep", keep, "-o", out]);
    assert_eq!(o.status.code(), Some(1), "{}", text(&o.stderr));
    assert!(
        text(&o.stdout).contains("on akit from ") && text(&o.stdout).contains("crates/akit"),
        "{}",
        text(&o.stdout)
    );
    // The command is in the help.
    assert!(text(&akit(&["help"]).stdout).contains("  build "));
}

/// fit's `TestBuildErrors`: each step that fails is reported: the options, the directory the
/// package is generated in, and cargo.
#[test]
fn build_errors() {
    let d = dir("errors");
    let file = d.join("file");
    fs::write(&file, "").unwrap();
    let main_dir = d.join("main");
    fs::create_dir_all(main_dir.join("main.rs")).unwrap();
    let out = d.join("akit");
    let git = build::parse_akit("v1.0.0");
    let mut log = Vec::new();
    for (name, o, want) in [
        (
            "no output",
            Options::new("", git.clone()),
            "no output".to_owned(),
        ),
        (
            "no akit",
            Options {
                akit: None,
                ..Options::new(&out, git.clone())
            },
            "no akit to build on".to_owned(),
        ),
        (
            "without",
            Options {
                without: vec!["example-x".into()],
                ..Options::new(&out, git.clone())
            },
            "example-x".to_owned(),
        ),
        (
            "keep under a file",
            Options {
                keep: Some(file.join("sub")),
                ..Options::new(&out, git.clone())
            },
            "file".to_owned(),
        ),
        (
            "main.rs a directory",
            Options {
                keep: Some(main_dir.clone()),
                ..Options::new(&out, git.clone())
            },
            "main.rs".to_owned(),
        ),
        (
            "no cargo",
            Options {
                cargo: d.join("no-cargo"),
                keep: Some(d.join("no-cargo-main")),
                ..Options::new(&out, git.clone())
            },
            "cargo build: ".to_owned(),
        ),
    ] {
        let e = build::build(&o, &mut log).unwrap_err().to_string();
        assert!(e.contains(&want), "{name}: {e}, want {want:?}");
    }

    // No network: what is not in cargo's caches cannot be fetched.
    #[cfg(unix)]
    for (name, o) in [
        (
            "akit not found",
            Options::new(&out, build::parse_akit("v9.9.9")),
        ),
        (
            "module not found",
            Options {
                with: vec![build::parse_module("akit-nothing-here").unwrap()],
                ..Options::new(&out, Source::Dir(root()))
            },
        ),
        (
            "akit not there",
            Options::new(&out, Source::Dir(d.join("empty"))),
        ),
    ] {
        let o = Options {
            keep: Some(dir(&format!("offline-{}", name.replace(' ', "-")))),
            cargo: offline_cargo(),
            release: false,
            ..o
        };
        let mut log = Vec::new();
        let e = build::build(&o, &mut log).unwrap_err().to_string();
        assert!(
            e.starts_with("cargo build: "),
            "{name}: {e}\n{}",
            text(&log)
        );
        assert!(!log.is_empty(), "{name}: cargo said nothing");
    }
}

/// A cargo that runs offline: a script that sets `CARGO_NET_OFFLINE` and runs the cargo that
/// built this test, with a target directory of its own (never the one the cargo running this test
/// holds, which `CARGO_TARGET_DIR` may name). Written once, by [`cargo_shim`].
#[cfg(unix)]
fn offline_cargo() -> PathBuf {
    use std::sync::OnceLock;
    static SHIM: OnceLock<PathBuf> = OnceLock::new();
    SHIM.get_or_init(|| {
        let d = dir("offline-cargo");
        cargo_shim(
            &d.join("cargo"),
            &format!(
                "CARGO_NET_OFFLINE=true CARGO_TARGET_DIR='{}'",
                d.join("target").display()
            ),
        )
    })
    .clone()
}

/// Writes `shim`, a script that runs the cargo that built this test with `env` set, and returns it
/// once it runs.
///
/// A child another test forks while the script is open for writing holds it open until that child
/// execs, and running the script until then fails with "text file busy" (Linux's `ETXTBSY`),
/// cargo never started. So the script is run (`cargo --version`) until it is no longer busy.
#[cfg(unix)]
fn cargo_shim(shim: &Path, env: &str) -> PathBuf {
    use std::os::unix::fs::PermissionsExt;
    fs::write(
        shim,
        format!("#!/bin/sh\n{env} exec '{}' \"$@\"\n", env!("CARGO")),
    )
    .unwrap();
    fs::set_permissions(shim, fs::Permissions::from_mode(0o755)).unwrap();
    for _ in 0..1000 {
        match Command::new(shim).arg("--version").output() {
            Err(e) if e.kind() == std::io::ErrorKind::ExecutableFileBusy => {
                std::thread::sleep(Duration::from_millis(10));
            }
            r => {
                let o = r.unwrap();
                assert!(
                    o.status.success(),
                    "{}: {}",
                    shim.display(),
                    text(&o.stderr)
                );
                return shim.to_owned();
            }
        }
    }
    panic!("{}: still busy after 10s", shim.display());
}

/// fit's `TestBuildNoWorkingDirectory`: a relative output cannot be made absolute when the working
/// directory is gone, and the build says so.
#[cfg(unix)]
#[test]
fn build_with_no_working_directory() {
    let d = dir("no-cwd");
    let script = format!(
        "mkdir gone && cd gone && rmdir ../gone && exec '{}' build -akit v1.0.0 -o akit-out",
        env!("CARGO_BIN_EXE_akit")
    );
    let o = Command::new("/bin/sh")
        .arg("-c")
        .arg(script)
        .current_dir(&d)
        .stdin(Stdio::null())
        .output()
        .unwrap();
    let stderr = text(&o.stderr);
    assert_eq!(o.status.code(), Some(1), "{stderr}");
    assert!(stderr.contains("akit build: output akit-out: "), "{stderr}");
}

/// fit's `TestBuildWithModule`: akit built from this checkout with the example module in
/// `testdata/hello-module` and without the cache, whose binary says its version, refuses the
/// module left out, and serves a route through the example module. It runs cargo, with the crates
/// akit uses from cargo's caches or the network, into a target directory of its own under this
/// one's, kept between runs so that a run after the first builds little.
#[test]
fn build_with_a_module_and_serve() {
    let root = root();
    let d = dir("with-module");
    let bin = d.join(format!("akit{}", std::env::consts::EXE_SUFFIX));
    let target = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("akit-build-target");
    let o = Options {
        with: vec![Module {
            name: "akit-hello".into(),
            source: Source::Dir(root.join("crates/akit/tests/testdata/hello-module")),
        }],
        without: vec!["akit-cache".into()],
        version: "test+build".into(),
        keep: Some(d.join("main")),
        cargo: shared_target_cargo(&target),
        release: false,
        ..Options::new(&bin, Source::Dir(root.clone()))
    };
    let mut log = Vec::new();
    if let Err(e) = build::build(&o, &mut log) {
        panic!("{e}\n{}", text(&log));
    }
    // The package it generated, kept.
    let main = fs::read_to_string(d.join("main/main.rs")).unwrap();
    assert!(
        main.contains("akit_hello::MODULES") && !main.contains("akit_cache"),
        "{main}"
    );
    assert!(d.join("main/Cargo.lock").is_file());

    let out = Command::new(&bin).arg("version").output().unwrap();
    assert!(
        text(&out.stdout).starts_with("akit test+build\n"),
        "{}",
        text(&out.stdout)
    );

    let cfg = d.join("akit.json");
    fs::write(
        &cfg,
        r#"{"listeners": [{"name": "web", "address": "127.0.0.1:0"}],
  "routes": [{"middleware": [{"module": "hello", "params": {"greeting": "from a module"}}],
    "handler": {"module": "respond", "params": {"body": "hi"}}}]}"#,
    )
    .unwrap();
    let out = Command::new(&bin)
        .args(["validate", "-c"])
        .arg(&cfg)
        .output()
        .unwrap();
    assert!(out.status.success(), "validate: {}", text(&out.stderr));

    // A module's params are its own to check.
    let bad = d.join("bad.json");
    fs::write(
        &bad,
        r#"{"listeners": [{"name": "web", "address": "127.0.0.1:0"}],
  "routes": [{"middleware": [{"module": "hello", "params": {"greting": "x"}}],
    "handler": {"module": "respond"}}]}"#,
    )
    .unwrap();
    let out = Command::new(&bin)
        .args(["validate", "-c"])
        .arg(&bad)
        .output()
        .unwrap();
    let stderr = text(&out.stderr);
    assert_eq!(out.status.code(), Some(1), "{stderr}");
    assert!(
        stderr.contains("routes[0].middleware[0].params.greting: unknown field"),
        "{stderr}"
    );
    // The module left out is not there, and akit's binary has no hello.
    let cache = d.join("cache.json");
    fs::write(
        &cache,
        r#"{"listeners": [{"name": "web", "address": "127.0.0.1:0"}],
  "routes": [{"middleware": [{"module": "cache"}], "handler": {"module": "respond"}}]}"#,
    )
    .unwrap();
    let out = Command::new(&bin)
        .args(["validate", "-c"])
        .arg(&cache)
        .output()
        .unwrap();
    let stderr = text(&out.stderr);
    assert!(
        stderr.contains(
            r#"routes[0].middleware[0].module: there is no module "cache" in http.middleware"#
        ) && stderr.contains(r#""hello""#),
        "{stderr}"
    );
    let out = akit(&["validate", "-c", cfg.to_str().unwrap()]);
    let stderr = text(&out.stderr);
    assert!(
        stderr.contains(r#"routes[0].middleware[0].module: there is no module "hello" in http.middleware (it has "basic_auth","#),
        "{stderr}"
    );

    let mut child = Running::start(&bin, &cfg);
    let resp = get(child.addr);
    assert!(resp.starts_with("HTTP/1.1 200"), "{resp}");
    assert!(resp.contains("\r\nX-Hello: from a module\r\n"), "{resp}");
    assert!(resp.ends_with("\r\n\r\nhi"), "{resp}");
    child.stop();
}

/// A cargo whose builds share `target` (`CARGO_TARGET_DIR`), apart from the target directory of
/// the cargo running this test, which it holds locked.
fn shared_target_cargo(target: &Path) -> PathBuf {
    let shim = dir("shared-target-cargo").join("cargo");
    #[cfg(unix)]
    {
        cargo_shim(&shim, &format!("CARGO_TARGET_DIR='{}'", target.display()))
    }
    #[cfg(not(unix))]
    {
        let _ = (shim, target);
        PathBuf::from(env!("CARGO"))
    }
}

/// The built akit running a config, and the address its listener got.
struct Running {
    child: Child,
    addr: SocketAddr,
}

impl Running {
    fn start(bin: &Path, cfg: &Path) -> Running {
        let mut child = Command::new(bin)
            .args(["run", "-c"])
            .arg(cfg)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .unwrap();
        let mut out = BufReader::new(child.stdout.take().unwrap());
        let mut line = String::new();
        out.read_line(&mut line).unwrap();
        assert!(line.starts_with("akit: listener \"web\" on "), "{line:?}");
        let addr = line.split(' ').nth(4).unwrap().trim().parse().unwrap();
        // The rest of stdout is drained, so that akit never waits on it.
        std::thread::spawn(move || std::io::copy(&mut out, &mut std::io::sink()));
        Running { child, addr }
    }

    fn stop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

impl Drop for Running {
    fn drop(&mut self) {
        self.stop();
    }
}

fn get(addr: SocketAddr) -> String {
    let mut s = TcpStream::connect(addr).unwrap();
    s.set_read_timeout(Some(Duration::from_secs(10))).unwrap();
    write!(s, "GET / HTTP/1.1\r\nHost: x\r\nConnection: close\r\n\r\n").unwrap();
    let mut resp = String::new();
    s.read_to_string(&mut resp).unwrap();
    resp
}
