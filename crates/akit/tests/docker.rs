//! End to end: `akit run` with fit's Docker Compose example's config (I58), against a fake
//! Docker daemon on a Unix socket: the services its containers' labels write are routed as the
//! containers start and stop, and what the config writes (the fallback route) stays.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing,
    clippy::arithmetic_side_effects
)]

use std::io::{BufRead, BufReader, Read, Write};
use std::net::{SocketAddr, TcpListener, TcpStream};
use std::os::unix::net::UnixListener;
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::sync::{Arc, Mutex, mpsc};
use std::time::{Duration, Instant};

use akio_sys::process::{SIGTERM, kill};

/// The running binary, killed when dropped, and its stdout and stderr lines.
struct Akit {
    child: Child,
    out: mpsc::Receiver<String>,
}

impl Drop for Akit {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

impl Akit {
    fn start(config: &str) -> (Akit, SocketAddr) {
        let mut child = Command::new(env!("CARGO_BIN_EXE_akit"))
            .args(["run", "-c", config])
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        let (tx, out) = mpsc::channel();
        for r in [
            Box::new(child.stdout.take().unwrap()) as Box<dyn Read + Send>,
            Box::new(child.stderr.take().unwrap()),
        ] {
            let tx = tx.clone();
            std::thread::spawn(move || {
                for line in BufReader::new(r).lines() {
                    let Ok(line) = line else { break };
                    if tx.send(line).is_err() {
                        break;
                    }
                }
            });
        }
        let akit = Akit { child, out };
        let line = akit.until("akit: listener").pop().unwrap();
        let addr = line.split(' ').nth(4).unwrap().parse().unwrap();
        (akit, addr)
    }

    /// Its output lines up to and including the first that starts with `last`.
    fn until(&self, last: &str) -> Vec<String> {
        let mut v = Vec::new();
        loop {
            let line = self
                .out
                .recv_timeout(Duration::from_secs(10))
                .unwrap_or_else(|e| panic!("waiting for {last:?} after {v:?}: {e}"));
            let done = line.starts_with(last);
            v.push(line);
            if done {
                return v;
            }
        }
    }

    fn stop(mut self) {
        kill(self.child.id(), SIGTERM).unwrap();
        self.until("akit: stopped");
        assert_eq!(self.child.wait().unwrap().code(), Some(0));
    }
}

/// A server answering every request with `body`; its port.
fn upstream(body: &'static str) -> u16 {
    let l = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = l.local_addr().unwrap().port();
    std::thread::spawn(move || {
        for c in l.incoming() {
            let Ok(c) = c else { return };
            std::thread::spawn(move || {
                let mut w = c.try_clone().unwrap();
                let mut r = BufReader::new(c);
                loop {
                    let mut line = String::new();
                    if r.read_line(&mut line).unwrap_or(0) == 0 {
                        return;
                    }
                    loop {
                        let mut h = String::new();
                        if r.read_line(&mut h).unwrap_or(0) == 0 || h == "\r\n" {
                            break;
                        }
                    }
                    if write!(
                        w,
                        "HTTP/1.1 200 OK\r\nContent-Length: {}\r\n\r\n{body}",
                        body.len()
                    )
                    .is_err()
                    {
                        return;
                    }
                }
            });
        }
    });
    port
}

/// A container of the Compose project `shop`, routing `host` to its `port` on 127.0.0.1, as the
/// daemon lists it.
fn container(service: &str, host: &str, port: u16) -> String {
    format!(
        r#"{{"Id":"id-{service}","Names":["/shop-{service}-1"],"State":"running","Status":"Up 1 second",
"Labels":{{"akit.enable":"true","com.docker.compose.project":"shop","com.docker.compose.service":"{service}",
"akit.port":"{port}","akit.routes.main.host":"{host}"}},
"Ports":[],"HostConfig":{{"NetworkMode":"bridge"}},
"NetworkSettings":{{"Networks":{{"shop_default":{{"IPAddress":"127.0.0.1","GlobalIPv6Address":""}}}}}}}}"#
    )
}

/// A fake Docker daemon: it lists the containers set, and streams the events sent.
struct Daemon {
    containers: Arc<Mutex<Vec<String>>>,
    events: mpsc::Sender<String>,
}

impl Daemon {
    fn start(socket: &PathBuf) -> Daemon {
        let l = UnixListener::bind(socket).unwrap();
        let containers = Arc::new(Mutex::new(Vec::new()));
        let (tx, rx) = mpsc::channel::<String>();
        let rx = Arc::new(Mutex::new(rx));
        let cs = Arc::clone(&containers);
        std::thread::spawn(move || {
            for c in l.incoming() {
                let Ok(c) = c else { return };
                let (cs, rx) = (Arc::clone(&cs), Arc::clone(&rx));
                std::thread::spawn(move || {
                    let mut w = c.try_clone().unwrap();
                    let mut r = BufReader::new(c);
                    loop {
                        let mut line = String::new();
                        if r.read_line(&mut line).unwrap_or(0) == 0 {
                            return;
                        }
                        loop {
                            let mut h = String::new();
                            if r.read_line(&mut h).unwrap_or(0) == 0 || h == "\r\n" {
                                break;
                            }
                        }
                        if line.starts_with("GET /events") {
                            let _ = w.write_all(
                                b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n",
                            );
                            loop {
                                let e = rx.lock().unwrap().recv_timeout(Duration::from_millis(10));
                                if let Ok(e) = e {
                                    let e = format!("{e}\n");
                                    if write!(w, "{:x}\r\n{e}\r\n", e.len()).is_err() {
                                        return;
                                    }
                                }
                            }
                        }
                        let body = format!("[{}]", cs.lock().unwrap().join(","));
                        if write!(
                            w,
                            "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{body}",
                            body.len()
                        )
                        .is_err()
                        {
                            return;
                        }
                    }
                });
            }
        });
        Daemon {
            containers,
            events: tx,
        }
    }

    /// The containers become `cs`, and an event says so.
    fn set(&self, cs: Vec<String>, action: &str) {
        *self.containers.lock().unwrap() = cs;
        self.events
            .send(format!(r#"{{"Type":"container","Action":"{action}"}}"#))
            .unwrap();
    }
}

/// `GET /` for `host`: the status code and the body.
fn get(addr: SocketAddr, host: &str) -> (u16, String) {
    let mut c = TcpStream::connect(addr).unwrap();
    c.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
    write!(
        c,
        "GET / HTTP/1.1\r\nHost: {host}\r\nConnection: close\r\n\r\n"
    )
    .unwrap();
    let mut resp = String::new();
    c.read_to_string(&mut resp).unwrap();
    let code = resp.get(9..12).unwrap().parse().unwrap();
    let body = resp.split_once("\r\n\r\n").unwrap().1.to_owned();
    (code, body)
}

/// Waits until `GET /` for `host` answers `code` and `body`.
fn wait_get(addr: SocketAddr, host: &str, code: u16, body: &str) {
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        let got = get(addr, host);
        if got == (code, body.to_owned()) {
            return;
        }
        assert!(
            Instant::now() < deadline,
            "{host}: {got:?}, want {code} {body:?}"
        );
        std::thread::sleep(Duration::from_millis(20));
    }
}

#[test]
fn compose_services_are_routed_as_they_start_and_stop() {
    let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("docker");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    // Short, for sun_path.
    let socket = PathBuf::from(format!("/tmp/akit-e2e-{}.sock", std::process::id()));
    let _ = std::fs::remove_file(&socket);
    let daemon = Daemon::start(&socket);
    let (web, api) = (upstream("web"), upstream("api"));
    daemon.set(vec![container("web", "web.example", web)], "start");

    // fit's examples/docker-compose/fit.conf, on a port of its own and the fake daemon.
    let config = dir.join("fit.conf");
    std::fs::write(
        &config,
        format!(
            r#"listener web {{
	address 127.0.0.1:0
	protocols http/1.1 h2c
}}

provider {{
	docker {{
		endpoint unix://{}
	}}
}}

route not-found {{
	priority -1
	handler respond {{
		status 404
		body "no route for this host\n"
	}}
}}
"#,
            socket.display()
        ),
    )
    .unwrap();
    let (akit, addr) = Akit::start(&config.display().to_string());
    akit.until("akit: applied what provider docker provides");
    wait_get(addr, "web.example", 200, "web");
    assert_eq!(
        get(addr, "api.example"),
        (404, "no route for this host\n".into())
    );

    // A service starts.
    daemon.set(
        vec![
            container("web", "web.example", web),
            container("api", "api.example", api),
        ],
        "start",
    );
    akit.until("akit: applied what provider docker provides");
    wait_get(addr, "api.example", 200, "api");
    wait_get(addr, "web.example", 200, "web");

    // One stops.
    daemon.set(vec![container("api", "api.example", api)], "die");
    akit.until("akit: applied what provider docker provides");
    wait_get(addr, "web.example", 404, "no route for this host\n");
    wait_get(addr, "api.example", 200, "api");

    akit.stop();
    let _ = std::fs::remove_file(&socket);
    let _ = std::fs::remove_dir_all(&dir);
}
