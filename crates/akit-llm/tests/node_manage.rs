//! fit's `llm/node` tests of the node's management, ported (I108: `manage_test.go`,
//! `gpu_test.go`): the methods as its pages call them, and as a hub does where the node allows
//! it, its edits written into its member by a control thread as `akit run`'s. Every tool a node
//! runs here is a stand-in on a directory of its own (`ollama`, `nvidia-smi`, `sh`), never a
//! real one: nothing is installed, no GPU is set.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing,
    clippy::arithmetic_side_effects
)]

mod support;

use std::io::{BufRead, BufReader, Write};
use std::net::TcpListener;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU16, Ordering};
use std::sync::{Arc, OnceLock};
use std::time::Duration;

use akit_llm::json;
use akit_llm::model::{
    CODE_BAD_REQUEST, CODE_FORBIDDEN, CODE_NOT_FOUND, CODE_RATE_LIMITED, DialReq, Identity,
    ROLE_NODE, ROLE_PROXY,
};
use support::{Roles, call, code, read_stream, until};

/// The stand-ins, for every thread of this test binary, set before any node starts: `ollama`
/// (`ps` sleeps), `nvidia-smi` answering the GPU queries (the hardware's counted in `calls`) and
/// the settings (a power limit of 999 refused), and `sh`, standing in for an install script's
/// shell.
fn tools() -> &'static PathBuf {
    static DIR: OnceLock<PathBuf> = OnceLock::new();
    DIR.get_or_init(|| {
        use std::os::unix::fs::PermissionsExt;
        // Under the build's own directory for tests, which goes with `target`.
        let dir = std::path::Path::new(env!("CARGO_TARGET_TMPDIR"))
            .join(format!("akit-llm-node-manage-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let calls = dir.join("calls");
        let nvidia = format!(
            r#"case "$*" in
*uuid*) echo "0, NVIDIA A10, GPU-a, 550.1, 24564, 100, 5, 40, 20.5, 150.00, 100.00, 150.00, 150.00, 210, 405, 1695, 6251, 30, P8, Enabled, Default" ;;
--query-gpu*) echo x >> '{}'; echo "0, NVIDIA A10, 24564, 100, 5, 40, 550.1, 20.5, 150.00" ;;
*"-pl 999"*) echo "refused"; exit 4 ;;
*) echo "set $*" ;;
esac"#,
            calls.display()
        );
        for (name, body) in [
            (
                "ollama",
                r#"case "$1" in ps) exec /bin/sleep 30 ;; *) echo "listed $1" ;; esac"#.to_owned(),
            ),
            ("nvidia-smi", nvidia),
            ("sh", r#"echo "would install""#.to_owned()),
        ] {
            let p = dir.join(name);
            std::fs::write(&p, format!("#!/bin/sh\n{body}\n")).unwrap();
            std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o755)).unwrap();
        }
        akit_llm::exec::tools_in(Some(dir.clone()));
        dir
    })
}

fn node_doc(extra: &str) -> String {
    node_with(r#"{"gpu_sudo": false}"#, extra)
}

fn node_with(settings: &str, extra: &str) -> String {
    let _ = tools();
    format!(
        r#"{{"llm": {{"node": {{"id": "node-0000000000000011", "name": "box",
          "settings": {settings}{extra}}}}}}}"#
    )
}

fn v(s: &str) -> akit_config::value::Value {
    json::parse(s.as_bytes()).unwrap()
}

/// Follows job `id` as the node's pages: its output and its end.
fn follow(l: &akit_llm::local::Local, id: i64) -> (String, Result<String, akit_llm::wire::Error>) {
    let st = l
        .open_stream("/node/job/follow", &format!(r#"{{"id":{id}}}"#))
        .unwrap();
    let (frames, end) = read_stream(&st);
    let out = frames
        .iter()
        .map(|f| json::get_str(&json::parse(f).unwrap(), "output"))
        .collect();
    (out, end.map(|b| String::from_utf8(b).unwrap()))
}

fn field(j: &str, k: &str) -> String {
    match v(j).get(k) {
        Some(akit_config::value::Value::String(s)) => s.clone(),
        Some(x) => json::write(x),
        None => String::new(),
    }
}

/// fit's `TestManageAccess`: a hub only where the node allows it, a proxy never, its pages
/// always; the setting followed as it changes.
#[test]
fn manage_access() {
    let _ = tools();
    let (roles, ports) = Roles::start_with(|p| {
        format!(
            r#"{{"llm": {{"hub": {{"id": "hub-0000000000000011",
            "listen": {{"address": "127.0.0.1:0", "node_password": "np"}}}},
          "node": {{"id": "node-0000000000000011", "name": "box",
            "listen": {{"address": "127.0.0.1:0", "proxy_password": "pp"}},
            "peers": [{{"address": "127.0.0.1:{}", "password": "np", "role": "hub"}}],
            "limits": {{"retry_interval": "200ms"}}, "settings": {{"gpu_sudo": false}}}}}}}}"#,
            p.hub
        )
    });
    let _ = tools();
    let (h, n) = (roles.local("llm.hub"), roles.local("llm.node"));
    until("the node to link", || {
        call(&h, "/hub/nodes", "")
            .unwrap()
            .contains(r#""online":true"#)
    });
    let list = r#"{"id":"node-0000000000000011","method":"/node/model/list"}"#;
    let r = call(&h, "/hub/node/call", list);
    assert_eq!(code(&r), CODE_FORBIDDEN);
    assert!(r.unwrap_err().to_string().contains("allow_hub_manage"));
    let st = h
        .open_stream(
            "/hub/node/stream",
            r#"{"id":"node-0000000000000011","method":"/node/job/follow","req":{"id":1}}"#,
        )
        .unwrap();
    assert!(matches!(
        read_stream(&st).1,
        Err(akit_llm::wire::Error::Remote {
            code: CODE_FORBIDDEN,
            ..
        })
    ));
    // A proxy never manages.
    let p = akit_llm::mesh::Mesh::new(
        akit_llm::mesh::Config {
            identity: Arc::new(|| Identity {
                role: ROLE_PROXY.to_owned(),
                id: "proxy-t".to_owned(),
                name: "p".to_owned(),
                version: akit_llm::mesh::version(),
                ..Identity::default()
            }),
            accept_roles: vec![ROLE_NODE.to_owned()],
            engine: Arc::clone(&roles.engine),
        },
        akit_llm::mesh::Router::new(),
    );
    p.dial(&DialReq {
        addr: format!("127.0.0.1:{}", ports.node),
        password: "pp".to_owned(),
        role: ROLE_NODE.to_owned(),
        ..DialReq::default()
    })
    .unwrap();
    until("the proxy to link", || p.online("node-0000000000000011"));
    let (tx, rx) = std::sync::mpsc::channel();
    p.call(
        "node-0000000000000011",
        "/node/model/list",
        "{}",
        Duration::from_secs(5),
        move |r| {
            let _ = tx.send(r);
        },
    );
    assert_eq!(
        code(&rx.recv_timeout(Duration::from_secs(10)).unwrap()),
        CODE_FORBIDDEN
    );
    p.stop();
    // Its pages get an empty list, never null.
    assert_eq!(call(&n, "/node/model/list", "").unwrap(), "[]");
    // Allowed: the hub manages it, the info says so.
    call(
        &n,
        "/node/settings/set",
        r#"{"allow_hub_manage":true,"gpu_sudo":false}"#,
    )
    .unwrap();
    assert_eq!(call(&h, "/hub/node/call", list).unwrap(), "[]");
    until("the hub to learn it", || {
        call(&h, "/hub/nodes", "")
            .unwrap()
            .contains(r#""hub_manage":true"#)
    });
    // A job of the node run and followed through the hub.
    let j = call(
        &h,
        "/hub/node/call",
        r#"{"id":"node-0000000000000011","method":"/node/action/run","req":{"id":"ollama_list"}}"#,
    )
    .unwrap();
    let st = h
        .open_stream(
            "/hub/node/stream",
            &format!(
                r#"{{"id":"node-0000000000000011","method":"/node/job/follow","req":{{"id":{}}}}}"#,
                field(&j, "id")
            ),
        )
        .unwrap();
    let (frames, end) = read_stream(&st);
    let out: String = frames
        .iter()
        .map(|f| json::get_str(&json::parse(f).unwrap(), "output"))
        .collect();
    assert!(out.contains("listed list"), "{out}");
    assert_eq!(
        field(&String::from_utf8(end.unwrap()).unwrap(), "status"),
        "done"
    );
}

/// fit's `TestJobsThroughMethods`.
#[test]
fn jobs_through_methods() {
    let roles = Roles::start(&node_doc(""));
    let n = roles.local("llm.node");
    for m in ["/node/action/list", "/node/job/list", "/node/gpu/catalog"] {
        call(&n, m, "").unwrap();
    }
    assert_eq!(
        code(&call(&n, "/node/action/run", r#"{"id":"rm_rf"}"#)),
        CODE_BAD_REQUEST
    );
    let j = call(&n, "/node/action/run", r#"{"id":"ollama_list"}"#).unwrap();
    assert_eq!(field(&j, "status"), "running");
    let id: i64 = field(&j, "id").parse().unwrap();
    let (out, end) = follow(&n, id);
    assert!(out.contains("listed list"), "{out}");
    assert_eq!(field(&end.unwrap(), "status"), "done");
    let g = call(&n, "/node/job/get", &format!(r#"{{"id":{id}}}"#)).unwrap();
    assert!(field(&g, "output").contains("listed"));
    assert_eq!(
        code(&call(&n, "/node/job/get", r#"{"id":99}"#)),
        CODE_NOT_FOUND
    );
    assert_eq!(
        code(&call(&n, "/node/job/cancel", r#"{"id":99}"#)),
        CODE_NOT_FOUND
    );
    assert!(matches!(
        follow(&n, 99).1,
        Err(akit_llm::wire::Error::Remote {
            code: CODE_NOT_FOUND,
            ..
        })
    ));
    // A running job canceled through the method.
    let long = call(&n, "/node/action/run", r#"{"id":"ollama_ps"}"#).unwrap();
    let lid = field(&long, "id");
    assert_eq!(
        call(&n, "/node/job/cancel", &format!(r#"{{"id":{lid}}}"#)).unwrap(),
        "{}"
    );
    assert_eq!(
        field(&follow(&n, lid.parse().unwrap()).1.unwrap(), "status"),
        "canceled"
    );
    // Past the bound of jobs running, one is refused as rate limited.
    let mut refused = false;
    let mut started = 0;
    for _ in 0..32 {
        let r = call(&n, "/node/action/run", r#"{"id":"ollama_ps"}"#);
        if code(&r) == CODE_RATE_LIMITED {
            refused = true;
            break;
        }
        r.unwrap();
        started += 1;
    }
    assert!(
        refused && started >= akit_llm::jobs::MAX_RUNNING,
        "{started}"
    );
    let info = call(&n, "/node/job/list", "").unwrap();
    assert!(info.matches(r#""status":"running""#).count() <= akit_llm::jobs::MAX_RUNNING);
    // The jobs running are counted in the node's info; those left run are canceled at stop.
    let jobs = v(&call(&n, "/mesh/status", "").unwrap());
    assert!(jobs.get("self").is_some());
}

/// fit's `TestProfile`.
#[test]
fn profile() {
    let roles = Roles::start(&node_doc(""));
    let n = roles.local("llm.node");
    for req in [
        r#"{"name":" "}"#.to_owned(),
        format!(r#"{{"name":"{}"}}"#, "x".repeat(65)),
        format!(r#"{{"name":"n","description":"{}"}}"#, "d".repeat(1025)),
    ] {
        assert_eq!(
            code(&call(&n, "/node/profile/set", &req)),
            CODE_BAD_REQUEST,
            "{req}"
        );
    }
    call(
        &n,
        "/node/profile/set",
        r#"{"name":" box2 ","description":"gpu box"}"#,
    )
    .unwrap();
    let m = roles.member("llm.node");
    assert!(
        m.contains(r#""name":"box2""#) && m.contains(r#""description":"gpu box""#),
        "{m}"
    );
    *roles.fail.lock().unwrap() = Some("read-only".to_owned());
    let r = call(&n, "/node/profile/set", r#"{"name":"box"}"#);
    assert!(r.unwrap_err().to_string().contains("read-only"));
}

/// fit's `TestSettings`.
#[test]
fn settings() {
    let roles = Roles::start(&node_doc(""));
    let n = roles.local("llm.node");
    for req in [
        r#"{"refresh_interval":"1s"}"#.to_owned(),
        r#"{"refresh_interval":"soon"}"#.to_owned(),
        r#"{"job_timeout":"1s"}"#.to_owned(),
        format!(r#"{{"gpu_data_dir":"{}"}}"#, "d".repeat(4097)),
    ] {
        assert_eq!(
            code(&call(&n, "/node/settings/set", &req)),
            CODE_BAD_REQUEST,
            "{req}"
        );
    }
    // The defaults and false values are left out of the member.
    let s = call(
        &n,
        "/node/settings/set",
        r#"{"gpu_sudo":true,"refresh_interval":"30s","job_timeout":"1h0m0s"}"#,
    )
    .unwrap();
    assert_eq!(
        s,
        r#"{"allow_hub_manage":false,"gpu_sudo":true,"refresh_interval":"30s","job_timeout":"1h0m0s"}"#
    );
    assert!(!roles.member("llm.node").contains("settings"));
    // Every other value is written.
    call(
        &n,
        "/node/settings/set",
        r#"{"allow_hub_manage":true,"refresh_interval":"10s","job_timeout":"20s","gpu_data_dir":"/data"}"#,
    )
    .unwrap();
    assert!(
        roles.member("llm.node").contains(
            r#""settings":{"allow_hub_manage":true,"gpu_data_dir":"/data","gpu_sudo":false,"job_timeout":"20s","refresh_interval":"10s"}"#
        ),
        "{}",
        roles.member("llm.node")
    );
    assert_eq!(
        call(&n, "/node/settings/get", "").unwrap(),
        r#"{"allow_hub_manage":true,"gpu_sudo":false,"refresh_interval":"10s","job_timeout":"20s","gpu_data_dir":"/data"}"#
    );
    *roles.fail.lock().unwrap() = Some("read-only".to_owned());
    assert!(call(&n, "/node/settings/set", "{}").is_err());
}

/// The bounds of the calls the node forwards, read and written.
#[test]
fn upstream_bounds() {
    let roles = Roles::start(&node_doc(""));
    let n = roles.local("llm.node");
    assert_eq!(
        call(&n, "/node/upstream/get", "").unwrap(),
        r#"{"connect_timeout":"10s","first_byte_timeout":"5m0s","idle_timeout":"5m0s","timeout":"30m0s","max_inflight":256,"max_body_bytes":33554432}"#
    );
    for req in [
        r#"{"connect_timeout":"-1s"}"#,
        r#"{"timeout":"soon"}"#,
        r#"{"max_inflight":-1}"#,
    ] {
        assert_eq!(
            code(&call(&n, "/node/upstream/set", req)),
            CODE_BAD_REQUEST,
            "{req}"
        );
    }
    let got = call(
        &n,
        "/node/upstream/set",
        r#"{"connect_timeout":"3s","timeout":"30m","max_inflight":8,"max_body_bytes":1048576}"#,
    )
    .unwrap();
    assert!(
        got.contains(r#""connect_timeout":"3s""#) && got.contains(r#""max_inflight":8"#),
        "{got}"
    );
    assert!(
        roles.member("llm.node").contains(
            r#""proxy":{"connect_timeout":"3s","max_body_bytes":1048576,"max_inflight":8}"#
        ),
        "{}",
        roles.member("llm.node")
    );
}

/// fit's `TestModelEdits`.
#[test]
fn model_edits() {
    let roles = Roles::start(&node_doc(""));
    let n = roles.local("llm.node");
    let save = |m: &str| call(&n, "/node/model/save", m);
    let models = || {
        let m = v(&roles.member("llm.node"));
        json::get_arr(&m, "models").to_vec()
    };
    assert_eq!(
        code(&save(
            r#"{"name":"m1","api":"nope","base_url":"http://127.0.0.1:1/v1"}"#
        )),
        CODE_BAD_REQUEST
    );
    let key = "sk-0123456789abcdef";
    let r = save(&format!(
        r#"{{"name":"m1","api":"openai","base_url":"http://127.0.0.1:1/v1/","api_key":"{key}"}}"#
    ))
    .unwrap();
    assert_eq!(field(&r, "api_key"), "sk-0****cdef");
    assert_eq!(field(&r, "base_url"), "http://127.0.0.1:1/v1");
    // The masked key sent back keeps the key; the rest is replaced.
    save(r#"{"name":"m1","api":"openai","base_url":"http://127.0.0.1:2/v1","api_key":"sk-0****cdef"}"#)
        .unwrap();
    let ms = models();
    assert_eq!(ms.len(), 1);
    assert_eq!(json::get_str(&ms[0], "api_key"), key);
    assert_eq!(json::get_str(&ms[0], "base_url"), "http://127.0.0.1:2/v1");
    assert!(
        call(&n, "/node/model/list", "")
            .unwrap()
            .contains("sk-0****cdef")
    );
    // The same name for another API: deleting needs the API.
    save(r#"{"name":"m1","api":"anthropic","base_url":"http://127.0.0.1:2"}"#).unwrap();
    let del = |r: &str| call(&n, "/node/model/delete", r);
    assert_eq!(code(&del(r#"{"name":"m1"}"#)), CODE_BAD_REQUEST);
    assert_eq!(code(&del(r#"{"name":"zz"}"#)), CODE_NOT_FOUND);
    del(r#"{"name":"m1","api":"anthropic"}"#).unwrap();
    assert_eq!(models().len(), 1);
    *roles.fail.lock().unwrap() = Some("read-only".to_owned());
    assert!(save(r#"{"name":"m2","api":"openai","base_url":"http://x"}"#).is_err());
    assert!(del(r#"{"name":"m1"}"#).is_err());
}

/// fit's `TestModelEdits`, past the bound of models.
#[test]
fn model_bound() {
    let ms: Vec<String> = (0..akit_config::roles::MAX_LLM_MODELS)
        .map(|i| format!(r#"{{"name":"m{i}","api":"openai","base_url":"http://x"}}"#))
        .collect();
    let roles = Roles::start(&node_doc(&format!(r#", "models": [{}]"#, ms.join(","))));
    let n = roles.local("llm.node");
    let r = call(
        &n,
        "/node/model/save",
        r#"{"name":"extra","api":"openai","base_url":"http://x"}"#,
    );
    assert_eq!(code(&r), CODE_BAD_REQUEST);
    assert!(r.unwrap_err().to_string().contains("at most 256 models"));
}

/// An upstream answering its model list with the status `status` holds.
fn status_upstream(status: Arc<AtomicU16>) -> u16 {
    let l = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = l.local_addr().unwrap().port();
    std::thread::spawn(move || {
        for s in l.incoming() {
            let Ok(s) = s else { break };
            let status = Arc::clone(&status);
            std::thread::spawn(move || {
                let mut w = s.try_clone().unwrap();
                let mut r = BufReader::new(s);
                loop {
                    let mut line = String::new();
                    if r.read_line(&mut line).unwrap_or(0) == 0 {
                        return;
                    }
                    if line.trim().is_empty() {
                        let body = "e".repeat(1000);
                        let _ = write!(
                            w,
                            "HTTP/1.1 {} X\r\nContent-Length: {}\r\n\r\n{body}",
                            status.load(Ordering::SeqCst),
                            body.len()
                        );
                    }
                }
            });
        }
    });
    port
}

/// fit's `TestCheckModel`.
#[test]
fn check_model() {
    let status = Arc::new(AtomicU16::new(401));
    let up = status_upstream(Arc::clone(&status));
    let roles = Roles::start(&node_doc(&format!(
        r#", "models": [{{"name": "m1", "api": "openai", "base_url": "http://127.0.0.1:{up}", "api_key": "k"}}]"#
    )));
    let n = roles.local("llm.node");
    let r = call(&n, "/node/model/check", r#"{"name":"m1"}"#).unwrap();
    assert_eq!(field(&r, "state"), "error");
    let e = field(&r, "error");
    assert!(e.contains("upstream HTTP 401") && e.len() <= 600, "{e}");
    status.store(404, Ordering::SeqCst);
    let r = call(&n, "/node/model/check", r#"{"name":"m1"}"#).unwrap();
    assert_eq!(field(&r, "state"), "ready", "{r}");
    assert_eq!(
        code(&call(&n, "/node/model/check", r#"{"name":"zz"}"#)),
        CODE_NOT_FOUND
    );
}

/// fit's `TestGPUMethods` and `TestGPUInstall`.
#[test]
fn gpu_methods() {
    let roles = Roles::start(&node_with(
        r#"{"gpu_sudo": false, "gpu_data_dir": "/tmp"}"#,
        "",
    ));
    let n = roles.local("llm.node");
    let st = call(&n, "/node/gpu/status", "").unwrap();
    let s = v(&st);
    let devs = json::get_arr(&s, "devices");
    assert_eq!(devs.len(), 1, "{st}");
    assert_eq!(json::get_str(&devs[0], "vendor"), "nvidia");
    // Cached, then read again when asked fresh.
    let again = call(&n, "/node/gpu/status", "").unwrap();
    assert_eq!(field(&again, "collected_at"), field(&st, "collected_at"));
    let fresh = call(&n, "/node/gpu/status", r#"{"fresh":true}"#).unwrap();
    assert_eq!(json::get_arr(&v(&fresh), "devices").len(), 1);
    for req in [
        r#"{"vendor":"amd","index":-1,"key":"power_limit","value":"100"}"#, // no AMD GPU
        r#"{"vendor":"nvidia","index":0,"key":"nope"}"#,
        r#"{"vendor":"nvidia","index":-1,"key":"power_limit","value":"lots"}"#,
    ] {
        assert_eq!(
            code(&call(&n, "/node/gpu/set", req)),
            CODE_BAD_REQUEST,
            "{req}"
        );
    }
    assert_eq!(
        code(&call(&n, "/node/gpu/install", r#"{"vendor":"nope"}"#)),
        CODE_BAD_REQUEST
    );
    let run = |req: &str| {
        let j = call(&n, "/node/gpu/set", req).unwrap();
        let (out, end) = follow(&n, field(&j, "id").parse().unwrap());
        (j, out, end.unwrap())
    };
    let (j, out, end) = run(r#"{"vendor":"nvidia","index":-1,"key":"power_limit","value":"150"}"#);
    assert_eq!(field(&end, "status"), "done");
    assert!(out.contains("set -i 0 -pl 150"), "{out}");
    assert_eq!(field(&j, "title"), "nvidia power_limit=150 · all GPUs");
    let (j, _, end) = run(r#"{"vendor":"nvidia","index":0,"key":"gpu_clock_reset"}"#);
    assert_eq!(field(&end, "status"), "done");
    assert_eq!(field(&j, "title"), "nvidia gpu_clock_reset · GPU 0");
    let (_, out, end) = run(r#"{"vendor":"nvidia","index":0,"key":"power_limit","value":"999"}"#);
    assert_eq!(
        (
            field(&end, "status").as_str(),
            field(&end, "exit_code").as_str()
        ),
        ("failed", "4")
    );
    assert!(out.contains("refused"));
    // An install, its shell a stand-in: nothing is installed.
    let vendor = akit_llm::gpu::VENDORS
        .iter()
        .find(|v| akit_llm::gpu::install_command(v.id).is_ok())
        .map(|v| v.id);
    if let Some(vendor) = vendor {
        let j = call(
            &n,
            "/node/gpu/install",
            &format!(r#"{{"vendor":"{vendor}"}}"#),
        )
        .unwrap();
        let (out, end) = follow(&n, field(&j, "id").parse().unwrap());
        assert_eq!(field(&end.unwrap(), "status"), "done");
        assert!(out.contains("would install"), "{out}");
    }
}

/// fit's `TestHardwareRefresh` and `TestCollectNvidia`: the hardware read at the start and every
/// `refresh_interval`, its GPUs in the node's info.
#[test]
fn hardware_refresh() {
    let roles = Roles::start(&node_with(
        r#"{"gpu_sudo": false, "refresh_interval": "5s"}"#,
        "",
    ));
    let calls = tools().join("calls");
    let count = || {
        std::fs::read_to_string(&calls)
            .unwrap_or_default()
            .matches('x')
            .count()
    };
    // The first read at the start; the other tests' nodes read theirs within a second or two,
    // then only this one's refresh, 5 s on, counts.
    until("the first read", || count() > 0);
    std::thread::sleep(Duration::from_secs(2));
    let start = count();
    let deadline = std::time::Instant::now() + Duration::from_secs(20);
    while count() < start + 1 {
        assert!(
            std::time::Instant::now() < deadline,
            "the hardware not read again"
        );
        std::thread::sleep(Duration::from_millis(100));
    }
    drop(roles);
}
