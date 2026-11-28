//! The assistant's evaluation (I165, fit's I129: `assistant/eval_test.go`, `eval_check_test.go`
//! and `eval_catch_test.go`): the skills `migrate` and `security-review` run end to end, as the
//! panel runs them, on fixed sets of configs, each with what it should come to
//! (`testdata/eval`):
//!
//! - `migrate/<case>`: a config of nginx, Caddy or Traefik (`source.*`) and `case.json`: requests
//!   and where each must go once the proposal the skill makes is applied on `base.json`, and the
//!   items it must name as not converted. The proposal must check as `akit validate` checks a
//!   config, compiled with akit's modules, and is applied.
//! - `review/<case>`: an akit config with planted problems (`akit.json`) and `case.json`: the
//!   certificates akit serves, and the findings the review must make, by the member each is about.
//!
//! [`assistant_eval`] runs them against each case's recorded model answers (`recorded.json`), as
//! CI does, without calling any model. With a provider in the environment it runs them against
//! that model instead, and reports how many convert to a config that validates and keeps the
//! routes, and how many reviews find their planted problems: `make assistant-eval`, never in CI.
//!
//! ```text
//! AKIT_EVAL_KIND         the provider's kind, as admin.assistant's: anthropic, openai, ...
//! AKIT_EVAL_MODEL        the model
//! AKIT_EVAL_API_KEY      its key, when it takes one (read by the provider's api_key_env)
//! AKIT_EVAL_BASE_URL     its base URL, when not the kind's own
//! AKIT_EVAL_API_VERSION  azure_openai's api-version
//! AKIT_EVAL_CONTEXT      the model's window in tokens, when known
//! AKIT_EVAL_CASES        a regular expression of the cases to run
//! AKIT_EVAL_REPORT       a file to write the report to, as JSON
//! AKIT_EVAL_RECORD=1     write each case's answers to its recorded.json
//! ```
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing,
    clippy::arithmetic_side_effects,
    clippy::too_many_lines
)]

use std::fmt::Write as _;
use std::net::IpAddr;
use std::path::{Path, PathBuf};
use std::sync::{Arc, OnceLock};
use std::time::Instant;

use akit_config::value::{Value, parse};

use crate::json::{self, Obj, compact, quoted};
use crate::standin::Turn;
use crate::testkit::*;

/// The skills the evaluation runs, by the set's directory.
fn skill_of(set: &str) -> &'static str {
    if set == "migrate" {
        "migrate"
    } else {
        "security-review"
    }
}

/// The set's directory.
fn eval_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("testdata/eval")
}

/// A request, and where it must go.
#[derive(Debug, Clone, Default)]
struct Route {
    port: u16,
    host: String,
    path: String,
    remote: String,
    header: Vec<(String, String)>,
    /// A proxy to upstreams holding these addresses.
    upstream: Vec<String>,
    /// A handler of this module.
    module: String,
    /// A redirect whose target holds this.
    redirect: String,
    /// A refusal: no route, or a respond of 401, 403 or 404.
    denied: bool,
    /// Text the route, with its upstreams, holds.
    contains: Vec<String>,
}

/// A certificate the stand-in serves, `days` from its expiry.
#[derive(Debug, Clone, Default)]
struct Cert {
    listener: String,
    names: Vec<String>,
    days: i64,
}

/// A planted problem: found by a finding about one of `members`, or inside one, or by one that
/// names one of `mentions` about a member under the same top-level member as `members`' first.
#[derive(Debug, Clone, Default)]
struct Planted {
    id: String,
    members: Vec<String>,
    mentions: Vec<String>,
}

/// A case of the set, as its `case.json` says.
#[derive(Debug, Clone, Default)]
struct Case {
    name: String,
    set: String,
    skill: &'static str,
    dir: PathBuf,
    message: String,
    from: String,
    source: String,
    routes: Vec<Route>,
    unconverted: Vec<String>,
    config: String,
    certificates: Vec<Cert>,
    findings: Vec<Planted>,
}

fn strings_of(v: &Value, k: &str) -> Vec<String> {
    arr(v, k)
        .iter()
        .filter_map(|x| match x {
            Value::String(s) => Some(s.clone()),
            _ => None,
        })
        .collect()
}

impl Case {
    fn load(set: &str, dir: PathBuf) -> Case {
        let text = std::fs::read_to_string(dir.join("case.json")).unwrap();
        let v = parse(&text).unwrap();
        let routes = arr(&v, "routes")
            .iter()
            .map(|r| {
                let w = r.get("want").cloned().unwrap_or(Value::Null);
                Route {
                    port: u16::try_from(n(r, "port")).unwrap(),
                    host: s(r, "host").to_owned(),
                    path: s(r, "path").to_owned(),
                    remote: s(r, "remote").to_owned(),
                    header: match r.get("header") {
                        Some(Value::Object(ms)) => ms
                            .iter()
                            .map(|(k, x)| match x {
                                Value::String(v) => (k.clone(), v.clone()),
                                other => (k.clone(), compact(other)),
                            })
                            .collect(),
                        _ => Vec::new(),
                    },
                    upstream: strings_of(&w, "upstream"),
                    module: s(&w, "module").to_owned(),
                    redirect: s(&w, "redirect").to_owned(),
                    denied: b(&w, "denied"),
                    contains: strings_of(&w, "contains"),
                }
            })
            .collect();
        let certificates = arr(&v, "certificates")
            .iter()
            .map(|c| Cert {
                listener: s(c, "listener").to_owned(),
                names: strings_of(c, "names"),
                days: match c.get("days") {
                    Some(Value::Number(x)) => x.parse().unwrap(),
                    _ => 0,
                },
            })
            .collect();
        let findings = arr(&v, "findings")
            .iter()
            .map(|f| Planted {
                id: s(f, "id").to_owned(),
                members: strings_of(f, "members"),
                mentions: strings_of(f, "mentions"),
            })
            .collect();
        Case {
            name: format!("{set}/{}", dir.file_name().unwrap().to_string_lossy()),
            set: set.to_owned(),
            skill: skill_of(set),
            message: s(&v, "message").to_owned(),
            from: s(&v, "from").to_owned(),
            source: s(&v, "source").to_owned(),
            routes,
            unconverted: strings_of(&v, "unconverted"),
            config: s(&v, "config").to_owned(),
            certificates,
            findings,
            dir,
        }
    }

    fn read(&self, name: &str) -> String {
        std::fs::read_to_string(self.dir.join(name)).unwrap()
    }

    /// The person's message.
    fn message(&self) -> String {
        match (self.message.is_empty(), self.set.as_str()) {
            (false, "migrate") => {
                format!("{}\n\n```\n{}```", self.message, self.read(&self.source))
            }
            (false, _) => self.message.clone(),
            (true, "migrate") => format!(
                "Move this {} config to akit, as one proposal:\n\n```\n{}```",
                self.from,
                self.read(&self.source)
            ),
            (true, _) => "Review this akit's config for security problems, and propose a fix for \
                          each finding you can."
                .to_owned(),
        }
    }

    /// The case's recorded answers as the stand-in model's turns, `"{{source}}"` in a call's
    /// arguments the case's source.
    fn turns(&self) -> Vec<Turn> {
        let rec = Record::read(&self.read("recorded.json"));
        let source = if self.source.is_empty() {
            String::new()
        } else {
            quoted(&self.read(&self.source))
        };
        rec.turns
            .iter()
            .map(|t| {
                if t.calls.is_empty() {
                    Turn::Text(t.text.clone())
                } else {
                    Turn::Tools(
                        t.calls
                            .iter()
                            .map(|(name, args)| {
                                (name.clone(), args.replace("\"{{source}}\"", &source))
                            })
                            .collect(),
                    )
                }
            })
            .collect()
    }
}

/// The cases of the set `AKIT_EVAL_CASES` matches, under `dir`.
fn load_cases(dir: &Path) -> Vec<Case> {
    let only = std::env::var("AKIT_EVAL_CASES")
        .ok()
        .filter(|s| !s.is_empty())
        .map(|s| akit_router::regexp(s.as_bytes()).unwrap());
    let mut out = Vec::new();
    for set in ["migrate", "review"] {
        let mut dirs: Vec<PathBuf> = std::fs::read_dir(dir.join(set))
            .unwrap()
            .filter_map(|e| e.ok().map(|e| e.path()))
            .filter(|p| p.join("case.json").exists())
            .collect();
        dirs.sort();
        for d in dirs {
            let c = Case::load(set, d);
            if only.as_ref().is_none_or(|r| r.is_match(&c.name)) {
                out.push(c);
            }
        }
    }
    assert!(!out.is_empty(), "no case of the set is chosen");
    out
}

/// A case's recorded model answers, in order: each answer's text and the tools it called.
#[derive(Debug, Clone, Default)]
struct Record {
    model: String,
    turns: Vec<RecordTurn>,
}

#[derive(Debug, Clone, Default)]
struct RecordTurn {
    text: String,
    /// The tools called: name and arguments, JSON.
    calls: Vec<(String, String)>,
}

impl Record {
    fn read(text: &str) -> Record {
        let v = parse(text).unwrap();
        Record {
            model: s(&v, "model").to_owned(),
            turns: arr(&v, "turns")
                .iter()
                .map(|t| RecordTurn {
                    text: s(t, "text").to_owned(),
                    calls: arr(t, "calls")
                        .iter()
                        .map(|c| {
                            (
                                s(c, "name").to_owned(),
                                c.get("args").map(compact).unwrap_or_else(|| "{}".into()),
                            )
                        })
                        .collect(),
                })
                .collect(),
        }
    }

    /// As `recorded.json` keeps it, indented as fit's.
    fn json(&self) -> String {
        let turns: Vec<Value> = self
            .turns
            .iter()
            .map(|t| {
                let mut ms = Vec::new();
                if !t.text.is_empty() {
                    ms.push(("text".to_owned(), Value::String(t.text.clone())));
                }
                if !t.calls.is_empty() {
                    ms.push((
                        "calls".to_owned(),
                        Value::Array(
                            t.calls
                                .iter()
                                .map(|(name, args)| {
                                    Value::Object(vec![
                                        ("name".to_owned(), Value::String(name.clone())),
                                        ("args".to_owned(), parse(args).unwrap_or(Value::Null)),
                                    ])
                                })
                                .collect(),
                        ),
                    ));
                }
                Value::Object(ms)
            })
            .collect();
        Value::Object(vec![
            ("model".to_owned(), Value::String(self.model.clone())),
            ("turns".to_owned(), Value::Array(turns)),
        ])
        .to_json()
    }
}

/// What a case came to.
#[derive(Debug, Default)]
struct Outcome {
    case: String,
    skill: &'static str,
    proposal: bool,
    valid: bool,
    applied: bool,
    routes: usize,
    routes_kept: usize,
    unconverted: usize,
    named: usize,
    planted: usize,
    found: usize,
    findings: usize,
    calls: usize,
    failed: usize,
    errors: Vec<String>,
    tokens: u64,
    seconds: f64,
    problems: Vec<String>,
}

impl Outcome {
    fn passed(&self) -> bool {
        self.problems.is_empty()
    }

    fn json(&self) -> String {
        Obj::new()
            .str("case", &self.case)
            .str("skill", self.skill)
            .bool("proposal", self.proposal)
            .bool("valid", self.valid)
            .bool("applied", self.applied)
            .num("routes", self.routes)
            .num("routes_kept", self.routes_kept)
            .num("unconverted", self.unconverted)
            .num("named", self.named)
            .num("planted", self.planted)
            .num("found", self.found)
            .num("findings", self.findings)
            .num("calls", self.calls)
            .num("failed", self.failed)
            .raw("errors", &json::strings(&self.errors))
            .num("tokens", self.tokens)
            .raw("seconds", &format!("{:.1}", self.seconds))
            .raw("problems", &json::strings(&self.problems))
            .end()
    }
}

/// The model the set runs against, from the environment; `None` runs it against the recorded
/// answers.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
struct Model {
    kind: String,
    model: String,
    base_url: String,
    api_version: String,
    context: u64,
    record: bool,
}

impl Model {
    fn from_env() -> Option<Model> {
        let var = |k: &str| std::env::var(k).unwrap_or_default();
        let kind = var("AKIT_EVAL_KIND");
        if kind.is_empty() {
            return None;
        }
        let m = Model {
            model: var("AKIT_EVAL_MODEL"),
            base_url: var("AKIT_EVAL_BASE_URL"),
            api_version: var("AKIT_EVAL_API_VERSION"),
            context: match var("AKIT_EVAL_CONTEXT").as_str() {
                "" => 0,
                s => s.parse().expect("AKIT_EVAL_CONTEXT is a number of tokens"),
            },
            record: var("AKIT_EVAL_RECORD") == "1",
            kind,
        };
        assert!(
            !m.model.is_empty(),
            "AKIT_EVAL_MODEL names the model AKIT_EVAL_KIND's provider is called with"
        );
        assert!(
            m.kind != "llm_proxy",
            "AKIT_EVAL_KIND: llm_proxy needs a running akit's llm.proxy; use openai_compatible \
             at its address"
        );
        Some(m)
    }

    fn name(m: Option<&Model>) -> String {
        m.map_or_else(
            || "the recorded answers".to_owned(),
            |m| format!("{}/{}", m.kind, m.model),
        )
    }

    /// `admin.assistant`'s settings for the model, the key read from `AKIT_EVAL_API_KEY`;
    /// `extra` is what every run sets beside the provider.
    fn settings(&self, extra: &str) -> String {
        let mut model = Obj::new().str("name", &self.model);
        if self.context > 0 {
            model = model.num("context", self.context);
        }
        let p = Obj::new()
            .str("name", "eval")
            .str("kind", &self.kind)
            .str("api_key_env", "AKIT_EVAL_API_KEY")
            .raw("models", &format!("[{}]", model.end()))
            .str_omit("base_url", &self.base_url)
            .str_omit("api_version", &self.api_version)
            .end();
        format!(
            r#"{{"providers": [{p}], "default_model": {{"provider": "eval", "model": {}}}, {extra}}}"#,
            quoted(&self.model)
        )
    }
}

/// The settings every run has: the user applies, and the skills have room for a config's worth
/// of calls and answer.
const EXTRA: &str = r#""default_level": "apply", "params": {"max_output_tokens": 16384},
    "limits": {"max_tool_calls": 40, "max_tool_result_bytes": 65536, "max_tokens_per_day": 50000000}"#;

/// The certificate and key that stand for the files a case's config names on the machine it
/// came from, made once.
struct TestCert {
    cert: String,
    key: String,
}

fn test_cert() -> &'static TestCert {
    static C: OnceLock<TestCert> = OnceLock::new();
    C.get_or_init(|| {
        let dir = std::env::temp_dir().join(format!("akit-eval-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let key =
            akio_crypto::pkey::PrivateKey::P256(akio_crypto::p256::SecretKey::generate().unwrap());
        let mut p = akio_x509::CertificateParams::new(vec!["eval.example"]);
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs();
        let now = i64::try_from(now).unwrap();
        p.not_before = now - 3600;
        p.not_after = now + 365 * 86_400;
        let cert = p.self_signed(&key).unwrap();
        let (c, k) = (dir.join("eval.crt"), dir.join("eval.key"));
        std::fs::write(&c, cert.to_pem()).unwrap();
        std::fs::write(
            &k,
            akio_crypto::pem::encode("PRIVATE KEY", &key.to_pkcs8_der()),
        )
        .unwrap();
        TestCert {
            cert: c.to_string_lossy().into_owned(),
            key: k.to_string_lossy().into_owned(),
        }
    })
}

/// `v` with each file a member `*_file` names that is not here replaced with the test
/// certificate's: its key for a key's member, its certificate for the rest; and each root
/// directory that is not, with the certificate's.
fn local_files(v: Value, key: &str) -> Value {
    let cert = test_cert();
    match v {
        Value::Object(ms) => Value::Object(
            ms.into_iter()
                .map(|(k, x)| {
                    let y = local_files(x, &k);
                    (k, y)
                })
                .collect(),
        ),
        Value::Array(a) => Value::Array(a.into_iter().map(|x| local_files(x, key)).collect()),
        Value::String(s)
            if (key.ends_with("_file") || key.ends_with("_files") || key == "root")
                && !Path::new(&s).exists() =>
        {
            Value::String(if key == "root" {
                Path::new(&cert.cert)
                    .parent()
                    .unwrap()
                    .to_string_lossy()
                    .into_owned()
            } else if key.contains("key") {
                cert.key.clone()
            } else {
                cert.cert.clone()
            })
        }
        other => other,
    }
}

/// akit's modules, as its binary registers them, but the BitTorrent role's, which nothing here
/// uses.
fn registry() -> akit_runtime::Registry<akit_runtime::HandlerParams> {
    akit_runtime::Registry::new(&[
        akit_handler::MODULES,
        akit_middleware::MODULES,
        akit_proxy::MODULES,
    ])
    .unwrap()
}

/// A config loaded and compiled as `akit validate` checks one, with the files it names that are
/// not here standing for the test certificate's.
fn compile(text: &str) -> Result<(akit_runtime::Graph, Value), String> {
    let v = parse(text).map_err(|e| e.to_string())?;
    let v = local_files(v, "");
    let text = v.to_json();
    let config = akit_config::from_json(&text, None).map_err(|e| e.to_string())?;
    let g = akit_runtime::compile(&config, &registry(), &akit_runtime::Env::default())
        .map_err(|e| e.to_string())?;
    Ok((g, v))
}

/// `POST /config/validate` as the evaluation's stand-in answers it: compiled.
fn check(text: &str) -> String {
    match compile(text) {
        Ok(_) => r#"{"valid":true}"#.to_owned(),
        Err(e) => format!(
            r#"{{"valid":false,"errors":[{}]}}"#,
            Obj::new().str("msg", &e).end()
        ),
    }
}

/// `GET /status` of the case's stand-in: its listeners, and the certificates the case serves.
fn status_of(c: &Case) -> Status {
    let certs = c.certificates.clone();
    Arc::new(move |version, data| {
        let v = parse(data).unwrap_or(Value::Null);
        let listeners: Vec<String> = arr(&v, "listeners")
            .iter()
            .map(|l| {
                Obj::new()
                    .str("name", s(l, "name"))
                    .str("address", s(l, "address"))
                    .end()
            })
            .collect();
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs();
        let certs: Vec<String> = certs
            .iter()
            .map(|ct| {
                let at = i64::try_from(now).unwrap() + ct.days * 86_400 + 3600;
                let mut when = Vec::new();
                akit_observe::time::rfc3339_nano(&mut when, at, 0, 0);
                Obj::new()
                    .str("listener", &ct.listener)
                    .raw("names", &json::strings(&ct.names))
                    .str("issuer", "R11")
                    .str("not_after", &String::from_utf8(when).unwrap())
                    .end()
            })
            .collect();
        Obj::new()
            .num("version", version)
            .str("applied", "2026-11-06T00:00:00Z")
            .str("source", "file")
            .raw("listeners", &format!("[{}]", listeners.join(",")))
            .raw("connections", r#"{"open":3}"#)
            .raw("upstreams", "[]")
            .raw("certificates", &format!("[{}]", certs.join(",")))
            .end()
    })
}

/// The conversation's messages as the panel reads them.
fn messages(f: &Fx, conv: &str) -> Vec<Value> {
    let (_, body) = f.get(&format!("/api/conversations/{conv}"));
    arr(&parse(&body).unwrap(), "messages").to_vec()
}

/// Runs a case: the chat, as the panel would send it, then the checks of what it left.
fn run_case(m: Option<&Model>, c: &Case) -> Outcome {
    let start = Instant::now();
    let mut r = Outcome {
        case: c.name.clone(),
        skill: c.skill,
        ..Outcome::default()
    };
    let base = if c.set == "review" {
        c.read(&c.config)
    } else {
        std::fs::read_to_string(c.dir.join("../../base.json")).unwrap()
    };
    let cfgs = Fake::new(&base);
    cfgs.with(|x| {
        x.validate = Some(Arc::new(check));
        x.status = Some(status_of(c));
    });
    let f = match m {
        None => start_on(cfgs.clone(), &format!(", {EXTRA}"), c.turns(), |_| {}),
        Some(m) => {
            let settings = m.settings(EXTRA);
            start_on(cfgs.clone(), "", vec![Turn::Text("-".into())], move |o| {
                o.settings = akit_config::assistant::load(&settings, o.console.as_ref()).unwrap();
            })
        }
    };

    let mut body = Obj::new()
        .str("message", &c.message())
        .str("skill", c.skill);
    if !c.from.is_empty() {
        body = body.raw("inputs", &Obj::new().str("from", &c.from).end());
    }
    let mut body = body.end();
    let mut conv = String::new();
    for round in 0.. {
        let chat = f.chat(&body);
        for (_, e) in &chat.events {
            match s(e, "type") {
                "tool_call" => r.calls += 1,
                "tool_result" if b(e, "error") => {
                    r.failed += 1;
                    r.errors.push(format!(
                        "{}: {}",
                        s(e, "name"),
                        crate::prompt::clip(s(e, "content"), 300)
                    ));
                }
                "usage" => {
                    if let Some(u) = e.get("usage") {
                        r.tokens += n(u, "input_tokens") + n(u, "output_tokens");
                    }
                }
                _ => {}
            }
        }
        let Some(end) = chat.end() else {
            r.problems
                .push(format!("the chat failed: {:?}", chat.err()));
            break;
        };
        conv = s(end, "conversation").to_owned();
        let asks = chat.of("ask");
        let reason = s(end, "reason");
        if reason != "ask" || asks.is_empty() || round == 3 {
            if reason != "answer" {
                r.problems.push(format!("the chat ended with {reason}"));
            }
            break;
        }
        // A question is answered with its first option.
        let ask = asks.last().unwrap();
        let choice = strs(ask, "options").first().cloned().unwrap_or_default();
        body = Obj::new()
            .str("conversation", &conv)
            .raw(
                "answer",
                &Obj::new()
                    .str("id", s(ask, "id"))
                    .str("choice", &choice)
                    .end(),
            )
            .end();
    }
    if !conv.is_empty() {
        if c.set == "migrate" {
            migration(&f, &cfgs, c, &mut r, &conv);
        } else {
            review(&f, c, &mut r, &conv);
        }
        if let Some(m) = m.filter(|m| m.record) {
            record(&f, c, m, &conv);
        }
    }
    r.seconds = start.elapsed().as_secs_f64();
    r
}

/// The conversation's proposal as the panel reads it.
fn view(f: &Fx, conv: &str) -> (Option<Value>, String) {
    let (status, p, body) = f.view(conv);
    if status != 200 {
        return (None, body);
    }
    (p, body)
}

/// What a migration left: its proposal valid, the items not converted named, applied, and the
/// requests going where they must on the config it made.
fn migration(f: &Fx, cfgs: &Fake, c: &Case, r: &mut Outcome, conv: &str) {
    r.routes = c.routes.len();
    r.unconverted = c.unconverted.len();
    let (v, body) = view(f, conv);
    let Some(v) = v else {
        r.problems
            .push(format!("no proposal: {}", crate::prompt::clip(&body, 300)));
        return;
    };
    r.proposal = true;
    r.valid = b(&v, "valid");
    if !r.valid {
        r.problems.push(format!(
            "the proposal does not check: {} {}",
            v.get("errors").map(compact).unwrap_or_default(),
            v.get("steps").map(compact).unwrap_or_default()
        ));
    }
    let items = strs(&v, "unconverted");
    for want in &c.unconverted {
        if named(&items, want) {
            r.named += 1;
        } else {
            r.problems.push(format!(
                "the proposal does not name {} as not converted: {items:?}",
                quoted(want)
            ));
        }
    }
    if !r.valid {
        return;
    }
    let (status, out) = f.apply_it(conv, &v);
    if status != 200 {
        r.problems.push(format!(
            "the apply: {status} {}",
            crate::prompt::clip(&compact(&out), 300)
        ));
        return;
    }
    r.applied = true;
    let (_, data) = cfgs.current();
    // The requests are checked on the config as applied, not on its files' stand-ins.
    let cfg = parse(&data).unwrap();
    let (g, _) = match compile(&data) {
        Ok(x) => x,
        Err(e) => {
            r.problems
                .push(format!("the config applied does not compile: {e}"));
            return;
        }
    };
    for rt in &c.routes {
        match check_route(&g, &cfg, rt) {
            Ok(()) => r.routes_kept += 1,
            Err(e) => r.problems.push(e),
        }
    }
}

/// Whether one of `items` names `want`: holds one of its words, `|` between them, in any case.
fn named(items: &[String], want: &str) -> bool {
    items.iter().any(|it| {
        want.split('|')
            .any(|w| it.to_lowercase().contains(&w.trim().to_lowercase()))
    })
}

/// What a review found: each planted problem named by a finding with a fix, and its proposal,
/// when it made one, valid.
fn review(f: &Fx, c: &Case, r: &mut Outcome, conv: &str) {
    r.planted = c.findings.len();
    let answer = messages(f, conv)
        .iter()
        .rev()
        .find(|m| s(m, "role") == "assistant" && !s(m, "content").trim().is_empty())
        .map(|m| s(m, "content").to_owned())
        .unwrap_or_default();
    let found = parse_findings(&answer);
    r.findings = found.len();
    for p in &c.findings {
        if found.iter().any(|fd| fd.fix && p.matches(fd)) {
            r.found += 1;
        } else {
            r.problems.push(format!(
                "no finding with a fix names {} ({})",
                p.id,
                p.members.join(", ")
            ));
        }
    }
    if let (Some(v), _) = view(f, conv) {
        r.proposal = true;
        r.valid = b(&v, "valid");
        if !r.valid {
            r.problems.push(format!(
                "the proposal does not check: {}",
                v.get("errors").map(compact).unwrap_or_default()
            ));
        }
    }
}

/// Writes the conversation's answers to the case's `recorded.json`, the case's source put back
/// as `"{{source}}"`.
fn record(f: &Fx, c: &Case, m: &Model, conv: &str) {
    let source = if c.source.is_empty() {
        String::new()
    } else {
        quoted(&c.read(&c.source))
    };
    let mut rec = Record {
        model: Model::name(Some(m)),
        turns: Vec::new(),
    };
    for msg in messages(f, conv) {
        if s(&msg, "role") != "assistant" || !s(&msg, "error").is_empty() {
            continue;
        }
        rec.turns.push(RecordTurn {
            text: s(&msg, "content").to_owned(),
            calls: arr(&msg, "tool_calls")
                .iter()
                .map(|tc| {
                    let args = tc
                        .get("arguments")
                        .map(compact)
                        .unwrap_or_else(|| "{}".into());
                    let args = if source.is_empty() {
                        args
                    } else {
                        args.replace(&source, "\"{{source}}\"")
                    };
                    (s(tc, "name").to_owned(), args)
                })
                .collect(),
        });
    }
    std::fs::write(c.dir.join("recorded.json"), rec.json()).unwrap();
}

/// A request as the router reads it.
struct Req {
    host: String,
    path: Vec<u8>,
    remote: Option<IpAddr>,
    headers: Vec<(String, String)>,
}

impl akit_router::Request for Req {
    fn method(&self) -> &str {
        "GET"
    }

    fn host(&self) -> &str {
        &self.host
    }

    fn path(&self) -> &[u8] {
        &self.path
    }

    fn query(&self) -> &str {
        ""
    }

    fn remote_ip(&self) -> Option<IpAddr> {
        self.remote
    }

    fn any_header(&self, name: &str, mut f: impl FnMut(&str) -> bool) -> bool {
        self.headers
            .iter()
            .any(|(k, v)| k.eq_ignore_ascii_case(name) && f(v))
    }
}

/// Whether `rt`'s request goes where it must on `g`, compiled from `cfg`.
fn check_route(g: &akit_runtime::Graph, cfg: &Value, rt: &Route) -> Result<(), String> {
    let mut what = format!("{}{} on :{}", rt.host, rt.path, rt.port);
    if !rt.remote.is_empty() {
        let _ = write!(what, " from {}", rt.remote);
    }
    let port = rt.port.to_string();
    let found = g.listeners().iter().enumerate().find(|(_, l)| {
        l.address.rsplit_once(':').is_some_and(|(_, p)| p == port)
            && (l.redirect_https != 0
                || l.protocols.iter().any(|p| {
                    matches!(
                        p,
                        akit_config::Protocol::Http11
                            | akit_config::Protocol::H2
                            | akit_config::Protocol::H2c
                            | akit_config::Protocol::H3
                    )
                }))
    });
    let Some((i, l)) = found else {
        return Err(format!(
            "{what}: no listener serves HTTP on port {}",
            rt.port
        ));
    };
    if l.redirect_https != 0 {
        if rt.redirect.starts_with("https://") {
            return Ok(());
        }
        return Err(format!("{what}: the listener only redirects to HTTPS"));
    }
    let remote = if rt.remote.is_empty() {
        "198.51.100.7"
    } else {
        rt.remote.as_str()
    };
    let req = Req {
        host: rt.host.clone(),
        path: rt.path.as_bytes().to_vec(),
        remote: remote.parse().ok(),
        headers: rt.header.clone(),
    };
    let Some((index, _)) = g.route_request_at(i, &req) else {
        if rt.denied {
            return Ok(());
        }
        return Err(format!("{what}: no route takes it"));
    };
    let routes = arr(cfg, "routes");
    let Some(rc) = routes.get(index) else {
        return Err(format!("{what}: route {index} is not in the config"));
    };
    let handler = rc.get("handler").cloned().unwrap_or(Value::Null);
    let module = s(&handler, "module");
    let params = handler.get("params").cloned().unwrap_or(Value::Null);
    let got = format!("{what}: route {}, handler {module}", quoted(s(rc, "name")));
    if rt.denied {
        let status = n(&params, "status");
        if module != "respond" || ![401, 403, 404].contains(&status) {
            return Err(format!("{got}, not refused"));
        }
    }
    if !rt.module.is_empty() && module != rt.module {
        return Err(format!("{got}, not {}", rt.module));
    }
    // The route's text, with every upstream it names.
    let mut text = compact(rc);
    let names = akit_router::regexp(br#""(?:upstream|failover)":"([^"]+)""#).unwrap();
    let mut at = 0;
    let route_text = text.clone();
    while let Some(caps) = names.captures_at(&route_text, at) {
        let whole = caps.get(0).unwrap();
        at = whole.end.max(at + 1);
        let name = &route_text[caps.get(1).unwrap()];
        if let Some(u) = upstream_of(cfg, name) {
            text.push_str(&compact(u));
        }
    }
    if !rt.upstream.is_empty() {
        if module != "proxy" {
            return Err(format!("{got}, not a proxy to {:?}", rt.upstream));
        }
        let addrs: Vec<String> = proxy_upstreams(&params)
            .iter()
            .filter_map(|name| upstream_of(cfg, name))
            .flat_map(|u| strs(u, "addresses"))
            .collect();
        if rt.upstream.iter().any(|a| !addrs.contains(a)) {
            return Err(format!("{got}, to {addrs:?}, not {:?}", rt.upstream));
        }
    }
    if !rt.redirect.is_empty() && !redirects(rc, &rt.redirect) {
        return Err(format!("{got}, not a redirect to {}", rt.redirect));
    }
    for want in &rt.contains {
        if !text.contains(want.as_str()) {
            return Err(format!("{got}, without {}", quoted(want)));
        }
    }
    Ok(())
}

/// The upstreams a proxy's params send requests to.
fn proxy_upstreams(params: &Value) -> Vec<String> {
    let mut out = Vec::new();
    if !s(params, "upstream").is_empty() {
        out.push(s(params, "upstream").to_owned());
    }
    for x in arr(params, "upstreams") {
        for k in ["upstream", "name"] {
            if !s(x, k).is_empty() {
                out.push(s(x, k).to_owned());
            }
        }
    }
    out
}

/// The upstream `name` of `cfg`.
fn upstream_of<'v>(cfg: &'v Value, name: &str) -> Option<&'v Value> {
    arr(cfg, "upstreams").iter().find(|u| s(u, "name") == name)
}

/// Whether route `rc` redirects to a URL that holds `to`: a redirect middleware, or handler.
fn redirects(rc: &Value, to: &str) -> bool {
    arr(rc, "middleware")
        .iter()
        .chain(rc.get("handler"))
        .any(|m| {
            s(m, "module") == "redirect" && m.get("params").is_some_and(|p| s(p, "to").contains(to))
        })
}

/// A review's finding, as its answer writes one.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
struct Finding {
    severity: String,
    member: String,
    line: String,
    fix: bool,
}

/// The findings of a review's answer: each an item of a list, its severity, maybe bold or in
/// brackets, then its member in backticks.
fn parse_findings(answer: &str) -> Vec<Finding> {
    static RE: OnceLock<akio_regex::Regex> = OnceLock::new();
    let re = RE.get_or_init(|| {
        akit_router::regexp(
            "(?im)^[ \\t]*(?:[-*•]|\\d+[.)])[ \\t]*(?:\\*\\*|__|\\[)?[ \\t]*(high|medium|low|高|中|低)[ \\t]*(?:\\*\\*|__|\\])?[ \\t]*[:：-]?[ \\t]*`([^`\\n]+)`(.*)$"
                .as_bytes(),
        )
        .unwrap()
    });
    let mut out = Vec::new();
    let mut at = 0;
    while let Some(caps) = re.captures_at(answer, at) {
        let whole = caps.get(0).unwrap();
        at = whole.end.max(at + 1);
        let sev = answer[caps.get(1).unwrap()].to_lowercase();
        let sev = match sev.as_str() {
            "高" => "high".to_owned(),
            "中" => "medium".to_owned(),
            "低" => "low".to_owned(),
            _ => sev,
        };
        let rest = answer[caps.get(3).unwrap()].to_lowercase();
        out.push(Finding {
            severity: sev,
            member: norm_path(&answer[caps.get(2).unwrap()]),
            line: answer[whole].to_lowercase(),
            fix: rest.contains("fix") || rest.contains("修复"),
        });
    }
    out
}

/// A member's path as written in a finding, compared.
fn norm_path(p: &str) -> String {
    let p = p.trim().replace(' ', "").to_lowercase();
    let p = p.strip_prefix('$').unwrap_or(&p);
    p.strip_prefix('.').unwrap_or(p).to_owned()
}

/// Whether `path` is `want`, or a member inside it.
fn within(path: &str, want: &str) -> bool {
    let want = norm_path(want);
    path == want || path.starts_with(&format!("{want}.")) || path.starts_with(&format!("{want}["))
}

/// A path's top-level member.
fn top(path: &str) -> &str {
    path.find(['.', '[']).map_or(path, |i| &path[..i])
}

impl Planted {
    /// Whether `fd` names this planted problem.
    fn matches(&self, fd: &Finding) -> bool {
        if self.members.iter().any(|m| within(&fd.member, m)) {
            return true;
        }
        let Some(first) = self.members.first() else {
            return false;
        };
        if top(&fd.member) != top(&norm_path(first)) {
            return false;
        }
        self.mentions
            .iter()
            .any(|m| fd.line.contains(&m.to_lowercase()))
    }
}

/// What each case came to, and the counts.
fn report(m: Option<&Model>, results: &[Outcome]) -> String {
    let mut b = format!("The assistant's evaluation, against {}:\n", Model::name(m));
    let (mut migrates, mut migrated, mut reviews, mut reviewed) = (0, 0, 0, 0);
    for r in results {
        let state = if r.passed() {
            "ok".to_owned()
        } else {
            format!("FAILED: {}", r.problems.join("; "))
        };
        if r.skill == "migrate" {
            migrates += 1;
            migrated += usize::from(r.passed());
            let _ = writeln!(
                b,
                "  {:<26} valid {:<5} applied {:<5} routes {}/{}  unconverted named {}/{}  calls {} ({} failed)  {} tokens  {:.1}s  {state}",
                r.case,
                r.valid,
                r.applied,
                r.routes_kept,
                r.routes,
                r.named,
                r.unconverted,
                r.calls,
                r.failed,
                r.tokens,
                r.seconds
            );
        } else {
            reviews += 1;
            reviewed += usize::from(r.passed());
            let _ = writeln!(
                b,
                "  {:<26} planted found {}/{}  findings {}  proposal {} (valid {})  calls {} ({} failed)  {} tokens  {:.1}s  {state}",
                r.case,
                r.found,
                r.planted,
                r.findings,
                r.proposal,
                r.valid,
                r.calls,
                r.failed,
                r.tokens,
                r.seconds
            );
        }
    }
    let _ = writeln!(
        b,
        "migrate: {migrated} of {migrates} convert to a config that validates and keeps the routes, their unconverted items named"
    );
    let _ = writeln!(
        b,
        "security-review: {reviewed} of {reviews} find the planted problems of their config"
    );
    b
}

/// fit's `TestAssistantEval`: the set, against the recorded answers, where every case must come
/// to what it should, or against the model the environment names (`make assistant-eval`), and
/// what each came to.
#[test]
fn assistant_eval() {
    let m = Model::from_env();
    let cases = load_cases(&eval_dir());
    let mut results = Vec::new();
    for c in &cases {
        let r = run_case(m.as_ref(), c);
        for e in &r.errors {
            eprintln!("{}: a call failed: {e}", c.name);
        }
        results.push(r);
    }
    let text = report(m.as_ref(), &results);
    eprintln!("\n{text}");
    if let Ok(path) = std::env::var("AKIT_EVAL_REPORT") {
        if !path.is_empty() {
            let items: Vec<String> = results.iter().map(Outcome::json).collect();
            let doc = Obj::new()
                .str("model", &Model::name(m.as_ref()))
                .raw("results", &format!("[{}]", items.join(",")))
                .end();
            std::fs::write(path, doc).unwrap();
        }
    }
    assert!(results.iter().all(Outcome::passed), "{text}");
}

/// Copies the set's case `name` (`set/case`) to a directory of its own, with `base.json`, its
/// recorded answers changed by `edit`.
fn copy_case(name: &str, edit: Option<&dyn Fn(&mut Record)>) -> Case {
    static N: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
    let dir = std::env::temp_dir().join(format!(
        "akit-eval-{}-{}",
        std::process::id(),
        N.fetch_add(1, std::sync::atomic::Ordering::SeqCst)
    ));
    let src = eval_dir().join(name);
    let dst = dir.join(name);
    std::fs::create_dir_all(&dst).unwrap();
    std::fs::copy(eval_dir().join("base.json"), dir.join("base.json")).unwrap();
    for e in std::fs::read_dir(&src).unwrap() {
        let e = e.unwrap();
        let mut data = std::fs::read_to_string(e.path()).unwrap();
        if e.file_name() == "recorded.json" {
            if let Some(edit) = edit {
                let mut rec = Record::read(&data);
                edit(&mut rec);
                data = rec.json();
            }
        }
        std::fs::write(dst.join(e.file_name()), data).unwrap();
    }
    let set = name.split('/').next().unwrap();
    let mut c = Case::load(set, dst);
    c.name = name.to_owned();
    c
}

/// The arguments of the record's `config_propose` call, and the call.
fn propose_args(rec: &mut Record) -> (Value, &mut (String, String)) {
    let call = rec
        .turns
        .iter_mut()
        .flat_map(|t| t.calls.iter_mut())
        .find(|(n, _)| n == "config_propose")
        .expect("no config_propose in the record");
    (parse(&call.1).unwrap(), call)
}

fn has(r: &Outcome, want: &[&str]) {
    let all = r.problems.join("\n");
    for w in want {
        assert!(all.contains(w), "{}: no problem says {w:?}:\n{all}", r.case);
    }
}

fn member_mut<'v>(v: &'v mut Value, k: &str) -> &'v mut Value {
    let Value::Object(ms) = v else {
        panic!("not an object")
    };
    &mut ms.iter_mut().find(|(n, _)| n == k).unwrap().1
}

fn items_mut(v: &mut Value) -> &mut Vec<Value> {
    let Value::Array(a) = v else {
        panic!("not a list")
    };
    a
}

/// fit's `TestAssistantEvalCatches`: the evaluation fails what it should: a migration whose
/// proposal drops a route's restriction and names nothing as not converted, one that makes no
/// proposal, one whose proposal does not check, and a review that misses a planted problem or
/// gives no fix.
#[test]
fn assistant_eval_catches() {
    let run = |name: &str, edit: &dyn Fn(&mut Record)| run_case(None, &copy_case(name, Some(edit)));

    // The office-only route left open to all, and no item named.
    let r = run("migrate/nginx-shop", &|rec| {
        let (mut args, call) = propose_args(rec);
        let routes = items_mut(member_mut(member_mut(&mut args, "members"), "routes"));
        routes.retain(|m| s(m, "name") != "shop.example.com-admin-others");
        for m in routes.iter_mut() {
            crate::tree::del(member_mut(m, "match"), "remote_ip");
        }
        crate::tree::del(&mut args, "unconverted");
        call.1 = compact(&args);
    });
    assert!(
        !r.passed() && r.applied && r.routes_kept == r.routes - 1 && r.named == 0,
        "{r:?}"
    );
    has(
        &r,
        &[
            "/admin/users on :443 from 203.0.113.9: route \"shop.example.com-admin\", handler proxy, not refused",
            "does not name \"sub_filter\"",
            "does not name \"error_page\"",
        ],
    );

    // The upstream of the API pointed elsewhere, and the redirect lost.
    let r = run("migrate/nginx-shop", &|rec| {
        let (mut args, call) = propose_args(rec);
        let members = member_mut(&mut args, "members");
        let up = &mut items_mut(member_mut(members, "upstreams"))[0];
        *member_mut(up, "addresses") = parse(r#"["10.9.9.9:80"]"#).unwrap();
        for m in items_mut(member_mut(members, "routes")).iter_mut() {
            if s(m, "name") == "shop.example.com" {
                crate::tree::del(m, "middleware");
            }
        }
        call.1 = compact(&args);
    });
    has(
        &r,
        &[
            "to [\"10.9.9.9:80\"], not [\"10.0.1.10:8080\", \"10.0.1.11:8080\"]",
            "not a redirect to https://",
        ],
    );

    // No proposal: the answer alone.
    let r = run("migrate/traefik-labels", &|rec| {
        let last = rec.turns.pop().unwrap();
        rec.turns = vec![last];
    });
    has(&r, &["no proposal"]);

    // A proposal that does not check is not applied.
    let r = run("migrate/traefik-labels", &|rec| {
        let (mut args, call) = propose_args(rec);
        let up = &mut items_mut(member_mut(member_mut(&mut args, "members"), "upstreams"))[0];
        *member_mut(up, "addresses") = Value::Array(Vec::new());
        call.1 = compact(&args);
    });
    assert!(!r.applied && !r.valid, "{r:?}");
    has(&r, &["the proposal does not check"]);

    // A review that misses one problem, and gives another no fix.
    let r = run("review/tls-and-limits", &|rec| {
        let last = rec.turns.last_mut().unwrap();
        let keep: Vec<String> = last
            .text
            .split('\n')
            .filter(|l| !l.contains("`routes[3]`"))
            .map(|l| {
                if l.contains("`upstreams[1]`") {
                    l[..l.find("Fix:").unwrap()].to_owned()
                } else {
                    l.to_owned()
                }
            })
            .collect();
        last.text = keep.join("\n");
    });
    assert_eq!(r.found, r.planted - 2, "{r:?}");
    has(
        &r,
        &[
            "no finding with a fix names browse-and-dotfiles",
            "no finding with a fix names long-timeouts",
        ],
    );

    // A review with no findings at all, and a proposal that does not check.
    let r = run("review/exposed", &|rec| {
        rec.turns = vec![
            RecordTurn {
                text: String::new(),
                calls: vec![(
                    "config_propose".into(),
                    r#"{"summary": "Drop the routes", "members": {"routes": [{"name": "x"}]}}"#
                        .into(),
                )],
            },
            RecordTurn {
                text: "Nothing to report.".into(),
                calls: Vec::new(),
            },
        ];
    });
    assert!(
        r.found == 0 && r.findings == 0 && r.proposal && !r.valid,
        "{r:?}"
    );
    has(
        &r,
        &[
            "no finding with a fix names console-everywhere",
            "the proposal does not check",
        ],
    );
}

/// fit's `TestAssistantEvalRecord`: a run's answers are written back as the case's
/// `recorded.json`, its source as `"{{source}}"`, and replay the same.
#[test]
fn assistant_eval_record() {
    let c = copy_case("migrate/nginx-docs", None);
    let before = Record::read(&c.read("recorded.json"));
    let f = start(&format!(", {EXTRA}"), c.turns());
    let body = Obj::new()
        .str("message", &c.message())
        .str("skill", c.skill)
        .raw("inputs", &Obj::new().str("from", &c.from).end())
        .end();
    let chat = f.chat(&body);
    let conv = s(chat.end().expect("the chat ended"), "conversation").to_owned();
    let m = Model {
        kind: "openai".into(),
        model: "gpt-x".into(),
        ..Model::default()
    };
    record(&f, &c, &m, &conv);
    let got = c.read("recorded.json");
    let after = Record::read(&got);
    assert!(
        after.model == "openai/gpt-x"
            && after.turns.len() == before.turns.len()
            && got.contains(r#""text": "{{source}}""#),
        "{got}"
    );
    let r = run_case(None, &c);
    assert!(r.passed(), "the record does not replay: {r:?}");
}

/// fit's `TestAssistantEvalSettings`: the settings made for a model in the environment load, for
/// each kind, and name it.
#[test]
fn assistant_eval_settings() {
    assert_eq!(Model::name(None), "the recorded answers");
    let m = Model {
        kind: "azure_openai".into(),
        model: "gpt-4.1".into(),
        base_url: "https://example.openai.azure.com".into(),
        api_version: "2025-04-01-preview".into(),
        context: 128_000,
        record: true,
    };
    assert_eq!(Model::name(Some(&m)), "azure_openai/gpt-4.1");
    let (_, console) = settings_of("http://127.0.0.1:1", "");
    let s = akit_config::assistant::load(&m.settings(EXTRA), Some(&console)).unwrap();
    let p = s.provider("eval").unwrap();
    assert!(
        p.api_version == "2025-04-01-preview"
            && p.api_key_env == "AKIT_EVAL_API_KEY"
            && s.limits.tool_calls() == 40,
        "{s:?}"
    );
    for kind in [
        "anthropic",
        "openai",
        "openai_responses",
        "gemini",
        "ollama",
    ] {
        let m = Model {
            kind: kind.into(),
            model: "m".into(),
            ..Model::default()
        };
        akit_config::assistant::load(&m.settings(EXTRA), Some(&console)).unwrap();
    }
}

/// fit's `TestEvalFindings`: findings are read from an answer however a model formats the list,
/// in English or Chinese, and a planted problem is found by its member, one inside it, or a
/// mention under the same member.
#[test]
fn eval_findings() {
    let answer = "Findings:\n\n\
        - **high** `routes[2]` (admin-panel): no auth. Fix: add basic_auth.\n\
        1. [medium] `admin.console.address`: everyone reaches it. Fix: listen on loopback.\n\
        * LOW: `Upstreams[1].response_timeout`: 24h. Fix: 60s.\n\
        - **高** `listeners[0]`：明文 HTTP。修复：重定向到 HTTPS。\n\
        - **medium** `routes[5]` (login) has no rate limit, nothing to change\n\
        - the `routes` look fine otherwise.\n";
    let got = parse_findings(answer);
    let want = [
        ("high", "routes[2]", true),
        ("medium", "admin.console.address", true),
        ("low", "upstreams[1].response_timeout", true),
        ("high", "listeners[0]", true),
        ("medium", "routes[5]", false),
    ];
    assert_eq!(got.len(), want.len(), "{got:?}");
    for (i, (sev, member, fix)) in want.iter().enumerate() {
        assert!(
            got[i].severity == *sev && got[i].member == *member && got[i].fix == *fix,
            "{i}: {:?}",
            got[i]
        );
    }
    let planted = |members: &[&str], mentions: &[&str]| Planted {
        id: String::new(),
        members: members.iter().map(|s| (*s).to_owned()).collect(),
        mentions: mentions.iter().map(|s| (*s).to_owned()).collect(),
    };
    for (p, fd, want) in [
        (planted(&["routes[2]"], &[]), 0, true),
        (planted(&["routes[2].middleware"], &[]), 0, false),
        (planted(&["admin.console"], &[]), 1, true),
        (planted(&["admin"], &[]), 1, true),
        (planted(&["routes[20]"], &[]), 0, false),
        (planted(&["routes[7]"], &["Admin-Panel"]), 0, true),
        (planted(&["upstreams[7]"], &["admin-panel"]), 0, false),
        (planted(&[], &["admin-panel"]), 0, false),
        (planted(&["upstreams[1]"], &[]), 2, true),
    ] {
        assert_eq!(p.matches(&got[fd]), want, "{p:?} on {fd}");
    }
}
