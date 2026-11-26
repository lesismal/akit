//! What the stores reach the network with: an akio engine of the store's own, with one client
//! thread; TLS client configurations from files; and HTTP calls through akio-http's client,
//! waited for on the caller's thread (never an I/O thread), each within its deadline.

use std::sync::Arc;
use std::time::{Duration, Instant};

use akio::Engine;
use akio::tls::{Certificate, ClientConfig, PrivateKey, RootCerts};
use akio_http::client::{
    Client, ClientError, ClientOptions, ClientRequest, ClientResponse, RedirectPolicy,
};
use akit_config::ClientTlsFiles;

use super::MAX_KEYS;
use crate::storage::MAX_VALUE_BYTES;

/// The largest answer read from etcd or Consul: a value, base64'd and quoted, or a listing.
pub(crate) const MAX_ANSWER_BYTES: usize = 3 * (4 << 20) + MAX_KEYS * 128;
const _: () = assert!(MAX_VALUE_BYTES == 4 << 20);

/// The most of an error's answer quoted.
const MAX_ERROR_BYTES: usize = 4 << 10;

/// An engine of one client thread, shut down when dropped.
pub(crate) struct NetEngine {
    engine: Arc<Engine>,
}

impl NetEngine {
    pub(crate) fn start() -> Result<NetEngine, String> {
        let mut cfg = akio::Config::default();
        cfg.shutdown_timeout = Duration::from_secs(1);
        let engine = Engine::builder()
            .client_threads(1)
            .config(cfg)
            .build()
            .map_err(|e| format!("its client: {e}"))?;
        engine.start().map_err(|e| format!("its client: {e}"))?;
        Ok(NetEngine {
            engine: Arc::new(engine),
        })
    }

    pub(crate) fn engine(&self) -> &Arc<Engine> {
        &self.engine
    }
}

impl Drop for NetEngine {
    fn drop(&mut self) {
        akit_observe::stop_client_engine(&self.engine);
    }
}

/// A TLS client configuration from `files`: its roots from `ca_file` (the system's when empty),
/// and the client certificate of `cert_file` and `key_file` when set. `None` when no file is
/// set and `always` is false: the client's defaults then.
pub(crate) fn tls(files: &ClientTlsFiles, always: bool) -> Result<Option<ClientConfig>, String> {
    if !always && files.ca_file.is_empty() && files.cert_file.is_empty() {
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

/// An HTTP API's client, on an engine of its own.
pub(crate) struct Http {
    client: Client,
    // Dropped after the client: shuts the engine down.
    _engine: NetEngine,
}

impl Http {
    /// A client with the TLS of `files` (read now), keeping a few connections.
    pub(crate) fn new(files: &ClientTlsFiles) -> Result<Http, String> {
        let tls = tls(files, false)?;
        let engine = NetEngine::start()?;
        let mut opts = ClientOptions::default();
        opts.max_body_bytes = MAX_ANSWER_BYTES;
        opts.max_idle_per_host = 4;
        opts.max_idle = 4;
        opts.idle_timeout = Some(Duration::from_secs(30));
        let mut client = Client::new(Arc::clone(engine.engine()))
            .with_options(opts)
            .with_redirects(RedirectPolicy::none());
        if let Some(tls) = tls {
            client = client.with_tls(tls);
        }
        Ok(Http {
            client,
            _engine: engine,
        })
    }

    /// Sends `req` and waits for its answer, until `deadline`.
    pub(crate) fn call(
        &self,
        req: ClientRequest,
        deadline: Instant,
        what: &str,
    ) -> Result<ClientResponse, String> {
        let left = deadline.saturating_duration_since(Instant::now());
        if left.is_zero() {
            return Err(format!("{what}: timed out"));
        }
        // The client's own timeout ends the request; this one bounds the wait should it not.
        match self
            .client
            .send(req.timeout(left))
            .wait_timeout(left.saturating_add(Duration::from_secs(1)))
        {
            Some(Ok(r)) => Ok(r),
            Some(Err(ClientError::TooLarge(_))) => Err(format!(
                "{what}: the answer is larger than {MAX_ANSWER_BYTES} bytes"
            )),
            Some(Err(e)) => Err(format!("{what}: {e}")),
            None => Err(format!("{what}: timed out")),
        }
    }
}

/// An answer that is not what was asked for: "`what`: 500 Internal Server Error: `body`", the
/// body cut to 4 KiB.
pub(crate) fn status_error(r: &ClientResponse, what: &str) -> String {
    let body = r.body.get(..MAX_ERROR_BYTES).unwrap_or(&r.body);
    let text = String::from_utf8_lossy(body);
    format!("{what}: {} {}: {}", r.status, r.reason, text.trim())
}

/// A string as JSON.
pub(crate) fn json_string(s: &str) -> String {
    let mut b = String::with_capacity(s.len().saturating_add(2));
    akit_config::value::write_string(&mut b, s);
    b
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
    escape(s, |b| {
        b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.' | b'~' | b' ')
    })
    .replace(' ', "+")
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
