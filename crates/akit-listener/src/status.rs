//! What akit is doing now (I63), as fit's `GET /status` says it (fit's `admin.Configs.Status`):
//! the configs applied, the listeners and their connections, the upstreams and their backends'
//! health, the certificates the listeners serve, and, when the running config counts them, the
//! requests answered. The Admin API writes the running config's version, when it was applied and
//! where it came from, then [`Scrape::write_status`]'s members; the dashboard reads them.
//!
//! Like the scrape, it reads counters and the current graph: control plane work, a few
//! microseconds a route or backend, never on the request path.

use std::fmt::Write as _;

use akit_config::value::write_string;
use akit_observe::metrics::STATUS_CLASSES;
use akit_observe::time::{Stamp, rfc3339};

use crate::metrics::Scrape;

/// A time in seconds since the epoch, as RFC 3339 in UTC, quoted.
fn time(b: &mut String, secs: i64) {
    let mut t = Vec::new();
    rfc3339(&mut t, Stamp { secs, millis: 0 }, 0);
    write_string(b, &String::from_utf8_lossy(&t));
}

fn strings<'a>(b: &mut String, ss: impl IntoIterator<Item = &'a str>) {
    b.push('[');
    for (i, s) in ss.into_iter().enumerate() {
        if i > 0 {
            b.push(',');
        }
        write_string(b, s);
    }
    b.push(']');
}

impl Scrape {
    /// Appends what akit is doing now as the members of a JSON object, with fit's names,
    /// comma-separated, with no comma before the first or after the last: `reloads`,
    /// `listeners`, `connections`, `upstreams`, `certificates`, and `requests` when the running
    /// config counts them (`metrics`). The listeners are those of
    /// [`with_listeners`](Scrape::with_listeners)'s view, none without one.
    pub fn write_status(&self, b: &mut String) {
        let (tried, failed) = self.config().reloads();
        let _ = write!(
            b,
            "\"reloads\":{{\"succeeded\":{},\"failed\":{failed}}}",
            tried.saturating_sub(failed)
        );

        b.push_str(",\"listeners\":[");
        if let Some(view) = self.listeners() {
            for (i, l) in view.get().iter().enumerate() {
                if i > 0 {
                    b.push(',');
                }
                b.push_str("{\"name\":");
                write_string(b, &l.name);
                b.push_str(",\"address\":");
                write_string(b, &l.address.to_string());
                b.push_str(",\"protocols\":");
                strings(b, l.protocols.iter().map(|p| p.as_str()));
                b.push('}');
            }
        }
        b.push(']');

        let tel = self.engine().telemetry().metrics();
        let _ = write!(
            b,
            ",\"connections\":{{\"open\":{},\"refused\":{},\"pending_bytes\":{}}}",
            tel.open_connections, tel.total.rejected, tel.total.queued_bytes
        );

        self.cell().with(|g| {
            let mut pools: Vec<_> = g.upstreams().iter().collect();
            pools.sort_by(|a, b| a.name().cmp(b.name()));
            b.push_str(",\"upstreams\":[");
            for (n, p) in pools.iter().enumerate() {
                if n > 0 {
                    b.push(',');
                }
                b.push_str("{\"name\":");
                write_string(b, p.name());
                let _ = write!(b, ",\"in_flight\":{},\"backends\":[", p.in_flight());
                for (i, addr) in p.addresses().enumerate() {
                    if i > 0 {
                        b.push(',');
                    }
                    let (done, failed) = p.backend_requests(i);
                    b.push_str("{\"address\":");
                    write_string(b, addr);
                    let _ = write!(b, ",\"up\":{}", !p.is_down(i));
                    if p.is_probed_down(i) {
                        b.push_str(",\"probed_down\":true");
                    }
                    let _ = write!(
                        b,
                        ",\"in_flight\":{},\"succeeded\":{},\"failed\":{failed}}}",
                        p.backend_active(i),
                        done.saturating_sub(failed)
                    );
                }
                b.push_str("]}");
            }
            b.push(']');

            let mut certs = Vec::new();
            for (i, l) in g.listeners().iter().enumerate() {
                for c in g.certificates(i) {
                    certs.push((l.name.as_str(), c));
                }
            }
            // By listener, then by expiry, as fit sorts them.
            certs.sort_by(|a, b| (a.0, a.1.not_after).cmp(&(b.0, b.1.not_after)));
            b.push_str(",\"certificates\":[");
            for (i, (listener, c)) in certs.iter().enumerate() {
                if i > 0 {
                    b.push(',');
                }
                b.push_str("{\"listener\":");
                write_string(b, listener);
                b.push_str(",\"names\":");
                strings(b, c.names.iter().map(String::as_str));
                b.push_str(",\"issuer\":");
                write_string(b, &c.issuer);
                b.push_str(",\"serial\":");
                write_string(b, &c.serial);
                b.push_str(",\"not_before\":");
                time(b, c.not_before);
                b.push_str(",\"not_after\":");
                time(b, c.not_after);
                b.push('}');
            }
            b.push(']');

            if let Some(m) = g.metrics() {
                let totals = m.totals();
                let total = totals.iter().fold(0u64, |a, n| a.wrapping_add(*n));
                let _ = write!(b, ",\"requests\":{{\"total\":{total},\"classes\":{{");
                for (i, (class, n)) in STATUS_CLASSES.iter().zip(totals).enumerate() {
                    if i > 0 {
                        b.push(',');
                    }
                    let _ = write!(b, "\"{class}\":{n}");
                }
                b.push_str("}}");
            }
        });
    }
}
