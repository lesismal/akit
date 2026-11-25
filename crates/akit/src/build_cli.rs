//! `akit build` (I70), as fit's `fit build`: an akit with more modules, or fewer, built into a
//! binary by [`crate::build`].

use std::path::{Path, PathBuf};
use std::process::ExitCode;

use crate::build::{self, Options, Source};

use super::flags::{self, Spec};

const BUILD_FLAGS: &[Spec] = &[
    Spec {
        name: "o",
        bool: false,
        usage: "the binary to write (default akit)",
    },
    Spec {
        name: "with",
        bool: false,
        usage: "a crate to build in, repeated for each: name, name@version (crates.io), name=dir for one whose source is in dir, or name=git+url#reference for one in a git repository. The crate's library exports MODULES at its root, which the binary registers",
    },
    Spec {
        name: "without",
        bool: false,
        usage: "leave out one of akit's own module crates, repeated for each: akit-handler, akit-middleware, akit-cache or akit-proxy",
    },
    Spec {
        name: "akit",
        bool: false,
        usage: "the akit to build on: a revision of github.com/lesismal/akit (a commit, or a tag v...), git+url#reference for another repository of it, or a directory holding its source; this akit's own source when left out",
    },
    Spec {
        name: "keep",
        bool: false,
        usage: "generate the package in this directory and keep it there",
    },
];

/// `akit build`: 0 when the binary was written, 1 when the build failed, 2 when used wrongly.
pub(crate) fn build(args: &[String]) -> ExitCode {
    let flags = match flags::parse("build", args, BUILD_FLAGS) {
        Ok(f) => f,
        Err(u) => return super::usage_error(&u),
    };
    let mut output = PathBuf::from("akit");
    let mut akit = None;
    let mut keep = None;
    let mut with = Vec::new();
    let mut without = Vec::new();
    for (name, value) in flags {
        match name {
            "o" => output = PathBuf::from(value),
            "akit" => akit = Some(value),
            "keep" => keep = Some(PathBuf::from(value)),
            "without" => without.push(value),
            _ => match build::parse_module(&value) {
                Ok(m) => with.push(m),
                Err(e) => {
                    eprintln!("akit build: {e}");
                    return ExitCode::from(2);
                }
            },
        }
    }
    let akit = match akit {
        Some(v) => build::parse_akit(&v),
        None => match own_akit() {
            Some(s) => s,
            None => {
                eprintln!(
                    "akit build: this akit does not know where its source is; name a revision of akit with -akit, or a directory of its source"
                );
                return ExitCode::from(2);
            }
        },
    };
    let mut o = Options::new(output, akit.clone());
    o.with = with;
    o.without = without;
    o.keep = keep;
    o.version = match &akit {
        Source::Git {
            reference: Some(r), ..
        } => format!("{r}+build"),
        _ => format!("{}-devel+build", crate::VERSION),
    };
    if let Err(e) = build::crates(&o.with, &o.without) {
        eprintln!("akit build: {e}");
        return ExitCode::from(2);
    }
    println!(
        "akit build: building {} on akit {}",
        o.output.display(),
        describe(&akit)
    );
    if let Err(e) = build::build(&o, &mut std::io::stderr()) {
        eprintln!("akit build: {e}");
        return ExitCode::FAILURE;
    }
    println!("akit build: wrote {}", o.output.display());
    ExitCode::SUCCESS
}

/// The akit this binary was built from: the directory of the source it was compiled from, while
/// it is there (a checkout, or the copy cargo fetched of akit's repository).
fn own_akit() -> Option<Source> {
    let krate = Path::new(env!("CARGO_MANIFEST_DIR"));
    krate
        .join("Cargo.toml")
        .is_file()
        .then(|| Source::Dir(krate.to_path_buf()))
}

/// `source`, as the build's first line names it.
fn describe(source: &Source) -> String {
    match source {
        Source::Dir(d) => format!("from {}", d.display()),
        Source::Git { url, reference } => match reference {
            Some(r) if url == build::AKIT_GIT => r.clone(),
            Some(r) => format!("{url}#{r}"),
            None => url.clone(),
        },
        Source::Registry(v) => v.clone(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sources_are_described() {
        assert_eq!(describe(&Source::Dir("/src".into())), "from /src");
        assert_eq!(describe(&build::parse_akit("v0.1.0")), "v0.1.0");
        assert_eq!(
            describe(&build::parse_akit("git+https://example.com/akit#main")),
            "https://example.com/akit#main"
        );
        assert_eq!(
            describe(&build::parse_akit("git+https://example.com/akit")),
            "https://example.com/akit"
        );
        // A test binary is compiled from this checkout, which is there.
        assert!(matches!(own_akit(), Some(Source::Dir(d)) if d.ends_with("crates/akit")));
    }
}
