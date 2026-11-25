//! Fuzz entry points (feature `fuzzing`; not a public API): the `fuzz/` cargo-fuzz project calls
//! them, and the tests below replay `fuzz/corpus/<target>` on stable. fit's `FuzzLoad`
//! (`config/fuzz_test.go`), `FuzzAdapt` (`adapter/fitconf`, `adapter/yaml`) and `FuzzConvert`
//! (`adapter/nginx`, `adapter/caddyfile`).
#![allow(
    clippy::indexing_slicing,
    clippy::arithmetic_side_effects,
    clippy::unwrap_used,
    clippy::panic,
    clippy::expect_used
)]

use crate::adapt::{self, Target};
use crate::convert::Converted;
use crate::{Format, Source, value};

/// `config_json`: reading a JSON config never panics, whatever the bytes; a config that reads is
/// read the same way again from its own source.
pub fn config_json(data: &[u8]) {
    let Ok(src) = std::str::from_utf8(data) else {
        return;
    };
    assert_eq!(crate::from_json(src, None), crate::from_json(src, None));
}

/// `config_conf`: reading a `.conf` config never panics, whatever the bytes (the lexer, the
/// parser, `def` and `use`, sites and the conversion by fit's field types, then the decoder).
pub fn config_conf(data: &[u8]) {
    let Ok(src) = std::str::from_utf8(data) else {
        return;
    };
    assert_eq!(crate::from_conf(src, None), crate::from_conf(src, None));
}

/// `config_yaml`: reading a YAML config never panics, whatever the bytes, and stays within its
/// bounds on aliases and merge keys.
pub fn config_yaml(data: &[u8]) {
    let Ok(src) = std::str::from_utf8(data) else {
        return;
    };
    assert_eq!(crate::from_yaml(src, None), crate::from_yaml(src, None));
}

/// `config_value`: the Admin API's JSON (I26). Standardizing never panics, keeps the length and
/// yields what parses to the same value; a value written out parses back to itself.
pub fn config_value(data: &[u8]) {
    let Ok(src) = std::str::from_utf8(data) else {
        return;
    };
    let Ok(v) = value::parse(src) else {
        assert!(value::standardize(src).is_err());
        return;
    };
    let std = value::standardize(src).unwrap();
    assert_eq!(std.len(), src.len());
    assert_eq!(value::parse(&std).unwrap(), v);
    assert_eq!(value::parse(&v.to_json()).unwrap(), v);
}

/// What any converter's result must be: JSON, warnings that say their line, and a config akit
/// reads without a panic, its errors placed by the converter's positions.
fn check_converted(r: &Converted) {
    if let Err(e) = value::parse(r.json()) {
        panic!("not JSON ({e:?}): {}", r.json());
    }
    for w in r.warnings() {
        assert!(w.starts_with("line "), "a warning with no line: {w:?}");
    }
    if let Err(errs) = crate::from_json(r.json(), None) {
        let _ = r.map_errors(errs);
    }
}

/// `convert_nginx`: no nginx config, however broken, makes the converter panic; what it
/// converts is JSON, with warnings that say their line.
pub fn convert_nginx(data: &[u8]) {
    let Ok(src) = std::str::from_utf8(data) else {
        return;
    };
    if let Ok(r) = crate::convert::nginx(src) {
        check_converted(&r);
    }
}

/// `convert_caddyfile`: the same for Caddyfiles, `{$NAME}` placeholders given `NAME`'s own
/// name as their value.
pub fn convert_caddyfile(data: &[u8]) {
    let Ok(src) = std::str::from_utf8(data) else {
        return;
    };
    let env = |name: &str| (name.len() < 16).then(|| name.to_ascii_lowercase());
    if let Ok(r) = crate::convert::caddyfile(src, &env) {
        check_converted(&r);
    }
}

/// `config_adapt`: `akit adapt` (I73). `data[0]` picks the source's format (JSON, YAML, `.conf`);
/// a document read from it writes in every format without a panic, and, when the source is a
/// config akit reads, what is written reads as the same config.
pub fn config_adapt(data: &[u8]) {
    let Some((&f, rest)) = data.split_first() else {
        return;
    };
    let Ok(src) = std::str::from_utf8(rest) else {
        return;
    };
    // `.conf`'s `${NAME}` reads the environment: kept out, so that a run does not depend on it.
    let format = match f % 3 {
        0 => Format::Json,
        1 => Format::Yaml,
        _ if src.contains("${") => return,
        _ => Format::Conf,
    };
    let read = |format: Format, src: &str| match format {
        Format::Yaml => crate::from_yaml(src, None),
        Format::Conf => crate::from_conf(src, None),
        _ => crate::from_json(src, None),
    };
    let Ok(doc) = adapt::document(&[Source {
        name: None,
        format,
        src,
    }]) else {
        return;
    };
    let config = read(format, src);
    for (to, as_format) in [
        (Target::Json, Format::Json),
        (Target::Yaml, Format::Yaml),
        (Target::Conf, Format::Conf),
    ] {
        let Ok(out) = doc.write(to) else {
            continue;
        };
        let again = read(as_format, &out);
        if let Ok(c) = &config {
            match again {
                Ok(a) => assert!(
                    a == *c,
                    "{format:?} written as {to:?} reads as another config:\n{out}"
                ),
                Err(e) => panic!("{format:?} written as {to:?} does not read ({e}):\n{out}"),
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Runs `f` on every file of `fuzz/corpus/<target>`.
    fn replay(target: &str, f: fn(&[u8])) {
        let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../fuzz/corpus")
            .join(target);
        let mut n = 0;
        for e in std::fs::read_dir(&dir).unwrap() {
            f(&std::fs::read(e.unwrap().path()).unwrap());
            n += 1;
        }
        assert!(n > 0, "{} is empty", dir.display());
    }

    #[test]
    fn corpus_config_json() {
        replay("config_json", config_json);
    }

    #[test]
    fn corpus_config_conf() {
        replay("config_conf", config_conf);
    }

    #[test]
    fn corpus_config_yaml() {
        replay("config_yaml", config_yaml);
    }

    #[test]
    fn corpus_config_value() {
        replay("config_value", config_value);
    }

    #[test]
    fn corpus_convert_nginx() {
        replay("convert_nginx", convert_nginx);
    }

    #[test]
    fn corpus_convert_caddyfile() {
        replay("convert_caddyfile", convert_caddyfile);
    }

    #[test]
    fn corpus_config_adapt() {
        replay("config_adapt", config_adapt);
    }
}
