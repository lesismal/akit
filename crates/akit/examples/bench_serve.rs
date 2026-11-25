//! akit serving a config on the akio backend it is told, for `bench/modules.sh` (I56): the
//! path `akit run` takes (the config compiled with every module akit's binary registers, its
//! listeners on `akit_listener::engine`), with the backend chosen, which `akit run` does not do
//! yet (it serves on epoll). On the benchmark host only (rule 23).
//!
//! `bench_serve CONFIG.json [epoll|uring]`; serves until killed.
#![allow(clippy::print_stderr)]

use std::process::ExitCode;
use std::sync::Arc;

use akio::{Backend, UringConfig};
use akit_runtime::upstream::EngineSlot;
use akit_runtime::{Env, GraphCell, HandlerParams, Registry, compile};

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let (Some(path), backend) = (args.first(), args.get(1).map(String::as_str)) else {
        eprintln!("usage: bench_serve CONFIG.json [epoll|uring]");
        return ExitCode::from(2);
    };
    let json = match std::fs::read_to_string(path) {
        Ok(j) => j,
        Err(e) => {
            eprintln!("bench_serve: {path}: {e}");
            return ExitCode::FAILURE;
        }
    };
    let config = match akit_config::from_json(&json, None) {
        Ok(c) => c,
        Err(e) => {
            eprintln!("bench_serve: {path}: {e}");
            return ExitCode::FAILURE;
        }
    };
    let registry: Registry<HandlerParams> = match Registry::new(akit::STANDARD) {
        Ok(r) => r,
        Err(e) => {
            eprintln!("bench_serve: {e}");
            return ExitCode::FAILURE;
        }
    };
    let slot = EngineSlot::new();
    let graph = match compile(&config, &registry, &Env::new(slot.clone())) {
        Ok(g) => g,
        Err(e) => {
            eprintln!("bench_serve: {e}");
            return ExitCode::FAILURE;
        }
    };
    let cell = GraphCell::new(graph);
    let mut opts = akit_listener::Options::default();
    opts.backend = match backend {
        None | Some("epoll") => Backend::Epoll,
        Some("uring") => Backend::IoUring({
            let mut u = UringConfig::default();
            u.strict = true;
            u
        }),
        Some(b) => {
            eprintln!("bench_serve: unknown backend {b}");
            return ExitCode::from(2);
        }
    };
    let engine = match akit_listener::engine(&cell, &opts) {
        Ok(e) => Arc::new(e),
        Err(e) => {
            eprintln!("bench_serve: {e}");
            return ExitCode::FAILURE;
        }
    };
    if let Err(e) = engine.start() {
        eprintln!("bench_serve: listening: {e}");
        return ExitCode::FAILURE;
    }
    if !slot.set(Arc::clone(&engine)) {
        eprintln!("bench_serve: the engine slot was set twice");
        return ExitCode::FAILURE;
    }
    eprintln!(
        "bench_serve: serving {path} on {}",
        backend.unwrap_or("epoll")
    );
    loop {
        std::thread::park();
    }
}
