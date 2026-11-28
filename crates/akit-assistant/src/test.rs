//! `POST /api/providers/test` (I161, fit's `test`): a provider as the Settings page has it, saved
//! or not, tried: its model list, then a short call to a model, each bounded; the answer goes once
//! both are over, and a browser that leaves cancels them. At most [`MAX_TESTS`] at once.

use std::sync::atomic::Ordering;
use std::sync::{Arc, Mutex, PoisonError};
use std::time::{Duration, Instant};

use akio_http::{Context, Detached};
use akit_config::assistant::ProviderKind;
use akit_config::value::Value;
use akit_roles::pages;

use crate::Assistant;
use crate::json::{self, Obj};
use crate::model::{self, Call, Message, ModelInfo, Request, Role, Secret};
use crate::settings::{self, MAX_KEY_BYTES};

/// How many provider tests run at once; one more is answered 429.
pub const MAX_TESTS: usize = 4;
/// The bounds of a test, whatever the provider's own: its list, and its call.
pub const MODELS_WAIT: Duration = Duration::from_secs(15);
/// See [`MODELS_WAIT`].
pub const CALL_WAIT: Duration = Duration::from_secs(30);

/// A test's outcome, fit's `testResult`.
#[derive(Debug, Default)]
struct Outcome {
    models: Vec<ModelInfo>,
    models_ms: u128,
    models_error: String,
    model: String,
    reply: String,
    call_ms: u128,
    call_error: String,
    usage: Option<model::Usage>,
}

impl Outcome {
    fn json(&self) -> String {
        let ok = if self.model.is_empty() {
            self.models_error.is_empty()
        } else {
            self.call_error.is_empty()
        };
        let models: Vec<String> = self.models.iter().map(ModelInfo::json).collect();
        let mut o = Obj::new()
            .bool("ok", ok)
            .raw("models", &format!("[{}]", models.join(",")))
            .num("models_ms", self.models_ms)
            .str_omit("models_error", &self.models_error)
            .str_omit("model", &self.model)
            .str_omit("reply", &self.reply);
        if self.call_ms > 0 {
            o = o.num("call_ms", self.call_ms);
        }
        o = o.str_omit("call_error", &self.call_error);
        if let Some(u) = &self.usage {
            o = o.raw("usage", &u.json());
        }
        o.end()
    }
}

/// A test under way.
struct Test {
    a: Assistant,
    p: model::Provider,
    model: String,
    models_only: bool,
    st: Mutex<TestState>,
}

struct TestState {
    reply: Option<Detached>,
    call: Option<Call>,
    gone: bool,
    out: Outcome,
}

impl Test {
    fn state(&self) -> std::sync::MutexGuard<'_, TestState> {
        self.st.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Keeps the call under way, cancelling it when the browser has gone.
    fn set_call(&self, c: Call) {
        let gone = {
            let mut st = self.state();
            st.call = Some(c.clone());
            st.gone
        };
        if gone {
            c.cancel("the browser went away");
        }
    }

    fn models(self: Arc<Self>) {
        let mut q = self.p.clone();
        q.first_byte_timeout = q.first_byte_timeout.min(MODELS_WAIT);
        q.timeout = q.timeout.min(MODELS_WAIT);
        let start = Instant::now();
        let me = Arc::clone(&self);
        let Some(client) = self.a.client() else {
            self.state().out.models_error = "the assistant has no engine to call with".into();
            return self.end();
        };
        let c = client.models(&q, move |got| {
            let next = {
                let mut st = me.state();
                st.out.models_ms = start.elapsed().as_millis();
                match got {
                    Err(e) => st.out.models_error = e.to_string(),
                    Ok(list) => st.out.models = list,
                }
                let model = if me.model.is_empty() {
                    st.out
                        .models
                        .first()
                        .map(|m| m.id.clone())
                        .unwrap_or_default()
                } else {
                    me.model.clone()
                };
                (!model.is_empty() && !me.models_only).then_some(model)
            };
            match next {
                Some(m) => Arc::clone(&me).chat(m),
                None => me.end(),
            }
        });
        self.set_call(c);
    }

    fn chat(self: Arc<Self>, model: String) {
        let mut q = self.p.clone();
        q.first_byte_timeout = q.first_byte_timeout.min(CALL_WAIT);
        q.timeout = q.timeout.min(CALL_WAIT);
        q.retries = 0;
        self.state().out.model.clone_from(&model);
        let req = Request {
            model,
            system: "You are being tested. Reply with the single word OK.".into(),
            messages: vec![Message::new(Role::User, "ping")],
            max_tokens: 32,
            ..Request::default()
        };
        let start = Instant::now();
        let me = Arc::clone(&self);
        let Some(client) = self.a.client() else {
            return self.end();
        };
        let c = client.stream(
            &q,
            &req,
            64 << 10,
            |_| {},
            move |got| {
                {
                    let mut st = me.state();
                    st.out.call_ms = start.elapsed().as_millis().max(1);
                    match got {
                        Err(e) => st.out.call_error = e.to_string(),
                        Ok(a) => {
                            st.out.reply = clip(&a.text, 200);
                            st.out.usage = Some(a.usage);
                        }
                    }
                }
                me.end();
            },
        );
        self.set_call(c);
    }

    /// Answers the test, and gives its slot back.
    fn end(&self) {
        let (reply, body) = {
            let mut st = self.state();
            (st.reply.take(), st.out.json())
        };
        if let Some(d) = reply {
            d.respond(pages::json(200, body));
            self.a.0.tests.fetch_sub(1, Ordering::SeqCst);
        }
    }
}

fn clip(s: &str, n: usize) -> String {
    if s.chars().count() <= n {
        return s.to_owned();
    }
    let mut out: String = s.chars().take(n).collect();
    out.push('…');
    out
}

impl Assistant {
    /// `POST /api/providers/test`.
    pub(crate) fn test(&self, ctx: &mut Context) {
        let body = match pages::decode(ctx) {
            Ok(b) => b.0,
            Err(r) => return ctx.respond(r),
        };
        let provider = match body.get("provider") {
            Some(v @ Value::Object(_)) => json::compact(v),
            _ => "null".to_owned(),
        };
        let s = match akit_config::assistant::load(
            &format!("{{\"providers\":[{provider}]}}"),
            self.0.o.console.as_ref(),
        ) {
            Ok(s) if s.providers.len() == 1 => s,
            Ok(_) => {
                return ctx.respond(pages::fail(400, "the provider is not valid"));
            }
            Err(e) => {
                return ctx.respond(pages::json(
                    400,
                    settings::errors_body("the provider is not valid", &e),
                ));
            }
        };
        let typed = json::str_of(&body, "key").to_owned();
        if typed.len() > MAX_KEY_BYTES {
            return ctx.respond(pages::fail(400, "the key is too long"));
        }
        let model = json::str_of(&body, "model").to_owned();
        let models_only = matches!(body.get("models_only"), Some(Value::Bool(true)));
        // A slot taken, or 429 (a compare-and-swap, as fetch_update and try_update do).
        let mut n = self.0.tests.load(Ordering::SeqCst);
        loop {
            if n >= MAX_TESTS {
                return ctx.respond(pages::fail(429, "other tests are under way; try again"));
            }
            match self.0.tests.compare_exchange_weak(
                n,
                n.saturating_add(1),
                Ordering::SeqCst,
                Ordering::SeqCst,
            ) {
                Ok(_) => break,
                Err(now) => n = now,
            }
        }
        let Some(p) = s.providers.into_iter().next() else {
            self.0.tests.fetch_sub(1, Ordering::SeqCst);
            return ctx.respond(pages::fail(400, "the provider is not valid"));
        };
        let Some(mut d) = ctx.detach() else {
            self.0.tests.fetch_sub(1, Ordering::SeqCst);
            return;
        };
        let me = self.clone();
        let slot: Arc<Mutex<Option<Arc<Test>>>> = Arc::new(Mutex::new(None));
        let gone = Arc::clone(&slot);
        let gone_flag = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let g2 = Arc::clone(&gone_flag);
        d.on_gone(move || {
            g2.store(true, Ordering::SeqCst);
            let t = gone.lock().unwrap_or_else(PoisonError::into_inner).clone();
            if let Some(t) = t {
                let call = {
                    let mut st = t.state();
                    st.gone = true;
                    st.call.clone()
                };
                if let Some(c) = call {
                    c.cancel("the browser went away");
                }
            }
        });
        // An llm_proxy provider reads its certificate's file: on the pool.
        self.run(d, move |d| {
            let key = if typed.is_empty() {
                me.key(&p.name).map(|(k, _)| k).unwrap_or_default()
            } else {
                Secret::new(typed)
            };
            let mp = match me.resolve(&p, key) {
                Ok(mp) => mp,
                Err(why) => {
                    let out = Outcome {
                        models_error: why,
                        ..Outcome::default()
                    };
                    d.respond(pages::json(200, out.json()));
                    me.0.tests.fetch_sub(1, Ordering::SeqCst);
                    return;
                }
            };
            let model = if model.is_empty() {
                p.models.first().map(|m| m.name.clone()).unwrap_or_default()
            } else {
                model
            };
            let t = Arc::new(Test {
                a: me.clone(),
                p: mp,
                model,
                models_only,
                st: Mutex::new(TestState {
                    reply: Some(d),
                    call: None,
                    gone: gone_flag.load(Ordering::SeqCst),
                    out: Outcome::default(),
                }),
            });
            *slot.lock().unwrap_or_else(PoisonError::into_inner) = Some(Arc::clone(&t));
            t.models();
        });
    }

    /// `p` as a call needs it, with `key`: an `llm_proxy` provider reached on loopback at
    /// `llm.proxy.api`'s port, its certificate, when it has one, pinned. Reads that certificate's
    /// file: on a pool.
    pub(crate) fn resolve(
        &self,
        p: &akit_config::assistant::Provider,
        key: Secret,
    ) -> Result<model::Provider, String> {
        if p.kind != ProviderKind::LlmProxy {
            return Ok(model::Provider::of(p, key, p.url()));
        }
        let Some(api) = self.proxy_api() else {
            return Err(format!(
                "{}: this akit runs no llm.proxy with an api to call",
                p.name
            ));
        };
        let (host, port) = api
            .address
            .rsplit_once(':')
            .ok_or_else(|| format!("{}: llm.proxy.api.address: no port", p.name))?;
        // The port as bound, when the role says: the config's may be 0.
        let bound = self.0.o.proxy_addr.as_ref().and_then(|f| f());
        let port = bound.map_or_else(|| port.to_owned(), |a| a.port().to_string());
        let host = host.trim_start_matches('[').trim_end_matches(']');
        // The API is reached on loopback, whatever address it listens on.
        let host = match host.parse::<std::net::IpAddr>() {
            _ if host.is_empty() => "127.0.0.1".to_owned(),
            Ok(std::net::IpAddr::V4(ip)) if ip.is_unspecified() => "127.0.0.1".to_owned(),
            Ok(std::net::IpAddr::V6(ip)) if ip.is_unspecified() => "[::1]".to_owned(),
            Ok(std::net::IpAddr::V6(ip)) => format!("[{ip}]"),
            _ => host.to_owned(),
        };
        let (scheme, pin) = match &api.cert_file {
            Some(f) => (
                "https",
                Some(leaf_pin(f).map_err(|e| format!("{}: llm.proxy.api.cert_file: {e}", p.name))?),
            ),
            None => ("http", None),
        };
        let mut mp = model::Provider::of(p, key, &format!("{scheme}://{host}:{port}/v1"));
        mp.pin = pin;
        Ok(mp)
    }
}

/// The SHA-256 of the first certificate of a PEM file.
fn leaf_pin(path: &str) -> Result<[u8; 32], String> {
    let data = std::fs::read(path).map_err(|e| e.to_string())?;
    let certs = akio::tls::Certificate::from_pem(&data).map_err(|e| e.to_string())?;
    let leaf = certs.first().ok_or("no certificate in the file")?;
    Ok(akio_crypto::hash::Sha256::digest(leaf.as_der()))
}
