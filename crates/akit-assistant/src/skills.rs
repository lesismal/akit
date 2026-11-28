//! Skills (I164, fit's `assistant/skills.go`, `skillstore.go` and `chatskill.go`): a skill is a
//! named task the assistant runs: when it is for, the prompt that says how, the tools it may call
//! (never wider than the user's level), the inputs it takes and the model it wants. A skill is
//! written as Markdown: YAML front matter between two lines `---`, then the prompt.
//!
//! ```text
//! ---
//! name: route-builder
//! title: Route builder
//! title_zh: 路由生成
//! description: A route, its matcher, handler and middleware from a sentence.
//! description_zh: 用一句话生成路由及其匹配、处理器与中间件。
//! tools: [config_get, schema_describe, config_validate, config_propose]
//! inputs:
//!   - name: host
//!     description: The host name the route answers.
//! model: anthropic/claude-sonnet-4-5
//! examples:
//!   - Send /api/* on example.com to 10.0.0.7:9000.
//! ---
//! The prompt, in Markdown.
//! ```
//!
//! fit's built-in skills are embedded with akit's names (`skills/*.md`); users add theirs on the
//! Skills page, kept in the store's section [`SECTION`] with which built-in ones are disabled.
//! A chat runs one at a time: set by `/name`, the panel's picker, or the model's `skill_load`
//! from the catalog the system prompt lists.

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex, PoisonError};

use akio_http::{Context, Method};
use akit_config::assistant::{Assistant as Settings, Level};
use akit_config::located::{self, Located, Value as Y};
use akit_config::value::Value;
use akit_roles::pages;

use crate::Assistant;
use crate::json::{self, Obj, quoted};
use crate::registry::{Call, Kind, Tool};

/// The store's section the users' skills are kept in, with which skills are disabled.
pub const SECTION: &str = "assistant.skills";

/// A skill's Markdown, front matter and prompt.
pub const MAX_SKILL_BYTES: usize = 32 << 10;
/// The skills users add.
pub const MAX_USER_SKILLS: usize = 64;
const MAX_TOOLS: usize = 32;
/// The inputs a skill takes.
pub const MAX_INPUTS: usize = 8;
const MAX_EXAMPLES: usize = 8;
const MAX_TITLE: usize = 120;
const MAX_TEXT: usize = 400;
const MAX_MODEL: usize = 128;
/// An input's value, as a chat gives it.
pub const MAX_INPUT_VALUE: usize = 2048;

/// The level a user needs to change the skills: they change what the assistant does for every
/// user.
pub const CHANGE_LEVEL: Level = Level::Apply;

/// The built-in skills offered on a cluster's manager only (I169).
pub const CLUSTER_SKILLS: &[&str] = &["fleet-health", "rollout-planner", "cluster-drift"];

/// The built-in skills, in the order the Skills page lists them: fit's plan's.
const BUILTIN: &[(&str, &str)] = &[
    ("explain", include_str!("../skills/explain.md")),
    ("route-builder", include_str!("../skills/route-builder.md")),
    ("reverse-proxy", include_str!("../skills/reverse-proxy.md")),
    ("tls-acme", include_str!("../skills/tls-acme.md")),
    (
        "rate-limit-auth",
        include_str!("../skills/rate-limit-auth.md"),
    ),
    ("cache-tuning", include_str!("../skills/cache-tuning.md")),
    ("diagnose", include_str!("../skills/diagnose.md")),
    ("drift-explain", include_str!("../skills/drift-explain.md")),
    ("log-analysis", include_str!("../skills/log-analysis.md")),
    (
        "security-review",
        include_str!("../skills/security-review.md"),
    ),
    ("migrate", include_str!("../skills/migrate.md")),
    ("media-setup", include_str!("../skills/media-setup.md")),
    (
        "llm-mesh-setup",
        include_str!("../skills/llm-mesh-setup.md"),
    ),
    ("tunnel-setup", include_str!("../skills/tunnel-setup.md")),
    ("vod-setup", include_str!("../skills/vod-setup.md")),
    (
        "channel-scheduler",
        include_str!("../skills/channel-scheduler.md"),
    ),
    (
        "library-health",
        include_str!("../skills/library-health.md"),
    ),
    (
        "playback-troubleshoot",
        include_str!("../skills/playback-troubleshoot.md"),
    ),
    ("fleet-health", include_str!("../skills/fleet-health.md")),
    (
        "rollout-planner",
        include_str!("../skills/rollout-planner.md"),
    ),
    ("cluster-drift", include_str!("../skills/cluster-drift.md")),
    ("sync-pipeline", include_str!("../skills/sync-pipeline.md")),
];

/// An input a skill takes.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Input {
    /// Its name.
    pub name: String,
    /// What it is.
    pub description: String,
    /// Whether the skill needs it.
    pub required: bool,
}

/// A skill as its Markdown says it.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Skill {
    /// How it is called, as `/name`: lower case letters, digits and `-`, starting with a letter.
    pub name: String,
    /// What it is for, in English and, when given, in Chinese.
    pub title: String,
    /// See [`title`](Skill::title).
    pub title_zh: String,
    /// See [`title`](Skill::title); the model's catalog has the English one.
    pub description: String,
    /// See [`description`](Skill::description).
    pub description_zh: String,
    /// What it takes beside the user's words.
    pub inputs: Vec<Input>,
    /// The tools it may call, narrowed again by the user's level; `ask_user` is always among
    /// them.
    pub tools: Vec<String>,
    /// The model it runs with, when not empty: `provider/model`, or a model of the default
    /// provider.
    pub model: String,
    /// Requests it is for, shown to the user.
    pub examples: Vec<String>,
    /// How to do the task, the Markdown after the front matter.
    pub prompt: String,
    /// The Markdown it was read from, which export gives back.
    pub source: String,
}

/// A skill's Markdown refused: the line of the file it is about, from 1, or 0 for the whole, and
/// what is wrong.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SkillError {
    /// The line.
    pub line: u32,
    /// What is wrong.
    pub msg: String,
}

impl std::fmt::Display for SkillError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        if self.line == 0 {
            f.write_str(&self.msg)
        } else {
            write!(f, "line {}: {}", self.line, self.msg)
        }
    }
}

fn err(line: u32, msg: impl Into<String>) -> SkillError {
    SkillError {
        line,
        msg: msg.into(),
    }
}

fn skill_name(s: &str) -> bool {
    let b = s.as_bytes();
    b.len() <= 40
        && b.first().is_some_and(u8::is_ascii_lowercase)
        && b.iter()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || *c == b'-')
}

fn input_name(s: &str) -> bool {
    let b = s.as_bytes();
    b.len() <= 32
        && b.first().is_some_and(u8::is_ascii_lowercase)
        && b.iter()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || *c == b'_')
}

fn model_name(s: &str) -> bool {
    !s.is_empty() && s.len() <= MAX_MODEL && s.bytes().all(|c| (0x21..=0x7e).contains(&c))
}

/// A front matter's line: its line in the YAML, which starts on the file's second.
fn at(line: u32) -> u32 {
    line.saturating_add(1)
}

/// Reads a skill from its Markdown, refusing front matter that is not what a skill's is, with
/// the line it is about. fit's `ParseSkill`.
///
/// # Errors
///
/// What is wrong, at its line.
pub fn parse(data: &[u8]) -> Result<Skill, SkillError> {
    if data.len() > MAX_SKILL_BYTES {
        return Err(err(
            0,
            format!(
                "the skill is {} bytes; a skill is at most {MAX_SKILL_BYTES}",
                data.len()
            ),
        ));
    }
    let data = data.strip_prefix(b"\xef\xbb\xbf").unwrap_or(data);
    for (i, l) in data.split(|b| *b == b'\n').enumerate() {
        if std::str::from_utf8(l).is_err() {
            return Err(err(
                u32::try_from(i.saturating_add(1)).unwrap_or(u32::MAX),
                "the line is not UTF-8",
            ));
        }
    }
    let src = String::from_utf8_lossy(data).replace("\r\n", "\n");
    let lines: Vec<&str> = src.split('\n').collect();
    if lines.first().map(|l| l.trim_end_matches([' ', '\t'])) != Some("---") {
        return Err(err(
            1,
            "a skill starts with its front matter: a line \"---\", the YAML, and another \"---\"",
        ));
    }
    let Some(end) = lines
        .iter()
        .skip(1)
        .position(|l| l.trim_end_matches([' ', '\t']) == "---")
        .map(|i| i.saturating_add(1))
    else {
        return Err(err(1, "the front matter has no closing line \"---\""));
    };
    let front = lines.get(1..end).unwrap_or_default().join("\n");
    let mut sk = front_matter(&front)?;
    sk.prompt = lines
        .get(end.saturating_add(1)..)
        .unwrap_or_default()
        .join("\n")
        .trim()
        .to_owned();
    if sk.prompt.is_empty() {
        return Err(err(
            u32::try_from(end.saturating_add(1)).unwrap_or(u32::MAX),
            "the skill has no prompt after its front matter",
        ));
    }
    sk.source = src.clone();
    Ok(sk)
}

/// The front matter's message for a YAML error of akit's reader, as fit words its own.
fn yaml_message(msg: &str) -> String {
    let msg = msg.strip_prefix("invalid YAML: ").unwrap_or(msg);
    if let Some(key) = msg
        .strip_suffix(" is a key twice")
        .map(|k| k.trim_matches('"'))
    {
        return format!("{key} is given twice");
    }
    if msg == "a key must be a string" {
        return "a key of the front matter is not a name".to_owned();
    }
    format!("the front matter is not YAML: {msg}")
}

fn front_matter(text: &str) -> Result<Skill, SkillError> {
    let doc = located::yaml(text).map_err(|e| err(at(e.line.max(1)), yaml_message(&e.msg)))?;
    let root = doc.value;
    let members = match &root.value {
        Y::Object(ms) if ms.is_empty() => {
            return Err(err(
                1,
                "the front matter is empty: it needs name, title and description",
            ));
        }
        Y::Object(ms) => ms,
        Y::Null => {
            return Err(err(
                1,
                "the front matter is empty: it needs name, title and description",
            ));
        }
        _ => {
            return Err(err(
                at(root.line),
                "the front matter is not a mapping of keys to values",
            ));
        }
    };
    let mut sk = Skill::default();
    let mut seen: Vec<&str> = Vec::new();
    for (i, (k, kline, v)) in members.iter().enumerate() {
        seen.push(k);
        // An alias between this key and the next is this one's.
        let next = members.get(i.saturating_add(1)).map_or(u32::MAX, |m| m.1);
        if doc.alias.is_some_and(|l| l >= *kline && l < next) {
            return Err(err(
                at(doc.alias.unwrap_or(*kline)),
                format!("{k}: anchors and aliases are not taken"),
            ));
        }
        match k.as_str() {
            "name" => {
                sk.name = text1(v, k, 40)?;
                if !skill_name(&sk.name) {
                    return Err(err(
                        at(v.line),
                        format!(
                            "name {} is not a skill's name: lower case letters, digits and '-', \
                             starting with a letter, at most 40",
                            quoted(&sk.name)
                        ),
                    ));
                }
            }
            "title" => sk.title = text1(v, k, MAX_TITLE)?,
            "title_zh" => sk.title_zh = text1(v, k, MAX_TITLE)?,
            "description" => sk.description = text1(v, k, MAX_TEXT)?,
            "description_zh" => sk.description_zh = text1(v, k, MAX_TEXT)?,
            "model" => {
                sk.model = text1(v, k, MAX_MODEL)?;
                if !sk.model.is_empty() && !model_name(&sk.model) {
                    return Err(err(
                        at(v.line),
                        format!(
                            "model {} is not a model: \"provider/model\" or a model's ID, \
                             without spaces",
                            quoted(&sk.model)
                        ),
                    ));
                }
            }
            "tools" => {
                sk.tools = list(v, k, MAX_TOOLS, 64)?;
                let items = match &v.value {
                    Y::Array(a) => a.as_slice(),
                    _ => &[],
                };
                for (j, name) in sk.tools.iter().enumerate() {
                    let line = items.get(j).map_or(v.line, |e| e.line);
                    if !crate::registry::valid_name(name) {
                        return Err(err(
                            at(line),
                            format!("{} is not a tool's name", quoted(name)),
                        ));
                    }
                    if sk.tools.get(..j).is_some_and(|b| b.contains(name)) {
                        return Err(err(at(line), format!("the tool {name} is named twice")));
                    }
                }
            }
            "examples" => sk.examples = list(v, k, MAX_EXAMPLES, MAX_TEXT)?,
            "inputs" => sk.inputs = inputs(v)?,
            _ => {
                return Err(err(
                    at(*kline),
                    format!(
                        "{} is not a key of a skill's front matter: name, title, title_zh, \
                         description, description_zh, inputs, tools, model and examples are",
                        quoted(k)
                    ),
                ));
            }
        }
    }
    for (need, have) in [
        ("name", &sk.name),
        ("title", &sk.title),
        ("description", &sk.description),
    ] {
        if !seen.contains(&need) || have.is_empty() {
            return Err(err(1, format!("the front matter has no {need}")));
        }
    }
    Ok(sk)
}

/// A scalar's text, at most `n` bytes on one line.
fn text1(v: &Located, key: &str, n: usize) -> Result<String, SkillError> {
    let s = match &v.value {
        Y::Null => String::new(),
        Y::Bool(b) => b.to_string(),
        Y::Number(s) | Y::String(s) => s.trim().to_owned(),
        _ => return Err(err(at(v.line), format!("{key} is not a text"))),
    };
    if s.len() > n {
        return Err(err(
            at(v.line),
            format!("{key} is {} bytes; it is at most {n}", s.len()),
        ));
    }
    if s.contains(['\n', '\r']) {
        return Err(err(at(v.line), format!("{key} is one line")));
    }
    Ok(s)
}

/// A sequence of texts, at most `max` of at most `n` bytes each.
fn list(v: &Located, key: &str, max: usize, n: usize) -> Result<Vec<String>, SkillError> {
    let items = match &v.value {
        Y::Null => return Ok(Vec::new()),
        Y::Array(a) => a,
        _ => {
            return Err(err(
                at(v.line),
                format!("{key} is not a list, as [a, b] or lines of \"- a\""),
            ));
        }
    };
    if items.len() > max {
        return Err(err(
            at(v.line),
            format!("{key} has {} items; it has at most {max}", items.len()),
        ));
    }
    let mut out = Vec::with_capacity(items.len());
    for e in items {
        let s = text1(e, &format!("{key}'s item"), n)?;
        if s.is_empty() {
            return Err(err(at(e.line), format!("{key} has an empty item")));
        }
        out.push(s);
    }
    Ok(out)
}

/// A skill's inputs: a sequence of mappings of name, description and required.
fn inputs(v: &Located) -> Result<Vec<Input>, SkillError> {
    let items = match &v.value {
        Y::Null => return Ok(Vec::new()),
        Y::Array(a) => a,
        _ => {
            return Err(err(
                at(v.line),
                "inputs is not a list of inputs, each with a name",
            ));
        }
    };
    if items.len() > MAX_INPUTS {
        return Err(err(
            at(v.line),
            format!(
                "inputs has {} items; it has at most {MAX_INPUTS}",
                items.len()
            ),
        ));
    }
    let mut out: Vec<Input> = Vec::new();
    for e in items {
        let Y::Object(ms) = &e.value else {
            return Err(err(
                at(e.line),
                "an input is a mapping of name, description and required",
            ));
        };
        let mut input = Input::default();
        for (k, kline, val) in ms {
            match k.as_str() {
                "name" => {
                    input.name = text1(val, "an input's name", 32)?;
                    if !input_name(&input.name) {
                        return Err(err(
                            at(val.line),
                            format!(
                                "{} is not an input's name: lower case letters, digits and '_', \
                                 starting with a letter",
                                quoted(&input.name)
                            ),
                        ));
                    }
                }
                "description" => {
                    input.description = text1(val, "an input's description", MAX_TEXT)?;
                }
                "required" => match val.value {
                    Y::Bool(b) => input.required = b,
                    _ => {
                        return Err(err(at(val.line), "an input's required is true or false"));
                    }
                },
                _ => {
                    return Err(err(
                        at(*kline),
                        format!(
                            "{} is not a key of an input: name, description and required are",
                            quoted(k)
                        ),
                    ));
                }
            }
        }
        if input.name.is_empty() {
            return Err(err(at(e.line), "an input has no name"));
        }
        if out.iter().any(|x| x.name == input.name) {
            return Err(err(
                at(e.line),
                format!("the input {} is given twice", input.name),
            ));
        }
        out.push(input);
    }
    Ok(out)
}

impl Skill {
    /// The input `name`.
    #[must_use]
    pub fn input(&self, name: &str) -> Option<&Input> {
        self.inputs.iter().find(|i| i.name == name)
    }

    /// The members fit's `Skill` writes, without the braces' end: the prompt left out when
    /// `whole` is false.
    fn members(&self, whole: bool) -> Obj {
        let mut o = Obj::new()
            .str("name", &self.name)
            .str("title", &self.title)
            .str_omit("title_zh", &self.title_zh)
            .str("description", &self.description)
            .str_omit("description_zh", &self.description_zh);
        if !self.inputs.is_empty() {
            let items: Vec<String> = self
                .inputs
                .iter()
                .map(|i| {
                    let mut o = Obj::new()
                        .str("name", &i.name)
                        .str_omit("description", &i.description);
                    if i.required {
                        o = o.bool("required", true);
                    }
                    o.end()
                })
                .collect();
            o = o.raw("inputs", &format!("[{}]", items.join(",")));
        }
        o = o
            .raw("tools", &json::strings(&self.tools))
            .str_omit("model", &self.model);
        if !self.examples.is_empty() {
            o = o.raw("examples", &json::strings(&self.examples));
        }
        if whole {
            o = o.str_omit("prompt", &self.prompt);
        }
        o
    }

    /// The skill's prompt as the model is given it: akit's own instructions for the task, which
    /// cannot close their own block.
    #[must_use]
    pub fn block(&self) -> String {
        format!(
            "<skill name=\"{}\">\n{}\n</skill>",
            self.name,
            self.prompt.replace("</skill", "<\\/skill")
        )
    }
}

/// The built-in skills, in their order. One that does not parse is a fault of the build, which
/// a test catches; it is left out.
#[must_use]
pub fn builtin() -> Vec<Skill> {
    BUILTIN
        .iter()
        .filter_map(|(name, src)| parse(src.as_bytes()).ok().filter(|s| s.name == *name))
        .collect()
}

/// A skill a user added, and who and when.
#[derive(Debug, Clone)]
struct UserSkill {
    skill: Arc<Skill>,
    disabled: bool,
    by: String,
    at: i64,
}

/// The skills as they stand: never changed once made, so that a chat reads them without a lock
/// held; a change makes a new one.
#[derive(Debug, Clone, Default)]
pub(crate) struct Snap {
    builtin: Vec<Arc<Skill>>,
    user: Vec<UserSkill>,
    /// The built-ins disabled.
    off: Vec<String>,
}

impl Snap {
    /// The skill `name`, whether it is built in, and whether it is enabled.
    fn find(&self, name: &str) -> Option<(Arc<Skill>, bool, bool)> {
        if let Some(b) = self.builtin.iter().find(|b| b.name == name) {
            return Some((Arc::clone(b), true, !self.off.iter().any(|o| o == name)));
        }
        self.user
            .iter()
            .find(|u| u.skill.name == name)
            .map(|u| (Arc::clone(&u.skill), false, !u.disabled))
    }

    fn enabled(&self) -> Vec<Arc<Skill>> {
        self.builtin
            .iter()
            .filter(|b| !self.off.contains(&b.name))
            .chain(self.user.iter().filter(|u| !u.disabled).map(|u| &u.skill))
            .cloned()
            .collect()
    }

    /// As the store keeps it, fit's `keptSkills`.
    fn kept(&self) -> String {
        let mut o = Obj::new();
        if !self.user.is_empty() {
            let items: Vec<String> = self
                .user
                .iter()
                .map(|u| {
                    let mut o = Obj::new().str("source", &u.skill.source);
                    if u.disabled {
                        o = o.bool("disabled", true);
                    }
                    o = o.str_omit("by", &u.by);
                    if u.at != 0 {
                        o = o.num("at", u.at);
                    }
                    o.end()
                })
                .collect();
            o = o.raw("user", &format!("[{}]", items.join(",")));
        }
        let off: Vec<&String> = self
            .builtin
            .iter()
            .map(|b| &b.name)
            .filter(|n| self.off.contains(n))
            .collect();
        if !off.is_empty() {
            o = o.raw("disabled", &json::strings(&off));
        }
        o.end()
    }
}

/// The assistant's skills: the snapshot in use, and the lock that orders the changes, each
/// written to the store before it is used.
#[derive(Debug, Default)]
pub(crate) struct Skills {
    cur: Mutex<Arc<Snap>>,
    change: Mutex<()>,
}

/// A tool a skill names, as a user may call it: ok, or why not.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ToolState {
    /// The tool.
    pub name: String,
    /// Whether the user may call it.
    pub ok: bool,
    /// Why not.
    pub why: String,
}

/// The tools of `base` a chat running `sk` offers: those it names, and `ask_user`.
#[must_use]
pub fn narrow(base: &[Tool], sk: &Skill) -> Vec<Tool> {
    base.iter()
        .filter(|t| t.name == "ask_user" || sk.tools.iter().any(|n| n == t.name))
        .cloned()
        .collect()
}

/// The provider and model a chat running `sk` calls: the skill's own when it names one, as
/// `provider/model` of a provider the settings have, or a model of the default provider; `prov`
/// and `model` else.
#[must_use]
pub fn model_of(s: &Settings, sk: Option<&Skill>, prov: &str, model: &str) -> (String, String) {
    let Some(sk) = sk.filter(|sk| !sk.model.is_empty()) else {
        return (prov.to_owned(), model.to_owned());
    };
    if let Some((p, m)) = sk.model.split_once('/') {
        if !m.is_empty() && s.provider(p).is_some() {
            return (p.to_owned(), m.to_owned());
        }
    }
    (prov.to_owned(), sk.model.clone())
}

/// A catalog's entry: a skill's name and English description, as the system prompt lists it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Entry {
    /// See [`Entry`].
    pub name: String,
    /// See [`Entry`].
    pub description: String,
}

/// The catalog within `max` bytes, when not 0: each skill with its description, or, when they do
/// not fit, the names alone, as many as fit. fit's `catalogText`.
#[must_use]
pub fn catalog_text(catalog: &[Entry], max: usize) -> String {
    let full: String = catalog
        .iter()
        .map(|s| format!("\n- {}: {}", s.name, s.description))
        .collect();
    if max == 0 || full.len() <= max {
        return full;
    }
    let mut names = String::from(" (by name only: the window is small)");
    for (i, s) in catalog.iter().enumerate() {
        if names.len().saturating_add(s.name.len()).saturating_add(4) > max {
            names.push_str(&format!("\n- and {} more", catalog.len().saturating_sub(i)));
            break;
        }
        names.push_str("\n- ");
        names.push_str(&s.name);
    }
    names
}

fn lock<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap_or_else(PoisonError::into_inner)
}

impl Assistant {
    /// Reads the built-in skills and the store's: a user's skill that no longer parses, as after
    /// an upgrade, is left out and said in the log, and is gone from the store at the next
    /// change. On the control plane, at the start.
    pub(crate) fn load_skills(&self) {
        let builtin: Vec<Arc<Skill>> = builtin()
            .into_iter()
            // The cluster's skills are a manager's (I169).
            .filter(|s| !CLUSTER_SKILLS.contains(&s.name.as_str()))
            .map(Arc::new)
            .collect();
        let mut snap = Snap {
            builtin,
            ..Snap::default()
        };
        let sec = self.0.o.store.as_ref().and_then(|s| s.section(SECTION));
        if let Some(sec) = sec {
            let v = std::str::from_utf8(&sec)
                .ok()
                .and_then(|t| akit_config::value::parse(t).ok())
                .filter(|v| matches!(v, Value::Object(_)));
            match v {
                None => (self.0.o.log)(
                    "assistant: the skills kept in the store do not load, and only the built-in \
                     ones are offered"
                        .to_owned(),
                ),
                Some(v) => self.read_kept(&mut snap, &v),
            }
        }
        *lock(&self.0.skills.cur) = Arc::new(snap);
    }

    fn read_kept(&self, snap: &mut Snap, v: &Value) {
        if let Some(Value::Array(off)) = v.get("disabled") {
            snap.off = off
                .iter()
                .filter_map(|x| match x {
                    Value::String(s) => Some(s.clone()),
                    _ => None,
                })
                .collect();
        }
        let Some(Value::Array(user)) = v.get("user") else {
            return;
        };
        for k in user {
            let r = parse(json::str_of(k, "source").as_bytes())
                .map_err(|e| e.to_string())
                .and_then(|sk| match snap.find(&sk.name) {
                    Some(_) => Err(format!("the name {} is taken", sk.name)),
                    None => Ok(sk),
                });
            match r {
                Ok(sk) => snap.user.push(UserSkill {
                    skill: Arc::new(sk),
                    disabled: matches!(k.get("disabled"), Some(Value::Bool(true))),
                    by: json::str_of(k, "by").to_owned(),
                    at: match k.get("at") {
                        Some(Value::Number(n)) => n.parse().unwrap_or(0),
                        _ => 0,
                    },
                }),
                Err(e) => (self.0.o.log)(format!(
                    "assistant: a skill kept in the store does not load, and is left out: {e}"
                )),
            }
        }
    }

    pub(crate) fn skill_snap(&self) -> Arc<Snap> {
        Arc::clone(&lock(&self.0.skills.cur))
    }

    /// The skills users may run, the built-ins first: what the chat's catalog lists, and what an
    /// MCP server (I166) serves as its prompts.
    #[must_use]
    pub fn enabled_skills(&self) -> Vec<Arc<Skill>> {
        self.skill_snap().enabled()
    }

    /// The enabled skill `name`.
    #[must_use]
    pub fn enabled_skill(&self, name: &str) -> Option<Arc<Skill>> {
        match self.skill_snap().find(name) {
            Some((sk, _, true)) => Some(sk),
            _ => None,
        }
    }

    /// The catalog of the enabled skills, as the system prompt lists them for the model to pick
    /// from: names and English descriptions.
    #[must_use]
    pub fn catalog(&self) -> Vec<Entry> {
        self.enabled_skills()
            .iter()
            .map(|s| Entry {
                name: s.name.clone(),
                description: s.description.clone(),
            })
            .collect()
    }

    /// Makes a change to the skills: `f` changes a copy, which is written to the store and then
    /// used; `f`'s refusal, a status and why, refuses it. It writes a file: on the pool.
    fn change_skills(
        &self,
        f: impl FnOnce(&mut Snap) -> Result<(), (u16, String)>,
    ) -> Result<(), (u16, String)> {
        let _order = lock(&self.0.skills.change);
        let mut next = (*self.skill_snap()).clone();
        f(&mut next)?;
        if let Some(store) = &self.0.o.store {
            store
                .set_section(SECTION, next.kept().as_bytes())
                .map_err(|e| (500, format!("saving the skills: {e}")))?;
        }
        *lock(&self.0.skills.cur) = Arc::new(next);
        Ok(())
    }

    /// For each tool `sk` names, whether a user of `level` may call it under `s`.
    #[must_use]
    pub fn skill_tools(&self, sk: &Skill, level: Level, s: &Settings) -> Vec<ToolState> {
        sk.tools
            .iter()
            .map(|name| match self.registry().get(name) {
                None => ToolState {
                    name: name.clone(),
                    ok: false,
                    why: "this akit has no such tool".to_owned(),
                },
                Some(t) => match t.allowed(level, s) {
                    Ok(()) => ToolState {
                        name: name.clone(),
                        ok: true,
                        why: String::new(),
                    },
                    Err(why) => ToolState {
                        name: name.clone(),
                        ok: false,
                        why,
                    },
                },
            })
            .collect()
    }

    /// `sk` as a user of `level` sees it on the Skills page; with its prompt and Markdown when
    /// `whole`.
    fn skill_view(
        &self,
        snap: &Snap,
        sk: &Skill,
        level: Level,
        s: &Settings,
        whole: bool,
    ) -> String {
        let (builtin, enabled) = snap
            .find(&sk.name)
            .map_or((false, false), |(_, b, e)| (b, e));
        let mut o = sk
            .members(whole)
            .bool("builtin", builtin)
            .bool("enabled", enabled);
        if let Some(u) = snap.user.iter().find(|u| u.skill.name == sk.name) {
            o = o.str_omit("by", &u.by);
            if u.at != 0 {
                o = o.num("at", u.at);
            }
        }
        let state: Vec<String> = self
            .skill_tools(sk, level, s)
            .iter()
            .map(|t| {
                Obj::new()
                    .str("name", &t.name)
                    .bool("ok", t.ok)
                    .str_omit("why", &t.why)
                    .end()
            })
            .collect();
        o = o.raw("tool_state", &format!("[{}]", state.join(",")));
        if whole {
            o = o.str_omit("source", &sk.source);
        }
        o.end()
    }

    /// `/api/skills[/<name>[/<action>]]`.
    pub(crate) fn skills_endpoint(&self, ctx: &mut Context, user: &str, rest: &str) {
        let method = ctx.method().clone();
        let (name, action) = rest.split_once('/').unwrap_or((rest, ""));
        match (name, action, &method) {
            ("", "", Method::Get) => {
                let s = self.settings();
                let level = s.level(user);
                let snap = self.skill_snap();
                let list: Vec<String> = snap
                    .builtin
                    .iter()
                    .chain(snap.user.iter().map(|u| &u.skill))
                    .map(|sk| self.skill_view(&snap, sk, level, &s, false))
                    .collect();
                ctx.respond(pages::json(
                    200,
                    Obj::new()
                        .raw("skills", &format!("[{}]", list.join(",")))
                        .str("level", level.name())
                        .bool("can_change", level >= CHANGE_LEVEL)
                        .str("change_level", CHANGE_LEVEL.name())
                        .bool("kept", self.0.o.store.is_some())
                        .num("max", MAX_USER_SKILLS)
                        .num("max_bytes", MAX_SKILL_BYTES)
                        .end(),
                ));
            }
            ("", "", Method::Post) => self.skill_change(ctx, user, Change::Add),
            ("", "", _) => ctx.respond(pages::fail(405, "GET the skills, or POST one")),
            (name, "", Method::Get) => {
                let s = self.settings();
                let snap = self.skill_snap();
                match snap.find(name) {
                    Some((sk, _, _)) => ctx.respond(pages::json(
                        200,
                        self.skill_view(&snap, &sk, s.level(user), &s, true),
                    )),
                    None => ctx.respond(pages::fail(404, &format!("no skill {name}"))),
                }
            }
            (name, "", Method::Put) => self.skill_change(ctx, user, Change::Put(name.to_owned())),
            (name, "", Method::Delete) => {
                self.skill_change(ctx, user, Change::Delete(name.to_owned()));
            }
            (name, "enable" | "disable", Method::Post) => {
                let off = action == "disable";
                self.skill_change(ctx, user, Change::Switch(name.to_owned(), off));
            }
            _ => ctx.respond(pages::fail(
                405,
                "GET, PUT or DELETE a skill, or POST its enable or disable",
            )),
        }
    }

    /// A change of the skills: refused below [`CHANGE_LEVEL`], its body read here, and made on
    /// the pool, which writes the store.
    fn skill_change(&self, ctx: &mut Context, user: &str, change: Change) {
        let level = self.level(user);
        if level < CHANGE_LEVEL {
            return ctx.respond(pages::fail(
                403,
                &format!(
                    "changing the skills needs the level {}; {user}'s is {}",
                    CHANGE_LEVEL.name(),
                    level.name()
                ),
            ));
        }
        let source = match change {
            Change::Add | Change::Put(_) => match pages::decode(ctx) {
                Ok(b) => Some(json::str_of(&b.0, "source").to_owned()),
                Err(r) => return ctx.respond(r),
            },
            _ => None,
        };
        let Some(d) = ctx.detach() else { return };
        let (me, user) = (self.clone(), user.to_owned());
        self.run(d, move |d| {
            let sk = match source.as_deref().map(|s| parse(s.as_bytes())) {
                Some(Err(e)) => {
                    let mut o = Obj::new().str("error", &e.to_string());
                    if e.line > 0 {
                        o = o.num("line", e.line);
                    }
                    return d.respond(pages::json(400, o.end()));
                }
                Some(Ok(sk)) => Some(Arc::new(sk)),
                None => None,
            };
            let (r, name, did) = me.make_change(&user, change, sk);
            match r {
                Err((status, msg)) => d.respond(pages::fail(status, &msg)),
                Ok(()) => {
                    (me.0.o.log)(format!(
                        "console: {user:?} {did} the assistant's skill {name}"
                    ));
                    let s = me.settings();
                    let snap = me.skill_snap();
                    match snap.find(&name) {
                        Some((sk, _, _)) => d.respond(pages::json(
                            200,
                            me.skill_view(&snap, &sk, s.level(&user), &s, true),
                        )),
                        None => d.respond(pages::json(200, Obj::new().str("deleted", &name).end())),
                    }
                }
            }
        });
    }

    fn make_change(
        &self,
        user: &str,
        change: Change,
        sk: Option<Arc<Skill>>,
    ) -> (Result<(), (u16, String)>, String, &'static str) {
        let now = crate::conv::now_ms();
        match (change, sk) {
            (Change::Add, Some(sk)) => {
                let name = sk.name.clone();
                let r = self.change_skills(|s| {
                    if let Some((_, builtin, _)) = s.find(&sk.name) {
                        return Err((409, name_taken(&sk.name, builtin)));
                    }
                    if s.user.len() >= MAX_USER_SKILLS {
                        return Err((
                            409,
                            format!(
                                "there are {MAX_USER_SKILLS} skills added, as many as may be: \
                                 delete one first"
                            ),
                        ));
                    }
                    s.user.push(UserSkill {
                        skill: sk,
                        disabled: false,
                        by: user.to_owned(),
                        at: now,
                    });
                    Ok(())
                });
                (r, name, "added")
            }
            (Change::Put(name), Some(sk)) => {
                let new = sk.name.clone();
                let r = self.change_skills(|s| {
                    let Some(i) = s.user.iter().position(|u| u.skill.name == name) else {
                        if s.find(&name).is_some_and(|(_, b, _)| b) {
                            return Err((
                                409,
                                "a built-in skill is not changed: export it, give it another \
                                 name, and add it as yours"
                                    .to_owned(),
                            ));
                        }
                        return Err((404, format!("no skill {name} was added")));
                    };
                    if sk.name != name {
                        if let Some((_, builtin, _)) = s.find(&sk.name) {
                            return Err((409, name_taken(&sk.name, builtin)));
                        }
                    }
                    let disabled = s.user.get(i).is_some_and(|u| u.disabled);
                    if let Some(slot) = s.user.get_mut(i) {
                        *slot = UserSkill {
                            skill: sk,
                            disabled,
                            by: user.to_owned(),
                            at: now,
                        };
                    }
                    Ok(())
                });
                (r, new, "changed")
            }
            (Change::Delete(name), _) => {
                let r = self.change_skills(|s| {
                    let Some(i) = s.user.iter().position(|u| u.skill.name == name) else {
                        if s.find(&name).is_some_and(|(_, b, _)| b) {
                            return Err((
                                409,
                                "a built-in skill is not deleted: disable it".into(),
                            ));
                        }
                        return Err((404, format!("no skill {name} was added")));
                    };
                    s.user.remove(i);
                    Ok(())
                });
                (r, name, "deleted")
            }
            (Change::Switch(name, off), _) => {
                let r = self.change_skills(|s| {
                    if let Some(u) = s.user.iter_mut().find(|u| u.skill.name == name) {
                        u.disabled = off;
                        return Ok(());
                    }
                    if s.find(&name).is_some_and(|(_, b, _)| b) {
                        s.off.retain(|o| *o != name);
                        if off {
                            s.off.push(name.clone());
                        }
                        return Ok(());
                    }
                    Err((404, format!("no skill {name}")))
                });
                (r, name, if off { "disabled" } else { "enabled" })
            }
            (_, None) => (Err((400, "no skill".into())), String::new(), ""),
        }
    }
}

/// A change the Skills page asks for.
#[derive(Debug)]
enum Change {
    Add,
    Put(String),
    Delete(String),
    Switch(String, bool),
}

fn name_taken(name: &str, builtin: bool) -> String {
    if builtin {
        format!(
            "a built-in skill is named {name}: give yours another name, and disable the built-in \
             one if yours replaces it"
        )
    } else {
        format!("a skill is named {name} already")
    }
}

/// `skill_load`: an enabled skill's whole prompt, which from then on is the chat's current skill.
#[must_use]
pub fn load_tool() -> Tool {
    Tool {
        name: "skill_load",
        group: "agent",
        level: Level::Read,
        description: "Load a skill of the catalog the system prompt lists, by its name: its whole prompt, which says how to do the task, after which your tools are the ones it names that the person's level allows. Call it when the person's request fits a skill, before you start on the task; there is one skill at a time.",
        parameters: r#"{"type":"object","properties":{"name":{"type":"string","description":"the skill's name, as the catalog lists it"}},"required":["name"]}"#,
        max_bytes: 0,
        kind: Kind::Pool,
        enabled: None,
        run: Some(run_load),
    }
}

fn run_load(c: &Call<'_>) -> Result<String, String> {
    let name = c.str("name")?;
    let Some(sk) = c.a.enabled_skill(&name) else {
        return Err(format!(
            "no enabled skill is named {}; the catalog in the system prompt lists them",
            quoted(&name)
        ));
    };
    let mut b = format!(
        "The skill {} ({}) is loaded: follow it for this task.",
        sk.name, sk.title
    );
    if let Some(chat) = c.chat {
        let (ok, not) = chat.load_skill(&sk, c.expired());
        b.push_str(&format!(" Your tools now: {}.", ok.join(", ")));
        if !not.is_empty() {
            b.push_str(&format!(
                " Tools it names that you may not call: {}.",
                not.join("; ")
            ));
        }
    }
    b.push_str("\n\n");
    b.push_str(&sk.block());
    Ok(b)
}

/// The inputs a chat gives its skill, checked against `sk`.
///
/// # Errors
///
/// The status and why.
pub fn check_inputs(sk: &Skill, inputs: &BTreeMap<String, String>) -> Result<(), (u16, String)> {
    if inputs.len() > MAX_INPUTS {
        return Err((400, format!("a skill takes at most {MAX_INPUTS} inputs")));
    }
    for (k, v) in inputs {
        if sk.input(k).is_none() {
            return Err((400, format!("the skill {} takes no input {k}", sk.name)));
        }
        if v.len() > MAX_INPUT_VALUE {
            return Err((
                400,
                format!("the input {k} is at most {MAX_INPUT_VALUE} bytes of text"),
            ));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests;
