#![forbid(unsafe_code)]
//! The `akit` command, with fit's commands, flags and exit codes: 0 when it worked, 1 when it did
//! not (a config with errors, a shutdown that ran out of time), 2 when it was used wrongly.
//!
//! - `run`: reads the config (`-c`, repeated, files and directories, merged), compiles it,
//!   serves it, and on SIGINT or SIGTERM shuts down gracefully, waiting up to
//!   `-shutdown-timeout` for requests in flight; a second signal exits at once.
//! - `validate`: reads and compiles the config, as `run` would, and says whether it is valid.
//! - `version`, `help`.

mod flags;

use std::io::Read;
use std::process::ExitCode;
use std::sync::{Arc, mpsc};
use std::time::Duration;

use akit_config::{Config, Errors};
use akit_runtime::upstream::EngineSlot;
use akit_runtime::{Env, Graph, GraphCell, HandlerParams, Registry, compile};
use flags::{Spec, Usage};

const USAGE: &str = "\
akit: a reverse proxy, load balancer and API gateway on akio

Usage:
  akit <command> [flags]

Commands:
  run        run a config
  validate   check a config, without running it
  version    print akit's version and the akio commit it is built on
  help       print this help

Run akit <command> -h for a command's flags.
";

const CONFIG_USAGE: &str = "a config file, or a directory of them, to use; give -c again for more, which are merged (fit.json when none is given). A file ending in .yaml or .yml is YAML, any other JSON (comments and trailing commas allowed); fit's .conf format comes later; a directory's files are those, in its subdirectories too";

const RUN_FLAGS: &[Spec] = &[
    Spec {
        name: "c",
        bool: false,
        usage: CONFIG_USAGE,
    },
    Spec {
        name: "shutdown-timeout",
        bool: false,
        usage: "how long shutdown waits for requests in flight before closing their connections (default 30s)",
    },
    // fit's, not supported yet: refused when given.
    Spec {
        name: "watch",
        bool: true,
        usage: "not supported yet (I22, I29)",
    },
    Spec {
        name: "watch-interval",
        bool: false,
        usage: "not supported yet (I22, I29)",
    },
    Spec {
        name: "watch-certs",
        bool: true,
        usage: "not supported yet (I30)",
    },
    Spec {
        name: "admin",
        bool: true,
        usage: "not supported yet (the Admin API, I26)",
    },
    Spec {
        name: "timeout",
        bool: false,
        usage: "not supported yet (the Admin API, I26)",
    },
];

const VALIDATE_FLAGS: &[Spec] = &[Spec {
    name: "c",
    bool: false,
    usage: CONFIG_USAGE,
}];

fn version() -> String {
    format!(
        "akit {}\nakio {}\n{}/{}",
        env!("CARGO_PKG_VERSION"),
        akit_runtime::AKIO_REV
            .get(..12)
            .unwrap_or(akit_runtime::AKIO_REV),
        std::env::consts::OS,
        std::env::consts::ARCH,
    )
}

/// Prints a config's errors as fit does: each with its file, or with `name`, the files' names,
/// when it has none.
fn print_errors(name: &str, errs: &Errors) {
    for e in errs.as_slice() {
        if e.file.is_some() {
            eprintln!("{e}");
        } else {
            eprintln!("{name}: {e}");
        }
    }
}

/// The config files the flags name, `fit.json` when none.
fn config_paths(flags: &[(&str, String)]) -> Vec<String> {
    let paths: Vec<String> = flags
        .iter()
        .filter(|(n, _)| *n == "c")
        .map(|(_, v)| v.clone())
        .collect();
    if paths.is_empty() {
        vec!["fit.json".to_owned()]
    } else {
        paths
    }
}

/// Reads and compiles the config at `paths`, its upstream pools dialing through the engine
/// `engine` will hold; the errors are printed, as fit prints them.
fn load(paths: &[String], engine: &EngineSlot) -> Option<(Config, Graph)> {
    let name = paths.join(", ");
    let config = match akit_config::from_files(paths) {
        Ok(c) => c,
        Err(errs) => {
            print_errors(&name, &errs);
            return None;
        }
    };
    let registry: Registry<HandlerParams> =
        match Registry::new(&[akit_handler::MODULES, akit_proxy::MODULES]) {
            Ok(r) => r,
            Err(e) => {
                eprintln!("akit: {e}");
                return None;
            }
        };
    match compile(&config, &registry, &Env::new(engine.clone())) {
        Ok(g) => Some((config, g)),
        Err(errs) => {
            print_errors(&name, &errs);
            None
        }
    }
}

fn usage_error(u: &Usage) -> ExitCode {
    eprintln!("{}", u.0);
    ExitCode::from(2)
}

/// `akit validate`: the config is read and compiled (every module provisioned, which checks its
/// params) and released.
fn validate(args: &[String]) -> ExitCode {
    let flags = match flags::parse("validate", args, VALIDATE_FLAGS) {
        Ok(f) => f,
        Err(u) => return usage_error(&u),
    };
    let paths = config_paths(&flags);
    // No engine: an upstream's pool is made, and never dials.
    match load(&paths, &EngineSlot::new()) {
        Some(_) => {
            println!("{}: valid", paths.join(", "));
            ExitCode::SUCCESS
        }
        None => ExitCode::FAILURE,
    }
}

/// What the signal thread passes on.
enum Signal {
    Stop,
    Hangup,
}

/// Delivers SIGINT, SIGTERM and SIGHUP on a channel, read from akio's signal pipe by a thread
/// of its own (the handlers only write a byte).
fn signals() -> std::io::Result<mpsc::Receiver<Signal>> {
    use akio_sys::process::{SIGHUP, SIGINT, SIGTERM, signal_pipe};
    let mut pipe = signal_pipe(&[SIGINT, SIGTERM, SIGHUP])?;
    let (tx, rx) = mpsc::channel();
    std::thread::Builder::new()
        .name("akit-signals".into())
        .spawn(move || {
            let mut b = [0u8; 1];
            while pipe.read_exact(&mut b).is_ok() {
                let sig = i32::from(b[0]);
                let s = if sig == SIGHUP {
                    Signal::Hangup
                } else {
                    Signal::Stop
                };
                if tx.send(s).is_err() {
                    break;
                }
            }
        })?;
    Ok(rx)
}

/// `akit run`.
fn run(args: &[String]) -> ExitCode {
    let flags = match flags::parse("run", args, RUN_FLAGS) {
        Ok(f) => f,
        Err(u) => return usage_error(&u),
    };
    let mut grace = Duration::from_secs(30);
    for (name, value) in &flags {
        match *name {
            "c" => {}
            "shutdown-timeout" => match flags::parse_duration(value) {
                Some(d) => grace = d,
                None => {
                    return usage_error(&Usage(format!(
                        "invalid value {value:?} for flag -shutdown-timeout: parse error\n{}",
                        flags::usage("run", RUN_FLAGS)
                    )));
                }
            },
            other => {
                eprintln!("akit run: not supported yet: -{other}");
                return ExitCode::from(2);
            }
        }
    }
    let paths = config_paths(&flags);
    // The pools are made with the graph, before the engine they dial through, which is built
    // from the graph's listeners: they reach it through this slot, filled once it is built.
    let slot = EngineSlot::new();
    let Some((config, graph)) = load(&paths, &slot) else {
        return ExitCode::FAILURE;
    };
    // Before the listeners: a signal from then on shuts down gracefully.
    let signals = match signals() {
        Ok(s) => s,
        Err(e) => {
            eprintln!("akit run: signals: {e}");
            return ExitCode::FAILURE;
        }
    };
    let cell = GraphCell::new(graph);
    let mut opts = akit_listener::Options::default();
    opts.shutdown_timeout = grace;
    let engine = match akit_listener::engine(&cell, &opts) {
        Ok(e) => Arc::new(e),
        Err(e) => {
            eprintln!("akit run: {e}");
            return ExitCode::FAILURE;
        }
    };
    if let Err(e) = engine.start() {
        eprintln!("akit run: listening: {e}");
        return ExitCode::FAILURE;
    }
    // Upstream connections are dialed on the listeners' threads.
    slot.set(Arc::clone(&engine));
    let addrs = akit_listener::addrs(&config.listeners, &engine);
    for (l, a) in config.listeners.iter().zip(addrs) {
        let protocols: Vec<&str> = l.protocols.iter().map(|p| p.as_str()).collect();
        // HTTP/3 listens on the port TCP has; one serving it alone, on its UDP address.
        let Some(addr) = a.addr() else { continue };
        println!(
            "akit: listener {:?} on {addr} ({})",
            l.name,
            protocols.join(", ")
        );
    }
    loop {
        match signals.recv() {
            Ok(Signal::Stop) | Err(_) => break,
            Ok(Signal::Hangup) => {
                eprintln!("akit: SIGHUP: reloading the config is not supported yet (I22)");
            }
        }
    }
    // A second SIGINT or SIGTERM does not wait for the shutdown, as fit's.
    std::thread::spawn(move || {
        while let Ok(s) = signals.recv() {
            if let Signal::Stop = s {
                eprintln!("akit run: stopped by a second signal");
                std::process::exit(1);
            }
        }
    });
    println!("akit: shutting down");
    if !akit_listener::shutdown(&engine, grace) {
        eprintln!(
            "akit run: shutdown took longer than {}; connections still open were closed",
            flags::format_duration(grace)
        );
        return ExitCode::FAILURE;
    }
    drop(engine);
    drop(cell);
    println!("akit: stopped");
    ExitCode::SUCCESS
}

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let Some((cmd, rest)) = args.split_first() else {
        eprint!("{USAGE}");
        return ExitCode::from(2);
    };
    match cmd.as_str() {
        "run" => run(rest),
        "validate" => validate(rest),
        "version" | "-V" | "--version" => {
            if let Err(u) = flags::parse("version", rest, &[]) {
                return usage_error(&u);
            }
            println!("{}", version());
            ExitCode::SUCCESS
        }
        "help" | "-h" | "-help" | "--help" => {
            print!("{USAGE}");
            ExitCode::SUCCESS
        }
        other => {
            eprint!("akit: unknown command {other:?}\n\n{USAGE}");
            ExitCode::from(2)
        }
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn version_names_akit_akio_and_the_platform() {
        let v = super::version();
        let lines: Vec<&str> = v.lines().collect();
        assert!(lines[0].starts_with("akit "));
        assert_eq!(lines[1], "akio 50a823155a06");
        assert_eq!(
            lines[2],
            format!("{}/{}", std::env::consts::OS, std::env::consts::ARCH)
        );
    }
}
