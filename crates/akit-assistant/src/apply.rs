//! Proposals in the panel (I163, fit's `assistant/apply.go`): a conversation's proposal checked
//! on the running config and shown with its diff (`GET /api/proposals/<conversation>`), the
//! pending ones listed (`GET /api/proposals`), and applied or discarded (`POST
//! .../apply`, `.../discard`).
//!
//! An apply names the revision and the config's ETag the person confirmed, so that what is
//! applied is what they saw. It is made as the console's change, through the Admin API in process
//! with `If-Match`: the config steps as one change, which the control thread makes and answers
//! without the assistant waiting on it, then the purges. A proposal whose config changed meanwhile
//! is moved to the running config and offered again (412). One apply runs at a time; the next
//! waits its turn, as fit's `applyMu` makes it.

use std::collections::VecDeque;
use std::sync::{Arc, Mutex, PoisonError};

use akio_http::{Context, Detached, Method};
use akit_config::assistant::Level;
use akit_config::value::Value;
use akit_roles::pages;

use crate::audit::{self, Entry, Prop};
use crate::conv::now_ms;
use crate::json::{self, Obj, quoted};
use crate::proposal::{self, Built, DIFF_FORMATS, KIND_PATCH, Proposal, Purge};
use crate::redact::Redactor;
use crate::{ApiAnswer, Assistant};

/// The applies waiting for the one under way.
const MAX_APPLIES_WAITING: usize = 16;

/// An apply's turn: given to a job when it may run, `None` when too many wait.
type Turn = Box<dyn FnOnce(Option<ApplySlot>) + Send>;

/// The applies: whether one runs, and those waiting.
#[derive(Default)]
pub(crate) struct Applies {
    busy: bool,
    waiting: VecDeque<Turn>,
}

impl std::fmt::Debug for Applies {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Applies")
            .field("busy", &self.busy)
            .field("waiting", &self.waiting.len())
            .finish()
    }
}

/// The turn of the apply under way: the next one's begins when it is dropped.
pub(crate) struct ApplySlot(Assistant);

impl Drop for ApplySlot {
    fn drop(&mut self) {
        let next = {
            let mut q = self.0.applies();
            let next = q.waiting.pop_front();
            if next.is_none() {
                q.busy = false;
            }
            next
        };
        if let Some(job) = next {
            let slot = ApplySlot(self.0.clone());
            self.0.on_pool(Box::new(move || job(Some(slot))));
        }
    }
}

/// An apply's or a discard's body: the proposal, its revision, the config's ETag the person
/// confirmed, and, for another user's proposal, whose it is.
#[derive(Debug, Default)]
struct Body {
    user: String,
    id: String,
    revision: u64,
    etag: String,
}

/// An apply's answer, fit's `applyResult`.
#[derive(Debug, Default)]
struct Res {
    outcome: &'static str,
    version: u64,
    etag: String,
    warnings: Vec<String>,
    purged: Vec<(Purge, u64, String)>,
    proposal: String,
    error: String,
    errors: String,
}

impl Res {
    fn json(&self) -> String {
        let mut o = Obj::new().str("outcome", self.outcome);
        if self.version > 0 {
            o = o.num("version", self.version);
        }
        o = o.str_omit("etag", &self.etag);
        if !self.warnings.is_empty() {
            o = o.raw("warnings", &json::strings(&self.warnings));
        }
        if !self.purged.is_empty() {
            let items: Vec<String> = self
                .purged
                .iter()
                .map(|(p, n, e)| p.members().num("purged", n).str_omit("error", e).end())
                .collect();
            o = o.raw("purged", &format!("[{}]", items.join(",")));
        }
        if !self.proposal.is_empty() {
            o = o.raw("proposal", &self.proposal);
        }
        o = o.str_omit("error", &self.error);
        if !self.errors.is_empty() {
            o = o.raw("errors", &self.errors);
        }
        o.end()
    }
}

/// An apply under way, from its config's change to its answer.
struct Applying {
    a: Assistant,
    d: Detached,
    viewer: String,
    user: String,
    conv: String,
    p: Proposal,
    b: Built,
    red: Redactor,
    entry: Entry,
    res: Res,
    _turn: ApplySlot,
}

fn lock<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap_or_else(PoisonError::into_inner)
}

impl Assistant {
    pub(crate) fn applies(&self) -> std::sync::MutexGuard<'_, Applies> {
        lock(&self.0.applies)
    }

    /// Runs `job` on the tools' pool; here, when its queue is full: the work that comes back
    /// from the control thread is short, and goes on rather than being dropped.
    pub(crate) fn on_pool(&self, job: Box<dyn FnOnce() + Send>) {
        if let Err(akio::Rejected(job)) = self.0.tools_pool.try_execute(job) {
            job();
        }
    }

    /// `user`'s conversation `conv`'s proposal: `None` when there is no such conversation.
    pub(crate) fn proposal_of(&self, user: &str, conv: &str) -> Option<Option<Proposal>> {
        let mut chats = self.chats();
        let u = self.user_chats(&mut chats, user);
        u.get(conv).map(|c| c.proposal.clone())
    }

    /// Makes `p` `user`'s conversation `conv`'s proposal, the note of the last one cleared,
    /// unless the step is late (`expired`) or the conversation gone; then has it written.
    pub(crate) fn keep_proposal(
        &self,
        user: &str,
        conv: &str,
        p: &Proposal,
        expired: impl Fn() -> bool,
    ) -> Result<(), String> {
        {
            let mut chats = self.chats();
            if expired() {
                return Err("the step took too long and was not added".to_owned());
            }
            let u = self.user_chats(&mut chats, user);
            let Some(c) = u.get(conv) else {
                return Err(proposal::NO_CONVERSATION.to_owned());
            };
            c.proposal = Some(p.clone());
            c.proposal_note.clear();
        }
        self.save_chats(user);
        Ok(())
    }

    /// Makes `p` (or none) `user`'s conversation `conv`'s proposal, and `note` what the model is
    /// told of the last one, unless the proposal there is no longer `was` at revision `rev`: a
    /// step added meanwhile, or a discard, came first, and what it left stands.
    pub(crate) fn set_proposal(
        &self,
        user: &str,
        conv: &str,
        was: &str,
        rev: u64,
        p: Option<Proposal>,
        note: &str,
    ) {
        let ok = {
            let mut chats = self.chats();
            let u = self.user_chats(&mut chats, user);
            match u.get(conv) {
                Some(c)
                    if c.proposal
                        .as_ref()
                        .is_some_and(|q| q.id == was && q.revision == rev) =>
                {
                    c.proposal = p;
                    if !note.is_empty() {
                        note.clone_into(&mut c.proposal_note);
                    }
                    c.updated = now_ms();
                    true
                }
                _ => false,
            }
        };
        if ok {
            self.save_chats(user);
        }
    }

    /// `p`, of `user`'s conversation `conv`, as `viewer` sees it, `b` being `p` checked on the
    /// running config: fit's `ProposalView`. A diff that cannot be rendered says why in its
    /// place.
    fn view_from(
        &self,
        user: &str,
        conv: &str,
        p: &Proposal,
        viewer: &str,
        b: &Built,
        red: &Redactor,
    ) -> String {
        let level = self.level(viewer);
        let steps: Vec<String> = p
            .steps
            .iter()
            .enumerate()
            .map(|(i, st)| {
                let args = red.text(&st.args);
                Obj::new()
                    .str("tool", &st.tool)
                    .str("summary", &red.text(&st.summary))
                    .raw("args", if args.is_empty() { "null" } else { &args })
                    .num("at", st.at)
                    .str_omit("error", b.step_errs.get(i).map_or("", String::as_str))
                    .end()
            })
            .collect();
        // Go writes a map's members by name.
        let mut formats = DIFF_FORMATS;
        formats.sort_unstable();
        let mut diff = Obj::new();
        for f in formats {
            let d = proposal::diff(self, red, b, f)
                .unwrap_or_else(|e| format!("(the diff could not be rendered in {f}: {e})"));
            diff = diff.str(f, &d);
        }
        let stale = !b.kind.is_empty() && p.base_etag != b.etag;
        let mut o = Obj::new()
            .str("id", &p.id)
            .str("user", user)
            .str("conversation", conv)
            .num("revision", p.revision)
            .num("created", p.created)
            .num("updated", p.updated)
            .num("base_version", p.base_version)
            .str("base_etag", &p.base_etag)
            .num("version", b.version)
            .str("etag", &b.etag)
            .bool("stale", stale)
            .str_omit("kind", b.kind)
            .raw("steps", &format!("[{}]", steps.join(",")))
            .raw("changed", &json::strings(&b.changed))
            .raw("diff", &diff.end())
            .bool("valid", b.valid);
        if !b.errors.is_empty() {
            o = o.raw("errors", &proposal::problems_json(&b.errors));
        }
        if !b.fixes.is_empty() {
            let fixes: Vec<String> = b.fixes.iter().map(proposal::FixRef::json).collect();
            o = o.raw("fixes", &format!("[{}]", fixes.join(",")));
        }
        if b.rollback > 0 {
            o = o.num("rollback", b.rollback);
        }
        if !b.purges.is_empty() {
            let purges: Vec<String> = b.purges.iter().map(|x| x.members().end()).collect();
            o = o.raw("purges", &format!("[{}]", purges.join(",")));
        }
        let unconverted: Vec<String> = proposal::unconverted(&p.steps)
            .iter()
            .map(|it| red.text(it))
            .collect();
        if !unconverted.is_empty() {
            o = o.raw("unconverted", &json::strings(&unconverted));
        }
        o.str_omit("error", &p.error)
            .str("level", level.name())
            .bool("can_apply", level == Level::Apply)
            .end()
    }

    /// `p` checked on the running config and seen by `viewer`.
    pub(crate) fn view_proposal(
        &self,
        user: &str,
        conv: &str,
        p: &Proposal,
        viewer: &str,
    ) -> Result<(String, Built, Redactor), String> {
        let red = self.redactor(&self.settings());
        let b = proposal::build(self, &red, &p.steps)?;
        let v = self.view_from(user, conv, p, viewer, &b, &red);
        Ok((v, b, red))
    }

    /// Whose proposal a request names: the viewer's own, or, for a user whose level is apply,
    /// another's; and whether the viewer may.
    fn owner(&self, viewer: &str, named: &str) -> (String, bool) {
        if named.is_empty() || named == viewer {
            return (viewer.to_owned(), true);
        }
        (named.to_owned(), self.level(viewer) == Level::Apply)
    }

    /// `GET /api/proposals`: the pending proposals of the viewer's conversations, and, for a
    /// user whose level is apply, every user's, the latest first.
    pub(crate) fn proposals(&self, ctx: &mut Context, viewer: &str) {
        let Some(d) = ctx.detach() else { return };
        let (me, viewer) = (self.clone(), viewer.to_owned());
        self.run_tools(d, move |d| {
            let level = me.level(&viewer);
            let mut users = vec![viewer.clone()];
            if level == Level::Apply {
                users.extend(me.console_users().into_iter().filter(|u| *u != viewer));
            }
            let mut list: Vec<(i64, String)> = Vec::new();
            {
                let mut chats = me.chats();
                for user in &users {
                    let u = me.user_chats(&mut chats, user);
                    for c in &u.convs {
                        let Some(p) = &c.proposal else { continue };
                        let steps: Vec<(String, String)> = p
                            .steps
                            .iter()
                            .map(|s| (s.tool.clone(), s.summary.clone()))
                            .collect();
                        list.push((
                            p.updated,
                            Obj::new()
                                .str("user", user)
                                .str("conversation", &c.id)
                                .str("title", &c.title)
                                .str("id", &p.id)
                                .num("revision", p.revision)
                                .num("updated", p.updated)
                                .num("base_version", p.base_version)
                                .bool("valid", p.valid)
                                .raw("steps", &audit::steps_json(&steps))
                                .end(),
                        ));
                    }
                }
            }
            list.sort_by_key(|x| std::cmp::Reverse(x.0));
            let items: Vec<String> = list.into_iter().map(|(_, v)| v).collect();
            d.respond(pages::json(
                200,
                Obj::new()
                    .raw("proposals", &format!("[{}]", items.join(",")))
                    .str("level", level.name())
                    .end(),
            ));
        });
    }

    /// `GET /api/proposals/<conversation>` (`?user=` for another user's), and `POST
    /// /api/proposals/<conversation>/apply` and `/discard`.
    pub(crate) fn proposal(&self, ctx: &mut Context, viewer: &str, rest: &str) {
        let (conv, action) = rest.split_once('/').unwrap_or((rest, ""));
        let (conv, action) = (conv.to_owned(), action.to_owned());
        let method = ctx.method().clone();
        match (&method, action.as_str()) {
            (Method::Get, "") => {
                let named = ctx
                    .req()
                    .query_param("user")
                    .map(|u| akio_http::message::percent_decode(u).unwrap_or_default())
                    .unwrap_or_default();
                let (user, ok) = self.owner(viewer, &named);
                if !ok {
                    return ctx.respond(pages::fail(
                        403,
                        "only a user whose level is apply reads another user's proposal",
                    ));
                }
                let Some(d) = ctx.detach() else { return };
                let (me, viewer) = (self.clone(), viewer.to_owned());
                self.run_tools(d, move |d| match me.proposal_of(&user, &conv) {
                    None => d.respond(pages::fail(404, &format!("no conversation {conv}"))),
                    Some(None) => d.respond(pages::json(200, "{\"proposal\":null}".to_owned())),
                    Some(Some(p)) => match me.view_proposal(&user, &conv, &p, &viewer) {
                        Ok((v, _, _)) => {
                            d.respond(pages::json(200, Obj::new().raw("proposal", &v).end()));
                        }
                        Err(e) => d.respond(pages::fail(502, &e)),
                    },
                });
            }
            (Method::Post, "apply" | "discard") => {
                let body = match pages::decode(ctx) {
                    Ok(b) => b.0,
                    Err(r) => return ctx.respond(r),
                };
                let body = Body {
                    user: json::str_of(&body, "user").to_owned(),
                    id: json::str_of(&body, "id").to_owned(),
                    revision: match body.get("revision") {
                        Some(Value::Number(n)) => n.parse().unwrap_or(0),
                        _ => 0,
                    },
                    etag: json::str_of(&body, "etag").to_owned(),
                };
                let (user, ok) = self.owner(viewer, &body.user);
                if !ok {
                    return ctx.respond(pages::fail(
                        403,
                        "only a user whose level is apply acts on another user's proposal",
                    ));
                }
                let Some(d) = ctx.detach() else { return };
                let (me, viewer) = (self.clone(), viewer.to_owned());
                self.run_tools(d, move |d| {
                    if action == "apply" {
                        me.apply(d, viewer, user, conv, body);
                    } else {
                        me.discard(d, &viewer, &user, &conv, &body);
                    }
                });
            }
            _ => ctx.respond(pages::fail(
                405,
                "GET a proposal, or POST its apply or discard",
            )),
        }
    }

    /// `user`'s conversation `conv`'s proposal `id`, or why there is none to act on: no such
    /// conversation, another proposal, or the conversation answering.
    fn pending(&self, user: &str, conv: &str, id: &str) -> Result<Proposal, (u16, String)> {
        let mut chats = self.chats();
        let u = self.user_chats(&mut chats, user);
        let Some(c) = u.get(conv) else {
            return Err((404, format!("no conversation {conv}")));
        };
        match &c.proposal {
            Some(p) if p.id == id => {
                if c.busy {
                    return Err((
                        409,
                        "the conversation is answering, and may change its proposal: wait for \
                         the answer, or stop it"
                            .to_owned(),
                    ));
                }
                Ok(p.clone())
            }
            _ => Err((
                409,
                format!("the conversation has no proposal {id}: read it again"),
            )),
        }
    }

    /// `POST .../apply`, on the pool: refused below the level apply; otherwise in its turn.
    fn apply(&self, d: Detached, viewer: String, user: String, conv: String, body: Body) {
        let entry = Entry {
            user: viewer.clone(),
            owner: if user == viewer {
                String::new()
            } else {
                user.clone()
            },
            kind: audit::KIND_APPLY.to_owned(),
            conversation: conv.clone(),
            ..Entry::default()
        };
        let level = self.level(&viewer);
        if level != Level::Apply {
            match self.pending(&user, &conv, &body.id) {
                Err((status, msg)) => d.respond(pages::fail(status, &msg)),
                Ok(p) => {
                    let msg = format!(
                        "{viewer}'s level is {}: only a user whose level is apply applies a \
                         proposal",
                        level.name()
                    );
                    self.refuse(d, entry, 403, &msg, &p, String::new());
                }
            }
            return;
        }
        let me = self.clone();
        let turn: Turn = Box::new(move |slot| match slot {
            Some(slot) => me.apply_now(slot, d, viewer, user, conv, &body, entry),
            None => d.respond(pages::fail(
                503,
                "too many applies wait for their turn; try again",
            )),
        });
        let now = {
            let mut q = self.applies();
            if !q.busy {
                q.busy = true;
                Some(turn)
            } else if q.waiting.len() >= MAX_APPLIES_WAITING {
                drop(q);
                turn(None);
                return;
            } else {
                q.waiting.push_back(turn);
                None
            }
        };
        if let Some(turn) = now {
            turn(Some(ApplySlot(self.clone())));
        }
    }

    /// Answers a refused apply, `status`, and logs it in the audit log.
    fn refuse(
        &self,
        d: Detached,
        mut entry: Entry,
        status: u16,
        msg: &str,
        p: &Proposal,
        view: String,
    ) {
        entry.outcome = audit::REFUSED.to_owned();
        msg.clone_into(&mut entry.detail);
        if entry.proposal.is_none() {
            entry.proposal = Some(Prop::of(p, &[]));
        }
        self.audit(entry);
        let res = Res {
            outcome: audit::REFUSED,
            error: msg.to_owned(),
            proposal: view,
            ..Res::default()
        };
        d.respond(pages::json(status, res.json()));
    }

    /// The apply, in its turn: the proposal the person confirmed, at the revision and on the
    /// config they saw, made as the console's change.
    #[allow(clippy::too_many_arguments)]
    fn apply_now(
        &self,
        turn: ApplySlot,
        d: Detached,
        viewer: String,
        user: String,
        conv: String,
        body: &Body,
        mut entry: Entry,
    ) {
        let p = match self.pending(&user, &conv, &body.id) {
            Ok(p) => p,
            Err((status, msg)) => return d.respond(pages::fail(status, &msg)),
        };
        let (v, b, red) = match self.view_proposal(&user, &conv, &p, &viewer) {
            Ok(x) => x,
            Err(e) => return d.respond(pages::fail(502, &e)),
        };
        entry.proposal = Some(Prop::of(&p, &b.changed));
        let stale = !b.kind.is_empty() && p.base_etag != b.etag;
        if body.revision != p.revision {
            return self.refuse(
                d,
                entry,
                409,
                "the proposal changed since it was confirmed: review it again",
                &p,
                v,
            );
        }
        let ap = Applying {
            a: self.clone(),
            d,
            viewer,
            user,
            conv,
            p,
            b,
            red,
            entry,
            res: Res {
                outcome: audit::APPLIED,
                ..Res::default()
            },
            _turn: turn,
        };
        // Purges alone are not of a config, and are never stale.
        if !ap.b.kind.is_empty() && (stale || body.etag != ap.b.etag) {
            return ap.rebase("");
        }
        if !ap.b.valid {
            let Applying { a, d, entry, p, .. } = ap;
            return a.refuse(
                d,
                entry,
                422,
                "the proposal does not check, so it cannot be applied",
                &p,
                v,
            );
        }
        if !ap.b.kind.is_empty() && (ap.b.kind != KIND_PATCH || !ap.b.changed.is_empty()) {
            return ap.change_config();
        }
        ap.purge(0);
    }

    /// `POST .../discard`: the proposal is dropped, and nothing else changes.
    fn discard(&self, d: Detached, viewer: &str, user: &str, conv: &str, body: &Body) {
        if self.level(viewer) == Level::Off {
            return d.respond(pages::fail(
                403,
                &format!("the assistant is off for {viewer}"),
            ));
        }
        let p = match self.pending(user, conv, &body.id) {
            Ok(p) => p,
            Err((status, msg)) => return d.respond(pages::fail(status, &msg)),
        };
        self.set_proposal(
            user,
            conv,
            &p.id,
            p.revision,
            None,
            &format!("{viewer} discarded the proposal {}.", p.id),
        );
        self.audit(Entry {
            user: viewer.to_owned(),
            owner: if user == viewer {
                String::new()
            } else {
                user.to_owned()
            },
            kind: audit::KIND_DISCARD.to_owned(),
            conversation: conv.to_owned(),
            proposal: Some(Prop::of(&p, &[])),
            outcome: audit::DISCARDED.to_owned(),
            ..Entry::default()
        });
        d.respond(pages::json(200, Obj::new().str("discarded", &p.id).end()));
    }

    /// A conversation deleted with its proposal: the log says so.
    pub(crate) fn deleted_with(&self, user: &str, conv: &str, p: &Proposal) {
        self.audit(Entry {
            user: user.to_owned(),
            kind: audit::KIND_DISCARD.to_owned(),
            conversation: conv.to_owned(),
            proposal: Some(Prop::of(p, &[])),
            outcome: audit::DISCARDED.to_owned(),
            detail: "deleted with its conversation".to_owned(),
            ..Entry::default()
        });
    }

    /// Answers `d` with `work` run on the tools' pool, which reads the Admin API: 503 at once
    /// when its queue is full.
    pub(crate) fn run_tools(&self, d: Detached, work: impl FnOnce(Detached) + Send + 'static) {
        let cell = Arc::new(Mutex::new(Some(d)));
        let taken = Arc::clone(&cell);
        let job = Box::new(move || {
            if let Some(d) = lock(&taken).take() {
                work(d);
            }
        });
        if self.0.tools_pool.try_execute(job).is_err() {
            if let Some(d) = lock(&cell).take() {
                d.respond(pages::fail(503, "the assistant is busy; try again"));
            }
        }
    }
}

impl Applying {
    /// Moves the stale proposal to the running config and answers 412 with it, to be confirmed
    /// again, saying why: `msg`, or the config's change when empty.
    fn rebase(self, msg: &str) {
        let Applying {
            a,
            d,
            viewer,
            user,
            conv,
            mut p,
            b,
            red,
            mut entry,
            ..
        } = self;
        p.base_etag.clone_from(&b.etag);
        p.base_version = b.version;
        p.updated = now_ms();
        p.revision = p.revision.saturating_add(1);
        p.valid = b.valid;
        a.set_proposal(
            &user,
            &conv,
            &p.id,
            p.revision.saturating_sub(1),
            Some(p.clone()),
            "",
        );
        let v = a.view_from(&user, &conv, &p, &viewer, &b, &red);
        let msg = if msg.is_empty() {
            format!(
                "the config changed since the proposal was checked: it is offered again on \
                 version {}; review it and apply again",
                b.version
            )
        } else {
            msg.to_owned()
        };
        entry.outcome = audit::STALE.to_owned();
        entry.detail.clone_from(&msg);
        a.audit(entry);
        let res = Res {
            outcome: audit::STALE,
            error: msg,
            proposal: v,
            ..Res::default()
        };
        d.respond(pages::json(412, res.json()));
    }

    /// The config steps as one change, through the endpoint of their kind, with `If-Match` the
    /// ETag they were checked on; the rest once the control thread answered, back on the pool.
    fn change_config(self) {
        let b = &self.b;
        let (method, target, body) = match b.kind {
            KIND_PATCH => (
                "PUT",
                format!(
                    "/config/text?format=json&members={}",
                    query_escape(&b.changed.join(","))
                ),
                // The members changed, set or removed, the rest left as written.
                json::compact(&proposal::subset(&b.after, &b.changed)),
            ),
            proposal::KIND_RECONCILE => {
                let fixes: Vec<String> = b.fixes.iter().map(proposal::FixRef::json).collect();
                (
                    "POST",
                    "/config/reconcile".to_owned(),
                    format!("{{\"fixes\":[{}]}}", fixes.join(",")),
                )
            }
            _ => ("POST", format!("/rollback/{}", b.rollback), String::new()),
        };
        let Some(api) = self.a.0.o.api.clone() else {
            return self.failed(ApiAnswer {
                status: 502,
                body: format!("{{\"error\":{}}}", quoted(crate::NO_API)).into_bytes(),
                ..ApiAnswer::default()
            });
        };
        let etag = b.etag.clone();
        let me = Arc::new(Mutex::new(Some(self)));
        api.change(
            method,
            &target,
            &etag,
            body.into_bytes(),
            Box::new(move |ans| {
                let Some(ap) = lock(&me).take() else { return };
                let a = ap.a.clone();
                a.on_pool(Box::new(move || ap.changed(ans)));
            }),
        );
    }

    /// The Admin API's answer to the config's change.
    fn changed(mut self, ans: ApiAnswer) {
        if ans.status == 412 {
            return self.rebase("");
        }
        if !(200..300).contains(&ans.status) {
            return self.failed(ans);
        }
        if let Ok(v) = akit_config::value::parse(&ans.text()) {
            self.res.version = match v.get("version") {
                Some(Value::Number(n)) => n.parse().unwrap_or(0),
                _ => 0,
            };
            json::str_of(&v, "etag").clone_into(&mut self.res.etag);
            if let Some(Value::Array(ws)) = v.get("warnings") {
                self.res.warnings = ws
                    .iter()
                    .filter_map(|w| match w {
                        Value::String(s) => Some(s.clone()),
                        _ => None,
                    })
                    .collect();
            }
        }
        self.purge(0);
    }

    /// The Admin API refused the change: nothing was applied, the proposal stays with why.
    fn failed(mut self, ans: ApiAnswer) {
        let msg = self.red.text(&ans.message());
        self.res.outcome = audit::FAILED;
        self.res.error.clone_from(&msg);
        if let Ok(v) = akit_config::value::parse(&ans.text()) {
            if let Some(errs @ Value::Array(_)) = v.get("errors") {
                self.res.errors = self.red.text(&json::compact(errs));
            }
        }
        let Applying {
            a,
            d,
            viewer,
            user,
            conv,
            mut p,
            b,
            red,
            mut entry,
            mut res,
            ..
        } = self;
        p.error.clone_from(&msg);
        a.set_proposal(&user, &conv, &p.id, p.revision, Some(p.clone()), "");
        res.proposal = a.view_from(&user, &conv, &p, &viewer, &b, &red);
        entry.outcome = audit::FAILED.to_owned();
        entry.detail = msg.clone();
        a.audit(entry);
        (a.0.o.log)(format!(
            "console: {viewer:?}: the assistant's proposal {} was not applied: {msg}",
            p.id
        ));
        d.respond(pages::json(422, res.json()));
    }

    /// The purges from `i` on, one after the other, then the answer.
    fn purge(mut self, i: usize) {
        let Some(pg) = self.b.purges.get(i).cloned() else {
            return self.done();
        };
        let Some(api) = self.a.0.o.api.clone() else {
            self.res.purged.push((pg, 0, crate::NO_API.to_owned()));
            self.res.outcome = audit::PARTIAL;
            return self.purge(i.saturating_add(1));
        };
        let body = pg.members().end();
        let me = Arc::new(Mutex::new(Some(self)));
        api.change(
            "POST",
            "/cache/purge",
            "",
            body.into_bytes(),
            Box::new(move |ans| {
                let Some(mut ap) = lock(&me).take() else {
                    return;
                };
                if (200..300).contains(&ans.status) {
                    let n = akit_config::value::parse(&ans.text())
                        .ok()
                        .and_then(|v| match v.get("purged") {
                            Some(Value::Number(n)) => n.parse().ok(),
                            _ => None,
                        })
                        .unwrap_or(0);
                    ap.res.purged.push((pg, n, String::new()));
                } else {
                    let e = format!("the Admin API answered {}: {}", ans.status, ans.message());
                    ap.res.purged.push((pg, 0, e));
                    ap.res.outcome = audit::PARTIAL;
                }
                let a = ap.a.clone();
                a.on_pool(Box::new(move || ap.purge(i.saturating_add(1))));
            }),
        );
    }

    /// The proposal applied: it is gone from its conversation, which says what became of it.
    fn done(self) {
        let Applying {
            a,
            d,
            viewer,
            user,
            conv,
            p,
            mut entry,
            res,
            ..
        } = self;
        let mut note = format!(
            "{viewer} applied the proposal {} ({} steps)",
            p.id,
            p.steps.len()
        );
        if res.version > 0 {
            note.push_str(&format!(" as config version {}", res.version));
        }
        if res.purged.iter().any(|(_, _, e)| !e.is_empty()) {
            note.push_str("; a cache purge failed");
        }
        a.set_proposal(&user, &conv, &p.id, p.revision, None, &format!("{note}."));
        entry.outcome = res.outcome.to_owned();
        entry.version = res.version;
        if let Some((_, _, e)) = res.purged.iter().rev().find(|(_, _, e)| !e.is_empty()) {
            entry.detail.clone_from(e);
        }
        a.audit(entry);
        (a.0.o.log)(format!("console: {viewer:?}: {note}"));
        d.respond(pages::json(200, res.json()));
    }
}

/// `s` as Go's `url.QueryEscape` writes it.
fn query_escape(s: &str) -> String {
    let mut out = String::new();
    for b in s.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(char::from(b));
            }
            b' ' => out.push('+'),
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}
