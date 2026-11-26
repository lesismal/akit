//! A client of a service's HTTP API, for the providers that follow one (Consul, etcd): fit's
//! `internal/apiclient`, on akio's HTTP client.
//!
//! Each provider has an akio engine of its own with one client thread, started with the
//! provider and shut down when it stops. Requests are made from the provider's threads, which
//! wait for their outcome: the client's callback, on its I/O thread, only hands it over. Every
//! request has a timeout, and is cancelled when its [`Stop`] is.

use std::sync::Arc;
use std::sync::mpsc::{self, RecvTimeoutError};
use std::time::Duration;

use akio::Engine;
use akio::tls::{Certificate, ClientConfig, PrivateKey, RootCerts};
use akio_http::client::{
    Client, ClientError, ClientOptions, ClientRequest, ClientResponse, RedirectPolicy,
};

use crate::watch::Stop;

/// The largest answer read, and the largest message of a watch: fit's `maxBodyBytes`, 32 MiB.
pub(crate) const MAX_BODY_BYTES: usize = 32 << 20;

/// The most of an error's answer quoted: fit's `maxErrorBytes`.
const MAX_ERROR_BYTES: usize = 4 << 10;

/// The engine a provider's clients run on.
pub(crate) struct ApiEngine {
    engine: Arc<Engine>,
}

impl ApiEngine {
    /// Starts an engine of one client thread, for the provider `name`.
    pub(crate) fn start(name: &str) -> Result<ApiEngine, String> {
        let mut cfg = akio::Config::default();
        cfg.shutdown_timeout = Duration::from_secs(1);
        let engine = Engine::builder()
            .client_threads(1)
            .config(cfg)
            .build()
            .map_err(|e| format!("provider {name}: its client: {e}"))?;
        engine
            .start()
            .map_err(|e| format!("provider {name}: its client: {e}"))?;
        Ok(ApiEngine {
            engine: Arc::new(engine),
        })
    }

    pub(crate) fn engine(&self) -> &Arc<Engine> {
        &self.engine
    }
}

impl Drop for ApiEngine {
    fn drop(&mut self) {
        akit_observe::stop_client_engine(&self.engine);
    }
}

/// The TLS files of a client: fit's `apiclient.TLSFiles`.
pub(crate) fn tls(files: &akit_config::ClientTlsFiles) -> Result<Option<ClientConfig>, String> {
    if files.ca_file.is_empty() && files.cert_file.is_empty() {
        return Ok(None);
    }
    let read = |path: &str| std::fs::read(path).map_err(|e| format!("{path}: {e}"));
    let roots = if files.ca_file.is_empty() {
        RootCerts::system().map_err(|e| format!("the system's CAs: {e}"))?
    } else {
        let pem = read(&files.ca_file)?;
        let mut r = RootCerts::empty();
        if !matches!(r.add_pem(&pem), Ok(n) if n > 0) {
            return Err(format!("{}: no PEM certificates", files.ca_file));
        }
        r
    };
    let mut b = ClientConfig::builder().roots(roots);
    if !files.cert_file.is_empty() {
        let (cert, key) = (&files.cert_file, &files.key_file);
        let chain = Certificate::from_pem(&read(cert)?).map_err(|e| format!("{cert}: {e}"))?;
        if chain.is_empty() {
            return Err(format!("{cert}: no PEM certificate found"));
        }
        let k = PrivateKey::from_pem(&read(key)?).map_err(|e| format!("{key}: {e}"))?;
        b = b.client_cert(chain, k);
    }
    b.build().map(Some).map_err(|e| e.to_string())
}

/// A client of one API, with its TLS read: what a watch and its reads share.
#[derive(Clone)]
pub(crate) struct Api {
    pub(crate) client: Client,
    /// The largest answer read.
    pub(crate) max_body: usize,
}

impl Api {
    /// A client on `engine`, with the TLS of `files` (read now).
    pub(crate) fn open(
        engine: &ApiEngine,
        files: &akit_config::ClientTlsFiles,
        max_body: usize,
    ) -> Result<Api, String> {
        let tls = tls(files)?;
        let mut opts = ClientOptions::default();
        opts.max_body_bytes = max_body;
        opts.max_idle_per_host = 4;
        opts.max_idle = 4;
        opts.idle_timeout = Some(Duration::from_secs(30));
        let mut client = Client::new(Arc::clone(engine.engine()))
            .with_options(opts)
            .with_redirects(RedirectPolicy::none());
        if let Some(tls) = tls {
            client = client.with_tls(tls);
        }
        Ok(Api { client, max_body })
    }

    /// Sends `req`, which `what` names in errors, and waits for its answer: at most `timeout`,
    /// and no longer than `stop` lasts.
    pub(crate) fn call(
        &self,
        req: ClientRequest,
        timeout: Duration,
        stop: &Stop,
        what: &str,
    ) -> Result<ClientResponse, String> {
        let req = req.timeout(timeout).cancel_with(stop.token());
        let (tx, rx) = mpsc::sync_channel(1);
        // On the client's I/O thread: hands the outcome over, never blocks (room for one).
        self.client.request(req, move |r| {
            let _ = tx.try_send(r);
        });
        // The client's own timeout ends the request; this one bounds the wait should it not.
        match rx.recv_timeout(timeout.saturating_add(Duration::from_secs(1))) {
            Ok(Ok(r)) => Ok(r),
            Ok(Err(e)) => Err(format!("{what}: {}", self.error(&e))),
            Err(RecvTimeoutError::Timeout) => Err(format!("{what}: timed out")),
            Err(RecvTimeoutError::Disconnected) => Err(format!("{what}: the request was dropped")),
        }
    }

    /// A client error as fit's words have it.
    pub(crate) fn error(&self, e: &ClientError) -> String {
        match e {
            ClientError::TooLarge(_) => {
                format!("the answer is larger than {} bytes", self.max_body)
            }
            ClientError::Cancelled => "stopped".to_owned(),
            e => e.to_string(),
        }
    }
}

/// `200 OK`, as Go's `Response.Status` gives a status.
pub(crate) fn status_line(status: u16, reason: &str) -> String {
    let reason = if reason.is_empty() {
        default_reason(status)
    } else {
        reason
    };
    format!("{status} {reason}")
}

fn default_reason(status: u16) -> &'static str {
    match status {
        200 => "OK",
        400 => "Bad Request",
        401 => "Unauthorized",
        403 => "Forbidden",
        404 => "Not Found",
        429 => "Too Many Requests",
        500 => "Internal Server Error",
        502 => "Bad Gateway",
        503 => "Service Unavailable",
        504 => "Gateway Timeout",
        _ => "",
    }
}

/// An answer that is not what was asked for: fit's `apiclient.StatusError`, "`what`: 500
/// Internal Server Error: `body`", the body cut to 4 KiB.
pub(crate) fn status_error(r: &ClientResponse, what: &str) -> String {
    let body = r.body.get(..MAX_ERROR_BYTES).unwrap_or(&r.body);
    let text = String::from_utf8_lossy(body);
    format!(
        "{what}: {}: {}",
        status_line(r.status, &r.reason),
        text.trim()
    )
}

/// A string as JSON, for the bodies sent.
pub(crate) fn json_string(s: &str) -> String {
    let mut b = String::with_capacity(s.len().saturating_add(2));
    akit_config::value::write_string(&mut b, s);
    b
}

/// Reads an answer as JSON, `what` naming it in an error.
pub(crate) fn parse_json(body: &[u8], what: &str) -> Result<akit_config::value::Value, String> {
    let text =
        std::str::from_utf8(body).map_err(|e| format!("{what}: the answer is not UTF-8: {e}"))?;
    akit_config::value::parse(text).map_err(|e| {
        let first = e
            .as_slice()
            .first()
            .map_or_else(|| "not JSON".to_owned(), ToString::to_string);
        format!("{what}: {first}")
    })
}

/// Percent-escapes a path segment as Go's `url.PathEscape` does.
pub(crate) fn path_escape(s: &str) -> String {
    escape(s, |b| {
        b.is_ascii_alphanumeric()
            || matches!(
                b,
                b'-' | b'_' | b'.' | b'~' | b'$' | b'&' | b'+' | b',' | b'=' | b':' | b'@'
            )
    })
}

/// Percent-escapes a query's key or value as Go's `url.QueryEscape` does.
pub(crate) fn query_escape(s: &str) -> String {
    let e = escape(s, |b| {
        b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.' | b'~' | b' ')
    });
    e.replace(' ', "+")
}

fn escape(s: &str, keep: impl Fn(u8) -> bool) -> String {
    const HEX: &[u8; 16] = b"0123456789ABCDEF";
    let mut out = String::with_capacity(s.len());
    for &b in s.as_bytes() {
        if keep(b) {
            out.push(char::from(b));
        } else {
            out.push('%');
            for nibble in [b >> 4, b & 15] {
                out.push(HEX.get(usize::from(nibble)).map_or('0', |&h| char::from(h)));
            }
        }
    }
    out
}
