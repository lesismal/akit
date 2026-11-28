//! The manager in its own Admin API (I139c, fit's `adminapi.go`, its I150), for `akit cluster`:
//! the members, their states, the groups, drift, the members' diffs against their templates,
//! the series, the certificates' inventory, the logs, and rollouts planned, started, followed
//! and steered, at `/cluster/...` behind the Admin API's token. The console does not serve them:
//! its users reach the manager through the module `cluster`, under their roles.
//!
//! Whoever holds the Admin API's token may change the manager's own config,
//! `admin.cluster.admins` included, so the API acts as a cluster-wide admin,
//! [`ADMIN_API_USER`](crate::roles::ADMIN_API_USER), whatever the roles say; its actions are
//! audited under that name. A console user of that name has no role, so that no one logs in as
//! the Admin API.
//!
//! ```text
//! GET    /cluster                         the endpoints
//! GET    /cluster/nodes                   the members
//! GET    /cluster/state                   their states
//! GET    /cluster/groups                  the groups
//! GET    /cluster/drift                   the members' drift
//! POST   /cluster/sync                    the members compared with their templates now
//! GET    /cluster/metrics                 the series
//! GET    /cluster/certificates            the certificates' inventory
//! GET    /cluster/logs                    the members' logs merged by time
//! GET    /cluster/rollouts                the rollouts; POST plans one
//! GET    /cluster/rollouts/<id>           a rollout; DELETE discards it
//! POST   /cluster/rollouts/<id>/<action>  start, pause, resume, go, abort, rollback
//! ```

use akio_http::{Context, Method, Response};
use akit_roles::pages::{fail, json as answer};

use crate::Manager;
use crate::json::Obj;
use crate::roles::ADMIN_API_USER;

/// Where the Admin API serves the manager's endpoints: fit's `AdminAPIPrefix`.
pub const ADMIN_API_PREFIX: &str = "/cluster";

/// The endpoints `GET /cluster` lists.
const ENDPOINTS: [&str; 9] = [
    "nodes",
    "state",
    "groups",
    "drift",
    "sync",
    "metrics",
    "certificates",
    "logs",
    "rollouts",
];

fn not_allowed(allow: &str) -> Response {
    fail(405, "the method is not allowed").header("Allow", allow)
}

impl Manager {
    /// Answers a request of the Admin API's `/cluster` or below it, already past the API's
    /// token; `false` for a path that is not the manager's. Called on the Admin API's I/O
    /// thread: what writes the store goes to the manager's pool, what calls members is answered
    /// when they answer.
    pub fn serve_admin_api(&self, ctx: &mut Context) -> bool {
        let path = ctx.path().to_owned();
        let Some(rest) = path.strip_prefix(ADMIN_API_PREFIX) else {
            return false;
        };
        if !rest.is_empty() && !rest.starts_with('/') {
            return false;
        }
        let method = ctx.method().clone();
        let get = matches!(method, Method::Get | Method::Head);
        let m = self;
        match rest {
            "" | "/" => {
                let list = ENDPOINTS
                    .iter()
                    .map(|e| format!("\"{e}\""))
                    .collect::<Vec<_>>()
                    .join(",");
                ctx.respond(answer(
                    200,
                    Obj::new().raw("endpoints", &format!("[{list}]")).end(),
                ));
            }
            "/nodes" if get => ctx.respond(crate::api::list(m)),
            "/state" if get => ctx.respond(crate::api::state(m)),
            "/groups" if get => {
                crate::groups::serve(m, ctx, ADMIN_API_USER, "/api/groups");
            }
            "/drift" if get => ctx.respond(answer(200, m.drift_json())),
            "/metrics" if get => {
                let q = ctx.req().query().unwrap_or("").to_owned();
                ctx.respond(crate::insight::metrics_route(m, &q));
            }
            "/certificates" if get => ctx.respond(answer(200, m.certificates())),
            "/logs" if get => crate::insight::logs_route(m, ctx, None),
            "/nodes" | "/state" | "/groups" | "/drift" | "/metrics" | "/certificates" | "/logs" => {
                ctx.respond(not_allowed("GET"));
            }
            "/sync" if method == Method::Post => crate::sync::sync_route(m, ctx, ADMIN_API_USER),
            "/sync" => ctx.respond(not_allowed("POST")),
            r if r == "/rollouts" || r.starts_with("/rollouts/") => {
                if m.0.closed.load(std::sync::atomic::Ordering::Acquire) && !get {
                    ctx.respond(fail(503, "the manager is shutting down"));
                    return true;
                }
                let (_, id_action) = r.split_at("/rollouts".len());
                let parts: Vec<&str> = id_action.trim_start_matches('/').split('/').collect();
                let ok = match (parts.as_slice(), &method) {
                    ([""], Method::Get | Method::Head | Method::Post) => true,
                    ([""], _) => {
                        ctx.respond(not_allowed("GET, POST"));
                        return true;
                    }
                    ([_], Method::Get | Method::Head | Method::Delete) => true,
                    ([_], _) => {
                        ctx.respond(not_allowed("GET, DELETE"));
                        return true;
                    }
                    ([_, a], Method::Post) => matches!(
                        *a,
                        "start" | "pause" | "resume" | "go" | "abort" | "rollback"
                    ),
                    _ => false,
                };
                if !ok {
                    ctx.respond(fail(
                        404,
                        "a rollout's actions are start, pause, resume, go, abort and rollback",
                    ));
                    return true;
                }
                let api = format!("/api{r}");
                crate::rollouts::serve(m, ctx, ADMIN_API_USER, &api);
            }
            _ => ctx.respond(fail(404, "no such endpoint of the manager's")),
        }
        true
    }
}
