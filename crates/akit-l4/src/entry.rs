//! What a `tcp` or `udp` listener hands its connections to (fit's `l4.Entry`): its services, one
//! picked by the server name a TLS client's ClientHello asks for when any of them has
//! `server_names`, and the one without as the fallback.

use std::collections::HashMap;
use std::fmt;
use std::sync::Arc;

use crate::Service;

/// The services of one listener, by the server names they take. Built once, when the graph is
/// compiled; cheap to clone: clones share it.
#[derive(Clone, Default)]
pub struct Entry(Arc<Inner>);

#[derive(Default)]
struct Inner {
    /// The service without `server_names`, if any.
    fallback: Option<Service>,
    /// The services of exact names, by name in lower case.
    exact: HashMap<String, Service>,
    /// The services of names `*.d`, by `d` in lower case.
    wild: HashMap<String, Service>,
    /// The listener's `sockmap` (I51b): its TCP connections are spliced in the kernel where
    /// the relay is loaded ([`crate::sockmap`]).
    sockmap: bool,
}

impl fmt::Debug for Entry {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Entry")
            .field("fallback", &self.0.fallback.as_ref().map(Service::name))
            .field("sockmap", &self.0.sockmap)
            .field(
                "names",
                &self.0.exact.len().saturating_add(self.0.wild.len()),
            )
            .finish()
    }
}

/// `name` as the entry compares it: in lower case, without a trailing dot.
fn normalize(name: &str) -> String {
    name.strip_suffix('.').unwrap_or(name).to_ascii_lowercase()
}

impl Entry {
    /// An entry with no service, which closes every connection.
    #[must_use]
    pub fn new() -> Entry {
        Entry::default()
    }

    /// The entry of `services`, each with its server names (none for the fallback), in the
    /// config's order: a later service takes a name or the fallback an earlier one had, which a
    /// config's checks make sure does not happen.
    #[must_use]
    pub fn of<'a>(services: impl IntoIterator<Item = (Service, &'a [String])>) -> Entry {
        Entry::of_listener(services, false)
    }

    /// [`of`](Self::of), for a listener whose `sockmap` is `sockmap` (I51b).
    #[must_use]
    pub fn of_listener<'a>(
        services: impl IntoIterator<Item = (Service, &'a [String])>,
        sockmap: bool,
    ) -> Entry {
        let mut inner = Inner {
            sockmap,
            ..Inner::default()
        };
        for (s, names) in services {
            if names.is_empty() {
                inner.fallback = Some(s);
                continue;
            }
            for name in names {
                let name = normalize(name);
                match name.strip_prefix("*.") {
                    Some(d) => inner.wild.insert(d.to_owned(), s.clone()),
                    None => inner.exact.insert(name, s.clone()),
                };
            }
        }
        Entry(Arc::new(inner))
    }

    /// Whether its listener asks for its TCP connections to be spliced in the kernel
    /// (`sockmap`, I51b).
    #[must_use]
    pub fn sockmap(&self) -> bool {
        self.0.sockmap
    }

    /// Whether it picks services by the ClientHello's server name: one of them has
    /// `server_names`.
    #[must_use]
    pub fn routes(&self) -> bool {
        !self.0.exact.is_empty() || !self.0.wild.is_empty()
    }

    /// The service without `server_names`: for a connection whose server name no other takes,
    /// that is not TLS, or that names none.
    #[must_use]
    pub fn fallback(&self) -> Option<&Service> {
        self.0.fallback.as_ref()
    }

    /// The service whose `server_names` take `name`, compared without case or a trailing dot:
    /// an exact name first, then a `*.` wildcard one label above it.
    #[must_use]
    pub fn named(&self, name: &str) -> Option<&Service> {
        let name = normalize(name);
        if let Some(s) = self.0.exact.get(&name) {
            return Some(s);
        }
        let (_, d) = name.split_once('.')?;
        self.0.wild.get(d)
    }

    /// The service a connection asking for `name` (or none) goes to: [`named`](Self::named),
    /// else the [`fallback`](Self::fallback).
    #[must_use]
    pub fn pick(&self, name: Option<&str>) -> Option<&Service> {
        name.and_then(|n| self.named(n))
            .or(self.0.fallback.as_ref())
    }

    /// Its services, each once, the fallback first.
    #[must_use]
    pub fn services(&self) -> Vec<&Service> {
        let mut v: Vec<&Service> = Vec::new();
        for s in self
            .0
            .fallback
            .iter()
            .chain(self.0.exact.values())
            .chain(self.0.wild.values())
        {
            if !v.iter().any(|x| x.same(s)) {
                v.push(s);
            }
        }
        v
    }

    /// How many connections its services proxy now, theirs from other listeners too.
    #[must_use]
    pub fn conns(&self) -> usize {
        self.services()
            .iter()
            .fold(0, |n, s| n.saturating_add(s.conns()))
    }
}
