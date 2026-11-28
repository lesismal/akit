//! The assistant's settings and keys (I160, fit's `assistant/assistant.go`): `admin.assistant`'s
//! until the Settings page saves them, and the store's from then on, in its section `assistant`,
//! fit's document `{"settings", "keys"}`. A key is written once and never answered: the pages see
//! whether a provider has one, where from, and its last four characters.

use std::collections::BTreeMap;
use std::fmt;
use std::sync::Arc;

use akit_config::assistant::{Assistant as Settings, Level, ProviderKind};
use akit_config::value::Value;

use crate::json::{self, Obj};

/// The most bytes of the settings a page sends: fit's `MaxSettingsBytes`.
pub const MAX_SETTINGS_BYTES: usize = 256 << 10;
/// The most bytes of a key: fit's `MaxKeyBytes`.
pub const MAX_KEY_BYTES: usize = 4096;
/// The most bytes of a request's body the console takes for the module: the settings, a key
/// for each provider the settings may name, and room for the JSON around them.
pub const MAX_BODY_BYTES: usize =
    MAX_SETTINGS_BYTES + akit_config::assistant::MAX_PROVIDERS * MAX_KEY_BYTES + (64 << 10);

/// A key: never printed.
#[derive(Clone, PartialEq, Eq, Default)]
pub struct Secret(String);

impl fmt::Debug for Secret {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.0.is_empty() {
            f.write_str("\"\"")
        } else {
            f.write_str("[redacted]")
        }
    }
}

impl Secret {
    /// `s` as a secret.
    #[must_use]
    pub fn new(s: impl Into<String>) -> Secret {
        Secret(s.into())
    }

    /// The key itself, for the one place that sends it: a model call's header.
    #[must_use]
    pub fn expose(&self) -> &str {
        &self.0
    }

    /// Whether there is none.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    /// The key as the pages see it: its last four characters, and those only of a key of 16 or
    /// more, which they give nothing away of. fit's `mask`.
    #[must_use]
    pub fn masked(&self) -> String {
        let n = self.0.chars().count();
        if n < 16 {
            return "••••".to_owned();
        }
        let tail: String = self.0.chars().skip(n.saturating_sub(4)).collect();
        format!("••••{tail}")
    }
}

/// Where a provider's key is from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KeySource {
    /// The store, as the Settings page saved it.
    Store,
    /// The environment variable `api_key_env` names.
    Env,
}

impl KeySource {
    fn name(self) -> &'static str {
        match self {
            KeySource::Store => "store",
            KeySource::Env => "env",
        }
    }
}

/// The provider `name`'s key among `keys`, or in its `api_key_env`, and where it is from. Reads
/// the environment.
#[must_use]
pub fn key_of(
    p: &akit_config::assistant::Provider,
    keys: &BTreeMap<String, Secret>,
) -> Option<(Secret, KeySource)> {
    if let Some(k) = keys.get(&p.name).filter(|k| !k.is_empty()) {
        return Some((k.clone(), KeySource::Store));
    }
    if !p.api_key_env.is_empty() {
        if let Ok(k) = std::env::var(&p.api_key_env) {
            if !k.is_empty() {
                return Some((Secret(k), KeySource::Env));
            }
        }
    }
    None
}

/// The settings in use.
#[derive(Debug, Clone)]
pub(crate) struct State {
    /// The settings as written, JSON: the store's, or `admin.assistant` as the config wrote it.
    pub(crate) raw: String,
    pub(crate) settings: Arc<Settings>,
    /// The keys the store keeps, by provider.
    pub(crate) keys: BTreeMap<String, Secret>,
    /// Whether the settings are the store's.
    pub(crate) from_store: bool,
    /// The saves since akit started, the pages' version (fit's `rev`).
    pub(crate) rev: u64,
}

/// The store's section, as fit writes it: `{"settings": <the settings>, "keys": {<provider>:
/// <key>}}`, each left out when empty.
#[must_use]
pub fn section(raw: &str, keys: &BTreeMap<String, Secret>) -> String {
    let mut o = Obj::new();
    if !raw.is_empty() {
        o = o.raw("settings", raw);
    }
    if !keys.is_empty() {
        let mut k = Obj::new();
        for (name, key) in keys {
            k = k.str(name, &key.0);
        }
        o = o.raw("keys", &k.end());
    }
    o.end()
}

/// What a section kept in the store holds: its settings, as written, and its keys.
#[derive(Debug, Default)]
pub(crate) struct Kept {
    pub(crate) settings: Option<String>,
    pub(crate) keys: BTreeMap<String, Secret>,
}

/// Reads a section `data`, or why it is not one.
pub(crate) fn read_section(data: &[u8]) -> Result<Kept, String> {
    let text = std::str::from_utf8(data).map_err(|_| "not UTF-8".to_owned())?;
    let v = akit_config::value::parse(text).map_err(|e| e.to_string())?;
    let Value::Object(_) = &v else {
        return Err("not a JSON object".to_owned());
    };
    let mut k = Kept::default();
    match v.get("settings") {
        None | Some(Value::Null) => {}
        Some(s) => k.settings = Some(json::compact(s)),
    }
    match v.get("keys") {
        None | Some(Value::Null) => {}
        Some(Value::Object(ms)) => {
            for (name, key) in ms {
                let Value::String(key) = key else {
                    return Err(format!("the key of {name:?} is not a string"));
                };
                k.keys.insert(name.clone(), Secret(key.clone()));
            }
        }
        Some(_) => return Err("keys is not an object".to_owned()),
    }
    Ok(k)
}

/// llm.proxy's API as the running config has it: its address, and whether it is served over TLS.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProxyApi {
    /// `llm.proxy.api.address`.
    pub address: String,
    /// `llm.proxy.api.cert_file`, when it is served over TLS.
    pub cert_file: Option<String>,
}

/// llm.proxy's API in the running config `text`, JSON, when it has one.
#[must_use]
pub fn proxy_api(text: &str) -> Option<ProxyApi> {
    let v = akit_config::value::parse(text).ok()?;
    let api = v.get("llm")?.get("proxy")?.get("api")?;
    let address = json::str_of(api, "address");
    if address.is_empty() {
        return None;
    }
    let cert = json::str_of(api, "cert_file");
    Some(ProxyApi {
        address: address.to_owned(),
        cert_file: (!cert.is_empty()).then(|| cert.to_owned()),
    })
}

/// `GET /api/settings`'s answer, and `PUT`'s: fit's `settingsBody`, members in its order.
pub(crate) fn view(s: &State, store: bool, users: &[String], proxy: Option<&ProxyApi>) -> String {
    let mut keys = Obj::new();
    let mut names: Vec<&str> = s
        .settings
        .providers
        .iter()
        .map(|p| p.name.as_str())
        .collect();
    names.sort_unstable();
    names.dedup();
    for name in names {
        let Some(p) = s.settings.provider(name) else {
            continue;
        };
        let got = key_of(p, &s.keys);
        let mut v = Obj::new().bool("set", got.is_some());
        if let Some((_, src)) = &got {
            v = v.str("source", src.name());
        }
        if let Some((k, _)) = &got {
            v = v.str("masked", &k.masked());
        }
        v = v.str_omit("env", &p.api_key_env);
        keys = keys.raw(name, &v.end());
    }
    let kinds: Vec<&str> = ProviderKind::ALL.iter().map(|k| k.name()).collect();
    let levels: Vec<&str> = Level::ALL.iter().map(|l| l.name()).collect();
    let mut defaults: Vec<&str> = kinds.clone();
    defaults.sort_unstable();
    let mut d = Obj::new();
    for k in defaults {
        let url = ProviderKind::parse(k).map_or("", ProviderKind::default_url);
        d = d.str(k, url);
    }
    let proxy = proxy.map_or_else(
        || "null".to_owned(),
        |p| {
            Obj::new()
                .str("address", &p.address)
                .bool("tls", p.cert_file.is_some())
                .end()
        },
    );
    Obj::new()
        .raw("settings", if s.raw.is_empty() { "{}" } else { &s.raw })
        .str("source", if s.from_store { "store" } else { "config" })
        .bool("store", store)
        .str("version", &s.rev.to_string())
        .raw("keys", &keys.end())
        .raw("kinds", &json::strings(&kinds))
        .raw("levels", &json::strings(&levels))
        .raw("users", &json::strings(users))
        .raw("llm_proxy", &proxy)
        .raw("defaults", &d.end())
        .end()
}

/// A change of the settings a page sent (fit's `putBody`), checked: the settings whole, as
/// written and as read, the keys to set by provider (an empty one removed, one not named kept),
/// and the version the page read.
#[derive(Debug)]
pub(crate) struct Put {
    pub(crate) raw: String,
    pub(crate) settings: Settings,
    pub(crate) keys: BTreeMap<String, Secret>,
    pub(crate) version: String,
}

/// Why a change is refused: its status and its body.
pub(crate) type Refusal = (u16, String);

/// `{"error": msg}`.
pub(crate) fn error_body(msg: &str) -> String {
    Obj::new().str("error", msg).end()
}

/// `{"error": msg, "errors": [...]}`: config errors, each at its path, as the pages read them.
pub(crate) fn errors_body(msg: &str, errs: &akit_config::Errors) -> String {
    let mut b = String::from("[");
    for (i, e) in errs.0.iter().enumerate() {
        if i > 0 {
            b.push(',');
        }
        b.push_str(
            &Obj::new()
                .str("file", e.file.as_deref().unwrap_or(""))
                .str("path", &e.path)
                .num("line", e.line)
                .num("col", e.col)
                .str("msg", &e.msg)
                .end(),
        );
    }
    b.push(']');
    Obj::new().str("error", msg).raw("errors", &b).end()
}

/// Checks a `PUT /api/settings` body, `v`, against `console`: the settings load as
/// `admin.assistant` does, the keys name providers and are printable. fit's `put`, before it
/// saves.
pub(crate) fn check_put(v: &Value, console: Option<&akit_config::Console>) -> Result<Put, Refusal> {
    if !matches!(v, Value::Object(_)) {
        return Err((
            400,
            error_body("the body is not the JSON expected: not an object"),
        ));
    }
    let raw = match v.get("settings") {
        None | Some(Value::Null) => String::new(),
        Some(s) => json::compact(s),
    };
    if raw.is_empty() || raw.len() > MAX_SETTINGS_BYTES {
        return Err((
            400,
            error_body(&format!(
                "the body must hold the settings, at most {MAX_SETTINGS_BYTES} bytes"
            )),
        ));
    }
    let settings = akit_config::assistant::load(&raw, console)
        .map_err(|e| (400, errors_body("the settings are not valid", &e)))?;
    let mut keys = BTreeMap::new();
    match v.get("keys") {
        None | Some(Value::Null) => {}
        Some(Value::Object(ms)) => {
            for (name, key) in ms {
                let Value::String(key) = key else {
                    return Err((
                        400,
                        error_body("the body is not the JSON expected: a key is not a string"),
                    ));
                };
                if settings.provider(name).is_none() {
                    return Err((
                        400,
                        error_body(&format!("a key for {name:?}, which is no provider")),
                    ));
                }
                if key.len() > MAX_KEY_BYTES || key.bytes().any(|c| c <= b' ' || c >= 0x7f) {
                    return Err((
                        400,
                        error_body(&format!(
                            "the key of {name:?} is not 1 to {MAX_KEY_BYTES} printable characters \
                             without spaces"
                        )),
                    ));
                }
                keys.insert(name.clone(), Secret(key.clone()));
            }
        }
        Some(_) => {
            return Err((
                400,
                error_body("the body is not the JSON expected: keys is not an object"),
            ));
        }
    }
    let version = match v.get("version") {
        None | Some(Value::Null) => String::new(),
        Some(Value::String(s)) => s.clone(),
        Some(_) => {
            return Err((
                400,
                error_body("the body is not the JSON expected: version is not a string"),
            ));
        }
    };
    Ok(Put {
        raw,
        settings,
        keys,
        version,
    })
}

/// The keys after `put`: the kept ones of providers that still are, with those it names set, or
/// removed when empty; and the names it changed, in order.
pub(crate) fn merged_keys(
    kept: &BTreeMap<String, Secret>,
    put: &Put,
) -> (BTreeMap<String, Secret>, Vec<String>) {
    let mut keys: BTreeMap<String, Secret> = kept
        .iter()
        .filter(|(name, _)| put.settings.provider(name).is_some())
        .map(|(n, k)| (n.clone(), k.clone()))
        .collect();
    let mut changed = Vec::new();
    for (name, key) in &put.keys {
        changed.push(name.clone());
        if key.is_empty() {
            keys.remove(name);
        } else {
            keys.insert(name.clone(), key.clone());
        }
    }
    (keys, changed)
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]
    use super::*;

    fn console() -> akit_config::Console {
        akit_config::Console {
            users: vec![akit_config::ConsoleUser {
                username: "ada".into(),
                password_hash: String::new(),
            }],
            ..akit_config::Console::default()
        }
    }

    #[test]
    fn masks() {
        assert_eq!(Secret::new("short").masked(), "••••");
        assert_eq!(Secret::new("sk-0123456789abcdef").masked(), "••••cdef");
        assert_eq!(
            format!("{:?}", Secret::new("sk-0123456789abcdef")),
            "[redacted]"
        );
    }

    #[test]
    fn sections_round_trip() {
        let mut keys = BTreeMap::new();
        keys.insert("b".to_owned(), Secret::new("kb"));
        keys.insert("a".to_owned(), Secret::new("ka"));
        let s = section(r#"{"providers":[]}"#, &keys);
        assert_eq!(
            s,
            r#"{"settings":{"providers":[]},"keys":{"a":"ka","b":"kb"}}"#
        );
        let k = read_section(s.as_bytes()).unwrap();
        assert_eq!(k.settings.as_deref(), Some(r#"{"providers":[]}"#));
        assert_eq!(k.keys, keys);
        assert_eq!(section("", &BTreeMap::new()), "{}");
        assert!(read_section(b"[1]").is_err());
        assert!(read_section(br#"{"keys": {"a": 1}}"#).is_err());
    }

    #[test]
    fn puts_checked() {
        let c = console();
        let body = |s: &str| akit_config::value::parse(s).unwrap();
        let ok = check_put(
            &body(
                r#"{"settings": {"providers": [{"name": "o", "kind": "ollama"}]}, "keys": {"o": "k-1"}, "version": "3"}"#,
            ),
            Some(&c),
        )
        .unwrap();
        assert_eq!(ok.version, "3");
        assert_eq!(ok.keys.get("o"), Some(&Secret::new("k-1")));
        let refused = |s: &str| check_put(&body(s), Some(&c)).unwrap_err();
        assert_eq!(refused(r#"{"keys": {}}"#).0, 400);
        let (st, b) = refused(r#"{"settings": {"x": 1}}"#);
        assert_eq!(st, 400);
        assert!(
            b.contains(r#""path":"admin.assistant.x""#) && b.contains("unknown field"),
            "{b}"
        );
        assert!(
            refused(r#"{"settings": {}, "keys": {"none": "k"}}"#)
                .1
                .contains("which is no provider")
        );
        assert!(
            refused(r#"{"settings": {"providers": [{"name": "o", "kind": "ollama"}]}, "keys": {"o": "a b"}}"#)
                .1
                .contains("printable")
        );
        let big = format!(
            r#"{{"settings": {{"data": {{"redact": ["{}"]}}}}}}"#,
            "a".repeat(MAX_SETTINGS_BYTES)
        );
        assert!(refused(&big).1.contains("at most"));
    }

    #[test]
    fn keys_merged() {
        let c = console();
        let put = check_put(
            &akit_config::value::parse(
                r#"{"settings": {"providers": [{"name": "a", "kind": "ollama"}, {"name": "b", "kind": "ollama"}]}, "keys": {"a": "", "b": "kb2"}}"#,
            )
            .unwrap(),
            Some(&c),
        )
        .unwrap();
        let mut kept = BTreeMap::new();
        kept.insert("a".to_owned(), Secret::new("ka"));
        kept.insert("gone".to_owned(), Secret::new("kg"));
        let (keys, changed) = merged_keys(&kept, &put);
        assert_eq!(changed, ["a", "b"]);
        assert_eq!(keys.len(), 1);
        assert_eq!(keys.get("b"), Some(&Secret::new("kb2")));
    }

    #[test]
    fn proxy_read() {
        assert_eq!(
            proxy_api(
                r#"{"llm": {"proxy": {"api": {"address": ":8080", "cert_file": "/c.pem"}}}}"#
            ),
            Some(ProxyApi {
                address: ":8080".into(),
                cert_file: Some("/c.pem".into())
            })
        );
        assert_eq!(proxy_api(r#"{"llm": {"proxy": {}}}"#), None);
        assert_eq!(proxy_api("{"), None);
    }
}
