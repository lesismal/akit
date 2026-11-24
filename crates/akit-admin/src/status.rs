//! `GET /status` (I63), which the dashboard reads with `GET /config/effective` (I57), with
//! fit's payload.
//!
//! `/status` is what akit is doing now: the running config's version, when it was applied and
//! where it came from, from the history; then what [`Options::status`](crate::Options) writes
//! (akit-listener's `Scrape::write_status`): the configs applied, listeners, connections,
//! upstreams and their backends' health, certificates and, when counted, requests. Without it,
//! those are empty.

use akio_http::Response;
use akit_config::value::write_string;

use crate::configs::Configs;
use crate::server::{Live, VERSION_HEADER, json};

/// The members `/status` has when nothing says what akit is doing: none of anything.
const NOTHING: &str = "\"reloads\":{\"succeeded\":0,\"failed\":0},\"listeners\":[],\
    \"connections\":{\"open\":0,\"refused\":0,\"pending_bytes\":0},\"upstreams\":[],\
    \"certificates\":[]";

/// `GET /status`.
pub(crate) fn status(configs: &Configs, live: Option<&Live>) -> Response {
    let v = configs.current();
    let mut b = format!("{{\"version\":{},\"applied\":", v.number);
    let mut t = Vec::new();
    akit_observe::time::rfc3339(&mut t, akit_observe::time::Stamp::of(v.applied), 0);
    write_string(&mut b, &String::from_utf8_lossy(&t));
    b.push_str(",\"source\":");
    write_string(&mut b, &v.source);
    b.push(',');
    match live {
        Some(write) => write(&mut b),
        None => b.push_str(NOTHING),
    }
    b.push_str("}\n");
    json(200, b).header(VERSION_HEADER, v.number.to_string())
}
