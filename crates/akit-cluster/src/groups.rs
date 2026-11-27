//! Resource groups (I135, fit's `cluster/groups.go`, its I144): a set of members by a label
//! selector, by name, or both; a member in any number of groups; a group's variables for the
//! templates of I136. Membership is computed from the labels as they are, never stored.

use std::collections::BTreeMap;

use akio_http::{Context, Method, Response};
use akit_config::value::Value;
use akit_roles::pages::{fail, json as answer, no_endpoint};

use crate::json::{self, Obj};
use crate::member::{
    Document, FieldError, MAX_BODY_BYTES, MAX_LABEL_KEY, MAX_LABEL_VALUE, MAX_LABELS,
    MAX_NAME_BYTES,
};
use crate::{Manager, Refusal};

/// How many groups the manager keeps.
pub const MAX_GROUPS: usize = 256;
/// The longest selector.
pub const MAX_SELECTOR_BYTES: usize = 1024;
/// The longest description.
pub const MAX_DESCRIPTION_BYTES: usize = 256;
/// How many variables a group has, the longest name and value.
pub const MAX_VARIABLES: usize = 64;
/// See [`MAX_VARIABLES`].
pub const MAX_VARIABLE_NAME: usize = 64;
/// See [`MAX_VARIABLES`].
pub const MAX_VARIABLE_VALUE: usize = 4096;

/// A group as the store keeps it, fit's `Group`; what akit does not read of it kept in `extra`.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Group {
    /// Its name.
    pub name: String,
    /// What it is for.
    pub description: String,
    /// Its label selector, as [`Selector`] writes it.
    pub selector: String,
    /// Members by name, sorted.
    pub members: Vec<String>,
    /// Its variables.
    pub variables: BTreeMap<String, String>,
    /// When it was added and changed, as written.
    pub created: String,
    /// See [`created`](Group::created).
    pub updated: String,
    /// Members of fit's group akit keeps as they were read.
    pub extra: Vec<(String, Value)>,
}

const KNOWN: &[&str] = &[
    "name",
    "description",
    "selector",
    "members",
    "variables",
    "created",
    "updated",
];

fn field(f: &str, msg: impl Into<String>) -> FieldError {
    FieldError {
        field: f.to_owned(),
        msg: msg.into(),
    }
}

impl Group {
    /// Reads a group as fit's document holds it.
    #[must_use]
    pub fn from_value(v: &Value) -> Group {
        let mut g = Group {
            name: json::str_of(v, "name").to_owned(),
            description: json::str_of(v, "description").to_owned(),
            selector: json::str_of(v, "selector").to_owned(),
            members: json::strings_of(v, "members"),
            created: json::str_of(v, "created").to_owned(),
            updated: json::str_of(v, "updated").to_owned(),
            ..Group::default()
        };
        if let Some(Value::Object(vs)) = v.get("variables") {
            for (k, x) in vs {
                if let Value::String(s) = x {
                    g.variables.insert(k.clone(), s.clone());
                }
            }
        }
        if let Value::Object(ms) = v {
            for (k, x) in ms {
                if !KNOWN.contains(&k.as_str()) {
                    g.extra.push((k.clone(), x.clone()));
                }
            }
        }
        g
    }

    fn variables_json(&self) -> String {
        let mut o = Obj::new();
        for (k, v) in &self.variables {
            o = o.str(k, v);
        }
        o.end()
    }

    /// The group as the store keeps it, its empty members left out as Go's `omitempty`.
    #[must_use]
    pub fn to_json(&self) -> String {
        let mut o = Obj::new()
            .str("name", &self.name)
            .str_omit("description", &self.description)
            .str_omit("selector", &self.selector);
        if !self.members.is_empty() {
            o = o.raw("members", &json::string_list(&self.members));
        }
        if !self.variables.is_empty() {
            o = o.raw("variables", &self.variables_json());
        }
        for (k, v) in &self.extra {
            o = o.raw(k, &json::compact(v));
        }
        o.raw("created", &json::quoted(&self.created))
            .raw("updated", &json::quoted(&self.updated))
            .end()
    }

    /// Checks the group as fit's `Group.check`; `max_members` bounds its members by name.
    ///
    /// # Errors
    ///
    /// The field refused.
    pub fn check(&self, max_members: usize) -> Result<(), FieldError> {
        let name_ok = self
            .name
            .as_bytes()
            .first()
            .is_some_and(u8::is_ascii_alphanumeric)
            && self
                .name
                .bytes()
                .all(|c| c.is_ascii_alphanumeric() || matches!(c, b'_' | b'.' | b'-'));
        if self.name.is_empty() || self.name.len() > MAX_NAME_BYTES || !name_ok {
            return Err(field(
                "name",
                format!(
                    "{:?} is not a name: 1 to {MAX_NAME_BYTES} letters, digits, '_', '.' and '-', \
                     starting with a letter or digit",
                    self.name
                ),
            ));
        }
        if self.description.len() > MAX_DESCRIPTION_BYTES
            || self.description.chars().any(|c| c < ' ' || c == '\u{7f}')
        {
            return Err(field(
                "description",
                format!(
                    "the description is at most {MAX_DESCRIPTION_BYTES} bytes of printable \
                     characters"
                ),
            ));
        }
        if self.selector.is_empty() && self.members.is_empty() {
            return Err(field(
                "selector",
                "a group is a label selector, members by name, or both: give one",
            ));
        }
        if self.members.len() > max_members {
            return Err(field(
                "members",
                format!(
                    "{} members by name, more than {max_members} (max_nodes)",
                    self.members.len()
                ),
            ));
        }
        if self.variables.len() > MAX_VARIABLES {
            return Err(field(
                "variables",
                format!(
                    "{} variables, more than {MAX_VARIABLES}",
                    self.variables.len()
                ),
            ));
        }
        Selector::parse(&self.selector).map_err(|e| field("selector", e))?;
        for n in &self.members {
            let ok = n.as_bytes().first().is_some_and(u8::is_ascii_alphanumeric)
                && n.len() <= MAX_NAME_BYTES
                && n.bytes()
                    .all(|c| c.is_ascii_alphanumeric() || matches!(c, b'_' | b'.' | b'-'));
            if !ok {
                return Err(field("members", format!("{n:?} is not a member's name")));
            }
        }
        for (k, v) in &self.variables {
            let ok = !k.is_empty()
                && k.len() <= MAX_VARIABLE_NAME
                && !k.as_bytes().first().is_some_and(u8::is_ascii_digit)
                && k.bytes().all(|c| c.is_ascii_alphanumeric() || c == b'_');
            if !ok {
                return Err(field(
                    "variables",
                    format!(
                        "{k:?} is not a variable's name: 1 to {MAX_VARIABLE_NAME} letters, digits \
                         and '_', not starting with a digit"
                    ),
                ));
            }
            if v.len() > MAX_VARIABLE_VALUE || v.contains('\0') {
                return Err(field(
                    "variables",
                    format!(
                        "the value of {k:?} is not at most {MAX_VARIABLE_VALUE} bytes of UTF-8"
                    ),
                ));
            }
        }
        Ok(())
    }

    /// Whether `m` is in the group: named, or matched by its selector.
    #[must_use]
    pub fn has(&self, name: &str, labels: &BTreeMap<String, String>) -> bool {
        self.members.iter().any(|n| n == name)
            || Selector::parse(&self.selector).is_ok_and(|s| s.matches(labels))
    }
}

/// One requirement of a selector.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Term {
    Exists(String),
    Absent(String),
    Equal(String, String),
    Not(String, String),
}

/// A label selector, Kubernetes' equality-based ones: terms separated by commas, each
/// `key=value` (or `key==value`), `key!=value`, `key` or `!key`. The empty selector selects no
/// member. fit's `Selector`.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Selector(Vec<Term>);

fn label_ok(s: &str) -> bool {
    let b = s.as_bytes();
    b.first().is_some_and(u8::is_ascii_alphanumeric)
        && b.last().is_some_and(u8::is_ascii_alphanumeric)
        && b.iter()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, b'_' | b'.' | b'/' | b'-'))
}

impl Selector {
    /// Reads a selector: fit's `ParseSelector`.
    ///
    /// # Errors
    ///
    /// Why it is not one.
    pub fn parse(s: &str) -> Result<Selector, String> {
        if s.len() > MAX_SELECTOR_BYTES {
            return Err(format!(
                "the selector is more than {MAX_SELECTOR_BYTES} bytes"
            ));
        }
        let mut out = Vec::new();
        for part in s.split(',') {
            let part = part.trim();
            if part.is_empty() {
                if !s.trim().is_empty() {
                    return Err(format!(
                        "{s:?} has an empty term: separate terms by single commas"
                    ));
                }
                continue;
            }
            let t = if let Some(k) = part.strip_prefix('!').filter(|_| !part.contains('=')) {
                Term::Absent(k.trim().to_owned())
            } else if let Some((k, v)) = part.split_once("!=") {
                Term::Not(k.trim().to_owned(), v.trim().to_owned())
            } else if let Some((k, v)) = part.split_once('=') {
                Term::Equal(
                    k.trim().to_owned(),
                    v.trim()
                        .strip_prefix('=')
                        .unwrap_or(v.trim())
                        .trim()
                        .to_owned(),
                )
            } else {
                Term::Exists(part.to_owned())
            };
            let (k, v) = match &t {
                Term::Exists(k) | Term::Absent(k) => (k.as_str(), ""),
                Term::Equal(k, v) | Term::Not(k, v) => (k.as_str(), v.as_str()),
            };
            if k.len() > MAX_LABEL_KEY || !label_ok(k) {
                return Err(format!(
                    "{part:?}: {k:?} is not a label key: 1 to {MAX_LABEL_KEY} letters, digits, \
                     '_', '.', '/' and '-', starting and ending with a letter or digit"
                ));
            }
            if v.len() > MAX_LABEL_VALUE || !v.is_empty() && !label_ok(v) {
                return Err(format!(
                    "{part:?}: {v:?} is not a label value: 0 to {MAX_LABEL_VALUE} letters, \
                     digits, '_', '.', '/' and '-', starting and ending with a letter or digit"
                ));
            }
            out.push(t);
        }
        if out.len() > MAX_LABELS {
            return Err(format!(
                "the selector has {} terms, more than {MAX_LABELS}",
                out.len()
            ));
        }
        Ok(Selector(out))
    }

    /// Whether `labels` meet every term; the empty selector matches nothing.
    #[must_use]
    pub fn matches(&self, labels: &BTreeMap<String, String>) -> bool {
        !self.0.is_empty()
            && self.0.iter().all(|t| match t {
                Term::Exists(k) => labels.contains_key(k),
                Term::Absent(k) => !labels.contains_key(k),
                Term::Equal(k, v) => labels.get(k) == Some(v),
                Term::Not(k, v) => labels.get(k) != Some(v),
            })
    }

    /// The selector as [`parse`](Selector::parse) reads it, its terms in their order.
    #[must_use]
    pub fn text(&self) -> String {
        self.0
            .iter()
            .map(|t| match t {
                Term::Exists(k) => k.clone(),
                Term::Absent(k) => format!("!{k}"),
                Term::Equal(k, v) => format!("{k}={v}"),
                Term::Not(k, v) => format!("{k}!={v}"),
            })
            .collect::<Vec<_>>()
            .join(", ")
    }
}

/// A group as the API takes it, fit's `GroupInput`.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct GroupInput {
    /// `name`.
    pub name: String,
    /// `description`.
    pub description: String,
    /// `selector`.
    pub selector: String,
    /// `members`.
    pub members: Vec<String>,
    /// `variables`.
    pub variables: BTreeMap<String, String>,
}

impl GroupInput {
    /// Reads a group from a request's JSON body.
    ///
    /// # Errors
    ///
    /// Why it is not one.
    pub fn from_value(v: &Value) -> Result<GroupInput, String> {
        let Value::Object(ms) = v else {
            return Err("the body is not an object".to_owned());
        };
        let mut i = GroupInput::default();
        for (k, x) in ms {
            match (k.as_str(), x) {
                (_, Value::Null) => {}
                ("name", Value::String(s)) => i.name.clone_from(s),
                ("description", Value::String(s)) => i.description.clone_from(s),
                ("selector", Value::String(s)) => i.selector.clone_from(s),
                ("members", Value::Array(a)) => {
                    for m in a {
                        match m {
                            Value::String(s) => i.members.push(s.clone()),
                            _ => return Err("members is not a list of strings".to_owned()),
                        }
                    }
                }
                ("variables", Value::Object(vs)) => {
                    for (vk, vv) in vs {
                        match vv {
                            Value::String(s) => {
                                i.variables.insert(vk.clone(), s.clone());
                            }
                            _ => return Err(format!("variables.{vk} is not a string")),
                        }
                    }
                }
                ("name" | "description" | "selector" | "members" | "variables", _) => {
                    return Err(format!("{k} is not of the type expected"));
                }
                _ => {}
            }
        }
        Ok(i)
    }

    /// The group this input makes, checked, stamped `now`, keeping `old`'s creation: fit's
    /// `GroupInput.group`.
    ///
    /// # Errors
    ///
    /// The field refused.
    pub fn group(
        &self,
        old: Option<&Group>,
        now: &str,
        max_members: usize,
    ) -> Result<Group, FieldError> {
        let sel = Selector::parse(&self.selector).map_err(|e| field("selector", e))?;
        let mut members: Vec<String> = Vec::new();
        for n in &self.members {
            let n = n.trim();
            if !n.is_empty() && !members.iter().any(|x| x == n) {
                members.push(n.to_owned());
            }
        }
        members.sort();
        let g = Group {
            name: self.name.trim().to_owned(),
            description: self.description.trim().to_owned(),
            selector: sel.text(),
            members,
            variables: self.variables.clone(),
            created: old.map_or_else(|| now.to_owned(), |o| o.created.clone()),
            updated: now.to_owned(),
            extra: old.map(|o| o.extra.clone()).unwrap_or_default(),
        };
        g.check(max_members)?;
        Ok(g)
    }
}

/// The members of `d` a selector or names select, sorted, and the names no member has.
fn select_in(d: &Document, sel: &Selector, names: &[String]) -> (Vec<String>, Vec<String>) {
    let mut nodes: Vec<String> = d
        .members
        .iter()
        .filter(|m| names.contains(&m.name) || sel.matches(&m.labels))
        .map(|m| m.name.clone())
        .collect();
    let mut missing: Vec<String> = Vec::new();
    for n in names {
        if d.index(n).is_none() && !missing.contains(n) {
            missing.push(n.clone());
        }
    }
    nodes.sort();
    missing.sort();
    (nodes, missing)
}

/// The members of group `g` of `d`, sorted.
#[must_use]
pub fn nodes_of(d: &Document, g: &Group) -> Vec<String> {
    let sel = Selector::parse(&g.selector).unwrap_or_default();
    select_in(d, &sel, &g.members).0
}

/// The names of the groups of `d` member `name` with `labels` is in, sorted.
#[must_use]
pub fn groups_of(d: &Document, name: &str, labels: &BTreeMap<String, String>) -> Vec<String> {
    let mut out: Vec<String> = d
        .groups
        .iter()
        .filter(|g| g.check(usize::MAX).is_ok() && g.has(name, labels))
        .map(|g| g.name.clone())
        .collect();
    out.sort();
    out
}

/// What the API shows of `g`, a group of `d`: fit's `GroupView`.
#[must_use]
pub fn view(d: &Document, g: &Group) -> String {
    let sel = Selector::parse(&g.selector).unwrap_or_default();
    let (nodes, missing) = select_in(d, &sel, &g.members);
    let mut o = Obj::new()
        .str("name", &g.name)
        .str_omit("description", &g.description)
        .str_omit("selector", &g.selector)
        .raw("members", &json::string_list(&g.members))
        .raw("variables", &g.variables_json());
    for (k, v) in &g.extra {
        o = o.raw(k, &json::compact(v));
    }
    o.raw("created", &json::quoted(&g.created))
        .raw("updated", &json::quoted(&g.updated))
        .raw("nodes", &json::string_list(&nodes))
        .raw("missing", &json::string_list(&missing))
        .end()
}

/// Has the groups that name member `old` name `new` instead; a member removed (`new` empty) is
/// taken out of them. fit's `renameMember`.
pub fn rename_member(d: &mut Document, old: &str, new: &str) {
    for g in &mut d.groups {
        if let Some(j) = g.members.iter().position(|n| n == old) {
            g.members.remove(j);
            if !new.is_empty() && !g.members.iter().any(|n| n == new) {
                g.members.push(new.to_owned());
                g.members.sort();
            }
        }
    }
}

fn describe(g: &Group) -> String {
    let mut parts = Vec::new();
    if !g.selector.is_empty() {
        parts.push(format!("selector {}", g.selector));
    }
    if !g.members.is_empty() {
        parts.push(format!("members {}", g.members.join(", ")));
    }
    if !g.variables.is_empty() {
        parts.push(format!(
            "variables {}",
            g.variables.keys().cloned().collect::<Vec<_>>().join(", ")
        ));
    }
    parts.join("; ")
}

fn changes(a: &Group, b: &Group) -> String {
    let mut out = Vec::new();
    for (c, n) in [
        (a.name != b.name, "name"),
        (a.description != b.description, "description"),
        (a.selector != b.selector, "selector"),
        (a.members != b.members, "members"),
        (a.variables != b.variables, "variables"),
    ] {
        if c {
            out.push(n);
        }
    }
    if out.is_empty() {
        "nothing".to_owned()
    } else {
        out.join(", ")
    }
}

fn entry(action: &str, group: &str, detail: &str, nodes: &[String]) -> Vec<(String, Value)> {
    let mut e = vec![
        ("action".to_owned(), Value::String(action.to_owned())),
        ("group".to_owned(), Value::String(group.to_owned())),
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

fn decode(ctx: &mut Context) -> Option<Value> {
    if ctx.body().len() > MAX_BODY_BYTES {
        ctx.respond(fail(
            413,
            &format!("a group is {MAX_BODY_BYTES} bytes at most"),
        ));
        return None;
    }
    match akit_roles::pages::decode(ctx) {
        Ok(b) => Some(b.0),
        Err(r) => {
            ctx.respond(r);
            None
        }
    }
}

fn no_group(name: &str) -> String {
    format!("no group is named {}", json::quoted(name))
}

/// The groups' endpoints, when `path` is one: fit's `groupRoutes`.
///
/// ```text
/// GET    /api/groups          the groups, each with its members now, with the members' ETag
/// POST   /api/groups          a group added
/// GET    /api/groups/<name>   a group, and its members' states
/// PUT    /api/groups/<name>   a group changed, or renamed
/// DELETE /api/groups/<name>   a group removed; its members stay
/// POST   /api/select          the members a selector and names select now, for a group's form
/// ```
pub(crate) fn serve(m: &Manager, ctx: &mut Context, user: &str, path: &str) -> bool {
    let method = ctx.method().clone();
    let get = matches!(method, Method::Get | Method::Head);
    match path {
        "/api/groups" if get => {
            let d = m.doc();
            let mut gs: Vec<&Group> = d.groups.iter().collect();
            gs.sort_by(|a, b| a.name.cmp(&b.name));
            let list = gs.iter().map(|g| view(&d, g)).collect::<Vec<_>>().join(",");
            let tag = m.etag();
            ctx.respond(
                answer(
                    200,
                    Obj::new()
                        .str("version", tag.trim_matches('"'))
                        .num("max_groups", MAX_GROUPS)
                        .str_omit("read_only", &m.read_only())
                        .raw("groups", &format!("[{list}]"))
                        .end(),
                )
                .header("ETag", tag),
            );
        }
        "/api/groups" if method == Method::Post => add(m, ctx, user),
        "/api/groups" => ctx.respond(not_allowed("GET, POST")),
        "/api/select" if method == Method::Post => select(m, ctx),
        "/api/select" => ctx.respond(not_allowed("POST")),
        p if p.starts_with("/api/groups/") => {
            let rest = p.trim_start_matches("/api/groups/");
            let (raw, sub) = rest.split_once('/').unwrap_or((rest, ""));
            if !sub.is_empty() {
                ctx.respond(no_endpoint());
                return true;
            }
            let name = crate::api::percent_decode(raw);
            match method {
                Method::Get | Method::Head => {
                    let d = m.doc();
                    match d.groups.iter().find(|g| g.name == name) {
                        None => ctx.respond(fail(404, &no_group(&name))),
                        Some(g) => {
                            let v = view(&d, g);
                            let nodes = nodes_of(&d, g);
                            let states = nodes
                                .iter()
                                .filter_map(|n| d.members.iter().find(|x| &x.name == n))
                                .map(|mem| m.state_json(mem, m.node(&mem.name).as_ref()))
                                .collect::<Vec<_>>()
                                .join(",");
                            let body = format!(
                                "{},\"states\":[{states}]}}",
                                v.strip_suffix('}').unwrap_or(&v)
                            );
                            ctx.respond(answer(200, body).header("ETag", m.etag()));
                        }
                    }
                }
                Method::Put => update(m, ctx, user, name),
                Method::Delete => remove(m, ctx, user, name),
                _ => ctx.respond(not_allowed("GET, PUT, DELETE")),
            }
        }
        _ => return false,
    }
    true
}

fn not_allowed(allow: &str) -> Response {
    fail(405, "the method is not allowed").header("Allow", allow)
}

fn select(m: &Manager, ctx: &mut Context) {
    let Some(v) = decode(ctx) else { return };
    let sel = match Selector::parse(json::str_of(&v, "selector")) {
        Ok(s) => s,
        Err(e) => return ctx.respond(Refusal::Field(field("selector", e)).response()),
    };
    let names = json::strings_of(&v, "members");
    let (nodes, missing) = select_in(&m.doc(), &sel, &names);
    ctx.respond(answer(
        200,
        Obj::new()
            .raw("nodes", &json::string_list(&nodes))
            .raw("missing", &json::string_list(&missing))
            .end(),
    ));
}

fn input(ctx: &mut Context) -> Option<GroupInput> {
    let v = decode(ctx)?;
    match GroupInput::from_value(&v) {
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

fn add(m: &Manager, ctx: &mut Context, user: &str) {
    let Some(input) = input(ctx) else { return };
    if !crate::api::if_match(m, ctx) {
        return;
    }
    let (me, user) = (m.clone(), user.to_owned());
    crate::api::on_pool(m, ctx, move || {
        let max = me.max_nodes();
        let r = me.change(&user, |d: &mut Document| {
            let g = input
                .group(None, &crate::now_text(), max)
                .map_err(Refusal::Field)?;
            if d.groups.iter().any(|x| x.name == g.name) {
                return Err(Refusal::Conflict(format!(
                    "a group is already named {}",
                    json::quoted(&g.name)
                )));
            }
            if d.groups.len() >= MAX_GROUPS {
                return Err(Refusal::Conflict(format!(
                    "the manager keeps {MAX_GROUPS} groups at most"
                )));
            }
            let nodes = nodes_of(d, &g);
            let e = entry("group.add", &g.name, &describe(&g), &nodes);
            let out = (view(d, &g), g.name.clone());
            d.groups.push(g);
            Ok((e, out))
        });
        match r {
            Ok((v, name)) => {
                me.log(format!("cluster: {user:?} added the group {name:?}"));
                answer(201, v).header("ETag", me.etag())
            }
            Err(e) => e.response(),
        }
    });
}

fn update(m: &Manager, ctx: &mut Context, user: &str, name: String) {
    let Some(mut input) = input(ctx) else { return };
    if !crate::api::if_match(m, ctx) {
        return;
    }
    if input.name.trim().is_empty() {
        input.name.clone_from(&name);
    }
    let (me, user) = (m.clone(), user.to_owned());
    crate::api::on_pool(m, ctx, move || {
        let max = me.max_nodes();
        let r = me.change(&user, |d: &mut Document| {
            let i = d
                .groups
                .iter()
                .position(|g| g.name == name)
                .ok_or_else(|| Refusal::NotFound(no_group(&name)))?;
            let old = d.groups.get(i).cloned().unwrap_or_default();
            let g = input
                .group(Some(&old), &crate::now_text(), max)
                .map_err(Refusal::Field)?;
            if g.name != name && d.groups.iter().any(|x| x.name == g.name) {
                return Err(Refusal::Conflict(format!(
                    "a group is already named {}",
                    json::quoted(&g.name)
                )));
            }
            let detail = changes(&old, &g);
            let new_name = g.name.clone();
            if let Some(slot) = d.groups.get_mut(i) {
                *slot = g.clone();
            }
            if new_name != name {
                crate::templates::rename_group(d, &name, &new_name);
            }
            let nodes = nodes_of(d, &g);
            Ok((entry("group.update", &name, &detail, &nodes), view(d, &g)))
        });
        match r {
            Ok(v) => {
                me.log(format!("cluster: {user:?} changed the group {name:?}"));
                answer(200, v).header("ETag", me.etag())
            }
            Err(e) => e.response(),
        }
    });
}

fn remove(m: &Manager, ctx: &mut Context, user: &str, name: String) {
    if !crate::api::if_match(m, ctx) {
        return;
    }
    let (me, user) = (m.clone(), user.to_owned());
    crate::api::on_pool(m, ctx, move || {
        let r = me.change(&user, |d: &mut Document| {
            let i = d
                .groups
                .iter()
                .position(|g| g.name == name)
                .ok_or_else(|| Refusal::NotFound(no_group(&name)))?;
            let g = d.groups.remove(i);
            let nodes = nodes_of(d, &g);
            crate::templates::rename_group(d, &name, "");
            Ok((entry("group.remove", &name, "", &nodes), ()))
        });
        match r {
            Ok(()) => {
                me.log(format!("cluster: {user:?} removed the group {name:?}"));
                answer(200, Obj::new().str("removed", &name).end()).header("ETag", me.etag())
            }
            Err(e) => e.response(),
        }
    });
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]
    use super::*;

    fn labels(kv: &[(&str, &str)]) -> BTreeMap<String, String> {
        kv.iter()
            .map(|(k, v)| ((*k).to_owned(), (*v).to_owned()))
            .collect()
    }

    #[test]
    fn selectors() {
        // fit's TestParseSelector and TestSelectorMatches.
        let s = Selector::parse("region=eu, tier!=db,  canary, !old, zone==a").unwrap();
        assert_eq!(s.text(), "region=eu, tier!=db, canary, !old, zone=a");
        let l = labels(&[("region", "eu"), ("canary", ""), ("zone", "a")]);
        assert!(s.matches(&l));
        assert!(!s.matches(&labels(&[
            ("region", "eu"),
            ("canary", ""),
            ("zone", "a"),
            ("tier", "db")
        ])));
        assert!(!s.matches(&labels(&[
            ("region", "eu"),
            ("canary", ""),
            ("zone", "a"),
            ("old", "1")
        ])));
        assert!(!Selector::parse("").unwrap().matches(&l));
        for bad in ["a,,b", "-a", "a=-b", "a in (b)", &"k,".repeat(40)] {
            assert!(Selector::parse(bad).is_err(), "{bad}");
        }
        assert!(Selector::parse(&"a".repeat(MAX_SELECTOR_BYTES + 1)).is_err());
    }

    #[test]
    fn groups_checked() {
        let now = "2026-11-27T00:00:00Z";
        let i = GroupInput {
            name: "eu".into(),
            selector: "region=eu".into(),
            members: vec!["b".into(), "a".into(), "a".into(), " ".into()],
            variables: [("domain".to_owned(), "eu.example".to_owned())].into(),
            ..GroupInput::default()
        };
        let g = i.group(None, now, 256).unwrap();
        assert_eq!(g.members, ["a", "b"]);
        let mut bad = i.clone();
        bad.selector = String::new();
        bad.members.clear();
        assert_eq!(bad.group(None, now, 256).unwrap_err().field, "selector");
        let mut bad = i.clone();
        bad.variables = [("1x".to_owned(), String::new())].into();
        assert_eq!(bad.group(None, now, 256).unwrap_err().field, "variables");
        assert_eq!(i.group(None, now, 1).unwrap_err().field, "members");
        let mut bad = i.clone();
        bad.name = "-x".into();
        assert_eq!(bad.group(None, now, 256).unwrap_err().field, "name");
        // Kept as fit writes it, read back the same.
        let again = Group::from_value(&json::parse(g.to_json().as_bytes()).unwrap());
        assert_eq!(again, g);
    }
}
