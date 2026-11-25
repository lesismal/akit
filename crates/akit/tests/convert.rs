//! End to end (I74): an nginx config and a Caddyfile converted with `akit adapt -from`, then
//! served by `akit run`: a file from the root, a response, a redirect, and a proxy with the
//! location's prefix replaced, as fit's converters' `TestServe` serve them.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]

use std::io::{BufRead, BufReader, Read, Write};
use std::net::{SocketAddr, TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::Duration;

/// The running binary, killed when dropped.
struct Akit(Child);

impl Drop for Akit {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

fn dir(name: &str) -> PathBuf {
    let d = PathBuf::from(env!("CARGO_TARGET_TMPDIR"))
        .join("convert")
        .join(name);
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

/// A backend answering `backend <path> <X-Env>` to every request, on connections kept open.
fn backend() -> SocketAddr {
    let l = TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = l.local_addr().unwrap();
    std::thread::spawn(move || {
        for conn in l.incoming() {
            let Ok(conn) = conn else { return };
            std::thread::spawn(move || {
                conn.set_read_timeout(Some(Duration::from_secs(10)))
                    .unwrap();
                let mut w = conn.try_clone().unwrap();
                let mut r = BufReader::new(conn);
                loop {
                    let mut line = String::new();
                    if r.read_line(&mut line).unwrap_or(0) == 0 {
                        return;
                    }
                    let path = line.split(' ').nth(1).unwrap_or_default().to_owned();
                    let mut env = String::new();
                    loop {
                        let mut h = String::new();
                        if r.read_line(&mut h).unwrap_or(0) == 0 {
                            return;
                        }
                        let h = h.trim_end();
                        if h.is_empty() {
                            break;
                        }
                        if let Some((k, v)) = h.split_once(':')
                            && k.eq_ignore_ascii_case("x-env")
                        {
                            env = v.trim().to_owned();
                        }
                    }
                    let body = format!("backend {path} {env}");
                    let resp = format!(
                        "HTTP/1.1 200 OK\r\nContent-Length: {}\r\n\r\n{body}",
                        body.len()
                    );
                    if w.write_all(resp.as_bytes()).is_err() {
                        return;
                    }
                }
            });
        }
    });
    addr
}

/// Converts `src` with `akit adapt -from <from>` and starts `akit run` on the result; returns
/// where its one listener listens.
fn serve(d: &Path, from: &str, src: &str) -> (Akit, SocketAddr) {
    let input = d.join("source");
    std::fs::write(&input, src).unwrap();
    let config = d.join("fit.json");
    let out = Command::new(env!("CARGO_BIN_EXE_akit"))
        .args(["adapt", "-from", from, "-c"])
        .arg(&input)
        .arg("-o")
        .arg(&config)
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let mut child = Command::new(env!("CARGO_BIN_EXE_akit"))
        .arg("run")
        .arg("-c")
        .arg(&config)
        .stdout(Stdio::piped())
        .stderr(Stdio::inherit())
        .spawn()
        .unwrap();
    let stdout = child.stdout.take().unwrap();
    let akit = Akit(child);
    let mut line = String::new();
    BufReader::new(stdout).read_line(&mut line).unwrap();
    // akit: listener "http_127_0_0_1_0" on 127.0.0.1:PORT (http/1.1)
    assert!(line.starts_with("akit: listener "), "{line:?}");
    let addr = line.split(' ').nth(4).unwrap().parse().unwrap();
    (akit, addr)
}

/// `status body`, or for a redirect `status location`.
fn get(addr: SocketAddr, path: &str) -> String {
    let mut s = TcpStream::connect(addr).unwrap();
    s.set_read_timeout(Some(Duration::from_secs(10))).unwrap();
    write!(
        s,
        "GET {path} HTTP/1.1\r\nHost: 127.0.0.1\r\nConnection: close\r\n\r\n"
    )
    .unwrap();
    let mut resp = String::new();
    s.read_to_string(&mut resp).unwrap();
    let (head, body) = resp.split_once("\r\n\r\n").unwrap();
    let status = head.split(' ').nth(1).unwrap();
    if status.starts_with('3') {
        let location = head
            .lines()
            .find_map(|l| {
                l.split_once(':')
                    .filter(|(k, _)| k.eq_ignore_ascii_case("location"))
            })
            .map(|(_, v)| v.trim())
            .unwrap_or_default();
        return format!("{status} {location}");
    }
    format!("{status} {body}")
}

#[test]
fn a_converted_nginx_config_serves() {
    let d = dir("nginx");
    let root = d.join("www");
    std::fs::create_dir_all(&root).unwrap();
    std::fs::write(root.join("index.html"), "home").unwrap();
    let up = backend();
    let src = format!(
        r#"http {{
  server {{
    listen 127.0.0.1:0;
    root {};
    location = /health {{ return 200 "ok"; }}
    location /old {{ return 301 /new; }}
    location /api/ {{
      proxy_pass http://{up}/v1/;
      proxy_set_header X-Env test;
    }}
    location / {{ try_files $uri $uri/ /index.html; }}
  }}
}}"#,
        root.display()
    );
    let (_akit, addr) = serve(&d, "nginx", &src);
    for (path, want) in [
        ("/health", "200 ok"),
        ("/", "200 home"),
        ("/app/deep", "200 home"),
        ("/old", "301 /new"),
        ("/api/users/1", "200 backend /v1/users/1 test"),
    ] {
        assert_eq!(get(addr, path), want, "{path}");
    }
    drop(_akit);
    let _ = std::fs::remove_dir_all(&d);
}

#[test]
fn a_converted_caddyfile_serves() {
    let d = dir("caddyfile");
    let root = d.join("www");
    std::fs::create_dir_all(&root).unwrap();
    std::fs::write(root.join("index.html"), "home").unwrap();
    let up = backend();
    // fit's test takes the Admin API off the converted config; here the Caddyfile says so.
    let src = format!(
        r#"# comments are fine anywhere
{{
	admin off
}}
:0 {{
	bind 127.0.0.1
	root * {}
	respond /health "ok" # a trailing comment
	redir /old /new 301
	handle_path /api/* {{
		request_header X-Env test
		reverse_proxy http://{up}
	}}
	try_files {{path}} /index.html
	file_server
}}"#,
        root.display()
    );
    let (_akit, addr) = serve(&d, "caddyfile", &src);
    for (path, want) in [
        ("/health", "200 ok"),
        ("/", "200 home"),
        ("/app/deep", "200 home"),
        ("/old", "301 /new"),
        ("/api/users/1", "200 backend /users/1 test"),
    ] {
        assert_eq!(get(addr, path), want, "{path}");
    }
    drop(_akit);
    let _ = std::fs::remove_dir_all(&d);
}
