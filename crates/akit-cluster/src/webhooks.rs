//! Webhooks (I138, fit's `cluster/webhooks.go`): where alerts are sent, in fit's formats (its
//! JSON, Slack, Discord, Feishu, DingTalk, WeCom, or a template of one's own), signed with the
//! webhook's secret, each through a sender of its own on akio's asynchronous client: one
//! notification at a time, 64 waiting at most (the oldest dropped), each within the webhook's
//! timeout. Its URL is https, or http to a loopback address; its URL's path and query and its
//! secret are never answered, logged or kept in a message.
//!
//! The payload and the signature are fit's, with akit's names (decision 037): the source
//! `akit-cluster`, the headers `X-Akit-Signature` and `X-Akit-Timestamp` (decision 041, 10).

use std::collections::VecDeque;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use akio::tls::ClientConfig;
use akio_crypto::base64::STANDARD;
use akio_crypto::hash::Sha256;
use akio_crypto::mac::Hmac;
use akio_http::Context;
use akio_http::client::{Client, ClientOptions, ClientRequest};
use akit_config::value::Value;
use akit_roles::pages::json as answer;

use crate::Manager;
use crate::Refusal;
use crate::alerts::{Alert, FIRING, field, lock, printable, time_of, time_or};
use crate::api::clip;
use crate::json::{self, Obj, quoted};
use crate::member::{MAX_NAME_BYTES, MAX_URL_BYTES, host_of, loopback, name_ok};
use crate::node::Log;

/// The formats.
pub const FORMATS: &[&str] = &[
    "json", "slack", "discord", "feishu", "dingtalk", "wecom", "custom",
];
/// The notifications waiting for a webhook at most.
pub const MAX_QUEUE: usize = 64;
const MAX_TEMPLATE: usize = 4 << 10;
const MAX_SECRET: usize = 1024;
const MAX_NOTIFY_ALERTS: usize = 50;
const MAX_TEXT_ALERTS: usize = 20;
const MAX_ANSWER: usize = 64 << 10;
const DEFAULT_TIMEOUT_MS: i64 = 10_000;
const MIN_TIMEOUT_MS: i64 = 1000;
const MAX_TIMEOUT_MS: i64 = 60_000;
/// The signature's header fields, fit's renamed (decision 037).
pub const HEADER_SIGNATURE: &str = "X-Akit-Signature";
/// See [`HEADER_SIGNATURE`].
pub const HEADER_TIMESTAMP: &str = "X-Akit-Timestamp";

/// A webhook: fit's `Webhook`.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Webhook {
    /// Its name.
    pub name: String,
    /// Where it is sent.
    pub url: String,
    /// What signs it.
    pub secret: String,
    /// One of [`FORMATS`].
    pub format: String,
    /// A custom webhook's body, or a chat's line of each alert.
    pub template: String,
    /// How long a sending takes at most.
    pub timeout_ms: i64,
    /// Whether it is turned off.
    pub disabled: bool,
    /// Whether alerts resolved are sent too.
    pub send_resolved: bool,
    /// When it was added and last changed.
    pub created: Option<SystemTime>,
    /// See [`created`](Webhook::created).
    pub updated: Option<SystemTime>,
}

/// The URL's scheme and host, which is all of it that is answered.
#[must_use]
pub fn url_host(raw: &str) -> String {
    raw.split_once("://")
        .map(|(s, rest)| format!("{s}://{}", rest.split(['/', '?']).next().unwrap_or("")))
        .unwrap_or_default()
}

/// `msg` with `raw`'s path and query taken out: fit's `redactURL`.
fn redact_url(msg: &str, raw: &str) -> String {
    if raw.is_empty() {
        return msg.to_owned();
    }
    let host = url_host(raw);
    let mut out = msg.replace(raw, &format!("{host}/…"));
    if let Some(rest) = raw.strip_prefix(&host).filter(|r| r.len() > 1) {
        out = out.replace(rest, "/…");
    }
    if let Some((_, q)) = raw.split_once('?').filter(|(_, q)| !q.is_empty()) {
        out = out.replace(q, "…");
    }
    out
}

/// Checks a webhook's URL: fit's `webhookURL`.
fn check_url(raw: &str) -> Result<(), Refusal> {
    if raw.is_empty() || raw.len() > MAX_URL_BYTES {
        return Err(field(
            "url",
            format!("the URL is 1 to {MAX_URL_BYTES} bytes"),
        ));
    }
    let bad = || {
        field(
            "url",
            "the URL is not an http or https URL without a user or a fragment",
        )
    };
    let (scheme, rest) = raw.split_once("://").ok_or_else(bad)?;
    if rest.contains('#') || rest.chars().any(char::is_whitespace) {
        return Err(bad());
    }
    let authority = rest.split(['/', '?']).next().unwrap_or("");
    if authority.is_empty() || authority.contains('@') {
        return Err(bad());
    }
    let host = host_of(authority).ok_or_else(bad)?;
    match scheme {
        "https" => Ok(()),
        "http" if loopback(&host) => Ok(()),
        "http" => Err(field(
            "url",
            "the URL is http to a host that is not a loopback address: use https, so that the \
             alerts and the URL's token do not cross a network in the clear",
        )),
        _ => Err(field("url", "the URL is not an http or https URL")),
    }
}

impl Webhook {
    /// A webhook as sent, `old` the one it changes: fit's `WebhookInput.webhook`.
    pub(crate) fn of(
        v: &Value,
        old: Option<&Webhook>,
        now: SystemTime,
    ) -> Result<Webhook, Refusal> {
        let clear = json::bool_of(v, "clear_secret");
        let mut w = Webhook {
            name: json::str_of(v, "name").to_owned(),
            url: json::str_of(v, "url").to_owned(),
            secret: json::str_of(v, "secret").to_owned(),
            format: json::str_of(v, "format").to_owned(),
            template: json::str_of(v, "template").to_owned(),
            timeout_ms: crate::rollouts::whole(v, "timeout_ms", "timeout_ms")?,
            disabled: json::bool_of(v, "disabled"),
            send_resolved: !matches!(v.get("send_resolved"), Some(Value::Bool(false))),
            created: Some(now),
            updated: Some(now),
        };
        if let Some(o) = old {
            w.created = o.created;
            if w.name.is_empty() {
                w.name.clone_from(&o.name);
            }
            if w.url.is_empty() {
                w.url.clone_from(&o.url);
            }
            if w.secret.is_empty() && !clear {
                w.secret.clone_from(&o.secret);
            }
        }
        if clear {
            w.secret.clear();
        }
        if w.format.is_empty() {
            "json".clone_into(&mut w.format);
        }
        if w.timeout_ms == 0 {
            w.timeout_ms = DEFAULT_TIMEOUT_MS;
        }
        w.check()?;
        Ok(w)
    }

    fn check(&self) -> Result<(), Refusal> {
        if self.name.is_empty() || self.name.len() > MAX_NAME_BYTES || !name_ok(&self.name) {
            return Err(field(
                "name",
                format!(
                    "{} is not a name: 1 to {MAX_NAME_BYTES} letters, digits, '_', '.' and '-', \
                     starting with a letter or digit",
                    quoted(&self.name)
                ),
            ));
        }
        if !FORMATS.contains(&self.format.as_str()) {
            return Err(field(
                "format",
                format!(
                    "{} is not a format: {}",
                    quoted(&self.format),
                    FORMATS.join(", ")
                ),
            ));
        }
        if self.secret.len() > MAX_SECRET || !printable(&self.secret) {
            return Err(field(
                "secret",
                format!("a secret is at most {MAX_SECRET} bytes of printable characters"),
            ));
        }
        if self.template.len() > MAX_TEMPLATE {
            return Err(field(
                "template",
                format!("a template is at most {MAX_TEMPLATE} bytes of UTF-8"),
            ));
        }
        if !(MIN_TIMEOUT_MS..=MAX_TIMEOUT_MS).contains(&self.timeout_ms) {
            return Err(field("timeout_ms", "a webhook's timeout is 1s to 1m0s"));
        }
        if self.format == "custom" && self.template.is_empty() {
            return Err(field(
                "template",
                "a custom webhook's template is its body: JSON with ${text}, ${status}, \
                 ${count}, ${manager} and ${alerts}",
            ));
        }
        check_url(&self.url)?;
        if self.format == "custom" {
            let body = custom_body(
                &self.template,
                "[FIRING] \"akit\"\n• a",
                FIRING,
                "m",
                1,
                r#"[{"rule":"r"}]"#,
            );
            if json::parse(body.as_bytes()).is_none() {
                return Err(field(
                    "template",
                    format!(
                        "the template does not make JSON once its ${{...}} are replaced: {}",
                        clip(&body, 120)
                    ),
                ));
            }
        }
        Ok(())
    }

    /// As the store keeps it, its secret with it.
    pub(crate) fn stored(&self) -> String {
        let mut o = Obj::new()
            .str("name", &self.name)
            .str("url", &self.url)
            .str_omit("secret", &self.secret)
            .str("format", &self.format)
            .str_omit("template", &self.template)
            .num("timeout_ms", self.timeout_ms);
        if self.disabled {
            o = o.bool("disabled", true);
        }
        o = o.bool("send_resolved", self.send_resolved);
        o = time_or(o, "created", self.created);
        time_or(o, "updated", self.updated).end()
    }

    pub(crate) fn from_value(v: &Value) -> Webhook {
        Webhook {
            name: json::str_of(v, "name").to_owned(),
            url: json::str_of(v, "url").to_owned(),
            secret: json::str_of(v, "secret").to_owned(),
            format: json::str_of(v, "format").to_owned(),
            template: json::str_of(v, "template").to_owned(),
            timeout_ms: json::int_of(v, "timeout_ms"),
            disabled: json::bool_of(v, "disabled"),
            send_resolved: json::bool_of(v, "send_resolved"),
            created: time_of(v, "created"),
            updated: time_of(v, "updated"),
        }
    }

    /// As the API answers it: its URL's host, whether a secret is set, and how its sendings go:
    /// fit's `WebhookView`.
    pub(crate) fn view(&self, status: Option<String>) -> String {
        let mut o = Obj::new()
            .str("name", &self.name)
            .str("url_host", &url_host(&self.url))
            .bool("secret_set", !self.secret.is_empty())
            .str("format", &self.format)
            .str_omit("template", &self.template)
            .num("timeout_ms", self.timeout_ms);
        if self.disabled {
            o = o.bool("disabled", true);
        }
        o = o.bool("send_resolved", self.send_resolved);
        o = time_or(o, "created", self.created);
        o = time_or(o, "updated", self.updated);
        o.raw(
            "status",
            &status.unwrap_or_else(|| Status::default().json(0)),
        )
        .end()
    }

    /// What changed from `self` to `b`, by name: fit's `webhookChanges`.
    pub(crate) fn changes(&self, b: &Webhook) -> String {
        let out: Vec<&str> = [
            (self.name != b.name, "name"),
            (self.url != b.url, "url"),
            (self.secret != b.secret, "secret"),
            (self.format != b.format, "format"),
            (self.template != b.template, "template"),
            (self.timeout_ms != b.timeout_ms, "timeout"),
            (self.disabled != b.disabled, "disabled"),
            (self.send_resolved != b.send_resolved, "send_resolved"),
        ]
        .iter()
        .filter(|(c, _)| *c)
        .map(|(_, n)| *n)
        .collect();
        if out.is_empty() {
            "nothing".to_owned()
        } else {
            out.join(", ")
        }
    }
}

/// How a sending went: fit's `WebhookResult`.
#[derive(Debug, Clone, Default)]
pub(crate) struct Outcome {
    ok: bool,
    status: u16,
    error: String,
    took_ms: u64,
}

impl Outcome {
    fn json(&self) -> String {
        let mut o = Obj::new().bool("ok", self.ok);
        if self.status != 0 {
            o = o.num("status", self.status);
        }
        o.str_omit("error", &self.error)
            .num("took_ms", self.took_ms)
            .end()
    }
}

/// How a webhook's sendings went: fit's `WebhookStatus`.
#[derive(Debug, Clone, Default)]
struct Status {
    sent: u64,
    failed: u64,
    dropped: u64,
    last_at: Option<SystemTime>,
    last_ok: Option<SystemTime>,
    last_status: u16,
    last_error: String,
    last_took_ms: u64,
}

impl Status {
    fn json(&self, queued: usize) -> String {
        let mut o = Obj::new()
            .num("sent", self.sent)
            .num("failed", self.failed)
            .num("dropped", self.dropped)
            .num("queued", queued);
        o = time_or(o, "last_at", self.last_at);
        o = time_or(o, "last_ok", self.last_ok);
        if self.last_status != 0 {
            o = o.num("last_status", self.last_status);
        }
        o = o.str_omit("last_error", &self.last_error);
        if self.last_took_ms != 0 {
            o = o.num("last_took_ms", self.last_took_ms);
        }
        o.end()
    }
}

type Then = Box<dyn FnOnce(Outcome) + Send>;

/// A notification: alerts firing or resolved, or a test.
pub(crate) struct Notice {
    pub(crate) status: &'static str,
    pub(crate) alerts: Vec<Alert>,
    test: bool,
    done: Option<Then>,
}

impl Notice {
    pub(crate) fn of(status: &'static str, alerts: Vec<Alert>) -> Notice {
        Notice {
            status,
            alerts,
            test: false,
            done: None,
        }
    }

    fn finish(&mut self, o: Outcome) {
        if let Some(f) = self.done.take() {
            f(o);
        }
    }
}

#[derive(Default)]
struct Queue {
    queue: VecDeque<Notice>,
    busy: bool,
    closed: bool,
    st: Status,
}

/// A webhook's sender: fit's `sender`.
pub(crate) struct Sender {
    pub(crate) hook: Webhook,
    client: Client,
    manager: String,
    log: Log,
    q: Mutex<Queue>,
}

impl Sender {
    fn new(m: &Manager, hook: Webhook) -> Sender {
        let t = crate::alerts::ms(hook.timeout_ms);
        let mut opts = ClientOptions::default();
        opts.max_head_bytes = crate::transport::MAX_HEADER_BYTES;
        opts.max_body_bytes = MAX_ANSWER;
        opts.idle_timeout = Some(crate::transport::IDLE_CONN_TIMEOUT);
        opts.max_idle_per_host = 1;
        opts.max_idle = 1;
        opts.response_header_timeout = Some(t);
        opts.happy_eyeballs.connect_timeout = t.min(crate::transport::MAX_DIAL_TIMEOUT);
        let mut client = Client::new(Arc::clone(&m.0.o.engine))
            .with_options(opts)
            .with_timeout(t);
        if hook.url.starts_with("https://") {
            if let Ok(cfg) = ClientConfig::builder()
                .resumption(false)
                .roots(m.0.system.clone())
                .build()
            {
                client = client.with_tls(cfg);
            }
        }
        Sender {
            hook,
            client,
            manager: m.0.o.node_id.clone(),
            log: Arc::clone(&m.0.o.log),
            q: Mutex::new(Queue::default()),
        }
    }

    pub(crate) fn status(&self) -> String {
        let q = lock(&self.q);
        q.st.json(q.queue.len().saturating_add(usize::from(q.busy)))
    }

    /// Queues `n`, dropping the oldest past [`MAX_QUEUE`]: fit's `enqueue`.
    pub(crate) fn enqueue(self: &Arc<Self>, mut n: Notice) {
        let (dropped, next) = {
            let mut q = lock(&self.q);
            if q.closed {
                drop(q);
                n.finish(Outcome {
                    error: "the webhook was changed or removed".to_owned(),
                    ..Outcome::default()
                });
                return;
            }
            let dropped = if q.queue.len() >= MAX_QUEUE {
                q.st.dropped = q.st.dropped.saturating_add(1);
                q.queue.pop_front()
            } else {
                None
            };
            q.queue.push_back(n);
            (dropped, next_of(&mut q))
        };
        if let Some(mut d) = dropped {
            (self.log)(format!(
                "cluster: the webhook {:?} has {MAX_QUEUE} notifications waiting: the oldest was \
                 dropped",
                self.hook.name
            ));
            d.finish(Outcome {
                error: "dropped: the webhook had too many notifications waiting".to_owned(),
                ..Outcome::default()
            });
        }
        if let Some(n) = next {
            self.send(n);
        }
    }

    fn send(self: &Arc<Self>, n: Notice) {
        let start = Instant::now();
        let (target, headers, body) = render(&self.hook, &n, SystemTime::now(), &self.manager);
        let mut req = ClientRequest::new(akio_http::Method::Post, &target)
            .timeout(crate::alerts::ms(self.hook.timeout_ms));
        for (k, v) in headers {
            req = req.header(&k, &v);
        }
        req = req.body(body.into_bytes());
        let me = Arc::clone(self);
        let mut n = Some(n);
        self.client.request(req, move |r| {
            let res = match r {
                Err(e) => Outcome {
                    error: e.to_string(),
                    ..Outcome::default()
                },
                Ok(resp) => {
                    let ok = (200..300).contains(&resp.status);
                    let text = String::from_utf8_lossy(&resp.body).trim().to_owned();
                    let mut o = Outcome {
                        ok,
                        status: resp.status,
                        ..Outcome::default()
                    };
                    if !ok {
                        o.error =
                            format!("the webhook answered {}: {}", resp.status, clip(&text, 200));
                    } else if let Some(msg) = chat_refusal(&me.hook.format, &resp.body) {
                        o.ok = false;
                        o.error = format!("the chat service refused it: {}", clip(&msg, 200));
                    }
                    o
                }
            };
            if let Some(n) = n.take() {
                me.finish(n, res, start);
            }
        });
    }

    fn finish(self: &Arc<Self>, mut n: Notice, mut res: Outcome, start: Instant) {
        res.took_ms = json::millis(start.elapsed());
        res.error = redact_url(&res.error, &self.hook.url);
        if !self.hook.secret.is_empty() {
            res.error = res.error.replace(&self.hook.secret, "••••");
        }
        let now = SystemTime::now();
        let next = {
            let mut q = lock(&self.q);
            let st = &mut q.st;
            st.last_at = Some(now);
            st.last_status = res.status;
            st.last_error.clone_from(&res.error);
            st.last_took_ms = res.took_ms;
            if res.ok {
                st.sent = st.sent.saturating_add(1);
                st.last_ok = Some(now);
            } else {
                st.failed = st.failed.saturating_add(1);
            }
            q.busy = false;
            next_of(&mut q)
        };
        if !res.ok {
            (self.log)(format!(
                "cluster: the webhook {:?} was not sent a {} notification: {}",
                self.hook.name, n.status, res.error
            ));
        }
        n.finish(res);
        if let Some(n) = next {
            self.send(n);
        }
    }

    /// Ends it: what waits is finished as not sent.
    pub(crate) fn close(&self) {
        let q = {
            let mut q = lock(&self.q);
            q.closed = true;
            std::mem::take(&mut q.queue)
        };
        for mut n in q {
            n.finish(Outcome {
                error: "the webhook was changed or removed".to_owned(),
                ..Outcome::default()
            });
        }
    }
}

fn next_of(q: &mut Queue) -> Option<Notice> {
    if q.busy {
        return None;
    }
    let n = q.queue.pop_front()?;
    q.busy = true;
    Some(n)
}

/// The error a chat service answers with 200: fit's `chatRefusal`.
fn chat_refusal(format: &str, body: &[u8]) -> Option<String> {
    if !matches!(format, "feishu" | "dingtalk" | "wecom") {
        return None;
    }
    let b = json::parse(body)?;
    for (code, msg) in [("errcode", "errmsg"), ("code", "msg")] {
        if matches!(b.get(code), Some(Value::Number(n)) if n != "0") {
            return Some(format!(
                "{} {}",
                json::int_of(&b, code),
                json::str_of(&b, msg)
            ));
        }
    }
    None
}

/// A JSON string's inside, escaped.
fn esc(s: &str) -> String {
    let q = quoted(s);
    q.get(1..q.len().saturating_sub(1)).unwrap_or("").to_owned()
}

fn custom_body(
    tpl: &str,
    text: &str,
    status: &str,
    manager: &str,
    count: usize,
    alerts: &str,
) -> String {
    tpl.replace("${text}", &esc(text))
        .replace("${status}", &esc(status))
        .replace("${manager}", &esc(manager))
        .replace("${count}", &count.to_string())
        .replace("${alerts}", alerts)
}

fn hmac(key: &[u8], parts: &[&[u8]]) -> Vec<u8> {
    let mut m = Hmac::<Sha256>::new(key);
    for p in parts {
        m.update(p);
    }
    m.finalize().as_ref().to_vec()
}

/// The target, header fields and body of `n` to `w`: fit's `render`.
fn render(
    w: &Webhook,
    n: &Notice,
    now: SystemTime,
    manager: &str,
) -> (String, Vec<(String, String)>, String) {
    let host = akit_config::roles::host_name();
    let status = if n.test { "test" } else { n.status };
    let text = notice_text(w, n, &host);
    let list: Vec<&Alert> = n.alerts.iter().take(MAX_NOTIFY_ALERTS).collect();
    let more = n.alerts.len().saturating_sub(MAX_NOTIFY_ALERTS);
    let alerts = format!(
        "[{}]",
        list.iter().map(|a| a.json()).collect::<Vec<_>>().join(",")
    );
    let mut h = vec![
        ("Content-Type".to_owned(), "application/json".to_owned()),
        ("User-Agent".to_owned(), "akit-cluster-manager".to_owned()),
    ];
    let mut target = w.url.clone();
    let d = now.duration_since(UNIX_EPOCH).unwrap_or_default();
    let ts = d.as_secs().to_string();
    let text_of = |n: usize| clip(&text, n);
    let body = match w.format.as_str() {
        "slack" => Obj::new().str("text", &text_of(3800)).end(),
        "discord" => Obj::new().str("content", &text_of(1900)).end(),
        "wecom" => Obj::new()
            .str("msgtype", "text")
            .raw("text", &Obj::new().str("content", &text_of(2000)).end())
            .end(),
        "feishu" => {
            let mut o = Obj::new()
                .str("msg_type", "text")
                .raw("content", &Obj::new().str("text", &text_of(3800)).end());
            if !w.secret.is_empty() {
                let key = format!("{ts}\n{}", w.secret);
                o = o
                    .str("timestamp", &ts)
                    .str("sign", &STANDARD.encode(hmac(key.as_bytes(), &[])));
            }
            o.end()
        }
        "dingtalk" => {
            if !w.secret.is_empty() {
                let ms = d.as_millis().to_string();
                let sig = STANDARD.encode(hmac(
                    w.secret.as_bytes(),
                    &[format!("{ms}\n{}", w.secret).as_bytes()],
                ));
                let sep = if target.contains('?') { '&' } else { '?' };
                target = format!("{target}{sep}timestamp={ms}&sign={}", query_escape(&sig));
            }
            Obj::new()
                .str("msgtype", "text")
                .raw("text", &Obj::new().str("content", &text_of(3800)).end())
                .end()
        }
        "custom" => custom_body(&w.template, &text, status, manager, n.alerts.len(), &alerts),
        _ => {
            let mut o = Obj::new()
                .num("version", 1)
                .str("source", "akit-cluster")
                .str("manager", manager)
                .str("manager_host", &host)
                .str("status", status);
            if n.test {
                o = o.bool("test", true);
            }
            o = o
                .raw("sent", &json::time(now))
                .str("text", &text)
                .raw("alerts", &alerts);
            if more > 0 {
                o = o.num("more", more);
            }
            o.end()
        }
    };
    if !w.secret.is_empty() && w.format != "feishu" && w.format != "dingtalk" {
        let sig = hmac(
            w.secret.as_bytes(),
            &[format!("{ts}.").as_bytes(), body.as_bytes()],
        );
        h.push((HEADER_TIMESTAMP.to_owned(), ts));
        h.push((
            HEADER_SIGNATURE.to_owned(),
            format!("sha256={}", akit_roles::linktoken::hex(&sig)),
        ));
    }
    (target, h, body)
}

/// `s` as Go's `url.QueryEscape` writes it.
fn query_escape(s: &str) -> String {
    let mut out = String::new();
    for b in s.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(char::from(b));
            }
            b' ' => out.push('+'),
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}

/// The notification in words: fit's `noticeText`, with akit's name.
fn notice_text(w: &Webhook, n: &Notice, host: &str) -> String {
    let mut b = if n.test {
        format!(
            "[TEST] akit cluster on {host}: a test of the webhook {}",
            w.name
        )
    } else {
        let s = if n.alerts.len() == 1 { "" } else { "s" };
        format!(
            "[{}] akit cluster on {host}: {} alert{s}",
            n.status.to_uppercase(),
            n.alerts.len()
        )
    };
    let tpl = if w.format == "json" || w.format == "custom" {
        ""
    } else {
        w.template.as_str()
    };
    for (i, a) in n.alerts.iter().enumerate() {
        if i == MAX_TEXT_ALERTS {
            b.push_str(&format!(
                "\n… and {} more",
                n.alerts.len().saturating_sub(MAX_TEXT_ALERTS)
            ));
            break;
        }
        b.push('\n');
        b.push_str(&alert_line(tpl, a));
    }
    b
}

/// An alert's line: fit's `alertLine`.
fn alert_line(tpl: &str, a: &Alert) -> String {
    let on = if a.node.is_empty() {
        &a.subject
    } else {
        &a.node
    };
    if tpl.is_empty() {
        let on = if on.is_empty() {
            String::new()
        } else {
            format!(" · {on}")
        };
        return format!("• [{}] {}{on}: {}", a.severity, a.rule, a.summary);
    }
    tpl.replace("${status}", &a.state)
        .replace("${rule}", &a.rule)
        .replace("${kind}", &a.kind)
        .replace("${severity}", &a.severity)
        .replace("${node}", &a.node)
        .replace("${subject}", &a.subject)
        .replace("${on}", on)
        .replace("${summary}", &a.summary)
        .replace(
            "${since}",
            &crate::alerts::rfc3339(a.since.unwrap_or(UNIX_EPOCH)),
        )
        .replace("${id}", &a.id)
}

impl Manager {
    /// Makes the senders those of the webhooks: one unchanged keeps its sender, its queue and
    /// its counts; one changed or removed is closed. fit's `syncSenders`.
    pub(crate) fn sync_senders(&self) {
        let gone = {
            let mut al = lock(&self.0.al);
            let mut keep = std::collections::HashMap::new();
            let hooks = al.cfg.webhooks.clone();
            for w in hooks {
                let s = match al.senders.remove(&w.name) {
                    Some(s) if s.hook == w => s,
                    Some(s) => {
                        s.close();
                        Arc::new(Sender::new(self, w.clone()))
                    }
                    None => Arc::new(Sender::new(self, w.clone())),
                };
                keep.insert(w.name.clone(), s);
            }
            let gone: Vec<Arc<Sender>> = al.senders.drain().map(|(_, s)| s).collect();
            al.senders = keep;
            gone
        };
        for s in gone {
            s.close();
        }
    }

    /// Closes every sender: the manager's close.
    pub(crate) fn close_senders(&self) {
        let all: Vec<Arc<Sender>> = lock(&self.0.al).senders.drain().map(|(_, s)| s).collect();
        for s in all {
            s.close();
        }
    }
}

/// `POST /api/alerts/webhooks/<name>/test`: a test sent, answered once it went: fit's
/// `TestWebhook`.
pub(crate) fn test(m: &Manager, ctx: &mut Context, user: &str, name: &str) {
    let Some(s) = lock(&m.0.al).senders.get(name).cloned() else {
        ctx.respond(akit_roles::pages::fail(
            404,
            &format!("no webhook is named {}", quoted(name)),
        ));
        return;
    };
    m.record(
        user,
        vec![
            (
                "action".to_owned(),
                Value::String(crate::alerts::WEBHOOK_TEST.to_owned()),
            ),
            ("detail".to_owned(), Value::String(name.to_owned())),
        ],
    );
    let Some(d) = ctx.detach() else { return };
    let now = SystemTime::now();
    let sample = Alert {
        id: "test".to_owned(),
        rule: "test".to_owned(),
        kind: crate::alerts::MEMBER_DOWN.to_owned(),
        severity: "info".to_owned(),
        state: FIRING.to_owned(),
        summary: format!("a test of the webhook {name}, sent by {user}"),
        since: Some(now),
        fired: Some(now),
        ..Alert::default()
    };
    let d = Arc::new(Mutex::new(Some(d)));
    s.enqueue(Notice {
        status: FIRING,
        alerts: vec![sample],
        test: true,
        done: Some(Box::new(move |o| {
            if let Some(d) = lock(&d).take() {
                d.respond(answer(200, o.json()));
            }
        })),
    });
}

/// How long until `d` from `since`, or `None` when that is past or never.
pub(crate) fn due(since: Option<SystemTime>, d: Duration) -> Option<SystemTime> {
    since.and_then(|s| s.checked_add(d))
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::indexing_slicing)]
    use super::*;

    fn v(s: &str) -> Value {
        json::parse(s.as_bytes()).unwrap()
    }

    #[test]
    fn webhooks_checked() {
        let now = SystemTime::now();
        let w = Webhook::of(
            &v(r#"{"name": "ops", "url": "https://hooks.example/T0/B1?token=x", "secret": "s3"}"#),
            None,
            now,
        )
        .unwrap();
        assert_eq!(
            (w.format.as_str(), w.timeout_ms),
            ("json", DEFAULT_TIMEOUT_MS)
        );
        assert!(w.send_resolved);
        let view = w.view(None);
        assert!(
            view.contains(r#""url_host":"https://hooks.example""#)
                && !view.contains("token")
                && !view.contains("s3"),
            "{view}"
        );
        // An empty secret keeps the one kept; clear_secret clears it.
        let kept = Webhook::of(&v(r#"{"format": "slack"}"#), Some(&w), now).unwrap();
        assert_eq!(
            (kept.secret.as_str(), kept.url.as_str()),
            ("s3", w.url.as_str())
        );
        assert_eq!(kept.changes(&w), "format");
        let cleared = Webhook::of(&v(r#"{"clear_secret": true}"#), Some(&w), now).unwrap();
        assert!(cleared.secret.is_empty());
        let err = |s: &str| match Webhook::of(&v(s), None, now) {
            Err(Refusal::Field(f)) => f.field,
            other => panic!("{other:?}"),
        };
        assert_eq!(
            err(r#"{"name": "x", "url": "http://hooks.example/x"}"#),
            "url"
        );
        assert_eq!(
            err(r#"{"name": "x", "url": "https://u@h.example/"}"#),
            "url"
        );
        assert_eq!(
            err(r#"{"name": "x", "url": "https://h.example/", "format": "custom"}"#),
            "template"
        );
        assert_eq!(
            err(
                r#"{"name": "x", "url": "https://h.example/", "format": "custom", "template": "{\"a\": ${text"}"#
            ),
            "template"
        );
        assert!(Webhook::of(&v(r#"{"name": "x", "url": "http://127.0.0.1:9/", "format": "custom", "template": "{\"t\": \"${text}\", \"n\": ${count}, \"a\": ${alerts}}"}"#), None, now).is_ok());
        assert_eq!(
            redact_url(
                "Post https://h.example/T0/B1?token=x: refused",
                "https://h.example/T0/B1?token=x"
            ),
            "Post https://h.example/…: refused"
        );
    }

    #[test]
    fn rendered_and_signed() {
        let w = Webhook {
            name: "ops".into(),
            url: "http://127.0.0.1:9/hook".into(),
            secret: "k".into(),
            format: "json".into(),
            timeout_ms: 1000,
            ..Webhook::default()
        };
        let a = Alert {
            rule: "member-down".into(),
            severity: "critical".into(),
            node: "a".into(),
            summary: "down".into(),
            ..Alert::default()
        };
        let n = Notice::of(FIRING, vec![a]);
        let now = UNIX_EPOCH + Duration::from_secs(1_700_000_000);
        let (target, h, body) = render(&w, &n, now, "mid");
        assert_eq!(target, w.url);
        let sig = &h.iter().find(|(k, _)| k == HEADER_SIGNATURE).unwrap().1;
        let want = hmac(b"k", &[b"1700000000.", body.as_bytes()]);
        assert_eq!(
            *sig,
            format!("sha256={}", akit_roles::linktoken::hex(&want))
        );
        let b = v(&body);
        assert_eq!(json::str_of(&b, "source"), "akit-cluster");
        assert!(json::str_of(&b, "text").contains("• [critical] member-down · a: down"));
        let slack = Webhook {
            format: "slack".into(),
            template: "${severity}/${on}".into(),
            secret: String::new(),
            ..w.clone()
        };
        let (_, h, body) = render(&slack, &n, now, "mid");
        assert!(!h.iter().any(|(k, _)| k == HEADER_SIGNATURE));
        assert!(
            json::str_of(&v(&body), "text").ends_with("\ncritical/a"),
            "{body}"
        );
        let ding = Webhook {
            format: "dingtalk".into(),
            ..w.clone()
        };
        let (target, _, _) = render(&ding, &n, now, "mid");
        assert!(
            target.starts_with("http://127.0.0.1:9/hook?timestamp=1700000000000&sign="),
            "{target}"
        );
        assert_eq!(
            chat_refusal("feishu", br#"{"code": 19021, "msg": "sign"}"#).as_deref(),
            Some("19021 sign")
        );
        assert_eq!(chat_refusal("feishu", br#"{"code": 0}"#), None);
        assert_eq!(query_escape("a+b/c="), "a%2Bb%2Fc%3D");
    }
}
