//! A member as the poller reaches it, fit's `cluster/poll.go`: its transport, its console session
//! (logged in again on 401, a refused login waited out with a backoff, a member's rate limit
//! respected), its state, and the poll: `GET /node`, then `/status` and `/config/check` when the
//! member lists them, each started from the answer of the one before, never waiting.

use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::{Duration, Instant, SystemTime};

use akit_config::value::Value;

use crate::json::{self, Obj};
use crate::member::{ACCESS_ADMIN_API, ACCESS_CONSOLE, ACCESS_LINK, Member};
use crate::transport::{
    CLOSED, CertInfo, KIND_AUTH, KIND_BAD_ANSWER, KIND_FORBIDDEN, KIND_NOT_LINKED,
    KIND_PASSWORD_CHANGE, KIND_RATE_LIMITED, KIND_STATUS, KIND_TIMEOUT, KIND_UNREACHABLE,
    KIND_UNSUPPORTED, Problem, Reply, Transport,
};

/// A member's state, `State.status`: not polled yet.
pub const STATUS_UNKNOWN: &str = "unknown";
/// The last poll read the member.
pub const STATUS_UP: &str = "up";
/// The last poll did not reach the member, or it did not answer in time.
pub const STATUS_DOWN: &str = "down";
/// The last poll reached the member and failed otherwise.
pub const STATUS_ERROR: &str = "error";

/// The capabilities of a member's `GET /node` the poller looks for.
pub const CAP_STATUS: &str = "status";
/// See [`CAP_STATUS`].
pub const CAP_CHECK: &str = "config.check";

/// The bounds of the login backoff of a member whose credentials were refused.
pub const MIN_LOGIN_BACKOFF: Duration = Duration::from_secs(60);
/// See [`MIN_LOGIN_BACKOFF`].
pub const MAX_LOGIN_BACKOFF: Duration = Duration::from_secs(15 * 60);
/// The longest problem's message, much of which may come from the member.
pub const MAX_MESSAGE: usize = 512;

/// The paths the poller reads, through a member's console or its Admin API.
#[derive(Debug, Clone, Copy)]
pub struct Paths {
    /// The console's login.
    pub login: &'static str,
    /// The console's logout.
    pub logout: &'static str,
    /// `GET /node`.
    pub node: &'static str,
    /// `GET /status`.
    pub status: &'static str,
    /// `GET /config/check`.
    pub check: &'static str,
    /// What the Admin API's paths take before them.
    pub prefix: &'static str,
}

const CONSOLE_PATHS: Paths = Paths {
    login: "/api/login",
    logout: "/api/logout",
    node: "/api/node",
    status: "/api/m/admin/status",
    check: "/api/m/admin/config/check",
    prefix: "/api/m/admin",
};
const ADMIN_PATHS: Paths = Paths {
    login: "",
    logout: "",
    node: "/node",
    status: "/status",
    check: "/config/check",
    prefix: "",
};

/// A console's login: its cookies and CSRF token.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Session {
    cookies: String,
    csrf: String,
}

/// What a node reports to the manager: a member's state changing, for the log.
pub type Log = Arc<dyn Fn(String) + Send + Sync>;

/// What a login's waiters hear: why there is no session, or `None`.
pub type LoginDone = Box<dyn FnOnce(Option<Problem>) + Send>;

/// A read's answer: its body and how long it took, or why there is none.
pub type GetDone = Box<dyn FnOnce(Result<(Vec<u8>, Duration), Problem>) + Send>;

/// A call's answer and the session it was sent with.
pub type CallDone = Box<dyn FnOnce(Result<(Reply, Option<Arc<Session>>), Problem>) + Send>;

/// What the manager knows of a member: how its last poll went, and what the last poll that read
/// it found, kept while it is down. fit's `State`, without what the document says of the member.
#[derive(Debug, Clone)]
pub struct State {
    /// [`STATUS_UP`], [`STATUS_DOWN`], [`STATUS_ERROR`] or [`STATUS_UNKNOWN`].
    pub status: &'static str,
    /// When it became that.
    pub since: SystemTime,
    /// Why the last poll failed.
    pub error: Option<Problem>,
    /// Whether the last poll reached the member at all.
    pub reachable: bool,
    /// How long its `GET /node` took at the last poll that read it.
    pub latency_ms: u64,
    /// When the last poll ended, and when the last that succeeded did.
    pub polled: Option<SystemTime>,
    /// See [`polled`](State::polled).
    pub seen: Option<SystemTime>,
    /// How many polls in a row failed.
    pub failures: u64,
    /// A member whose console asks for its temporary password to be changed.
    pub must_change_password: bool,
    /// The member's `GET /node`, as fit's `NodeInfo`.
    pub node: Option<String>,
    /// Its capabilities.
    pub caps: Vec<String>,
    /// Its running config, its reloads, connections, upstreams, certificates and drift, each as
    /// fit writes them.
    pub config: Option<String>,
    /// See [`config`](State::config).
    pub reloads: Option<String>,
    /// See [`config`](State::config).
    pub connections: Option<String>,
    /// See [`config`](State::config).
    pub upstreams: Option<String>,
    /// See [`config`](State::config).
    pub certificates: Option<String>,
    /// See [`config`](State::config).
    pub drift: Option<String>,
    /// The certificate the member presented at the latest probe.
    pub tls: Option<CertInfo>,
}

impl Default for State {
    fn default() -> State {
        State::new()
    }
}

impl State {
    /// A member not polled yet.
    #[must_use]
    pub fn new() -> State {
        State {
            status: STATUS_UNKNOWN,
            since: SystemTime::now(),
            error: None,
            reachable: false,
            latency_ms: 0,
            polled: None,
            seen: None,
            failures: 0,
            must_change_password: false,
            node: None,
            caps: Vec::new(),
            config: None,
            reloads: None,
            connections: None,
            upstreams: None,
            certificates: None,
            drift: None,
            tls: None,
        }
    }

    /// The state as the API answers it, with what the document says of `m` and its `groups`.
    #[must_use]
    pub fn json(&self, m: &Member, groups: &[String]) -> String {
        let v = m.view();
        let labels = json::parse(v.as_bytes())
            .and_then(|x| x.get("labels").map(json::compact))
            .unwrap_or_else(|| "{}".to_owned());
        let mut o = Obj::new()
            .str("name", &m.name)
            .raw("labels", &labels)
            .raw("groups", &json::string_list(groups))
            .str("url", &m.url)
            .str("access", &m.access)
            .str("status", self.status)
            .raw("since", &json::time(self.since));
        if let Some(e) = &self.error {
            o = o.raw("error", &e.json());
        }
        o = o
            .bool("reachable", self.reachable)
            .num("latency_ms", self.latency_ms);
        if let Some(t) = self.polled {
            o = o.raw("polled", &json::time(t));
        }
        if let Some(t) = self.seen {
            o = o.raw("seen", &json::time(t));
        }
        o = o.num("failures", self.failures);
        if self.must_change_password {
            o = o.bool("must_change_password", true);
        }
        for (k, v) in [
            ("node", &self.node),
            ("config", &self.config),
            ("reloads", &self.reloads),
            ("connections", &self.connections),
            ("upstreams", &self.upstreams),
            ("certificates", &self.certificates),
            ("drift", &self.drift),
        ] {
            o = o.opt(k, v.as_deref());
        }
        if let Some(c) = &self.tls {
            o = o.raw("tls", &c.json());
        }
        o.end()
    }
}

struct Mu {
    session: Option<Arc<Session>>,
    login_after: Option<Instant>,
    login_problem: Option<Problem>,
    backoff: Duration,
    state: State,
    logins: Option<Vec<LoginDone>>,
    probed: bool,
}

/// A member as the poller reaches it.
pub struct Node {
    /// The member as the node was made for it, never changed.
    pub spec: Member,
    t: Option<Arc<Transport>>,
    paths: Paths,
    interval: Duration,
    log: Log,
    /// A poll under way.
    pub busy: AtomicBool,
    /// When the next poll is due, in milliseconds since the manager started.
    pub next: AtomicU64,
    /// A member removed or changed.
    pub closed: AtomicBool,
    mu: Mutex<Mu>,
}

impl std::fmt::Debug for Node {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Node")
            .field("spec", &self.spec)
            .finish_non_exhaustive()
    }
}

fn lock<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap_or_else(PoisonError::into_inner)
}

impl Node {
    /// A node for `spec`, reached through `t` (`None` for a member that links out, which akit
    /// reaches from I139b).
    #[must_use]
    pub fn new(spec: Member, t: Option<Arc<Transport>>, interval: Duration, log: Log) -> Node {
        let paths = if spec.access == ACCESS_CONSOLE {
            CONSOLE_PATHS
        } else {
            ADMIN_PATHS
        };
        Node {
            spec,
            t,
            paths,
            interval,
            log,
            busy: AtomicBool::new(false),
            next: AtomicU64::new(0),
            closed: AtomicBool::new(false),
            mu: Mutex::new(Mu {
                session: None,
                login_after: None,
                login_problem: None,
                backoff: Duration::ZERO,
                state: State::new(),
                logins: None,
                probed: false,
            }),
        }
    }

    /// The paths the member is read at.
    #[must_use]
    pub fn paths(&self) -> Paths {
        self.paths
    }

    /// A copy of the node's state.
    #[must_use]
    pub fn state(&self) -> State {
        lock(&self.mu).state.clone()
    }

    /// The capabilities the last poll that read the member found, `None` before one.
    #[must_use]
    pub fn caps(&self) -> Option<Vec<String>> {
        let mu = lock(&self.mu);
        mu.state.node.as_ref().map(|_| mu.state.caps.clone())
    }

    /// Takes the member's credentials out of `msg`, and bounds it.
    #[must_use]
    pub fn redact(&self, msg: &str) -> String {
        let mut out = msg.to_owned();
        for secret in [&self.spec.password, &self.spec.token] {
            if secret.len() >= 4 {
                out = out.replace(secret.as_str(), "••••");
            }
        }
        if out.len() > MAX_MESSAGE {
            let mut cut = MAX_MESSAGE;
            while !out.is_char_boundary(cut) {
                cut = cut.saturating_sub(1);
            }
            out.truncate(cut);
            out.push('…');
        }
        out
    }

    fn current_session(&self) -> Option<Arc<Session>> {
        lock(&self.mu).session.clone()
    }

    /// A call's credentials: the session's cookies, with its CSRF token in both akit's and fit's
    /// header for what is not a read (decision 041, 3), or the bearer token.
    fn credentials(&self, s: Option<&Session>, csrf: bool) -> Vec<(String, String)> {
        let mut h = Vec::new();
        if self.spec.access == ACCESS_ADMIN_API {
            h.push((
                "Authorization".to_owned(),
                format!("Bearer {}", self.spec.token),
            ));
        } else if let Some(s) = s {
            h.push(("Cookie".to_owned(), s.cookies.clone()));
            if csrf {
                h.push(("X-Akit-Csrf".to_owned(), s.csrf.clone()));
                h.push(("X-Fit-Csrf".to_owned(), s.csrf.clone()));
            }
        }
        h
    }

    /// Sends `method path` to the member with its credentials, and calls `done` once with the
    /// answer, whatever its status, and the session it was sent with; or with why there is none.
    /// A member reached by its console is logged in first when the node has no session, and
    /// again when a call answers 401, once a call. `headers`' credentials, Host and the
    /// hop-by-hop fields are not sent: the member's own are. fit's `node.call`.
    pub fn call(
        self: &Arc<Self>,
        method: &str,
        path: &str,
        headers: Vec<(String, String)>,
        body: Option<Vec<u8>>,
        done: CallDone,
    ) {
        if self.spec.access == ACCESS_CONSOLE && self.current_session().is_none() {
            let me = Arc::clone(self);
            let (method, path) = (method.to_owned(), path.to_owned());
            return self.login(Box::new(move |prob| match prob {
                Some(p) => done(Err(p)),
                None => me.send(&method, &path, headers, body, true, done),
            }));
        }
        self.send(method, path, headers, body, false, done);
    }

    fn send(
        self: &Arc<Self>,
        method: &str,
        path: &str,
        headers: Vec<(String, String)>,
        body: Option<Vec<u8>>,
        relogged: bool,
        done: CallDone,
    ) {
        let Some(t) = self.t.clone() else {
            return done(Err(Problem::new(
                KIND_NOT_LINKED,
                "the member links out to the manager, which akit reaches from I139b",
            )));
        };
        if self.closed.load(Ordering::Acquire) {
            return done(Err(Problem::new(KIND_UNREACHABLE, CLOSED)));
        }
        let s = self.current_session();
        let mut h = self.credentials(s.as_deref(), method != "GET" && method != "HEAD");
        for (n, v) in &headers {
            if !DROP_HEADERS.iter().any(|d| d.eq_ignore_ascii_case(n)) {
                h.push((n.clone(), v.clone()));
            }
        }
        let me = Arc::clone(self);
        let (m2, p2) = (method.to_owned(), path.to_owned());
        let resend_body = body.clone();
        t.call(
            method,
            path,
            &h,
            body,
            Box::new(move |r| match r {
                Err(p) => done(Err(p)),
                Ok(reply)
                    if reply.status == 401 && me.spec.access == ACCESS_CONSOLE && !relogged =>
                {
                    // The session ended: log in again and send it once more.
                    me.drop_session(s.as_ref());
                    let me2 = Arc::clone(&me);
                    me.login(Box::new(move |prob| match prob {
                        Some(p) => done(Err(p)),
                        None => me2.send(&m2, &p2, headers, resend_body, true, done),
                    }));
                }
                Ok(reply) => done(Ok((reply, s))),
            }),
        );
    }

    /// What a reply other than 200 means, or `None` for 200: fit's `answerProblem`. A console
    /// asking for its password to be changed ends the session.
    #[must_use]
    pub fn answer_problem(
        &self,
        r: &Reply,
        path: &str,
        s: Option<&Arc<Session>>,
    ) -> Option<Problem> {
        if r.status == 200 {
            return None;
        }
        let body = json::parse(&r.body);
        let mut msg = body
            .as_ref()
            .map(|b| json::str_of(b, "error").to_owned())
            .unwrap_or_default();
        if msg.is_empty() {
            msg = status_text(r.status).to_owned();
        }
        let must_change = body
            .as_ref()
            .is_some_and(|b| json::bool_of(b, "must_change_password"));
        let mut p = Problem::new(KIND_STATUS, self.redact(&format!("GET {path}: {msg}")))
            .with_status(r.status);
        match r.status {
            401 => {
                p.kind = KIND_AUTH;
                if self.spec.access == ACCESS_ADMIN_API {
                    p.message = format!("the Admin API refused the token: {}", self.redact(&msg));
                }
            }
            403 if must_change => {
                p.kind = KIND_PASSWORD_CHANGE;
                p.message = PASSWORD_CHANGE.to_owned();
                self.drop_session(s);
            }
            403 => p.kind = KIND_FORBIDDEN,
            404 if path == self.paths.node => {
                p.kind = KIND_UNSUPPORTED;
                p.message = format!(
                    "the member serves no {path}: it is not an akit or a fit, or one older than \
                     cluster management"
                );
            }
            _ => {}
        }
        Some(p)
    }

    /// Reads `path` as JSON with the node's session, logging in first when there is none: the
    /// body and how long it took, or why not.
    pub fn get(self: &Arc<Self>, path: &str, done: GetDone) {
        let me = Arc::clone(self);
        let p = path.to_owned();
        self.call(
            "GET",
            path,
            Vec::new(),
            None,
            Box::new(move |r| match r {
                Err(prob) => done(Err(prob)),
                Ok((reply, s)) => {
                    if let Some(prob) = me.answer_problem(&reply, &p, s.as_ref()) {
                        return done(Err(prob));
                    }
                    if json::parse(&reply.body).is_none() {
                        return done(Err(Problem::new(
                            KIND_BAD_ANSWER,
                            format!("the member's {p} is not the JSON expected"),
                        )
                        .with_status(reply.status)));
                    }
                    done(Ok((reply.body, reply.took)));
                }
            }),
        );
    }

    /// Polls the member once, and calls `done` when it is over, whatever came of it.
    pub fn poll(self: &Arc<Self>, done: Box<dyn FnOnce() + Send>) {
        // The certificate is read by a probe at the first poll (A277's stopgap).
        let probe = {
            let mut mu = lock(&self.mu);
            let first = !mu.probed;
            mu.probed = true;
            first
        };
        if let (true, Some(t)) = (probe, &self.t) {
            if t.tls() {
                t.probe(Box::new(|_| {}));
            }
        }
        let me = Arc::clone(self);
        let path = self.paths.node;
        self.get(
            path,
            Box::new(move |r| match r {
                Err(p) => me.finish(Some(p), None, None, None, done),
                Ok((body, took)) => me.read_rest(body, took, done),
            }),
        );
    }

    fn read_rest(self: &Arc<Self>, node: Vec<u8>, took: Duration, done: Box<dyn FnOnce() + Send>) {
        let caps = json::parse(&node)
            .map(|v| json::strings_of(&v, "capabilities"))
            .unwrap_or_default();
        if !caps.iter().any(|c| c == CAP_STATUS) {
            return self.read_check(node, took, None, &caps, done);
        }
        let me = Arc::clone(self);
        let path = self.paths.status;
        self.get(
            path,
            Box::new(move |r| match r {
                Err(p) => me.finish(Some(p), None, None, None, done),
                Ok((st, _)) => me.read_check(node, took, Some(st), &caps, done),
            }),
        );
    }

    fn read_check(
        self: &Arc<Self>,
        node: Vec<u8>,
        took: Duration,
        status: Option<Vec<u8>>,
        caps: &[String],
        done: Box<dyn FnOnce() + Send>,
    ) {
        if !caps.iter().any(|c| c == CAP_CHECK) {
            return self.finish(None, Some((node, took)), status, None, done);
        }
        let me = Arc::clone(self);
        let path = self.paths.check;
        self.get(
            path,
            Box::new(move |r| match r {
                Err(p) => me.finish(Some(p), None, None, None, done),
                Ok((ck, _)) => me.finish(None, Some((node, took)), status, Some(ck), done),
            }),
        );
    }

    /// Records how the poll went, and ends it: fit's `poller.finish`.
    fn finish(
        &self,
        prob: Option<Problem>,
        node: Option<(Vec<u8>, Duration)>,
        status: Option<Vec<u8>>,
        check: Option<Vec<u8>>,
        done: Box<dyn FnOnce() + Send>,
    ) {
        if self.closed.load(Ordering::Acquire) {
            return done();
        }
        let now = SystemTime::now();
        let cert = self.t.as_ref().and_then(|t| t.certificate());
        let (changed, status_now, why) = {
            let mut mu = lock(&self.mu);
            let st = &mut mu.state;
            let old = st.status;
            st.polled = Some(now);
            if cert.is_some() {
                st.tls = cert;
            }
            let why = match prob {
                Some(mut p) => {
                    p.message = self.redact(&p.message);
                    st.reachable =
                        !matches!(p.kind, KIND_UNREACHABLE | KIND_TIMEOUT | KIND_NOT_LINKED);
                    st.status = if st.reachable {
                        STATUS_ERROR
                    } else {
                        STATUS_DOWN
                    };
                    st.failures = st.failures.saturating_add(1);
                    st.must_change_password = p.kind == KIND_PASSWORD_CHANGE;
                    let w = format!("{}: {}", p.kind, p.message);
                    st.error = Some(p);
                    Some(w)
                }
                None => {
                    if let Some((n, took)) = &node {
                        fill(st, n, *took, status.as_deref(), check.as_deref());
                    }
                    st.status = STATUS_UP;
                    st.error = None;
                    st.failures = 0;
                    st.reachable = true;
                    st.seen = Some(now);
                    st.must_change_password = false;
                    None
                }
            };
            let changed = st.status != old;
            if changed {
                st.since = now;
            }
            (changed, st.status, why)
        };
        if changed {
            let name = &self.spec.name;
            (self.log)(match why {
                Some(w) => format!("cluster: member {name:?} is {status_now} ({w})"),
                None => format!("cluster: member {name:?} is {status_now}"),
            });
        }
        done();
    }

    /// Logs in to the member's console, keeping the session, and calls `then` with `None`, or
    /// with why it could not. A login refused is not tried again before its backoff ends, nor
    /// one the member rate-limits before the wait it asks for; a read and a poll arriving
    /// together share one login. fit's `node.login`.
    pub fn login(self: &Arc<Self>, then: Box<dyn FnOnce(Option<Problem>) + Send>) {
        let Some(t) = self.t.clone() else {
            return then(Some(Problem::new(
                KIND_NOT_LINKED,
                "the member is not reached by its console",
            )));
        };
        if self.closed.load(Ordering::Acquire) {
            return then(Some(Problem::new(KIND_UNREACHABLE, CLOSED)));
        }
        {
            let mut mu = lock(&self.mu);
            if let (Some(after), Some(p)) = (mu.login_after, &mu.login_problem) {
                if Instant::now() < after {
                    let p = p.clone();
                    drop(mu);
                    return then(Some(p));
                }
            }
            if let Some(waiting) = &mut mu.logins {
                waiting.push(then);
                return;
            }
            mu.logins = Some(vec![then]);
        }
        let body = Obj::new()
            .str("username", &self.spec.username)
            .str("password", &self.spec.password)
            .end();
        let me = Arc::clone(self);
        t.call(
            "POST",
            self.paths.login,
            &[],
            Some(body.into_bytes()),
            Box::new(move |r| {
                let prob = match r {
                    Err(p) => Some(p),
                    Ok(reply) => {
                        let (prob, temp) = me.login_result(&reply);
                        if let Some(s) = temp {
                            // Logged in with a temporary password: the session can do nothing
                            // but change it, which the manager does not.
                            me.logout(&s, None);
                        }
                        prob
                    }
                };
                let waiting = lock(&me.mu).logins.take().unwrap_or_default();
                for f in waiting {
                    f(prob.clone());
                }
            }),
        );
    }

    /// Reads a login's answer: why there is no session, or `None`; and a session whose password
    /// must change. fit's `loginResult`.
    fn login_result(&self, r: &Reply) -> (Option<Problem>, Option<Session>) {
        let now = Instant::now();
        let mut mu = lock(&self.mu);
        let backoff = |mu: &mut Mu| {
            mu.backoff = mu
                .backoff
                .saturating_mul(2)
                .max(MIN_LOGIN_BACKOFF)
                .max(self.interval)
                .min(MAX_LOGIN_BACKOFF);
            mu.backoff
        };
        let refuse = |mu: &mut Mu, p: Problem, wait: Duration| {
            mu.login_after = now.checked_add(wait);
            mu.login_problem = Some(p.clone());
            Some(p)
        };
        let body = json::parse(&r.body);
        match r.status {
            200 => {
                let cookies = cookie_header(r);
                let csrf = body
                    .as_ref()
                    .map(|b| json::str_of(b, "csrf").to_owned())
                    .unwrap_or_default();
                if csrf.is_empty() || cookies.is_empty() {
                    return (
                        Some(
                            Problem::new(
                                KIND_BAD_ANSWER,
                                "the member's login answered no session: it is not an akit's or a \
                                 fit's console",
                            )
                            .with_status(200),
                        ),
                        None,
                    );
                }
                let s = Session { cookies, csrf };
                if body
                    .as_ref()
                    .is_some_and(|b| json::bool_of(b, "must_change_password"))
                {
                    let wait = backoff(&mut mu);
                    let p = Problem::new(KIND_PASSWORD_CHANGE, PASSWORD_CHANGE).with_status(200);
                    return (refuse(&mut mu, p, wait), Some(s));
                }
                mu.login_after = None;
                mu.login_problem = None;
                mu.backoff = Duration::ZERO;
                mu.session = Some(Arc::new(s));
                (None, None)
            }
            401 => {
                let wait = backoff(&mut mu);
                let expired = body
                    .as_ref()
                    .is_some_and(|b| json::bool_of(b, "password_expired"));
                let msg = if expired {
                    "the member's console refused the user's temporary password, which expired \
                     without being changed"
                } else {
                    "the member's console refused the user name or password"
                };
                let p = Problem::new(
                    KIND_AUTH,
                    format!(
                        "{msg}; not tried again for {} unless the member is changed or tested",
                        akit_config::duration::format(wait)
                    ),
                )
                .with_status(401);
                (refuse(&mut mu, p, wait), None)
            }
            429 => {
                let secs = r
                    .header("Retry-After")
                    .and_then(|v| v.trim().parse::<u64>().ok())
                    .unwrap_or(1)
                    .clamp(1, MAX_LOGIN_BACKOFF.as_secs());
                let mut why = body
                    .as_ref()
                    .map(|b| json::str_of(b, "error").to_owned())
                    .unwrap_or_default();
                if why.is_empty() {
                    why = "too many logins".to_owned();
                }
                let mut p = Problem::new(
                    KIND_RATE_LIMITED,
                    self.redact(&format!(
                        "the member's console refuses logins for {secs}s: {why}"
                    )),
                )
                .with_status(429);
                p.retry_after = secs;
                (refuse(&mut mu, p, Duration::from_secs(secs)), None)
            }
            404 => (
                Some(
                    Problem::new(
                        KIND_UNSUPPORTED,
                        format!("the member serves no console at {}", self.paths.login),
                    )
                    .with_status(404),
                ),
                None,
            ),
            st => {
                let why = body
                    .as_ref()
                    .map(|b| json::str_of(b, "error").to_owned())
                    .unwrap_or_default();
                (
                    Some(
                        Problem::new(
                            KIND_STATUS,
                            self.redact(&format!("the member's login answered {st} {why}")),
                        )
                        .with_status(st),
                    ),
                    None,
                )
            }
        }
    }

    /// Forgets `s`, the session a call was made with, when it is still the node's.
    pub fn drop_session(&self, s: Option<&Arc<Session>>) {
        let mut mu = lock(&self.mu);
        if let (Some(cur), Some(s)) = (&mu.session, s) {
            if Arc::ptr_eq(cur, s) {
                mu.session = None;
            }
        }
    }

    /// Lets the next poll log in at once: the member was tested and its credentials worked.
    pub fn reset_login(&self) {
        let mut mu = lock(&self.mu);
        mu.login_after = None;
        mu.login_problem = None;
        mu.backoff = Duration::ZERO;
    }

    /// Ends `s` on the member, so that it does not count against its sessions; `done` is called
    /// once it is over.
    pub fn logout(&self, s: &Session, done: Option<Box<dyn FnOnce() + Send>>) {
        let Some(t) = &self.t else {
            if let Some(d) = done {
                d();
            }
            return;
        };
        let h = self.credentials(Some(s), true);
        t.call(
            "POST",
            self.paths.logout,
            &h,
            Some(b"{}".to_vec()),
            Box::new(move |_| {
                if let Some(d) = done {
                    d();
                }
            }),
        );
    }

    /// Ends the node: its calls abandoned, its session logged out, and its connections closed;
    /// `done` is called once that is over.
    pub fn shutdown(&self, done: Box<dyn FnOnce() + Send>) {
        self.closed.store(true, Ordering::Release);
        let s = lock(&self.mu).session.take();
        let Some(t) = self.t.clone() else {
            return done();
        };
        t.abandon();
        match s {
            None => {
                t.close();
                done();
            }
            Some(s) => {
                let t2 = Arc::clone(&t);
                // The logout is the last call; the transport refuses calls once closed, so the
                // logout goes first.
                let h = self.credentials(Some(&s), true);
                t.call(
                    "POST",
                    self.paths.logout,
                    &h,
                    Some(b"{}".to_vec()),
                    Box::new(move |_| {
                        t2.close();
                        done();
                    }),
                );
            }
        }
    }

    /// The transport, for a test's probe.
    #[must_use]
    pub fn transport(&self) -> Option<&Arc<Transport>> {
        self.t.as_ref()
    }

    /// Whether it is a member that links out.
    #[must_use]
    pub fn linked(&self) -> bool {
        self.spec.access == ACCESS_LINK
    }
}

/// What a caller's header never sends to a member: fit's `dropHeaders`, and akit's CSRF header.
const DROP_HEADERS: &[&str] = &[
    "Authorization",
    "Cookie",
    "X-Fit-Csrf",
    "X-Akit-Csrf",
    "Host",
    "Content-Length",
    "Connection",
    "Transfer-Encoding",
    "Upgrade",
];

const PASSWORD_CHANGE: &str = "the member's console asks for the user's temporary password to be \
     changed; change it on the member, then here";

fn status_text(s: u16) -> &'static str {
    match s {
        400 => "Bad Request",
        401 => "Unauthorized",
        403 => "Forbidden",
        404 => "Not Found",
        405 => "Method Not Allowed",
        409 => "Conflict",
        412 => "Precondition Failed",
        413 => "Request Entity Too Large",
        429 => "Too Many Requests",
        500 => "Internal Server Error",
        502 => "Bad Gateway",
        503 => "Service Unavailable",
        _ => "",
    }
}

/// The cookies a login set, as a `Cookie` header sends them back: those with a value and not
/// ended (`Max-Age=0`).
fn cookie_header(r: &Reply) -> String {
    let mut parts = Vec::new();
    for (n, v) in &r.headers {
        if !n.eq_ignore_ascii_case("Set-Cookie") {
            continue;
        }
        let mut attrs = v.split(';');
        let Some((name, value)) = attrs.next().and_then(|nv| nv.trim().split_once('=')) else {
            continue;
        };
        let ended = attrs.any(|a| {
            let a = a.trim();
            a.len() > 8
                && a.get(..8)
                    .is_some_and(|k| k.eq_ignore_ascii_case("max-age="))
                && a.get(8..)
                    .is_some_and(|n| n.trim().starts_with('-') || n.trim() == "0")
        });
        if !value.is_empty() && !ended {
            parts.push(format!("{}={}", name.trim(), value.trim()));
        }
    }
    parts.join("; ")
}

/// Sets `st` from what the poll read: fit's `poller.fill`.
fn fill(st: &mut State, node: &[u8], took: Duration, status: Option<&[u8]>, check: Option<&[u8]>) {
    let n = json::parse(node).unwrap_or(Value::Null);
    st.caps = json::strings_of(&n, "capabilities");
    st.node = Some(node_info_json(&n));
    st.config = Some(
        Obj::new()
            .num("version", json::uint_of(&n, "config_version"))
            .str("etag", json::str_of(&n, "config_etag"))
            .raw(
                "applied",
                &raw_or(&n, "config_applied", "\"0001-01-01T00:00:00Z\""),
            )
            .str("source", json::str_of(&n, "config_source"))
            .end(),
    );
    st.latency_ms = json::millis(took);
    st.reloads = None;
    st.connections = None;
    st.upstreams = None;
    st.certificates = None;
    st.drift = None;
    if let Some(s) = status.and_then(json::parse) {
        let r = s.get("reloads").cloned().unwrap_or(Value::Null);
        st.reloads = Some(
            Obj::new()
                .num("succeeded", json::uint_of(&r, "succeeded"))
                .num("failed", json::uint_of(&r, "failed"))
                .end(),
        );
        let c = s.get("connections").cloned().unwrap_or(Value::Null);
        st.connections = Some(
            Obj::new()
                .num("open", json::int_of(&c, "open"))
                .num("refused", json::uint_of(&c, "refused"))
                .num("pending_bytes", json::int_of(&c, "pending_bytes"))
                .end(),
        );
        let ups = match s.get("upstreams") {
            Some(Value::Array(a)) => a.clone(),
            _ => Vec::new(),
        };
        let (mut backends, mut down) = (0usize, 0usize);
        for u in &ups {
            if let Some(Value::Array(bs)) = u.get("backends") {
                backends = backends.saturating_add(bs.len());
                down = down.saturating_add(
                    bs.iter()
                        .filter(|b| !matches!(b.get("up"), Some(Value::Bool(true))))
                        .count(),
                );
            }
        }
        st.upstreams = Some(
            Obj::new()
                .num("count", ups.len())
                .num("backends", backends)
                .num("backends_down", down)
                .end(),
        );
        let certs = match s.get("certificates") {
            Some(Value::Array(a)) => a.clone(),
            _ => Vec::new(),
        };
        let mut o = Obj::new().num("count", certs.len());
        if let Some(near) = certs
            .iter()
            .filter(|c| !json::str_of(c, "not_after").is_empty())
            .min_by(|a, b| json::str_of(a, "not_after").cmp(json::str_of(b, "not_after")))
        {
            o = o
                .str("nearest_expiry", json::str_of(near, "not_after"))
                .raw(
                    "nearest_names",
                    &json::string_list(&json::strings_of(near, "names")),
                )
                .str_omit("nearest_listener", json::str_of(near, "listener"));
        }
        st.certificates = Some(o.end());
    }
    if let Some(c) = check.and_then(json::parse) {
        let sum = c.get("summary").cloned().unwrap_or(Value::Null);
        st.drift = Some(
            Obj::new()
                .num("count", json::int_of(&sum, "drift"))
                .num("info", json::int_of(&sum, "info"))
                .raw(
                    "checked_at",
                    &raw_or(&c, "checked_at", "\"0001-01-01T00:00:00Z\""),
                )
                .end(),
        );
    }
}

fn raw_or(v: &Value, key: &str, default: &str) -> String {
    v.get(key).map_or_else(|| default.to_owned(), json::compact)
}

/// A member's `GET /node` as fit's `NodeInfo`, the version akit's pages read in `akit_version`:
/// an akit's, or a fit's as `fit <version>` (decision row 72).
pub(crate) fn node_info_json(n: &Value) -> String {
    let mut version = json::str_of(n, "akit_version").to_owned();
    if version.is_empty() {
        let fit = json::str_of(n, "fit_version");
        if !fit.is_empty() {
            version = format!("fit {fit}");
        }
    }
    let list = |k: &str| json::string_list(&json::strings_of(n, k));
    Obj::new()
        .str("id", json::str_of(n, "id"))
        .str("hostname", json::str_of(n, "hostname"))
        .str("akit_version", &version)
        .str("fib_version", json::str_of(n, "fib_version"))
        .str("go_version", json::str_of(n, "go_version"))
        .str("os", json::str_of(n, "os"))
        .str("arch", json::str_of(n, "arch"))
        .raw("started", &raw_or(n, "started", "\"0001-01-01T00:00:00Z\""))
        .num("uptime_seconds", json::int_of(n, "uptime_seconds"))
        .raw("roles", &list("roles"))
        .num("schema_version", json::int_of(n, "schema_version"))
        .str("schema_digest", json::str_of(n, "schema_digest"))
        .raw("capabilities", &list("capabilities"))
        .end()
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]
    use super::*;

    #[test]
    fn cookies_from_a_login() {
        let r = Reply {
            status: 200,
            headers: vec![
                (
                    "Set-Cookie".into(),
                    "akit_console_2020=abc; Path=/; HttpOnly".into(),
                ),
                ("set-cookie".into(), "old=; Max-Age=0".into()),
                ("Set-Cookie".into(), "gone=x; Max-Age=0".into()),
                ("Set-Cookie".into(), "fit_console_2021=def".into()),
            ],
            body: Vec::new(),
            took: Duration::ZERO,
        };
        assert_eq!(
            cookie_header(&r),
            "akit_console_2020=abc; fit_console_2021=def"
        );
    }

    #[test]
    fn fit_versions_shown_as_fit() {
        let akit = json::parse(br#"{"akit_version": "0.1.0", "capabilities": ["node"]}"#).unwrap();
        assert!(node_info_json(&akit).contains(r#""akit_version":"0.1.0""#));
        let fit = json::parse(br#"{"fit_version": "v1.2.3", "fib_version": "v0.9"}"#).unwrap();
        let info = node_info_json(&fit);
        assert!(info.contains(r#""akit_version":"fit v1.2.3""#), "{info}");
        assert!(info.contains(r#""fib_version":"v0.9""#), "{info}");
    }

    #[test]
    fn fill_reads_status_and_check() {
        let mut st = State::new();
        fill(
            &mut st,
            br#"{"id": "n", "config_version": 3, "config_etag": "\"e\"", "capabilities": ["status"]}"#,
            Duration::from_millis(7),
            Some(br#"{"reloads": {"succeeded": 2, "failed": 1}, "connections": {"open": 5, "refused": 0, "pending_bytes": 10},
                "upstreams": [{"backends": [{"up": true}, {"up": false}]}, {"backends": []}],
                "certificates": [{"listener": "a", "names": ["x"], "not_after": "2027-01-01T00:00:00Z"},
                                 {"listener": "b", "names": ["y"], "not_after": "2026-12-01T00:00:00Z"}]}"#),
            Some(br#"{"checked_at": "2026-11-27T00:00:00Z", "summary": {"drift": 2, "info": 1}}"#),
        );
        assert_eq!(st.latency_ms, 7);
        assert_eq!(
            st.upstreams.as_deref(),
            Some(r#"{"count":2,"backends":2,"backends_down":1}"#)
        );
        assert!(
            st.certificates
                .as_deref()
                .unwrap()
                .contains(r#""nearest_listener":"b""#)
        );
        assert_eq!(
            st.drift.as_deref(),
            Some(r#"{"count":2,"info":1,"checked_at":"2026-11-27T00:00:00Z"}"#)
        );
        assert!(st.config.as_deref().unwrap().contains(r#""version":3"#));
    }
}
