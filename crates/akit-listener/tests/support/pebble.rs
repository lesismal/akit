//! Pebble, Let's Encrypt's ACME test server, with its DNS test server answering 127.0.0.1 for
//! every name, for tests that obtain certificates: fit's `internal/pebbletest`.
//!
//! The binaries are found in `$AKIT_PEBBLE_BIN`, or on `PATH`; a test without them is skipped,
//! or fails when `$AKIT_PEBBLE_REQUIRED` is set. Install them with:
//!
//! ```text
//! go install github.com/letsencrypt/pebble/v2/cmd/pebble@v2.10.1
//! go install github.com/letsencrypt/pebble/v2/cmd/pebble-challtestsrv@v2.10.1
//! ```
#![allow(
    dead_code,
    unreachable_pub,
    clippy::unwrap_used,
    clippy::panic,
    clippy::arithmetic_side_effects
)]

use std::net::{TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::Arc;
use std::time::{Duration, Instant};

use akio::Engine;
use akio::tls::{ClientConfig, RootCerts};
use akio_http::client::{Client, ClientRequest};

use super::certs::Ca;

/// The Pebble the tests are written against (fit's).
pub const VERSION: &str = "v2.10.1";

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
}

impl Drop for Pebble {
    fn drop(&mut self) {
        for c in &mut self.children {
            let _ = c.kill();
            let _ = c.wait();
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

/// The path of the binary `name`, or `None` when it is not there and not required.
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
    if std::env::var_os("AKIT_PEBBLE_REQUIRED").is_some() {
        panic!("{name} not found in $AKIT_PEBBLE_BIN or on PATH, and $AKIT_PEBBLE_REQUIRED is set");
    }
    eprintln!("{name} not found in $AKIT_PEBBLE_BIN or on PATH: skipped (see support/pebble.rs)");
    None
}

fn spawn(cmd: &Path, env: &[(&str, &str)], args: &[&str]) -> Child {
    let mut c = Command::new(cmd);
    c.args(args).stdin(Stdio::null());
    if std::env::var_os("AKIT_PEBBLE_VERBOSE").is_none() {
        c.stdout(Stdio::null()).stderr(Stdio::null());
    }
    for (k, v) in env {
        c.env(k, v);
    }
    c.spawn().unwrap()
}

fn wait_listening(addr: &str) {
    let deadline = Instant::now() + Duration::from_secs(10);
    while TcpStream::connect(addr).is_err() {
        assert!(Instant::now() < deadline, "{addr} did not come up");
        std::thread::sleep(Duration::from_millis(50));
    }
}

impl Pebble {
    /// Starts Pebble and its DNS server, its files in `dir`, checking HTTP-01 challenges on
    /// `http_port`; `None` (the test skipped) when the binaries are not there.
    pub fn start(dir: &Path, http_port: u16) -> Option<Pebble> {
        let pebble = binary("pebble")?;
        let challtestsrv = binary("pebble-challtestsrv")?;
        let mut children = Vec::new();
        let dns = format!("127.0.0.1:{}", free_port());
        let dns_mgmt = format!("127.0.0.1:{}", free_port());
        children.push(spawn(
            &challtestsrv,
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
                "tlsPort": {},
                "ocspResponderURL": "",
                "retryAfter": {{"authz": 1, "order": 1}},
                "keyAlgorithm": "ecdsa",
                "profiles": {{"default": {{"description": "akit's tests", "validityPeriod": 7776000}}}}
            }}}}"#,
            cert.display(),
            key.display(),
            free_port(),
        );
        let config_file = dir.join("pebble.json");
        std::fs::write(&config_file, config).unwrap();
        // No random sleeps before validating, and no nonces refused at random: the tests are
        // about akit, not about its retries of a bad nonce.
        children.push(spawn(
            &pebble,
            &[("PEBBLE_VA_NOSLEEP", "1"), ("PEBBLE_WFE_NONCEREJECT", "0")],
            &["-config", config_file.to_str().unwrap(), "-dnsserver", &dns],
        ));
        let p = Pebble {
            directory: format!("https://{listen}/dir"),
            ca_file,
            http_port,
            mgmt: format!("https://{mgmt}"),
            children,
        };
        wait_listening(&listen);
        wait_listening(&mgmt);
        Some(p)
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
