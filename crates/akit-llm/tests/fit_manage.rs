//! A node's management across akit and fit (decision 038, I106b and I108): akit's hub manages
//! fit's node through `/hub/node/call` and `/hub/node/stream`. Its jobs run a stand-in `ollama`
//! put first on the node's PATH, never a real tool. Needs fit's binary:
//! `AKIT_FIT_BIN=/path/to/fit cargo test -p akit-llm --test fit_manage -- --ignored`.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing,
    clippy::arithmetic_side_effects
)]

mod support;

use std::path::PathBuf;
use std::process::{Child, Command, Stdio};

use support::{Roles, call, free_port, read_stream, until};

/// fit running a config, killed when dropped.
struct Fit(Child, PathBuf);

impl Drop for Fit {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
        let _ = std::fs::remove_dir_all(&self.1);
    }
}

/// The stand-in `ollama`, which says what it was asked: written once, before any test of this
/// binary starts a process (each calls this first), so that no child forked meanwhile holds it
/// open for writing; fit's node finds it first on its `PATH`, akit's node through
/// `exec::tools_in`.
fn stand_ins() -> &'static PathBuf {
    static BIN: std::sync::OnceLock<PathBuf> = std::sync::OnceLock::new();
    BIN.get_or_init(|| {
        use std::os::unix::fs::PermissionsExt;
        let bin = std::path::Path::new(env!("CARGO_TARGET_TMPDIR"))
            .join(format!("akit-llm-fit-manage-{}", std::process::id()));
        std::fs::create_dir_all(&bin).unwrap();
        let tool = bin.join("ollama");
        std::fs::write(&tool, "#!/bin/sh\necho \"listed $1\"\n").unwrap();
        std::fs::set_permissions(&tool, std::fs::Permissions::from_mode(0o755)).unwrap();
        bin
    })
}

/// A directory of its own for `tag`: fit's config and log.
fn dir(tag: &str) -> PathBuf {
    let dir = std::path::Path::new(env!("CARGO_TARGET_TMPDIR")).join(format!(
        "akit-llm-fit-manage-{tag}-{}-{}",
        std::process::id(),
        akit_roles::linktoken::unix_now()
    ));
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

fn fit(tag: &str, config: &str) -> Fit {
    let bin = std::env::var("AKIT_FIT_BIN").expect("AKIT_FIT_BIN: the path of fit's binary");
    let dir = dir(tag);
    let path = dir.join("fit.json");
    std::fs::write(&path, config).unwrap();
    let log = std::fs::File::create(dir.join("fit.log")).unwrap();
    let child = Command::new(bin)
        .args(["run", "-c"])
        .arg(&path)
        .env("PATH", format!("{}:/usr/bin:/bin", stand_ins().display()))
        .stdout(Stdio::null())
        .stderr(log)
        .spawn()
        .unwrap();
    Fit(child, dir)
}

fn json(s: &str) -> akit_config::value::Value {
    akit_llm::json::parse(s.as_bytes()).unwrap()
}

#[test]
#[ignore = "needs fit's binary in AKIT_FIT_BIN"]
fn akit_hub_manages_fit_node() {
    let _ = stand_ins();
    let hub_port = free_port();
    let roles = Roles::start(&format!(
        r#"{{"llm": {{"hub": {{"id": "hub-akit000000000011",
          "listen": {{"address": "127.0.0.1:{hub_port}", "node_password": "np"}}}}}}}}"#
    ));
    let node = fit(
        "node",
        &format!(
            r#"{{"llm": {{"node": {{"id": "node-fit0000000000011", "name": "box",
              "peers": [{{"address": "127.0.0.1:{hub_port}", "password": "np", "role": "hub"}}],
              "settings": {{"allow_hub_manage": true, "gpu_sudo": false}},
              "models": [{{"name": "m1", "api": "openai", "base_url": "http://127.0.0.1:1/v1", "api_key": "sk-0123456789abcdef"}}]}}}}}}"#
        ),
    );
    let hub = roles.local("llm.hub");
    let id = "node-fit0000000000011";
    let nc = |method: &str, req: &str| {
        call(
            &hub,
            "/hub/node/call",
            &format!(r#"{{"id":"{id}","method":"{method}","req":{req}}}"#),
        )
    };
    until("fit's node to link", || {
        call(&hub, "/hub/nodes", "")
            .unwrap()
            .contains(r#""online":true"#)
    });
    let list = nc("/node/model/list", "{}").unwrap();
    assert!(list.contains(r#""api_key":"sk-0****cdef""#), "{list}");

    // A job of fit's node, followed through akit's hub.
    let job = nc("/node/action/run", r#"{"id":"ollama_list"}"#).unwrap();
    let jid = akit_llm::json::get_int(&json(&job), "id");
    let st = hub
        .open_stream(
            "/hub/node/stream",
            &format!(r#"{{"id":"{id}","method":"/node/job/follow","req":{{"id":{jid}}}}}"#),
        )
        .unwrap();
    let (frames, end) = read_stream(&st);
    let out: String = frames
        .iter()
        .map(|f| akit_llm::json::get_str(&json(std::str::from_utf8(f).unwrap()), "output"))
        .collect();
    assert!(
        out.contains("$ ollama list") && out.contains("listed list"),
        "{out}"
    );
    let end = String::from_utf8(end.unwrap()).unwrap();
    assert_eq!(
        akit_llm::json::get_str(&json(&end), "status"),
        "done",
        "{end}"
    );
    let r = nc("/node/action/run", r#"{"id":"rm_rf"}"#);
    assert!(r.unwrap_err().to_string().contains("unknown action"));

    // A model saved through the hub lands in fit's running config.
    nc(
        "/node/model/save",
        r#"{"name":"m2","api":"openai","base_url":"http://127.0.0.1:2/v1"}"#,
    )
    .unwrap();
    let list = nc("/node/model/list", "{}").unwrap();
    assert!(list.contains(r#""name":"m2""#), "{list}");
    drop(node);
}

/// An HTTP/1.1 request to fit's console: the status, the `Set-Cookie`, the body.
fn console(port: u16, path: &str, cookie: &str, csrf: &str, body: &str) -> (u16, String, String) {
    use std::io::{Read, Write};
    let mut s = std::net::TcpStream::connect(("127.0.0.1", port)).unwrap();
    s.set_read_timeout(Some(std::time::Duration::from_secs(30)))
        .unwrap();
    write!(
        s,
        "POST {path} HTTP/1.1\r\nHost: 127.0.0.1\r\nContent-Type: application/json\r\nCookie: {cookie}\r\nX-Fit-Csrf: {csrf}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    )
    .unwrap();
    let mut out = String::new();
    s.read_to_string(&mut out).unwrap();
    let (head, body) = out.split_once("\r\n\r\n").unwrap_or((&out, ""));
    let status = head
        .split_whitespace()
        .nth(1)
        .and_then(|c| c.parse().ok())
        .unwrap_or(0);
    let cookie = head
        .lines()
        .find_map(|l| {
            l.to_ascii_lowercase().starts_with("set-cookie:").then(|| {
                l.split_once(':')
                    .unwrap()
                    .1
                    .trim()
                    .split(';')
                    .next()
                    .unwrap()
                    .to_owned()
            })
        })
        .unwrap_or_default();
    // A chunked body is read as it came: its JSON is between the sizes.
    let body = if head
        .to_ascii_lowercase()
        .contains("transfer-encoding: chunked")
    {
        let mut b = String::new();
        let mut rest = body;
        while let Some((size, r)) = rest.split_once("\r\n") {
            let n = usize::from_str_radix(size.trim(), 16).unwrap_or(0);
            if n == 0 {
                break;
            }
            b.push_str(&r[..n]);
            rest = &r[n + 2..];
        }
        b
    } else {
        body.to_owned()
    };
    (status, cookie, body)
}

#[test]
#[ignore = "needs fit's binary in AKIT_FIT_BIN"]
fn fit_hub_manages_akit_node() {
    // The stand-in `ollama` on akit's lookups, so the job runs no real tool.
    akit_llm::exec::tools_in(Some(stand_ins().clone()));
    let (hub_port, console_port) = (free_port(), free_port());
    // The bcrypt hash of "secret".
    let hash = "$2b$10$VF3oqQjY3UmazI3yLOspje5eihWk5sQc05lgEHVxOZSbpOgdQqDPa";
    let _hub = fit(
        "hub",
        &format!(
            r#"{{"llm": {{"hub": {{"id": "hub-fit0000000000012",
              "listen": {{"address": "127.0.0.1:{hub_port}", "node_password": "np"}},
              "console": {{"address": "127.0.0.1:{console_port}", "users": [{{"username": "admin", "password_hash": "{hash}"}}]}}}}}}}}"#
        ),
    );
    let roles = Roles::start(&format!(
        r#"{{"llm": {{"node": {{"id": "node-akit000000000012", "name": "abox",
          "peers": [{{"address": "127.0.0.1:{hub_port}", "password": "np", "role": "hub"}}],
          "limits": {{"retry_interval": "200ms"}},
          "settings": {{"allow_hub_manage": true, "gpu_sudo": false}},
          "models": [{{"name": "m1", "api": "openai", "base_url": "http://127.0.0.1:1/v1", "api_key": "sk-0123456789abcdef"}}]}}}}}}"#
    ));
    until("fit's console", || {
        std::net::TcpStream::connect(("127.0.0.1", console_port)).is_ok()
    });
    let (st, cookie, body) = console(
        console_port,
        "/api/login",
        "",
        "",
        r#"{"username":"admin","password":"secret"}"#,
    );
    assert_eq!(st, 200, "{body}");
    let csrf = akit_llm::json::get_str(&json(&body), "csrf");
    let rpc = |method: &str, req: &str| {
        console(
            console_port,
            &format!("/api/rpc{method}"),
            &cookie,
            &csrf,
            req,
        )
    };
    let id = "node-akit000000000012";
    until("akit's node to link", || {
        rpc("/hub/nodes", "{}").2.contains(r#""online":true"#)
    });
    let nc = |method: &str, req: &str| {
        rpc(
            "/hub/node/call",
            &format!(r#"{{"id":"{id}","method":"{method}","req":{req}}}"#),
        )
    };
    let (st, _, list) = nc("/node/model/list", "{}");
    assert_eq!(st, 200, "{list}");
    assert!(list.contains(r#""api_key":"sk-0****cdef""#), "{list}");
    let (st, _, job) = nc("/node/action/run", r#"{"id":"ollama_list"}"#);
    assert_eq!(st, 200, "{job}");
    let jid = akit_llm::json::get_int(&json(&job), "id");
    until("the job to end", || {
        let (_, _, j) = nc("/node/job/get", &format!(r#"{{"id":{jid}}}"#));
        akit_llm::json::get_str(&json(&j), "status") == "done"
    });
    let (_, _, j) = nc("/node/job/get", &format!(r#"{{"id":{jid}}}"#));
    assert!(
        akit_llm::json::get_str(&json(&j), "output").contains("listed list"),
        "{j}"
    );
    let (st, _, cat) = nc("/node/gpu/catalog", "{}");
    assert!(st == 200 && cat.contains(r#""id":"nvidia""#), "{cat}");
    // A model saved through fit's hub lands in akit's node's member.
    let (st, _, saved) = nc(
        "/node/model/save",
        r#"{"name":"m2","api":"openai","base_url":"http://127.0.0.1:2/v1"}"#,
    );
    assert_eq!(st, 200, "{saved}");
    assert!(roles.member("llm.node").contains(r#""name":"m2""#));
    let (st, _, e) = nc("/node/settings/set", "{}");
    assert_ne!(st, 200, "a pages-only method relayed: {e}");
}
