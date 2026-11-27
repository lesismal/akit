//! Templates (I136, fit's `cluster/templates.go` and `render.go`, its I145): config the manager
//! owns on the members of the groups a template is attached to. A template's config is a fragment
//! of an akit's (or fit's) config, whose top-level members it owns whole, but for the elements of
//! the named lists (`listeners`, `routes`, `services`, `upstreams`), which it owns one by one, by
//! name. Strings may use variables: `${node.name}`, `${node.id}`, `${node.hostname}`,
//! `${node.labels.<key>}` and a group's variables, `${<name>}`; `$${` is a `${` as written. A
//! member's desired config is its own with the parts its templates own taken from them, the
//! templates applied by order, then name, a later one winning a part two own.

use std::collections::BTreeMap;

use akio_http::{Context, Method, Response};
use akit_config::value::Value;
use akit_roles::pages::{fail, json as answer, no_endpoint};

use crate::json::{self, Obj};
use crate::jv;
use crate::member::{Document, FieldError, MAX_BODY_BYTES, MAX_LABEL_KEY, MAX_NAME_BYTES};
use crate::{Manager, Refusal};

/// How many templates the manager keeps.
pub const MAX_TEMPLATES: usize = 128;
/// The largest template's config, as JSON.
pub const MAX_TEMPLATE_BYTES: usize = 64 << 10;
/// The most parts a template owns.
pub const MAX_TEMPLATE_PARTS: usize = 256;
/// The most groups a template is attached to.
pub const MAX_TEMPLATE_GROUPS: usize = 32;
/// A template's order, either way.
pub const MAX_ORDER: i64 = 1000;
/// What stands for a secret a template keeps, in what the API answers: fit's.
pub const SECRET_MASK: &str = "[secret]";

fn field(f: &str, msg: impl Into<String>) -> FieldError {
    FieldError {
        field: f.to_owned(),
        msg: msg.into(),
    }
}

/// A template as the store keeps it: fit's `Template`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Template {
    /// Its name.
    pub name: String,
    /// What it is for.
    pub description: String,
    /// The groups it is attached to.
    pub groups: Vec<String>,
    /// Its order among the templates a member has.
    pub order: i64,
    /// Its config, a JSON object.
    pub config: Value,
    /// When it was added and changed, as written.
    pub created: String,
    /// See [`created`](Template::created).
    pub updated: String,
    /// What akit does not read of fit's template.
    pub extra: Vec<(String, Value)>,
}

const KNOWN: &[&str] = &[
    "name",
    "description",
    "groups",
    "order",
    "config",
    "created",
    "updated",
];

impl Template {
    /// Reads a template as fit's document holds it.
    #[must_use]
    pub fn from_value(v: &Value) -> Template {
        let mut t = Template {
            name: json::str_of(v, "name").to_owned(),
            description: json::str_of(v, "description").to_owned(),
            groups: json::strings_of(v, "groups"),
            order: json::int_of(v, "order"),
            config: v
                .get("config")
                .cloned()
                .unwrap_or(Value::Object(Vec::new())),
            created: json::str_of(v, "created").to_owned(),
            updated: json::str_of(v, "updated").to_owned(),
            extra: Vec::new(),
        };
        if let Value::Object(ms) = v {
            for (k, x) in ms {
                if !KNOWN.contains(&k.as_str()) {
                    t.extra.push((k.clone(), x.clone()));
                }
            }
        }
        t
    }

    /// The template as the store keeps it, fit's.
    #[must_use]
    pub fn to_json(&self) -> String {
        self.json_with(&json::compact(&self.config))
    }

    fn json_with(&self, config: &str) -> String {
        let mut o = Obj::new()
            .str("name", &self.name)
            .str_omit("description", &self.description)
            .raw("groups", &json::string_list(&self.groups))
            .num("order", self.order)
            .raw("config", config);
        for (k, v) in &self.extra {
            o = o.raw(k, &json::compact(v));
        }
        o.raw("created", &json::quoted(&self.created))
            .raw("updated", &json::quoted(&self.updated))
            .end()
    }

    /// Checks it as fit's `Template.check`.
    ///
    /// # Errors
    ///
    /// The field refused.
    pub fn check(&self) -> Result<(), FieldError> {
        let name_ok = |n: &str| {
            !n.is_empty()
                && n.len() <= MAX_NAME_BYTES
                && n.as_bytes().first().is_some_and(u8::is_ascii_alphanumeric)
                && n.bytes()
                    .all(|c| c.is_ascii_alphanumeric() || matches!(c, b'_' | b'.' | b'-'))
        };
        if !name_ok(&self.name) {
            return Err(field(
                "name",
                format!(
                    "{:?} is not a name: 1 to {MAX_NAME_BYTES} letters, digits, '_', '.' and '-', \
                     starting with a letter or digit",
                    self.name
                ),
            ));
        }
        if self.description.len() > crate::groups::MAX_DESCRIPTION_BYTES
            || self.description.chars().any(|c| c < ' ' || c == '\u{7f}')
        {
            return Err(field(
                "description",
                format!(
                    "the description is at most {} bytes of printable characters",
                    crate::groups::MAX_DESCRIPTION_BYTES
                ),
            ));
        }
        if self.groups.len() > MAX_TEMPLATE_GROUPS {
            return Err(field(
                "groups",
                format!(
                    "attached to {} groups, more than {MAX_TEMPLATE_GROUPS}",
                    self.groups.len()
                ),
            ));
        }
        if !(-MAX_ORDER..=MAX_ORDER).contains(&self.order) {
            return Err(field(
                "order",
                format!(
                    "the order is from {} to {MAX_ORDER}",
                    MAX_ORDER.saturating_neg()
                ),
            ));
        }
        for g in &self.groups {
            if !name_ok(g) {
                return Err(field("groups", format!("{g:?} is not a group's name")));
            }
        }
        read_config(&self.config, &mut |_| {}).map(|_| ())
    }
}

/// A part a template owns and its value.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Part {
    /// The top-level member.
    pub member: String,
    /// The element's name, for a named list.
    pub name: String,
    /// Its value.
    pub value: Value,
}

impl Part {
    /// As the pages write it.
    #[must_use]
    pub fn display(&self) -> String {
        jv::display(&self.member, &self.name)
    }
}

/// The variables a template may name: a member's own.
const NODE_VARS: &[&str] = &["node.name", "node.id", "node.hostname"];

fn check_var(name: &str) -> Result<(), String> {
    if NODE_VARS.contains(&name) {
        return Ok(());
    }
    if let Some(k) = name.strip_prefix("node.labels.") {
        let ok = !k.is_empty()
            && k.len() <= MAX_LABEL_KEY
            && k.as_bytes().first().is_some_and(u8::is_ascii_alphanumeric)
            && k.as_bytes().last().is_some_and(u8::is_ascii_alphanumeric)
            && k.bytes()
                .all(|c| c.is_ascii_alphanumeric() || matches!(c, b'_' | b'.' | b'/' | b'-'));
        return if ok {
            Ok(())
        } else {
            Err(format!("${{{name}}}: {k:?} is not a label key"))
        };
    }
    if name.starts_with("node.") {
        return Err(format!(
            "${{{name}}}: a member's variables are {} and node.labels.<key>",
            NODE_VARS.join(", ")
        ));
    }
    let ok = !name.is_empty()
        && name.len() <= crate::groups::MAX_VARIABLE_NAME
        && !name.as_bytes().first().is_some_and(u8::is_ascii_digit)
        && name.bytes().all(|c| c.is_ascii_alphanumeric() || c == b'_');
    if ok {
        Ok(())
    } else {
        Err(format!(
            "${{{name}}}: a group's variable is named with letters, digits and '_'"
        ))
    }
}

/// Replaces the variables of `s` with what `lookup` gives them (`None` checks them alone),
/// telling `seen` each: fit's `expand`.
///
/// # Errors
///
/// A variable not written right, not known, or one `lookup` has no value of.
#[allow(clippy::type_complexity)]
pub fn expand(
    s: &str,
    lookup: Option<&dyn Fn(&str) -> Result<String, String>>,
    seen: &mut dyn FnMut(&str),
) -> Result<String, String> {
    if !s.contains("${") {
        return Ok(s.to_owned());
    }
    let mut out = String::new();
    let mut rest = s;
    while let Some(j) = rest.find("${") {
        let (before, after) = rest.split_at(j);
        if before.ends_with('$') {
            // $${ is a ${ as written.
            out.push_str(before.get(..before.len().saturating_sub(1)).unwrap_or(""));
            out.push_str("${");
            rest = after.get(2..).unwrap_or("");
            continue;
        }
        out.push_str(before);
        let body = after.get(2..).unwrap_or("");
        let Some(end) = body.find('}') else {
            return Err(format!(
                "{s:?} has a ${{ that does not end with }}; write $${{ for a ${{ as it is"
            ));
        };
        let name = body.get(..end).unwrap_or("");
        check_var(name)?;
        seen(name);
        if let Some(f) = lookup {
            out.push_str(&f(name)?);
        }
        rest = body.get(end.saturating_add(1)..).unwrap_or("");
    }
    out.push_str(rest);
    Ok(out)
}

/// Whether a member named `name` holds a secret: fit's `secretName`, akit-admin's rule.
fn secret_name(name: &str) -> bool {
    let n = name.to_ascii_lowercase();
    if n.ends_with("_file")
        || n.ends_with("_env")
        || n.ends_with("_ttl")
        || n.starts_with("max_")
        || n.starts_with("min_")
    {
        return false;
    }
    if n.contains("password") || n.contains("secret") || n.contains("passphrase") {
        return true;
    }
    if matches!(
        n.as_str(),
        "passwd"
            | "pass"
            | "token"
            | "tokens"
            | "key"
            | "keys"
            | "apikey"
            | "hash"
            | "credential"
            | "credentials"
            | "authorization"
            | "cookie"
            | "signature"
            | "psk"
            | "private_key"
    ) {
        return true;
    }
    [
        "_token",
        "_tokens",
        "_key",
        "_keys",
        "_hash",
        "_sha256",
        "_signature",
    ]
    .iter()
    .any(|s| n.ends_with(s))
}

fn rate_key(s: &str) -> bool {
    s == "ip"
        || s == "route"
        || s.strip_prefix("header:").is_some_and(|h| {
            (1..=64).contains(&h.len())
                && h.bytes()
                    .all(|c| c.is_ascii_alphanumeric() || b"!#$%&'*+.^_|~-".contains(&c))
        })
}

/// Whether string `s` under the object members `path` is a secret: fit's `secretAt`.
#[must_use]
pub fn secret_at(path: &[String], s: &str) -> bool {
    if s.is_empty() || s == SECRET_MASK {
        return s == SECRET_MASK;
    }
    let last = path.len().saturating_sub(1);
    for (i, k) in path.iter().enumerate() {
        if secret_name(k) && !(k == "key" && i == last && rate_key(s)) {
            return true;
        }
    }
    path.len() >= 2
        && path.get(last).is_some_and(|k| k == "url")
        && path
            .get(last.saturating_sub(1))
            .is_some_and(|k| k == "push")
}

/// `v` with its secrets replaced by `mask`.
#[must_use]
pub fn masked(v: &Value, mask: &str) -> Value {
    let mut c = v.clone();
    let _ = jv::walk_strings(&mut c, &mut Vec::new(), &mut |path, s| {
        Ok(if secret_at(path, s) {
            mask.to_owned()
        } else {
            s.to_owned()
        })
    });
    c
}

/// Puts back, in `nv`, the secrets `old` keeps where `nv` has [`SECRET_MASK`]: by object member,
/// by name in a list of named objects, or by index. fit's `restoreSecrets`.
///
/// # Errors
///
/// A mask with no secret kept in its place.
pub fn restore_secrets(nv: &mut Value, old: Option<&Value>, at: &str) -> Result<(), String> {
    match nv {
        Value::String(s) if s == SECRET_MASK => match old {
            Some(Value::String(o)) if o != SECRET_MASK => {
                s.clone_from(o);
                Ok(())
            }
            _ => Err(format!(
                "{at} is {SECRET_MASK}, but no secret is kept there: write the secret itself"
            )),
        },
        Value::Object(ms) => {
            for (k, x) in ms {
                restore_secrets(x, old.and_then(|o| o.get(k)), &format!("{at}.{k}"))?;
            }
            Ok(())
        }
        Value::Array(items) => {
            for (i, it) in items.iter_mut().enumerate() {
                let name = jv::name_of(it).to_owned();
                let o = match old {
                    Some(Value::Array(olds)) if !name.is_empty() => {
                        olds.iter().find(|x| jv::name_of(x) == name)
                    }
                    Some(Value::Array(olds)) => olds.get(i),
                    _ => None,
                };
                restore_secrets(it, o, &format!("{at}[{i}]"))?;
            }
            Ok(())
        }
        _ => Ok(()),
    }
}

/// Reads and checks a template's config: an object of top-level members but `admin` and
/// `schema_version`, the named lists' elements objects with names, each once, the variables as
/// they may be written; `seen` is told each variable. Its parts, in their order. fit's
/// `readTemplateConfig`.
///
/// # Errors
///
/// The config refused.
pub fn read_config(cfg: &Value, seen: &mut dyn FnMut(&str)) -> Result<Vec<Part>, FieldError> {
    if json::compact(cfg).len() > MAX_TEMPLATE_BYTES {
        return Err(field(
            "config",
            format!("a template's config is {MAX_TEMPLATE_BYTES} bytes at most"),
        ));
    }
    let Value::Object(ms) = cfg else {
        return Err(field(
            "config",
            "a template's config is a JSON object of top-level members of the config",
        ));
    };
    let mut parts: Vec<Part> = Vec::new();
    for (k, v) in ms {
        if !jv::is_list(k) {
            if let Some(e) = jv::check_part(k, "") {
                return Err(field("config", e));
            }
            parts.push(Part {
                member: k.clone(),
                name: String::new(),
                value: v.clone(),
            });
            continue;
        }
        let Value::Array(items) = v else {
            return Err(field(
                "config",
                format!("{k} is a list, whose elements the template owns by name"),
            ));
        };
        for (j, it) in items.iter().enumerate() {
            let name = jv::name_of(it);
            if !matches!(it, Value::Object(_)) {
                return Err(field("config", format!("{k}[{j}] is not an object")));
            }
            if name.is_empty() {
                return Err(field(
                    "config",
                    format!(
                        "{k}[{j}] has no name: a template owns the elements of {k} by name, so \
                         each is named"
                    ),
                ));
            }
            if name.contains("${") {
                return Err(field(
                    "config",
                    format!(
                        "{k}[{j}]: its name, {name:?}, is how the template owns it on every \
                         member: it takes no variables"
                    ),
                ));
            }
            if let Some(e) = jv::check_part(k, name) {
                return Err(field("config", format!("{k}[{j}]: {e}")));
            }
            if parts.iter().any(|p| &p.member == k && p.name == name) {
                return Err(field(
                    "config",
                    format!("{k} has two elements named {name:?}"),
                ));
            }
            parts.push(Part {
                member: k.clone(),
                name: name.to_owned(),
                value: it.clone(),
            });
        }
    }
    if parts.len() > MAX_TEMPLATE_PARTS {
        return Err(field(
            "config",
            format!(
                "the template owns {} parts, more than {MAX_TEMPLATE_PARTS}",
                parts.len()
            ),
        ));
    }
    let mut c = cfg.clone();
    jv::walk_strings(&mut c, &mut Vec::new(), &mut |_, s| expand(s, None, seen))
        .map_err(|e| field("config", e))?;
    Ok(parts)
}

/// The warnings of a template's config about paths to files, which each member reads from its
/// own disk: fit's `fileWarnings`.
fn file_warnings(parts: &[Part]) -> Vec<String> {
    fn walk(v: &Value, at: &str, out: &mut Vec<String>) {
        match v {
            Value::Object(ms) => {
                for (k, x) in ms {
                    let sub = format!("{at}.{k}");
                    let file = k.ends_with("_file") || k.ends_with("_files") || k.ends_with("_dir");
                    if file {
                        out.push(format!(
                            "{sub} names a file, which each member reads from its own disk: it \
                             must be there on every member"
                        ));
                        continue;
                    }
                    walk(x, &sub, out);
                }
            }
            Value::Array(items) => {
                for (j, x) in items.iter().enumerate() {
                    walk(x, &format!("{at}[{j}]"), out);
                }
            }
            _ => {}
        }
    }
    let mut out = Vec::new();
    for p in parts {
        walk(&p.value, &p.display(), &mut out);
    }
    out
}

/// A template as the API takes it: fit's `TemplateInput`.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct TemplateInput {
    /// `name`.
    pub name: String,
    /// `description`.
    pub description: String,
    /// `groups`.
    pub groups: Vec<String>,
    /// `order`.
    pub order: i64,
    /// `config`, or `None`.
    pub config: Option<Value>,
    /// `from`: the member and the parts of its config the template is made of.
    pub from: Option<(String, Vec<String>)>,
}

impl TemplateInput {
    /// Reads one from a request's JSON body.
    ///
    /// # Errors
    ///
    /// Why it is not one.
    pub fn from_value(v: &Value) -> Result<TemplateInput, String> {
        let Value::Object(ms) = v else {
            return Err("the body is not an object".to_owned());
        };
        let mut i = TemplateInput::default();
        for (k, x) in ms {
            match (k.as_str(), x) {
                (_, Value::Null) => {}
                ("name", Value::String(s)) => i.name.clone_from(s),
                ("description", Value::String(s)) => i.description.clone_from(s),
                ("groups", Value::Array(_)) => i.groups = json::strings_of(v, "groups"),
                ("order", Value::Number(n)) => {
                    i.order = n
                        .parse()
                        .map_err(|_| "order is not an integer".to_owned())?;
                }
                ("config", c) => i.config = Some(c.clone()),
                ("from", f @ Value::Object(_)) => {
                    i.from = Some((
                        json::str_of(f, "node").to_owned(),
                        json::strings_of(f, "paths"),
                    ));
                }
                ("name" | "description" | "groups" | "order" | "from", _) => {
                    return Err(format!("{k} is not of the type expected"));
                }
                _ => {}
            }
        }
        Ok(i)
    }

    /// The template this input makes, checked, stamped `now`, keeping `old`'s creation and the
    /// secrets it keeps where the input sends [`SECRET_MASK`]: fit's `TemplateInput.template`.
    ///
    /// # Errors
    ///
    /// The field refused.
    pub fn template(&self, old: Option<&Template>, now: &str) -> Result<Template, FieldError> {
        let mut groups: Vec<String> = Vec::new();
        for g in &self.groups {
            let g = g.trim();
            if !g.is_empty() && !groups.iter().any(|x| x == g) {
                groups.push(g.to_owned());
            }
        }
        let mut config = self
            .config
            .clone()
            .unwrap_or_else(|| Value::Object(Vec::new()));
        restore_secrets(&mut config, old.map(|o| &o.config), "config")
            .map_err(|e| field("config", e))?;
        let t = Template {
            name: self.name.trim().to_owned(),
            description: self.description.trim().to_owned(),
            groups,
            order: self.order,
            config,
            created: old.map_or_else(|| now.to_owned(), |o| o.created.clone()),
            updated: now.to_owned(),
            extra: old.map(|o| o.extra.clone()).unwrap_or_default(),
        };
        t.check()?;
        Ok(t)
    }
}

/// The members of the groups `t` is attached to, sorted, and the groups it names that there are
/// none of.
#[must_use]
pub fn nodes_of(d: &Document, t: &Template) -> (Vec<String>, Vec<String>) {
    let mut nodes: Vec<String> = Vec::new();
    let mut missing = Vec::new();
    for g in &t.groups {
        match d.groups.iter().find(|x| &x.name == g) {
            None => missing.push(g.clone()),
            Some(gr) => {
                for n in crate::groups::nodes_of(d, gr) {
                    if !nodes.contains(&n) {
                        nodes.push(n);
                    }
                }
            }
        }
    }
    nodes.sort();
    (nodes, missing)
}

/// What the API shows of `t`, a template of `d`: its secrets masked, its parts, variables,
/// warnings, members now and groups missing. fit's `TemplateView`.
#[must_use]
pub fn view(d: &Document, t: &Template) -> String {
    let mut vars: Vec<String> = Vec::new();
    let (parts, mut warnings) = match read_config(&t.config, &mut |n| {
        if !vars.iter().any(|x| x == n) {
            vars.push(n.to_owned());
        }
    }) {
        Ok(p) => {
            let w = file_warnings(&p);
            (p, w)
        }
        Err(e) => (Vec::new(), vec![e.to_string()]),
    };
    warnings.dedup();
    vars.sort();
    let (nodes, missing) = nodes_of(d, t);
    let parts_json = parts
        .iter()
        .map(|p| {
            Obj::new()
                .str("path", &p.display())
                .str("member", &p.member)
                .str_omit("name", &p.name)
                .end()
        })
        .collect::<Vec<_>>()
        .join(",");
    let base = t.json_with(&json::compact(&masked(&t.config, SECRET_MASK)));
    let tail = Obj::new()
        .raw("parts", &format!("[{parts_json}]"))
        .raw("variables", &json::string_list(&vars))
        .raw("warnings", &json::string_list(&warnings))
        .raw("nodes", &json::string_list(&nodes))
        .raw("missing_groups", &json::string_list(&missing))
        .end();
    format!(
        "{},{}",
        base.strip_suffix('}').unwrap_or(&base),
        tail.strip_prefix('{').unwrap_or(&tail)
    )
}

/// Has the templates attached to group `old` attached to `new`, or to none of it when `new` is
/// empty: fit's `renameGroupInTemplates`.
pub fn rename_group(d: &mut Document, old: &str, new: &str) {
    for t in &mut d.templates {
        if let Some(j) = t.groups.iter().position(|g| g == old) {
            if new.is_empty() || t.groups.iter().any(|g| g == new) {
                t.groups.remove(j);
            } else if let Some(slot) = t.groups.get_mut(j) {
                new.clone_into(slot);
            }
        }
    }
}

fn changes(a: &Template, b: &Template) -> String {
    let mut out = Vec::new();
    for (c, n) in [
        (a.name != b.name, "name"),
        (a.description != b.description, "description"),
        (a.groups != b.groups, "groups"),
        (a.order != b.order, "order"),
    ] {
        if c {
            out.push(n.to_owned());
        }
    }
    let hashes = |t: &Template| -> BTreeMap<String, String> {
        read_config(&t.config, &mut |_| {})
            .unwrap_or_default()
            .iter()
            .map(|p| (p.display(), jv::hash(&p.value)))
            .collect()
    };
    let (ha, hb) = (hashes(a), hashes(b));
    let mut parts: Vec<String> = hb
        .iter()
        .filter(|(k, v)| ha.get(*k) != Some(v))
        .map(|(k, _)| k.clone())
        .collect();
    parts.extend(ha.keys().filter(|k| !hb.contains_key(*k)).cloned());
    if !parts.is_empty() {
        out.push(format!(
            "config ({})",
            crate::api::clip(&parts.join(", "), 512)
        ));
    }
    if out.is_empty() {
        "nothing".to_owned()
    } else {
        out.join(", ")
    }
}

fn entry(action: &str, name: &str, detail: &str, nodes: &[String]) -> Vec<(String, Value)> {
    let mut e = vec![
        ("action".to_owned(), Value::String(action.to_owned())),
        ("template".to_owned(), Value::String(name.to_owned())),
    ];
    if !detail.is_empty() {
        e.push(("detail".to_owned(), Value::String(detail.to_owned())));
    }
    if !nodes.is_empty() {
        e.push((
            "nodes".to_owned(),
            Value::Array(nodes.iter().map(|n| Value::String(n.clone())).collect()),
        ));
    }
    e
}

fn no_template(name: &str) -> String {
    format!("no template is named {}", json::quoted(name))
}

fn decode(ctx: &mut Context) -> Option<TemplateInput> {
    if ctx.body().len() > MAX_BODY_BYTES {
        ctx.respond(fail(
            413,
            &format!("a template is {MAX_BODY_BYTES} bytes at most"),
        ));
        return None;
    }
    let v = match akit_roles::pages::decode(ctx) {
        Ok(b) => b.0,
        Err(r) => {
            ctx.respond(r);
            return None;
        }
    };
    match TemplateInput::from_value(&v) {
        Ok(i) => Some(i),
        Err(e) => {
            ctx.respond(fail(
                400,
                &format!("the body is not the JSON expected: {e}"),
            ));
            None
        }
    }
}

fn not_allowed(allow: &str) -> Response {
    fail(405, "the method is not allowed").header("Allow", allow)
}

/// Adds template `input` as `user`: fit's `AddTemplate`. Writes the store: on the pool.
fn add(me: &Manager, user: &str, input: &TemplateInput) -> Response {
    {
        let r = me.change(user, |d: &mut Document| {
            let t = input
                .template(None, &crate::now_text())
                .map_err(Refusal::Field)?;
            if d.templates.iter().any(|x| x.name == t.name) {
                return Err(Refusal::Conflict(format!(
                    "a template is already named {}",
                    json::quoted(&t.name)
                )));
            }
            if d.templates.len() >= MAX_TEMPLATES {
                return Err(Refusal::Conflict(format!(
                    "the manager keeps {MAX_TEMPLATES} templates at most"
                )));
            }
            let (nodes, _) = nodes_of(d, &t);
            let parts = read_config(&t.config, &mut |_| {}).unwrap_or_default();
            let mut detail = if parts.is_empty() {
                "owns nothing".to_owned()
            } else {
                format!(
                    "owns {}",
                    crate::api::clip(
                        &parts
                            .iter()
                            .map(Part::display)
                            .collect::<Vec<_>>()
                            .join(", "),
                        512
                    )
                )
            };
            if !t.groups.is_empty() {
                detail.push_str(&format!("; attached to {}", t.groups.join(", ")));
            }
            let e = entry("template.add", &t.name, &detail, &nodes);
            let out = (view(d, &t), t.name.clone());
            d.templates.push(t);
            Ok((e, out))
        });
        match r {
            Ok((v, name)) => {
                me.log(format!("cluster: {user:?} added the template {name:?}"));
                answer(201, v).header("ETag", me.etag())
            }
            Err(e) => e.response(),
        }
    }
}

/// The templates' endpoints, when `path` is one:
///
/// ```text
/// GET    /api/templates          the templates, their secrets masked, with the members' ETag
/// POST   /api/templates          a template added; with from: {node, paths}, made from the parts
///                                of a member's config, as it has them now
/// GET    /api/templates/<name>   a template
/// PUT    /api/templates/<name>   a template changed, or renamed; a secret sent as [secret] keeps
///                                the one kept
/// DELETE /api/templates/<name>   a template removed; the members keep what it gave them
/// ```
pub(crate) fn serve(m: &Manager, ctx: &mut Context, user: &str, path: &str) -> bool {
    let method = ctx.method().clone();
    let get = matches!(method, Method::Get | Method::Head);
    match path {
        "/api/templates" if get => {
            let d = m.doc();
            let mut ts: Vec<&Template> = d.templates.iter().collect();
            ts.sort_by(|a, b| a.name.cmp(&b.name));
            let list = ts.iter().map(|t| view(&d, t)).collect::<Vec<_>>().join(",");
            let tag = m.etag();
            ctx.respond(
                answer(
                    200,
                    Obj::new()
                        .str("version", tag.trim_matches('"'))
                        .num("max_templates", MAX_TEMPLATES)
                        .str_omit("read_only", &m.read_only())
                        .raw("templates", &format!("[{list}]"))
                        .end(),
                )
                .header("ETag", tag),
            );
        }
        "/api/templates" if method == Method::Post => {
            let Some(mut input) = decode(ctx) else {
                return true;
            };
            if !crate::api::if_match(m, ctx) {
                return true;
            }
            match input.from.take() {
                None => {
                    let (me, user) = (m.clone(), user.to_owned());
                    crate::api::on_pool(m, ctx, move || add(&me, &user, &input));
                }
                Some((node, paths)) => {
                    // Made from a member's config: read it now, then added on the pool.
                    let Some(d) = ctx.detach() else { return true };
                    let (me, user) = (m.clone(), user.to_owned());
                    crate::sync::fetch_config(
                        m,
                        &node,
                        Box::new(move |r| match r {
                            Err(p) => d.respond(crate::sync::read_failure(&p)),
                            Ok((cfg, _)) => match crate::sync::fragment_of(&cfg, &paths) {
                                Err(e) => d.respond(Refusal::Field(e).response()),
                                Ok(frag) => {
                                    input.config = Some(frag);
                                    let cell = std::sync::Arc::new(std::sync::Mutex::new(Some(d)));
                                    let (cell2, me2) = (std::sync::Arc::clone(&cell), me.clone());
                                    let job = Box::new(move || {
                                        let d = cell2.lock().ok().and_then(|mut g| g.take());
                                        if let Some(d) = d {
                                            d.respond(add(&me2, &user, &input));
                                        }
                                    });
                                    if !me.on_pool(job) {
                                        let d = cell.lock().ok().and_then(|mut g| g.take());
                                        if let Some(d) = d {
                                            d.respond(fail(503, "the manager is busy; try again"));
                                        }
                                    }
                                }
                            },
                        }),
                    );
                }
            }
        }
        "/api/templates" => ctx.respond(not_allowed("GET, POST")),
        p if p.starts_with("/api/templates/") => {
            let rest = p.trim_start_matches("/api/templates/");
            let (raw, sub) = rest.split_once('/').unwrap_or((rest, ""));
            if !sub.is_empty() {
                ctx.respond(no_endpoint());
                return true;
            }
            let name = crate::api::percent_decode(raw);
            match method {
                Method::Get | Method::Head => {
                    let d = m.doc();
                    match d.templates.iter().find(|t| t.name == name) {
                        None => ctx.respond(fail(404, &no_template(&name))),
                        Some(t) => ctx.respond(answer(200, view(&d, t)).header("ETag", m.etag())),
                    }
                }
                Method::Put => {
                    let Some(mut input) = decode(ctx) else {
                        return true;
                    };
                    if !crate::api::if_match(m, ctx) {
                        return true;
                    }
                    if input.from.is_some() {
                        ctx.respond(fail(
                            400,
                            "a template is made from a member's config when it is added",
                        ));
                        return true;
                    }
                    if input.name.trim().is_empty() {
                        input.name.clone_from(&name);
                    }
                    let (me, user) = (m.clone(), user.to_owned());
                    crate::api::on_pool(m, ctx, move || {
                        let r = me.change(&user, |d: &mut Document| {
                            let i = d
                                .templates
                                .iter()
                                .position(|t| t.name == name)
                                .ok_or_else(|| Refusal::NotFound(no_template(&name)))?;
                            let old = d
                                .templates
                                .get(i)
                                .cloned()
                                .ok_or_else(|| Refusal::NotFound(no_template(&name)))?;
                            let t = input
                                .template(Some(&old), &crate::now_text())
                                .map_err(Refusal::Field)?;
                            if t.name != name && d.templates.iter().any(|x| x.name == t.name) {
                                return Err(Refusal::Conflict(format!(
                                    "a template is already named {}",
                                    json::quoted(&t.name)
                                )));
                            }
                            let detail = changes(&old, &t);
                            let (nodes, _) = nodes_of(d, &t);
                            let v = view(d, &t);
                            if let Some(slot) = d.templates.get_mut(i) {
                                *slot = t;
                            }
                            Ok((entry("template.update", &name, &detail, &nodes), v))
                        });
                        match r {
                            Ok(v) => {
                                me.log(format!("cluster: {user:?} changed the template {name:?}"));
                                answer(200, v).header("ETag", me.etag())
                            }
                            Err(e) => e.response(),
                        }
                    });
                }
                Method::Delete => {
                    if !crate::api::if_match(m, ctx) {
                        return true;
                    }
                    let (me, user) = (m.clone(), user.to_owned());
                    crate::api::on_pool(m, ctx, move || {
                        let r = me.change(&user, |d: &mut Document| {
                            let i = d
                                .templates
                                .iter()
                                .position(|t| t.name == name)
                                .ok_or_else(|| Refusal::NotFound(no_template(&name)))?;
                            let t = d.templates.remove(i);
                            let (nodes, _) = nodes_of(d, &t);
                            Ok((entry("template.remove", &name, "", &nodes), ()))
                        });
                        match r {
                            Ok(()) => {
                                me.log(format!("cluster: {user:?} removed the template {name:?}"));
                                answer(200, Obj::new().str("removed", &name).end())
                                    .header("ETag", me.etag())
                            }
                            Err(e) => e.response(),
                        }
                    });
                }
                _ => ctx.respond(not_allowed("GET, PUT, DELETE")),
            }
        }
        _ => return false,
    }
    true
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]
    use super::*;

    fn v(s: &str) -> Value {
        json::parse(s.as_bytes()).unwrap()
    }

    #[test]
    fn variables() {
        // fit's TestExpand.
        let look = |n: &str| -> Result<String, String> {
            match n {
                "node.name" => Ok("edge-1".into()),
                "domain" => Ok("eu.example".into()),
                _ => Err(format!("no {n}")),
            }
        };
        let mut seen = Vec::new();
        let out = expand(
            "${node.name}.${domain} $${kept} plain",
            Some(&look),
            &mut |n| seen.push(n.to_owned()),
        )
        .unwrap();
        assert_eq!(out, "edge-1.eu.example ${kept} plain");
        assert_eq!(seen, ["node.name", "domain"]);
        assert!(expand("${node.other}", None, &mut |_| {}).is_err());
        assert!(expand("${node.labels.-x}", None, &mut |_| {}).is_err());
        assert!(expand("${1x}", None, &mut |_| {}).is_err());
        assert!(expand("${open", None, &mut |_| {}).is_err());
        assert!(expand("${missing}", Some(&look), &mut |_| {}).is_err());
    }

    #[test]
    fn configs_read() {
        let parts = read_config(
            &v(r#"{"log": {"level": "info"}, "routes": [{"name": "api", "match": {"host": ["${node.name}.x"]}}]}"#),
            &mut |_| {},
        )
        .unwrap();
        assert_eq!(
            parts.iter().map(Part::display).collect::<Vec<_>>(),
            ["log", "routes[api]"]
        );
        for (bad, want) in [
            (r#"[1]"#, "JSON object"),
            (r#"{"admin": {}}"#, "member's own"),
            (r#"{"routes": {}}"#, "is a list"),
            (r#"{"routes": [{"x": 1}]}"#, "has no name"),
            (r#"{"routes": [{"name": "${a}"}]}"#, "takes no variables"),
            (
                r#"{"routes": [{"name": "a"}, {"name": "a"}]}"#,
                "two elements",
            ),
            (r#"{"log": {"x": "${node.bad}"}}"#, "a member's variables"),
        ] {
            let e = read_config(&v(bad), &mut |_| {}).unwrap_err();
            assert!(e.msg.contains(want), "{bad}: {e}");
        }
    }

    #[test]
    fn secrets_masked_and_kept() {
        // fit's TestSecrets.
        let cfg = v(
            r#"{"routes": [{"name": "a", "middlewares": [{"module": "basic_auth", "params": {"password": "s3cret", "key": "ip"}}]}],
            "telemetry": {"headers": {"api_key": "k"}}, "push": {"url": "rtmp://x/key"}}"#,
        );
        let m = masked(&cfg, SECRET_MASK);
        let text = json::compact(&m);
        assert!(
            !text.contains("s3cret") && text.contains(r#""key":"ip""#),
            "{text}"
        );
        assert!(!text.contains(r#""api_key":"k""#), "{text}");
        let mut back = m.clone();
        restore_secrets(&mut back, Some(&cfg), "config").unwrap();
        assert_eq!(back, cfg);
        let mut lost = m;
        assert!(restore_secrets(&mut lost, None, "config").is_err());
    }

    #[test]
    fn templates_checked() {
        let now = "2026-11-27T00:00:00Z";
        let i = TemplateInput {
            name: "edge".into(),
            groups: vec!["eu".into(), "eu".into(), " ".into()],
            config: Some(v(r#"{"log": {"level": "info"}}"#)),
            ..TemplateInput::default()
        };
        let t = i.template(None, now).unwrap();
        assert_eq!(t.groups, ["eu"]);
        let mut bad = i.clone();
        bad.order = 1001;
        assert_eq!(bad.template(None, now).unwrap_err().field, "order");
        let again = Template::from_value(&v(&t.to_json()));
        assert_eq!(again, t);
    }
}
