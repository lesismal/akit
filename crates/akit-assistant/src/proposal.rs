//! Proposals (I163, fit's `assistant/proposals.go`): the propose tools, each adding a step to the
//! conversation's one pending proposal; none changes anything. A proposal's steps are checked
//! together on the running config, as the Admin API in process gives it, and shown as a diff in
//! the three formats; a person whose level is apply applies it from the panel, with a second
//! confirm (`POST /api/proposals/<conversation>/apply`).
//!
//! A proposal's config steps are of one kind, applied as one change through its own endpoint:
//! patches merged into one `PUT /config/text`, the fixes of every reconcile step in one `POST
//! /config/reconcile`, or one `POST /rollback/{n}`. Cache purges go with any, after the config.

use akit_config::assistant::Level;
use akit_config::value::Value;

use crate::Assistant;
use crate::conv::{new_id, now_ms};
use crate::json::{self, Obj, compact, quoted};
use crate::prompt::{clip, cut_text};
use crate::redact::{HIDDEN, REDACTED, Redactor, secret_name, secret_path, selector_value};
use crate::registry::{Call, Kind, Tool};
use crate::tree::{self, OP_ADD, OP_REMOVE, OP_REPLACE};

/// The propose tools.
pub const CONFIG_PROPOSE: &str = "config_propose";
/// See [`CONFIG_PROPOSE`].
pub const RECONCILE_PROPOSE: &str = "reconcile_propose";
/// See [`CONFIG_PROPOSE`].
pub const ROLLBACK_PROPOSE: &str = "rollback_propose";
/// See [`CONFIG_PROPOSE`].
pub const PURGE_PROPOSE: &str = "cache_purge_propose";

/// The kinds of a proposal's config steps.
pub const KIND_PATCH: &str = "patch";
/// See [`KIND_PATCH`].
pub const KIND_RECONCILE: &str = "reconcile";
/// See [`KIND_PATCH`].
pub const KIND_ROLLBACK: &str = "rollback";

/// A proposal's steps.
pub const MAX_STEPS: usize = 32;
/// Its steps' arguments together, and so one step's: a change to a large member is made with ops.
pub const MAX_BYTES: usize = 256 << 10;
/// A step's summary.
pub const MAX_SUMMARY_BYTES: usize = 300;
/// A purge's URLs, and its tags.
const MAX_PURGE_ITEMS: usize = 100;
/// A step's unconverted items, and each one's bytes.
const MAX_UNCONVERTED: usize = 100;
const MAX_UNCONVERTED_BYTES: usize = 500;
/// Each format's diff.
const MAX_DIFF_BYTES: usize = 256 << 10;
/// The errors of a check a proposal lists.
const MAX_ERRORS: usize = 50;

/// The formats a proposal's diff is shown in.
pub const DIFF_FORMATS: [&str; 3] = ["conf", "yaml", "json"];

/// The header field of the Admin API's answers that gives the config's version.
pub const VERSION_HEADER: &str = "Akit-Config-Version";

/// A propose tool called outside a conversation: fit's `errNoConversation`.
pub const NO_CONVERSATION: &str = "the proposal has no conversation to be kept in";

/// A propose tool's call: its tool, its summary and its arguments, as the model gave them.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Step {
    /// The tool.
    pub tool: String,
    /// What the step does, in a sentence.
    pub summary: String,
    /// The arguments, compact JSON.
    pub args: String,
    /// When it was added, Unix milliseconds.
    pub at: i64,
}

impl Step {
    fn json(&self, args: &str) -> String {
        Obj::new()
            .str("tool", &self.tool)
            .str("summary", &self.summary)
            .raw("args", if args.is_empty() { "null" } else { args })
            .num("at", self.at)
            .end()
    }

    fn from(v: &Value) -> Step {
        Step {
            tool: json::str_of(v, "tool").to_owned(),
            summary: json::str_of(v, "summary").to_owned(),
            args: v.get("args").map(compact).unwrap_or_default(),
            at: int(v, "at"),
        }
    }
}

fn int(v: &Value, k: &str) -> i64 {
    match v.get(k) {
        Some(Value::Number(n)) => n.parse().unwrap_or(0),
        _ => 0,
    }
}

fn uint(v: &Value, k: &str) -> u64 {
    u64::try_from(int(v, k)).unwrap_or(0)
}

/// A conversation's pending change: the steps the model proposed, on the config whose ETag and
/// version they were checked on. Kept with its conversation until it is applied or discarded.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Proposal {
    /// Its ID.
    pub id: String,
    /// When it was made and last changed, Unix milliseconds.
    pub created: i64,
    /// See [`created`](Proposal::created).
    pub updated: i64,
    /// Counts its changes: a step added, or a move to a newer config. An apply names the
    /// revision it confirmed.
    pub revision: u64,
    /// The config's version its steps were last checked on.
    pub base_version: u64,
    /// That config's ETag.
    pub base_etag: String,
    /// Its steps, in order.
    pub steps: Vec<Step>,
    /// Whether the whole checked when it last changed, for the prompt; the view checks again.
    pub valid: bool,
    /// The problems of that check.
    pub problems: usize,
    /// Why the last apply failed, until the next.
    pub error: String,
}

impl Proposal {
    /// As fit writes it, in the store and to the model.
    #[must_use]
    pub fn json(&self) -> String {
        let steps: Vec<String> = self.steps.iter().map(|s| s.json(&s.args)).collect();
        let mut o = Obj::new()
            .str("id", &self.id)
            .num("created", self.created)
            .num("updated", self.updated)
            .num("revision", self.revision)
            .num("base_version", self.base_version)
            .str("base_etag", &self.base_etag)
            .raw("steps", &format!("[{}]", steps.join(",")))
            .bool("valid", self.valid);
        if self.problems > 0 {
            o = o.num("problems", self.problems);
        }
        o.str_omit("error", &self.error).end()
    }

    /// As fit's store keeps it; `None` for what is not a proposal.
    #[must_use]
    pub fn from(v: &Value) -> Option<Proposal> {
        let id = json::str_of(v, "id");
        if id.is_empty() {
            return None;
        }
        let steps = match v.get("steps") {
            Some(Value::Array(a)) => a.iter().map(Step::from).collect(),
            _ => Vec::new(),
        };
        Some(Proposal {
            id: id.to_owned(),
            created: int(v, "created"),
            updated: int(v, "updated"),
            revision: uint(v, "revision"),
            base_version: uint(v, "base_version"),
            base_etag: json::str_of(v, "base_etag").to_owned(),
            steps,
            valid: matches!(v.get("valid"), Some(Value::Bool(true))),
            problems: usize::try_from(uint(v, "problems")).unwrap_or(0),
            error: json::str_of(v, "error").to_owned(),
        })
    }

    /// About its steps' size, against [`MAX_BYTES`].
    #[must_use]
    pub fn bytes(&self) -> usize {
        self.steps.iter().fold(0usize, |n, s| {
            n.saturating_add(s.args.len())
                .saturating_add(s.summary.len())
                .saturating_add(64)
        })
    }
}

/// What the model is told of a conversation's proposal: the pending one's steps, or what became
/// of the last one, `note`. fit's `proposalState`.
#[must_use]
pub fn state(p: Option<&Proposal>, note: &str) -> String {
    let Some(p) = p else {
        return note.to_owned();
    };
    let mut b = format!(
        "Pending proposal {}, revision {}, on config version {}, {} steps:",
        p.id,
        p.revision,
        p.base_version,
        p.steps.len()
    );
    for (i, s) in p.steps.iter().enumerate() {
        b.push_str(&format!(
            "\n{}. {}: {}",
            i.saturating_add(1),
            s.tool,
            clip(&s.summary, MAX_SUMMARY_BYTES)
        ));
    }
    if !p.error.is_empty() {
        b.push_str(&format!(
            "\nIts last apply failed: {}",
            clip(&p.error, 1024)
        ));
    } else if !p.valid {
        b.push_str(&format!(
            "\nIt does not check ({} problems), so it cannot be applied until a step mends it.",
            p.problems
        ));
    } else {
        b.push_str("\nIt checks, and waits for the person to apply it.");
    }
    b
}

/// A Consistency fix chosen: its difference's ID and the action.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FixRef {
    /// The difference.
    pub id: String,
    /// The action of one of its fixes.
    pub action: String,
}

impl FixRef {
    /// As fit writes it.
    #[must_use]
    pub fn json(&self) -> String {
        Obj::new()
            .str("id", &self.id)
            .str("action", &self.action)
            .end()
    }
}

/// A cache purge a proposal makes after the config.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Purge {
    /// The zone; every zone when empty.
    pub zone: String,
    /// The absolute URLs.
    pub urls: Vec<String>,
    /// The tags.
    pub tags: Vec<String>,
    /// Everything.
    pub all: bool,
}

impl Purge {
    /// Its members as fit writes them, without the braces.
    #[must_use]
    pub fn members(&self) -> Obj {
        let mut o = Obj::new().str_omit("zone", &self.zone);
        if !self.urls.is_empty() {
            o = o.raw("urls", &json::strings(&self.urls));
        }
        if !self.tags.is_empty() {
            o = o.raw("tags", &json::strings(&self.tags));
        }
        if self.all {
            o = o.bool("all", true);
        }
        o
    }
}

/// A difference of `GET /config/check`, as far as a fix needs.
#[derive(Debug, Clone)]
struct CheckDiff {
    id: String,
    fixes: Vec<CheckFix>,
}

/// A fix a difference offers, and the patch it makes.
#[derive(Debug, Clone)]
struct CheckFix {
    action: String,
    patch: Option<(String, String, Option<Value>)>,
}

fn check_diffs(text: &str) -> Result<Vec<CheckDiff>, String> {
    let v = akit_config::value::parse(text).map_err(|_| "the answer is not JSON".to_owned())?;
    let Some(Value::Array(ds)) = v.get("differences") else {
        return Ok(Vec::new());
    };
    Ok(ds
        .iter()
        .map(|d| CheckDiff {
            id: json::str_of(d, "id").to_owned(),
            fixes: match d.get("fixes") {
                Some(Value::Array(fs)) => fs
                    .iter()
                    .map(|f| CheckFix {
                        action: json::str_of(f, "action").to_owned(),
                        patch: f.get("patch").filter(|p| **p != Value::Null).map(|p| {
                            (
                                json::str_of(p, "op").to_owned(),
                                json::str_of(p, "path").to_owned(),
                                p.get("value").cloned(),
                            )
                        }),
                    })
                    .collect(),
                _ => Vec::new(),
            },
        })
        .collect())
}

/// A config error a check lists: its path and its message.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Problem {
    /// Where, a dotted path; empty for the whole.
    pub path: String,
    /// What.
    pub msg: String,
}

impl Problem {
    fn json(&self) -> String {
        Obj::new()
            .str_omit("path", &self.path)
            .str("msg", &self.msg)
            .end()
    }

    fn whole(msg: impl Into<String>) -> Problem {
        Problem {
            path: String::new(),
            msg: msg.into(),
        }
    }
}

/// Problems as a JSON array.
#[must_use]
pub fn problems_json(ps: &[Problem]) -> String {
    let items: Vec<String> = ps.iter().map(Problem::json).collect();
    format!("[{}]", items.join(","))
}

/// A proposal checked on the running config: what its steps make of it, each step's error, and
/// the check of the whole.
#[derive(Debug)]
pub struct Built {
    /// The running config's ETag and version, as `GET /config` gave them.
    pub etag: String,
    /// See [`etag`](Built::etag).
    pub version: u64,
    /// The running config.
    pub base: Value,
    /// What the steps make of it.
    pub after: Value,
    /// The kind of its config steps; empty for none.
    pub kind: &'static str,
    /// The fixes of its reconcile steps.
    pub fixes: Vec<FixRef>,
    /// The version its rollback applies again, or 0.
    pub rollback: u64,
    /// Its purges, in order.
    pub purges: Vec<Purge>,
    /// Each step's error; empty for none.
    pub step_errs: Vec<String>,
    /// The top-level members it changes: those set, then those removed.
    pub changed: Vec<String>,
    /// Whether the whole checks.
    pub valid: bool,
    /// What the check of the whole found.
    pub errors: Vec<Problem>,
    check: Option<Vec<CheckDiff>>,
    zones: Option<Vec<String>>,
    fix_at: Vec<usize>,
    reread: Option<String>,
}

/// The arguments of a step, read as fit's `stepArgs` reads them.
struct Args<'v>(&'v Value);

impl Args<'_> {
    fn bad(tool: &str, why: &str) -> String {
        format!("the arguments are not what {tool} takes: {why}")
    }

    fn str(&self, tool: &str, k: &str) -> Result<String, String> {
        match self.0.get(k) {
            None | Some(Value::Null) => Ok(String::new()),
            Some(Value::String(s)) => Ok(s.clone()),
            Some(_) => Err(Self::bad(tool, &format!("{k} is not a string"))),
        }
    }

    fn strings(&self, tool: &str, k: &str) -> Result<Vec<String>, String> {
        match self.0.get(k) {
            None | Some(Value::Null) => Ok(Vec::new()),
            Some(Value::Array(a)) => a
                .iter()
                .map(|x| match x {
                    Value::String(s) => Ok(s.clone()),
                    _ => Err(Self::bad(tool, &format!("{k} holds a non-string"))),
                })
                .collect(),
            Some(_) => Err(Self::bad(tool, &format!("{k} is not a list"))),
        }
    }

    fn uint(&self, tool: &str, k: &str) -> Result<u64, String> {
        match self.0.get(k) {
            None | Some(Value::Null) => Ok(0),
            Some(Value::Number(n)) => n
                .parse()
                .map_err(|_| Self::bad(tool, &format!("{k} is not a whole number"))),
            Some(_) => Err(Self::bad(tool, &format!("{k} is not a number"))),
        }
    }

    fn flag(&self, tool: &str, k: &str) -> Result<bool, String> {
        match self.0.get(k) {
            None | Some(Value::Null) => Ok(false),
            Some(Value::Bool(b)) => Ok(*b),
            Some(_) => Err(Self::bad(tool, &format!("{k} is not true or false"))),
        }
    }

    fn list(&self, tool: &str, k: &str) -> Result<&[Value], String> {
        match self.0.get(k) {
            None | Some(Value::Null) => Ok(&[]),
            Some(Value::Array(a)) => Ok(a),
            Some(_) => Err(Self::bad(tool, &format!("{k} is not a list"))),
        }
    }
}

/// Refuses a path into `admin`: the members no tool reads are in it, and it changes only when
/// akit restarts, so no proposal touches it.
fn forbidden<S: AsRef<str>>(tokens: &[S]) -> Result<(), String> {
    if tokens.first().is_some_and(|t| t.as_ref() == "admin") {
        return Err(
            "admin is out of the assistant's reach: it holds the console's users, the \
                    store and the Admin API's own settings, and changes only when akit restarts; \
                    the person changes it in the config file"
                .to_owned(),
        );
    }
    Ok(())
}

/// Whether `s` holds what redaction put in place of a secret.
fn is_redacted(s: &str) -> bool {
    s.contains("[redacted")
}

/// The running config, as `GET /config` answers it: the config, its ETag and its version.
fn current(a: &Assistant) -> Result<(Value, String, u64), String> {
    let ans = a.api_call("GET", "/config", Vec::new())?;
    if ans.status != 200 {
        return Err(format!(
            "reading the config: the Admin API answered {}",
            ans.status
        ));
    }
    let version = ans
        .header(VERSION_HEADER)
        .and_then(|v| v.parse().ok())
        .unwrap_or(0);
    let etag = ans.header("ETag").unwrap_or("").to_owned();
    match akit_config::value::parse(&ans.text()) {
        Ok(v @ Value::Object(_)) => Ok((v, etag, version)),
        _ => Err("the running config is not a JSON object".to_owned()),
    }
}

/// Checks `steps` on the running config. Its error is only the config not being read; what is
/// wrong with a step is that step's error.
///
/// # Errors
///
/// The running config could not be read, or is not a JSON object.
pub fn build(a: &Assistant, red: &Redactor, steps: &[Step]) -> Result<Built, String> {
    let (base, etag, version) = current(a)?;
    let mut b = Built {
        etag,
        version,
        after: base.clone(),
        base,
        kind: "",
        fixes: Vec::new(),
        rollback: 0,
        purges: Vec::new(),
        step_errs: vec![String::new(); steps.len()],
        changed: Vec::new(),
        valid: false,
        errors: Vec::new(),
        check: None,
        zones: None,
        fix_at: Vec::new(),
        reread: None,
    };
    for (i, st) in steps.iter().enumerate() {
        if let Err(e) = b.step(a, red, i, st) {
            if let Some(slot) = b.step_errs.get_mut(i) {
                *slot = e;
            }
        }
    }
    if b.kind == KIND_RECONCILE {
        b.apply_fixes();
    }
    if let (Value::Object(after), Value::Object(base)) = (&b.after, &b.base) {
        for (k, v) in after {
            if b.base.get(k).is_none_or(|w| !tree::equal(w, v)) {
                b.changed.push(k.clone());
            }
        }
        for (k, _) in base {
            if b.after.get(k).is_none() {
                b.changed.push(k.clone());
            }
        }
    }
    b.valid = b.step_errs.iter().all(String::is_empty);
    if b.kind == KIND_PATCH && b.changed.is_empty() && b.purges.is_empty() {
        b.valid = false;
        b.errors
            .push(Problem::whole("the proposal changes nothing"));
    } else if !b.kind.is_empty() && b.valid && !b.changed.is_empty() {
        b.errors = validate(a, red, &b.after);
        b.valid = b.errors.is_empty();
    }
    Ok(b)
}

/// What the Admin API's check finds in `config`, as `POST /config/validate` answers it, redacted:
/// at most [`MAX_ERRORS`], and how many more.
fn validate(a: &Assistant, red: &Redactor, config: &Value) -> Vec<Problem> {
    let text = match a.api_json("POST", "/config/validate", compact(config).into_bytes()) {
        Ok(t) => t,
        Err(e) => return vec![Problem::whole(red.text(&e))],
    };
    let Ok(v) = akit_config::value::parse(&text) else {
        return vec![Problem::whole("the check's answer is not JSON")];
    };
    if matches!(v.get("valid"), Some(Value::Bool(true))) {
        return Vec::new();
    }
    let mut out = Vec::new();
    let errs = match v.get("errors") {
        Some(Value::Array(a)) => a.as_slice(),
        _ => &[],
    };
    for e in errs.iter().take(MAX_ERRORS) {
        let msg = match json::str_of(e, "msg") {
            "" => json::str_of(e, "message"),
            m => m,
        };
        out.push(Problem {
            path: json::str_of(e, "path").to_owned(),
            msg: red.text(msg),
        });
    }
    let more = errs
        .len()
        .saturating_sub(MAX_ERRORS)
        .saturating_add(usize::try_from(uint(&v, "more")).unwrap_or(0));
    if more > 0 {
        out.push(Problem::whole(format!("and {more} more")));
    }
    if out.is_empty() {
        out.push(Problem::whole("the config does not check"));
    }
    out
}

impl Built {
    /// Makes `kind` the proposal's config steps' kind, refusing another.
    fn set_kind(&mut self, kind: &'static str) -> Result<(), String> {
        if self.kind.is_empty() {
            self.kind = kind;
            return Ok(());
        }
        if self.kind != kind {
            return Err(format!(
                "the proposal's config steps are a {} already; a {kind} cannot join them, as each \
                 is applied as one change through its own endpoint: discard the proposal, or \
                 propose this after it is applied",
                self.kind
            ));
        }
        if kind == KIND_ROLLBACK {
            return Err(
                "the proposal rolls back to a version already; it holds one rollback".into(),
            );
        }
        Ok(())
    }

    /// Checks step `i` and adds what it does.
    fn step(&mut self, a: &Assistant, red: &Redactor, i: usize, st: &Step) -> Result<(), String> {
        let v = akit_config::value::parse(&st.args)
            .ok()
            .filter(|v| matches!(v, Value::Object(_)))
            .ok_or_else(|| Args::bad(&st.tool, "not a JSON object"))?;
        let args = Args(&v);
        let summary = args.str(&st.tool, "summary")?;
        let summary = summary.trim();
        if summary.is_empty() || summary.len() > MAX_SUMMARY_BYTES {
            return Err(format!(
                "summary is 1 to {MAX_SUMMARY_BYTES} bytes saying what the step does"
            ));
        }
        match st.tool.as_str() {
            CONFIG_PROPOSE => {
                self.set_kind(KIND_PATCH)?;
                check_unconverted(&args.strings(&st.tool, "unconverted")?)?;
                self.patch(red, &args)
            }
            RECONCILE_PROPOSE => {
                self.set_kind(KIND_RECONCILE)?;
                self.reconcile(a, i, &args)
            }
            ROLLBACK_PROPOSE => {
                self.set_kind(KIND_ROLLBACK)?;
                let n = args.uint(&st.tool, "version")?;
                self.rollback_to(a, n)
            }
            PURGE_PROPOSE => self.purge(a, &args),
            other => Err(format!("{other} is not a propose tool")),
        }
    }

    /// Applies `config_propose`'s members and ops to what the steps make, all or none.
    fn patch(&mut self, red: &Redactor, args: &Args<'_>) -> Result<(), String> {
        let tool = CONFIG_PROPOSE;
        let members = args.0.get("members").filter(|m| **m != Value::Null);
        let ops = args.list(tool, "ops")?;
        if members.is_none() && ops.is_empty() {
            return Err("give members, ops or both".into());
        }
        let mut next = self.after.clone();
        if let Some(m) = members {
            let Value::Object(ms) = m else {
                return Err("members is not a JSON object".into());
            };
            for (k, v) in ms {
                forbidden(&[k])?;
                if *v == Value::Null {
                    tree::del(&mut next, k);
                    continue;
                }
                let v = self.guard(red, std::slice::from_ref(k), v.clone())?;
                tree::set(&mut next, k, v);
            }
        }
        for (n, op) in ops.iter().enumerate() {
            let o = Args(op);
            let name = o.str(tool, "op")?;
            let tokens =
                tree::parse_pointer(&o.str(tool, "path")?).map_err(|e| format!("ops[{n}]: {e}"))?;
            if tokens.is_empty() {
                return Err(format!(
                    "ops[{n}]: the path names the whole config; set top-level members with members"
                ));
            }
            forbidden(&tokens).map_err(|e| format!("ops[{n}]: {e}"))?;
            let v = match name.as_str() {
                OP_ADD | OP_REPLACE => {
                    let Some(v) = op.get("value") else {
                        return Err(format!("ops[{n}]: {name} needs a value"));
                    };
                    Some(
                        self.guard(red, &tokens, v.clone())
                            .map_err(|e| format!("ops[{n}]: {e}"))?,
                    )
                }
                OP_REMOVE => None,
                other => {
                    return Err(format!(
                        "ops[{n}]: op is add, replace or remove, not {}",
                        quoted(other)
                    ));
                }
            };
            tree::patch(&mut next, &name, &tokens, v, false)
                .map_err(|e| format!("ops[{n}]: {e}"))?;
        }
        self.after = next;
        Ok(())
    }

    /// `v`, to be written at `tokens`, with each redacted value put back as the config has it
    /// at the same place; refused when it is redacted with no such secret there, or is a secret
    /// the config does not already have there: the assistant never writes a secret.
    fn guard(&self, red: &Redactor, tokens: &[String], v: Value) -> Result<Value, String> {
        let secret = tokens.iter().any(|t| secret_name(t)) || secret_path(tokens);
        let base = tree::lookup(&self.base, tokens);
        let mut at = tokens.to_vec();
        guard_at(red, &mut at, v, base, secret)
    }

    /// Adds `reconcile_propose`'s fixes, checked against the differences `GET /config/check`
    /// lists now.
    fn reconcile(&mut self, a: &Assistant, i: usize, args: &Args<'_>) -> Result<(), String> {
        let tool = RECONCILE_PROPOSE;
        let mut fixes = Vec::new();
        for f in args.list(tool, "fixes")? {
            let f = Args(f);
            fixes.push(FixRef {
                id: f.str(tool, "id")?,
                action: f.str(tool, "action")?,
            });
        }
        if fixes.is_empty() {
            return Err("name at least one fix".into());
        }
        if self.check.is_none() && self.reread.is_none() {
            match a
                .api_json("GET", "/config/check", Vec::new())
                .and_then(|t| check_diffs(&t))
            {
                Ok(d) => self.check = Some(d),
                Err(e) => self.reread = Some(e),
            }
        }
        if let Some(e) = &self.reread {
            return Err(format!("the Consistency check could not be read: {e}"));
        }
        let check = self.check.as_deref().unwrap_or_default();
        for f in &fixes {
            if self.fixes.iter().any(|x| x.id == f.id) {
                return Err(format!(
                    "the difference {} is fixed by the proposal already",
                    quoted(&f.id)
                ));
            }
            let Some(d) = check.iter().find(|d| d.id == f.id) else {
                return Err(format!(
                    "the Consistency check has no difference {} now: check again",
                    quoted(&f.id)
                ));
            };
            let Some(fx) = d.fixes.iter().find(|x| x.action == f.action) else {
                return Err(format!(
                    "the difference {} offers no {} fix",
                    quoted(&f.id),
                    quoted(&f.action)
                ));
            };
            if let Some((_, path, _)) = &fx.patch {
                if let Ok(tokens) = tree::parse_pointer(path) {
                    forbidden(&tokens).map_err(|e| format!("the fix of {}: {e}", quoted(&f.id)))?;
                }
            }
        }
        for f in fixes {
            self.fixes.push(f);
            self.fix_at.push(i);
        }
        Ok(())
    }

    /// Applies the fixes' patches as `POST /config/reconcile` does: adds and replaces in order,
    /// then the removals, the deepest and the highest index first. A fix that does not apply is
    /// its step's error.
    fn apply_fixes(&mut self) {
        struct Pending {
            op: String,
            tokens: Vec<String>,
            value: Option<Value>,
            step: usize,
        }
        let check = self.check.clone().unwrap_or_default();
        let (mut sets, mut dels) = (Vec::new(), Vec::new());
        let mut errs: Vec<(usize, String)> = Vec::new();
        for (n, f) in self.fixes.iter().enumerate() {
            let step = self.fix_at.get(n).copied().unwrap_or(0);
            let Some(d) = check.iter().find(|d| d.id == f.id) else {
                continue;
            };
            for fx in d.fixes.iter().filter(|x| x.action == f.action) {
                let Some((op, path, value)) = &fx.patch else {
                    continue;
                };
                let tokens = tree::parse_pointer(path).unwrap_or_default();
                if tokens.is_empty() {
                    errs.push((
                        step,
                        format!("the fix of {} has no path to change", quoted(&f.id)),
                    ));
                    continue;
                }
                let p = Pending {
                    op: op.clone(),
                    tokens,
                    value: value.clone(),
                    step,
                };
                if p.op == "delete" {
                    dels.push(p);
                } else {
                    sets.push(p);
                }
            }
        }
        dels.sort_by(|x, y| {
            for (a, b) in x.tokens.iter().zip(&y.tokens) {
                if a == b {
                    continue;
                }
                return match (a.parse::<i64>(), b.parse::<i64>()) {
                    (Ok(ai), Ok(bi)) => bi.cmp(&ai),
                    _ => a.cmp(b),
                };
            }
            y.tokens.len().cmp(&x.tokens.len())
        });
        let mut root = self.after.clone();
        for p in sets.into_iter().chain(dels) {
            let last = p.tokens.last().map_or("", String::as_str);
            let (op, v) = if p.op == "delete" {
                (OP_REMOVE, None)
            } else {
                let Some(v) = p.value.clone() else {
                    errs.push((
                        p.step,
                        format!("the fix for {} has no value", tree::pointer_of(&p.tokens)),
                    ));
                    continue;
                };
                // The Admin API sets an array's element in place.
                let op = if last.parse::<i64>().is_ok() {
                    OP_REPLACE
                } else {
                    OP_ADD
                };
                (op, Some(v))
            };
            // Only an add at the end of an array the config leaves out makes it, as the Admin
            // API does.
            let create = p.op == "add" && last == "-";
            let mut next = root.clone();
            match tree::patch(&mut next, op, &p.tokens, v, create) {
                Ok(()) => root = next,
                Err(e) => errs.push((p.step, e)),
            }
        }
        for (step, e) in errs {
            if let Some(slot) = self.step_errs.get_mut(step) {
                *slot = e;
            }
        }
        self.after = root;
    }

    /// Makes what the steps make the config kept as version `n`.
    fn rollback_to(&mut self, a: &Assistant, n: u64) -> Result<(), String> {
        if n == 0 {
            return Err("version is the kept version to apply again".into());
        }
        if n == self.version {
            return Err(format!("version {n} is the one running"));
        }
        let text = a.api_json("GET", &format!("/versions/{n}"), Vec::new())?;
        let v = match akit_config::value::parse(&text) {
            Ok(v @ Value::Object(_)) => v,
            _ => return Err(format!("version {n} is not a JSON object")),
        };
        let same = match (self.base.get("admin"), v.get("admin")) {
            (None, None) => true,
            (Some(x), Some(y)) => tree::equal(x, y),
            _ => false,
        };
        if !same {
            return Err(format!(
                "version {n} has another admin, which the assistant may not propose: the person \
                 rolls back to it on the Versions page"
            ));
        }
        self.rollback = n;
        self.after = v;
        Ok(())
    }

    /// Adds `cache_purge_propose`'s purge, checked against the zones the cache has now.
    fn purge(&mut self, a: &Assistant, args: &Args<'_>) -> Result<(), String> {
        let tool = PURGE_PROPOSE;
        let p = Purge {
            zone: args.str(tool, "zone")?,
            urls: args.strings(tool, "urls")?,
            tags: args.strings(tool, "tags")?,
            all: args.flag(tool, "all")?,
        };
        if p.urls.is_empty() && p.tags.is_empty() && !p.all {
            return Err("name urls, tags, or all".into());
        }
        if p.urls.len() > MAX_PURGE_ITEMS || p.tags.len() > MAX_PURGE_ITEMS {
            return Err(format!(
                "at most {MAX_PURGE_ITEMS} urls and {MAX_PURGE_ITEMS} tags"
            ));
        }
        if let Some(u) = p.urls.iter().find(|u| !absolute_url(u)) {
            return Err(format!(
                "{} is not an absolute http or https URL",
                quoted(u)
            ));
        }
        if !p.zone.is_empty() {
            if self.zones.is_none() {
                let text = a
                    .api_json("GET", "/cache", Vec::new())
                    .map_err(|e| format!("the cache's zones could not be read: {e}"))?;
                let zones = match akit_config::value::parse(&text) {
                    Ok(Value::Array(zs)) => zs
                        .iter()
                        .map(|z| json::str_of(z, "zone").to_owned())
                        .collect(),
                    _ => Vec::new(),
                };
                self.zones = Some(zones);
            }
            if !self.zones.as_ref().is_some_and(|z| z.contains(&p.zone)) {
                return Err(format!("no cache uses the zone {}", quoted(&p.zone)));
            }
        }
        self.purges.push(p);
        Ok(())
    }
}

/// Whether `u` is an absolute http or https URL with a host, as Go's `url.Parse` reads one.
fn absolute_url(u: &str) -> bool {
    let Some((scheme, rest)) = u.split_once("://") else {
        return false;
    };
    let scheme = scheme.to_ascii_lowercase();
    let host = rest.split(['/', '?', '#']).next().unwrap_or("");
    let host = host.rsplit('@').next().unwrap_or("");
    (scheme == "http" || scheme == "https") && !host.is_empty() && !u.contains(char::is_whitespace)
}

fn guard_at(
    red: &Redactor,
    tokens: &mut Vec<String>,
    v: Value,
    base: Option<&Value>,
    secret: bool,
) -> Result<Value, String> {
    match v {
        Value::Object(ms) => {
            let mut out = Vec::with_capacity(ms.len());
            for (k, x) in ms {
                let bv = match base {
                    Some(b @ Value::Object(_)) => b.get(&k),
                    _ => None,
                };
                tokens.push(k.clone());
                let s = secret || secret_path(tokens);
                let y = guard_at(red, tokens, x, bv, s);
                tokens.pop();
                out.push((k, y?));
            }
            Ok(Value::Object(out))
        }
        Value::Array(a) => {
            let mut out = Vec::with_capacity(a.len());
            for (i, x) in a.into_iter().enumerate() {
                let bv = match base {
                    Some(Value::Array(ba)) => ba.get(i),
                    _ => None,
                };
                tokens.push(i.to_string());
                let y = guard_at(red, tokens, x, bv, secret);
                tokens.pop();
                out.push(y?);
            }
            Ok(Value::Array(out))
        }
        Value::String(t) => {
            let bs = match base {
                Some(Value::String(s)) => Some(s.as_str()),
                _ => None,
            };
            if is_redacted(&t) {
                if let Some(bs) = bs.filter(|s| !s.is_empty()) {
                    if t == REDACTED || red.text(bs) == t {
                        return Ok(Value::String(bs.to_owned()));
                    }
                }
                return Err(format!(
                    "{} is {}, a redacted value the config does not have there: leave that member \
                     as it is, and change what is around it with ops",
                    tree::pointer_of(tokens),
                    quoted(&t)
                ));
            }
            let last = tokens.last().map_or("", String::as_str);
            if secret && !t.is_empty() && Some(t.as_str()) != bs && !selector_value(last, &t) {
                return Err(format!(
                    "{} holds a secret, which the assistant never writes: leave it out, or as it \
                     is, and the person sets it on the page",
                    tree::pointer_of(tokens)
                ));
            }
            Ok(Value::String(t))
        }
        other => Ok(other),
    }
}

/// Checks `config_propose`'s unconverted items.
fn check_unconverted(items: &[String]) -> Result<(), String> {
    if items.len() > MAX_UNCONVERTED {
        return Err(format!("unconverted holds at most {MAX_UNCONVERTED} items"));
    }
    if items.iter().any(|it| {
        let s = it.trim();
        s.is_empty() || s.len() > MAX_UNCONVERTED_BYTES
    }) {
        return Err(format!(
            "each of unconverted is 1 to {MAX_UNCONVERTED_BYTES} bytes saying what was not \
             carried over"
        ));
    }
    Ok(())
}

/// The unconverted items of `steps`, in order, each once.
#[must_use]
pub fn unconverted(steps: &[Step]) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for st in steps.iter().filter(|s| s.tool == CONFIG_PROPOSE) {
        let Ok(v) = akit_config::value::parse(&st.args) else {
            continue;
        };
        let Some(Value::Array(items)) = v.get("unconverted") else {
            continue;
        };
        for it in items {
            if let Value::String(s) = it {
                let s = s.trim();
                if !s.is_empty() && !out.iter().any(|o| o == s) {
                    out.push(s.to_owned());
                }
            }
        }
    }
    out
}

/// The members `names` of `v`, an object, in that order.
#[must_use]
pub fn subset(v: &Value, names: &[String]) -> Value {
    Value::Object(
        names
            .iter()
            .filter_map(|n| v.get(n).map(|x| (n.clone(), x.clone())))
            .collect(),
    )
}

/// The members `names` of `v` in `format`, redacted.
fn render(
    a: &Assistant,
    red: &Redactor,
    v: &Value,
    names: &[String],
    format: &str,
) -> Result<String, String> {
    let data = red.json(&subset(v, names), HIDDEN);
    if format == "json" {
        return akit_config::value::parse(&data)
            .map(|v| v.to_json())
            .map_err(|_| "the members are not JSON".to_owned());
    }
    a.api_json(
        "POST",
        &format!("/config/render?format={format}"),
        data.into_bytes(),
    )
}

/// The diff of the changed members in `format`, from the running config to what the proposal
/// makes of it, redacted and bounded; empty when nothing changes.
///
/// # Errors
///
/// A side could not be rendered in `format`.
pub fn diff(a: &Assistant, red: &Redactor, b: &Built, format: &str) -> Result<String, String> {
    if b.changed.is_empty() {
        return Ok(String::new());
    }
    let from = render(a, red, &b.base, &b.changed, format)?;
    let to = render(a, red, &b.after, &b.changed, format)?;
    let d = crate::linediff::unified(&format!("version {}", b.version), "proposed", &from, &to);
    Ok(cut_text(&d, MAX_DIFF_BYTES).0)
}

/// The steps' errors by their number, from 1, as a JSON object.
fn step_errors(errs: &[String]) -> String {
    let mut o = Obj::new();
    for (i, e) in errs.iter().enumerate().filter(|(_, e)| !e.is_empty()) {
        o = o.str(&i.saturating_add(1).to_string(), e);
    }
    o.end()
}

/// The propose tools.
#[must_use]
pub fn tools() -> Vec<Tool> {
    let tool = |name, description, parameters, run| Tool {
        name,
        group: "change",
        level: Level::Propose,
        description,
        parameters,
        max_bytes: 0,
        kind: Kind::Pool,
        enabled: None,
        run: Some(run),
    };
    vec![
        tool(
            CONFIG_PROPOSE,
            concat!(
                "Propose a change to akit's config: top-level members set whole (members), or operations at JSON pointers (ops: add, replace or remove; /routes/- appends). Read the config first and change only what was asked; prefer ops for a part of a large member. Values shown as [redacted] may be written back where they were, never elsewhere; secrets and admin cannot be proposed.",
                " The step joins the conversation's one pending proposal, which akit checks as a whole and shows the person as a diff; only the person applies it, in the panel, after a second confirm. You cannot apply it."
            ),
            r#"{"type":"object","properties":{"summary":{"type":"string","description":"what this step does, in one short sentence the person reads before applying"},"members":{"type":"object","description":"top-level members set to these values; null removes one"},"unconverted":{"type":"array","items":{"type":"string"},"maxItems":100,"description":"when the change moves a config from another server: each item of it not carried over, as 'line 12: sub_filter: akit does not rewrite response bodies', shown to the person with the diff"},"ops":{"type":"array","items":{"type":"object","properties":{"op":{"type":"string","enum":["add","replace","remove"]},"path":{"type":"string","description":"a JSON pointer, as /routes/0/handle or /upstreams/-"},"value":{"description":"the value of an add or a replace"}},"required":["op","path"]}}},"required":["summary"]}"#,
            run_config,
        ),
        tool(
            RECONCILE_PROPOSE,
            concat!(
                "Propose fixes of the Consistency check's differences, by each difference's id and the action of one of its fixes, as consistency_check lists them. Reconcile steps are applied together, and not with config_propose or rollback_propose steps: propose those in a proposal of their own.",
                " The step joins the conversation's one pending proposal, which akit checks as a whole and shows the person as a diff; only the person applies it, in the panel, after a second confirm. You cannot apply it."
            ),
            r#"{"type":"object","properties":{"summary":{"type":"string","description":"what this step does, in one short sentence the person reads before applying"},"fixes":{"type":"array","items":{"type":"object","properties":{"id":{"type":"string"},"action":{"type":"string"}},"required":["id","action"]},"minItems":1}},"required":["summary","fixes"]}"#,
            run_reconcile,
        ),
        tool(
            ROLLBACK_PROPOSE,
            concat!(
                "Propose applying a kept config version again, as a new version (versions_diff lists them and what changed). A rollback is the proposal's only config step.",
                " The step joins the conversation's one pending proposal, which akit checks as a whole and shows the person as a diff; only the person applies it, in the panel, after a second confirm. You cannot apply it."
            ),
            r#"{"type":"object","properties":{"summary":{"type":"string","description":"what this step does, in one short sentence the person reads before applying"},"version":{"type":"integer","description":"the kept version to apply again"}},"required":["summary","version"]}"#,
            run_rollback,
        ),
        tool(
            PURGE_PROPOSE,
            concat!(
                "Propose removing responses the cache keeps, after the proposal's config change: in one zone (every zone when left out), by absolute URL, by tag (Cache-Tag, Surrogate-Key), or all of them.",
                " The step joins the conversation's one pending proposal, which akit checks as a whole and shows the person as a diff; only the person applies it, in the panel, after a second confirm. You cannot apply it."
            ),
            r#"{"type":"object","properties":{"summary":{"type":"string","description":"what this step does, in one short sentence the person reads before applying"},"zone":{"type":"string"},"urls":{"type":"array","items":{"type":"string"}},"tags":{"type":"array","items":{"type":"string"}},"all":{"type":"boolean"}},"required":["summary"]}"#,
            run_purge,
        ),
    ]
}

fn run_config(c: &Call<'_>) -> Result<String, String> {
    propose(c, CONFIG_PROPOSE)
}

fn run_reconcile(c: &Call<'_>) -> Result<String, String> {
    propose(c, RECONCILE_PROPOSE)
}

fn run_rollback(c: &Call<'_>) -> Result<String, String> {
    propose(c, ROLLBACK_PROPOSE)
}

fn run_purge(c: &Call<'_>) -> Result<String, String> {
    propose(c, PURGE_PROPOSE)
}

/// A propose tool's call: the step is checked with the proposal's others on the running config
/// and, unless the step itself is wrong, added to the conversation's pending proposal, made when
/// there is none. A proposal whose whole does not check is kept, with its errors, for the next
/// step to mend; it cannot be applied. fit's `toolPropose`.
fn propose(c: &Call<'_>, name: &str) -> Result<String, String> {
    let a = c.a;
    let args = compact(&c.args);
    if args.len() > MAX_BYTES {
        return Err(format!(
            "the step's arguments are larger than {MAX_BYTES} bytes: change parts of large \
             members with ops"
        ));
    }
    let Some(p) = a.proposal_of(c.user, c.conv) else {
        return Err(NO_CONVERSATION.to_owned());
    };
    let now = now_ms();
    let mut p = p.unwrap_or_else(|| Proposal {
        id: new_id(),
        created: now,
        ..Proposal::default()
    });
    if p.steps.len() >= MAX_STEPS {
        return Err(format!(
            "the proposal has its {MAX_STEPS} steps: let the person apply or discard it first"
        ));
    }
    if p.bytes().saturating_add(args.len()) > MAX_BYTES {
        return Err(format!(
            "the proposal's steps would pass {MAX_BYTES} bytes: let the person apply or discard \
             it first"
        ));
    }
    let summary = json::str_of(&c.args, "summary").trim().to_owned();
    p.steps.push(Step {
        tool: name.to_owned(),
        summary,
        args,
        at: now,
    });
    let red = a.redactor(c.settings);
    let b = build(a, &red, &p.steps)?;
    if let Some(e) = b.step_errs.last().filter(|e| !e.is_empty()) {
        return Err(format!("{e} (the step was not added to the proposal)"));
    }
    let rebased = !p.base_etag.is_empty() && p.base_etag != b.etag;
    p.base_etag.clone_from(&b.etag);
    p.base_version = b.version;
    p.updated = now;
    p.error.clear();
    p.revision = p.revision.saturating_add(1);
    p.valid = b.valid;
    p.problems = b
        .errors
        .len()
        .saturating_add(b.step_errs.iter().filter(|e| !e.is_empty()).count());
    a.keep_proposal(c.user, c.conv, &p, || c.expired())?;
    c.proposed(&p, &b.changed);
    // The members in the order Go's encoding of a map writes them: by name.
    let mut o = Obj::new()
        .num("base_version", b.version)
        .raw("changed", &json::strings(&b.changed));
    if let Ok(d) = diff(a, &red, &b, "json") {
        o = o.str_omit("diff", &d);
    }
    if !b.errors.is_empty() {
        o = o.raw("errors", &problems_json(&b.errors));
    }
    o = o
        .str("kind", b.kind)
        .str(
            "note",
            "The person reviews the proposal in the panel and applies it, or not; you cannot \
             apply it. Add a step to mend an error, or say what the proposal does.",
        )
        .str("proposal", &p.id);
    if !b.purges.is_empty() {
        let purges: Vec<String> = b.purges.iter().map(|x| x.members().end()).collect();
        o = o.raw("purges", &format!("[{}]", purges.join(",")));
    }
    if rebased {
        o = o.str(
            "rebased",
            &format!(
                "the config changed while the proposal was made: its steps are checked on \
                 version {} now",
                b.version
            ),
        );
    }
    o = o.num("revision", p.revision);
    if b.rollback > 0 {
        o = o.num("rollback", b.rollback);
    }
    if b.step_errs.iter().any(|e| !e.is_empty()) {
        o = o.raw("step_errors", &step_errors(&b.step_errs));
    }
    Ok(o.num("steps", p.steps.len()).bool("valid", b.valid).end())
}

#[cfg(test)]
mod tests;
