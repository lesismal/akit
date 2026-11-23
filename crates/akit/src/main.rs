#![forbid(unsafe_code)]
//! The `akit` command, with fit's commands, flags and exit codes: 0 when it worked, 1 when it did
//! not (a config with errors, a shutdown that ran out of time), 2 when it was used wrongly.
//!
//! - `run`: reads the config (`-c`, repeated, files and directories, merged), compiles it,
//!   serves it, and on SIGINT or SIGTERM shuts down gracefully, waiting up to
//!   `-shutdown-timeout` for requests in flight; a second signal exits at once. On SIGHUP it
//!   reads the same files again and, when they compile, swaps the new graph in and moves the
//!   listeners to its (I22); a broken config leaves the running one.
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
        usage: "not supported yet (I29)",
    },
    Spec {
        name: "watch-interval",
        bool: false,
        usage: "not supported yet (I29)",
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

/// Why a config could not be loaded.
enum Failure {
    /// The config's errors.
    Config(Errors),
    /// The module registry could not be made (a bug in akit, not in the config).
    Registry(String),
}

impl Failure {
    /// Prints the errors, as fit prints them; `name` names the files.
    fn print(&self, name: &str) {
        match self {
            Failure::Config(errs) => print_errors(name, errs),
            Failure::Registry(e) => eprintln!("akit: {e}"),
        }
    }
}

/// Reads and compiles the config at `paths`, its upstream pools dialing through the engine
/// `engine` will hold.
fn read(paths: &[String], engine: &EngineSlot) -> Result<(Config, Graph), Failure> {
    let config = akit_config::from_files(paths).map_err(Failure::Config)?;
    let registry: Registry<HandlerParams> =
        Registry::new(&[akit_handler::MODULES, akit_proxy::MODULES])
            .map_err(|e| Failure::Registry(e.to_string()))?;
    let graph = compile(&config, &registry, &Env::new(engine.clone())).map_err(Failure::Config)?;
    Ok((config, graph))
}

/// [`read`], the errors printed.
fn load(paths: &[String], engine: &EngineSlot) -> Option<(Config, Graph)> {
    read(paths, engine)
        .map_err(|f| f.print(&paths.join(", ")))
        .ok()
}

/// SIGHUP: reads the config at `paths` again and, when it compiles and its listeners can be
/// served, swaps it in; otherwise the running config stays. Says what happened as fit says it.
fn reload(paths: &[String], slot: &EngineSlot, listeners: &mut akit_listener::Listeners) {
    let name = paths.join(", ");
    println!("akit: reloading {name}");
    let graph = match read(paths, slot) {
        Ok((_, g)) => g,
        Err(f) => {
            eprintln!("akit: reload failed; the running config stays");
            f.print(&name);
            return;
        }
    };
    match listeners.reload(graph) {
        Ok(r) => {
            for l in &r.removed {
                println!("akit: listener {:?} on {} removed", l.name, l.address);
            }
            for l in &r.added {
                print_listener(l);
            }
            println!("akit: reloaded {name}");
        }
        Err(e) => eprintln!("akit: reload failed; the running config stays: {e}"),
    }
}

/// `akit: listener "web" on 127.0.0.1:8080 (http/1.1, h2c)`.
fn print_listener(l: &akit_listener::Served) {
    let protocols: Vec<&str> = l.protocols.iter().map(|p| p.as_str()).collect();
    println!(
        "akit: listener {:?} on {} ({})",
        l.name,
        l.address,
        protocols.join(", ")
    );
}

/// How often the control thread retires replaced graphs while some are still in use.
const COLLECT_EVERY: Duration = Duration::from_millis(100);

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
    let Some((_, graph)) = load(&paths, &slot) else {
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
    let mut listeners = akit_listener::Listeners::new(Arc::clone(&engine), cell.clone());
    for l in listeners.served() {
        print_listener(&l);
    }
    loop {
        // Graphs replaced by a reload are retired from here once no request uses them
        // (decision 004): after each swap, and on a timer while any is left.
        let signal = if cell.retiring() > 0 {
            match signals.recv_timeout(COLLECT_EVERY) {
                Ok(s) => Some(s),
                Err(mpsc::RecvTimeoutError::Timeout) => {
                    cell.collect();
                    continue;
                }
                Err(mpsc::RecvTimeoutError::Disconnected) => None,
            }
        } else {
            signals.recv().ok()
        };
        match signal {
            Some(Signal::Stop) | None => break,
            Some(Signal::Hangup) => {
                reload(&paths, &slot, &mut listeners);
                cell.collect();
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
    // The listeners added by reloads stop accepting; the engine's own stop with it.
    listeners.close();
    if !akit_listener::shutdown(&engine, grace) {
        eprintln!(
            "akit run: shutdown took longer than {}; connections still open were closed",
            flags::format_duration(grace)
        );
        return ExitCode::FAILURE;
    }
    drop(listeners);
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
