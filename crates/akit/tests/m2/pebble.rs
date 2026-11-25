//! Pebble, Let's Encrypt's ACME test server, with its DNS test server answering 127.0.0.1 for
//! every name: `akit-listener`'s test support (fit's `internal/pebbletest`), run from its binaries
//! or in Docker.
//!
//! The binaries are found in `$AKIT_PEBBLE_BIN`, or on `PATH`:
//!
//! ```text
//! go install github.com/letsencrypt/pebble/v2/cmd/pebble@v2.10.1
//! go install github.com/letsencrypt/pebble/v2/cmd/pebble-challtestsrv@v2.10.1
//! ```
//!
//! With `AKIT_PEBBLE_DOCKER=1` they run in Docker instead, from Let's Encrypt's images
//! (`ghcr.io/letsencrypt/pebble:2.10.1` and `pebble-challtestsrv`), on the host's network so that
//! they reach akit at 127.0.0.1 as the binaries do; the containers are removed when the test
//! ends. A test with neither is skipped, or fails when `$AKIT_PEBBLE_REQUIRED` is set.
#![allow(
    dead_code,
    unreachable_pub,
    missing_debug_implementations,
    clippy::unwrap_used,
    clippy::panic,
    clippy::arithmetic_side_effects
)]

use std::net::{TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{Duration, Instant};

use akio::Engine;
use akio::tls::{ClientConfig, RootCerts};
use akio_http::client::{Client, ClientRequest};

use crate::support::certs::Ca;

/// The Pebble the tests are written against (fit's).
pub const VERSION: &str = "2.10.1";

/// How Pebble's two programs are run.
#[derive(Clone)]
enum Runner {
    Binaries { pebble: PathBuf, dns: PathBuf },
    Docker,
}

/// A running Pebble and its DNS server, stopped when dropped.
pub struct Pebble {
    /// The URL of its ACME directory.
    pub directory: String,
    /// The PEM file of the authority its HTTPS certificate is from: `acme.ca_file`.
    pub ca_file: PathBuf,
    /// The port it checks HTTP-01 challenges on, at 127.0.0.1.
    pub http_port: u16,
    mgmt: String,
    children: Vec<Child>,
    containers: Vec<String>,
}

impl Drop for Pebble {
    fn drop(&mut self) {
        for c in &mut self.children {
            let _ = c.kill();
            let _ = c.wait();
        }
        for name in &self.containers {
            let _ = Command::new("docker")
                .args(["rm", "-f", name])
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .status();
        }
    }
}

/// A TCP port on 127.0.0.1 nothing listens on, as the system hands it out.
pub fn free_port() -> u16 {
    TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .port()
}

/// The path of the binary `name`, if it is there.
fn binary(name: &str) -> Option<PathBuf> {
    let dirs = std::env::var_os("AKIT_PEBBLE_BIN")
        .into_iter()
        .chain(std::env::var_os("PATH"))
        .flat_map(|p| std::env::split_paths(&p).collect::<Vec<_>>());
    for d in dirs {
        let p = d.join(name);
        if p.is_file() {
            return Some(p);
        }
    }
    None
}

fn runner() -> Option<Runner> {
    if std::env::var_os("AKIT_PEBBLE_DOCKER").is_some() {
        return Some(Runner::Docker);
    }
    if let (Some(pebble), Some(dns)) = (binary("pebble"), binary("pebble-challtestsrv")) {
        return Some(Runner::Binaries { pebble, dns });
    }
    if std::env::var_os("AKIT_PEBBLE_REQUIRED").is_some() {
        panic!(
            "no Pebble: its binaries are not in $AKIT_PEBBLE_BIN or on PATH, $AKIT_PEBBLE_DOCKER is not set, and $AKIT_PEBBLE_REQUIRED is"
        );
    }
    eprintln!("no Pebble binaries and no $AKIT_PEBBLE_DOCKER: skipped (see tests/m2/pebble.rs)");
    None
}

/// Containers named for this process, numbered.
static CONTAINERS: AtomicUsize = AtomicUsize::new(0);

/// Starts `program` (`pebble` or `pebble-challtestsrv`) with `env` and `args`; `dir` is where its
/// files are, mounted at the same path in a container.
fn spawn(
    runner: &Runner,
    program: &str,
    dir: &Path,
    env: &[(&str, &str)],
    args: &[&str],
    containers: &mut Vec<String>,
) -> Child {
    let mut c = match runner {
        Runner::Binaries { pebble, dns } => {
            let mut c = Command::new(if program == "pebble" { pebble } else { dns });
            for (k, v) in env {
                c.env(k, v);
            }
            c.args(args);
            c
        }
        Runner::Docker => {
            let name = format!(
                "akit-i38-pebble-{}-{}",
                std::process::id(),
                CONTAINERS.fetch_add(1, Ordering::SeqCst)
            );
            let mut c = Command::new("docker");
            c.args(["run", "--rm", "--name", &name, "--network", "host", "-v"])
                .arg(format!("{0}:{0}", dir.display()));
            for (k, v) in env {
                c.arg("-e").arg(format!("{k}={v}"));
            }
            c.arg(format!("ghcr.io/letsencrypt/{program}:{VERSION}"));
            c.args(args);
            containers.push(name);
            c
        }
    };
    c.stdin(Stdio::null());
    if std::env::var_os("AKIT_PEBBLE_VERBOSE").is_none() {
        c.stdout(Stdio::null()).stderr(Stdio::null());
    }
    c.spawn().unwrap()
}

/// Whether something listens at `addr` within `within`.
fn wait_listening(addr: &str, within: Duration) -> bool {
    let deadline = Instant::now() + within;
    while TcpStream::connect(addr).is_err() {
        if Instant::now() >= deadline {
            return false;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    true
}

impl Pebble {
    /// Starts Pebble and its DNS server, its files in `dir`, checking HTTP-01 challenges on
    /// `http_port` and TLS-ALPN-01 on `tls_port`; `None` (the test skipped) when there is no
    /// Pebble to run.
    pub fn start(dir: &Path, http_port: u16, tls_port: u16) -> Option<Pebble> {
        let runner = runner()?;
        // Its ports are picked free, and a test running alongside may take one first: tried
        // again on others.
        for _ in 0..3 {
            if let Some(p) = Pebble::launch(&runner, dir, http_port, tls_port) {
                return Some(p);
            }
        }
        panic!("Pebble did not come up");
    }

    fn launch(runner: &Runner, dir: &Path, http_port: u16, tls_port: u16) -> Option<Pebble> {
        let (mut children, mut containers) = (Vec::new(), Vec::new());
        let dns = format!("127.0.0.1:{}", free_port());
        let dns_mgmt = format!("127.0.0.1:{}", free_port());
        children.push(spawn(
            runner,
            "pebble-challtestsrv",
            dir,
            &[],
            &[
                "-defaultIPv4",
                "127.0.0.1",
                "-defaultIPv6",
                "",
                "-dnsserver",
                &dns,
                "-management",
                &dns_mgmt,
                "-http01",
                "",
                "-https01",
                "",
                "-tlsalpn01",
                "",
                "-doh",
                "",
            ],
            &mut containers,
        ));
        let ca = Ca::new("pebble test ca");
        let ca_file = ca.write(dir, "pebble-ca.pem");
        let (cert, key) = ca.leaf(dir, "pebble", &["127.0.0.1", "localhost"]);
        let listen = format!("127.0.0.1:{}", free_port());
        let mgmt = format!("127.0.0.1:{}", free_port());
        let config = format!(
            r#"{{"pebble": {{
                "listenAddress": "{listen}",
                "managementListenAddress": "{mgmt}",
                "certificate": "{}",
                "privateKey": "{}",
                "httpPort": {http_port},
                "tlsPort": {tls_port},
                "ocspResponderURL": "",
                "retryAfter": {{"authz": 1, "order": 1}},
                "keyAlgorithm": "ecdsa",
                "profiles": {{"default": {{"description": "akit's tests", "validityPeriod": 7776000}}}}
            }}}}"#,
            cert.display(),
            key.display(),
        );
        let config_file = dir.join("pebble.json");
        std::fs::write(&config_file, config).unwrap();
        // No random sleeps before validating, and no nonces refused at random: the tests are
        // about akit, not about its retries of a bad nonce.
        children.push(spawn(
            runner,
            "pebble",
            dir,
            &[("PEBBLE_VA_NOSLEEP", "1"), ("PEBBLE_WFE_NONCEREJECT", "0")],
            &["-config", config_file.to_str().unwrap(), "-dnsserver", &dns],
            &mut containers,
        ));
        let p = Pebble {
            directory: format!("https://{listen}/dir"),
            ca_file,
            http_port,
            mgmt: format!("https://{mgmt}"),
            children,
            containers,
        };
        // A container takes longer to start than a binary. Dropped, and so stopped, when it did
        // not come up.
        let within = match runner {
            Runner::Docker => Duration::from_secs(60),
            Runner::Binaries { .. } => Duration::from_secs(10),
        };
        (wait_listening(&listen, within) && wait_listening(&mgmt, within)).then_some(p)
    }

    /// The roots the certificates it issues chain to.
    pub fn roots(&self) -> RootCerts {
        let mut tls_roots = RootCerts::empty();
        tls_roots
            .add_pem(&std::fs::read(&self.ca_file).unwrap())
            .unwrap();
        let engine = Arc::new(Engine::builder().client_threads(1).build().unwrap());
        engine.start().unwrap();
        let client = Client::new(Arc::clone(&engine))
            .with_tls(ClientConfig::builder().roots(tls_roots).build().unwrap());
        let r = client
            .send(ClientRequest::get(&format!("{}/roots/0", self.mgmt)))
            .wait_timeout(Duration::from_secs(10))
            .unwrap()
            .unwrap();
        engine.shutdown();
        engine.wait();
        assert_eq!(r.status, 200);
        let mut roots = RootCerts::empty();
        assert!(roots.add_pem(&r.body).unwrap() > 0);
        roots
    }
}
