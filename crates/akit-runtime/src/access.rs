//! Where each listener's access lists decide (I85, fit's `runtime/graph.go` `compileAccess`).
//!
//! A listener's policy is its own list, then the global one. When that is all there is (no route
//! or service of the listener has a list of its own) and the client's address is known when a
//! connection is accepted (no PROXY header names another), the listener's TCP side refuses at
//! accept, through akio's filter (A3): [`ListenerAccess::accept`]. Otherwise it decides later:
//! each HTTP request against its route's policy, the route's own list, then the listener's, then
//! the global one ([`ListenerAccess::request`]), a refused request answered 403.
//!
//! akio filters no UDP peers (akit's request A220): a listener's HTTP/3 requests are checked one
//! by one against the policy its TCP side asks at accept, until it does.

use std::collections::BTreeMap;
use std::sync::Arc;

use akit_access::{List, Policy, Sum};
use akit_config::{Access, Error, Listener, Protocol};

/// What a listener's access lists decide, and where. The default decides nothing: every address
/// is served.
#[derive(Debug, Default, Clone)]
pub struct ListenerAccess {
    /// The listener's own list, then the global one.
    policy: Option<Arc<Policy>>,
    /// What its TCP side asks at accept: `policy`, when the address is known then and no route
    /// of the listener has a list of its own.
    accept: Option<Arc<Policy>>,
    /// Every HTTP request is checked against [`routes`](Self::routes) (by its route), or against
    /// `policy` when no route takes it.
    requests: bool,
    /// By the index of the graph's routes: the policy of a request that route takes on this
    /// listener, when `requests`.
    routes: Vec<Option<Arc<Policy>>>,
}

impl ListenerAccess {
    /// The policy the listener's TCP side refuses connections by at accept (akio's
    /// `Listener::filter`); `None`: it accepts every connection, and decides later if at all.
    #[must_use]
    pub fn accept(&self) -> Option<&Arc<Policy>> {
        self.accept.as_ref()
    }

    /// The policy a request the route at `route` takes (`None`: no route takes it) is checked
    /// against: `None` when there is none to check, the connection having been checked at
    /// accept. `unfiltered`: its connection was not, as an HTTP/3 one is not (A220). Runs per
    /// request: it only reads.
    #[must_use]
    pub fn request(&self, route: Option<usize>, unfiltered: bool) -> Option<&Policy> {
        if self.requests {
            return match route {
                Some(i) => self.routes.get(i).and_then(Option::as_deref),
                None => self.policy.as_deref(),
            };
        }
        if unfiltered {
            return self.accept.as_deref();
        }
        None
    }

    /// The listener's own list, then the global one: what decides for it where no route or
    /// service of its own has a say.
    #[must_use]
    pub fn policy(&self) -> Option<&Arc<Policy>> {
        self.policy.as_ref()
    }
}

/// Compiles `a`, at `path` in the config, reading its files into `files`; a problem is added to
/// `errors`, and the access counts as none.
pub(crate) fn list(
    a: Option<&Access>,
    path: &str,
    files: &mut BTreeMap<String, Sum>,
    errors: &mut Vec<Error>,
) -> Option<Arc<List>> {
    match akit_access::compile(a, Some(files)) {
        Ok(l) => l,
        Err(es) => {
            errors.extend(es.into_iter().map(|e| Error {
                file: None,
                path: format!("{path}.{}", e.field),
                line: 0,
                col: 0,
                msg: e.msg,
            }));
            None
        }
    }
}

/// Whether listener `l` serves HTTP over TCP or QUIC, and so routes.
fn serves_http(l: &Listener) -> bool {
    l.redirect_https == 0
        && l.protocols.iter().any(|p| {
            matches!(
                p,
                Protocol::Http11 | Protocol::H2c | Protocol::H2 | Protocol::H3
            )
        })
}

/// The access of listener `l`, from the global list, its own and `routes`: by the index of the
/// graph's routes, each route's own list when the route serves on `l`, `None` otherwise.
pub(crate) fn listener(
    l: &Listener,
    global: Option<&Arc<List>>,
    own: Option<&Arc<List>>,
    routes: &[Option<Arc<List>>],
) -> ListenerAccess {
    let policy = Policy::chain([own, global]);
    let routed = routes.iter().any(Option::is_some);
    if policy.is_none() && !routed {
        return ListenerAccess::default();
    }
    let proxy = l.proxy_protocol.is_some();
    if !proxy && !routed {
        return ListenerAccess {
            accept: policy.clone(),
            policy,
            ..ListenerAccess::default()
        };
    }
    // Decided later: by each request, when the listener routes.
    let requests = serves_http(l);
    let routes = if requests {
        routes
            .iter()
            .map(|r| match r {
                Some(r) => Policy::chain([Some(r), own, global]),
                None => policy.clone(),
            })
            .collect()
    } else {
        Vec::new()
    };
    ListenerAccess {
        policy,
        accept: None,
        requests,
        routes,
    }
}
