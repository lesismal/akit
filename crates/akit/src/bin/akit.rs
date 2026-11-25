#![forbid(unsafe_code)]
//! The `akit` command: akit's command line ([`akit::main`]) with akit's own modules.

fn main() -> std::process::ExitCode {
    akit::main(akit::VERSION, akit::STANDARD)
}
