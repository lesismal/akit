//! The Consul store (fit's `consulBackend`): keys in Consul's KV store; locks held by sessions
//! whose TTL is the lock's and which delete the lock's key when they end.

use std::sync::Arc;
use std::time::{Duration, Instant};

use akio_http::client::ClientRequest;
use akio_http::message::Method;
use akit_config::ConsulStorage;
use akit_config::value::Value;

use super::net::{Http, json_string, parse_json, path_escape, query_escape, status_error};
use super::{Backend, CallError, Lease, MAX_KEYS};

pub(crate) struct Inner {
    opts: ConsulStorage,
    token: String,
    http: Http,
}

/// The Consul store; its sessions share its client.
pub(crate) struct ConsulBackend {
    inner: Arc<Inner>,
}

/// The API's path of `key`, escaped part by part.
fn kv_path(key: &str) -> String {
    let parts: Vec<String> = key.split('/').map(path_escape).collect();
    format!("/v1/kv/{}", parts.join("/"))
}

impl ConsulBackend {
    pub(crate) fn new(c: &ConsulStorage) -> Result<ConsulBackend, String> {
        let token = if c.token_file.is_empty() {
            String::new()
        } else {
            let data =
                std::fs::read(&c.token_file).map_err(|e| format!("{}: {e}", c.token_file))?;
            String::from_utf8_lossy(&data).trim().to_owned()
        };
        Ok(ConsulBackend {
            inner: Arc::new(Inner {
                opts: c.clone(),
                token,
                http: Http::new(&c.tls)?,
            }),
        })
    }
}

impl Inner {
    /// Sends `method` to `path` with `query` and `body`, and returns the answer's body, or
    /// `None` for a 404, which is no error.
    fn call(
        &self,
        method: Method,
        path: &str,
        query: &[(&str, &str)],
        body: Option<&[u8]>,
        deadline: Instant,
    ) -> Result<Option<Vec<u8>>, String> {
        let mut q: Vec<(String, String)> = query
            .iter()
            .map(|(k, v)| ((*k).to_owned(), (*v).to_owned()))
            .collect();
        if !self.opts.datacenter.is_empty() {
            q.push(("dc".to_owned(), self.opts.datacenter.clone()));
        }
        // Sorted by key, as Go's url.Values.Encode writes them.
        q.sort();
        let mut url = format!("{}{path}", self.opts.endpoint.trim_end_matches('/'));
        for (i, (k, v)) in q.iter().enumerate() {
            url.push(if i == 0 { '?' } else { '&' });
            url.push_str(&query_escape(k));
            url.push('=');
            url.push_str(&query_escape(v));
        }
        let what = format!("{} {path}", method.as_str());
        let mut req = ClientRequest::new(method, &url);
        if let Some(b) = body {
            req = req.body(b.to_vec());
        }
        if !self.token.is_empty() {
            req = req.header("X-Consul-Token", &self.token);
        }
        let r = self.http.call(req, deadline, &what)?;
        match r.status {
            200 => Ok(Some(r.body)),
            404 => Ok(None),
            _ => Err(status_error(&r, &what)),
        }
    }

    /// Destroys `session`, which deletes the lock's key.
    fn destroy(&self, session: &str, deadline: Instant) -> Result<(), String> {
        let path = format!("/v1/session/destroy/{}", path_escape(session));
        self.call(Method::Put, &path, &[], None, deadline).map(drop)
    }
}

impl Backend for ConsulBackend {
    fn get(&self, key: &str, deadline: Instant) -> Result<Vec<u8>, CallError> {
        match self
            .inner
            .call(Method::Get, &kv_path(key), &[("raw", "")], None, deadline)?
        {
            Some(v) => Ok(v),
            None => Err(CallError::NotFound),
        }
    }

    fn put(&self, key: &str, value: &[u8], deadline: Instant) -> Result<(), String> {
        let data = self
            .inner
            .call(Method::Put, &kv_path(key), &[], Some(value), deadline)?
            .unwrap_or_default();
        if String::from_utf8_lossy(&data).trim() != "true" {
            return Err(format!(
                "PUT {key}: Consul answered {:?}",
                String::from_utf8_lossy(&data)
            ));
        }
        Ok(())
    }

    fn del(&self, key: &str, deadline: Instant) -> Result<(), String> {
        self.inner
            .call(Method::Delete, &kv_path(key), &[], None, deadline)
            .map(drop)
    }

    fn list(&self, prefix: &str, deadline: Instant) -> Result<Vec<String>, String> {
        let Some(data) = self.inner.call(
            Method::Get,
            &kv_path(prefix),
            &[("keys", "")],
            None,
            deadline,
        )?
        else {
            return Ok(Vec::new());
        };
        let what = format!("GET {prefix}?keys");
        let Value::Array(items) = parse_json(&data, &what)? else {
            return Err(format!("{what}: not an array of keys"));
        };
        if items.len() > MAX_KEYS {
            return Err(format!("more than {MAX_KEYS} keys"));
        }
        items
            .into_iter()
            .map(|v| match v {
                Value::String(s) => Ok(s),
                _ => Err(format!("{what}: a key that is not a string")),
            })
            .collect()
    }

    /// Makes a session of `ttl` and acquires the lock's key with it; the session is destroyed
    /// when another holds the key.
    fn try_lock(
        &self,
        key: &str,
        ttl: Duration,
        deadline: Instant,
    ) -> Result<Option<Box<dyn Lease>>, String> {
        let body = format!(
            "{{\"Name\":{},\"TTL\":\"{}s\",\"Behavior\":\"delete\",\"LockDelay\":\"0s\"}}",
            json_string(&format!("akit {key}")),
            ttl.as_secs()
        );
        let data = self
            .inner
            .call(
                Method::Put,
                "/v1/session/create",
                &[],
                Some(body.as_bytes()),
                deadline,
            )?
            .unwrap_or_default();
        let id = parse_json(&data, "PUT /v1/session/create")
            .ok()
            .and_then(|v| match v.get("ID") {
                Some(Value::String(s)) if !s.is_empty() => Some(s.clone()),
                _ => None,
            })
            .ok_or_else(|| {
                format!(
                    "PUT /v1/session/create: no session in {:?}",
                    String::from_utf8_lossy(&data)
                )
            })?;
        let acquired = self.inner.call(
            Method::Put,
            &kv_path(key),
            &[("acquire", &id)],
            Some(id.as_bytes()),
            deadline,
        );
        match acquired {
            Err(e) => {
                let _ = self.inner.destroy(&id, deadline);
                Err(e)
            }
            Ok(data)
                if String::from_utf8_lossy(data.as_deref().unwrap_or_default()).trim()
                    != "true" =>
            {
                let _ = self.inner.destroy(&id, deadline);
                Ok(None)
            }
            Ok(_) => Ok(Some(Box::new(ConsulLease {
                inner: Arc::clone(&self.inner),
                session: id,
            }))),
        }
    }

    fn close(&self) {}
}

/// A lock held by the session `session`.
struct ConsulLease {
    inner: Arc<Inner>,
    session: String,
}

impl Lease for ConsulLease {
    fn renew(&self, deadline: Instant) -> Result<(), String> {
        let path = format!("/v1/session/renew/{}", path_escape(&self.session));
        match self.inner.call(Method::Put, &path, &[], None, deadline)? {
            Some(_) => Ok(()),
            None => Err(format!(
                "the lock was lost: its session {} expired",
                self.session
            )),
        }
    }

    /// Destroys the session, which deletes the lock's key.
    fn release(&self, deadline: Instant) -> Result<(), String> {
        self.inner.destroy(&self.session, deadline)
    }
}
