//! `forward_proxy`: an HTTP forward proxy for CONNECT (I55), as fit's `handler/forward`, which
//! opens a tunnel to the host and port a request names, for the clients and to the destinations
//! its rules allow ([`akit_l4::forward::Rules`]).
//!
//! The request's client and its destination, as written, are checked first: a client the rules
//! do not hold, or a destination they refuse, is answered 403. The tunnel is then akio-socks's
//! (`connect_proxy_with`): the name is resolved off the I/O thread, the connection to it decided
//! by the address it was opened to ([`akit_l4::forward::Rules::resolved`], answered 502 when
//! refused), `200 Connection Established` sent once it is open, and the bytes relayed both ways
//! with linked backpressure. A destination not connected within `connect_timeout` is answered
//! 504, one that cannot be reached 502; a tunnel that carries nothing for `idle_timeout` is
//! closed. A request that is not a CONNECT, an extended CONNECT among them, is answered 405, and
//! one whose target is not `host:port` 400.
//!
//! akio tunnels CONNECT over HTTP/1.1 alone: a CONNECT on an HTTP/2 or HTTP/3 stream, which fit
//! tunnels too, is answered 501 until akio does (akio API request A134).

use std::sync::Arc;

use akio_http::{Context, Method, Response, Service, Version};
use akit_l4::forward::{Rules, Verdict};
use akit_runtime::{Handler, HandlerParams, Provision, ProvisionError, Provisioned};

/// The handler: its rules, and akio's CONNECT proxy built from them.
struct ForwardProxy<S> {
    rules: Arc<Rules>,
    tunnel: S,
}

/// What a CONNECT's target names: `host:port`, an IPv6 host in brackets (which come off).
fn target(t: &str) -> Option<(&str, u16)> {
    let (host, port) = t.rsplit_once(':')?;
    let host = host
        .strip_prefix('[')
        .map_or(host, |h| h.strip_suffix(']').unwrap_or(h));
    if host.is_empty() || port.is_empty() || !port.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    Some((host, port.parse().ok()?))
}

impl<S: Service> Service for ForwardProxy<S> {
    fn call(&self, ctx: &mut Context) {
        if *ctx.method() != Method::Connect || ctx.req().protocol.is_some() {
            return ctx.respond(
                Response::text(405, "only CONNECT is proxied\n").header("Allow", "CONNECT"),
            );
        }
        if !matches!(ctx.req().version, Version::Http10 | Version::Http11) {
            return ctx.text(
                501,
                "CONNECT is tunneled over HTTP/1.1 alone so far (akio API request A134)\n",
            );
        }
        let verdict = match target(&ctx.req().target) {
            Some((host, port)) => self.rules.dest_host(host, port),
            None => return ctx.text(400, "CONNECT needs host:port\n"),
        };
        if !self.rules.client(ctx.peer().ip()) || verdict == Verdict::Refused {
            return ctx.text(403, "forbidden\n");
        }
        self.tunnel.call(ctx);
    }
}

impl<S: Service> Handler for ForwardProxy<S> {}

pub(crate) fn new(
    p: &Provision<'_, HandlerParams>,
) -> Result<Provisioned<dyn Handler>, ProvisionError> {
    let HandlerParams::ForwardProxy(f) = p.params() else {
        return Err(ProvisionError::new(
            "the forward_proxy handler's params are not forward_proxy's",
        ));
    };
    let rules = Rules::new(f);
    // The tunnels are dialed on the engine `akit run` builds once the graph is compiled.
    let engine = akio::EngineRef::new();
    let set = engine.clone();
    p.env().engine().when_set(move |e| set.set(e));
    let tunnel = akio_socks::connect_proxy_with(engine, rules.proxy_config());
    Ok(Provisioned::new(
        Arc::new(ForwardProxy { rules, tunnel }) as Arc<dyn Handler>
    ))
}

#[cfg(test)]
mod tests {
    #[test]
    fn targets() {
        use super::target;
        assert_eq!(target("example.com:443"), Some(("example.com", 443)));
        assert_eq!(target("[::1]:8080"), Some(("::1", 8080)));
        assert_eq!(target("127.0.0.1:1"), Some(("127.0.0.1", 1)));
        for bad in [
            "nohost",
            "example.com:",
            ":443",
            "example.com:x",
            "a:70000",
            "a:+1",
        ] {
            assert_eq!(target(bad), None, "{bad}");
        }
    }
}
