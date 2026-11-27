//! fit's `llm/hub` tests of the hub's calls for its pages, ported (I106b): `/hub/nodes`,
//! `/hub/node/call` and `/hub/node/stream` relayed to a node standing in for one (a mesh of the
//! node's role with a few of its methods), the hub's settings and name written back into its
//! member, `/mesh/status` with the routes' counts, and the node list's bounds.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing,
    clippy::arithmetic_side_effects
)]

mod support;

use std::sync::Arc;
use std::time::Duration;

use akit_llm::mesh::{self, Mesh, Router};
use akit_llm::model::{
    CODE_BAD_REQUEST, CODE_FORBIDDEN, CODE_UNAVAILABLE, DialReq, Identity, METHOD_NODE_INFO,
    ROLE_HUB, ROLE_NODE, ROLE_PROXY,
};
use akit_llm::wire::Error;
use support::{Roles, call, code, read_stream, until};

/// A node of `id` that allows management (or not): its info, its model list and a job's
/// follow (`"one "`, `"two "`, then the job done).
fn fake_node(e: &Arc<akio::Engine>, id: &str, manage: bool) -> Mesh {
    let mut r = Router::new();
    let name = format!("{id}-name");
    let n2 = name.clone();
    let id2 = id.to_owned();
    r.handle(
        METHOD_NODE_INFO,
        &[ROLE_HUB, ROLE_PROXY],
        Duration::ZERO,
        move |_, _, a| {
            a.ok(format!(
                r#"{{"id":"{id2}","name":"{n2}","hw":{{"arch":"amd64","disks":[{{"mount":"/"}}],"ips":["10.0.0.1"]}},"models":[{{"name":"m1","api":"openai","state":"ready","inflight":0}}],"inflight":0,"jobs":0,"hub_manage":{manage},"seq":1,"updated_at":1}}"#
            ));
        },
    );
    r.handle(
        "/node/model/list",
        &[ROLE_HUB],
        Duration::ZERO,
        move |_, _, a| {
            if manage {
                a.ok(
                    r#"[{"name":"m1","api":"openai","base_url":"http://x","api_key":"sk-…h1"}]"#
                        .into(),
                );
            } else {
                a.err(Error::code(
                    CODE_FORBIDDEN,
                    "this node does not allow hub management (allow_hub_manage)",
                ));
            }
        },
    );
    r.handle_stream("/node/job/follow", &[ROLE_HUB], |_, req, st| {
        let v = akit_llm::json::parse(req).unwrap();
        let id = akit_llm::json::get_int(&v, "id");
        if id != 7 {
            return st.end(Err(Error::code(3000, format!("no job {id}"))));
        }
        for out in ["one ", "two "] {
            assert_eq!(
                st.try_send(
                    format!(r#"{{"id":7,"status":"running","output":"{out}"}}"#).as_bytes()
                ),
                Ok(true)
            );
        }
        st.end(Ok(br#"{"id":7,"status":"done"}"#.to_vec()));
    });
    let id = id.to_owned();
    let m = Mesh::new(
        mesh::Config {
            identity: Arc::new(move || Identity {
                role: ROLE_NODE.to_owned(),
                id: id.clone(),
                name: name.clone(),
                version: mesh::version(),
                ..Identity::default()
            }),
            accept_roles: vec![ROLE_HUB.to_owned(), ROLE_PROXY.to_owned()],
            engine: Arc::clone(e),
        },
        r,
    );
    m.set_limits(mesh::Limits {
        retry: Duration::from_millis(200),
        ..mesh::Limits::default()
    });
    m
}

/// A hub, its listener on port 0.
const HUB_DOC: &str = r#"{"llm": {"hub": {"id": "hub-0000000000000001", "name": "h",
          "listen": {"address": "127.0.0.1:0", "node_password": "np", "proxy_password": "pp"}}}}"#;

fn link(m: &Mesh, port: u16) {
    m.dial(&DialReq {
        addr: format!("127.0.0.1:{port}"),
        password: "np".to_owned(),
        role: ROLE_HUB.to_owned(),
        ..DialReq::default()
    })
    .unwrap();
}

/// fit's `TestNodeCallForwarding` and `TestNodeStreamForwarding`.
#[test]
fn node_calls_and_streams_relayed() {
    let roles = Roles::start(HUB_DOC);
    let port = roles.mesh_port("llm.hub");
    let hub = roles.local("llm.hub");
    // fit's `TestNodeListSeqAndPace`: the list's sequence changes only when it changed.
    let seq =
        |s: &str| akit_llm::json::get_int(&akit_llm::json::parse(s.as_bytes()).unwrap(), "seq");
    let a = call(&hub, "/hub/node/list", "").unwrap();
    let b = call(&hub, "/hub/node/list", "").unwrap();
    assert_eq!(a, b);
    assert!(a.contains(r#""nodes":[],"total":0"#), "{a}");
    let n1 = fake_node(&roles.engine, "n1", true);
    let n2 = fake_node(&roles.engine, "n2", false);
    link(&n1, port);
    link(&n2, port);
    until("both nodes listed", || {
        let v = call(&hub, "/hub/nodes", "").unwrap();
        v.matches(r#""online":true"#).count() == 2
    });
    let nc = |id: &str, method: &str| {
        call(
            &hub,
            "/hub/node/call",
            &format!(r#"{{"id":"{id}","method":"{method}","req":{{}}}}"#),
        )
    };
    let b = nc("n1", "/node/model/list").unwrap();
    assert!(b.contains(r#""sk-…h1""#), "{b}");
    let r = nc("n2", "/node/model/list");
    assert_eq!(code(&r), CODE_FORBIDDEN);
    assert!(r.unwrap_err().to_string().contains(
        "node n2-name (n2) does not allow hub management (enable allow_hub_manage on the node)"
    ));
    assert_eq!(code(&nc("n1", "/node/settings/set")), CODE_BAD_REQUEST);
    assert_eq!(code(&nc("n1", "/node/job/follow")), CODE_BAD_REQUEST);
    let r = nc("nx", "/node/model/list");
    assert_eq!(code(&r), CODE_UNAVAILABLE);
    assert!(r.unwrap_err().to_string().contains("node nx is offline"));
    // A peer may not call the pages' methods.
    let (tx, rx) = std::sync::mpsc::channel();
    n1.call(
        "hub-0000000000000001",
        "/hub/node/call",
        r#"{"id":"n1","method":"/node/model/list"}"#,
        Duration::from_secs(5),
        move |r| {
            let _ = tx.send(r);
        },
    );
    assert_eq!(
        code(&rx.recv_timeout(Duration::from_secs(10)).unwrap()),
        CODE_FORBIDDEN
    );

    // A job followed through the hub's stream.
    let st = hub
        .open_stream(
            "/hub/node/stream",
            r#"{"id":"n1","method":"/node/job/follow","req":{"id":7}}"#,
        )
        .unwrap();
    let (frames, end) = read_stream(&st);
    let out: String = frames
        .iter()
        .map(|f| akit_llm::json::get_str(&akit_llm::json::parse(f).unwrap(), "output"))
        .collect();
    assert_eq!(out, "one two ");
    assert_eq!(end.unwrap(), br#"{"id":7,"status":"done"}"#);
    for (id, method, want) in [
        (
            "n1",
            "/node/model/list",
            "not a node management stream method",
        ),
        ("nx", "/node/job/follow", "offline"),
        ("n1", "/node/job/follow", "no job 0"), // no request: {} is sent, and the node fails it
    ] {
        let st = hub
            .open_stream(
                "/hub/node/stream",
                &format!(r#"{{"id":"{id}","method":"{method}"}}"#),
            )
            .unwrap();
        let (_, end) = read_stream(&st);
        let e = end.unwrap_err().to_string();
        assert!(e.contains(want), "{id} {method}: {e}");
    }

    let c = call(&hub, "/hub/node/list", "").unwrap();
    assert!(seq(&c) > seq(&a), "{c}");
    assert!(
        c.contains(r#""total":2"#) && c.contains(r#""disks":null,"ips":null"#),
        "{c}"
    );

    // The pages' list of the nodes: the info, the links.
    let nodes = call(&hub, "/hub/nodes", "").unwrap();
    assert!(
        nodes.starts_with(r#"[{"info":{"id":"n1","name":"n1-name""#),
        "{nodes}"
    );
    assert!(
        nodes.contains(r#""conns":[{"role":"node","id":"n1""#),
        "{nodes}"
    );
    // The proxies' list: no disks and addresses.
    let status = call(&hub, "/mesh/status", "").unwrap();
    assert!(status.starts_with(r#"{"self":{"role":"hub","id":"hub-0000000000000001""#));
    assert!(status.contains(r#""routes":[]"#), "{status}");
    n1.stop();
    n2.stop();
}

/// fit's `TestRole`: the hub's name and settings written back into its member, a failed edit
/// answered with its reason.
#[test]
fn settings_and_profile_written_back() {
    let roles = Roles::start(HUB_DOC);
    let hub = roles.local("llm.hub");
    assert_eq!(
        call(&hub, "/hub/settings/get", "").unwrap(),
        r#"{"max_nodes":1024}"#
    );
    assert_eq!(
        call(&hub, "/hub/settings/set", r#"{"max_nodes":8}"#).unwrap(),
        r#"{"max_nodes":8}"#
    );
    assert!(roles.member("llm.hub").contains(r#""max_nodes":8"#));
    // The default is left out of the member.
    call(&hub, "/hub/settings/set", r#"{"max_nodes":1024}"#).unwrap();
    assert!(!roles.member("llm.hub").contains("max_nodes"));
    assert_eq!(
        code(&call(&hub, "/hub/settings/set", r#"{"max_nodes":-1}"#)),
        CODE_BAD_REQUEST
    );
    assert_eq!(
        code(&call(&hub, "/hub/settings/set", r#"{"max_nodes":"x"}"#)),
        CODE_BAD_REQUEST
    );
    let id = call(&hub, "/hub/profile/set", r#"{"name":" new "}"#).unwrap();
    assert!(
        id.contains(r#""id":"hub-0000000000000001","name":"new""#),
        "{id}"
    );
    assert!(roles.member("llm.hub").contains(r#""name":"new""#));
    let long = format!(r#"{{"name":"{}"}}"#, "x".repeat(65));
    let r = call(&hub, "/hub/profile/set", &long);
    assert!(r.unwrap_err().to_string().contains("1-64"));
    *roles.fail.lock().unwrap() = Some("read-only".to_owned());
    let r = call(&hub, "/hub/profile/set", r#"{"name":"ok"}"#);
    assert!(r.unwrap_err().to_string().contains("read-only"));
}

/// The mesh listener's certificate files reported to `akit run -watch-certs` (`Role::files`).
#[test]
fn listener_files_reported() {
    let dir = std::path::Path::new(env!("CARGO_TARGET_TMPDIR"))
        .join(format!("akit-llm-hub-files-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let (c, k) = akit_llm::tls::self_signed(&["localhost".to_owned()]).unwrap();
    let (cf, kf) = (dir.join("c.pem"), dir.join("k.pem"));
    std::fs::write(&cf, &c).unwrap();
    std::fs::write(&kf, &k).unwrap();
    let roles = Roles::start(&format!(
        r#"{{"llm": {{"hub": {{"id": "hub-0000000000000003",
          "listen": {{"address": "127.0.0.1:0", "node_password": "np",
            "cert_file": "{}", "key_file": "{}"}}}}}}}}"#,
        cf.display(),
        kf.display()
    ));
    let files = roles.roles.lock().unwrap().files();
    let paths: Vec<&str> = files.iter().map(|(p, _)| p.as_str()).collect();
    assert!(
        paths.contains(&cf.to_str().unwrap()) && paths.contains(&kf.to_str().unwrap()),
        "{paths:?}"
    );
    drop(roles);
    let _ = std::fs::remove_dir_all(&dir);
}
