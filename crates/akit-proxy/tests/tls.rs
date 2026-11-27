//! The proxy behind a TLS listener: `X-Forwarded-Proto` and `Forwarded` say `https`. On the
//! backend `AKIO_TEST_BACKEND` names (epoll by default, `uring`).
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]

#[path = "../../akit-listener/tests/support/mod.rs"]
mod support;

use std::io::{BufRead, BufReader, Write};
use std::net::TcpListener;
use std::sync::Arc;
use std::thread;
use std::time::Duration;

use akio::{Backend, UringConfig};
use akit_runtime::upstream::EngineSlot;
use akit_runtime::{Env, GraphCell, HandlerParams, Registry, compile};
use support::certs::{Ca, dir};
use support::https::{client, get_h1};

fn options() -> akit_listener::Options {
    let mut o = akit_listener::Options::default();
    o.backend = Some(match std::env::var("AKIO_TEST_BACKEND").as_deref() {
        Ok("uring") => Backend::IoUring({
            let mut v = UringConfig::default();
            v.strict = true;
            v
        }),
        _ => Backend::Epoll,
    });
    o.shutdown_timeout = Duration::from_secs(5);
    o
}

#[test]
fn forwarded_proto_over_tls() {
    // An upstream that answers with the head it saw.
    let l = TcpListener::bind("127.0.0.1:0").unwrap();
    let up = l.local_addr().unwrap();
    thread::spawn(move || {
        for s in l.incoming() {
            let Ok(s) = s else { return };
            thread::spawn(move || {
                let mut w = s.try_clone().unwrap();
                let mut r = BufReader::new(s);
                loop {
                    let mut head = String::new();
                    loop {
                        let mut line = String::new();
                        if r.read_line(&mut line).unwrap_or(0) == 0 {
                            return;
                        }
                        head.push_str(&line);
                        if line == "\r\n" {
                            break;
                        }
                    }
                    let reply = format!(
                        "HTTP/1.1 200 OK\r\nContent-Length: {}\r\n\r\n{head}",
                        head.len()
                    );
                    if w.write_all(reply.as_bytes()).is_err() {
                        return;
                    }
                }
            });
        }
    });

    let d = dir("proxy-tls");
    let ca = Ca::new("akit proxy test ca");
    let ca_file = ca.write(&d, "ca.pem");
    let (cert, key) = ca.leaf(&d, "a", &["a.example"]);
    let json = format!(
        r#"{{
  "listeners": [{{"name": "s", "address": "127.0.0.1:0", "protocols": ["http/1.1"],
                  "tls": {{"certificates": [{{"cert_file": "{}", "key_file": "{}"}}]}}}}],
  "upstreams": [{{"name": "app", "addresses": ["{up}"]}}],
  "routes": [{{"handler": {{"module": "proxy", "params": {{"upstream": "app"}}}}}}]
}}"#,
        cert.display(),
        key.display()
    );
    let config = akit_config::from_json(&json, None).unwrap();
    let registry: Registry<HandlerParams> =
        Registry::new(&[akit_handler::MODULES, akit_proxy::MODULES]).unwrap();
    let slot = EngineSlot::new();
    let cell = GraphCell::new(compile(&config, &registry, &Env::new(slot.clone())).unwrap());
    let engine = Arc::new(akit_listener::engine(&cell, &options()).unwrap());
    engine.start().unwrap();
    assert!(slot.set(engine.clone()));
    let addr = *engine.local_addrs().first().unwrap();

    let seen = get_h1(
        addr,
        &client(&ca_file, &["http/1.1"], None),
        "a.example",
        "a.example",
        "/p",
    )
    .unwrap();
    assert_eq!(seen.status, "HTTP/1.1 200 OK");
    assert!(
        seen.body.contains("\r\nX-Forwarded-Proto: https\r\n")
            && seen
                .body
                .contains("\r\nForwarded: for=127.0.0.1;host=a.example;proto=https\r\n"),
        "{}",
        seen.body
    );
    akit_listener::shutdown(&engine, Duration::from_secs(5));
    let _ = std::fs::remove_dir_all(&d);
}
