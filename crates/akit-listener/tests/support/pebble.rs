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

/// A TCP port on 127.0.0.1 nothing listens on, as the system hands it out: for Pebble, an
/// external process whose config must name its ports, and for the ports Pebble dials back
/// (HTTP-01, TLS-ALPN-01), which it must know before akit binds them. Another process may take
/// it meanwhile; akit's other listeners bind port 0.
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

/// Whether something listens at `addr` within ten seconds.
fn wait_listening(addr: &str) -> bool {
    let deadline = Instant::now() + Duration::from_secs(10);
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
    /// `http_port`; `None` (the test skipped) when the binaries are not there.
    pub fn start(dir: &Path, http_port: u16) -> Option<Pebble> {
        Pebble::start_with(dir, http_port, free_port(), 7_776_000)
    }

    /// The same, checking TLS-ALPN-01 challenges on `tls_port`, its certificates valid for
    /// `validity` seconds (90 days by default).
    pub fn start_with(dir: &Path, http_port: u16, tls_port: u16, validity: u64) -> Option<Pebble> {
        let pebble = binary("pebble")?;
        let challtestsrv = binary("pebble-challtestsrv")?;
        // Its ports are picked free, and a test running alongside may take one first: tried
        // again on others.
        for _ in 0..3 {
            if let Some(p) =
                Pebble::launch(&pebble, &challtestsrv, dir, http_port, tls_port, validity)
            {
                return Some(p);
            }
        }
        panic!("Pebble did not come up");
    }

    fn launch(
        pebble: &Path,
        challtestsrv: &Path,
        dir: &Path,
        http_port: u16,
        tls_port: u16,
        validity: u64,
    ) -> Option<Pebble> {
        let mut children = Vec::new();
        let dns = format!("127.0.0.1:{}", free_port());
        let dns_mgmt = format!("127.0.0.1:{}", free_port());
        children.push(spawn(
            challtestsrv,
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
                "profiles": {{"default": {{"description": "akit's tests", "validityPeriod": {validity}}}}}
            }}}}"#,
            cert.display(),
            key.display(),
            tls_port,
        );
        let config_file = dir.join("pebble.json");
        std::fs::write(&config_file, config).unwrap();
        // No random sleeps before validating, and no nonces refused at random: the tests are
        // about akit, not about its retries of a bad nonce.
        children.push(spawn(
            pebble,
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
        // Dropped, and so stopped, when it did not come up.
        (wait_listening(&listen) && wait_listening(&mgmt)).then_some(p)
    }

    /// Has Pebble suggest, for the certificate in `cert_pem`, that it be renewed between
    /// `start` and `end` (RFC 3339): ACME Renewal Information.
    pub fn set_renewal_info(&self, cert_pem: &str, start: &str, end: &str) {
        let window = format!(r#"{{"suggestedWindow": {{"start": "{start}", "end": "{end}"}}}}"#);
        let body = format!(
            r#"{{"Certificate": {}, "ARIResponse": {}}}"#,
            json_string(cert_pem),
            json_string(&window)
        );
        let (client, engine) = self.mgmt_client();
        let r = client.send(
            ClientRequest::post(
                &format!("{}/set-renewal-info/", self.mgmt),
                body.into_bytes(),
            )
            .header("content-type", "application/json"),
        );
        let r = r.wait_timeout(Duration::from_secs(10)).unwrap().unwrap();
        engine.shutdown();
        engine.wait();
        assert_eq!(r.status, 200, "{}", String::from_utf8_lossy(&r.body));
    }

    fn mgmt_client(&self) -> (Client, Arc<Engine>) {
        let mut tls_roots = RootCerts::empty();
        tls_roots
            .add_pem(&std::fs::read(&self.ca_file).unwrap())
            .unwrap();
        let engine = Arc::new(Engine::builder().client_threads(1).build().unwrap());
        engine.start().unwrap();
        let c = Client::new(Arc::clone(&engine))
            .with_tls(ClientConfig::builder().roots(tls_roots).build().unwrap());
        (c, engine)
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

/// `s` as a JSON string.
fn json_string(s: &str) -> String {
    let mut out = String::from("\"");
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            c => out.push(c),
        }
    }
    out.push('"');
    out
}
