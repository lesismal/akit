//! Users' roles (I139, fit's `cluster/roles.go`, its I148): viewer, operator and admin, held
//! cluster-wide or on groups, a member's role the highest of the cluster's and its groups';
//! `admin.cluster.admins` cluster-wide admins whatever the roles say; a default role for console
//! users not named. Every endpoint is checked: a read needs a role somewhere, a change a
//! cluster-wide admin, but for the endpoints that check what they touch (a member's reads and
//! pages, operations, syncs, rollouts, silences). Until roles are set, and with no
//! `admins`, every console user is a cluster-wide admin (open), as in fit.
//!
//! The roles are kept in the store's document, `access`, fit's. On a manager with roles, the
//! console's changes to the manager itself (its config, users, store and services) are a
//! cluster-wide admin's ([`Manager::console_gate`], fit's `ConsoleGate`).
//!
//! ```text
//! GET /api/me       the user's roles: cluster-wide, on each group and on each member
//! GET /api/access   the roles, with every console user, the admins and the groups, and its ETag
//! PUT /api/access   the roles replaced (If-Match the members' ETag), by a cluster-wide admin
//! ```

// A forbidden is answered whole; its size does not matter.
#![allow(clippy::result_large_err)]

use std::collections::BTreeMap;

use akio_http::{Context, Method, Response};
use akit_config::value::Value;
use akit_roles::pages::{fail, json as answer};

use crate::json::{self, Obj, quoted};
use crate::member::{Document, FieldError, MAX_BODY_BYTES, MAX_USERNAME_BYTES, Member};
use crate::{Manager, Refusal};

/// The roles, least first.
pub const VIEWER: &str = "viewer";
/// See [`VIEWER`].
pub const OPERATOR: &str = "operator";
/// See [`VIEWER`].
pub const ADMIN: &str = "admin";
/// Every role, least first.
pub const ROLES: &[&str] = &[VIEWER, OPERATOR, ADMIN];
/// The most users named.
pub const MAX_USERS: usize = 256;
/// The most groups a user has roles on.
pub const MAX_USER_GROUPS: usize = 64;
/// The name the manager's Admin API acts under (fit's `AdminAPIUser`): a cluster-wide admin; a
/// console user of that name has no role.
pub const ADMIN_API_USER: &str = "Admin API";
const ACTION_ACCESS_UPDATE: &str = "access.update";

fn rank(r: &str) -> usize {
    ROLES
        .iter()
        .position(|x| *x == r)
        .map_or(0, |i| i.saturating_add(1))
}

fn higher<'a>(a: &'a str, b: &'a str) -> &'a str {
    if rank(b) > rank(a) { b } else { a }
}

fn field(f: &str, msg: impl Into<String>) -> Refusal {
    Refusal::Field(FieldError {
        field: f.to_owned(),
        msg: msg.into(),
    })
}

fn printable(s: &str) -> bool {
    s.chars().all(|c| c >= ' ' && c != '\u{7f}')
}

/// A user's roles: fit's `UserRoles`.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct UserRoles {
    /// The console user.
    pub user: String,
    /// Its role cluster-wide, or none.
    pub role: String,
    /// Its roles on groups.
    pub groups: BTreeMap<String, String>,
}

/// The users' roles: fit's `Access`.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Access {
    /// The users named.
    pub users: Vec<UserRoles>,
    /// The role of a console user not named, or none.
    pub default_role: String,
}

impl Access {
    /// As the store keeps it.
    #[must_use]
    pub fn json(&self) -> String {
        let users = self
            .users
            .iter()
            .map(|u| {
                let mut o = Obj::new().str("user", &u.user).str_omit("role", &u.role);
                if !u.groups.is_empty() {
                    let mut g = Obj::new();
                    for (k, v) in &u.groups {
                        g = g.str(k, v);
                    }
                    o = o.raw("groups", &g.end());
                }
                o.end()
            })
            .collect::<Vec<_>>()
            .join(",");
        Obj::new()
            .raw("users", &format!("[{users}]"))
            .str_omit("default_role", &self.default_role)
            .end()
    }

    /// As kept or sent.
    #[must_use]
    pub fn from_value(v: &Value) -> Access {
        let users = match v.get("users") {
            Some(Value::Array(a)) => a
                .iter()
                .map(|u| UserRoles {
                    user: json::str_of(u, "user").to_owned(),
                    role: json::str_of(u, "role").to_owned(),
                    groups: match u.get("groups") {
                        Some(Value::Object(ms)) => ms
                            .iter()
                            .map(|(k, v)| {
                                let r = match v {
                                    Value::String(s) => s.clone(),
                                    _ => String::new(),
                                };
                                (k.clone(), r)
                            })
                            .collect(),
                        _ => BTreeMap::new(),
                    },
                })
                .collect(),
            _ => Vec::new(),
        };
        Access {
            users,
            default_role: json::str_of(v, "default_role").to_owned(),
        }
    }

    fn user(&self, user: &str) -> Option<&UserRoles> {
        self.users.iter().find(|u| u.user == user)
    }

    /// Checks it against `d`'s groups: fit's `Access.check`.
    fn check(&self, d: &Document) -> Result<(), Refusal> {
        let roles = ROLES.join(", ");
        if !self.default_role.is_empty() && rank(&self.default_role) == 0 {
            return Err(field(
                "default_role",
                format!(
                    "{} is not a role: {roles}, or none",
                    quoted(&self.default_role)
                ),
            ));
        }
        if self.users.len() > MAX_USERS {
            return Err(field(
                "users",
                format!("{} users, more than {MAX_USERS}", self.users.len()),
            ));
        }
        for (i, u) in self.users.iter().enumerate() {
            let at = format!("users[{i}]");
            if u.user.is_empty() || u.user.len() > MAX_USERNAME_BYTES || !printable(&u.user) {
                return Err(field(
                    &format!("{at}.user"),
                    format!("a user's name is 1 to {MAX_USERNAME_BYTES} printable characters"),
                ));
            }
            if self.users.iter().take(i).any(|x| x.user == u.user) {
                return Err(field(
                    &format!("{at}.user"),
                    format!("{} is named twice", quoted(&u.user)),
                ));
            }
            if !u.role.is_empty() && rank(&u.role) == 0 {
                return Err(field(
                    &format!("{at}.role"),
                    format!("{} is not a role: {roles}, or none", quoted(&u.role)),
                ));
            }
            if u.groups.len() > MAX_USER_GROUPS {
                return Err(field(
                    &format!("{at}.groups"),
                    format!(
                        "roles on {} groups, more than {MAX_USER_GROUPS}",
                        u.groups.len()
                    ),
                ));
            }
            for (g, r) in &u.groups {
                if rank(r) == 0 {
                    return Err(field(
                        &format!("{at}.groups"),
                        format!(
                            "{} on the group {} is not a role: {roles}",
                            quoted(r),
                            quoted(g)
                        ),
                    ));
                }
                if !d.groups.iter().any(|x| &x.name == g) {
                    return Err(field(
                        &format!("{at}.groups"),
                        format!("no group is named {}", quoted(g)),
                    ));
                }
            }
        }
        Ok(())
    }
}

fn or_none(r: &str) -> &str {
    if r.is_empty() { "none" } else { r }
}

/// The roles' changes when group `old` is renamed `new`, or removed (empty): fit's
/// `renameGroupInAccess`.
pub fn rename_group(d: &mut Document, old: &str, new: &str) {
    let Some(a) = d.access.as_mut() else { return };
    for u in &mut a.users {
        let Some(r) = u.groups.remove(old) else {
            continue;
        };
        if !new.is_empty() {
            let have = u.groups.get(new).cloned().unwrap_or_default();
            let top = higher(&have, &r).to_owned();
            u.groups.insert(new.to_owned(), top);
        }
    }
}

/// A user's roles, resolved: fit's `roleSet`.
#[derive(Debug, Clone, Default)]
pub(crate) struct RoleSet {
    pub(crate) cluster: String,
    pub(crate) groups: BTreeMap<String, String>,
    /// No roles are set, and no admins: everyone is an admin.
    pub(crate) open: bool,
    /// One of `admin.cluster.admins`, or the Admin API.
    pub(crate) fixed: bool,
}

impl RoleSet {
    /// The role on a member in `groups`.
    pub(crate) fn on(&self, groups: &[String]) -> String {
        let mut r = self.cluster.as_str();
        for g in groups {
            r = higher(r, self.groups.get(g).map_or("", String::as_str));
        }
        r.to_owned()
    }

    fn on_group(&self, g: &str) -> String {
        higher(&self.cluster, self.groups.get(g).map_or("", String::as_str)).to_owned()
    }

    fn top(&self) -> String {
        let mut r = self.cluster.as_str();
        for g in self.groups.values() {
            r = higher(r, g);
        }
        r.to_owned()
    }
}

/// `user`'s roles in `d`: fit's `rolesIn`.
pub(crate) fn roles_in(d: &Document, admins: &[String], user: &str) -> RoleSet {
    let admin = |fixed, open| RoleSet {
        cluster: ADMIN.to_owned(),
        fixed,
        open,
        ..RoleSet::default()
    };
    if user == ADMIN_API_USER {
        return admin(true, false);
    }
    if d.access.is_none() && admins.is_empty() {
        return admin(false, true);
    }
    if admins.iter().any(|a| a == user) {
        return admin(true, false);
    }
    match &d.access {
        Some(a) => match a.user(user) {
            Some(u) => RoleSet {
                cluster: u.role.clone(),
                groups: u.groups.clone(),
                ..RoleSet::default()
            },
            None => RoleSet {
                cluster: a.default_role.clone(),
                ..RoleSet::default()
            },
        },
        None => RoleSet::default(),
    }
}

/// What a change needs of its user's roles: fit's `Need`.
#[derive(Debug, Clone, Default)]
pub struct Need {
    /// The role.
    pub role: &'static str,
    /// On each of these members,
    pub nodes: Vec<String>,
    /// each of these groups,
    pub groups: Vec<String>,
    /// or cluster-wide; with none of them, somewhere.
    pub cluster: bool,
}

impl Need {
    /// `role` somewhere.
    #[must_use]
    pub fn any(role: &'static str) -> Need {
        Need {
            role,
            ..Need::default()
        }
    }

    /// `role` on `nodes`.
    #[must_use]
    pub fn on(role: &'static str, nodes: Vec<String>) -> Need {
        Need {
            role,
            nodes,
            ..Need::default()
        }
    }

    /// `role` cluster-wide.
    #[must_use]
    pub fn cluster(role: &'static str) -> Need {
        Need {
            role,
            cluster: true,
            ..Need::default()
        }
    }
}

/// Why a user may not: fit's `ErrForbidden`, answered 403.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Forbidden {
    /// The user.
    pub user: String,
    /// The role it needs,
    pub role: String,
    /// on these members,
    pub nodes: Vec<String>,
    /// these groups,
    pub groups: Vec<String>,
    /// or cluster-wide.
    pub cluster: bool,
}

impl Forbidden {
    /// Why, in words.
    #[must_use]
    pub fn message(&self) -> String {
        let where_ = if self.cluster {
            "cluster-wide".to_owned()
        } else if !self.nodes.is_empty() {
            format!(
                "on the members {}",
                crate::api::clip(&self.nodes.join(", "), 512)
            )
        } else if !self.groups.is_empty() {
            format!(
                "on the groups {}",
                crate::api::clip(&self.groups.join(", "), 512)
            )
        } else {
            "on the cluster or one of its groups".to_owned()
        };
        format!(
            "{} may not do this: it needs the role {} {where_}, which an admin of the cluster \
             gives on the Users and audit page",
            quoted(&self.user),
            self.role
        )
    }

    /// The answer: fit's `forbiddenBody`.
    #[must_use]
    pub fn response(&self) -> Response {
        let mut o = Obj::new()
            .str("error", &self.message())
            .str("kind", crate::transport::KIND_FORBIDDEN)
            .str("user", &self.user)
            .str("role", &self.role);
        if !self.nodes.is_empty() {
            o = o.raw("nodes", &json::string_list(&self.nodes));
        }
        if !self.groups.is_empty() {
            o = o.raw("groups", &json::string_list(&self.groups));
        }
        if self.cluster {
            o = o.bool("cluster", true);
        }
        answer(403, o.end())
    }
}

fn forbidden(user: &str, role: &str) -> Forbidden {
    Forbidden {
        user: user.to_owned(),
        role: role.to_owned(),
        ..Forbidden::default()
    }
}

impl Manager {
    fn admins(&self) -> &[String] {
        &self.0.o.settings.admins
    }

    /// The groups member `m` of `d` is in.
    fn groups_of(d: &Document, m: &Member) -> Vec<String> {
        crate::groups::groups_of(d, &m.name, &m.labels)
    }

    /// Whether `user` may do what `need` says: fit's `Allow`.
    ///
    /// # Errors
    ///
    /// [`Refusal::Forbidden`].
    pub(crate) fn allow(&self, user: &str, need: &Need) -> Result<(), Refusal> {
        self.allow_doc(&self.doc(), user, need)
    }

    pub(crate) fn allow_doc(&self, d: &Document, user: &str, need: &Need) -> Result<(), Refusal> {
        let rs = roles_in(d, self.admins(), user);
        let want = rank(need.role);
        let mut refused = forbidden(user, need.role);
        if need.cluster && rank(&rs.cluster) < want {
            refused.cluster = true;
            return Err(Refusal::Forbidden(Box::new(refused)));
        }
        for n in &need.nodes {
            let role = d
                .members
                .iter()
                .find(|m| &m.name == n)
                .map_or_else(|| rs.cluster.clone(), |m| rs.on(&Manager::groups_of(d, m)));
            if rank(&role) < want {
                refused.nodes.push(n.clone());
            }
        }
        for g in &need.groups {
            if rank(&rs.on_group(g)) < want {
                refused.groups.push(g.clone());
            }
        }
        if !refused.nodes.is_empty() || !refused.groups.is_empty() {
            return Err(Refusal::Forbidden(Box::new(refused)));
        }
        if !need.cluster
            && need.nodes.is_empty()
            && need.groups.is_empty()
            && rank(&rs.top()) < want
        {
            return Err(Refusal::Forbidden(Box::new(refused)));
        }
        Ok(())
    }

    /// Whether `user` has `role` on each of `mems`, members as `d` would keep them: fit's
    /// `allowIn`.
    pub(crate) fn allow_in(
        &self,
        d: &Document,
        user: &str,
        role: &'static str,
        mems: &[&Member],
    ) -> Result<(), Refusal> {
        let rs = roles_in(d, self.admins(), user);
        let mut refused = forbidden(user, role);
        for m in mems {
            if rank(&rs.on(&Manager::groups_of(d, m))) < rank(role) {
                refused.nodes.push(m.name.clone());
            }
        }
        if refused.nodes.is_empty() {
            Ok(())
        } else {
            Err(Refusal::Forbidden(Box::new(refused)))
        }
    }

    /// Whether `user` is a cluster-wide admin in `d`: fit's `allowCluster`.
    pub(crate) fn allow_cluster(&self, d: &Document, user: &str) -> Result<(), Refusal> {
        if roles_in(d, self.admins(), user).cluster == ADMIN {
            return Ok(());
        }
        Err(Refusal::Forbidden(Box::new(Forbidden {
            cluster: true,
            ..forbidden(user, ADMIN)
        })))
    }

    /// The module's gate: a read needs a role somewhere; a change a cluster-wide admin, but on
    /// the endpoints that check what they touch (`checked`). fit's `gate`.
    pub(crate) fn gate(&self, user: &str, method: &Method, checked: bool) -> Option<Response> {
        if user == ADMIN_API_USER {
            return Some(fail(
                403,
                &format!(
                    "{} is the name the manager's Admin API acts under: a console user of that \
                     name has no role",
                    quoted(ADMIN_API_USER)
                ),
            ));
        }
        let read = matches!(method, Method::Get | Method::Head);
        let need = if !checked && !read {
            Need::cluster(ADMIN)
        } else {
            Need::any(VIEWER)
        };
        self.allow(user, &need).err().map(|e| e.response())
    }

    /// Whether a console request to the manager itself may go on, on a manager with roles: a
    /// read, the module's own endpoints, the node switcher, a password change and the text tools
    /// may; the rest is a cluster-wide admin's. fit's `ConsoleGate`.
    #[must_use]
    pub fn console_gate(
        &self,
        user: &str,
        method: &Method,
        path: &str,
        query: &str,
    ) -> Option<String> {
        let read = matches!(method, Method::Get | Method::Head);
        if user == ADMIN_API_USER && !read {
            return Some(format!(
                "{} is the name the manager's Admin API acts under: a console user of that name \
                 changes nothing",
                quoted(ADMIN_API_USER)
            ));
        }
        if read
            || path.starts_with(&format!("/api/m/{}/", crate::MODULE_ID))
            || path.starts_with(crate::pass::PASS_PREFIX)
            || path.starts_with("/api/m/ai/")
            || matches!(
                path,
                "/api/logout" | "/api/password" | "/api/m/admin/account/password"
            )
            || (*method == Method::Post
                && matches!(
                    path,
                    "/api/m/admin/config/render" | "/api/m/admin/config/parse"
                ))
        {
            return None;
        }
        if path.starts_with("/api/m/admin/")
            && query
                .split('&')
                .any(|p| p == "dry_run=1" || p == "dry_run=true")
        {
            return None;
        }
        self.allow(user, &Need::cluster(ADMIN)).err().map(|_| {
            format!(
                "{} may not change this manager: on a manager whose cluster has roles, its own \
                 config, users, store and services are changed by a cluster-wide admin only",
                quoted(user)
            )
        })
    }

    /// `GET /api/me`: fit's `me`.
    pub(crate) fn me_json(&self, user: &str) -> String {
        let d = self.doc();
        let rs = roles_in(&d, self.admins(), user);
        let mut groups = Obj::new();
        for g in &d.groups {
            let r = rs.on_group(&g.name);
            if !r.is_empty() {
                groups = groups.str(&g.name, &r);
            }
        }
        let mut nodes = Obj::new();
        for m in &d.members {
            let r = rs.on(&Manager::groups_of(&d, m));
            if !r.is_empty() {
                nodes = nodes.str(&m.name, &r);
            }
        }
        let mut o = Obj::new().str("user", user).bool("open", rs.open);
        if rs.fixed {
            o = o.bool("fixed", true);
        }
        o.str("role", &rs.cluster)
            .str("top", &rs.top())
            .raw("groups", &groups.end())
            .raw("nodes", &nodes.end())
            .bool("manager_read_only", !rs.open && rs.cluster != ADMIN)
            .str("assistant", "")
            .end()
    }

    /// `GET /api/access`: fit's `accessView`.
    fn access_json(&self) -> String {
        let d = self.doc();
        let admins = self.admins().to_vec();
        let open = d.access.is_none() && admins.is_empty();
        let console: Vec<String> = self
            .0
            .o
            .console_users
            .as_ref()
            .map(|f| f())
            .unwrap_or_default();
        let mut names: Vec<String> = Vec::new();
        if let Some(a) = &d.access {
            names.extend(a.users.iter().map(|u| u.user.clone()));
        }
        names.extend(console.iter().cloned());
        names.extend(admins.iter().cloned());
        names.sort();
        names.dedup();
        let users: Vec<String> = names
            .iter()
            .map(|n| {
                let fixed = admins.contains(n);
                let (mut role, groups) = match d.access.as_ref() {
                    Some(a) => match a.user(n) {
                        Some(u) => (u.role.clone(), u.groups.clone()),
                        None => (a.default_role.clone(), BTreeMap::new()),
                    },
                    None => (String::new(), BTreeMap::new()),
                };
                if fixed || open {
                    ADMIN.clone_into(&mut role);
                }
                let mut g = Obj::new();
                for (k, v) in &groups {
                    g = g.str(k, v);
                }
                let mut o = Obj::new()
                    .str("user", n)
                    .str_omit("role", &role)
                    .raw("groups", &g.end())
                    .bool("console", console.contains(n));
                if fixed {
                    o = o.bool("fixed", true);
                }
                o.end()
            })
            .collect();
        let mut groups: Vec<String> = d.groups.iter().map(|g| g.name.clone()).collect();
        groups.sort();
        let roles: Vec<String> = ROLES.iter().map(|r| (*r).to_owned()).collect();
        Obj::new()
            .str("version", self.etag().trim_matches('"'))
            .bool("open", open)
            .str_omit("read_only", &self.read_only())
            .raw("admins", &json::string_list(&admins))
            .str(
                "default_role",
                d.access.as_ref().map_or("", |a| a.default_role.as_str()),
            )
            .raw("users", &format!("[{}]", users.join(",")))
            .raw("groups", &json::string_list(&groups))
            .raw("roles", &json::string_list(&roles))
            .num("max_users", MAX_USERS)
            .end()
    }

    /// `PUT /api/access`: the roles replaced, by a cluster-wide admin, at least one left: fit's
    /// `SetAccess`. On the manager's pool.
    fn set_access(&self, user: &str, v: &Value) -> Result<String, Refusal> {
        let mut a = Access::from_value(v);
        self.change(user, |d| {
            self.allow_cluster(d, user)?;
            for u in &mut a.users {
                u.user = u.user.trim().to_owned();
                u.groups.retain(|_, r| !r.is_empty());
            }
            let default = a.default_role.clone();
            a.users
                .retain(|u| !(u.role.is_empty() && u.groups.is_empty() && default.is_empty()));
            a.users.sort_by(|x, y| x.user.cmp(&y.user));
            a.check(d)?;
            if self.admins().is_empty() && !a.users.iter().any(|u| u.role == ADMIN) {
                return Err(field(
                    "users",
                    "no one would be an admin of the cluster: give at least one user the role \
                     admin cluster-wide (or name one in admin.cluster.admins)",
                ));
            }
            let before = d.access.take();
            let detail = describe_change(before.as_ref(), &a);
            d.access = Some(a.clone());
            let s = |x: &str| Value::String(x.to_owned());
            Ok((
                vec![
                    ("action".to_owned(), s(ACTION_ACCESS_UPDATE)),
                    ("detail".to_owned(), s(&detail)),
                ],
                (),
            ))
        })?;
        self.log(format!("cluster: {user:?} changed the users' roles"));
        Ok(self.access_json())
    }
}

/// What changed of the roles, in words: fit's `describeAccessChange`.
fn describe_change(a: Option<&Access>, b: &Access) -> String {
    let mut old: BTreeMap<String, UserRoles> = a
        .map(|a| {
            a.users
                .iter()
                .map(|u| (u.user.clone(), u.clone()))
                .collect()
        })
        .unwrap_or_default();
    let mut changed = Vec::new();
    for u in &b.users {
        if old.get(&u.user) != Some(u) {
            changed.push(u.user.clone());
        }
        old.remove(&u.user);
    }
    changed.extend(old.into_keys());
    let mut s = if changed.is_empty() {
        "no user's roles".to_owned()
    } else {
        format!(
            "the roles of {}",
            crate::api::clip(&changed.join(", "), 512)
        )
    };
    if a.is_none() {
        s = format!("roles set for the first time; {s}");
    }
    if a.map_or(!b.default_role.is_empty(), |a| {
        a.default_role != b.default_role
    }) {
        s.push_str(&format!("; the default role {}", or_none(&b.default_role)));
    }
    s
}

/// The roles' endpoints; `false` for a path that is not theirs.
pub(crate) fn serve(m: &Manager, ctx: &mut Context, user: &str, path: &str) -> bool {
    let method = ctx.method().clone();
    match path {
        "/api/me" if matches!(method, Method::Get | Method::Head) => {
            ctx.respond(answer(200, m.me_json(user)));
        }
        "/api/access" if matches!(method, Method::Get | Method::Head) => {
            ctx.respond(answer(200, m.access_json()).header("ETag", m.etag()));
        }
        "/api/access" if method == Method::Put => {
            if ctx.body().len() > MAX_BODY_BYTES {
                ctx.respond(fail(
                    413,
                    &format!("a request is {MAX_BODY_BYTES} bytes at most"),
                ));
                return true;
            }
            let v = match akit_roles::pages::decode(ctx) {
                Ok(b) => b.0,
                Err(r) => {
                    ctx.respond(r);
                    return true;
                }
            };
            if !crate::api::if_match(m, ctx) {
                return true;
            }
            let (me, user) = (m.clone(), user.to_owned());
            crate::api::on_pool(m, ctx, move || match me.set_access(&user, &v) {
                Ok(b) => answer(200, b).header("ETag", me.etag()),
                Err(e) => e.response(),
            });
        }
        "/api/me" => ctx.respond(fail(405, "the method is not allowed").header("Allow", "GET")),
        "/api/access" => {
            ctx.respond(fail(405, "the method is not allowed").header("Allow", "GET, PUT"));
        }
        _ => return false,
    }
    true
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::indexing_slicing)]
    use super::*;
    use crate::groups::Group;

    fn doc(access: Option<Access>) -> Document {
        let mut d = Document {
            access,
            ..Document::default()
        };
        let m = |s: &str| Member::from_value(&json::parse(s.as_bytes()).unwrap());
        d.members
            .push(m(r#"{"name": "a", "labels": {"region": "eu"}}"#));
        d.members.push(m(r#"{"name": "b"}"#));
        d.groups.push(Group {
            name: "eu".into(),
            selector: "region=eu".into(),
            ..Group::default()
        });
        d
    }

    #[test]
    fn roles_resolved() {
        // fit's TestRoles: open without roles and admins; admins fixed; a group's role on its
        // members; the default role for those not named.
        let open = roles_in(&doc(None), &[], "ada");
        assert!(open.open && open.cluster == ADMIN);
        let fixed = roles_in(&doc(None), &["ada".into()], "ada");
        assert!(fixed.fixed && !fixed.open);
        assert_eq!(roles_in(&doc(None), &["ada".into()], "bob").cluster, "");
        let a = Access {
            users: vec![UserRoles {
                user: "bob".into(),
                role: VIEWER.into(),
                groups: [("eu".to_owned(), OPERATOR.to_owned())].into(),
            }],
            default_role: VIEWER.into(),
        };
        let d = doc(Some(a.clone()));
        let rs = roles_in(&d, &[], "bob");
        assert_eq!(rs.on(&["eu".into()]), OPERATOR);
        assert_eq!(rs.on(&[]), VIEWER);
        assert_eq!(rs.top(), OPERATOR);
        assert_eq!(roles_in(&d, &[], "carol").cluster, VIEWER);
        assert!(a.check(&d).is_ok());
        let mut bad = a.clone();
        bad.users[0].groups.insert("nope".into(), VIEWER.into());
        assert!(matches!(bad.check(&d), Err(Refusal::Field(f)) if f.field == "users[0].groups"));
        let back = Access::from_value(&json::parse(a.json().as_bytes()).unwrap());
        assert_eq!(back, a);
        let mut d2 = d.clone();
        rename_group(&mut d2, "eu", "europe");
        assert_eq!(d2.access.unwrap().users[0].groups["europe"], OPERATOR);
        let f = Forbidden {
            user: "bob".into(),
            role: ADMIN.into(),
            cluster: true,
            ..Forbidden::default()
        };
        assert!(f.message().contains("needs the role admin cluster-wide"));
        assert_eq!(
            describe_change(None, &a),
            "roles set for the first time; the roles of bob; the default role viewer"
        );
    }
}
