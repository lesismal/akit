//! The packaging (I65, deploy/README.md): the container image's placeholder config is valid and
//! serves its page; the systemd units, the Dockerfile and the Helm chart name commands akit has
//! and the config path each other's files use. CI's `packaging` job builds and runs them; this
//! catches the files drifting from akit (a renamed command, a moved config) in `cargo test`.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::io::{BufRead, BufReader, Read, Write};
use std::net::{SocketAddr, TcpStream};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::Duration;

fn root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}

fn read(rel: &str) -> String {
    std::fs::read_to_string(root().join(rel)).unwrap_or_else(|e| panic!("{rel}: {e}"))
}

/// The running binary, killed when dropped.
struct Akit(Child);

impl Drop for Akit {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

#[test]
fn image_placeholder_config_is_valid_and_serves() {
    let placeholder = root().join("deploy/docker/akit.json");
    let out = Command::new(env!("CARGO_BIN_EXE_akit"))
        .args(["validate", "-c"])
        .arg(&placeholder)
        .output()
        .unwrap();
    assert!(out.status.success(), "{out:?}");

    // The same config on a port of the test's own: the image listens on :8080.
    let text = read("deploy/docker/akit.json");
    assert!(text.contains(r#""address": ":8080""#), "{text}");
    let path = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("packaging-placeholder.json");
    std::fs::write(
        &path,
        text.replace(r#""address": ":8080""#, r#""address": "127.0.0.1:0""#),
    )
    .unwrap();
    let mut child = Command::new(env!("CARGO_BIN_EXE_akit"))
        .args(["run", "-c"])
        .arg(&path)
        .stdout(Stdio::piped())
        .stderr(Stdio::inherit())
        .spawn()
        .unwrap();
    let stdout = child.stdout.take().unwrap();
    let _akit = Akit(child);
    let mut line = String::new();
    BufReader::new(stdout).read_line(&mut line).unwrap();
    // akit: listener "web" on 127.0.0.1:PORT (http/1.1, h2c)
    assert!(line.starts_with("akit: listener \"web\" on "), "{line:?}");
    let addr: SocketAddr = line.split(' ').nth(4).unwrap().parse().unwrap();

    let mut s = TcpStream::connect(addr).unwrap();
    s.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
    s.write_all(b"GET / HTTP/1.1\r\nHost: x\r\nConnection: close\r\n\r\n")
        .unwrap();
    let mut resp = String::new();
    s.read_to_string(&mut resp).unwrap();
    assert!(resp.starts_with("HTTP/1.1 200"), "{resp}");
    assert!(
        resp.ends_with("akit is running; mount a config of your own at /etc/akit/akit.json\n"),
        "{resp}"
    );
}

/// The `akit` command lines a file runs: what follows `/usr/local/bin/akit ` on each line.
fn command_lines(text: &str) -> Vec<String> {
    text.lines()
        .filter(|l| !l.trim_start().starts_with('#'))
        .filter_map(|l| {
            l.split_once("/usr/local/bin/akit ")
                .map(|(_, c)| c.to_owned())
        })
        .collect()
}

#[test]
fn systemd_units_run_akit_commands_on_their_config() {
    for (unit, config) in [
        ("deploy/systemd/akit.service", "/etc/akit/akit.json"),
        ("deploy/systemd/akit@.service", "/etc/akit/%i.json"),
    ] {
        let text = read(unit);
        let lines = command_lines(&text);
        assert_eq!(
            lines,
            [
                format!("validate -c {config}"),
                format!("run -c {config}"),
                format!("validate -c {config}"),
            ],
            "{unit}"
        );
        for want in [
            "User=akit",
            "StateDirectory=akit",
            "RuntimeDirectory=akit",
            "ExecReload=/bin/kill -HUP $MAINPID",
            "AmbientCapabilities=CAP_NET_BIND_SERVICE",
            "CapabilityBoundingSet=CAP_NET_BIND_SERVICE",
            // run's -shutdown-timeout is 30s: systemd waits longer.
            "TimeoutStopSec=40",
        ] {
            assert!(text.contains(want), "{unit} lacks {want}");
        }
    }
    assert!(read("deploy/systemd/akit.sysusers").contains("u akit - \"akit\" /var/lib/akit"));

    // Each command the units run is one akit has: `-h` prints its flags and exits 0.
    for cmd in ["validate", "run"] {
        let out = Command::new(env!("CARGO_BIN_EXE_akit"))
            .args([cmd, "-h"])
            .output()
            .unwrap();
        let text = String::from_utf8_lossy(&out.stderr).into_owned()
            + &String::from_utf8_lossy(&out.stdout);
        assert!(text.contains("-c"), "akit {cmd} -h: {text}");
    }
}

#[test]
fn image_and_chart_agree_on_the_config_path() {
    let dockerfile = read("Dockerfile");
    assert!(dockerfile.contains("COPY deploy/docker/akit.json /etc/akit/akit.json"));
    assert!(dockerfile.contains(r#"CMD ["run", "-c", "/etc/akit/akit.json"]"#));
    assert!(dockerfile.contains("FROM gcr.io/distroless/static-debian12:nonroot"));

    let deployment = read("deploy/helm/akit/templates/deployment.yaml");
    for want in [
        "- run\n            - -c\n            - /etc/akit/akit.json",
        "mountPath: /etc/akit",
    ] {
        assert!(deployment.contains(want), "deployment.yaml lacks {want:?}");
    }
    assert!(read("deploy/helm/akit/templates/configmap.yaml").contains("  akit.json: |"));

    // The chart's image runs as the image's user.
    let values = read("deploy/helm/akit/values.yaml");
    assert!(values.contains("runAsUser: 65532"));
    assert!(values.contains("repository: ghcr.io/lesismal/akit"));
}

#[test]
fn version_names_akit() {
    let out = Command::new(env!("CARGO_BIN_EXE_akit"))
        .arg("version")
        .output()
        .unwrap();
    let text = String::from_utf8(out.stdout).unwrap();
    let first = text.lines().next().unwrap();
    // `akit <AKIT_VERSION or the crate's version>`, which scripts/check-dist.sh reads.
    let want = option_env!("AKIT_VERSION")
        .filter(|v| !v.is_empty())
        .unwrap_or(env!("CARGO_PKG_VERSION"));
    assert_eq!(first, format!("akit {want}"));
}
