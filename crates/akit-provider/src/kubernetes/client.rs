//! akit's own client of the API server (fit's `client.go`, on akio's clients): lists in pages,
//! watches and status patches, over HTTP/2 when the server is `https` and chooses `h2` in ALPN,
//! as the API server does, its requests multiplexed on one connection; over HTTP/1.1 otherwise,
//! a watch to a connection. Requests are started on the provider's thread and answered on the
//! I/O thread of an engine of the provider's own, whose callbacks only decode and hand over,
//! never block.

use std::fmt;
use std::fmt::Write as _;
use std::sync::Arc;
use std::time::Duration;

use akio::Engine;
use akio_http::client::{
    CancelToken, Client, ClientError, ClientOptions, ClientRequest, RedirectPolicy, ResponseBody,
    ResponseHandler, ResponseHead,
};
use akio_http::h2client::{H2Client, H2Event, H2Options, NotH2};
use akio_http::message::{Headers, Method};

use super::kubeconfig::{RestConfig, Token};

/// The largest page of a list read.
pub(crate) const MAX_PAGE_BYTES: usize = 64 << 20;
/// The most of an error's body read.
pub(crate) const MAX_ERROR_BYTES: usize = 4 << 10;
/// How many objects a page asks for.
pub(crate) const PAGE_SIZE: usize = 500;
/// How long a watch lasts before the server ends it and the provider makes another.
pub(crate) const WATCH_SECONDS: u64 = 300;

/// Why a request failed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Failure {
    /// 404: the kind is not served.
    NotFound,
    /// 410, or a watch's 410 event: the resource version is too old.
    Gone,
    /// 409 to a status patch: the object changed since the resource version it names.
    Conflict,
    /// Anything else, said.
    Other(String),
}

impl fmt::Display for Failure {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Failure::NotFound => f.write_str("not found"),
            Failure::Gone => f.write_str("the resource version is too old"),
            Failure::Conflict => f.write_str("the object changed"),
            Failure::Other(s) => f.write_str(s),
        }
    }
}

/// What an answer other than 200 means: 404 and 410 by themselves, else the status and the
/// message of the API server's `Status`, or the body.
pub(crate) fn failure(method: &str, path: &str, status: u16, reason: &str, body: &[u8]) -> Failure {
    match status {
        404 => return Failure::NotFound,
        410 => return Failure::Gone,
        _ => {}
    }
    let body = body.get(..MAX_ERROR_BYTES).unwrap_or(body);
    let text = String::from_utf8_lossy(body);
    let msg = akit_config::value::parse(&text)
        .ok()
        .and_then(|v| match v.get("message") {
            Some(akit_config::value::Value::String(m)) if !m.is_empty() => Some(m.clone()),
            _ => None,
        })
        .unwrap_or_else(|| text.trim().to_owned());
    let reason = if reason.is_empty() {
        default_reason(status)
    } else {
        reason
    };
    Failure::Other(format!("{method} {path}: {status} {reason}: {msg}"))
}

fn default_reason(status: u16) -> &'static str {
    match status {
        400 => "Bad Request",
        401 => "Unauthorized",
        403 => "Forbidden",
        409 => "Conflict",
        429 => "Too Many Requests",
        500 => "Internal Server Error",
        503 => "Service Unavailable",
        _ => "",
    }
}

/// A streamed answer, as a watch reads it: told its head, its bytes and its end on an I/O
/// thread. It must not block.
pub(crate) trait Stream: Send + 'static {
    /// The head: the status. A status other than 200 is followed by the error's body (at most
    /// [`MAX_ERROR_BYTES`]) through `data`, then `end`.
    fn head(&mut self, status: u16, reason: &str);
    /// Bytes of the body, as they arrive; valid for the call only.
    fn data(&mut self, chunk: &[u8]);
    /// The answer ended, completely or not.
    fn end(&mut self, r: Result<(), String>);
}

/// How the client reaches the server.
enum Transport {
    /// HTTP/1.1, a request to a connection.
    H1,
    /// HTTP/2, to be dialed.
    H2Idle,
    /// HTTP/2, being dialed.
    H2Dialing,
    H2(H2Client),
}

/// What [`Api::poll`] says of the client.
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum Ready {
    /// Requests can be made.
    Yes,
    /// The connection is being made: wait for [`Api::dialed`].
    Dialing,
}

/// The client, on the provider's thread.
pub(crate) struct Api {
    engine: Arc<Engine>,
    server: String,
    /// The host and port dialed for HTTP/2, and the name its certificate is checked against.
    host: String,
    port: u16,
    tls_name: String,
    h1: Client,
    h2_tls: Option<akio::tls::ClientConfig>,
    transport: Transport,
    token: Option<Token>,
    timeout: Duration,
}

impl fmt::Debug for Api {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Api")
            .field("server", &self.server)
            .finish_non_exhaustive()
    }
}

/// The scheme, host and port of `url`.
fn authority(url: &str) -> Result<(bool, String, u16), String> {
    let (https, rest) = if let Some(r) = url.strip_prefix("https://") {
        (true, r)
    } else if let Some(r) = url.strip_prefix("http://") {
        (false, r)
    } else {
        return Err(format!("server {url:?} is neither http:// nor https://"));
    };
    let auth = rest.split('/').next().unwrap_or_default();
    let (host, port) = if let Some(r) = auth.strip_prefix('[') {
        let (h, p) = r
            .split_once(']')
            .ok_or_else(|| format!("server {url:?}: a bad host"))?;
        (h, p.strip_prefix(':'))
    } else {
        match auth.rsplit_once(':') {
            Some((h, p)) => (h, Some(p)),
            None => (auth, None),
        }
    };
    let port = match port {
        Some(p) => p
            .parse()
            .map_err(|_| format!("server {url:?}: a bad port"))?,
        None if https => 443,
        None => 80,
    };
    if host.is_empty() {
        return Err(format!("server {url:?} has no host"));
    }
    Ok((https, host.to_owned(), port))
}

impl Api {
    /// A client of the server `rc` names, on `engine`; each request but watches takes at most
    /// `timeout`.
    pub(crate) fn new(
        engine: Arc<Engine>,
        rc: RestConfig,
        timeout: Duration,
    ) -> Result<Api, String> {
        let (https, host, port) = authority(&rc.server)?;
        let mut opts = ClientOptions::default();
        opts.max_body_bytes = MAX_PAGE_BYTES;
        opts.max_idle_per_host = 4;
        opts.max_idle = 4;
        opts.response_header_timeout = Some(timeout);
        let mut h1 = Client::new(Arc::clone(&engine))
            .with_options(opts)
            .with_redirects(RedirectPolicy::none());
        let mut h2_tls = None;
        let mut tls_name = host.clone();
        let transport = match (&rc.tls, https) {
            (Some(t), true) => {
                h1 = h1.with_tls(t.config(&["http/1.1"])?);
                if !t.server_name.is_empty() {
                    h1 = h1.with_tls_server_name(t.server_name.clone());
                    tls_name.clone_from(&t.server_name);
                }
                h2_tls = Some(t.config(&["h2", "http/1.1"])?);
                Transport::H2Idle
            }
            _ => Transport::H1,
        };
        Ok(Api {
            engine,
            server: rc.server,
            host,
            port,
            tls_name,
            h1,
            h2_tls,
            transport,
            token: rc.token,
            timeout,
        })
    }

    /// Whether requests can be made; when the HTTP/2 connection is to be made, starts making
    /// it, `dialed` called with its outcome on an I/O thread.
    pub(crate) fn poll(
        &mut self,
        dialed: impl FnOnce(Result<H2Client, std::io::Error>) + Send + 'static,
    ) -> Ready {
        if let Transport::H2(c) = &self.transport {
            if !c.is_open() {
                self.transport = Transport::H2Idle;
            }
        }
        match &self.transport {
            Transport::H1 | Transport::H2(_) => Ready::Yes,
            Transport::H2Dialing => Ready::Dialing,
            Transport::H2Idle => {
                let Some(tls) = self.h2_tls.clone() else {
                    self.transport = Transport::H1;
                    return Ready::Yes;
                };
                let mut opts = H2Options::default();
                opts.max_body_bytes = MAX_PAGE_BYTES;
                opts.happy_eyeballs.connect_timeout = self.timeout;
                self.transport = Transport::H2Dialing;
                H2Client::connect(
                    &self.engine,
                    &self.host,
                    self.port,
                    Some((tls, &self.tls_name)),
                    opts,
                    dialed,
                );
                Ready::Dialing
            }
        }
    }

    /// The outcome of the dial [`poll`](Self::poll) started: `Ok` when it failed for a reason
    /// other than the server's speaking HTTP/1.1 alone, which the client then speaks.
    pub(crate) fn dialed(&mut self, r: Result<H2Client, std::io::Error>) -> Result<(), String> {
        match r {
            Ok(c) => {
                self.transport = Transport::H2(c);
                Ok(())
            }
            Err(e) if NotH2::of(&e).is_some() => {
                self.transport = Transport::H1;
                Ok(())
            }
            Err(e) => {
                self.transport = Transport::H2Idle;
                Err(format!("connecting to {}: {e}", self.server))
            }
        }
    }

    fn request(&mut self, path_query: &str, cancel: &CancelToken) -> Result<ClientRequest, String> {
        self.request_of(Method::Get, path_query, cancel)
    }

    fn request_of(
        &mut self,
        method: Method,
        path_query: &str,
        cancel: &CancelToken,
    ) -> Result<ClientRequest, String> {
        let mut req = ClientRequest::new(method, &format!("{}{path_query}", self.server))
            .header("accept", "application/json")
            .cancel_with(cancel);
        if let Some(t) = &mut self.token {
            let t = t.get().map_err(|e| format!("token: {e}"))?;
            req = req.header("authorization", &format!("Bearer {t}"));
        }
        Ok(req)
    }

    /// Sends a GET of `path_query` (a page of a list), within the request timeout; `done` gets
    /// the body of a 200, on an I/O thread (or this one, when the request cannot be made).
    pub(crate) fn get(
        &mut self,
        path: &str,
        query: &str,
        cancel: &CancelToken,
        done: impl FnOnce(Result<Vec<u8>, Failure>) + Send + 'static,
    ) {
        let req = match self.request(&format!("{path}?{query}"), cancel) {
            Ok(r) => r.timeout(self.timeout),
            Err(e) => return done(Err(Failure::Other(e))),
        };
        let path = path.to_owned();
        let cb = move |r: Result<akio_http::client::ClientResponse, ClientError>| {
            done(match r {
                Ok(r) if r.status == 200 => Ok(r.body),
                Ok(r) => Err(failure("GET", &path, r.status, &r.reason, &r.body)),
                Err(ClientError::TooLarge(_)) => Err(Failure::Other(format!(
                    "GET {path}: a page is larger than {MAX_PAGE_BYTES} bytes"
                ))),
                Err(e) => Err(Failure::Other(format!("GET {path}: {e}"))),
            });
        };
        match &self.transport {
            Transport::H2(c) => c.request(req, cb),
            _ => self.h1.request(req, cb),
        }
    }

    /// Sends a JSON merge patch to the status of the object at `path` (its path on the API
    /// server, without `/status`), within the request timeout; `done` gets its outcome on an
    /// I/O thread (or this one, when the request cannot be made): 404 and 409, the object gone
    /// or changed since the resource version the patch names, by themselves.
    pub(crate) fn patch_status(
        &mut self,
        path: &str,
        patch: Vec<u8>,
        cancel: &CancelToken,
        done: impl FnOnce(Result<(), Failure>) + Send + 'static,
    ) {
        let req = match self.request_of(Method::Patch, &format!("{path}/status"), cancel) {
            Ok(r) => r
                .header("content-type", "application/merge-patch+json")
                .body(patch)
                .timeout(self.timeout),
            Err(e) => return done(Err(Failure::Other(e))),
        };
        let path = path.to_owned();
        let cb = move |r: Result<akio_http::client::ClientResponse, ClientError>| {
            done(match r {
                Ok(r) if r.status == 200 => Ok(()),
                Ok(r) if r.status == 409 => Err(Failure::Conflict),
                Ok(r) => Err(failure("PATCH", &path, r.status, &r.reason, &r.body)),
                Err(e) => Err(Failure::Other(format!("PATCH {path}: {e}"))),
            });
        };
        match &self.transport {
            Transport::H2(c) => c.request(req, cb),
            _ => self.h1.request(req, cb),
        }
    }

    /// Opens a watch of `path` narrowed by `query`, from the resource version `rv`, which lasts
    /// [`WATCH_SECONDS`], or until `cancel`; `s` reads it. A watch that sends nothing, bookmarks
    /// included, for twice its time is given up, so that a half-open connection does not stall
    /// it.
    pub(crate) fn watch(
        &mut self,
        path: &str,
        query: &str,
        rv: &str,
        cancel: &CancelToken,
        mut s: impl Stream,
    ) {
        let mut q = format!(
            "watch=1&resourceVersion={}&allowWatchBookmarks=true&timeoutSeconds={WATCH_SECONDS}",
            escape(rv)
        );
        if !query.is_empty() {
            q.push('&');
            q.push_str(query);
        }
        let req = match self.request(&format!("{path}?{q}"), cancel) {
            Ok(r) => r
                .response_header_timeout(self.timeout)
                .response_idle_timeout(Duration::from_secs(2 * WATCH_SECONDS)),
            Err(e) => return s.end(Err(e)),
        };
        match &self.transport {
            Transport::H2(c) => {
                let mut s = Some(s);
                let up = c.open(req, move |e| {
                    let Some(st) = s.as_mut() else {
                        return;
                    };
                    match e {
                        H2Event::Head { status, .. } => st.head(status, ""),
                        H2Event::Data(d) => st.data(d),
                        H2Event::Trailers(_) => {}
                        H2Event::End => {
                            if let Some(mut st) = s.take() {
                                st.end(Ok(()));
                            }
                        }
                        H2Event::Error(e) => {
                            if let Some(mut st) = s.take() {
                                st.end(Err(e.to_string()));
                            }
                        }
                    }
                });
                // A GET: no body.
                up.finish();
            }
            _ => self.h1.request_with(req, H1Stream(Some(s))),
        }
    }
}

/// A [`Stream`] read over HTTP/1.1.
struct H1Stream<S>(Option<S>);

impl<S: Stream> ResponseHandler for H1Stream<S> {
    fn on_head(&mut self, head: ResponseHead, _body: ResponseBody) {
        if let Some(s) = &mut self.0 {
            s.head(head.status, &head.reason);
        }
    }

    fn on_data(&mut self, chunk: &[u8]) {
        if let Some(s) = &mut self.0 {
            s.data(chunk);
        }
    }

    fn on_end(&mut self, _: &Headers) {
        if let Some(mut s) = self.0.take() {
            s.end(Ok(()));
        }
    }

    fn on_error(&mut self, e: ClientError) {
        if let Some(mut s) = self.0.take() {
            s.end(Err(e.to_string()));
        }
    }
}

/// `s` escaped for a URL's query.
pub(crate) fn escape(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for b in s.bytes() {
        if b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.' | b'~') {
            out.push(char::from(b));
        } else {
            let _ = write!(out, "%{b:02X}");
        }
    }
    out
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]

    use super::{Failure, authority, escape, failure};

    #[test]
    fn urls_and_failures() {
        assert_eq!(
            authority("https://k:6443").unwrap(),
            (true, "k".to_owned(), 6443)
        );
        assert_eq!(
            authority("http://[::1]/p").unwrap(),
            (false, "::1".to_owned(), 80)
        );
        assert_eq!(authority("https://h").unwrap().2, 443);
        assert!(authority("ftp://h").is_err());
        assert!(authority("https://h:x").is_err());
        assert_eq!(
            escape("type=kubernetes.io/tls"),
            "type%3Dkubernetes.io%2Ftls"
        );
        assert_eq!(failure("GET", "/x", 404, "", b""), Failure::NotFound);
        assert_eq!(failure("GET", "/x", 410, "", b""), Failure::Gone);
        assert_eq!(
            failure(
                "GET",
                "/x",
                401,
                "",
                br#"{"kind":"Status","message":"Unauthorized"}"#
            ),
            Failure::Other("GET /x: 401 Unauthorized: Unauthorized".to_owned())
        );
    }
}
