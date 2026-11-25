//! nginx configs and Caddyfiles converted to fit's native JSON (I74), as fit's `fit adapt
//! -from nginx` and `-from caddyfile` convert them: the common subset each converter knows, and
//! for everything else a warning naming its line ([`Converted::warnings`]). What they write is
//! byte for byte what fit's `adapter/nginx` and `adapter/caddyfile` write: Go's `json.Marshal`
//! of the same values, warnings in the same words, Go's regular expressions' errors quoted as
//! Go writes them.
//!
//! Converting is control plane code: it reads a whole file, never on an I/O thread. Its input
//! is bounded as fit bounds it (directives, tokens, nesting, imports of snippets, the routes a
//! site makes).
// Offsets and counts below are bounded by the input's length.
#![allow(clippy::arithmetic_side_effects)]

mod caddyfile;
mod goregexp;
mod gostd;
mod nginx;
mod nginx_parse;
#[cfg(test)]
mod tests;

use std::collections::HashMap;

use crate::error::{Error, Errors};

/// A config converted to fit's native JSON.
#[derive(Debug, Clone)]
pub struct Converted {
    json: String,
    warnings: Vec<String>,
    /// The line each listener, upstream and route (and some of their parts) came from, by
    /// their paths in the JSON, as config errors name them.
    positions: HashMap<String, i64>,
}

impl Converted {
    /// The config in fit's native JSON, compact, as Go's `json.Marshal` writes it.
    #[must_use]
    pub fn json(&self) -> &str {
        &self.json
    }

    /// What could not be converted, or was converted with doubts, each `line N: ...`, in the
    /// order of the source's lines.
    #[must_use]
    pub fn warnings(&self) -> &[String] {
        &self.warnings
    }

    /// The line in the source the value at `path` (as `routes[2]`) was written at, if known.
    #[must_use]
    pub fn line(&self, path: &str) -> Option<u32> {
        self.positions
            .get(path)
            .filter(|l| **l > 0)
            .and_then(|l| u32::try_from(*l).ok())
    }

    /// `errs`, about the JSON, each placed where the source wrote what it is about: at the line
    /// of the nearest value on its path whose line is known (column 1), or nowhere, as fit's
    /// `Result.MapErrors`.
    #[must_use]
    pub fn map_errors(&self, errs: Errors) -> Errors {
        Errors(
            errs.0
                .into_iter()
                .map(|mut e| {
                    e.line = 0;
                    e.col = 0;
                    let mut p = e.path.clone();
                    loop {
                        if let Some(line) = self.line(&p) {
                            e.line = line;
                            e.col = 1;
                            break;
                        }
                        if p.is_empty() {
                            break;
                        }
                        p = parent(&p).to_owned();
                    }
                    e
                })
                .collect::<Vec<Error>>(),
        )
    }
}

/// The path of what holds the value at `path`: `a.b[2]` for `a.b[2].c`, `a.b` for `a.b[2]`, `a`
/// for `a["k"]`, "" for `a`.
fn parent(path: &str) -> &str {
    if path.ends_with("\"]") {
        if let Some(i) = path.rfind("[\"") {
            return path.get(..i).unwrap_or_default();
        }
    } else if path.ends_with(']')
        && let Some(i) = path.rfind('[')
    {
        return path.get(..i).unwrap_or_default();
    }
    path.rfind('.')
        .and_then(|i| path.get(..i))
        .unwrap_or_default()
}

/// Converts an nginx config, as fit's `adapter/nginx`.
///
/// # Errors
///
/// Why the config cannot be read (`line N: ...`), or that it has no server to convert.
pub fn nginx(src: &str) -> Result<Converted, String> {
    nginx::convert(src)
}

/// Converts a Caddyfile, as fit's `adapter/caddyfile`; `env` gives the environment's values for
/// the Caddyfile's `{$NAME}` placeholders.
///
/// # Errors
///
/// Why the Caddyfile cannot be read (`line N: ...`), or that none of its sites converts.
pub fn caddyfile(src: &str, env: &dyn Fn(&str) -> Option<String>) -> Result<Converted, String> {
    caddyfile::convert(src, env)
}
