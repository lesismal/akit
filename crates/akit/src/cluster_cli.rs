//! `akit cluster` (I139c), as fit's `fit cluster` (`cli/cluster.go`, its I150): a running
//! manager's members, states, groups, diffs, certificates, series and logs, and its rollouts
//! planned, started, followed and steered, through the manager's Admin API (`/cluster/...`). It
//! reaches the API as `akit token` does ([`crate::cli::token::Api`]): the bootstrap config's
//! `admin`, its token and its TLS. The Admin API acts as a cluster-wide admin, and the manager's
//! audit log records its actions as "Admin API".
//!
//! A rollout followed, and logs followed with `-f`, are read again at an interval (a second, and
//! two): the manager offers no stream of them, and reading a remote state at an interval is what
//! following it is.

use std::io::{BufRead, Write as _};
use std::process::ExitCode;
use std::time::{Duration, Instant};

use akio_http::Method;
use akit_cluster::rollouts as ro;
use akit_config::value::{Value, write_string};

use crate::cli::bt::table;
use crate::cli::token::{Api, Given};
use crate::flags::{self, Spec};

const USAGE: &str = "Usage: akit cluster <command> [flags]

Manage the members of a running manager of other akits (or fits), one with
admin.cluster, through its Admin API, as akit token reaches it: the bootstrap
config given with -c says where, and the token in its admin.token_file. The
Admin API acts as a cluster-wide admin, whatever the roles of the console's
users say, and the manager's audit log records its actions as \"Admin API\".

\takit cluster nodes                    the members: URL, access and labels
\takit cluster state                    each member's state, as the poller last read it
\takit cluster groups                   the groups and their members
\takit cluster diff [-node a,b | -template T]
\t                                     each member's parts that differ from its templates
\takit cluster certs [-within 30d]      every member's certificates, the soonest to expire
\t                                     first; with -within, exit 1 when one expires in it
\takit cluster metrics [-node a,b] [-group g]
\t                                     requests, 4xx and 5xx a second, connections and
\t                                     bytes queued, per member and group, at the last poll
\takit cluster logs -node a,b [-level L] [-logger L] [-contains T] [-limit N] [-f]
\t                                     the members' logs merged by time; -f follows them
\takit cluster rollout list             the rollouts the manager keeps
\takit cluster rollout start (-template T [-group G] | -sync [-group G]) [-node a,b]
\t                          [strategy] [-yes] [-wait]
\takit cluster rollout start -id ID [strategy] [-yes] [-wait]
\t                                     plan a rollout (or take one planned), show each
\t                                     member's plan, ask, and start it
\takit cluster rollout status [-wait] ID
\takit cluster rollout <pause|resume|go|abort|rollback> ID

A rollout's strategy: -mode all|rolling|canary, -batch-size N, -batch-percent P,
-pause 30s, -canary N, -on-failure batch|all, -gate-wait 5s, -max-5xx 0.01,
-window 30s, -min-requests 20, -exclude a,b.

Every command takes -c, -timeout, -json and the TLS flags of akit token.
";

const COMMON: &[Spec] = &[
    Spec {
        name: "c",
        bool: false,
        usage: "the bootstrap config file, or a directory of them, the manager was started with, whose admin says where its Admin API is (akit.json when none is given); give -c again for more, which are merged",
    },
    Spec {
        name: "timeout",
        bool: false,
        usage: "how long akit cluster waits for each answer of the Admin API (default 1m0s)",
    },
    Spec {
        name: "ca",
        bool: false,
        usage: "with admin.cert_file, a PEM file with the CAs the Admin API's certificate is verified with",
    },
    Spec {
        name: "server-name",
        bool: false,
        usage: "with admin.cert_file, the name the Admin API's certificate is verified for",
    },
    Spec {
        name: "cert",
        bool: false,
        usage: "with admin.client_ca_file, a PEM file with the client certificate akit cluster presents",
    },
    Spec {
        name: "key",
        bool: false,
        usage: "the private key of -cert",
    },
    Spec {
        name: "json",
        bool: true,
        usage: "print the Admin API's answer as JSON",
    },
];

const DIFF: &[Spec] = &[
    Spec {
        name: "node",
        bool: false,
        usage: "the members to compare, separated by commas; every member when left out",
    },
    Spec {
        name: "template",
        bool: false,
        usage: "the members of a template, in place of -node",
    },
];

const CERTS: &[Spec] = &[Spec {
    name: "within",
    bool: false,
    usage: "exit 1 when a certificate expires within this, as 30d or 72h",
}];

const METRICS: &[Spec] = &[
    Spec {
        name: "node",
        bool: false,
        usage: "the members, separated by commas",
    },
    Spec {
        name: "group",
        bool: false,
        usage: "the groups, separated by commas; every member and group when neither is given",
    },
];

const LOGS: &[Spec] = &[
    Spec {
        name: "node",
        bool: false,
        usage: "the members whose logs to tail, separated by commas (required)",
    },
    Spec {
        name: "level",
        bool: false,
        usage: "the least level: debug, info, warn or error",
    },
    Spec {
        name: "logger",
        bool: false,
        usage: "only this logger's messages, as server or tls",
    },
    Spec {
        name: "contains",
        bool: false,
        usage: "only messages with this text, case ignored",
    },
    Spec {
        name: "limit",
        bool: false,
        usage: "at most this many messages from each member, the latest",
    },
    Spec {
        name: "f",
        bool: true,
        usage: "follow: print what comes next, until interrupted",
    },
];

const WAIT: &[Spec] = &[Spec {
    name: "wait",
    bool: true,
    usage: "follow the rollout until it ends, or waits for a person's go",
}];

const START: &[Spec] = &[
    Spec {
        name: "id",
        bool: false,
        usage: "start the rollout planned already with this ID, in place of planning one",
    },
    Spec {
        name: "template",
        bool: false,
        usage: "plan the rollout of this template's change to its members",
    },
    Spec {
        name: "sync",
        bool: true,
        usage: "plan a sync: members brought in sync with their templates",
    },
    Spec {
        name: "group",
        bool: false,
        usage: "the members of this group (a template's must be its groups')",
    },
    Spec {
        name: "node",
        bool: false,
        usage: "these members, separated by commas",
    },
    Spec {
        name: "description",
        bool: false,
        usage: "what the rollout is for, as the pages show it",
    },
    Spec {
        name: "exclude",
        bool: false,
        usage: "members of the plan to leave out, separated by commas",
    },
    Spec {
        name: "mode",
        bool: false,
        usage: "all, rolling or canary (default \"all\")",
    },
    Spec {
        name: "batch-size",
        bool: false,
        usage: "rolling and canary: members a batch",
    },
    Spec {
        name: "batch-percent",
        bool: false,
        usage: "rolling and canary: a batch as a percentage of the members",
    },
    Spec {
        name: "pause",
        bool: false,
        usage: "rolling and canary: the pause between batches",
    },
    Spec {
        name: "canary",
        bool: false,
        usage: "canary: the members of the first batch, after which the rollout waits for akit cluster rollout go",
    },
    Spec {
        name: "on-failure",
        bool: false,
        usage: "what a failed gate rolls back: batch or all (default \"batch\")",
    },
    Spec {
        name: "gate-wait",
        bool: false,
        usage: "how long after a batch its gate checks its members (default 5s)",
    },
    Spec {
        name: "max-5xx",
        bool: false,
        usage: "fail the gate when a member's share of 5xx over -window is above this, as 0.01",
    },
    Spec {
        name: "window",
        bool: false,
        usage: "with -max-5xx, the window its share is judged over (30s when left out)",
    },
    Spec {
        name: "min-requests",
        bool: false,
        usage: "with -max-5xx, the requests a window needs to be judged (20 when left out)",
    },
    Spec {
        name: "yes",
        bool: true,
        usage: "start without asking, once the plan is shown",
    },
];

/// How long `rollout start` waits for a plan.
const PLAN_WAIT: Duration = Duration::from_secs(300);

fn usage_error(cmd: &str, msg: &str) -> ExitCode {
    eprintln!("akit cluster {cmd}: {msg}");
    ExitCode::from(2)
}

/// A list separated by commas, without empty items.
fn split_list(s: &str) -> Vec<String> {
    s.split(',')
        .map(str::trim)
        .filter(|p| !p.is_empty())
        .map(str::to_owned)
        .collect()
}

/// A duration, or whole days as `30d`: fit's `parseDays`.
fn parse_days(s: &str) -> Result<Duration, String> {
    if let Some(d) = s.strip_suffix('d') {
        return match d.parse::<u64>() {
            Ok(n) if d.bytes().all(|c| c.is_ascii_digit()) => {
                Ok(Duration::from_secs(n.saturating_mul(86_400)))
            }
            _ => Err(format!("{s:?} is not a number of days, as 30d")),
        };
    }
    flags::parse_duration(s)
        .ok_or_else(|| format!("{s:?} is not a duration, as 72h, or days, as 30d"))
}

fn enc(s: &str) -> String {
    let mut out = String::new();
    for c in s.bytes() {
        if c.is_ascii_alphanumeric() || matches!(c, b'-' | b'_' | b'.' | b'~') {
            out.push(char::from(c));
        } else {
            out.push_str(&format!("%{c:02X}"));
        }
    }
    out
}

fn sv(v: &Value, k: &str) -> String {
    match v.get(k) {
        Some(Value::String(s)) => s.clone(),
        _ => String::new(),
    }
}

fn nv(v: &Value, k: &str) -> f64 {
    match v.get(k) {
        Some(Value::Number(n)) => n.parse().unwrap_or(0.0),
        _ => 0.0,
    }
}

fn iv(v: &Value, k: &str) -> i64 {
    match v.get(k) {
        Some(Value::Number(n)) => n.parse().unwrap_or(0),
        _ => 0,
    }
}

fn list<'v>(v: &'v Value, k: &str) -> &'v [Value] {
    match v.get(k) {
        Some(Value::Array(a)) => a,
        _ => &[],
    }
}

fn strings(v: &Value, k: &str) -> Vec<String> {
    list(v, k)
        .iter()
        .filter_map(|x| match x {
            Value::String(s) => Some(s.clone()),
            _ => None,
        })
        .collect()
}

fn dash(s: &str) -> String {
    if s.is_empty() {
        "-".to_owned()
    } else {
        s.to_owned()
    }
}

/// `s` on one line, its runs of white space one space, cut to `n` bytes.
fn one_line(s: &str, n: usize) -> String {
    let s = s.split_whitespace().collect::<Vec<_>>().join(" ");
    if s.len() <= n {
        return s;
    }
    let mut cut = n;
    while !s.is_char_boundary(cut) {
        cut = cut.saturating_sub(1);
    }
    format!("{}…", s.get(..cut).unwrap_or(""))
}

fn print_raw(out: &str) {
    print!("{out}");
    if !out.ends_with('\n') {
        println!();
    }
}

/// The date of an RFC 3339 time, its first ten characters.
fn date_of(t: &str) -> String {
    t.get(..10).unwrap_or(t).to_owned()
}

/// `akit cluster <cmd> [flags]`.
pub(crate) fn cluster(args: &[String]) -> ExitCode {
    let Some((cmd, rest)) = args.split_first() else {
        eprint!("{USAGE}");
        return ExitCode::from(2);
    };
    let cmd = cmd.as_str();
    let specs: Vec<Spec> = match cmd {
        "-h" | "-help" | "--help" | "help" => {
            print!("{USAGE}");
            return ExitCode::SUCCESS;
        }
        "rollout" => return rollout(rest),
        "nodes" | "state" | "groups" => COMMON.to_vec(),
        "diff" => [COMMON, DIFF].concat(),
        "certs" => [COMMON, CERTS].concat(),
        "metrics" => [COMMON, METRICS].concat(),
        "logs" => [COMMON, LOGS].concat(),
        other => {
            eprint!("akit cluster: unknown command {other:?}\n\n{USAGE}");
            return ExitCode::from(2);
        }
    };
    let given = match flags::parse(&format!("cluster {cmd}"), rest, &specs) {
        Ok(f) => Given(f),
        Err(u) => {
            eprintln!("{}", u.0);
            return ExitCode::from(2);
        }
    };
    let mut within = None;
    match cmd {
        "logs" if given.get("node").is_empty() => {
            return usage_error(cmd, "-node is required");
        }
        "diff" if !given.get("node").is_empty() && !given.get("template").is_empty() => {
            return usage_error(cmd, "give -node or -template, not both");
        }
        "certs" if !given.get("within").is_empty() => match parse_days(given.get("within")) {
            Ok(d) => within = Some(d),
            Err(e) => return usage_error(cmd, &format!("-within: {e}")),
        },
        "logs" if !given.get("limit").is_empty() && given.get("limit").parse::<u32>().is_err() => {
            return usage_error(
                cmd,
                &format!("invalid value {:?} for flag -limit", given.get("limit")),
            );
        }
        _ => {}
    }
    let Some(c) = Client::new(cmd, &given) else {
        return ExitCode::from(2);
    };
    let code = match cmd {
        "nodes" => c.nodes(),
        "state" => c.state(),
        "groups" => c.groups(),
        "diff" => c.diff(&split_list(given.get("node")), given.get("template")),
        "certs" => c.certs(within),
        "metrics" => c.metrics(
            &split_list(given.get("node")),
            &split_list(given.get("group")),
        ),
        _ => {
            let mut q = vec![format!("node={}", enc(given.get("node")))];
            for k in ["level", "logger", "contains"] {
                if !given.get(k).is_empty() {
                    q.push(format!("{k}={}", enc(given.get(k))));
                }
            }
            match given.get("limit") {
                "" | "0" => {}
                l => q.push(format!("limit={l}")),
            }
            c.logs(q, given.get("f") == "true")
        }
    };
    c.api.close();
    code
}

/// The manager's Admin API, as one command calls it.
struct Client {
    api: Api,
    cmd: String,
    json: bool,
}

impl Client {
    fn new(cmd: &str, given: &Given) -> Option<Client> {
        let timeout = match given.get("timeout") {
            "" => Duration::from_secs(60),
            t => match flags::parse_duration(t) {
                Some(d) => d,
                None => {
                    eprintln!("akit cluster {cmd}: invalid value {t:?} for flag -timeout");
                    return None;
                }
            },
        };
        let paths = crate::config_paths(&given.0);
        match Api::new(&paths, given, timeout) {
            Ok(api) => Some(Client {
                api,
                cmd: cmd.to_owned(),
                json: given.get("json") == "true",
            }),
            Err(e) => {
                eprintln!(
                    "akit cluster {cmd}: {}",
                    e.replace("akit token", "akit cluster")
                );
                None
            }
        }
    }

    /// Sends a request and returns a 2xx answer's body and its JSON, or says why it failed.
    fn call(
        &self,
        method: Method,
        path: &str,
        body: Option<String>,
    ) -> Result<(String, Value), String> {
        let (status, out) = self.api.call(
            method,
            &format!("{}{path}", akit_cluster::ADMIN_API_PREFIX),
            body,
        )?;
        if status == 404 && out.contains("admin.cluster is not set") {
            return Err(
                "the akit the config names manages no members: its admin sets no cluster"
                    .to_owned(),
            );
        }
        let v = akit_config::value::parse(&out).ok();
        if !(200..300).contains(&status) {
            let msg = v
                .as_ref()
                .map(|v| sv(v, "error"))
                .filter(|m| !m.is_empty())
                .unwrap_or_else(|| out.trim().to_owned());
            return Err(format!("the Admin API answered {status}: {msg}"));
        }
        match v {
            Some(v) => Ok((out, v)),
            None => Err("the Admin API's answer is not the JSON expected".to_owned()),
        }
    }

    fn fail(&self, e: &str) -> ExitCode {
        eprintln!("akit cluster {}: {e}", self.cmd);
        ExitCode::from(1)
    }

    fn read_only_note(&self, v: &Value) {
        let why = sv(v, "read_only");
        if !why.is_empty() {
            eprintln!("akit cluster: the manager is read-only: {why}");
        }
    }

    fn nodes(&self) -> ExitCode {
        let (out, v) = match self.call(Method::Get, "/nodes", None) {
            Ok(r) => r,
            Err(e) => return self.fail(&e),
        };
        if self.json {
            print_raw(&out);
            return ExitCode::SUCCESS;
        }
        self.read_only_note(&v);
        let nodes = list(&v, "nodes");
        if nodes.is_empty() {
            println!("no members yet");
            return ExitCode::SUCCESS;
        }
        let mut rows = vec![vec![
            "NAME".into(),
            "URL".into(),
            "ACCESS".into(),
            "LABELS".into(),
        ]];
        for n in nodes {
            let mut labels: Vec<String> = match n.get("labels") {
                Some(Value::Object(m)) => m
                    .iter()
                    .map(|(k, v)| {
                        format!(
                            "{k}={}",
                            match v {
                                Value::String(s) => s.as_str(),
                                _ => "",
                            }
                        )
                    })
                    .collect(),
                _ => Vec::new(),
            };
            labels.sort();
            rows.push(vec![
                sv(n, "name"),
                dash(&sv(n, "url")),
                sv(n, "access"),
                dash(&labels.join(",")),
            ]);
        }
        print!("{}", table(&rows));
        ExitCode::SUCCESS
    }

    /// Each member's state; exits 1 when one is not up.
    fn state(&self) -> ExitCode {
        let (out, v) = match self.call(Method::Get, "/state", None) {
            Ok(r) => r,
            Err(e) => return self.fail(&e),
        };
        let nodes = list(&v, "nodes");
        let code = if nodes.iter().all(|s| sv(s, "status") == "up") {
            ExitCode::SUCCESS
        } else {
            ExitCode::from(1)
        };
        if self.json {
            print_raw(&out);
            return code;
        }
        self.read_only_note(&v);
        if nodes.is_empty() {
            println!("no members yet");
            return ExitCode::SUCCESS;
        }
        let mut rows = vec![
            [
                "NAME",
                "STATUS",
                "VERSION",
                "CONFIG",
                "DRIFT",
                "NEAREST CERT",
                "CONNS",
                "LATENCY",
                "PROBLEM",
            ]
            .map(str::to_owned)
            .to_vec(),
        ];
        for s in nodes {
            let version = match s.get("node") {
                // An akit's version, or a fit's.
                Some(n) => match sv(n, "akit_version") {
                    a if a.is_empty() => dash(&sv(n, "fit_version")),
                    a => a,
                },
                None => "-".to_owned(),
            };
            let cfg = s
                .get("config")
                .map_or("-".to_owned(), |c| format!("v{}", iv(c, "version")));
            let drift = s
                .get("drift")
                .map_or("-".to_owned(), |d| iv(d, "count").to_string());
            let cert = s
                .get("certificates")
                .map(|c| sv(c, "nearest_expiry"))
                .filter(|t| !t.is_empty() && !t.starts_with("0001-"))
                .map_or("-".to_owned(), |t| date_of(&t));
            let conns = s
                .get("connections")
                .map_or("-".to_owned(), |c| iv(c, "open").to_string());
            let prob = s.get("error").map_or("-".to_owned(), |e| {
                format!("{}: {}", sv(e, "kind"), one_line(&sv(e, "message"), 80))
            });
            rows.push(vec![
                sv(s, "name"),
                sv(s, "status"),
                version,
                cfg,
                drift,
                cert,
                conns,
                format!("{}ms", iv(s, "latency_ms")),
                prob,
            ]);
        }
        print!("{}", table(&rows));
        code
    }

    fn groups(&self) -> ExitCode {
        let (out, v) = match self.call(Method::Get, "/groups", None) {
            Ok(r) => r,
            Err(e) => return self.fail(&e),
        };
        if self.json {
            print_raw(&out);
            return ExitCode::SUCCESS;
        }
        let groups = list(&v, "groups");
        if groups.is_empty() {
            println!("no groups yet");
            return ExitCode::SUCCESS;
        }
        let mut rows = vec![
            ["NAME", "SELECTOR", "MEMBERS", "MISSING"]
                .map(str::to_owned)
                .to_vec(),
        ];
        for g in groups {
            rows.push(vec![
                sv(g, "name"),
                dash(&sv(g, "selector")),
                dash(&strings(g, "nodes").join(",")),
                dash(&strings(g, "missing").join(",")),
            ]);
        }
        print!("{}", table(&rows));
        ExitCode::SUCCESS
    }

    /// Each member's parts not in sync with its templates; exits 1 when one is not.
    fn diff(&self, nodes: &[String], template: &str) -> ExitCode {
        let mut body = String::from("{\"nodes\":[");
        for (i, n) in nodes.iter().enumerate() {
            if i > 0 {
                body.push(',');
            }
            write_string(&mut body, n);
        }
        body.push_str("],\"template\":");
        write_string(&mut body, template);
        body.push('}');
        let (out, v) = match self.call(Method::Post, "/sync", Some(body)) {
            Ok(r) => r,
            Err(e) => return self.fail(&e),
        };
        let synced = list(&v, "nodes");
        let pending = strings(&v, "pending");
        let mut code = ExitCode::SUCCESS;
        if !pending.is_empty()
            || synced
                .iter()
                .any(|s| !matches!(sv(s, "state").as_str(), "in_sync" | "unmanaged"))
        {
            code = ExitCode::from(1);
        }
        if self.json {
            print_raw(&out);
            return code;
        }
        for s in synced {
            print!("{}: {}", sv(s, "node"), sv(s, "state"));
            if let Some(e) = s.get("error") {
                print!(" ({}: {})", sv(e, "kind"), one_line(&sv(e, "message"), 120));
            }
            println!();
            for e in strings(s, "errors") {
                println!("  error: {}", one_line(&e, 200));
            }
            for p in list(s, "parts") {
                if sv(p, "state") == "in_sync" {
                    continue;
                }
                print!(
                    "  {} {} (template {})",
                    sv(p, "state"),
                    sv(p, "path"),
                    sv(p, "template")
                );
                let reason = sv(p, "reason");
                if !reason.is_empty() {
                    print!(": {}", one_line(&reason, 120));
                }
                println!();
                for (k, sign) in [("have", '-'), ("want", '+')] {
                    if let Some(x) = p.get(k) {
                        println!("    {sign} {}", one_line(x.to_json().trim(), 200));
                    }
                }
            }
        }
        for n in pending {
            println!("{n}: not read in time");
        }
        code
    }

    /// The inventory; with `within`, exits 1 when a certificate expires in it.
    fn certs(&self, within: Option<Duration>) -> ExitCode {
        let (out, v) = match self.call(Method::Get, "/certificates", None) {
            Ok(r) => r,
            Err(e) => return self.fail(&e),
        };
        let certs = list(&v, "certificates");
        let mut code = ExitCode::SUCCESS;
        if let Some(w) = within {
            let now = crate::cli::token::parse_rfc3339(&sv(&v, "now")).unwrap_or(0);
            let limit = i64::try_from(w.as_secs()).unwrap_or(i64::MAX);
            if certs.iter().any(|c| {
                crate::cli::token::parse_rfc3339(&sv(c, "not_after"))
                    .is_some_and(|t| t.saturating_sub(now) < limit)
            }) {
                code = ExitCode::from(1);
            }
        }
        if self.json {
            print_raw(&out);
            return code;
        }
        if certs.is_empty() {
            println!("no member serves a certificate, as last read");
        } else {
            let mut rows = vec![
                ["EXPIRES", "DAYS", "NODE", "LISTENER", "NAMES", "ISSUER"]
                    .map(str::to_owned)
                    .to_vec(),
            ];
            for c in certs {
                rows.push(vec![
                    sv(c, "not_after"),
                    iv(c, "days_left").to_string(),
                    sv(c, "node"),
                    dash(&sv(c, "listener")),
                    dash(&strings(c, "names").join(",")),
                    dash(&sv(c, "issuer")),
                ]);
            }
            print!("{}", table(&rows));
        }
        for n in list(&v, "nodes") {
            if n.get("read").is_none() {
                println!(
                    "{}: its certificates were not read yet ({})",
                    sv(n, "name"),
                    sv(n, "status")
                );
            }
        }
        code
    }

    fn metrics(&self, nodes: &[String], groups: &[String]) -> ExitCode {
        let mut q = String::from("?points=2");
        if !nodes.is_empty() {
            q.push_str(&format!("&node={}", enc(&nodes.join(","))));
        }
        if !groups.is_empty() {
            q.push_str(&format!("&group={}", enc(&groups.join(","))));
        }
        let (out, v) = match self.call(Method::Get, &format!("/metrics{q}"), None) {
            Ok(r) => r,
            Err(e) => return self.fail(&e),
        };
        if self.json {
            print_raw(&out);
            return ExitCode::SUCCESS;
        }
        let last = |s: &Value, k: &str, prec: usize| match list(s, k).last() {
            Some(Value::Number(n)) => format!("{:.prec$}", n.parse::<f64>().unwrap_or(0.0)),
            _ => "-".to_owned(),
        };
        let mut rows = vec![
            [
                "NODE",
                "STATUS",
                "REQ/S",
                "4XX/S",
                "5XX/S",
                "CONNS",
                "QUEUED BYTES",
                "REQUESTS",
            ]
            .map(str::to_owned)
            .to_vec(),
        ];
        for n in list(&v, "nodes") {
            let s = n.get("series").cloned().unwrap_or(Value::Null);
            let total = n
                .get("counters")
                .and_then(|c| c.get("requests"))
                .map_or("-".to_owned(), |r| format!("{}", nv(r, "total")));
            rows.push(vec![
                sv(n, "name"),
                sv(n, "status"),
                last(&s, "requests", 2),
                last(&s, "errors_4xx", 2),
                last(&s, "errors_5xx", 2),
                last(&s, "connections", 0),
                last(&s, "pending_bytes", 0),
                total,
            ]);
        }
        for g in list(&v, "groups") {
            let s = g.get("series").cloned().unwrap_or(Value::Null);
            rows.push(vec![
                format!("group {}", sv(g, "name")),
                format!("{} members", list(g, "nodes").len()),
                last(&s, "requests", 2),
                last(&s, "errors_4xx", 2),
                last(&s, "errors_5xx", 2),
                last(&s, "connections", 0),
                last(&s, "pending_bytes", 0),
                "-".to_owned(),
            ]);
        }
        print!("{}", table(&rows));
        ExitCode::SUCCESS
    }

    /// The members' logs merged by time, and with `follow` what comes next, read every two
    /// seconds until akit cluster is interrupted; exits 1 when a member could not be read.
    fn logs(&self, mut q: Vec<String>, follow: bool) -> ExitCode {
        let mut code = ExitCode::SUCCESS;
        let mut reported: Vec<(String, String)> = Vec::new();
        loop {
            let (out, v) = match self.call(Method::Get, &format!("/logs?{}", q.join("&")), None) {
                Ok(r) => r,
                Err(e) => return self.fail(&e),
            };
            if self.json {
                print_raw(&out);
            } else {
                let mut so = std::io::stdout().lock();
                for r in list(&v, "records") {
                    let _ = writeln!(
                        so,
                        "{} {} {} {}: {}",
                        sv(r, "time"),
                        sv(r, "node"),
                        sv(r, "level").to_uppercase(),
                        sv(r, "logger"),
                        sv(r, "message")
                    );
                }
                let _ = so.flush();
            }
            for n in list(&v, "nodes") {
                let name = sv(n, "name");
                let msg = n.get("error").map_or(String::new(), |e| {
                    format!("{}: {}", sv(e, "kind"), one_line(&sv(e, "message"), 200))
                });
                if !msg.is_empty() {
                    code = ExitCode::from(1);
                }
                let before = reported.iter().position(|(n2, _)| *n2 == name);
                let same = before
                    .and_then(|i| reported.get(i))
                    .is_some_and(|(_, m)| *m == msg);
                if !same {
                    if !msg.is_empty() {
                        eprintln!("akit cluster logs: {name}: {msg}");
                    }
                    match before.and_then(|i| reported.get_mut(i)) {
                        Some(slot) => slot.1 = msg,
                        None => reported.push((name, msg)),
                    }
                }
            }
            if !follow {
                return code;
            }
            q.retain(|p| !p.starts_with("cursor="));
            q.push(format!("cursor={}", enc(&sv(&v, "cursor"))));
            std::thread::sleep(Duration::from_secs(2));
        }
    }
}

// ---- Rollouts ----

fn rollout(args: &[String]) -> ExitCode {
    let Some((sub, rest)) = args.split_first() else {
        eprint!("{USAGE}");
        return ExitCode::from(2);
    };
    let sub = sub.as_str();
    let specs: Vec<Spec> = match sub {
        "-h" | "-help" | "--help" => {
            print!("{USAGE}");
            return ExitCode::SUCCESS;
        }
        "start" => [COMMON, WAIT, START].concat(),
        "list" | "status" | "pause" | "resume" | "go" | "abort" | "rollback" => {
            [COMMON, WAIT].concat()
        }
        other => {
            eprint!("akit cluster rollout: unknown command {other:?}\n\n{USAGE}");
            return ExitCode::from(2);
        }
    };
    let cmd = format!("rollout {sub}");
    let (given, names) = match flags::parse_args(&format!("cluster {cmd}"), rest, &specs) {
        Ok((f, n)) => (Given(f), n),
        Err(u) => {
            eprintln!("{}", u.0);
            return ExitCode::from(2);
        }
    };
    let id = match (sub, names.as_slice()) {
        ("list" | "start", []) => String::new(),
        ("list" | "start", [a, ..]) | (_, [_, a, ..]) => {
            return usage_error(&cmd, &format!("unexpected argument {a:?}"));
        }
        (_, []) => {
            return usage_error(
                &cmd,
                "give the rollout's ID, as akit cluster rollout list shows it",
            );
        }
        (_, [id]) => id.clone(),
    };
    let start = if sub == "start" {
        match requests(&given) {
            Ok(r) => Some(r),
            Err(e) => return usage_error(&cmd, &e),
        }
    } else {
        None
    };
    let Some(c) = Client::new(&cmd, &given) else {
        return ExitCode::from(2);
    };
    let wait = given.get("wait") == "true";
    let code = match (sub, start) {
        ("list", _) => c.rollout_list(),
        ("status", _) => c.rollout_status(&id, wait),
        ("start", Some((plan, start))) => c.rollout_start(
            given.get("id"),
            plan,
            &start,
            &split_list(given.get("exclude")),
            given.get("yes") == "true",
            wait,
        ),
        _ => {
            let path = format!("/rollouts/{}/{sub}", enc(&id));
            match c.call(Method::Post, &path, None) {
                Err(e) => c.fail(&e),
                Ok((out, r)) => {
                    if c.json && !wait {
                        print_raw(&out);
                    } else if !c.json {
                        println!("rollout {}: {}", sv(&r, "id"), sv(&r, "state"));
                    }
                    if wait {
                        c.rollout_status(&id, true)
                    } else {
                        ExitCode::SUCCESS
                    }
                }
            }
        }
    };
    c.api.close();
    code
}

/// `rollout start`'s plan request, `None` for `-id`, and its start request: fit's `requests`.
fn requests(g: &Given) -> Result<(Option<String>, String), String> {
    let num = |name: &str| -> Result<i64, String> {
        match g.get(name) {
            "" => Ok(0),
            v => v
                .parse::<i64>()
                .map_err(|_| format!("invalid value {v:?} for flag -{name}")),
        }
    };
    let dur = |name: &str, default: Duration| -> Result<u128, String> {
        match g.get(name) {
            "" => Ok(default.as_millis()),
            v => flags::parse_duration(v)
                .map(|d| d.as_millis())
                .ok_or_else(|| format!("invalid value {v:?} for flag -{name}")),
        }
    };
    let max5xx = match g.get("max-5xx") {
        "" => 0.0,
        v => match v.parse::<f64>() {
            Ok(f) if f.is_finite() => f,
            _ => return Err(format!("invalid value {v:?} for flag -max-5xx")),
        },
    };
    let mode = match g.get("mode") {
        "" => ro::ALL,
        m => m,
    };
    let on_failure = match g.get("on-failure") {
        "" => ro::ON_FAILURE_BATCH,
        m => m,
    };
    let (batch_size, batch_percent, canary, min_requests) = (
        num("batch-size")?,
        num("batch-percent")?,
        num("canary")?,
        num("min-requests")?,
    );
    let gate_wait = dur(
        "gate-wait",
        Duration::from_millis(ro::DEFAULT_GATE_WAIT_MS.unsigned_abs()),
    )?;
    let (pause, window) = (
        dur("pause", Duration::ZERO)?,
        dur("window", Duration::ZERO)?,
    );
    let mut s = String::from("{\"strategy\":{\"mode\":");
    write_string(&mut s, mode);
    for (k, v) in [
        ("batch_size", i128::from(batch_size)),
        ("batch_percent", i128::from(batch_percent)),
        ("pause_ms", i128::try_from(pause).unwrap_or(i128::MAX)),
        ("canary", i128::from(canary)),
    ] {
        if v != 0 {
            s.push_str(&format!(",\"{k}\":{v}"));
        }
    }
    s.push_str(",\"on_failure\":");
    write_string(&mut s, on_failure);
    s.push_str(&format!(",\"gate\":{{\"wait_ms\":{gate_wait}"));
    if max5xx != 0.0 {
        s.push_str(&format!(",\"max_5xx_ratio\":{max5xx}"));
    }
    if window != 0 {
        s.push_str(&format!(",\"window_ms\":{window}"));
    }
    if min_requests != 0 {
        s.push_str(&format!(",\"min_requests\":{min_requests}"));
    }
    s.push_str("}}");
    let exclude = split_list(g.get("exclude"));
    if !exclude.is_empty() {
        s.push_str(",\"exclude\":[");
        for (i, e) in exclude.iter().enumerate() {
            if i > 0 {
                s.push(',');
            }
            write_string(&mut s, e);
        }
        s.push(']');
    }
    s.push('}');
    let (id, template, sync) = (g.get("id"), g.get("template"), g.get("sync") == "true");
    let kinds = [!id.is_empty(), !template.is_empty(), sync]
        .iter()
        .filter(|x| **x)
        .count();
    if kinds != 1 {
        return Err("give one of -template, -sync and -id".to_owned());
    }
    if !id.is_empty() {
        if !g.get("group").is_empty()
            || !g.get("node").is_empty()
            || !g.get("description").is_empty()
        {
            return Err(
                "-id starts a rollout planned already: -group, -node and -description plan one"
                    .to_owned(),
            );
        }
        return Ok((None, s));
    }
    let mut p = String::from("{\"kind\":");
    if template.is_empty() {
        write_string(&mut p, ro::KIND_SYNC);
    } else {
        write_string(&mut p, ro::KIND_TEMPLATE);
        p.push_str(",\"template\":");
        write_string(&mut p, template);
    }
    for (k, v) in [
        ("group", g.get("group")),
        ("description", g.get("description")),
    ] {
        if !v.is_empty() {
            p.push_str(&format!(",\"{k}\":"));
            write_string(&mut p, v);
        }
    }
    let nodes = split_list(g.get("node"));
    if !nodes.is_empty() {
        p.push_str(",\"nodes\":[");
        for (i, n) in nodes.iter().enumerate() {
            if i > 0 {
                p.push(',');
            }
            write_string(&mut p, n);
        }
        p.push(']');
    }
    p.push('}');
    Ok((Some(p), s))
}

/// Whether a rollout has ended, or waits for a person.
fn rollout_over(state: &str) -> bool {
    !matches!(state, ro::PLANNING | ro::RUNNING | ro::ROLLING_BACK)
}

/// The exit code a rollout's state means.
fn rollout_code(state: &str) -> ExitCode {
    match state {
        ro::FAILED | ro::ABORTED | ro::ROLLED_BACK | ro::EXPIRED => ExitCode::from(1),
        _ => ExitCode::SUCCESS,
    }
}

fn counts_text(c: Option<&Value>) -> String {
    let mut parts: Vec<(String, i64)> = match c {
        Some(Value::Object(m)) => m
            .iter()
            .map(|(k, v)| {
                (
                    k.clone(),
                    match v {
                        Value::Number(n) => n.parse().unwrap_or(0),
                        _ => 0,
                    },
                )
            })
            .collect(),
        _ => Vec::new(),
    };
    parts.sort();
    dash(
        &parts
            .iter()
            .map(|(k, n)| format!("{n} {k}"))
            .collect::<Vec<_>>()
            .join(", "),
    )
}

/// A rollout and each member's plan or outcome.
fn print_rollout(r: &Value) {
    let mut head = format!("rollout {} ({}", sv(r, "id"), sv(r, "kind"));
    let template = sv(r, "template");
    if !template.is_empty() {
        head.push_str(&format!(" of {template}"));
    }
    let group = sv(r, "group");
    if !group.is_empty() {
        head.push_str(&format!(", group {group}"));
    }
    head.push_str(&format!("): {}", sv(r, "state")));
    let reason = sv(r, "reason");
    if !reason.is_empty() {
        head.push_str(&format!(": {}", one_line(&reason, 200)));
    }
    println!("{head}");
    if sv(r, "state") == ro::WAITING {
        println!(
            "the canary passed its gate: go on with akit cluster rollout go {}, or abort it",
            sv(r, "id")
        );
    }
    let mut rows = vec![
        ["NODE", "STATUS", "VERSION", "CHANGES", "NOTE"]
            .map(str::to_owned)
            .to_vec(),
    ];
    for x in list(r, "members") {
        let mut changed = strings(x, "changed");
        for ch in list(x, "changes") {
            let p = sv(ch, "path");
            if !changed.contains(&p) {
                changed.push(p);
            }
        }
        let (version, applied, rolled_back) =
            (iv(x, "version"), iv(x, "applied"), iv(x, "rolled_back"));
        let mut ver = match (applied, version) {
            (a, v) if a != 0 => format!("v{v} → v{a}"),
            (_, v) if v != 0 => format!("v{v}"),
            _ => "-".to_owned(),
        };
        if rolled_back != 0 {
            ver.push_str(&format!(" → v{rolled_back} (rolled back)"));
        }
        let mut notes = Vec::new();
        for e in list(x, "errors") {
            notes.push(format!("{}: {}", dash(&sv(e, "path")), sv(e, "message")));
        }
        if let Some(e) = x.get("error") {
            notes.push(sv(e, "message"));
        }
        if let Some(g) = x.get("gate") {
            let reasons = strings(g, "reasons");
            if !matches!(g.get("passed"), Some(Value::Bool(true))) && !reasons.is_empty() {
                notes.push(format!("gate: {}", reasons.join("; ")));
            }
        }
        let note = sv(x, "note");
        if !note.is_empty() {
            notes.push(note);
        }
        rows.push(vec![
            sv(x, "node"),
            sv(x, "status"),
            ver,
            dash(&changed.join(",")),
            dash(&one_line(&notes.join(" | "), 160)),
        ]);
    }
    print!("{}", table(&rows));
}

impl Client {
    fn rollout_list(&self) -> ExitCode {
        let (out, v) = match self.call(Method::Get, "/rollouts", None) {
            Ok(r) => r,
            Err(e) => return self.fail(&e),
        };
        if self.json {
            print_raw(&out);
            return ExitCode::SUCCESS;
        }
        let rs = list(&v, "rollouts");
        if rs.is_empty() {
            println!("no rollouts yet");
            return ExitCode::SUCCESS;
        }
        let mut rows = vec![
            ["ID", "KIND", "STATE", "CREATED", "MEMBERS", "WHAT"]
                .map(str::to_owned)
                .to_vec(),
        ];
        for r in rs {
            let what = if sv(r, "kind") == ro::KIND_COPY {
                format!("{}: {}", sv(r, "from"), strings(r, "paths").join(","))
            } else {
                sv(r, "template")
            };
            rows.push(vec![
                sv(r, "id"),
                sv(r, "kind"),
                sv(r, "state"),
                sv(r, "created"),
                list(r, "members").len().to_string(),
                dash(&what),
            ]);
        }
        print!("{}", table(&rows));
        ExitCode::SUCCESS
    }

    /// A rollout, and with `wait` followed, read every second, until it ends.
    fn rollout_status(&self, id: &str, wait: bool) -> ExitCode {
        let mut shown = String::new();
        loop {
            let (out, r) = match self.call(Method::Get, &format!("/rollouts/{}", enc(id)), None) {
                Ok(x) => x,
                Err(e) => return self.fail(&e),
            };
            let state = sv(&r, "state");
            if wait && !self.json {
                // A line when the rollout moves on.
                let batches = list(&r, "batches").len();
                let batch = usize::try_from(iv(&r, "batch")).unwrap_or(0);
                let line = format!(
                    "rollout {}: {state}, batch {} of {batches}, {}",
                    sv(&r, "id"),
                    batch.saturating_add(1).min(batches.max(1)),
                    counts_text(r.get("counts"))
                );
                if line != shown {
                    println!("{line}");
                    shown = line;
                }
            }
            if !wait || rollout_over(&state) {
                if self.json {
                    print_raw(&out);
                } else {
                    print_rollout(&r);
                }
                return rollout_code(&state);
            }
            std::thread::sleep(Duration::from_secs(1));
        }
    }

    /// Plans a rollout, or takes the one `id` names, prints its plan, asks unless `yes`, starts
    /// it, and with `wait` follows it.
    fn rollout_start(
        &self,
        id: &str,
        plan: Option<String>,
        start: &str,
        exclude: &[String],
        yes: bool,
        wait: bool,
    ) -> ExitCode {
        let mut id = id.to_owned();
        if let Some(p) = plan {
            match self.call(Method::Post, "/rollouts", Some(p)) {
                Ok((_, r)) => id = sv(&r, "id"),
                Err(e) => return self.fail(&e),
            }
            if !self.json {
                println!("planning rollout {id}…");
            }
        }
        // The plan is read every half second until it is made: the manager says nothing when.
        let deadline = Instant::now().checked_add(PLAN_WAIT);
        let r = loop {
            let r = match self.call(Method::Get, &format!("/rollouts/{}", enc(&id)), None) {
                Ok((_, r)) => r,
                Err(e) => return self.fail(&e),
            };
            if sv(&r, "state") != ro::PLANNING {
                break r;
            }
            if deadline.is_some_and(|d| Instant::now() > d) {
                return self.fail(&format!(
                    "the rollout {id} is still being planned after 5m0s: follow it with akit \
                     cluster rollout status {id}"
                ));
            }
            std::thread::sleep(Duration::from_millis(500));
        };
        if !self.json {
            print_rollout(&r);
        }
        let state = sv(&r, "state");
        if state != ro::PLANNED {
            return self.fail(&format!(
                "the rollout {id} is {state}: only a planned rollout starts"
            ));
        }
        let ready = list(&r, "members")
            .iter()
            .filter(|x| sv(x, "status") == ro::M_READY && !exclude.contains(&sv(x, "node")))
            .count();
        if ready == 0 {
            return self.fail(&format!(
                "no member of the rollout {id} has anything to apply: nothing was started"
            ));
        }
        if !yes {
            let mode = akit_config::value::parse(start)
                .ok()
                .and_then(|v| v.get("strategy").map(|s| sv(s, "mode")))
                .unwrap_or_default();
            print!("Start the rollout {id} on {ready} members, {mode}? [y/N] ");
            let _ = std::io::stdout().flush();
            let mut line = String::new();
            let _ = std::io::stdin().lock().take_line(&mut line);
            let a = line.trim().to_lowercase();
            if a != "y" && a != "yes" {
                println!(
                    "not started: the plan is kept until it expires; start it with akit cluster \
                     rollout start -id {id}, or discard it on the Rollouts page"
                );
                return ExitCode::from(1);
            }
        }
        let out = match self.call(
            Method::Post,
            &format!("/rollouts/{}/start", enc(&id)),
            Some(start.to_owned()),
        ) {
            Ok((out, _)) => out,
            Err(e) => return self.fail(&e),
        };
        if !wait {
            if self.json {
                print_raw(&out);
            } else {
                println!(
                    "rollout {id} started: follow it with akit cluster rollout status -wait {id}"
                );
            }
            return ExitCode::SUCCESS;
        }
        self.rollout_status(&id, true)
    }
}

/// One line read, at most 1 KiB of it.
trait TakeLine {
    fn take_line(&mut self, line: &mut String) -> std::io::Result<usize>;
}

impl<R: BufRead> TakeLine for R {
    fn take_line(&mut self, line: &mut String) -> std::io::Result<usize> {
        use std::io::Read as _;
        let mut buf = Vec::new();
        let n = self.by_ref().take(1024).read_until(b'\n', &mut buf)?;
        line.push_str(&String::from_utf8_lossy(&buf));
        Ok(n)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn days_and_requests() {
        assert_eq!(
            parse_days("30d").ok(),
            Some(Duration::from_secs(30 * 86_400))
        );
        assert_eq!(parse_days("72h").ok(), Some(Duration::from_secs(72 * 3600)));
        for bad in ["-1d", "xd", "1.5d", "soon"] {
            assert!(parse_days(bad).is_err(), "{bad}");
        }
        let g = Given(vec![
            ("template", "edge".to_owned()),
            ("group", "eu".to_owned()),
            ("mode", "rolling".to_owned()),
            ("batch-size", "2".to_owned()),
            ("pause", "1s".to_owned()),
            ("max-5xx", "0.01".to_owned()),
            ("exclude", "a, b".to_owned()),
        ]);
        let (plan, start) = requests(&g).unwrap_or_default();
        assert_eq!(
            plan.as_deref(),
            Some(r#"{"kind":"template","template":"edge","group":"eu"}"#)
        );
        assert_eq!(
            start,
            r#"{"strategy":{"mode":"rolling","batch_size":2,"pause_ms":1000,"on_failure":"batch","gate":{"wait_ms":5000,"max_5xx_ratio":0.01}},"exclude":["a","b"]}"#
        );
        let sync = Given(vec![
            ("sync", "true".to_owned()),
            ("node", "a,b".to_owned()),
        ]);
        assert_eq!(
            requests(&sync).ok().and_then(|r| r.0).as_deref(),
            Some(r#"{"kind":"sync","nodes":["a","b"]}"#)
        );
        let by_id = Given(vec![("id", "r3".to_owned())]);
        assert_eq!(requests(&by_id).ok().map(|r| r.0), Some(None));
        for bad in [
            vec![],
            vec![("id", "r3".to_owned()), ("sync", "true".to_owned())],
            vec![("id", "r3".to_owned()), ("group", "g".to_owned())],
            vec![("sync", "true".to_owned()), ("canary", "x".to_owned())],
        ] {
            assert!(requests(&Given(bad)).is_err());
        }
        assert!(rollout_over(ro::WAITING) && !rollout_over(ro::RUNNING));
        assert_eq!(one_line("a\n  b", 10), "a b");
        assert_eq!(enc("a b,c"), "a%20b%2Cc");
    }
}
