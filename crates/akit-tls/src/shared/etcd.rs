//! The etcd store (fit's `etcdBackend`): etcd's v3 API through its JSON gateway, on one
//! endpoint at a time, the next after one cannot be reached; keys and values base64'd, as the
//! gateway has them. Locks are leases: the lock's key put with the lease in a transaction only if
//! it was never made, renewed with a keepalive, revoked to give it back.

use std::sync::{Arc, Mutex, PoisonError};
use std::time::{Duration, Instant};

use akio_crypto::base64::STANDARD;
use akio_http::client::ClientRequest;
use akit_config::EtcdStorage;
use akit_config::value::Value;

use super::net::{Http, json_string, parse_json, status_error};
use super::{Backend, CallError, Lease, MAX_KEYS};

/// A call's failure: an answer etcd gave that is not 200 (not tried at another endpoint), or
/// anything else (the endpoint not reached, which moves to the next).
#[derive(Debug)]
pub(crate) enum EtcdError {
    Status(String),
    Other(String),
}

impl EtcdError {
    fn text(self) -> String {
        match self {
            EtcdError::Status(s) | EtcdError::Other(s) => s,
        }
    }
}

#[derive(Default)]
struct State {
    /// The endpoint to use.
    next: usize,
    /// From authenticating, when a user is set.
    token: String,
}

pub(crate) struct Inner {
    opts: EtcdStorage,
    http: Http,
    st: Mutex<State>,
}

/// The etcd store; its leases share its client.
pub(crate) struct EtcdBackend {
    inner: Arc<Inner>,
}

/// A string member of an object.
fn string<'a>(v: &'a Value, key: &str) -> Option<&'a str> {
    match v.get(key) {
        Some(Value::String(s)) => Some(s),
        _ => None,
    }
}

/// `b` as the gateway takes bytes: a base64 JSON string.
fn b64(b: &[u8]) -> String {
    format!("\"{}\"", STANDARD.encode(b))
}

/// The end of the range of the keys starting with `prefix` (fit's `rangeEnd`).
pub(crate) fn range_end(prefix: &[u8]) -> Vec<u8> {
    let mut end = prefix.to_vec();
    while let Some(last) = end.pop() {
        if last < 0xff {
            end.push(last.saturating_add(1));
            return end;
        }
    }
    vec![0]
}

impl EtcdBackend {
    pub(crate) fn new(c: &EtcdStorage) -> Result<EtcdBackend, String> {
        Ok(EtcdBackend {
            inner: Arc::new(Inner {
                opts: c.clone(),
                http: Http::new(&c.tls)?,
                st: Mutex::new(State::default()),
            }),
        })
    }

    #[cfg(test)]
    pub(crate) fn inner(&self) -> &Inner {
        &self.inner
    }
}

impl Inner {
    #[cfg(test)]
    pub(crate) fn next(&self) -> usize {
        self.state().next
    }

    #[cfg(test)]
    pub(crate) fn token(&self) -> String {
        self.state().token.clone()
    }

    fn state(&self) -> std::sync::MutexGuard<'_, State> {
        self.st.lock().unwrap_or_else(PoisonError::into_inner)
    }

    fn endpoints(&self) -> &[String] {
        &self.opts.endpoints
    }

    /// Posts `body` to `path` and reads the answer's first JSON object (a stream, as
    /// `/v3/lease/keepalive` answers, has one a line), authenticating first when a user is set
    /// and no token is held, and again once when etcd refuses the token. An endpoint that cannot
    /// be reached makes the next one used, for this call and the ones after it.
    pub(crate) fn call(
        &self,
        path: &str,
        body: &str,
        deadline: Instant,
    ) -> Result<Value, EtcdError> {
        let mut err = EtcdError::Other("no endpoint".to_owned());
        for _ in 0..self.endpoints().len().max(1) {
            match self.call_one(path, body, deadline) {
                Ok(v) => return Ok(v),
                Err(e @ EtcdError::Status(..)) => return Err(e),
                Err(e) => err = e,
            }
            if Instant::now() >= deadline {
                break;
            }
        }
        Err(err)
    }

    fn call_one(&self, path: &str, body: &str, deadline: Instant) -> Result<Value, EtcdError> {
        for attempt in 0..2 {
            let (ep, mut token) = {
                let st = self.state();
                let eps = self.endpoints();
                let ep = eps
                    .get(st.next.checked_rem(eps.len()).unwrap_or(0))
                    .map(|e| e.trim_end_matches('/').to_owned())
                    .unwrap_or_default();
                (ep, st.token.clone())
            };
            if !self.opts.username.is_empty() && token.is_empty() {
                token = match self.authenticate(&ep, deadline) {
                    Ok(t) => t,
                    Err(e) => {
                        self.failed(&ep, &e);
                        return Err(e);
                    }
                };
            }
            let r = self.post(&ep, path, &token, body, deadline);
            if let Err(EtcdError::Status(msg)) = &r {
                if !self.opts.username.is_empty() && attempt == 0 && msg.contains("token") {
                    self.state().token.clear();
                    continue;
                }
            }
            if let Err(e) = &r {
                self.failed(&ep, e);
            }
            return r;
        }
        Err(EtcdError::Other(format!("POST {path}: the token refused")))
    }

    /// Moves to the next endpoint after `ep` could not be reached.
    fn failed(&self, ep: &str, e: &EtcdError) {
        if matches!(e, EtcdError::Status(..)) {
            return;
        }
        let mut st = self.state();
        let eps = self.endpoints();
        let cur = eps
            .get(st.next.checked_rem(eps.len()).unwrap_or(0))
            .map(|e| e.trim_end_matches('/'));
        if cur == Some(ep) {
            st.next = st.next.wrapping_add(1);
            st.token.clear();
        }
    }

    fn authenticate(&self, ep: &str, deadline: Instant) -> Result<String, EtcdError> {
        let password = std::fs::read(&self.opts.password_file)
            .map_err(|e| EtcdError::Other(format!("{}: {e}", self.opts.password_file)))?;
        let password = String::from_utf8_lossy(&password);
        let body = format!(
            "{{\"name\":{},\"password\":{}}}",
            json_string(&self.opts.username),
            json_string(password.trim_end_matches(['\r', '\n']))
        );
        let out = self.post(ep, "/v3/auth/authenticate", "", &body, deadline)?;
        let token = string(&out, "token").unwrap_or("").to_owned();
        self.state().token.clone_from(&token);
        Ok(token)
    }

    fn post(
        &self,
        ep: &str,
        path: &str,
        token: &str,
        body: &str,
        deadline: Instant,
    ) -> Result<Value, EtcdError> {
        let what = format!("POST {path}");
        let mut req = ClientRequest::post(&format!("{ep}{path}"), body.as_bytes().to_vec())
            .header("Content-Type", "application/json");
        if !token.is_empty() {
            req = req.header("Authorization", token);
        }
        let r = self
            .http
            .call(req, deadline, &what)
            .map_err(EtcdError::Other)?;
        if r.status != 200 {
            return Err(EtcdError::Status(status_error(&r, &what)));
        }
        let first = r
            .body
            .split(|b| *b == b'\n')
            .find(|l| !l.iter().all(u8::is_ascii_whitespace));
        let Some(first) = first else {
            return Err(EtcdError::Other(format!("{what}: an empty answer")));
        };
        parse_json(first, &what).map_err(EtcdError::Other)
    }

    fn field(kv: &Value, name: &str) -> Result<Vec<u8>, String> {
        match kv.get(name) {
            None | Some(Value::Null) => Ok(Vec::new()),
            Some(Value::String(s)) => STANDARD
                .decode(s)
                .map_err(|e| format!("POST /v3/kv/range: a {name} that is not base64: {e}")),
            Some(_) => Err(format!("POST /v3/kv/range: a {name} that is not a string")),
        }
    }

    /// Revokes the lease `id`; one that expired already, and its key with it, is no error.
    fn revoke(&self, id: &str, deadline: Instant) -> Result<(), String> {
        match self.call(
            "/v3/lease/revoke",
            &format!("{{\"ID\":{}}}", json_string(id)),
            deadline,
        ) {
            Ok(_) => Ok(()),
            Err(EtcdError::Status(msg)) if msg.contains("lease not found") => Ok(()),
            Err(e) => Err(e.text()),
        }
    }
}

fn kvs(v: &Value) -> &[Value] {
    match v.get("kvs") {
        Some(Value::Array(a)) => a,
        _ => &[],
    }
}

impl Backend for EtcdBackend {
    fn get(&self, key: &str, deadline: Instant) -> Result<Vec<u8>, CallError> {
        let out = self
            .inner
            .call(
                "/v3/kv/range",
                &format!("{{\"key\":{}}}", b64(key.as_bytes())),
                deadline,
            )
            .map_err(|e| CallError::Failed(e.text()))?;
        match kvs(&out).first() {
            None => Err(CallError::NotFound),
            Some(kv) => Ok(Inner::field(kv, "value")?),
        }
    }

    fn put(&self, key: &str, value: &[u8], deadline: Instant) -> Result<(), String> {
        let body = format!(
            "{{\"key\":{},\"value\":{}}}",
            b64(key.as_bytes()),
            b64(value)
        );
        self.inner
            .call("/v3/kv/put", &body, deadline)
            .map(drop)
            .map_err(EtcdError::text)
    }

    fn del(&self, key: &str, deadline: Instant) -> Result<(), String> {
        let body = format!("{{\"key\":{}}}", b64(key.as_bytes()));
        self.inner
            .call("/v3/kv/deleterange", &body, deadline)
            .map(drop)
            .map_err(EtcdError::text)
    }

    fn list(&self, prefix: &str, deadline: Instant) -> Result<Vec<String>, String> {
        let p = prefix.as_bytes();
        let body = format!(
            "{{\"key\":{},\"range_end\":{},\"keys_only\":true,\"limit\":{MAX_KEYS}}}",
            b64(p),
            b64(&range_end(p))
        );
        let out = self
            .inner
            .call("/v3/kv/range", &body, deadline)
            .map_err(EtcdError::text)?;
        if matches!(out.get("more"), Some(Value::Bool(true))) {
            return Err(format!("more than {MAX_KEYS} keys"));
        }
        kvs(&out)
            .iter()
            .map(|kv| Inner::field(kv, "key").map(|k| String::from_utf8_lossy(&k).into_owned()))
            .collect()
    }

    /// Grants a lease of `ttl`, and puts the lock's key with it if the key has never been made,
    /// in one transaction; the lease is revoked when the key is there.
    fn try_lock(
        &self,
        key: &str,
        ttl: Duration,
        deadline: Instant,
    ) -> Result<Option<Box<dyn Lease>>, String> {
        let grant = self
            .inner
            .call(
                "/v3/lease/grant",
                &format!("{{\"TTL\":{}}}", ttl.as_secs()),
                deadline,
            )
            .map_err(EtcdError::text)?;
        let id = match grant.get("ID") {
            Some(Value::String(s) | Value::Number(s)) if !s.is_empty() => s.clone(),
            _ => {
                return Err(format!(
                    "no lease granted: {}",
                    string(&grant, "error").unwrap_or("")
                ));
            }
        };
        let k = b64(key.as_bytes());
        let txn = format!(
            "{{\"compare\":[{{\"key\":{k},\"target\":\"CREATE\",\"result\":\"EQUAL\",\"create_revision\":\"0\"}}],\
             \"success\":[{{\"request_put\":{{\"key\":{k},\"value\":{},\"lease\":{}}}}}]}}",
            b64(id.as_bytes()),
            json_string(&id)
        );
        let out = match self.inner.call("/v3/kv/txn", &txn, deadline) {
            Ok(v) => v,
            Err(e) => {
                let _ = self.inner.revoke(&id, deadline);
                return Err(e.text());
            }
        };
        if !matches!(out.get("succeeded"), Some(Value::Bool(true))) {
            let _ = self.inner.revoke(&id, deadline);
            return Ok(None);
        }
        Ok(Some(Box::new(EtcdLease {
            inner: Arc::clone(&self.inner),
            id,
        })))
    }

    fn close(&self) {}
}

/// A lock held with the lease `id`.
pub(crate) struct EtcdLease {
    inner: Arc<Inner>,
    id: String,
}

impl Lease for EtcdLease {
    fn renew(&self, deadline: Instant) -> Result<(), String> {
        let out = self
            .inner
            .call(
                "/v3/lease/keepalive",
                &format!("{{\"ID\":{}}}", json_string(&self.id)),
                deadline,
            )
            .map_err(EtcdError::text)?;
        let ttl = out.get("result").and_then(|r| match r.get("TTL") {
            Some(Value::String(s) | Value::Number(s)) => Some(s.as_str()),
            _ => None,
        });
        match ttl {
            None | Some("" | "0") => Err("the lock was lost: its lease expired".to_owned()),
            Some(_) => Ok(()),
        }
    }

    fn release(&self, deadline: Instant) -> Result<(), String> {
        self.inner.revoke(&self.id, deadline)
    }
}
