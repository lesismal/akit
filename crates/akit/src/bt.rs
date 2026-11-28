//! `akit bt` (I157), as fit's `fit bt` (`cli/bt.go`): see and steer a running akit's BitTorrent
//! client through its Admin API's `/bittorrent/...` endpoints. It reaches the API as `akit token`
//! does ([`crate::token::Api`]): the bootstrap config's `admin`, its token and its TLS. fit's
//! `rss` commands come with RSS (I159).

use std::fmt::Write as _;
use std::process::ExitCode;
use std::time::Duration;

use akio_http::Method;
use akit_config::value::{Value, write_string};

use crate::cli::token::{Api, Given};
use crate::flags::{self, Spec};

const USAGE: &str = "Usage: akit bt <command> [flags] [arguments]

See and steer a running akit's BitTorrent client, through its Admin API:

\takit bt ls [-state S[,S]] [-category C] [-q TEXT]
\t                                       the torrents, in queue order
\takit bt add [-category C] [-paused] [-up RATE] [-down RATE]
\t        <magnet link | http(s) URL | file.torrent>...
\t                                       add torrents
\takit bt info <torrent>                  a torrent's status and files
\takit bt pause <torrent>...              pause torrents
\takit bt resume <torrent>...             resume them, as the queue and the
\t                                       disk let them
\takit bt rm [-delete-data] <torrent>...  remove torrents, and with
\t                                       -delete-data their files
\takit bt limit [-up RATE] [-down RATE] [-ratio X] [-seed-time D] [-clear]
\t        <torrent>
\t                                       set a torrent's own limits, over its
\t                                       category's; -clear removes them
\takit bt alt [auto | on | off]           show the alternative rates, or turn
\t                                       them on or off whatever their
\t                                       schedules say, or back to them

A <torrent> is its ID (its info hash), a unique start of it, or its name.
A RATE is bytes per second, with K, M or G (KiB, MiB, GiB) after it if
wanted; 0 takes the category's or the client's. Every command takes -c, the
bootstrap config whose admin says where the Admin API is, -timeout, -json
and the TLS flags of akit token. Exits 1 when the Admin API refuses.
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
        usage: "how long akit bt waits for the Admin API, a .torrent file downloaded by URL included (default 1m30s)",
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
        usage: "with admin.client_ca_file, a PEM file with the client certificate akit bt presents",
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

const LS: &[Spec] = &[
    Spec {
        name: "state",
        bool: false,
        usage: "only torrents in these states, with commas: metadata, checking, downloading, seeding, queued, paused, stopped, removed, error",
    },
    Spec {
        name: "category",
        bool: false,
        usage: "only torrents of this category",
    },
    Spec {
        name: "q",
        bool: false,
        usage: "only torrents whose name has this, in any case",
    },
];

const ADD: &[Spec] = &[
    Spec {
        name: "category",
        bool: false,
        usage: "the category to add them in",
    },
    Spec {
        name: "paused",
        bool: true,
        usage: "add them paused",
    },
    Spec {
        name: "up",
        bool: false,
        usage: "their own upload rate",
    },
    Spec {
        name: "down",
        bool: false,
        usage: "their own download rate",
    },
];

const RM: &[Spec] = &[Spec {
    name: "delete-data",
    bool: true,
    usage: "delete the torrents' files too",
}];

const LIMIT: &[Spec] = &[
    Spec {
        name: "up",
        bool: false,
        usage: "the torrent's upload rate; negative is the client's alone",
    },
    Spec {
        name: "down",
        bool: false,
        usage: "the torrent's download rate; negative is the client's alone",
    },
    Spec {
        name: "ratio",
        bool: false,
        usage: "pause it once it has uploaded this many times its size; negative seeds for ever",
    },
    Spec {
        name: "seed-time",
        bool: false,
        usage: "pause it once it has seeded this long, as 12h",
    },
    Spec {
        name: "clear",
        bool: true,
        usage: "remove the torrent's own limits",
    },
];

/// The most a .torrent file sent may be: as one the Admin API downloads by URL.
const MAX_TORRENT_FILE: u64 = 32 << 20;

fn usage_error(cmd: &str, msg: &str) -> ExitCode {
    eprintln!("akit bt {cmd}: {msg}");
    ExitCode::from(2)
}

/// A rate in bytes a second, with K, M or G (or KiB, MiB, GiB) after it, binary multiples; a
/// negative one is no limit (fit's `parseRate`).
fn parse_rate(s: &str) -> Result<i64, String> {
    let t = s.trim();
    let mut mult = 1f64;
    let mut num = t;
    for (suffix, m) in [
        ("KiB", 1024f64),
        ("MiB", 1_048_576f64),
        ("GiB", 1_073_741_824f64),
        ("K", 1024f64),
        ("M", 1_048_576f64),
        ("G", 1_073_741_824f64),
        ("k", 1024f64),
    ] {
        if let Some(v) = t.strip_suffix(suffix) {
            num = v.trim();
            mult = m;
            break;
        }
    }
    let bad = || format!("{s:?} is not a rate: bytes per second, as 524288, 512K or 1.5M");
    let f: f64 = num.parse().map_err(|_| bad())?;
    let v = f * mult;
    if !v.is_finite() || v.abs() > 1_099_511_627_776f64 {
        return Err(bad());
    }
    #[allow(clippy::cast_possible_truncation)]
    Ok(v as i64)
}

/// `akit bt <cmd> [flags] [arguments]`.
pub(crate) fn bt(args: &[String]) -> ExitCode {
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
        "ls" => [COMMON, LS].concat(),
        "add" => [COMMON, ADD].concat(),
        "rm" => [COMMON, RM].concat(),
        "limit" => [COMMON, LIMIT].concat(),
        "info" | "pause" | "resume" | "alt" => COMMON.to_vec(),
        "rss" => {
            eprintln!("akit bt rss: RSS comes with I159: not supported yet");
            return ExitCode::from(2);
        }
        other => {
            eprint!("akit bt: unknown command {other:?}\n\n{USAGE}");
            return ExitCode::from(2);
        }
    };
    let (given, names) = match flags::parse_args(&format!("bt {cmd}"), rest, &specs) {
        Ok((f, n)) => (Given(f), n),
        Err(u) => {
            eprintln!("{}", u.0);
            return ExitCode::from(2);
        }
    };
    let rate = |name: &str| -> Result<Option<i64>, String> {
        match given.get(name) {
            "" if !given.0.iter().any(|(n, _)| *n == name) => Ok(None),
            v => parse_rate(v)
                .map(Some)
                .map_err(|e| format!("invalid value for flag -{name}: {e}")),
        }
    };
    let (up, down) = match (rate("up"), rate("down")) {
        (Ok(u), Ok(d)) => (u, d),
        (Err(e), _) | (_, Err(e)) => return usage_error(cmd, &e),
    };
    let ratio = match given.get("ratio") {
        "" => 0.0,
        r => match r.parse::<f64>() {
            Ok(r) if r.is_finite() => r,
            _ => return usage_error(cmd, &format!("invalid value {r:?} for flag -ratio")),
        },
    };
    let seed_time = given.get("seed-time").to_owned();
    let clear = given.get("clear") == "true";
    let problem = match cmd {
        "ls" if !names.is_empty() => {
            Some("takes no arguments; filter with -state, -category and -q".to_owned())
        }
        "alt" if names.len() > 1 => Some("give auto, on or off, or nothing".to_owned()),
        "alt"
            if names
                .first()
                .is_some_and(|m| !matches!(m.as_str(), "auto" | "on" | "off")) =>
        {
            Some(format!(
                "the mode is auto, on or off, not {:?}",
                names.first().map_or("", String::as_str)
            ))
        }
        "info" | "limit" if names.len() != 1 => Some("name one torrent".to_owned()),
        "add" if names.is_empty() => Some("name a torrent".to_owned()),
        "pause" | "resume" | "rm" if names.is_empty() => Some("name a torrent".to_owned()),
        "limit"
            if clear
                && (up.is_some() || down.is_some() || ratio != 0.0 || !seed_time.is_empty()) =>
        {
            Some("-clear goes alone".to_owned())
        }
        "limit"
            if !clear && up.is_none() && down.is_none() && ratio == 0.0 && seed_time.is_empty() =>
        {
            Some("give a limit, or -clear".to_owned())
        }
        _ => None,
    };
    if let Some(p) = problem {
        return usage_error(cmd, &p);
    }
    if !seed_time.is_empty() && flags::parse_duration(&seed_time).is_none() {
        return usage_error(
            cmd,
            &format!("-seed-time {seed_time:?} is not a duration, as 12h"),
        );
    }
    let timeout = match given.get("timeout") {
        "" => Duration::from_secs(90),
        t => match flags::parse_duration(t) {
            Some(d) => d,
            None => return usage_error(cmd, &format!("invalid value {t:?} for flag -timeout")),
        },
    };
    let paths = crate::config_paths(&given.0);
    let api = match Api::new(&paths, &given, timeout) {
        Ok(a) => a,
        Err(e) => {
            eprintln!("akit bt: {e}");
            return ExitCode::from(2);
        }
    };
    let b = Cmd {
        api: &api,
        cmd,
        json: given.get("json") == "true",
    };
    let code = match cmd {
        "ls" => b.ls(&given),
        "add" => b.add(&names, &given, up, down),
        "alt" => b.alt(names.first().map(String::as_str)),
        _ => b.each(&names, &given, (up, down, ratio, &seed_time, clear)),
    };
    api.close();
    code
}

/// A command's Admin API.
struct Cmd<'a> {
    api: &'a Api,
    cmd: &'a str,
    json: bool,
}

/// A string member of `v`.
fn s(v: &Value, k: &str) -> String {
    match v.get(k) {
        Some(Value::String(s)) => s.clone(),
        _ => String::new(),
    }
}

/// A number member of `v`.
fn f(v: &Value, k: &str) -> f64 {
    match v.get(k) {
        Some(Value::Number(n)) => n.parse().unwrap_or(0.0),
        _ => 0.0,
    }
}

fn b(v: &Value, k: &str) -> bool {
    matches!(v.get(k), Some(Value::Bool(true)))
}

fn enc(s: &str) -> String {
    let mut out = String::new();
    for c in s.bytes() {
        if c.is_ascii_alphanumeric() || matches!(c, b'-' | b'_' | b'.' | b'~' | b',') {
            out.push(char::from(c));
        } else {
            let _ = write!(out, "%{c:02X}");
        }
    }
    out
}

impl Cmd<'_> {
    /// Sends a request and returns its answer, or reports why it failed.
    fn call(&self, method: Method, path: &str, body: Option<(Vec<u8>, &str)>) -> Option<String> {
        let (status, out) = match self.api.send(method, path, body) {
            Ok(r) => r,
            Err(e) => {
                eprintln!("akit bt {}: {e}", self.cmd);
                return None;
            }
        };
        if !(200..300).contains(&status) {
            let msg = akit_config::value::parse(&out)
                .ok()
                .map(|v| s(&v, "error"))
                .filter(|m| !m.is_empty())
                .unwrap_or_else(|| out.trim().to_owned());
            eprintln!(
                "akit bt {}: the Admin API answered {status}: {msg}",
                self.cmd
            );
            return None;
        }
        Some(out)
    }

    fn ls(&self, given: &Given) -> ExitCode {
        let mut q = Vec::new();
        for k in ["state", "category", "q"] {
            if !given.get(k).is_empty() {
                q.push(format!("{k}={}", enc(given.get(k))));
            }
        }
        let mut path = "/bittorrent/torrents".to_owned();
        if !q.is_empty() {
            path = format!("{path}?{}", q.join("&"));
        }
        let Some(out) = self.call(Method::Get, &path, None) else {
            return ExitCode::from(1);
        };
        if self.json {
            print!("{out}");
            return ExitCode::SUCCESS;
        }
        let v = akit_config::value::parse(&out).unwrap_or(Value::Null);
        let list = match v.get("torrents") {
            Some(Value::Array(a)) => a.clone(),
            _ => Vec::new(),
        };
        print!("{}", torrents_text(&list, f(&v, "total")));
        ExitCode::SUCCESS
    }

    fn add(
        &self,
        sources: &[String],
        given: &Given,
        up: Option<i64>,
        down: Option<i64>,
    ) -> ExitCode {
        let mut code = ExitCode::SUCCESS;
        let (category, paused) = (given.get("category"), given.get("paused") == "true");
        for src in sources {
            let mut req = String::from("{\"category\":");
            write_string(&mut req, category);
            let _ = write!(req, ",\"paused\":{paused}");
            if up.is_some() || down.is_some() {
                let _ = write!(
                    req,
                    ",\"limits\":{{\"upload_rate\":{},\"download_rate\":{}}}",
                    up.unwrap_or(0),
                    down.unwrap_or(0)
                );
            }
            let mut path = "/bittorrent/torrents".to_owned();
            let body: (Vec<u8>, &str) = if src.starts_with("magnet:") {
                req.push_str(",\"magnet\":");
                write_string(&mut req, src);
                req.push('}');
                (req.into_bytes(), "application/json")
            } else if src.starts_with("http://") || src.starts_with("https://") {
                req.push_str(",\"url\":");
                write_string(&mut req, src);
                req.push('}');
                (req.into_bytes(), "application/json")
            } else {
                let data = match read_torrent_file(src) {
                    Ok(d) => d,
                    Err(e) => {
                        eprintln!("akit bt add: {e}");
                        code = ExitCode::from(1);
                        continue;
                    }
                };
                if up.is_some() || down.is_some() {
                    // The limits do not go in a query: JSON then.
                    req.push_str(",\"torrent\":");
                    write_string(&mut req, &akio_crypto::base64::STANDARD.encode(&data));
                    req.push('}');
                    (req.into_bytes(), "application/json")
                } else {
                    let mut q = Vec::new();
                    if !category.is_empty() {
                        q.push(format!("category={}", enc(category)));
                    }
                    if paused {
                        q.push("paused=1".to_owned());
                    }
                    if !q.is_empty() {
                        path = format!("{path}?{}", q.join("&"));
                    }
                    (data, "application/x-bittorrent")
                }
            };
            let Some(out) = self.call(Method::Post, &path, Some(body)) else {
                code = ExitCode::from(1);
                continue;
            };
            if self.json {
                print!("{out}");
                continue;
            }
            let v = akit_config::value::parse(&out).unwrap_or(Value::Null);
            println!(
                "{} {}: added, {}",
                s(&v, "id"),
                s(&v, "name"),
                s(&v, "state")
            );
        }
        code
    }

    fn alt(&self, mode: Option<&str>) -> ExitCode {
        let out = match mode {
            None => self.call(Method::Get, "/bittorrent/alt_speed", None),
            Some(m) => {
                let mut body = String::from("{\"mode\":");
                write_string(&mut body, m);
                body.push('}');
                self.call(
                    Method::Put,
                    "/bittorrent/alt_speed",
                    Some((body.into_bytes(), "application/json")),
                )
            }
        };
        let Some(out) = out else {
            return ExitCode::from(1);
        };
        if self.json {
            print!("{out}");
            return ExitCode::SUCCESS;
        }
        let v = akit_config::value::parse(&out).unwrap_or(Value::Null);
        let holds = if b(&v, "active") {
            "hold"
        } else {
            "do not hold"
        };
        println!(
            "mode    {}\nnow     the alternative rates {holds}; the client goes by upload {}, download {}",
            s(&v, "mode"),
            rate_text(f(&v, "upload_rate")),
            rate_text(f(&v, "download_rate"))
        );
        if !b(&v, "configured") {
            println!("        (the config has no alt_speed: the alternative rates never hold)");
        }
        ExitCode::SUCCESS
    }

    /// The IDs of the torrents `names` name: an ID or info hash, a unique start of one, or a
    /// name.
    fn resolve(&self, names: &[String]) -> Option<Vec<String>> {
        let mut list: Option<Vec<Value>> = None;
        let mut ids = Vec::new();
        for n in names {
            if let Some(h) = akit_config::bittorrent::parse_info_hash(n) {
                ids.push(h.iter().fold(String::new(), |mut s, b| {
                    let _ = write!(s, "{b:02x}");
                    s
                }));
                continue;
            }
            if list.is_none() {
                let out = self.call(Method::Get, "/bittorrent/torrents?limit=10000", None)?;
                let v = akit_config::value::parse(&out).unwrap_or(Value::Null);
                list = Some(match v.get("torrents") {
                    Some(Value::Array(a)) => a.clone(),
                    _ => Vec::new(),
                });
            }
            let lower = n.to_lowercase();
            let found: Vec<String> = list
                .iter()
                .flatten()
                .filter(|t| {
                    s(t, "name") == *n || lower.len() >= 4 && s(t, "id").starts_with(&lower)
                })
                .map(|t| s(t, "id"))
                .collect();
            match found.as_slice() {
                [] => {
                    eprintln!("akit bt {}: no torrent is {n:?}", self.cmd);
                    return None;
                }
                [id] => ids.push(id.clone()),
                more => {
                    eprintln!(
                        "akit bt {}: {n:?} names {} torrents: give more of the ID",
                        self.cmd,
                        more.len()
                    );
                    return None;
                }
            }
        }
        Some(ids)
    }

    /// `info`, `pause`, `resume`, `rm` and `limit`, on each torrent named.
    fn each(
        &self,
        names: &[String],
        given: &Given,
        (up, down, ratio, seed_time, clear): (Option<i64>, Option<i64>, f64, &str, bool),
    ) -> ExitCode {
        let Some(ids) = self.resolve(names) else {
            return ExitCode::from(1);
        };
        let mut code = ExitCode::SUCCESS;
        for id in ids {
            let mut p = format!("/bittorrent/torrents/{id}");
            let json_body = |b: String| Some((b.into_bytes(), "application/json"));
            let out = match self.cmd {
                "info" => self.call(Method::Get, &p, None),
                "pause" | "resume" => self.call(
                    Method::Post,
                    &p,
                    json_body(format!("{{\"action\":\"{}\"}}", self.cmd)),
                ),
                "rm" => {
                    if given.get("delete-data") == "true" {
                        p.push_str("?delete_data=1");
                    }
                    self.call(Method::Delete, &p, None)
                }
                _ => {
                    let body = if clear {
                        "{\"limits\":null}".to_owned()
                    } else {
                        let mut l = format!(
                            "{{\"limits\":{{\"upload_rate\":{},\"download_rate\":{},\"seed_ratio\":{ratio}",
                            up.unwrap_or(0),
                            down.unwrap_or(0)
                        );
                        if !seed_time.is_empty() {
                            l.push_str(",\"seed_time\":");
                            write_string(&mut l, seed_time);
                        }
                        l.push_str("}}");
                        l
                    };
                    self.call(Method::Patch, &p, json_body(body))
                }
            };
            let Some(out) = out else {
                code = ExitCode::from(1);
                continue;
            };
            if self.json {
                print!("{out}");
                continue;
            }
            let v = akit_config::value::parse(&out).unwrap_or(Value::Null);
            match self.cmd {
                "info" => print!("{}", torrent_text(&v)),
                "rm" => println!("{id} removed"),
                _ => println!("{id} {}: {}", s(&v, "name"), s(&v, "state")),
            }
        }
        code
    }
}

/// Reads a .torrent file, at most as large as the Admin API takes one.
fn read_torrent_file(path: &str) -> Result<Vec<u8>, String> {
    use std::io::Read as _;
    let file = std::fs::File::open(path).map_err(|e| format!("{path}: {e}"))?;
    let mut data = Vec::new();
    file.take(MAX_TORRENT_FILE + 1)
        .read_to_end(&mut data)
        .map_err(|e| format!("{path}: {e}"))?;
    if u64::try_from(data.len()).unwrap_or(u64::MAX) > MAX_TORRENT_FILE {
        return Err(format!(
            "{path} is larger than {MAX_TORRENT_FILE} bytes, the most the Admin API takes"
        ));
    }
    if data.is_empty() {
        return Err(format!("{path} is empty"));
    }
    Ok(data)
}

/// A rate in bytes a second.
fn rate_text(v: f64) -> String {
    if v <= 0.0 {
        "unlimited".to_owned()
    } else {
        format!("{}/s", size_text(v))
    }
}

/// A number of bytes in binary units.
fn size_text(mut v: f64) -> String {
    for u in ["B", "KiB", "MiB", "GiB", "TiB"] {
        if v < 1024.0 || u == "TiB" {
            return if u == "B" {
                format!("{v:.0} {u}")
            } else {
                format!("{v:.1} {u}")
            };
        }
        v /= 1024.0;
    }
    String::new()
}

/// A transfer rate, `-` for none.
fn speed_text(v: f64) -> String {
    if v < 1.0 {
        "-".to_owned()
    } else {
        format!("{}/s", size_text(v))
    }
}

/// Rows in columns, as Go's `tabwriter` with a padding of 2 writes them.
pub(crate) fn table(rows: &[Vec<String>]) -> String {
    let cols = rows.iter().map(Vec::len).max().unwrap_or(0);
    let mut widths = vec![0usize; cols];
    for r in rows {
        for (i, c) in r.iter().enumerate() {
            if let Some(w) = widths.get_mut(i) {
                *w = (*w).max(c.chars().count());
            }
        }
    }
    let mut out = String::new();
    for r in rows {
        for (i, c) in r.iter().enumerate() {
            if i.saturating_add(1) == r.len() {
                out.push_str(c);
            } else {
                let w = widths.get(i).copied().unwrap_or(0);
                let _ = write!(out, "{c:<w$}  ");
            }
        }
        out.push('\n');
    }
    out
}

fn torrents_text(list: &[Value], total: f64) -> String {
    if list.is_empty() {
        return "no torrents\n".to_owned();
    }
    let mut rows = vec![
        [
            "ID", "NAME", "STATE", "PROGRESS", "SIZE", "DOWN", "UP", "PEERS", "CATEGORY",
        ]
        .map(str::to_owned)
        .to_vec(),
    ];
    for t in list {
        let mut name = s(t, "name");
        if name.chars().count() > 48 {
            name = format!("{}...", name.chars().take(45).collect::<String>());
        }
        let id = s(t, "id");
        rows.push(vec![
            id.chars().take(12).collect(),
            name,
            s(t, "state"),
            format!("{:.1}%", f(t, "progress") * 100.0),
            size_text(f(t, "size")),
            speed_text(f(t, "download_rate")),
            speed_text(f(t, "upload_rate")),
            format!("{}", f(t, "peers")),
            s(t, "category"),
        ]);
    }
    let mut out = table(&rows);
    #[allow(clippy::cast_precision_loss)]
    if total > list.len() as f64 {
        let _ = writeln!(out, "{} of {total} torrents", list.len());
    }
    out
}

fn torrent_text(v: &Value) -> String {
    let mut w = String::new();
    let _ = write!(
        w,
        "id         {}\nname       {}\nstate      {}",
        s(v, "id"),
        s(v, "name"),
        s(v, "state")
    );
    let held = s(v, "held");
    if !held.is_empty() && held != s(v, "state") {
        let _ = write!(w, " (held: {held})");
    }
    w.push('\n');
    for (label, k) in [
        ("error      ", "error"),
        ("info hash  ", "info_hash"),
        ("v2 hash    ", "info_hash_v2"),
    ] {
        let x = s(v, k);
        if !x.is_empty() {
            let _ = writeln!(w, "{label}{x}");
        }
    }
    let _ = writeln!(w, "dir        {}", s(v, "dir"));
    let cat = s(v, "category");
    if !cat.is_empty() {
        let _ = writeln!(w, "category   {cat}");
    }
    let _ = writeln!(
        w,
        "progress   {:.1}%, {} of {} wanted, {} of {} pieces",
        f(v, "progress") * 100.0,
        size_text(f(v, "completed")),
        size_text(f(v, "wanted")),
        f(v, "pieces_have"),
        f(v, "pieces")
    );
    let _ = writeln!(
        w,
        "transfer   down {} at {}, up {} at {}",
        size_text(f(v, "downloaded")),
        speed_text(f(v, "download_rate")),
        size_text(f(v, "uploaded")),
        speed_text(f(v, "upload_rate"))
    );
    let _ = writeln!(
        w,
        "peers      {}, {} of them seeds",
        f(v, "peers"),
        f(v, "seeders")
    );
    let _ = write!(w, "queue      {}", f(v, "queue_position"));
    if b(v, "paused") {
        w.push_str(", paused");
    }
    w.push('\n');
    if let Some(l) = v.get("effective_limits") {
        let secs = f(l, "seed_time") / 1e9;
        let _ = writeln!(
            w,
            "limits     up {}, down {}, ratio {}, seed time {}",
            rate_text(f(l, "upload_rate")),
            rate_text(f(l, "download_rate")),
            f(l, "seed_ratio"),
            flags::format_duration(Duration::from_secs_f64(secs.max(0.0)))
        );
    }
    if let Some(Value::Array(files)) = v.get("files") {
        let mut rows = vec![
            ["FILE", "SIZE", "DONE", "PRIORITY", "PATH"]
                .map(str::to_owned)
                .to_vec(),
        ];
        for file in files {
            let len = f(file, "length");
            let done = match file.get("completed") {
                Some(Value::Number(c)) => {
                    let c: f64 = c.parse().unwrap_or(0.0);
                    if len > 0.0 {
                        format!("{:.1}%", c * 100.0 / len)
                    } else {
                        "100.0%".to_owned()
                    }
                }
                _ => "?".to_owned(),
            };
            rows.push(vec![
                format!("{}", f(file, "index")),
                size_text(len),
                done,
                format!("{}", f(file, "priority")),
                s(file, "path"),
            ]);
        }
        w.push_str(&table(&rows));
    }
    w
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rates() {
        assert_eq!(parse_rate("524288"), Ok(524_288));
        assert_eq!(parse_rate("512K"), Ok(524_288));
        assert_eq!(parse_rate("1.5M"), Ok(1_572_864));
        assert_eq!(parse_rate("2 GiB"), Ok(2_147_483_648));
        assert_eq!(parse_rate("-1"), Ok(-1));
        assert!(parse_rate("fast").is_err());
        assert!(parse_rate("2T").is_err());
    }

    #[test]
    fn sizes_and_tables() {
        assert_eq!(size_text(512.0), "512 B");
        assert_eq!(size_text(1536.0), "1.5 KiB");
        assert_eq!(speed_text(0.0), "-");
        assert_eq!(rate_text(0.0), "unlimited");
        assert_eq!(
            table(&[
                vec!["A".into(), "BB".into(), "C".into()],
                vec!["aaa".into(), "b".into(), "c".into()]
            ]),
            "A    BB  C\naaa  b   c\n"
        );
    }
}
