//! `akit adapt` and `akit fmt` (I73): a config converted between fit's formats, `.conf`, YAML
//! and JSON, and a config's files written again in their own formats, each as fit's `fit adapt`
//! writes it (`akit_config::adapt`); and an nginx config or a Caddyfile converted to one of
//! fit's formats (I74, `akit_config::convert`), as `fit adapt -from nginx` and `-from caddyfile`
//! convert them, what could not be converted listed as warnings.
//!
//! The config is read and checked as `akit validate` reads it, without compiling it; a field akit
//! does not run yet ("not supported yet") is converted as written, and named in a warning, so that
//! every config fit reads converts.

use std::io::{Read, Write};
use std::path::Path;
use std::process::ExitCode;

use akit_config::adapt::{Target, document, has_comments};
use akit_config::{ConfigFile, Error, Errors, Format, MAX_CONFIG_BYTES, Source};

use crate::flags::{self, Spec};

pub(crate) const ADAPT_USAGE: &str = "\
Usage: akit adapt [flags]

Converts a config between fit's formats, .conf, YAML and native JSON, checked as akit validate
reads it, and writes it as fit adapt writes it, and converts an nginx config (-from nginx) or a
Caddyfile (-from caddyfile), of which a common subset converts, to help moving to akit: akit run
loads only fit's formats. What could not be converted is listed on standard error. Several files,
or directories of them, are merged into one config, as akit run merges them. A field akit does
not run yet is converted as written, with a warning.

Flags:

	-c path      a config file or a directory of them, or - for standard
	             input; give -c again for more; fit.json when left out
	-from name   the format of a single file or of standard input: json,
	             or an adapter's (caddyfile, fit, nginx, yaml); by the file's
	             extension when left out, json for standard input
	-to name     what to write: json, yaml or fit; json when left out
	-o file      where to write it; standard output when left out
";

pub(crate) const FMT_USAGE: &str = "\
Usage: akit fmt [flags]

Writes each config file again in its own format, as akit adapt writes that format: a .conf file
as fit writes one, YAML and JSON with two spaces, each file ending with one line break. The files are checked together first, as akit
validate reads them. Comments are not kept: with -w, a file that holds comments is left as it is
and named on standard error.

Flags:

	-c path   a config file or a directory of them; give -c again for more;
	          fit.json when left out
	-w        write each file in place rather than to standard output
	-l        list the files whose formatting differs, and write nothing
";

const ADAPT_FLAGS: &[Spec] = &[
    Spec {
        name: "c",
        bool: false,
        usage: "a config file or a directory of them, or - for standard input",
    },
    Spec {
        name: "from",
        bool: false,
        usage: "the format of a single file or of standard input: json, caddyfile, fit, nginx or yaml",
    },
    Spec {
        name: "to",
        bool: false,
        usage: "what to write: json, yaml or fit (default json)",
    },
    Spec {
        name: "o",
        bool: false,
        usage: "where to write it (default standard output)",
    },
];

const FMT_FLAGS: &[Spec] = &[
    Spec {
        name: "c",
        bool: false,
        usage: "a config file or a directory of them",
    },
    Spec {
        name: "w",
        bool: true,
        usage: "write each file in place rather than to standard output",
    },
    Spec {
        name: "l",
        bool: true,
        usage: "list the files whose formatting differs, and write nothing",
    },
];

/// The flags, or the usage error's exit; `-h` prints the command's usage.
fn parse_flags(
    cmd: &str,
    args: &[String],
    specs: &[Spec],
    usage: &str,
) -> Result<Vec<(&'static str, String)>, ExitCode> {
    flags::parse(cmd, args, specs).map_err(|u| {
        if u.0.starts_with("Usage of") {
            eprint!("{usage}");
        } else if let Some((first, _)) = u.0.split_once('\n') {
            eprintln!("{first}");
            eprint!("{usage}");
        } else {
            eprintln!("{}", u.0);
        }
        ExitCode::from(2)
    })
}

fn last<'a>(flags: &'a [(&str, String)], name: &str) -> Option<&'a str> {
    flags
        .iter()
        .rev()
        .find(|(n, _)| *n == name)
        .map(|(_, v)| v.as_str())
}

/// What `-from` names: one of fit's formats, or another program's config to convert.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Input {
    Format(Format),
    Nginx,
    Caddyfile,
}

/// The format `-from` names.
fn from_format(name: &str) -> Result<Input, String> {
    match name {
        "" | "json" => Ok(Input::Format(Format::Json)),
        "yaml" => Ok(Input::Format(Format::Yaml)),
        "fit" => Ok(Input::Format(Format::Conf)),
        "nginx" => Ok(Input::Nginx),
        "caddyfile" => Ok(Input::Caddyfile),
        _ => Err(format!(
            "there is no adapter {name:?} (there are caddyfile, fit, nginx, yaml)"
        )),
    }
}

/// The files `files` as sources, each in `format`, or in the format its extension says.
fn sources(files: &[ConfigFile], format: Option<Format>) -> Result<Vec<Source<'_>>, Errors> {
    files
        .iter()
        .map(|f| {
            let src = std::str::from_utf8(&f.data).map_err(|e| {
                Errors(vec![Error {
                    file: Some(f.path.clone()),
                    path: String::new(),
                    line: 0,
                    col: 0,
                    msg: format!("the config is not UTF-8 (at byte {})", e.valid_up_to()),
                }])
            })?;
            Ok(Source {
                name: Some(&f.path),
                format: format.unwrap_or_else(|| Format::of(Path::new(&f.path))),
                src,
            })
        })
        .collect()
}

/// Checks the config `srcs` hold as `akit validate` reads it: its problems are printed and
/// false returned; fields akit does not run yet are printed as warnings.
fn check(cmd: &str, name: &str, srcs: &[Source<'_>]) -> bool {
    check_mapped(cmd, name, srcs, &|e| e)
}

/// [`check`], each problem placed by `map` first.
fn check_mapped(
    cmd: &str,
    name: &str,
    srcs: &[Source<'_>],
    map: &dyn Fn(Errors) -> Errors,
) -> bool {
    let Err(errs) = akit_config::from_sources(srcs) else {
        return true;
    };
    let errs = map(errs);
    let only_unsupported = errs.as_slice().iter().all(Error::is_not_supported_yet);
    for e in errs.as_slice() {
        let line = named(name, e);
        if only_unsupported {
            eprintln!("akit {cmd}: warning: {line}; converted as written");
        } else {
            eprintln!("{line}");
        }
    }
    only_unsupported
}

/// A problem as fit prints it: with its own file, or with the config's name, `name:line:col: `
/// when it has a position and `name: ` when not.
fn named(name: &str, e: &Error) -> String {
    if e.file.is_some() {
        e.to_string()
    } else if e.line > 0 {
        format!("{name}:{e}")
    } else {
        format!("{name}: {e}")
    }
}

fn print_errors(name: &str, errs: &Errors) {
    for e in errs.as_slice() {
        eprintln!("{}", named(name, e));
    }
}

/// `akit adapt`.
pub(crate) fn adapt(args: &[String]) -> ExitCode {
    let flags = match parse_flags("adapt", args, ADAPT_FLAGS, ADAPT_USAGE) {
        Ok(f) => f,
        Err(code) => return code,
    };
    let from = last(&flags, "from").unwrap_or_default();
    let to_name = last(&flags, "to").unwrap_or("json");
    let Some(to) = Target::named(to_name) else {
        eprintln!("akit adapt: -to {to_name:?}: json, yaml or fit");
        return ExitCode::from(2);
    };
    let paths = crate::config_paths(&flags);
    let name = paths.join(", ");
    if paths.len() > 1 && paths.iter().any(|p| p == "-") {
        eprintln!("akit adapt: -c - reads standard input alone");
        return ExitCode::from(2);
    }
    if paths.len() > 1 && !from.is_empty() {
        eprintln!(
            "akit adapt: -from is for a single file; several take their formats from their extensions"
        );
        return ExitCode::from(2);
    }
    let format = if from.is_empty() && paths != ["-"] {
        None
    } else {
        match from_format(from) {
            Ok(Input::Format(f)) => Some(f),
            Ok(other) => return convert(other, &paths, &name, to, last(&flags, "o")),
            Err(e) => {
                eprintln!("akit adapt: {e}");
                return ExitCode::FAILURE;
            }
        }
    };
    let files = if paths == ["-"] {
        let data = match read_stdin() {
            Ok(d) => d,
            Err(e) => {
                eprintln!("akit adapt: standard input: {e}");
                return ExitCode::FAILURE;
            }
        };
        vec![ConfigFile {
            path: "-".into(),
            data,
        }]
    } else {
        match akit_config::read_config_files(&paths) {
            Ok(f) if format.is_some() && f.len() > 1 => {
                eprintln!(
                    "akit adapt: -from is for a single file; several take their formats from their extensions"
                );
                return ExitCode::from(2);
            }
            Ok(f) => f,
            Err(errs) => {
                print_errors(&name, &errs);
                return ExitCode::FAILURE;
            }
        }
    };
    let srcs = match sources(&files, format) {
        Ok(s) => s,
        Err(errs) => {
            print_errors(&name, &errs);
            return ExitCode::FAILURE;
        }
    };
    if !check("adapt", &name, &srcs) {
        return ExitCode::FAILURE;
    }
    let out = match document(&srcs).map(|d| d.write(to)) {
        Ok(Ok(out)) => out,
        Ok(Err(e)) => {
            eprintln!("akit adapt: {e}");
            return ExitCode::FAILURE;
        }
        Err(errs) => {
            print_errors(&name, &errs);
            return ExitCode::FAILURE;
        }
    };
    write_out(last(&flags, "o"), &out)
}

/// Standard input, at most [`MAX_CONFIG_BYTES`].
fn read_stdin() -> std::io::Result<Vec<u8>> {
    let mut data = Vec::new();
    let cap = u64::try_from(MAX_CONFIG_BYTES).unwrap_or(u64::MAX);
    std::io::stdin().take(cap).read_to_end(&mut data)?;
    Ok(data)
}

/// Writes `out` to the file `o` names, or to standard output.
fn write_out(o: Option<&str>, out: &str) -> ExitCode {
    let written = match o {
        None | Some("") => std::io::stdout().write_all(out.as_bytes()),
        Some(path) => std::fs::write(path, out.as_bytes()),
    };
    if let Err(e) = written {
        eprintln!("akit adapt: {e}");
        return ExitCode::FAILURE;
    }
    ExitCode::SUCCESS
}

/// `akit adapt -from nginx` or `-from caddyfile`: the single file (or standard input) converted
/// to fit's native JSON, as fit's converters convert it, checked as `akit validate` reads it,
/// its problems placed at the source's lines; what the converter could not convert printed as
/// warnings; and the config written as `to`.
fn convert(from: Input, paths: &[String], name: &str, to: Target, o: Option<&str>) -> ExitCode {
    let data = if paths == ["-"] {
        match read_stdin() {
            Ok(d) => d,
            Err(e) => {
                eprintln!("akit adapt: standard input: {e}");
                return ExitCode::FAILURE;
            }
        }
    } else {
        // The file as it is: an nginx config's include, or a Caddyfile's import of a file, is
        // not followed (and not converted).
        match akit_config::read_config_file(Path::new(name)) {
            Ok(f) => f.data,
            Err(errs) => {
                print_errors(name, &errs);
                return ExitCode::FAILURE;
            }
        }
    };
    let src = match std::str::from_utf8(&data) {
        Ok(s) => s,
        Err(e) => {
            eprintln!(
                "{name}: the config is not UTF-8 (at byte {})",
                e.valid_up_to()
            );
            return ExitCode::FAILURE;
        }
    };
    let converted = match from {
        Input::Nginx => akit_config::convert::nginx(src),
        Input::Caddyfile => akit_config::convert::caddyfile(src, &|v| std::env::var(v).ok()),
        Input::Format(_) => return ExitCode::FAILURE,
    };
    let converted = match converted {
        Ok(c) => c,
        Err(e) => {
            eprintln!("{name}: {e}");
            return ExitCode::FAILURE;
        }
    };
    let srcs = [Source {
        name: None,
        format: Format::Json,
        src: converted.json(),
    }];
    if !check_mapped("adapt", name, &srcs, &|e| converted.map_errors(e)) {
        return ExitCode::FAILURE;
    }
    for w in converted.warnings() {
        eprintln!("{name}: warning: {w}");
    }
    let out = match document(&srcs).map(|d| d.write(to)) {
        Ok(Ok(out)) => out,
        Ok(Err(e)) => {
            eprintln!("akit adapt: {e}");
            return ExitCode::FAILURE;
        }
        Err(errs) => {
            print_errors(name, &errs);
            return ExitCode::FAILURE;
        }
    };
    write_out(o, &out)
}

/// `akit fmt`.
pub(crate) fn fmt(args: &[String]) -> ExitCode {
    let flags = match parse_flags("fmt", args, FMT_FLAGS, FMT_USAGE) {
        Ok(f) => f,
        Err(code) => return code,
    };
    let in_place = last(&flags, "w") == Some("true");
    let list = last(&flags, "l") == Some("true");
    let paths = crate::config_paths(&flags);
    let name = paths.join(", ");
    let files = match akit_config::read_config_files(&paths) {
        Ok(f) => f,
        Err(errs) => {
            print_errors(&name, &errs);
            return ExitCode::FAILURE;
        }
    };
    let srcs = match sources(&files, None) {
        Ok(s) => s,
        Err(errs) => {
            print_errors(&name, &errs);
            return ExitCode::FAILURE;
        }
    };
    if !check("fmt", &name, &srcs) {
        return ExitCode::FAILURE;
    }
    let mut ok = true;
    let mut stdout = std::io::stdout().lock();
    for s in &srcs {
        let file = s.name.unwrap_or_default();
        let out = match document(std::slice::from_ref(s)).map(|d| d.write(Target::of(s.format))) {
            // What follows a JSON file's value, which akit adapt keeps as fit does, is dropped:
            // a file ends with one line break, and formatting it again changes nothing.
            Ok(Ok(out)) if s.format == Format::Json => format!("{}\n", out.trim_end()),
            Ok(Ok(out)) => out,
            Ok(Err(e)) => {
                eprintln!("akit fmt: {file}: {e}");
                ok = false;
                continue;
            }
            Err(errs) => {
                print_errors(file, &errs);
                ok = false;
                continue;
            }
        };
        let written = if list {
            if out == s.src {
                Ok(())
            } else {
                writeln!(stdout, "{file}")
            }
        } else if in_place {
            if out == s.src {
                Ok(())
            } else if has_comments(s.src, s.format) {
                eprintln!(
                    "akit fmt: {file}: holds comments, which akit fmt does not keep: left as it is"
                );
                ok = false;
                Ok(())
            } else {
                std::fs::write(file, out.as_bytes())
            }
        } else {
            stdout.write_all(out.as_bytes())
        };
        if let Err(e) = written {
            eprintln!("akit fmt: {file}: {e}");
            ok = false;
        }
    }
    if ok {
        ExitCode::SUCCESS
    } else {
        ExitCode::FAILURE
    }
}
