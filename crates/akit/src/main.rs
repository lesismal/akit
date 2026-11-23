#![forbid(unsafe_code)]
//! The `akit` command: `run` (I05), `version` and `help`. `validate`, several `-c`, directories,
//! signals and a graceful shutdown come with I06, with fit's flags.

use std::path::Path;
use std::process::ExitCode;

use akio::Backend;
use akit_config::Errors;
use akit_runtime::{Env, GraphCell, HandlerParams, Registry, compile};

const USAGE: &str = "\
akit: a reverse proxy, load balancer and API gateway on akio

Usage:
  akit <command> [flags]

Commands:
  run        run a config: akit run [-c fit.json]
  version    print akit's version and the akio commit it is built on
  help       print this help

Flags:
  -h, --help       print this help
  -V, --version    print the version
";

fn version() -> String {
    format!(
        "akit {} (akio {})",
        env!("CARGO_PKG_VERSION"),
        akit_runtime::AKIO_REV
            .get(..12)
            .unwrap_or(akit_runtime::AKIO_REV)
    )
}

/// Prints a config's errors as fit does: each with its file.
fn print_errors(file: &str, errs: &Errors) {
    for e in errs.as_slice() {
        if e.file.is_some() {
            eprintln!("{e}");
        } else {
            eprintln!("{file}: {e}");
        }
    }
}

/// The config files `run`'s flags name: `-c PATH` or `-c=PATH`; `fit.json` when none is given,
/// as fit's. Err is the exit code, the message printed.
fn config_paths(args: impl Iterator<Item = String>) -> Result<Vec<String>, ExitCode> {
    let mut paths = Vec::new();
    let mut args = args.peekable();
    while let Some(a) = args.next() {
        let flag = a.strip_prefix("--").or_else(|| a.strip_prefix('-'));
        match flag {
            Some("c") => match args.next() {
                Some(p) => paths.push(p),
                None => {
                    eprintln!("akit run: flag needs an argument: -c");
                    return Err(ExitCode::from(2));
                }
            },
            Some(f) if f.starts_with("c=") => paths.push(f.get(2..).unwrap_or("").to_owned()),
            Some(f) => {
                eprintln!("akit run: flag provided but not defined: -{f}");
                return Err(ExitCode::from(2));
            }
            None => {
                eprintln!("akit run: unexpected argument {a:?}");
                return Err(ExitCode::from(2));
            }
        }
    }
    if paths.is_empty() {
        paths.push("fit.json".to_owned());
    }
    Ok(paths)
}

/// `akit run`: reads the config, compiles it, and serves it until the process is stopped.
fn run(args: impl Iterator<Item = String>) -> ExitCode {
    let paths = match config_paths(args) {
        Ok(p) => p,
        Err(code) => return code,
    };
    let [path] = paths.as_slice() else {
        eprintln!("akit run: not supported yet: more than one -c (I06)");
        return ExitCode::from(2);
    };
    let config = match akit_config::from_json_file(Path::new(path)) {
        Ok(c) => c,
        Err(errs) => {
            print_errors(path, &errs);
            return ExitCode::FAILURE;
        }
    };
    let registry: Registry<HandlerParams> = match Registry::new(&[akit_handler::MODULES]) {
        Ok(r) => r,
        Err(e) => {
            eprintln!("akit run: {e}");
            return ExitCode::FAILURE;
        }
    };
    let graph = match compile(&config, &registry, &Env::default()) {
        Ok(g) => g,
        Err(errs) => {
            print_errors(path, &errs);
            return ExitCode::FAILURE;
        }
    };
    let cell = GraphCell::new(graph);
    let engine = match akit_listener::engine(&cell, &Backend::default()) {
        Ok(e) => e,
        Err(e) => {
            eprintln!("akit run: {e}");
            return ExitCode::FAILURE;
        }
    };
    if let Err(e) = engine.start() {
        eprintln!("akit run: listening: {e}");
        return ExitCode::FAILURE;
    }
    for (l, addr) in config.listeners.iter().zip(engine.local_addrs()) {
        let protocols: Vec<&str> = l.protocols.iter().map(|p| p.as_str()).collect();
        println!(
            "akit: listener {:?} on {addr} ({})",
            l.name,
            protocols.join(", ")
        );
    }
    engine.wait();
    ExitCode::SUCCESS
}

fn main() -> ExitCode {
    let mut args = std::env::args().skip(1);
    match args.next().as_deref() {
        Some("run") => run(args),
        Some("version" | "-V" | "--version") => {
            println!("{}", version());
            ExitCode::SUCCESS
        }
        Some("help" | "-h" | "--help") | None => {
            print!("{USAGE}");
            ExitCode::SUCCESS
        }
        Some(other) => {
            eprintln!("akit: unknown command {other:?}\n\n{USAGE}");
            ExitCode::from(2)
        }
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn version_names_akit_and_akio() {
        let v = super::version();
        assert!(v.starts_with("akit "));
        assert!(v.contains("(akio 50a823155a06)"));
    }
}
