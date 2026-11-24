//! The ACME protocol (RFC 8555) as a client of one CA: its directory, nonces, signed requests
//! and their problems, accounts, orders, authorizations, challenges, finalization and the
//! certificate's download.
//!
//! Requests go through akio-http's asynchronous client, on an engine of the client's own with one
//! thread: its callbacks only hand the response over, and the calls here wait for it on the
//! ACME worker's thread, which is akit's, never an I/O thread (rules.md rule 1). Every wait is
//! bounded by the request's timeout and the order's deadline, and ends early when the manager
//! stops.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc;
use std::sync::{Arc, Mutex, PoisonError};
use std::time::{Duration, Instant};

use akio::Engine;
use akio::tls::{ClientConfig, RootCerts};
use akio_http::client::{Client, ClientOptions, ClientRequest, ClientResponse, RedirectPolicy};
use akio_http::message::Method;
use akit_config::value::{Value, parse, write_string};

use super::jws::AccountKey;

/// What a failed call says.
pub(crate) type Error = String;

/// The largest response read from a CA: directories, orders and certificate chains are a few
/// kilobytes (rule 11).
pub(crate) const MAX_RESPONSE_BYTES: usize = 1 << 20;
/// How long a wait for a response sleeps before it looks at the stop flag again.
const SLICE: Duration = Duration::from_millis(50);
/// How many nonces are kept for later requests (rule 11).
const MAX_NONCES: usize = 16;
/// How many times a request the CA refused for its nonce is sent again with a fresh one.
const BAD_NONCE_RETRIES: usize = 3;
/// The bounds of the wait between two looks at an order or authorization under way: the CA's
/// `Retry-After` within them, or the lower one when it gives none.
const POLL_MIN: Duration = Duration::from_secs(1);
const POLL_MAX: Duration = Duration::from_secs(30);

/// The URLs of the CA's directory that akit uses.
#[derive(Debug, Clone)]
struct Directory {
    new_nonce: String,
    new_account: String,
    new_order: String,
    /// ARI's endpoint (RFC 9773), when the CA has one.
    renewal_info: Option<String>,
}

/// A CA's answer to a signed request.
#[derive(Debug)]
pub(crate) struct Answer {
    pub(crate) location: Option<String>,
    pub(crate) retry_after: Option<Duration>,
    pub(crate) body: Vec<u8>,
}

impl Answer {
    /// The body as JSON.
    pub(crate) fn json(&self) -> Result<Value, Error> {
        let text = std::str::from_utf8(&self.body).map_err(|_| "the CA's answer is not UTF-8")?;
        parse(text).map_err(|e| format!("the CA's answer is not JSON: {e}"))
    }
}

/// The string member `key` of an object.
pub(crate) fn string<'a>(v: &'a Value, key: &str) -> Option<&'a str> {
    match v.get(key) {
        Some(Value::String(s)) => Some(s),
        _ => None,
    }
}

/// The strings of the array member `key` of an object.
pub(crate) fn strings<'a>(v: &'a Value, key: &str) -> Vec<&'a str> {
    match v.get(key) {
        Some(Value::Array(a)) => a
            .iter()
            .filter_map(|x| match x {
                Value::String(s) => Some(s.as_str()),
                _ => None,
            })
            .collect(),
        _ => Vec::new(),
    }
}

/// A problem document (RFC 7807) as one line: its type and detail.
fn problem(body: &[u8]) -> String {
    let Some(v) = std::str::from_utf8(body).ok().and_then(|t| parse(t).ok()) else {
        return String::from_utf8_lossy(body.get(..200).unwrap_or(body)).into_owned();
    };
    let typ = string(&v, "type").unwrap_or("");
    let detail = string(&v, "detail").unwrap_or("");
    let typ = typ
        .strip_prefix("urn:ietf:params:acme:error:")
        .unwrap_or(typ);
    match (typ.is_empty(), detail.is_empty()) {
        (false, false) => format!("{typ}: {detail}"),
        (false, true) => typ.to_owned(),
        _ => detail.to_owned(),
    }
}

/// A client of the CA whose directory is at `directory`.
pub(crate) struct Ca {
    engine: Arc<Engine>,
    client: Client,
    directory_url: String,
    request_timeout: Duration,
    stop: Arc<AtomicBool>,
    directory: Mutex<Option<Directory>>,
    nonces: Mutex<Vec<String>>,
}

impl std::fmt::Debug for Ca {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Ca")
            .field("directory", &self.directory_url)
            .finish_non_exhaustive()
    }
}

impl Ca {
    /// A client of the CA at `directory`, its certificate checked against `roots` (the system's
    /// when `None`), each request given `request_timeout`, all of them ended when `stop` is set.
    pub(crate) fn new(
        directory: &str,
        roots: Option<RootCerts>,
        request_timeout: Duration,
        stop: Arc<AtomicBool>,
    ) -> Result<Ca, Error> {
        let mut cfg = akio::Config::default();
        cfg.shutdown_timeout = Duration::from_secs(1);
        let engine = Engine::builder()
            .client_threads(1)
            .config(cfg)
            .build()
            .map_err(|e| format!("the ACME client's engine: {e}"))?;
        engine
            .start()
            .map_err(|e| format!("the ACME client's engine: {e}"))?;
        let engine = Arc::new(engine);
        let mut opts = ClientOptions::default();
        opts.max_body_bytes = MAX_RESPONSE_BYTES;
        opts.max_idle_per_host = 2;
        opts.max_idle = 4;
        let mut client = Client::new(Arc::clone(&engine))
            .with_options(opts)
            .with_redirects(RedirectPolicy::none())
            .with_timeout(request_timeout);
        if let Some(roots) = roots {
            let tls = ClientConfig::builder()
                .roots(roots)
                .build()
                .map_err(|e| format!("acme.ca_file: {e}"))?;
            client = client.with_tls(tls);
        }
        Ok(Ca {
            engine,
            client,
            directory_url: directory.to_owned(),
            request_timeout,
            stop,
            directory: Mutex::new(None),
            nonces: Mutex::new(Vec::new()),
        })
    }

    /// Stops the client's engine: requests under way fail.
    pub(crate) fn close(&self) {
        self.engine.shutdown();
        self.engine.wait();
    }

    fn stopped(&self) -> Result<(), Error> {
        if self.stop.load(Ordering::Acquire) {
            return Err("stopped".into());
        }
        Ok(())
    }

    /// Sends `req` and waits for its response, at most until `deadline`.
    fn send(&self, mut req: ClientRequest, deadline: Instant) -> Result<ClientResponse, Error> {
        self.stopped()?;
        let left = deadline.saturating_duration_since(Instant::now());
        if left.is_zero() {
            return Err("acme.timeout passed".into());
        }
        req = req
            .timeout(left.min(self.request_timeout))
            .header("user-agent", "akit");
        let url = req.url.clone();
        let (tx, rx) = mpsc::sync_channel(1);
        // On an I/O thread: hands the outcome over, never blocks (the channel has room for it).
        self.client.request(req, move |r| {
            let _ = tx.try_send(r);
        });
        loop {
            match rx.recv_timeout(SLICE) {
                Ok(r) => return r.map_err(|e| format!("{url}: {e}")),
                Err(mpsc::RecvTimeoutError::Timeout) => self.stopped()?,
                Err(mpsc::RecvTimeoutError::Disconnected) => {
                    return Err(format!("{url}: the request was dropped"));
                }
            }
        }
    }

    /// Keeps the nonce `r` carries, if any, for a later request.
    fn keep_nonce(&self, r: &ClientResponse) {
        if let Some(n) = r.headers.get("replay-nonce").filter(|n| !n.is_empty()) {
            let mut nonces = self.nonces.lock().unwrap_or_else(PoisonError::into_inner);
            if nonces.len() >= MAX_NONCES {
                nonces.remove(0);
            }
            nonces.push(n.to_owned());
        }
    }

    fn directory(&self, deadline: Instant) -> Result<Directory, Error> {
        if let Some(d) = self
            .directory
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
        {
            return Ok(d);
        }
        let r = self.send(ClientRequest::get(&self.directory_url), deadline)?;
        if r.status != 200 {
            return Err(format!(
                "the directory {}: {} {}",
                self.directory_url,
                r.status,
                problem(&r.body)
            ));
        }
        let v = std::str::from_utf8(&r.body)
            .ok()
            .and_then(|t| parse(t).ok())
            .ok_or_else(|| format!("the directory {} is not JSON", self.directory_url))?;
        let url = |k: &str| {
            string(&v, k)
                .map(str::to_owned)
                .ok_or_else(|| format!("the directory {} has no {k}", self.directory_url))
        };
        let d = Directory {
            new_nonce: url("newNonce")?,
            new_account: url("newAccount")?,
            new_order: url("newOrder")?,
            renewal_info: string(&v, "renewalInfo")
                .filter(|u| !u.is_empty())
                .map(|u| u.trim_end_matches('/').to_owned()),
        };
        *self
            .directory
            .lock()
            .unwrap_or_else(PoisonError::into_inner) = Some(d.clone());
        Ok(d)
    }

    fn nonce(&self, deadline: Instant) -> Result<String, Error> {
        if let Some(n) = self
            .nonces
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .pop()
        {
            return Ok(n);
        }
        let dir = self.directory(deadline)?;
        let r = self.send(ClientRequest::new(Method::Head, &dir.new_nonce), deadline)?;
        r.headers
            .get("replay-nonce")
            .filter(|n| !n.is_empty())
            .map(str::to_owned)
            .ok_or_else(|| format!("{}: no Replay-Nonce", dir.new_nonce))
    }

    /// POSTs `payload` (JSON, or empty for a POST-as-GET) to `url`, signed by `key` as the account
    /// `kid` (or with its JWK when `None`), and returns the answer when its status is 2xx.
    /// A nonce the CA refuses is replaced and the request sent again.
    pub(crate) fn post(
        &self,
        key: &AccountKey,
        kid: Option<&str>,
        url: &str,
        payload: &str,
        accept: Option<&str>,
        deadline: Instant,
    ) -> Result<Answer, Error> {
        let mut tries = 0;
        loop {
            let nonce = self.nonce(deadline)?;
            let body = key.sign(url, &nonce, kid, payload)?;
            let mut req = ClientRequest::post(url, body.into_bytes())
                .header("content-type", "application/jose+json");
            if let Some(a) = accept {
                req = req.header("accept", a);
            }
            let r = self.send(req, deadline)?;
            self.keep_nonce(&r);
            if (200..300).contains(&r.status) {
                let location = r.headers.get("location").map(str::to_owned);
                let retry_after = r
                    .headers
                    .get("retry-after")
                    .and_then(|v| v.trim().parse::<u64>().ok())
                    .map(Duration::from_secs);
                return Ok(Answer {
                    location,
                    retry_after,
                    body: r.body,
                });
            }
            let why = problem(&r.body);
            if why.starts_with("badNonce") && tries < BAD_NONCE_RETRIES {
                tries = tries.saturating_add(1);
                continue;
            }
            return Err(format!("{url}: {} {why}", r.status));
        }
    }

    /// Registers the account of `key`, agreeing to the CA's terms, or finds it registered, and
    /// returns its URL.
    pub(crate) fn register(
        &self,
        key: &AccountKey,
        email: &str,
        deadline: Instant,
    ) -> Result<String, Error> {
        let dir = self.directory(deadline)?;
        let mut payload = String::from(r#"{"termsOfServiceAgreed":true"#);
        if !email.is_empty() {
            payload.push_str(r#","contact":["#);
            write_string(&mut payload, &format!("mailto:{email}"));
            payload.push(']');
        }
        payload.push('}');
        let a = self.post(key, None, &dir.new_account, &payload, None, deadline)?;
        a.location
            .ok_or_else(|| format!("{}: no account URL", dir.new_account))
    }

    /// Orders a certificate for `name`: the order's URL and its object.
    pub(crate) fn new_order(
        &self,
        key: &AccountKey,
        kid: &str,
        name: &str,
        deadline: Instant,
    ) -> Result<(String, Value), Error> {
        let dir = self.directory(deadline)?;
        let mut payload = String::from(r#"{"identifiers":[{"type":"dns","value":"#);
        write_string(&mut payload, name);
        payload.push_str("}]}");
        let a = self.post(key, Some(kid), &dir.new_order, &payload, None, deadline)?;
        let url = a
            .location
            .clone()
            .ok_or_else(|| format!("{}: no order URL", dir.new_order))?;
        Ok((url, a.json()?))
    }

    /// Reads the object at `url` (POST-as-GET), with how long the CA asks to wait before reading
    /// it again.
    pub(crate) fn get(
        &self,
        key: &AccountKey,
        kid: &str,
        url: &str,
        deadline: Instant,
    ) -> Result<(Value, Option<Duration>), Error> {
        let a = self.post(key, Some(kid), url, "", None, deadline)?;
        let v = a.json()?;
        Ok((v, a.retry_after))
    }

    /// Reads the object at `url` until its status is one of `done`, waiting between reads as
    /// the CA asks; a status of `invalid` or the deadline fails.
    pub(crate) fn poll(
        &self,
        key: &AccountKey,
        kid: &str,
        url: &str,
        done: &[&str],
        mut retry: Option<Duration>,
        deadline: Instant,
    ) -> Result<Value, Error> {
        loop {
            let (v, after) = self.get(key, kid, url, deadline)?;
            let status = string(&v, "status").unwrap_or("");
            if done.contains(&status) {
                return Ok(v);
            }
            if status == "invalid" {
                return Err(format!("{url} is invalid{}", why_invalid(&v)));
            }
            retry = after.or(retry);
            self.pause(
                retry.unwrap_or(POLL_MIN).clamp(POLL_MIN, POLL_MAX),
                deadline,
            )?;
        }
    }

    /// Tells the CA the challenge at `url` is ready to be checked.
    pub(crate) fn accept(
        &self,
        key: &AccountKey,
        kid: &str,
        url: &str,
        deadline: Instant,
    ) -> Result<(), Error> {
        self.post(key, Some(kid), url, "{}", None, deadline)
            .map(drop)
    }

    /// Sends the order's `finalize` URL the CSR `der`, and returns the order as the CA answers.
    pub(crate) fn finalize(
        &self,
        key: &AccountKey,
        kid: &str,
        url: &str,
        der: &[u8],
        deadline: Instant,
    ) -> Result<(Value, Option<Duration>), Error> {
        let payload = format!(
            r#"{{"csr":"{}"}}"#,
            akio_crypto::base64::URL_SAFE_NO_PAD.encode(der)
        );
        let a = self.post(key, Some(kid), url, &payload, None, deadline)?;
        Ok((a.json()?, a.retry_after))
    }

    /// Downloads the certificate chain at `url`, as PEM.
    pub(crate) fn certificate(
        &self,
        key: &AccountKey,
        kid: &str,
        url: &str,
        deadline: Instant,
    ) -> Result<Vec<u8>, Error> {
        self.post(
            key,
            Some(kid),
            url,
            "",
            Some("application/pem-certificate-chain"),
            deadline,
        )
        .map(|a| a.body)
    }

    /// What ARI says of the certificate whose identifier is `id`, with the `Retry-After` the CA
    /// answered with: `None` when the CA has no ARI.
    pub(crate) fn renewal_info(
        &self,
        id: &str,
        deadline: Instant,
    ) -> Result<Option<(Value, Option<String>)>, Error> {
        let Some(base) = self.directory(deadline)?.renewal_info else {
            return Ok(None);
        };
        let url = format!("{base}/{id}");
        let r = self.send(ClientRequest::get(&url), deadline)?;
        if r.status != 200 {
            return Err(format!("{url}: {} {}", r.status, problem(&r.body)));
        }
        if r.body.len() > super::ari::ARI_MAX_BYTES {
            return Err(format!("{url}: the answer is too large"));
        }
        let v = std::str::from_utf8(&r.body)
            .ok()
            .and_then(|t| parse(t).ok())
            .ok_or_else(|| format!("{url}: the answer is not JSON"))?;
        Ok(Some((v, r.headers.get("retry-after").map(str::to_owned))))
    }

    /// Sleeps `d`, or until the deadline, waking to stop.
    pub(crate) fn pause(&self, d: Duration, deadline: Instant) -> Result<(), Error> {
        let until = Instant::now()
            .checked_add(d)
            .map_or(deadline, |t| t.min(deadline));
        loop {
            self.stopped()?;
            let now = Instant::now();
            if now >= deadline {
                return Err("acme.timeout passed".into());
            }
            if now >= until {
                return Ok(());
            }
            std::thread::sleep(until.saturating_duration_since(now).min(SLICE));
        }
    }
}

/// What an invalid authorization's or order's challenges say went wrong, as `": ..."`.
fn why_invalid(v: &Value) -> String {
    let mut out = String::new();
    if let Some(e) = v.get("error") {
        out = problem(e.to_json().as_bytes());
    }
    if let Some(Value::Array(cs)) = v.get("challenges") {
        for c in cs {
            if let Some(e) = c.get("error") {
                out = problem(e.to_json().as_bytes());
            }
        }
    }
    if out.is_empty() {
        out
    } else {
        format!(": {out}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn problems_read_as_one_line() {
        assert_eq!(
            problem(br#"{"type":"urn:ietf:params:acme:error:badNonce","detail":"stale"}"#),
            "badNonce: stale"
        );
        assert_eq!(problem(br#"{"detail":"no"}"#), "no");
        assert_eq!(problem(b"plain"), "plain");
        let v = parse(
            r#"{"status":"invalid","challenges":[{"type":"http-01","error":{"type":"urn:ietf:params:acme:error:connection","detail":"refused"}}]}"#,
        )
        .unwrap();
        assert_eq!(why_invalid(&v), ": connection: refused");
    }
}
