//! The tools the model may call (I162, fit's `assistant/registry.go`): each a name, a group, a
//! JSON Schema of its arguments, the level a user needs for it, an optional bound of its result,
//! where a call runs, and whether the settings let it run.

use akit_config::assistant::{Assistant as Settings, Level};
use akit_config::value::Value;

use crate::Assistant;
use crate::json::Obj;

/// Where a tool's call runs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    /// On the assistant's tools' pool: every read tool.
    Pool,
    /// `ask_user`: the call is shown in the panel as a question with buttons, and the
    /// conversation waits for the choice, which comes back as the call's result.
    Ask,
}

/// A call being run.
#[derive(Debug)]
pub struct Call<'a> {
    /// The assistant.
    pub a: &'a Assistant,
    /// The call's arguments, a JSON object (an empty one when it gave none).
    pub args: Value,
    /// The console user the chat is for.
    pub user: &'a str,
    /// The settings the chat runs with.
    pub settings: &'a Settings,
}

impl Call<'_> {
    /// String argument `k`, `""` when left out.
    ///
    /// # Errors
    ///
    /// It is there and not a string.
    pub fn str(&self, k: &str) -> Result<String, String> {
        match self.args.get(k) {
            None | Some(Value::Null) => Ok(String::new()),
            Some(Value::String(s)) => Ok(s.clone()),
            Some(_) => Err(format!(
                "the arguments are not what the tool takes: {k} is not a string"
            )),
        }
    }

    /// Integer argument `k`, 0 when left out.
    ///
    /// # Errors
    ///
    /// It is there and not a whole number of 0 or more.
    pub fn uint(&self, k: &str) -> Result<u64, String> {
        match self.args.get(k) {
            None | Some(Value::Null) => Ok(0),
            Some(Value::Number(n)) => n.parse::<u64>().map_err(|_| {
                format!("the arguments are not what the tool takes: {k} is not a whole number")
            }),
            Some(_) => Err(format!(
                "the arguments are not what the tool takes: {k} is not a number"
            )),
        }
    }

    /// Boolean argument `k`, false when left out.
    #[must_use]
    pub fn flag(&self, k: &str) -> bool {
        matches!(self.args.get(k), Some(Value::Bool(true)))
    }

    /// String list argument `k`, empty when left out.
    ///
    /// # Errors
    ///
    /// It is there and not a list of strings.
    pub fn strings(&self, k: &str) -> Result<Vec<String>, String> {
        match self.args.get(k) {
            None | Some(Value::Null) => Ok(Vec::new()),
            Some(Value::Array(a)) => a
                .iter()
                .map(|x| match x {
                    Value::String(s) => Ok(s.clone()),
                    _ => Err(format!(
                        "the arguments are not what the tool takes: {k} holds a non-string"
                    )),
                })
                .collect(),
            Some(_) => Err(format!(
                "the arguments are not what the tool takes: {k} is not a list"
            )),
        }
    }
}

/// What a tool's call runs: its result for the model (text or JSON, redacted and bounded after),
/// or why it failed.
pub type Run = fn(&Call<'_>) -> Result<String, String>;

/// Whether the settings let a tool run, and why not.
pub type Enabled = fn(&Settings) -> Result<(), &'static str>;

/// A function the model may call.
#[derive(Debug, Clone)]
pub struct Tool {
    /// How the model calls it.
    pub name: &'static str,
    /// What the Functions page sorts it by.
    pub group: &'static str,
    /// The least level a user needs for it.
    pub level: Level,
    /// What it does and when to call it.
    pub description: &'static str,
    /// The JSON Schema of its arguments, an object.
    pub parameters: &'static str,
    /// Its result's bound below `max_tool_result_bytes`, when not 0.
    pub max_bytes: u64,
    /// Where a call runs.
    pub kind: Kind,
    /// Whether the settings let it run.
    pub enabled: Option<Enabled>,
    /// What a call runs (a [`Kind::Pool`] tool's).
    pub run: Option<Run>,
}

/// The groups, in the Functions page's order.
pub const GROUPS: &[&str] = &[
    "config",
    "runtime",
    "roles",
    "media",
    "sync",
    "bittorrent",
    "cluster",
    "logs",
    "page",
    "change",
    "convert",
    "agent",
];

fn group_rank(g: &str) -> usize {
    GROUPS.iter().position(|x| *x == g).unwrap_or(GROUPS.len())
}

impl Tool {
    /// Whether a user of `level` may call it under `s`, and why not.
    ///
    /// # Errors
    ///
    /// Why not.
    pub fn allowed(&self, level: Level, s: &Settings) -> Result<(), String> {
        if level < self.level {
            return Err(format!("needs the level {}", self.level.name()));
        }
        match self.enabled {
            Some(f) => f(s).map_err(str::to_owned),
            None => Ok(()),
        }
    }

    /// The tool as the model is offered it.
    #[must_use]
    pub fn model(&self) -> crate::model::Tool {
        crate::model::Tool {
            name: self.name.to_owned(),
            description: self.description.to_owned(),
            parameters: self.parameters.to_owned(),
        }
    }

    /// As `GET /api/tools` lists it, fit's `toolView`.
    #[must_use]
    pub fn view(&self, enabled: Result<(), String>) -> String {
        let mut o = Obj::new()
            .str("name", self.name)
            .str("group", self.group)
            .str("level", self.level.name());
        if self.kind == Kind::Ask {
            o = o.str("kind", "ask");
        }
        o = o
            .str("description", self.description)
            .raw("parameters", self.parameters)
            .bool("enabled", enabled.is_ok());
        if let Err(why) = enabled {
            o = o.str("why", &why);
        }
        o.end()
    }
}

/// The tools, by group and name.
#[derive(Debug, Clone, Default)]
pub struct Registry(Vec<Tool>);

impl Registry {
    /// The registry of `tools`, sorted.
    ///
    /// # Panics
    ///
    /// Never at run time: a tool's name, level and parameters are this crate's own, checked by
    /// its tests.
    #[must_use]
    pub fn new(mut tools: Vec<Tool>) -> Registry {
        tools.sort_by(|a, b| {
            group_rank(a.group)
                .cmp(&group_rank(b.group))
                .then(a.name.cmp(b.name))
        });
        Registry(tools)
    }

    /// Every tool.
    #[must_use]
    pub fn tools(&self) -> &[Tool] {
        &self.0
    }

    /// The tool named `name`.
    #[must_use]
    pub fn get(&self, name: &str) -> Option<&Tool> {
        self.0.iter().find(|t| t.name == name)
    }

    /// The tools a user of `level` may call under `s`.
    #[must_use]
    pub fn for_level(&self, level: Level, s: &Settings) -> Vec<Tool> {
        self.0
            .iter()
            .filter(|t| t.allowed(level, s).is_ok())
            .cloned()
            .collect()
    }
}

/// Whether `name` is a tool's name: lower case letters, digits and `_`, a letter first, 64 at
/// most.
#[must_use]
pub fn valid_name(name: &str) -> bool {
    let b = name.as_bytes();
    !b.is_empty()
        && b.len() <= 64
        && b.first().is_some_and(u8::is_ascii_lowercase)
        && b.iter()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || *c == b'_')
}
