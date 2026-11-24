//! The configs applied, numbered, kept as written: the history the Admin API reads, and the one
//! step every config takes to the running server, from the files (SIGHUP) or the API.

use std::collections::VecDeque;
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::SystemTime;

use akio_crypto::hash::Sha256;
use akit_config::Errors;

/// A config read from the files (at start and on SIGHUP).
pub const SOURCE_FILE: &str = "file";
/// A config loaded through the Admin API.
pub const SOURCE_ADMIN: &str = "admin";

/// A config that was applied.
#[derive(Debug)]
pub struct Version {
    /// Its number: 1 for the config akit started with, one more for each config applied since.
    pub number: u64,
    /// When it was applied.
    pub applied: SystemTime,
    /// Where it came from: [`SOURCE_FILE`] or [`SOURCE_ADMIN`].
    pub source: &'static str,
    /// A strong entity tag of it as written ([`etag_of`]), which `If-Match` compares.
    pub etag: String,
    data: String,
}

impl Version {
    /// The config as it was written: standard JSON (comments and trailing commas blanked out).
    #[must_use]
    pub fn data(&self) -> &str {
        &self.data
    }
}

/// A listener a config opened or closed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ListenerChange {
    /// Its name.
    pub name: String,
    /// Where it listens.
    pub address: String,
    /// Its protocols.
    pub protocols: Vec<String>,
}

/// What applying a config changed, as the caller's apply step reports it.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Applied {
    /// The listeners opened, the changed ones included.
    pub added: Vec<ListenerChange>,
    /// The listeners closed, the changed ones included.
    pub removed: Vec<ListenerChange>,
    /// Changes in the config that take effect only when akit restarts.
    pub warnings: Vec<String>,
}

/// Why the apply step refused a config; the running config stays.
#[derive(Debug)]
pub enum ApplyError {
    /// The config has errors (it does not load or does not compile): the API answers 400.
    Config(Errors),
    /// It could not be served (a listener's address is in use, ...): the API answers 500.
    Failed(String),
}

/// Why [`Configs::update`] applied nothing.
#[derive(Debug)]
pub enum Refused {
    /// `If-Match` names a config that is no longer the running one: the API answers 412.
    Changed,
    /// The apply step refused it.
    Apply(ApplyError),
}

/// A config applied.
#[derive(Debug, Clone)]
pub struct Outcome {
    /// Its version.
    pub version: u64,
    /// Its ETag.
    pub etag: String,
    /// Where it came from.
    pub source: &'static str,
    /// What it changed.
    pub applied: Applied,
}

#[derive(Debug)]
struct Inner {
    keep: usize,
    /// Oldest first; the last is the running config. Held for a push or a clone only: the API's
    /// I/O thread reads it.
    history: Mutex<VecDeque<Arc<Version>>>,
    /// Held for a whole [`Configs::update`], so that configs are applied one after another and
    /// `If-Match` is compared with the config each one replaces. Never taken on an I/O thread.
    applying: Mutex<()>,
}

/// The configs applied to a running akit, the running one last, and the one path every new
/// config takes ([`update`](Configs::update)). Cloned, it is the same history.
#[derive(Debug, Clone)]
pub struct Configs {
    inner: Arc<Inner>,
}

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(PoisonError::into_inner)
}

/// The ETag of `data`, a config as written: a strong entity tag of the first 96 bits of its
/// SHA-256, as fit's.
#[must_use]
pub fn etag_of(data: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut h = Sha256::new();
    h.update(data);
    let sum = h.finalize();
    let mut s = String::with_capacity(26);
    s.push('"');
    for b in sum.iter().take(12) {
        for nibble in [b >> 4, b & 0xf] {
            if let Some(c) = HEX.get(usize::from(nibble)) {
                s.push(char::from(*c));
            }
        }
    }
    s.push('"');
    s
}

/// Whether an `If-Match` or `If-None-Match` field's value names `etag`: `*`, or a list of entity
/// tags one of which is `etag`, compared strongly, as fit's.
#[must_use]
pub fn etag_matches(field: &str, etag: &str) -> bool {
    field
        .split(',')
        .map(str::trim)
        .any(|t| t == "*" || t == etag)
}

impl Configs {
    /// The history of a server running `data`, version 1 from `source`, keeping `keep` configs
    /// (at least one), the running one included. `data` is standard JSON
    /// ([`akit_config::value::standardize`], [`akit_config::load_files`]).
    #[must_use]
    pub fn new(data: String, source: &'static str, keep: usize) -> Configs {
        let first = Arc::new(Version {
            number: 1,
            applied: SystemTime::now(),
            source,
            etag: etag_of(data.as_bytes()),
            data,
        });
        let keep = keep.max(1);
        let mut history = VecDeque::with_capacity(keep);
        history.push_back(first);
        Configs {
            inner: Arc::new(Inner {
                keep,
                history: Mutex::new(history),
                applying: Mutex::new(()),
            }),
        }
    }

    /// The running config.
    #[must_use]
    pub fn current(&self) -> Arc<Version> {
        let h = lock(&self.inner.history);
        // Never empty: `new` puts one in, `record` only adds before it drops.
        match h.back() {
            Some(v) => Arc::clone(v),
            None => unreachable_empty(),
        }
    }

    /// The configs kept, oldest first.
    #[must_use]
    pub fn versions(&self) -> Vec<Arc<Version>> {
        lock(&self.inner.history).iter().cloned().collect()
    }

    /// The config applied as version `n`, while it is kept.
    #[must_use]
    pub fn version(&self, n: u64) -> Option<Arc<Version>> {
        lock(&self.inner.history)
            .iter()
            .find(|v| v.number == n)
            .cloned()
    }

    /// Applies `data`, a config as standard JSON from `source`, with `apply`, which loads,
    /// compiles and serves it (the caller's: on the control plane, never an I/O thread), and on
    /// success keeps it as a new version. With `if_match`, an `If-Match` field's value, nothing is
    /// applied unless it names the running config. Configs are applied one at a time.
    ///
    /// # Errors
    ///
    /// [`Refused::Changed`] when `if_match` names another config; [`Refused::Apply`] with what
    /// `apply` refused. The running config stays.
    pub fn update(
        &self,
        source: &'static str,
        if_match: Option<&str>,
        data: String,
        apply: impl FnOnce(&str) -> Result<Applied, ApplyError>,
    ) -> Result<Outcome, Refused> {
        let _one = lock(&self.inner.applying);
        let cur = self.current();
        if if_match.is_some_and(|m| !etag_matches(m, &cur.etag)) {
            return Err(Refused::Changed);
        }
        let applied = apply(&data).map_err(Refused::Apply)?;
        let v = Arc::new(Version {
            number: cur.number.saturating_add(1),
            applied: SystemTime::now(),
            source,
            etag: etag_of(data.as_bytes()),
            data,
        });
        let outcome = Outcome {
            version: v.number,
            etag: v.etag.clone(),
            source,
            applied,
        };
        let mut h = lock(&self.inner.history);
        h.push_back(v);
        while h.len() > self.inner.keep {
            h.pop_front();
        }
        Ok(outcome)
    }
}

#[cold]
fn unreachable_empty() -> Arc<Version> {
    // The history always holds the running config; this is never reached. Rather than panic on
    // an I/O thread, an empty config stands in.
    Arc::new(Version {
        number: 0,
        applied: SystemTime::UNIX_EPOCH,
        source: SOURCE_FILE,
        etag: etag_of(b""),
        data: String::new(),
    })
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::indexing_slicing)]

    use super::{Applied, ApplyError, Configs, Refused, SOURCE_ADMIN, etag_matches, etag_of};

    #[test]
    fn etags() {
        let e = etag_of(b"{}");
        assert_eq!(e.len(), 26);
        assert!(e.starts_with('"') && e.ends_with('"'));
        // SHA-256 of "{}": 44136fa355b3678a1146ad16f7e8649e94fb4fc21fe77e8310c060f61caaff8a.
        assert_eq!(e, "\"44136fa355b3678a1146ad16\"");
        assert!(etag_matches("*", &e));
        assert!(etag_matches(&format!("\"x\", {e}"), &e));
        assert!(!etag_matches("\"x\"", &e));
        assert!(!etag_matches(&format!("W/{e}"), &e));
    }

    #[test]
    fn versions_are_numbered_and_kept() {
        let cs = Configs::new("{}".into(), super::SOURCE_FILE, 2);
        assert_eq!(cs.current().number, 1);
        for i in 0..3 {
            let o = cs
                .update(SOURCE_ADMIN, None, format!("{{\"v\": {i}}}"), |_| {
                    Ok(Applied::default())
                })
                .unwrap();
            assert_eq!(o.version, i + 2);
        }
        let vs: Vec<u64> = cs.versions().iter().map(|v| v.number).collect();
        assert_eq!(vs, [3, 4]);
        assert!(cs.version(2).is_none());
        assert_eq!(cs.version(4).unwrap().data(), "{\"v\": 2}");

        // A stale If-Match, or a refused config, applies nothing.
        let stale = etag_of(b"{}");
        assert!(matches!(
            cs.update(SOURCE_ADMIN, Some(&stale), "{}".into(), |_| unreachable!()),
            Err(Refused::Changed)
        ));
        assert!(matches!(
            cs.update(SOURCE_ADMIN, None, "{}".into(), |_| Err(
                ApplyError::Failed("no".into())
            )),
            Err(Refused::Apply(ApplyError::Failed(_)))
        ));
        assert_eq!(cs.current().number, 4);
        let cur = cs.current().etag.clone();
        assert!(
            cs.update(SOURCE_ADMIN, Some(&cur), "{}".into(), |_| Ok(
                Applied::default()
            ))
            .is_ok()
        );
    }
}
