//! The console module `cluster`'s endpoints (I132, fit's `cluster.go`, `probe.go`, `read.go`,
//! `audit.go`'s route and `roles.go`'s `/api/me`): answered on the console's I/O thread when they
//! only read, on the manager's pool when they write the store, and from a member's answer when
//! they call one.

use std::sync::atomic::Ordering;
use std::sync::{Arc, Mutex, PoisonError};

use akio_http::{Context, Detached, Method, Response};
use akit_config::value::Value;
use akit_roles::pages::{fail, json as answer, no_endpoint};

use crate::json::{self, Obj, quoted};
use crate::member::{
    ACCESS_CONSOLE, Document, Input, MAX_AUDIT, MAX_AUDIT_BYTES, MAX_BODY_BYTES, Member, changes,
    describe,
};
use crate::node::Node;
use crate::roles::{Need, OPERATOR, VIEWER};
use crate::transport::{KIND_PASSWORD_CHANGE, KIND_UNSUPPORTED, Problem};
use crate::{MAX_READS, MAX_TESTS, Manager, Refusal};

/// The module, served by the console.
pub(crate) struct Module(pub(crate) Manager);

impl akit_roles::pages::Module for Module {
    fn id(&self) -> &'static str {
        crate::MODULE_ID
    }

    fn console_gate(&self, user: &str, method: &Method, path: &str, query: &str) -> Option<String> {
        self.0.console_gate(user, method, path, query)
    }

    fn serve(&self, ctx: &mut Context, user: &str, path: &str) {
        let m = &self.0;
        let method = ctx.method().clone();
        let get = matches!(method, Method::Get | Method::Head);
        // Every endpoint but /api/me is gated by the user's roles (I139).
        if path == "/api/me" {
            crate::roles::serve(m, ctx, user, path);
            return;
        }
        if let Some(refused) = m.gate(user, &method, checked(&method, path)) {
            return ctx.respond(refused);
        }
        // The node switcher (I134): the console's /api/n/, never the module's /api/m/cluster/n/.
        if path.starts_with(crate::pass::PASS_PREFIX) {
            return crate::pass::pass_on(m, ctx, user);
        }
        // Groups and operations on many (I135).
        if crate::groups::serve(m, ctx, user, path)
            || crate::ops::serve(m, ctx, user, path)
            || crate::templates::serve(m, ctx, user, path)
            || crate::sync::serve(m, ctx, user, path)
            || crate::rollouts::serve(m, ctx, user, path)
            || crate::alerts::serve(m, ctx, user, path)
            || crate::roles::serve(m, ctx, user, path)
            || crate::link::serve(m, ctx, user, path)
        {
            return;
        }
        match path {
            "/api/nodes" if get => ctx.respond(list(m)),
            "/api/nodes" if method == Method::Post => add(m, ctx, user),
            "/api/nodes" => ctx.respond(not_allowed("GET, POST")),
            "/api/test" if method == Method::Post => test_draft(m, ctx, user),
            "/api/test" => ctx.respond(not_allowed("POST")),
            "/api/state" if get => ctx.respond(state(m)),
            "/api/audit" if get => ctx.respond(audit(m, ctx.req().query().unwrap_or(""))),
            "/api/state" | "/api/audit" => ctx.respond(not_allowed("GET")),
            p if p.starts_with("/api/nodes/") => node_route(m, ctx, user, p),
            _ => ctx.respond(no_endpoint()),
        }
    }
}

fn not_allowed(allow: &str) -> Response {
    fail(405, "the method is not allowed").header("Allow", allow)
}

fn lock<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap_or_else(PoisonError::into_inner)
}

/// `GET /api/nodes`: the members, with their ETag.
fn list(m: &Manager) -> Response {
    let mut nodes = String::from("[");
    for (i, mem) in m.members().iter().enumerate() {
        if i > 0 {
            nodes.push(',');
        }
        nodes.push_str(&mem.view());
    }
    nodes.push(']');
    let tag = m.etag();
    let body = Obj::new()
        .str("version", tag.trim_matches('"'))
        .num("max_nodes", m.max_nodes())
        .str_omit("read_only", &m.read_only())
        .raw("nodes", &nodes)
        .end();
    answer(200, body).header("ETag", tag)
}

/// `GET /api/state`.
fn state(m: &Manager) -> Response {
    let lim = m.limits();
    answer(
        200,
        Obj::new()
            .num("interval_ms", json::millis(m.interval()))
            .num("timeout_ms", json::millis(lim.timeout))
            .str_omit("read_only", &m.read_only())
            .raw("nodes", &m.states_json())
            .end(),
    )
}

/// Whether an endpoint checks what it touches itself, rather than its change being a
/// cluster-wide admin's (fit's `Checked` routes).
fn checked(method: &Method, path: &str) -> bool {
    let post = *method == Method::Post;
    path.starts_with("/api/nodes/")
        || path.starts_with("/api/rollouts/")
        || path.starts_with("/api/alerts/silences/")
        || path.starts_with("/api/desired/")
        || path.starts_with(crate::pass::PASS_PREFIX)
        || (post
            && matches!(
                path,
                "/api/nodes"
                    | "/api/test"
                    | "/api/select"
                    | "/api/ops"
                    | "/api/sync"
                    | "/api/compare"
                    | "/api/import"
                    | "/api/text/render"
                    | "/api/text/parse"
                    | "/api/rollouts"
                    | "/api/alerts/silences"
                    | "/api/drift/fix"
                    | "/api/enroll"
            ))
}

/// `GET /api/audit`: the entries the query picks (`user`, `node`, `action`, a prefix ending with
/// `.` or `*` picking its family), oldest first; with `format=jsonl`, JSON lines.
fn audit(m: &Manager, query: &str) -> Response {
    let mut f = (String::new(), String::new(), String::new(), String::new());
    for kv in query.split('&').filter(|s| !s.is_empty()) {
        let (k, v) = kv.split_once('=').unwrap_or((kv, ""));
        let v = percent_decode(v);
        match k {
            "user" => f.0 = v,
            "node" => f.1 = v,
            "action" => f.2 = v,
            "format" => f.3 = v,
            _ => {}
        }
    }
    let all = m.audit();
    let (mut users, mut actions) = (Vec::new(), Vec::new());
    for e in &all {
        for (list, key) in [(&mut users, "user"), (&mut actions, "action")] {
            let s = json::str_of(e, key).to_owned();
            if !s.is_empty() && !list.contains(&s) {
                list.push(s);
            }
        }
    }
    users.sort();
    actions.sort();
    let picked: Vec<&Value> = all
        .iter()
        .filter(|e| {
            let action = json::str_of(e, "action");
            (f.0.is_empty() || json::str_of(e, "user") == f.0)
                && (f.1.is_empty()
                    || json::str_of(e, "node") == f.1
                    || json::strings_of(e, "nodes").contains(&f.1))
                && (f.2.is_empty()
                    || action == f.2
                    || f.2.strip_suffix('*').is_some_and(|p| action.starts_with(p))
                    || f.2.ends_with('.') && action.starts_with(f.2.as_str()))
        })
        .collect();
    match f.3.as_str() {
        "" | "json" => {}
        "jsonl" => {
            let mut b = String::new();
            for e in &picked {
                b.push_str(&json::compact(e));
                b.push('\n');
            }
            return Response::with_body(200, "application/jsonl", b.into_bytes()).header(
                "Content-Disposition",
                "attachment; filename=\"akit-cluster-audit.jsonl\"",
            );
        }
        _ => return fail(400, "format is json or jsonl"),
    }
    let mut entries = String::from("[");
    for (i, e) in picked.iter().enumerate() {
        if i > 0 {
            entries.push(',');
        }
        entries.push_str(&json::compact(e));
    }
    entries.push(']');
    answer(
        200,
        Obj::new()
            .raw("entries", &entries)
            .num("total", all.len())
            .num("max_entries", MAX_AUDIT)
            .num("max_bytes", MAX_AUDIT_BYTES)
            .raw("users", &json::string_list(&users))
            .raw("actions", &json::string_list(&actions))
            .end(),
    )
}

pub(crate) fn percent_decode(s: &str) -> String {
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0usize;
    while let Some(&c) = b.get(i) {
        let hex = |j: usize| {
            b.get(j)
                .and_then(|x| char::from(*x).to_digit(16))
                .and_then(|d| u8::try_from(d).ok())
        };
        match c {
            b'%' => match (hex(i.saturating_add(1)), hex(i.saturating_add(2))) {
                (Some(h), Some(l)) => {
                    out.push(h.saturating_mul(16).saturating_add(l));
                    i = i.saturating_add(3);
                    continue;
                }
                _ => out.push(c),
            },
            b'+' => out.push(b' '),
            _ => out.push(c),
        }
        i = i.saturating_add(1);
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// The member a request's body holds, bounded: `None` with the request answered when it is
/// not one.
fn decode(ctx: &mut Context) -> Option<Input> {
    if ctx.body().len() > MAX_BODY_BYTES {
        ctx.respond(fail(
            413,
            &format!("a member is {MAX_BODY_BYTES} bytes at most"),
        ));
        return None;
    }
    let body = match akit_roles::pages::decode(ctx) {
        Ok(b) => b,
        Err(r) => {
            ctx.respond(r);
            return None;
        }
    };
    match Input::from_value(&body.0) {
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

/// Whether the request's `If-Match`, when it sends one, is the members' ETag; answers 412 when
/// not.
pub(crate) fn if_match(m: &Manager, ctx: &mut Context) -> bool {
    let want = ctx.header("If-Match").unwrap_or("").to_owned();
    if want.is_empty() || want == "*" || want == m.etag() {
        return true;
    }
    ctx.respond(fail(
        412,
        "the members changed since the page read them; read them again",
    ));
    false
}

/// Runs `work` on the manager's pool, answering `ctx` with what it returns; 503 when too many
/// wait.
pub(crate) fn on_pool(
    m: &Manager,
    ctx: &mut Context,
    work: impl FnOnce() -> Response + Send + 'static,
) {
    let Some(d) = ctx.detach() else { return };
    let cell = Arc::new(Mutex::new(Some(d)));
    let taken = Arc::clone(&cell);
    let job = Box::new(move || {
        let d = lock(&taken).take();
        if let Some(d) = d {
            d.respond(work());
        }
    });
    if !m.on_pool(job) {
        let d = lock(&cell).take();
        if let Some(d) = d {
            d.respond(fail(503, "the manager is busy; try again"));
        }
    }
}

fn changed(m: &Manager, status: u16, body: String) -> Response {
    answer(status, body).header("ETag", m.etag())
}

fn entry(action: &str, node: &str, detail: &str) -> Vec<(String, Value)> {
    let mut e = vec![
        ("action".to_owned(), Value::String(action.to_owned())),
        ("node".to_owned(), Value::String(node.to_owned())),
    ];
    if !detail.is_empty() {
        e.push(("detail".to_owned(), Value::String(detail.to_owned())));
    }
    e
}

/// `POST /api/nodes`.
fn add(m: &Manager, ctx: &mut Context, user: &str) {
    let Some(input) = decode(ctx) else { return };
    if !if_match(m, ctx) {
        return;
    }
    let (me, user) = (m.clone(), user.to_owned());
    on_pool(m, ctx, move || {
        let max = me.max_nodes();
        let r = me.change(&user, |d: &mut Document| {
            let mem = input
                .member(None, &crate::now_text())
                .map_err(Refusal::Field)?;
            if d.index(&mem.name).is_some() {
                return Err(Refusal::Conflict(format!(
                    "a member is already named {}",
                    quoted(&mem.name)
                )));
            }
            if d.members.len() >= max {
                return Err(Refusal::Conflict(format!(
                    "the manager keeps {max} members at most (max_nodes)"
                )));
            }
            let e = entry("node.add", &mem.name, &describe(&mem));
            let out = (mem.view(), describe(&mem), mem.name.clone());
            // An admin of the member, in the groups its labels put it in (I139).
            d.members.push(mem.clone());
            me.allow_in(d, &user, crate::roles::ADMIN, &[&mem])?;
            Ok((e, out))
        });
        match r {
            Ok((view, what, name)) => {
                me.log(format!(
                    "cluster: {user:?} added the member {name:?}: {what}"
                ));
                changed(&me, 201, view)
            }
            Err(e) => e.response(),
        }
    });
}

/// `/api/nodes/<name>[/test | /read/<what>]`.
fn node_route(m: &Manager, ctx: &mut Context, user: &str, path: &str) {
    let rest = path.strip_prefix("/api/nodes/").unwrap_or("");
    let (raw, sub) = rest.split_once('/').unwrap_or((rest, ""));
    let name = percent_decode(raw);
    let method = ctx.method().clone();
    match (sub, &method) {
        ("test", Method::Post) => test_saved(m, ctx, user, &name),
        ("test", _) => ctx.respond(not_allowed("POST")),
        ("enroll", Method::Post) => crate::link::enroll_route(m, ctx, user, &name),
        ("enroll", _) => ctx.respond(not_allowed("POST")),
        (s, _) if s.starts_with("read/") => {
            if let Err(e) = m.allow(user, &Need::on(VIEWER, vec![name.clone()])) {
                return ctx.respond(e.response());
            }
            read(m, ctx, &name, s.trim_start_matches("read/"));
        }
        ("", Method::Get | Method::Head) => {
            if let Err(e) = m.allow(user, &Need::on(VIEWER, vec![name.clone()])) {
                return ctx.respond(e.response());
            }
            let mem = m.members().into_iter().find(|x| x.name == name);
            match mem {
                None => ctx.respond(fail(404, &not_found(&name))),
                Some(mem) => {
                    let st = m.state_json(&mem, m.node(&name).as_ref());
                    let view = mem.view();
                    let body = format!(
                        "{},\"state\":{st}}}",
                        view.strip_suffix('}').unwrap_or(&view)
                    );
                    ctx.respond(answer(200, body));
                }
            }
        }
        ("", Method::Put) => update(m, ctx, user, name),
        ("", Method::Delete) => remove(m, ctx, user, name),
        ("", _) => ctx.respond(not_allowed("GET, PUT, DELETE")),
        _ => ctx.respond(no_endpoint()),
    }
}

fn not_found(name: &str) -> String {
    format!("no member is named {}", quoted(name))
}

/// `PUT /api/nodes/<name>`: replaces the member, which may rename it.
fn update(m: &Manager, ctx: &mut Context, user: &str, name: String) {
    let Some(mut input) = decode(ctx) else { return };
    if !if_match(m, ctx) {
        return;
    }
    if input.name.is_empty() {
        input.name.clone_from(&name);
    }
    let (me, user) = (m.clone(), user.to_owned());
    on_pool(m, ctx, move || {
        let r = me.change(&user, |d: &mut Document| {
            let i = d
                .index(&name)
                .ok_or_else(|| Refusal::NotFound(not_found(&name)))?;
            let old = d
                .members
                .get(i)
                .cloned()
                .ok_or_else(|| Refusal::NotFound(not_found(&name)))?;
            let mem = input
                .member(Some(&old), &crate::now_text())
                .map_err(Refusal::Field)?;
            // An admin of the member as it is and as it would be (I139).
            me.allow_in(d, &user, crate::roles::ADMIN, &[&old])?;
            if mem.name != name && d.index(&mem.name).is_some() {
                return Err(Refusal::Conflict(format!(
                    "a member is already named {}",
                    quoted(&mem.name)
                )));
            }
            let mut e = entry("node.update", &name, &changes(&old, &mem));
            if mem.name != name {
                e.push((
                    "nodes".to_owned(),
                    Value::Array(vec![Value::String(mem.name.clone())]),
                ));
            }
            let out = (mem.view(), describe(&mem));
            let new_name = mem.name.clone();
            if let Some(slot) = d.members.get_mut(i) {
                *slot = mem;
            }
            if new_name != name {
                crate::groups::rename_member(d, &name, &new_name);
            }
            if let Some(now) = d.members.get(i).cloned() {
                me.allow_in(d, &user, crate::roles::ADMIN, &[&now])?;
            }
            Ok((e, out))
        });
        match r {
            Ok((view, what)) => {
                me.log(format!(
                    "cluster: {user:?} changed the member {name:?}: {what}"
                ));
                changed(&me, 200, view)
            }
            Err(e) => e.response(),
        }
    });
}

/// `DELETE /api/nodes/<name>`.
fn remove(m: &Manager, ctx: &mut Context, user: &str, name: String) {
    if !if_match(m, ctx) {
        return;
    }
    let (me, user) = (m.clone(), user.to_owned());
    on_pool(m, ctx, move || {
        let r = me.change(&user, |d: &mut Document| {
            let i = d
                .index(&name)
                .ok_or_else(|| Refusal::NotFound(not_found(&name)))?;
            if let Some(old) = d.members.get(i).cloned() {
                me.allow_in(d, &user, crate::roles::ADMIN, &[&old])?;
            }
            d.members.remove(i);
            crate::groups::rename_member(d, &name, "");
            Ok((entry("node.remove", &name, ""), ()))
        });
        match r {
            Ok(()) => {
                me.log(format!("cluster: {user:?} removed the member {name:?}"));
                changed(&me, 200, Obj::new().str("removed", &name).end())
            }
            Err(e) => e.response(),
        }
    });
}

/// The result of a test, fit's `TestResult`.
fn test_result(
    p: Option<&Problem>,
    latency_ms: u64,
    cert: Option<String>,
    node: Option<String>,
) -> String {
    let mut o = Obj::new().bool("ok", p.is_none());
    if let Some(p) = p {
        o = o.raw("error", &p.json());
    }
    o = o
        .num("latency_ms", latency_ms)
        .opt("certificate", cert.as_deref())
        .opt("node", node.as_deref());
    if p.is_some_and(|p| p.kind == KIND_PASSWORD_CHANGE) {
        o = o.bool("must_change_password", true);
    }
    o.end()
}

/// `POST /api/test`: a member not added yet, as the body has it.
fn test_draft(m: &Manager, ctx: &mut Context, user: &str) {
    let Some(input) = decode(ctx) else { return };
    match input.member(None, &crate::now_text()) {
        Ok(mem) => {
            // A draft is an admin's of the member it would be (I139).
            if let Err(e) = m.allow_in(&m.doc(), user, crate::roles::ADMIN, &[&mem]) {
                return ctx.respond(e.response());
            }
            test(m, ctx, user, mem, true, None);
        }
        Err(e) => ctx.respond(Refusal::Field(e).response()),
    }
}

/// `POST /api/nodes/<name>/test`: the member as kept, or as the body would change it, not saved.
fn test_saved(m: &Manager, ctx: &mut Context, user: &str, name: &str) {
    let draft = !String::from_utf8_lossy(ctx.body()).trim().is_empty();
    let input = if draft {
        match decode(ctx) {
            Some(i) => Some(i),
            None => return,
        }
    } else {
        None
    };
    let Some(saved) = m.members().into_iter().find(|x| x.name == name) else {
        return ctx.respond(fail(404, &not_found(name)));
    };
    // A test of the member as kept is an operator's; of a draft, an admin's (I139).
    let role = if draft { crate::roles::ADMIN } else { OPERATOR };
    if let Err(e) = m.allow(user, &Need::on(role, vec![name.to_owned()])) {
        return ctx.respond(e.response());
    }
    let mem = match input {
        Some(mut i) => {
            if i.name.is_empty() {
                i.name = name.to_owned();
            }
            match i.member(Some(&saved), &crate::now_text()) {
                Ok(x) => x,
                Err(e) => return ctx.respond(Refusal::Field(e).response()),
            }
        }
        None => {
            if let Err(e) = saved.check() {
                return ctx.respond(Refusal::Field(e).response());
            }
            saved
        }
    };
    let node = m.node(name);
    test(m, ctx, user, mem, draft, node);
}

/// Tests `mem` on connections of its own, answering `ctx` once it is over: reached, logged in,
/// its `GET /node` read, its certificate shown. The kept node's login backoff is reset when its
/// credentials worked. fit's `Manager.test`.
fn test(
    m: &Manager,
    ctx: &mut Context,
    user: &str,
    mem: Member,
    draft: bool,
    kept: Option<Arc<Node>>,
) {
    if m.0.tests.fetch_add(1, Ordering::AcqRel) >= MAX_TESTS {
        m.0.tests.fetch_sub(1, Ordering::AcqRel);
        return ctx.respond(fail(429, "other tests are under way; try again"));
    }
    let n = match m.new_node(&mem) {
        Ok(n) => n,
        Err(e) => {
            m.0.tests.fetch_sub(1, Ordering::AcqRel);
            return ctx.respond(fail(400, &e));
        }
    };
    let Some(d) = ctx.detach() else {
        m.0.tests.fetch_sub(1, Ordering::AcqRel);
        return;
    };
    let what = if draft {
        format!("a draft: {}", describe(&mem))
    } else {
        "the member as kept".to_owned()
    };
    let (me, user, name) = (m.clone(), user.to_owned(), mem.name.clone());
    let n2 = Arc::clone(&n);
    let end = move |d: Detached, prob: Option<Problem>, latency: u64, node: Option<String>| {
        let cert = n2
            .transport()
            .and_then(|t| t.certificate())
            .map(|c| c.json());
        let prob = prob.map(|mut p| {
            p.message = n2.redact(&p.message);
            // The certificate is the result's, once.
            p.certificate = None;
            p
        });
        if prob.is_none() {
            if let Some(k) = kept.as_ref().filter(|k| k.spec.same_link(&n2.spec)) {
                k.reset_login();
            }
        }
        let mut e = vec![
            ("action".to_owned(), Value::String("node.test".to_owned())),
            ("node".to_owned(), Value::String(name.clone())),
            (
                "outcome".to_owned(),
                Value::String(if prob.is_none() { "ok" } else { "failed" }.to_owned()),
            ),
        ];
        let detail = match &prob {
            None => what.clone(),
            Some(p) => format!("{what}; {}: {}", p.kind, p.message),
        };
        e.push(("detail".to_owned(), Value::String(clip(&detail, 512))));
        me.record(&user, e);
        d.respond(answer(200, test_result(prob.as_ref(), latency, cert, node)));
        let me2 = me.clone();
        n2.shutdown(Box::new(move || {
            me2.0.tests.fetch_sub(1, Ordering::AcqRel);
        }));
    };
    let path = n.paths().node;
    let n3 = Arc::clone(&n);
    let run = move || {
        n3.get(
            path,
            Box::new(move |r| match r {
                Err(p) => end(d, Some(p), 0, None),
                Ok((body, took)) => {
                    let info = json::parse(&body).map(|v| crate::node::node_info_json(&v));
                    end(d, None, json::millis(took), info);
                }
            }),
        );
    };
    // Over TLS the probe reads the certificate first (akio request A277's stopgap), so that a
    // person sees it whatever the call finds.
    match n.transport().filter(|t| t.tls()) {
        Some(t) => t.probe(Box::new(move |_| run())),
        None => run(),
    }
}

pub(crate) fn clip(s: &str, max: usize) -> String {
    if s.len() <= max {
        return s.to_owned();
    }
    let mut cut = max;
    while !s.is_char_boundary(cut) {
        cut = cut.saturating_sub(1);
    }
    format!("{}…", s.get(..cut).unwrap_or(""))
}

/// The member's reads `GET /api/nodes/<name>/read/<what>` passes on, fit's `Reads`: the path
/// through its console, through its Admin API, and the capability its `GET /node` lists for it.
const READS: &[(&str, &str, &str, &str)] = &[
    (
        "check",
        "/api/m/admin/config/check",
        "/config/check",
        "config.check",
    ),
    ("node", "/api/node", "/node", "node"),
    ("status", "/api/m/admin/status", "/status", "status"),
    (
        "versions",
        "/api/m/admin/versions",
        "/versions",
        "config.versions",
    ),
];

/// `GET /api/nodes/<name>/read/<what>`: the member's answer, as it answered it, through the node
/// the poller reads it with; or 502 with the problem.
fn read(m: &Manager, ctx: &mut Context, name: &str, what: &str) {
    let Some(&(_, console, admin, cap)) = READS.iter().find(|r| r.0 == what) else {
        return ctx.respond(fail(
            404,
            "a member's reads are check, node, status, versions",
        ));
    };
    if !matches!(ctx.method(), Method::Get | Method::Head) {
        return ctx.respond(not_allowed("GET"));
    }
    let Some(n) = m.node(name) else {
        return ctx.respond(fail(404, &not_found(name)));
    };
    // A member that said what it serves is not asked for what it does not.
    if let Some(caps) = n.caps() {
        if cap != "node" && !caps.iter().any(|c| c == cap) {
            let msg = format!("the member does not list {cap} in its capabilities");
            let p = Problem::new(KIND_UNSUPPORTED, msg.clone());
            return ctx.respond(answer(
                502,
                Obj::new()
                    .str("error", &msg)
                    .raw("problem", &p.json())
                    .end(),
            ));
        }
    }
    if m.0.reads.fetch_add(1, Ordering::AcqRel) >= MAX_READS {
        m.0.reads.fetch_sub(1, Ordering::AcqRel);
        return ctx.respond(fail(429, "other reads of members are under way; try again"));
    }
    let Some(d) = ctx.detach() else {
        m.0.reads.fetch_sub(1, Ordering::AcqRel);
        return;
    };
    let path = if n.spec.access == ACCESS_CONSOLE {
        console
    } else {
        admin
    };
    let me = m.clone();
    n.get(
        path,
        Box::new(move |r| {
            me.0.reads.fetch_sub(1, Ordering::AcqRel);
            d.respond(match r {
                Ok((body, _)) => Response::with_body(200, "application/json", body),
                Err(p) => answer(
                    502,
                    Obj::new()
                        .str("error", &p.message)
                        .raw("problem", &p.json())
                        .end(),
                ),
            });
        }),
    );
}
