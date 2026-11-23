#![forbid(unsafe_code)]
#![warn(clippy::unwrap_used)]
//! The akit config model: fit's config (decision 002), read strictly, with every problem
//! reported with its file, line, column and path, as fit reports it.
//!
//! I02 gives the model's first part ([`Config`]: `listeners`, `routes` and the `respond`
//! handler) and the JSON reader: JSON with `//` and `/* */` comments and trailing commas, as fit
//! reads it; an unknown field, a key given twice or a wrong type is an error. A field fit reads
//! and akit does not support yet is refused as "not supported yet", never read and ignored
//! ([`Error::is_not_supported_yet`]). The YAML and .conf readers come with I24 and I23.
//!
//! Reading a file blocks: this is control plane code, never called on an I/O thread
//! (rules.md rule 1).

mod decode;
mod error;
mod json;
mod model;

use std::path::Path;

pub use error::{Error, Errors, NOT_SUPPORTED_YET};
pub use json::MAX_DEPTH;
pub use model::{
    Config, DEFAULT_PROTOCOLS, Handler, Listener, Match, Protocol, Respond, Route, SCHEMA_VERSION,
};

/// The largest config document read, in bytes: fit's own configs are kilobytes; this bounds
/// what a mistaken path (a log file, a device) can make akit read.
pub const MAX_CONFIG_BYTES: usize = 64 << 20;

/// Reads a config written in JSON. `file`, when given, is named in every problem.
pub fn from_json(src: &str, file: Option<&str>) -> Result<Config, Errors> {
    let mut d = decode::Decoder::new(src, file);
    if src.len() > MAX_CONFIG_BYTES {
        d.error(
            0,
            "",
            format!("the config is larger than {MAX_CONFIG_BYTES} bytes"),
        );
        return Err(Errors(d.errors));
    }
    let node = match json::parse(src) {
        Ok(n) => n,
        Err(e) => {
            d.error(e.off, "", e.msg);
            return Err(Errors(d.errors));
        }
    };
    let config = model::config(&mut d, &node);
    match config {
        Some(c) if d.errors.is_empty() => Ok(c),
        _ => {
            if d.errors.is_empty() {
                d.error(node.off, "", "the config could not be read");
            }
            Err(Errors(d.errors))
        }
    }
}

/// Reads a JSON config from a file, at most [`MAX_CONFIG_BYTES`] long.
pub fn from_json_file(path: &Path) -> Result<Config, Errors> {
    let name = path.display().to_string();
    let fail = |msg: String| {
        Errors(vec![Error {
            file: Some(name.clone()),
            path: String::new(),
            line: 0,
            col: 0,
            msg,
        }])
    };
    let meta = std::fs::metadata(path).map_err(|e| fail(e.to_string()))?;
    if meta.len() > MAX_CONFIG_BYTES as u64 {
        return Err(fail(format!(
            "the config is larger than {MAX_CONFIG_BYTES} bytes"
        )));
    }
    let bytes = std::fs::read(path).map_err(|e| fail(e.to_string()))?;
    let src = String::from_utf8(bytes).map_err(|e| {
        fail(format!(
            "the config is not UTF-8 (at byte {})",
            e.utf8_error().valid_up_to()
        ))
    })?;
    from_json(&src, Some(&name))
}
