//! The `webrtc_proxy` handler (I114, fit's `webrtc/media/proxyhttp.go`): WHIP and WHEP passed to
//! the media servers of an upstream, their peers' media through `webrtc.proxy`'s one UDP port.
//!
//! An endpoint's POST (an offer, its candidates taken out: the media server's peers reach it
//! through akit alone) goes to a media server the upstream picks; a 201 is answered with the
//! media server's answer, its candidates replaced by the proxy port's and its `c=` line by the
//! port's address, the answer's ufrag and pwd recorded against the address its peers are relayed
//! to (`media_addrs`, or the answer's first host or server-reflexive candidate, IPv4 first), and
//! its `Location` replaced by a URL of akit's: the endpoint's path followed by the session's ID.
//! That URL, and the paths below it, go to the same media server: a DELETE forgets the session,
//! a PATCH with an ICE restart's offer records the new credentials (a trickled candidate is
//! answered 204 without reaching the media server, which the peer reaches through akit alone),
//! anything else is passed on. Each exchange waits `timeout` at most (504), its bodies at most
//! `max_sdp_bytes` (413, 502).

use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

use akio_http::client::{CancelToken, ClientError, ClientRequest, ClientResponse};
use akio_http::{Context, Headers, Method, Response};
use akit_runtime::{Handler, HandlerParams, Provision, ProvisionError, Provisioned};
use akit_upstream::{EngineSlot, Pool};

use crate::proxy::Cred;

/// The `webrtc_proxy` handler.
struct ProxyHandler {
    pool: Pool,
    engine: EngineSlot,
    /// `media_addrs`: by the media server's address in the upstream.
    media: Vec<(String, SocketAddr)>,
    timeout: Duration,
    limit: usize,
}

fn session_id(s: &str) -> bool {
    s.len() == 32
        && s.bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}

fn respond(ctx: &mut Context, status: u16, msg: &str, headers: &[(&str, &str)]) {
    let mut resp = Response::new(status);
    for (n, v) in headers {
        resp.headers.set(n, v);
    }
    if !msg.is_empty() && status != 204 {
        resp.headers
            .set("Content-Type", "text/plain; charset=utf-8");
        resp.body = format!("{msg}\n").into_bytes();
    }
    ctx.respond(resp);
}

/// The header fields a proxy does not pass on.
const HOP_BY_HOP: [&str; 10] = [
    "connection",
    "keep-alive",
    "proxy-connection",
    "proxy-authenticate",
    "proxy-authorization",
    "te",
    "trailer",
    "transfer-encoding",
    "upgrade",
    "content-length",
];

fn hop_by_hop(name: &str) -> bool {
    HOP_BY_HOP.iter().any(|h| h.eq_ignore_ascii_case(name))
}

/// What came back of an exchange with a media server.
type Outcome = Result<ClientResponse, Failure>;

enum Failure {
    /// Answered with this status and message.
    Status(u16, &'static str),
    Timeout,
    Unreachable,
}

impl ProxyHandler {
    fn backend_of(&self, address: &str) -> Option<usize> {
        self.pool.addresses().position(|a| a == address)
    }

    /// Sends the request in `ctx`, with `body` and to `target` on `backend`, and calls `finish`
    /// with what came back, the request resumed.
    fn forward(
        self: &Arc<Self>,
        ctx: &mut Context,
        backend: usize,
        target: &str,
        body: Vec<u8>,
        finish: impl FnOnce(&mut Context, &Arc<ProxyHandler>, Outcome) + Send + 'static,
    ) {
        let mut url = target.to_owned();
        if !url.contains('?') {
            if let Some(q) = ctx.req().query() {
                url.push('?');
                url.push_str(q);
            }
        }
        let mut out = ClientRequest::new(ctx.method().clone(), &url);
        let mut headers = Headers::with_capacity(ctx.req().headers.len(), 0);
        for (n, v) in ctx.req().headers.iter() {
            if !hop_by_hop(n) && !n.eq_ignore_ascii_case("x-forwarded-for") {
                headers.append(n, v);
            }
        }
        headers.set("X-Forwarded-For", ctx.peer().ip().to_string());
        out.headers = headers;
        out.body = body;
        out.timeout = Some(self.timeout);
        let token = CancelToken::new();
        out = out.cancel_with(&token);
        let gone = token.clone();
        ctx.on_gone(move || gone.cancel());
        let Some(suspended) = ctx.suspend() else {
            return respond(ctx, 500, "the request cannot wait", &[]);
        };
        let me = Arc::clone(self);
        let limit = self.limit;
        self.pool.send(backend, out, move |outcome| {
            let r = match outcome {
                Ok(resp) if resp.body.len() > limit => Err(Failure::Status(
                    502,
                    "the media server's response is too large",
                )),
                Ok(resp) => Ok(resp),
                Err(akit_upstream::Error::Client(
                    ClientError::Timeout | ClientError::HeaderTimeout | ClientError::IdleTimeout,
                ))
                | Err(akit_upstream::Error::Timeout) => Err(Failure::Timeout),
                Err(akit_upstream::Error::Busy | akit_upstream::Error::Closed) => {
                    Err(Failure::Status(503, "the media servers are busy"))
                }
                Err(_) => Err(Failure::Unreachable),
            };
            suspended.resume(move |ctx| finish(ctx, &me, r));
        });
    }

    fn pass_on(ctx: &mut Context, r: Outcome, links: Option<(&str, &str)>) {
        match r {
            Err(Failure::Status(code, msg)) => respond(ctx, code, msg, &[]),
            Err(Failure::Timeout) => {
                respond(ctx, 504, "the media server did not answer in time", &[]);
            }
            Err(Failure::Unreachable) => {
                respond(ctx, 502, "the media server could not be reached", &[]);
            }
            Ok(resp) => {
                let mut out = Response::new(resp.status);
                out.headers = response_headers(&resp.headers);
                if let Some((loc, base)) = links {
                    for l in rewrite_links(&resp.headers, loc, base) {
                        out.headers.append("Link", &l);
                    }
                }
                out.body = resp.body;
                ctx.respond(out);
            }
        }
    }

    fn offer(self: &Arc<Self>, ctx: &mut Context, p: &Arc<crate::proxy::Core>, endpoint: String) {
        let sdp_type = ctx.header("Content-Type").is_some_and(|t| {
            t.split(';')
                .next()
                .is_some_and(|m| m.trim().eq_ignore_ascii_case("application/sdp"))
        });
        if !sdp_type {
            return respond(
                ctx,
                415,
                "the offer must be application/sdp",
                &[("Accept-Post", "application/sdp")],
            );
        }
        if ctx.req().body.len() > self.limit {
            return respond(ctx, 413, "the offer is too large", &[]);
        }
        if !p.reserve() {
            return respond(ctx, 503, "too many sessions", &[("Retry-After", "1")]);
        }
        let backend = self.pool.pick();
        let Some(address) = self.pool.address(backend).map(str::to_owned) else {
            p.unreserve();
            return respond(ctx, 502, "the upstream has no media server", &[]);
        };
        let body = strip_candidates(&ctx.req().body);
        let path = ctx.path().to_owned();
        let target = path.clone();
        let p = Arc::clone(p);
        self.forward(ctx, backend, &target, body, move |ctx, h, r| {
            let resp = match r {
                Ok(resp) if resp.status == 201 => resp,
                other => {
                    p.unreserve();
                    return ProxyHandler::pass_on(ctx, other, None);
                }
            };
            let Some(loc) = resp
                .headers
                .get("Location")
                .and_then(|l| upstream_path(&path, l))
            else {
                p.unreserve();
                return respond(ctx, 502, "the media server's answer has no Location", &[]);
            };
            let over = h.media.iter().find(|(a, _)| *a == address).map(|(_, m)| *m);
            let rewritten = rewrite_sdp(&resp.body, &p.addrs(), over);
            let added = rewritten
                .and_then(|(sdp, creds)| {
                    if creds.first().is_none_or(|c| c.media.is_none()) {
                        return Err(
                            "the answer has no ice-ufrag, or no candidate akit can relay to"
                                .to_owned(),
                        );
                    }
                    Ok((sdp, creds))
                })
                .inspect_err(|_| p.unreserve())
                .and_then(|(sdp, creds)| p.add(&address, &loc, &creds).map(|id| (sdp, id)));
            match added {
                Err(e) => {
                    respond(ctx, 502, &format!("the media server's answer: {e}"), &[]);
                    h.hang_up(backend, &loc);
                }
                Ok((sdp, id)) => {
                    let url = format!("{endpoint}/{id}");
                    let mut out = Response::new(201);
                    out.headers = response_headers(&resp.headers);
                    out.headers.set("Location", &url);
                    for l in rewrite_links(&resp.headers, &loc, &url) {
                        out.headers.append("Link", &l);
                    }
                    out.body = sdp;
                    ctx.respond(out);
                }
            }
        });
    }

    /// Ends a session at the media server whose answer akit could not take.
    fn hang_up(&self, backend: usize, location: &str) {
        let mut req = ClientRequest::new(Method::Delete, location);
        req.timeout = Some(self.timeout);
        self.pool.send(backend, req, |_| {});
    }

    fn session(
        self: &Arc<Self>,
        ctx: &mut Context,
        p: &Arc<crate::proxy::Core>,
        s: &Arc<crate::proxy::Session>,
        base: String,
        rest: String,
    ) {
        let method = ctx.method().clone();
        let Some(backend) = self.backend_of(&s.backend) else {
            if method == Method::Delete {
                p.end(&s.id);
            }
            return respond(
                ctx,
                502,
                "the session's media server is no longer in the upstream",
                &[],
            );
        };
        let target = format!("{}{rest}", s.location);
        let body = ctx.req().body.clone();
        if body.len() > self.limit {
            return respond(ctx, 413, "the request is too large", &[]);
        }
        let (loc, id) = (s.location.clone(), s.id.clone());
        let p = Arc::clone(p);
        match method {
            Method::Delete if rest.is_empty() => {
                self.forward(ctx, backend, &target, body, move |ctx, _, r| {
                    p.end(&id);
                    ProxyHandler::pass_on(ctx, r, None);
                });
            }
            Method::Patch if rest.is_empty() => {
                if !body.windows(12).any(|w| w == b"a=ice-ufrag:") {
                    // A trickled candidate: the peer reaches the media server through akit.
                    return respond(ctx, 204, "", &[]);
                }
                let body = strip_candidates(&body);
                let backend_addr = s.backend.clone();
                self.forward(ctx, backend, &target, body, move |ctx, h, r| {
                    let r = match r {
                        Ok(mut resp)
                            if (200..300).contains(&resp.status) && !resp.body.is_empty() =>
                        {
                            let over = h
                                .media
                                .iter()
                                .find(|(a, _)| *a == backend_addr)
                                .map(|(_, m)| *m);
                            match rewrite_sdp(&resp.body, &p.addrs(), over)
                                .and_then(|(sdp, creds)| p.restart(&id, &creds).map(|()| sdp))
                            {
                                Ok(sdp) => {
                                    resp.body = sdp;
                                    Ok(resp)
                                }
                                Err(e) => {
                                    return respond(
                                        ctx,
                                        502,
                                        &format!("the media server's answer: {e}"),
                                        &[],
                                    );
                                }
                            }
                        }
                        other => other,
                    };
                    ProxyHandler::pass_on(ctx, r, Some((&loc, &base)));
                });
            }
            _ => {
                self.forward(ctx, backend, &target, body, move |ctx, _, r| {
                    ProxyHandler::pass_on(ctx, r, Some((&loc, &base)));
                });
            }
        }
    }
}

/// The handler in an `Arc`, which the exchanges it starts keep.
struct Shared(Arc<ProxyHandler>);

impl akio_http::Service for Shared {
    fn call(&self, ctx: &mut Context) {
        let h = &self.0;
        let Some(p) = h.engine.get().and_then(crate::proxy::lookup) else {
            return respond(ctx, 503, "webrtc.proxy is not running", &[]);
        };
        let path = ctx.path().to_owned();
        if let Some((s, base, rest)) = session_in(&p, &path) {
            return h.session(ctx, &p, &s, base, rest);
        }
        let last = path.trim_end_matches('/').rsplit('/').next().unwrap_or("");
        match ctx.method().clone() {
            Method::Post => {
                let endpoint = path.trim_end_matches('/').to_owned();
                h.offer(ctx, &p, endpoint);
            }
            Method::Options => {
                let backend = h.pool.pick();
                h.forward(ctx, backend, &path, Vec::new(), |ctx, _, r| {
                    ProxyHandler::pass_on(ctx, r, None);
                });
            }
            _ if session_id(last) => respond(ctx, 404, "no such session", &[]),
            _ => respond(
                ctx,
                405,
                "the endpoint takes POST",
                &[("Allow", "POST, OPTIONS")],
            ),
        }
    }
}

impl Handler for Shared {}

/// The session whose URL `path` is or is below, with the URL and what follows it: a segment of
/// the path's last four that is a session's ID.
fn session_in(
    p: &crate::proxy::Core,
    path: &str,
) -> Option<(Arc<crate::proxy::Session>, String, String)> {
    let mut end = path.len();
    for _ in 0..4 {
        let head = path.get(..end)?;
        let i = head.rfind('/')?;
        let seg = head.get(i.checked_add(1)?..)?;
        if session_id(seg) {
            if let Some(s) = p.session(seg) {
                return Some((s, head.to_owned(), path.get(end..)?.to_owned()));
            }
        }
        end = i;
    }
    None
}

/// A response's header fields but the hop-by-hop ones, `Location` and `Link`.
fn response_headers(h: &Headers) -> Headers {
    let mut out = Headers::with_capacity(h.len(), 0);
    for (n, v) in h.iter() {
        if !hop_by_hop(n) && !n.eq_ignore_ascii_case("location") && !n.eq_ignore_ascii_case("link")
        {
            out.append(n, v);
        }
    }
    out
}

/// The path (and query) a `Location` names, resolved against the request's `path`.
fn upstream_path(path: &str, loc: &str) -> Option<String> {
    let loc = loc.trim();
    if loc.is_empty() {
        return None;
    }
    if let Some(rest) = loc
        .strip_prefix("http://")
        .or_else(|| loc.strip_prefix("https://"))
    {
        return Some(
            rest.find('/')
                .map_or("/".to_owned(), |i| rest.get(i..).unwrap_or("/").to_owned()),
        );
    }
    if loc.starts_with('/') {
        return Some(loc.to_owned());
    }
    let dir = path.rfind('/').map_or("", |i| path.get(..=i).unwrap_or(""));
    Some(format!("{dir}{loc}"))
}

/// The `Link` header values of a media server's response as akit passes them on: ICE servers as
/// they are, and links below the session's URL `loc` moved below akit's `base` (fit's `setLinks`).
fn rewrite_links(h: &Headers, loc: &str, base: &str) -> Vec<String> {
    let mut out = Vec::new();
    for field in h.get_all("Link") {
        for l in split_links(field) {
            let l = l.trim();
            let Some(rest) = l.strip_prefix('<') else {
                continue;
            };
            let Some((uri, params)) = rest.split_once('>') else {
                continue;
            };
            if params.contains("ice-server") {
                out.push(l.to_owned());
                continue;
            }
            let Some(p) = upstream_path(loc, uri) else {
                continue;
            };
            if let Some(suffix) = p.strip_prefix(loc) {
                out.push(format!("<{base}{suffix}>{params}"));
            }
        }
    }
    out
}

/// A `Link` field's links, split at commas outside `<>` and quotes.
fn split_links(field: &str) -> Vec<&str> {
    let mut out = Vec::new();
    let (mut in_uri, mut in_quote, mut start) = (false, false, 0usize);
    for (i, c) in field.char_indices() {
        match c {
            '<' if !in_quote => in_uri = true,
            '>' if !in_quote => in_uri = false,
            '"' if !in_uri => in_quote = !in_quote,
            ',' if !in_uri && !in_quote => {
                out.push(field.get(start..i).unwrap_or(""));
                start = i.saturating_add(1);
            }
            _ => {}
        }
    }
    out.push(field.get(start..).unwrap_or(""));
    out
}

/// An SDP without its candidates (fit's `stripCandidates`).
fn strip_candidates(sdp: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(sdp.len());
    for line in sdp.split_inclusive(|b| *b == b'\n') {
        let t = line.strip_suffix(b"\n").unwrap_or(line);
        let t = t.strip_suffix(b"\r").unwrap_or(t);
        if t.starts_with(b"a=candidate:") || t == b"a=end-of-candidates" {
            continue;
        }
        out.extend_from_slice(line);
    }
    out
}

/// fit's `rewriteSDP`: the answer with the proxy's addresses as its only candidates and its
/// `c=` lines, and the credentials of each of its ICE ufrags, with where their peers are
/// relayed: `over` (with its port, or the best candidate's), or the answer's best candidate, a
/// host one before a server-reflexive one, IPv4 first.
pub(crate) fn rewrite_sdp(
    sdp: &[u8],
    addrs: &[SocketAddr],
    over: Option<SocketAddr>,
) -> Result<(Vec<u8>, Vec<Cred>), String> {
    let Some(first) = addrs.first() else {
        return Err("webrtc.proxy has no address".to_owned());
    };
    let text = String::from_utf8_lossy(sdp);
    let lines: Vec<&str> = text
        .trim_end_matches(['\r', '\n'])
        .split('\n')
        .map(|l| l.trim_end_matches('\r'))
        .collect();
    let mut best: Option<(u8, SocketAddr)> = None;
    let mut creds: Vec<Cred> = Vec::new();
    let (mut ufrag, mut pwd) = (String::new(), String::new());
    let flush = |creds: &mut Vec<Cred>, ufrag: &str, pwd: &str| {
        if !ufrag.is_empty() && !pwd.is_empty() && !creds.iter().any(|c| c.ufrag == ufrag) {
            creds.push(Cred {
                ufrag: ufrag.to_owned(),
                pwd: pwd.to_owned(),
                media: None,
            });
        }
    };
    for l in &lines {
        if l.starts_with("m=") {
            flush(&mut creds, &ufrag, &pwd);
        } else if let Some(u) = l.strip_prefix("a=ice-ufrag:") {
            u.clone_into(&mut ufrag);
        } else if let Some(p) = l.strip_prefix("a=ice-pwd:") {
            p.clone_into(&mut pwd);
        } else if let Some(c) = l.strip_prefix("a=candidate:") {
            let f: Vec<&str> = c.split_whitespace().collect();
            let (Some(comp), Some(transport), Some(ip), Some(port), Some(typ)) = (
                f.get(1),
                f.get(2),
                f.get(4).and_then(|a| a.parse::<std::net::IpAddr>().ok()),
                f.get(5).and_then(|p| p.parse::<u16>().ok()),
                f.get(7),
            ) else {
                continue;
            };
            if *comp != "1" || !transport.eq_ignore_ascii_case("udp") {
                continue;
            }
            let ip = match ip {
                std::net::IpAddr::V6(v6) => v6
                    .to_ipv4_mapped()
                    .map_or(std::net::IpAddr::V6(v6), std::net::IpAddr::V4),
                v4 => v4,
            };
            let link_local = match ip {
                std::net::IpAddr::V4(v4) => v4.is_link_local(),
                std::net::IpAddr::V6(v6) => (v6.segments()[0] & 0xffc0) == 0xfe80,
            };
            if link_local || ip.is_unspecified() {
                continue;
            }
            let mut rank = match *typ {
                "host" => 0u8,
                "srflx" => 2,
                _ => continue,
            };
            if !ip.is_ipv4() {
                rank = rank.saturating_add(1);
            }
            if best.is_none_or(|(r, _)| rank < r) {
                best = Some((rank, SocketAddr::new(ip, port)));
            }
        }
    }
    flush(&mut creds, &ufrag, &pwd);
    let media = match (over, best) {
        (Some(o), _) if o.port() != 0 => Some(o),
        (Some(o), Some((_, b))) => Some(SocketAddr::new(o.ip(), b.port())),
        (None, Some((_, b))) => Some(b),
        _ => None,
    };
    for c in &mut creds {
        c.media = media;
    }
    let cands: Vec<String> = addrs
        .iter()
        .enumerate()
        .map(|(i, a)| {
            let pref = 65535u32.saturating_sub(u32::try_from(i).unwrap_or(0));
            let prio = (126u32 << 24) | (pref << 8) | 255;
            format!(
                "a=candidate:{} 1 udp {prio} {} {} typ host",
                i.saturating_add(1),
                a.ip(),
                a.port()
            )
        })
        .chain(std::iter::once("a=end-of-candidates".to_owned()))
        .collect();
    let mut out: Vec<String> = Vec::new();
    let mut media_at: Option<usize> = None;
    let close = |out: &mut Vec<String>, at: Option<usize>| {
        if let Some(i) = at {
            let port = out
                .get(i)
                .and_then(|m| m.split_whitespace().nth(1))
                .unwrap_or("0");
            if port != "0" {
                out.extend(cands.iter().cloned());
            }
        }
    };
    let mut sections = 0usize;
    for l in &lines {
        if l.starts_with("a=candidate:") || *l == "a=end-of-candidates" {
            continue;
        }
        let mut line = (*l).to_owned();
        if l.starts_with("m=") {
            close(&mut out, media_at);
            media_at = Some(out.len());
            sections = sections.saturating_add(1);
        } else if l.starts_with("c=IN ") {
            let family = if first.is_ipv4() { "IP4" } else { "IP6" };
            line = format!("c=IN {family} {}", first.ip());
        }
        out.push(line);
    }
    close(&mut out, media_at);
    if sections == 0 {
        out.extend(cands);
    }
    let mut s = out.join("\r\n");
    s.push_str("\r\n");
    Ok((s.into_bytes(), creds))
}

pub(crate) fn webrtc_proxy(
    p: &Provision<'_, HandlerParams>,
) -> Result<Provisioned<dyn Handler>, ProvisionError> {
    let HandlerParams::WebRtcProxy(params) = p.params() else {
        return Err(ProvisionError::new(
            "the webrtc_proxy handler was given another's params",
        ));
    };
    let Some(pool) = p.env().upstream(&params.upstream).cloned() else {
        return Err(ProvisionError::field(
            "upstream",
            format!("there is no upstream named {:?}", params.upstream),
        ));
    };
    let h = Arc::new(ProxyHandler {
        pool,
        engine: p.env().engine().clone(),
        media: params.media_addrs.clone(),
        timeout: params.timeout(),
        limit: params.sdp_bytes(),
    });
    Ok(Provisioned::new(Arc::new(Shared(h)) as Arc<dyn Handler>))
}

#[cfg(test)]
mod tests;
