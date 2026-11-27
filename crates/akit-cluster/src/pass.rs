//! The node switcher's pass-through (I134, fit's `cluster/passthrough.go`, its I143). The
//! workbench, with a member chosen in its header, draws its own pages against that member: each
//! call a page makes to `/api/m/<module>/<path>` goes to `/api/n/<member>/m/<module>/<path>` on the
//! manager's console instead, and the manager passes it on to the member as it reaches it (its
//! console with the session it keeps, its Admin API with its token), and its answer back. The
//! browser never holds the member's credentials, and its own stop at the manager. Only the
//! request headers a page sends go on, and only the answer headers it reads come back.
//!
//! A write (anything but a GET or a HEAD, the text conversions and a dry run) is refused while
//! the member's config schema is not the manager's own, as the latest poll read it, and while the
//! manager's members cannot be written; otherwise it is recorded in the audit log, with its user,
//! method and path, how the member answered, and the digest of what it sent.
//!
//! fit members answer fit's header fields: their `Fit-Config-Version` and `Fit-Dry-Run` come back
//! as the `Akit-Config-Version` and `Akit-Dry-Run` akit's pages read, and the store password a
//! page sends in `Akit-Store-Password` goes in both akit's and fit's field (decision 041, 3).

use std::collections::{HashMap, VecDeque};
use std::sync::atomic::Ordering;
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Instant;

use akio_crypto::hash::Sha256;
use akio_http::{Context, Detached, Method, Response};
use akit_roles::pages::{fail, json as answer};

use crate::Manager;
use crate::json::{self, Obj};
use crate::member::ACCESS_CONSOLE;
use crate::node::Node;
use crate::transport::{
    KIND_AUTH, KIND_BAD_ANSWER, KIND_PASSWORD_CHANGE, KIND_RATE_LIMITED, Problem, Reply,
};

/// Where the manager's console passes calls on to a member.
pub const PASS_PREFIX: &str = "/api/n/";
/// How many calls passed on are under way or waiting at once, to every member.
pub const MAX_PASS_CALLS: usize = 64;
/// How many go to one member at once; the others wait their turn, at most
/// [`MAX_PASS_WAITING`] of them, each for the call timeout at most.
pub const MAX_PASS_PER_NODE: usize = 3;
/// See [`MAX_PASS_PER_NODE`].
pub const MAX_PASS_WAITING: usize = 16;
/// How many writes may wait for their audit entries; one more is answered 429 before it is sent.
pub const MAX_PASS_AUDIT: usize = 64;
/// The audit action of a write passed on.
pub const ACTION_API: &str = "node.api";

/// Why a member's pages are read-only: not read yet.
pub const READ_ONLY_NOT_READ: &str = "not_read";
/// Its config schema is not the manager's.
pub const READ_ONLY_VERSION: &str = "version";
/// The manager's members cannot be written.
pub const READ_ONLY_MANAGER: &str = "manager";

/// The request headers passed on: what the pages send.
const PASS_HEADERS: &[&str] = &[
    "Content-Type",
    "If-Match",
    "If-None-Match",
    "Upload-Offset",
    "Upload-Length",
    "Akit-Store-Password",
];

/// The answer headers passed back, and the names a fit member answers them by.
const ANSWER_HEADERS: &[(&str, &str)] = &[
    ("ETag", "ETag"),
    ("Akit-Config-Version", "Fit-Config-Version"),
    ("Akit-Dry-Run", "Fit-Dry-Run"),
    ("Retry-After", "Retry-After"),
    ("Allow", "Allow"),
    ("Upload-Offset", "Upload-Offset"),
    ("Upload-Length", "Upload-Length"),
];

/// The media types an answer keeps; any other is answered as `application/octet-stream`, which
/// no browser renders on the manager's origin.
const ANSWER_TYPES: &[&str] = &[
    "application/json",
    "application/schema+json",
    "text/plain",
    "application/octet-stream",
    "application/yaml",
    "text/yaml",
];

/// The console modules never opened through the switcher: the member's own management, its
/// assistant, and the LLM roles, whose pages read through POST RPCs and streams.
fn not_passed(module: &str) -> bool {
    module == crate::MODULE_ID || module == "ai" || module.starts_with("llm.")
}

type Release = Box<dyn FnOnce() + Send>;
type Run = Box<dyn FnOnce(Release) + Send>;

struct Wait {
    at: Instant,
    run: Run,
    stale: Box<dyn FnOnce() + Send>,
}

#[derive(Default)]
struct Queue {
    running: usize,
    waiting: VecDeque<Wait>,
}

/// The calls passed on: those under way and waiting, by node.
#[derive(Default)]
pub(crate) struct Hub {
    total: usize,
    queues: HashMap<usize, Queue>,
}

fn lock<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap_or_else(PoisonError::into_inner)
}

/// Where a call to `/api/n/<member>/<sub>` goes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Route {
    what: &'static str,
    path: String,
    module: String,
}

/// Where `sub` goes, or the status and message of why it goes nowhere: fit's `routeOf`.
pub(crate) fn route_of(sub: &str) -> Result<Route, (u16, String)> {
    let nowhere = || (404, "no such path".to_owned());
    for seg in sub.split('/') {
        let s = crate::api::percent_decode(seg);
        if s.is_empty() || s == "." || s == ".." || s.contains(['\\', '\0']) {
            return Err(nowhere());
        }
    }
    match sub {
        "workbench" => {
            return Ok(Route {
                what: "workbench",
                path: String::new(),
                module: String::new(),
            });
        }
        "node" => {
            return Ok(Route {
                what: "node",
                path: "/node".to_owned(),
                module: "admin".to_owned(),
            });
        }
        _ => {}
    }
    let Some(rest) = sub.strip_prefix("m/") else {
        return Err((
            404,
            "no such path: a member's API is passed on at m/<module>/<path>".to_owned(),
        ));
    };
    let (module, p) = rest.split_once('/').unwrap_or((rest, ""));
    if p.is_empty() {
        return Err(nowhere());
    }
    if module == "admin" {
        let head = p.split('/').next().unwrap_or("");
        return match head {
            "users" | "account" => Err((
                403,
                "a member's console users and passwords are changed on the member, not through \
                 the manager"
                    .to_owned(),
            )),
            "debug" | "dashboard" => Err(nowhere()),
            _ => Ok(Route {
                what: "admin",
                path: format!("/{p}"),
                module: module.to_owned(),
            }),
        };
    }
    if not_passed(module) {
        return Err((
            403,
            format!(
                "the member's module {} is not opened through the manager: open the member's own \
                 console for it",
                json::quoted(module)
            ),
        ));
    }
    Ok(Route {
        what: "module",
        path: format!("/api/m/{module}/{p}"),
        module: module.to_owned(),
    })
}

/// Whether a call changes the member: anything but a GET or a HEAD, the text conversions and a
/// dry run.
pub(crate) fn writes(method: &Method, rt: &Route, query: &str) -> bool {
    if matches!(method, Method::Get | Method::Head) {
        return false;
    }
    if rt.what == "admin"
        && *method == Method::Post
        && (rt.path == "/config/render" || rt.path == "/config/parse")
    {
        return false;
    }
    if rt.what == "admin"
        && query
            .split('&')
            .any(|kv| kv == "dry_run=1" || kv == "dry_run=true")
    {
        return false;
    }
    true
}

fn short_digest(d: &str) -> String {
    match d.get(..19) {
        Some(s) if d.len() > 19 => format!("{s}…"),
        _ => d.to_owned(),
    }
}

impl Manager {
    /// Why the pages may not write to a member whose schema digest is `digest`, its kind and its
    /// message, or `None` when they may.
    pub(crate) fn compatible(&self, digest: &str) -> Option<(&'static str, String)> {
        let own = &self.0.schema_digest;
        if digest.is_empty() {
            return Some((
                READ_ONLY_NOT_READ,
                "the member has not been read yet, so its version is not known: its pages are \
                 read-only until it is"
                    .to_owned(),
            ));
        }
        if digest != own {
            return Some((
                READ_ONLY_VERSION,
                format!(
                    "the member's config schema ({}) is not this manager's ({}): it runs another \
                     version, or fit, whose pages this manager's do not know, so they are read-only",
                    short_digest(digest),
                    short_digest(own)
                ),
            ));
        }
        None
    }

    /// Runs `run` when `n` has a turn, at once or after the calls before it, and bounds the
    /// calls: `Err` with why when there are too many. A call that waited longer than the call
    /// timeout is not run: `stale` is called in its place. fit's `enqueue`.
    fn enqueue(
        &self,
        n: &Arc<Node>,
        run: Run,
        stale: Box<dyn FnOnce() + Send>,
    ) -> Result<(), String> {
        let key = Arc::as_ptr(n) as usize;
        let mut h = lock(&self.0.pass);
        if h.total >= MAX_PASS_CALLS {
            return Err("other calls to members are under way; try again".to_owned());
        }
        let q = h.queues.entry(key).or_default();
        if q.running >= MAX_PASS_PER_NODE && q.waiting.len() >= MAX_PASS_WAITING {
            return Err("other calls to the member are under way; try again".to_owned());
        }
        if q.running < MAX_PASS_PER_NODE {
            q.running = q.running.saturating_add(1);
            h.total = h.total.saturating_add(1);
            drop(h);
            run(self.releaser(key));
            return Ok(());
        }
        q.waiting.push_back(Wait {
            at: Instant::now(),
            run,
            stale,
        });
        h.total = h.total.saturating_add(1);
        Ok(())
    }

    fn releaser(&self, key: usize) -> Release {
        let me = self.clone();
        Box::new(move || me.next_turn(key))
    }

    /// Ends a turn of node `key`'s and gives it to the next call waiting.
    fn next_turn(&self, key: usize) {
        loop {
            let w = {
                let mut h = lock(&self.0.pass);
                h.total = h.total.saturating_sub(1);
                let Some(q) = h.queues.get_mut(&key) else {
                    return;
                };
                match q.waiting.pop_front() {
                    Some(w) => w,
                    None => {
                        q.running = q.running.saturating_sub(1);
                        if q.running == 0 {
                            h.queues.remove(&key);
                        }
                        return;
                    }
                }
            };
            if w.at.elapsed() <= self.limits().timeout {
                (w.run)(self.releaser(key));
                return;
            }
            // It waited too long: its turn goes on to the next.
            (w.stale)();
        }
    }

    /// Takes a place for a write's audit entry: false when [`MAX_PASS_AUDIT`] wait for theirs.
    fn hold_audit(&self) -> bool {
        self.0
            .pass_held
            .fetch_update(Ordering::AcqRel, Ordering::Acquire, |h| {
                (h < MAX_PASS_AUDIT).then_some(h.saturating_add(1))
            })
            .is_ok()
    }

    fn unhold_audit(&self) {
        let _ = self
            .0
            .pass_held
            .fetch_update(Ordering::AcqRel, Ordering::Acquire, |h| {
                Some(h.saturating_sub(1))
            });
    }

    /// Records a write passed on to member `name`: in the log at once, in the audit log on the
    /// pool. The body sent is not kept, as a config holds secrets: its digest is.
    #[allow(clippy::too_many_arguments)]
    fn audited(
        &self,
        user: &str,
        name: &str,
        method: &str,
        path: &str,
        body: &[u8],
        before: u64,
        rep: Option<&Reply>,
        prob: Option<&Problem>,
    ) {
        let mut after = String::new();
        let (outcome, result) = match (prob, rep) {
            (Some(p), None)
                if matches!(p.kind, KIND_AUTH | KIND_RATE_LIMITED | KIND_PASSWORD_CHANGE) =>
            {
                (format!("not sent ({}): {}", p.kind, p.message), "not_sent")
            }
            (Some(p), None) => (
                format!(
                    "not answered ({}), so it may or may not have been applied: {}",
                    p.kind, p.message
                ),
                "unknown",
            ),
            (Some(p), Some(r)) => (format!("{}, {}", r.status, p.kind), "failed"),
            (None, Some(r)) => {
                let mut o = r.status.to_string();
                let v = r
                    .header("Akit-Config-Version")
                    .or_else(|| r.header("Fit-Config-Version"))
                    .filter(|_| r.status < 300)
                    .map(str::to_owned);
                if let Some(v) = v {
                    o.push_str(&format!(", config version {v}"));
                    after = v;
                }
                (o, if r.status >= 300 { "failed" } else { "ok" })
            }
            (None, None) => ("not answered".to_owned(), "unknown"),
        };
        let detail = crate::api::clip(
            &format!("{method} {path}: {outcome}"),
            crate::node::MAX_MESSAGE,
        );
        self.log(format!(
            "cluster: {user:?}, through the node switcher, on the member {name:?}: {detail}"
        ));
        let s = |v: &str| akit_config::value::Value::String(v.to_owned());
        let mut e = vec![
            ("action".to_owned(), s(ACTION_API)),
            ("node".to_owned(), s(name)),
            ("detail".to_owned(), s(&detail)),
            ("outcome".to_owned(), s(result)),
        ];
        if before != 0 {
            e.push(("before".to_owned(), s(&before.to_string())));
        }
        if !after.is_empty() {
            e.push(("after".to_owned(), s(&after)));
        }
        if !body.is_empty() {
            e.push((
                "digest".to_owned(),
                s(&format!(
                    "sha256:{}",
                    akit_roles::linktoken::hex(&Sha256::digest(body))
                )),
            ));
        }
        let me = self.clone();
        self.record_then(user, e, Box::new(move || me.unhold_audit()));
    }
}

/// The member's answer, passed back: the headers the pages read (fit's under akit's names), its
/// type when a browser may render it, its body as it came. fit's `passAnswer`.
fn pass_answer(rep: &Reply) -> Response {
    let ct = rep
        .header("Content-Type")
        .unwrap_or("")
        .split(';')
        .next()
        .unwrap_or("")
        .trim()
        .to_ascii_lowercase();
    let ct = if ANSWER_TYPES.contains(&ct.as_str()) {
        rep.header("Content-Type").unwrap_or("").to_owned()
    } else {
        "application/octet-stream".to_owned()
    };
    let mut r = Response::with_body(rep.status, &ct, rep.body.clone());
    for (name, fit) in ANSWER_HEADERS {
        if let Some(v) = rep.header(name).or_else(|| rep.header(fit)) {
            r = r.header(name, v);
        }
    }
    if let Some(d) = rep
        .header("Content-Disposition")
        .filter(|d| d.starts_with("attachment") && d.len() <= 512)
    {
        r = r.header("Content-Disposition", d);
    }
    r
}

/// What an answer of the member means when it is no answer to pass on: 401, the credentials
/// refused (logged in again once already), and 403 asking for a temporary password to be changed;
/// a page would take either for the manager's own session. fit's `passProblem`.
fn pass_problem(n: &Node, r: &Reply, s: Option<&Arc<crate::node::Session>>) -> Option<Problem> {
    let body = json::parse(&r.body);
    match r.status {
        401 => Some(
            Problem::new(
                KIND_AUTH,
                format!(
                    "the member refused the manager's credentials: {}",
                    body.as_ref().map_or("", |b| json::str_of(b, "error"))
                ),
            )
            .with_status(401),
        ),
        403 if body
            .as_ref()
            .is_some_and(|b| json::bool_of(b, "must_change_password")) =>
        {
            n.drop_session(s);
            Some(
                Problem::new(
                    KIND_PASSWORD_CHANGE,
                    "the member's console asks for the user's temporary password to be changed; \
                     change it on the member, then on the Nodes page",
                )
                .with_status(403),
            )
        }
        _ => None,
    }
}

fn failure(n: &Node, mut p: Problem) -> Response {
    p.message = n.redact(&p.message);
    answer(
        502,
        Obj::new()
            .str("error", &p.message)
            .raw("problem", &p.json())
            .end(),
    )
}

/// Answers `/api/n/<member>/...`: the call passed on to the member. fit's `passOn`.
pub(crate) fn pass_on(m: &Manager, ctx: &mut Context, user: &str) {
    let full = ctx.path().to_owned();
    let Some(rest) = full.strip_prefix(PASS_PREFIX) else {
        // The module's own path, /api/m/cluster/n/...: only the console's /api/n/ passes on.
        return ctx.respond(akit_roles::pages::no_endpoint());
    };
    let (raw, sub) = rest.split_once('/').unwrap_or((rest, ""));
    let name = crate::api::percent_decode(raw);
    let rt = match route_of(sub) {
        Ok(r) => r,
        Err((st, msg)) => return ctx.respond(fail(st, &msg)),
    };
    let method = ctx.method().clone();
    let m_str = match method {
        Method::Get => "GET",
        Method::Head => "HEAD",
        Method::Post => "POST",
        Method::Put => "PUT",
        Method::Patch => "PATCH",
        Method::Delete => "DELETE",
        _ => {
            return ctx.respond(
                fail(405, "the method is not allowed")
                    .header("Allow", "GET, HEAD, POST, PUT, PATCH, DELETE"),
            );
        }
    };
    if rt.what == "workbench" && method != Method::Get {
        return ctx.respond(fail(405, "the method is not allowed").header("Allow", "GET"));
    }
    if ctx
        .header("Accept")
        .is_some_and(|a| a.contains("text/event-stream"))
    {
        return ctx.respond(fail(
            501,
            "streams are not passed on to members: open the member's own console for this page",
        ));
    }
    let max = m.limits().max_bytes;
    if ctx.body().len() > max {
        return ctx.respond(fail(
            413,
            &format!("a request passed on to a member is {max} bytes at most (max_response_bytes)"),
        ));
    }
    let Some(n) = m.node(&name) else {
        return ctx.respond(fail(
            404,
            &format!("no member is named {}", json::quoted(&name)),
        ));
    };
    // A member's pages are read by a viewer of it, and changed by an admin of it (I139).
    let role = if writes(&method, &rt, ctx.req().query().unwrap_or("")) {
        crate::roles::ADMIN
    } else {
        crate::roles::VIEWER
    };
    if let Err(e) = m.allow(user, &crate::roles::Need::on(role, vec![name.clone()])) {
        return ctx.respond(e.response());
    }
    if rt.what == "module" && n.spec.access != ACCESS_CONSOLE {
        return ctx.respond(fail(
            404,
            &format!(
                "the member's role pages are reached through its console: the manager reaches it \
                 by {}",
                n.spec.access
            ),
        ));
    }
    let st = n.state();
    let caps_known = st.node.is_some();
    // A member that serves no schema is shown with the manager's, and its pages are read-only.
    if rt.what == "admin"
        && rt.path == "/schema"
        && method == Method::Get
        && n.spec.access != ACCESS_CONSOLE
        && caps_known
        && !st.caps.iter().any(|c| c == "schema")
    {
        return ctx.respond(
            Response::with_body(200, "application/schema+json", m.0.schema.to_vec())
                .header("Akit-Schema-From", "manager"),
        );
    }
    let query = ctx.req().query().unwrap_or("").to_owned();
    let write = writes(&method, &rt, &query);
    if write {
        let ro = m.read_only();
        if !ro.is_empty() {
            return ctx.respond(fail(
                409,
                &format!(
                    "the manager's members cannot be written ({ro}), so a change through it could \
                     not be recorded"
                ),
            ));
        }
        if let Some((_, why)) = m.compatible(&st.digest) {
            return ctx.respond(fail(409, &why));
        }
    }
    let body = (!ctx.body().is_empty() || !matches!(method, Method::Get | Method::Head))
        .then(|| ctx.body().to_vec());
    let mut headers = Vec::new();
    for h in PASS_HEADERS {
        if let Some(v) = ctx.header(h) {
            headers.push(((*h).to_owned(), v.to_owned()));
            if *h == "Akit-Store-Password" {
                headers.push(("Fit-Store-Password".to_owned(), v.to_owned()));
            }
        }
    }
    let mut path = match rt.what {
        "node" => n.paths().node.to_owned(),
        "admin" if n.spec.access == ACCESS_CONSOLE => format!("/api/m/admin{}", rt.path),
        _ => rt.path.clone(),
    };
    if !query.is_empty() && rt.what != "workbench" && rt.what != "node" {
        path.push('?');
        path.push_str(&query);
    }
    if write && !m.hold_audit() {
        return ctx.respond(fail(
            429,
            "other changes to members are still being recorded; try again",
        ));
    }
    let Some(d) = ctx.detach() else {
        if write {
            m.unhold_audit();
        }
        return;
    };
    let cell = Arc::new(Mutex::new(Some(d)));
    let before = st.config_version;
    let (me, user, nn, cell2) = (
        m.clone(),
        user.to_owned(),
        Arc::clone(&n),
        Arc::clone(&cell),
    );
    let shown = rt.path.clone();
    let run: Run = Box::new(move |release: Release| {
        let Some(d) = lock(&cell2).take() else {
            return release();
        };
        if rt.what == "workbench" {
            return pass_workbench(&me, d, &nn, &user, release);
        }
        let (me2, nn2) = (me.clone(), Arc::clone(&nn));
        let sent = body.clone().unwrap_or_default();
        nn.call(
            m_str,
            &path,
            headers,
            body,
            Box::new(move |r| {
                let (rep, prob) = match r {
                    Ok((rep, s)) => {
                        let p = pass_problem(&nn2, &rep, s.as_ref());
                        (Some(rep), p)
                    }
                    Err(p) => (None, Some(p)),
                };
                if write {
                    me2.audited(
                        &user,
                        &nn2.spec.name,
                        m_str,
                        &shown,
                        &sent,
                        before,
                        rep.as_ref(),
                        prob.as_ref(),
                    );
                }
                d.respond(match (prob, rep) {
                    (Some(p), _) => failure(&nn2, p),
                    (None, Some(rep)) => pass_answer(&rep),
                    (None, None) => fail(502, "the member did not answer"),
                });
                release();
            }),
        );
    });
    let me3 = m.clone();
    let stale_cell = Arc::clone(&cell);
    let stale = Box::new(move || {
        if write {
            me3.unhold_audit();
        }
        if let Some(d) = lock(&stale_cell).take() {
            d.respond(fail(
                503,
                "the member is busy with other calls from the manager; try again",
            ));
        }
    });
    if let Err(e) = m.enqueue(&n, run, stale) {
        if write {
            m.unhold_audit();
        }
        if let Some(d) = lock(&cell).take() {
            d.respond(fail(429, &e));
        }
    }
}

/// Answers `GET /api/n/<member>/workbench`: the member's modules the switcher opens, and the
/// member as the pages show it, whose `/node` is read now. fit's `passWorkbench`.
fn pass_workbench(m: &Manager, d: Detached, n: &Arc<Node>, user: &str, release: Release) {
    let (me, nn, user) = (m.clone(), Arc::clone(n), user.to_owned());
    n.get(
        n.paths().node,
        Box::new(move |r| {
            let body = match r {
                Err(p) => {
                    d.respond(failure(&nn, p));
                    return release();
                }
                Ok((b, _)) => b,
            };
            let Some(v) = json::parse(&body) else {
                d.respond(failure(
                    &nn,
                    Problem::new(KIND_BAD_ANSWER, "the member's /node is not the JSON expected"),
                ));
                return release();
            };
            let digest = json::str_of(&v, "schema_digest").to_owned();
            let polled = nn.state().digest;
            if polled != digest {
                // The member changed since the poll: read it now, which the writes go by.
                nn.next.store(0, Ordering::Release);
                me.poke();
            }
            let mut ro = me.compatible(&digest).or_else(|| me.compatible(&polled));
            let manager_ro = me.read_only();
            if ro.is_none() && !manager_ro.is_empty() {
                ro = Some((
                    READ_ONLY_MANAGER,
                    format!(
                        "the manager's members cannot be written ({manager_ro}), so a change through \
                         it could not be recorded"
                    ),
                ));
            }
            let info = json::parse(crate::node::node_info_json(&v).as_bytes())
                .unwrap_or(akit_config::value::Value::Null);
            let member = Obj::new()
                .str("name", &nn.spec.name)
                .str("access", &nn.spec.access)
                .str_omit("url", &nn.spec.url)
                .str("id", json::str_of(&v, "id"))
                .str("hostname", json::str_of(&v, "hostname"))
                .str("akit_version", json::str_of(&info, "akit_version"))
                .str("schema_digest", &digest)
                .raw("roles", &json::string_list(&json::strings_of(&v, "roles")))
                .str("manager_schema_digest", &me.0.schema_digest)
                .bool("read_only", ro.is_some())
                .str_omit("reason_kind", ro.as_ref().map_or("", |r| r.0))
                .str_omit("reason", ro.as_ref().map_or("", |r| r.1.as_str()))
                .end();
            let wb = move |mods: &[String]| {
                answer(
                    200,
                    Obj::new()
                        .str("user", &user)
                        .raw("modules", &json::string_list(mods))
                        .raw("member", &member)
                        .end(),
                )
            };
            if nn.spec.access != ACCESS_CONSOLE {
                d.respond(wb(&["admin".to_owned()]));
                return release();
            }
            let nn2 = Arc::clone(&nn);
            nn.get(
                "/api/workbench",
                Box::new(move |r| {
                    d.respond(match r {
                        Err(p) => failure(&nn2, p),
                        Ok((b, _)) => {
                            let mods: Vec<String> = json::parse(&b)
                                .map(|v| json::strings_of(&v, "modules"))
                                .unwrap_or_default()
                                .into_iter()
                                .filter(|id| !not_passed(id))
                                .take(64)
                                .collect();
                            wb(&mods)
                        }
                    });
                    release();
                }),
            );
        }),
    );
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]
    use super::*;

    #[test]
    fn routes() {
        // fit's TestRouteOf.
        assert_eq!(route_of("workbench").unwrap().what, "workbench");
        assert_eq!(route_of("node").unwrap().path, "/node");
        let r = route_of("m/admin/config/routes/0").unwrap();
        assert_eq!((r.what, r.path.as_str()), ("admin", "/config/routes/0"));
        let r = route_of("m/tunnel.hub/api/status").unwrap();
        assert_eq!(
            (r.what, r.path.as_str()),
            ("module", "/api/m/tunnel.hub/api/status")
        );
        for (sub, status) in [
            ("m/admin/users", 403),
            ("m/admin/account/password", 403),
            ("m/admin/debug/pprof", 404),
            ("m/admin/dashboard/", 404),
            ("m/cluster/api/nodes", 403),
            ("m/ai/x", 403),
            ("m/llm.hub/x", 403),
            ("m/admin/../x", 404),
            ("m/admin/%2e%2e/x", 404),
            ("m/admin", 404),
            ("other", 404),
            ("m/admin/a//b", 404),
        ] {
            assert_eq!(route_of(sub).unwrap_err().0, status, "{sub}");
        }
        let admin = route_of("m/admin/config").unwrap();
        assert!(!writes(&Method::Get, &admin, ""));
        assert!(writes(&Method::Put, &admin, ""));
        assert!(!writes(&Method::Put, &admin, "dry_run=1"));
        assert!(!writes(
            &Method::Post,
            &route_of("m/admin/config/render").unwrap(),
            ""
        ));
        assert!(writes(
            &Method::Post,
            &route_of("m/tunnel.hub/api/x").unwrap(),
            "dry_run=1"
        ));
    }

    #[test]
    fn fit_headers_answered_under_akit_names() {
        let rep = Reply {
            status: 200,
            headers: vec![
                ("Content-Type".into(), "text/html".into()),
                ("Fit-Config-Version".into(), "7".into()),
                ("Fit-Dry-Run".into(), "1".into()),
                ("Set-Cookie".into(), "x=y".into()),
                ("Etag".into(), "\"e\"".into()),
            ],
            body: b"<b>".to_vec(),
            took: std::time::Duration::ZERO,
        };
        let r = pass_answer(&rep);
        let h = |n: &str| r.headers.get(n).map(str::to_owned);
        assert_eq!(h("Akit-Config-Version").as_deref(), Some("7"));
        assert_eq!(h("Akit-Dry-Run").as_deref(), Some("1"));
        assert_eq!(h("ETag").as_deref(), Some("\"e\""));
        assert!(h("Set-Cookie").is_none());
        assert_eq!(
            h("Content-Type").as_deref(),
            Some("application/octet-stream")
        );
    }
}
