//! `akit token` (I100), as fit's `fit token` (`cli/token.go`): the link tokens a running akit's
//! listeners issue, which the tunnel's nodes and proxies and the LLM mesh's peers link with,
//! managed through its Admin API (`/tokens`, fit's decision 035).
//!
//! The command reads the bootstrap config's `admin` to find the API: its address (a Unix socket
//! too), its token from `token_file`, and TLS when it has `cert_file`. Requests go out on akio's
//! HTTP client, on an engine of the command's own, and are waited for here, off any I/O thread.

use std::process::ExitCode;
use std::sync::Arc;
use std::time::Duration;

use akio_http::Method;
use akio_http::client::{Client, ClientRequest};
use akit_config::value::{Value, write_string};

use crate::flags::{self, Spec};

const USAGE: &str = "Usage: akit token <list|create|expiry|revoke> [flags]

Manage the link tokens a running akit's listeners issue, which the tunnel's
nodes and proxies and the LLM mesh's peers link with in place of a shared
password, through its Admin API. The roles that issue them are tunnel.hub,
llm.hub, llm.node and llm.proxy.

\takit token list   [-role R]                    the tokens, their expiry, last use,
\t                                               members and links
\takit token create -role R -name N [-note T] [-ttl 30d | -expires TIME]
\t                                               issue one: the token is printed on
\t                                               standard output, this once
\takit token expiry -role R -id ID (-ttl 30d | -expires TIME | -never)
\takit token revoke -role R -id ID               its links close at once

Every command takes -c, -timeout, -ca, -server-name, -cert, -key and -json.
";

const COMMON: &[Spec] = &[
    Spec {
        name: "c",
        bool: false,
        usage: "the bootstrap config file, or a directory of them, akit was started with, whose admin says where its Admin API is (akit.json when none is given); give -c again for more, which are merged",
    },
    Spec {
        name: "timeout",
        bool: false,
        usage: "how long akit token waits for the Admin API (default 1m0s)",
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
        usage: "with admin.client_ca_file, a PEM file with the client certificate akit token presents",
    },
    Spec {
        name: "key",
        bool: false,
        usage: "the private key of -cert",
    },
    Spec {
        name: "role",
        bool: false,
        usage: "the role whose listener issues the tokens: tunnel.hub, llm.hub, llm.node or llm.proxy",
    },
    Spec {
        name: "id",
        bool: false,
        usage: "the token's ID, 16 hex digits, as akit token list shows it",
    },
    Spec {
        name: "json",
        bool: true,
        usage: "print the Admin API's answer as JSON",
    },
];

const CREATE: &[Spec] = &[
    Spec {
        name: "name",
        bool: false,
        usage: "what the token is for, as the pages list it (required)",
    },
    Spec {
        name: "note",
        bool: false,
        usage: "anything else about the token",
    },
];

const EXPIRY: &[Spec] = &[
    Spec {
        name: "ttl",
        bool: false,
        usage: "how long the token is good for, from now: a duration, as 720h, or days, as 30d",
    },
    Spec {
        name: "expires",
        bool: false,
        usage: "when the token expires, as RFC 3339, as 2027-01-01T00:00:00Z",
    },
];

const NEVER: &[Spec] = &[Spec {
    name: "never",
    bool: true,
    usage: "the token never expires",
}];

/// The flags given, last one wins.
struct Given(Vec<(&'static str, String)>);

impl Given {
    fn get(&self, name: &str) -> &str {
        self.0
            .iter()
            .rev()
            .find(|(n, _)| *n == name)
            .map_or("", |(_, v)| v.as_str())
    }
}

fn usage_error(cmd: &str, msg: &str) -> ExitCode {
    eprintln!("akit token {cmd}: {msg}");
    ExitCode::from(2)
}

/// `akit token <cmd> [flags]`.
pub(crate) fn token(args: &[String]) -> ExitCode {
    let Some((cmd, rest)) = args.split_first() else {
        eprint!("{USAGE}");
        return ExitCode::from(2);
    };
    let cmd = cmd.as_str();
    if matches!(cmd, "-h" | "-help" | "--help" | "help") {
        eprint!("{USAGE}");
        return ExitCode::SUCCESS;
    }
    let specs: Vec<Spec> = match cmd {
        "create" => [COMMON, CREATE, EXPIRY].concat(),
        "expiry" => [COMMON, EXPIRY, NEVER].concat(),
        "list" | "revoke" => COMMON.to_vec(),
        other => {
            eprint!("akit token: unknown command {other:?}\n\n{USAGE}");
            return ExitCode::from(2);
        }
    };
    let given = match flags::parse(&format!("token {cmd}"), rest, &specs) {
        Ok(f) => Given(f),
        Err(u) => {
            eprintln!("{}", u.0);
            return ExitCode::from(2);
        }
    };
    let (role, id, name) = (given.get("role"), given.get("id"), given.get("name"));
    let (ttl, expires) = (given.get("ttl"), given.get("expires"));
    let never = given.get("never") == "true";
    let problem = match () {
        () if cmd != "list" && role.is_empty() => Some("-role is required"),
        () if matches!(cmd, "expiry" | "revoke") && id.is_empty() => Some("-id is required"),
        () if cmd == "create" && name.trim().is_empty() => Some("-name is required"),
        () if !ttl.is_empty() && !expires.is_empty() => Some("give -ttl or -expires, not both"),
        () if cmd == "expiry" && ttl.is_empty() && expires.is_empty() && !never => {
            Some("give -ttl, -expires or -never")
        }
        () if cmd == "expiry" && never && (!ttl.is_empty() || !expires.is_empty()) => {
            Some("-never takes no -ttl or -expires")
        }
        () => None,
    };
    if let Some(p) = problem {
        return usage_error(cmd, p);
    }
    if !ttl.is_empty() {
        if let Err(e) = akit_roles::linktoken::parse_ttl(ttl) {
            return usage_error(cmd, &e);
        }
    }
    let expires_at = if expires.is_empty() {
        0
    } else {
        match parse_rfc3339(expires) {
            Some(t) => t,
            None => {
                return usage_error(
                    cmd,
                    &format!(
                        "-expires {expires:?} is not an RFC 3339 time, as 2027-01-01T00:00:00Z"
                    ),
                );
            }
        }
    };
    let timeout = match given.get("timeout") {
        "" => Duration::from_secs(60),
        t => match flags::parse_duration(t) {
            Some(d) => d,
            None => return usage_error(cmd, &format!("invalid value {t:?} for flag -timeout")),
        },
    };
    let enc = |s: &str| s.replace('%', "%25").replace('/', "%2F");
    let (method, path, body) = match cmd {
        "list" if role.is_empty() => (Method::Get, "/tokens".to_owned(), None),
        "list" => (Method::Get, format!("/tokens/{}", enc(role)), None),
        "create" => {
            let mut b = String::from("{\"name\":");
            write_string(&mut b, name);
            for (k, v) in [("note", given.get("note")), ("ttl", ttl)] {
                if !v.is_empty() {
                    b.push_str(&format!(",\"{k}\":"));
                    write_string(&mut b, v);
                }
            }
            if expires_at != 0 {
                b.push_str(&format!(",\"expires_at\":{expires_at}"));
            }
            b.push('}');
            (Method::Post, format!("/tokens/{}", enc(role)), Some(b))
        }
        "expiry" => {
            let mut b = String::from("{");
            if !ttl.is_empty() {
                b.push_str("\"ttl\":");
                write_string(&mut b, ttl);
            } else if expires_at != 0 {
                b.push_str(&format!("\"expires_at\":{expires_at}"));
            }
            b.push('}');
            (
                Method::Patch,
                format!("/tokens/{}/{}", enc(role), enc(id)),
                Some(b),
            )
        }
        _ => (
            Method::Delete,
            format!("/tokens/{}/{}", enc(role), enc(id)),
            None,
        ),
    };
    let paths = crate::config_paths(&given.0);
    let api = match Api::new(&paths, &given, timeout) {
        Ok(a) => a,
        Err(e) => {
            eprintln!("akit token: {e}");
            return ExitCode::from(2);
        }
    };
    let out = api.call(method, &path, body);
    api.close();
    let (status, out) = match out {
        Ok(r) => r,
        Err(e) => {
            eprintln!("akit token {cmd}: {e}");
            return ExitCode::from(1);
        }
    };
    if status != 200 {
        let msg = akit_config::value::parse(&out)
            .ok()
            .and_then(|v| match v.get("error") {
                Some(Value::String(s)) => Some(s.clone()),
                _ => None,
            })
            .unwrap_or_else(|| out.trim().to_owned());
        eprintln!("akit token {cmd}: the Admin API answered {status}: {msg}");
        return ExitCode::from(1);
    }
    if given.get("json") == "true" {
        print!("{out}");
        return ExitCode::SUCCESS;
    }
    print_answer(cmd, role, &out)
}

/// Prints an answer as fit's `printToken` does.
fn print_answer(cmd: &str, role: &str, out: &str) -> ExitCode {
    let Ok(v) = akit_config::value::parse(out) else {
        eprintln!("akit token {cmd}: the Admin API's answer is not JSON: {out}");
        return ExitCode::from(1);
    };
    let s = |v: &Value, k: &str| match v.get(k) {
        Some(Value::String(s)) => s.clone(),
        _ => String::new(),
    };
    let n = |v: &Value, k: &str| match v.get(k) {
        Some(Value::Number(n)) => n.parse::<i64>().unwrap_or(0),
        _ => 0,
    };
    match cmd {
        "create" => {
            let token = s(&v, "token");
            if token.is_empty() {
                eprintln!("akit token create: the Admin API's answer is not a token: {out}");
                return ExitCode::from(1);
            }
            println!("{token}");
            eprintln!(
                "akit token create: {role} issued token {} ({:?}), {}; it is shown this once: give it to the member as its token",
                s(&v, "id"),
                s(&v, "name"),
                akit_roles::linktoken::expires_text(n(&v, "expires_at"))
            );
        }
        "expiry" => println!(
            "{role}: token {} now {}",
            s(&v, "id"),
            akit_roles::linktoken::expires_text(n(&v, "expires_at"))
        ),
        "revoke" => println!("{role}: token revoked; its links are closed"),
        _ => {
            let mut lists: Vec<(String, Vec<Value>)> = Vec::new();
            match (v.get("roles"), v.get("tokens")) {
                (Some(Value::Object(roles)), _) => {
                    for (r, l) in roles {
                        if let Value::Array(items) = l {
                            lists.push((r.clone(), items.clone()));
                        }
                    }
                }
                (_, Some(Value::Array(items))) => lists.push((role.to_owned(), items.clone())),
                _ => {}
            }
            if lists.iter().all(|(_, l)| l.is_empty()) {
                println!("no tokens");
            }
            for (r, items) in lists {
                for t in items {
                    let members = match t.get("members") {
                        Some(Value::Array(m)) => m
                            .iter()
                            .filter_map(|m| match m {
                                Value::String(s) => Some(s.as_str()),
                                _ => None,
                            })
                            .collect::<Vec<_>>()
                            .join(","),
                        _ => String::new(),
                    };
                    let links = match t.get("links") {
                        Some(Value::Array(l)) => l.len(),
                        _ => 0,
                    };
                    let last = match n(&t, "last_used") {
                        0 => "never".to_owned(),
                        at => akit_roles::linktoken::rfc3339(at),
                    };
                    println!(
                        "{r}\t{}\t{}\t{}\tlast used {last}\tmembers {}\tlinks {links}",
                        s(&t, "id"),
                        s(&t, "name"),
                        akit_roles::linktoken::expires_text(n(&t, "expires_at")),
                        if members.is_empty() { "-" } else { &members },
                    );
                }
            }
        }
    }
    ExitCode::SUCCESS
}

/// `s`, an RFC 3339 time with a `Z` or an offset, in seconds since 1970.
// Every part is bounded first (a year of four digits, an offset of two), so nothing overflows.
#[allow(clippy::arithmetic_side_effects)]
fn parse_rfc3339(s: &str) -> Option<i64> {
    let (date, rest) = s.split_once('T')?;
    let mut d = date.splitn(3, '-');
    let y = d.next()?;
    if y.len() != 4 {
        return None;
    }
    let y: i64 = y.parse().ok()?;
    let m: i64 = d.next()?.parse().ok()?;
    let day: i64 = d.next()?.parse().ok()?;
    let (time, offset) = if let Some(t) = rest.strip_suffix('Z') {
        (t, 0)
    } else {
        let i = rest.rfind(['+', '-'])?;
        let (t, o) = rest.split_at(i);
        let sign = if o.starts_with('-') { -1 } else { 1 };
        let (oh, om) = o.get(1..)?.split_once(':')?;
        if oh.len() != 2 || om.len() != 2 {
            return None;
        }
        (
            t,
            sign * (oh.parse::<i64>().ok()? * 3600 + om.parse::<i64>().ok()? * 60),
        )
    };
    let time = time.split('.').next()?;
    let mut t = time.splitn(3, ':');
    let h: i64 = t.next()?.parse().ok()?;
    let mi: i64 = t.next()?.parse().ok()?;
    let se: i64 = t.next()?.parse().ok()?;
    if !(1..=12).contains(&m) || !(1..=31).contains(&day) || h > 23 || mi > 59 || se > 60 {
        return None;
    }
    // Howard Hinnant's days_from_civil.
    let y = if m <= 2 { y - 1 } else { y };
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let mp = (m + 9) % 12;
    let doy = (153 * mp + 2) / 5 + day - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    let days = era * 146_097 + doe - 719_468;
    Some(days * 86_400 + h * 3600 + mi * 60 + se - offset)
}

/// The Admin API, reached as the bootstrap config's `admin` says.
struct Api {
    engine: Arc<akio::Engine>,
    client: Client,
    base: String,
    token: Option<String>,
    timeout: Duration,
}

impl Api {
    fn new(paths: &[String], given: &Given, timeout: Duration) -> Result<Api, String> {
        let files = akit_config::read_config_files(paths).map_err(|e| e.to_string())?;
        let (config, _) = akit_config::load_config_files(&files).map_err(|e| e.to_string())?;
        let Some(admin) = config.admin else {
            return Err(format!(
                "{} sets no admin: akit token reaches akit through its Admin API",
                paths.join(", ")
            ));
        };
        let token = match &admin.token_file {
            None => None,
            Some(f) => Some(
                std::fs::read_to_string(f)
                    .map_err(|e| format!("admin.token_file: {e}"))?
                    .trim()
                    .to_owned(),
            ),
        };
        let mut cfg = akio::Config::default();
        cfg.shutdown_timeout = Duration::from_secs(1);
        let engine = akio::Engine::builder()
            .client_threads(1)
            .config(cfg)
            .build()
            .map_err(|e| format!("the client's engine: {e}"))?;
        engine
            .start()
            .map_err(|e| format!("the client's engine: {e}"))?;
        let engine = Arc::new(engine);
        let mut client = Client::new(Arc::clone(&engine)).with_timeout(timeout);
        let base = if let Some(sock) = admin.address.strip_prefix("unix/") {
            client = client.with_unix_socket(Some(sock.into()));
            "http://localhost".to_owned()
        } else {
            let (host, port) = admin.address.rsplit_once(':').ok_or_else(|| {
                format!("admin.address {:?} is not a host and port", admin.address)
            })?;
            let host = match host {
                "" | "0.0.0.0" => "127.0.0.1",
                "[::]" => "[::1]",
                h => h,
            };
            if admin.cert_file.is_some() {
                let mut b = akio::tls::ClientConfig::builder();
                let roots = match given.get("ca") {
                    "" => akio::tls::RootCerts::system()
                        .map_err(|e| format!("the system's roots: {e}"))?,
                    ca => {
                        let pem = std::fs::read(ca).map_err(|e| format!("-ca: {e}"))?;
                        let mut r = akio::tls::RootCerts::empty();
                        r.add_pem(&pem).map_err(|e| format!("-ca: {e}"))?;
                        r
                    }
                };
                b = b.roots(roots);
                if !given.get("cert").is_empty() {
                    let chain = std::fs::read(given.get("cert"))
                        .map_err(|e| format!("-cert: {e}"))
                        .and_then(|p| {
                            akio::tls::Certificate::from_pem(&p).map_err(|e| format!("-cert: {e}"))
                        })?;
                    let key = std::fs::read(given.get("key"))
                        .map_err(|e| format!("-key: {e}"))
                        .and_then(|p| {
                            akio::tls::PrivateKey::from_pem(&p).map_err(|e| format!("-key: {e}"))
                        })?;
                    b = b.client_cert(chain, key);
                }
                client = client.with_tls(b.build().map_err(|e| format!("TLS: {e}"))?);
                if !given.get("server-name").is_empty() {
                    client = client.with_tls_server_name(given.get("server-name"));
                }
                format!("https://{host}:{port}")
            } else {
                format!("http://{host}:{port}")
            }
        };
        Ok(Api {
            engine,
            client,
            base,
            token,
            timeout,
        })
    }

    /// Sends a request and waits for its answer: its status and body.
    fn call(
        &self,
        method: Method,
        path: &str,
        body: Option<String>,
    ) -> Result<(u16, String), String> {
        let mut req = ClientRequest::new(method, &format!("{}{path}", self.base));
        if let Some(t) = &self.token {
            req = req.header("Authorization", &format!("Bearer {t}"));
        }
        if let Some(b) = body {
            req = req
                .header("Content-Type", "application/json")
                .body(b.into_bytes());
        }
        let (tx, rx) = std::sync::mpsc::sync_channel(1);
        self.client.request(req, move |r| {
            let _ = tx.send(r);
        });
        match rx.recv_timeout(self.timeout.saturating_add(Duration::from_secs(1))) {
            Ok(Ok(r)) => Ok((r.status, String::from_utf8_lossy(&r.body).into_owned())),
            Ok(Err(e)) => Err(format!("the Admin API at {}: {e}", self.base)),
            Err(_) => Err(format!("the Admin API at {} did not answer", self.base)),
        }
    }

    fn close(&self) {
        self.engine.shutdown();
        self.engine.wait();
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn rfc3339_times() {
        assert_eq!(super::parse_rfc3339("1970-01-01T00:00:00Z"), Some(0));
        assert_eq!(
            super::parse_rfc3339("2027-01-01T00:00:00Z"),
            Some(1_798_761_600)
        );
        assert_eq!(
            super::parse_rfc3339("2027-01-01T01:00:00+01:00"),
            Some(1_798_761_600)
        );
        assert_eq!(super::parse_rfc3339("2027-13-01T00:00:00Z"), None);
        assert_eq!(super::parse_rfc3339("soon"), None);
        assert_eq!(
            akit_roles::linktoken::rfc3339(1_798_761_600),
            "2027-01-01T00:00:00Z"
        );
    }
}
