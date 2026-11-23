#![forbid(unsafe_code)]
//! The `akit` command. I01 gives it `version` and `help`; `run` and `validate` come with I05
//! and I06, with fit's flags.

use std::process::ExitCode;

const USAGE: &str = "\
akit: a reverse proxy, load balancer and API gateway on akio

Usage:
  akit <command> [flags]

Commands:
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

fn main() -> ExitCode {
    let mut args = std::env::args().skip(1);
    match args.next().as_deref() {
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
