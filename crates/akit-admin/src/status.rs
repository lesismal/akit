//! `GET /status` and `GET /config/effective` (I63), which the dashboard reads, with fit's
//! payloads.
//!
//! `/status` is what akit is doing now: the running config's version, when it was applied and
//! where it came from, from the history; then what [`Options::status`](crate::Options) writes
//! (akit-listener's `Scrape::write_status`): the configs applied, listeners, connections,
//! upstreams and their backends' health, certificates and, when counted, requests. Without it,
//! those are empty.
//!
//! `/config/effective` is the running config as it runs and where each part came from, as fit's
//! `Configs.Effective`. akit has no providers yet (I57), so it is the running config, every part
//! from the config's own source: one entry in `precedence`, a source for each top-level member,
//! and for each element of `listeners`, `routes`, `upstreams` and `services`, as fit's
//! `provider.Merge` lists a base config's.

use akio_http::Response;
use akit_config::value::{Value, write_string};

use crate::configs::Configs;
use crate::server::{Live, VERSION_HEADER, error, json};

/// The top-level members whose elements fit's merge lists one by one (fit's `provider.Kinds`).
const KINDS: &[&str] = &["listeners", "routes", "upstreams", "services"];

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
    write_string(&mut b, v.source);
    b.push(',');
    match live {
        Some(write) => write(&mut b),
        None => b.push_str(NOTHING),
    }
    b.push_str("}\n");
    json(200, b).header(VERSION_HEADER, v.number.to_string())
}

/// `GET /config/effective`.
pub(crate) fn effective(configs: &Configs) -> Response {
    let v = configs.current();
    let Ok(Value::Object(members)) = akit_config::value::parse(v.data()) else {
        return error(500, "the running config is not a JSON object");
    };
    let mut b = format!("{{\"version\":{},\"etag\":", v.number);
    write_string(&mut b, &v.etag);
    b.push_str(",\"precedence\":[");
    write_string(&mut b, v.source);
    b.push_str("],\"config\":");
    b.push_str(v.data().trim());
    b.push_str(",\"sources\":[");
    let mut first = true;
    let mut source = |b: &mut String, path: &str| {
        if !first {
            b.push(',');
        }
        first = false;
        b.push_str("{\"path\":");
        write_string(b, path);
        b.push_str(",\"source\":");
        write_string(b, v.source);
        b.push('}');
    };
    for (key, value) in &members {
        match value {
            Value::Array(elems) if KINDS.contains(&key.as_str()) => {
                for i in 0..elems.len() {
                    source(&mut b, &format!("/{}/{i}", pointer_token(key)));
                }
            }
            // fit lists a kind that is not an array as nothing: its config does not load.
            _ if KINDS.contains(&key.as_str()) => {}
            _ => source(&mut b, &format!("/{}", pointer_token(key))),
        }
    }
    b.push_str("]}\n");
    json(200, b).header(VERSION_HEADER, v.number.to_string())
}

/// `key` as a JSON pointer's reference token (RFC 6901): `~` as `~0`, `/` as `~1`.
fn pointer_token(key: &str) -> String {
    key.replace('~', "~0").replace('/', "~1")
}

#[cfg(test)]
mod tests {
    #[test]
    fn pointer_tokens() {
        assert_eq!(super::pointer_token("a/b~c"), "a~1b~0c");
        assert_eq!(super::pointer_token("routes"), "routes");
    }
}
