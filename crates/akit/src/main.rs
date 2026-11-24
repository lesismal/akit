#![forbid(unsafe_code)]
//! The `akit` command, with fit's commands, flags and exit codes: 0 when it worked, 1 when it did
//! not (a config with errors, a shutdown that ran out of time), 2 when it was used wrongly.
//!
//! - `run`: reads the config (`-c`, repeated, files and directories, merged), compiles it,
//!   serves it, and on SIGINT or SIGTERM shuts down gracefully, waiting up to
//!   `-shutdown-timeout` for requests in flight; a second signal exits at once. On SIGHUP it
//!   reads the same files again and, when they compile, swaps the new graph in and moves the
//!   listeners to its (I22); a broken config leaves the running one. With `access_log`, a line is
//!   written for each request (I21): the log is opened when akit starts, and closed, once every
//!   line is written, when it stops; a reload changes its level and time zone, as fit's does,
//!   while its output, format, flush interval and buffer take a restart. With `admin`, the Admin
//!   API (I26) serves the running config and its versions and loads new configs, each applied
//!   by the control thread through the same reload as SIGHUP, one config at a time. With
//!   `-watch-certs` (on by default, as fit's), the certificate, key and authority files the
//!   running config loaded are looked at every `-watch-interval`, and once a change has settled
//!   the running config is applied again as a new version (I30): a listener whose files changed
//!   only in their certificates is kept, and its next handshakes get the new ones; an h2
//!   upstream's `tls_ca_file`, `tls_cert_file` and `tls_key_file` (I54) are watched too, and its
//!   new pool reads them again. With `-watch`
//!   (I29, fit's file provider), the config's files are looked at every `-watch-interval` too,
//!   and once a change has settled for 500ms, and the files hold something other than what was
//!   applied last, they are applied as SIGHUP applies them: a file added to a directory, or taken
//!   out, is a change too; a broken edit is reported once and the running config stays.
//!   One metrics registry serves the process (I37): the graphs share their upstreams' counters
//!   through it and, with `metrics`, count their requests in it; every reload (SIGHUP, the Admin
//!   API, the certificate files) is counted; the Admin API serves the scrape at `GET /metrics`.
//! - `validate`: reads and compiles the config, as `run` would, and says whether it is valid.
//! - `version`, `help`.

mod flags;

use std::io::Read;
use std::process::ExitCode;
use std::sync::{Arc, mpsc};
use std::time::{Duration, Instant};

use akit_admin::{
    Applied, ApplyError, Configs, ListenerChange, Mode, Outcome, Refused, WatchOptions,
    WatchReport, Watcher,
};
use akit_config::{Config, ConfigFile, Error, Errors, LogFormat};
use akit_observe::metrics::{ConfigStats, Metrics};
use akit_observe::{AccessLog, Format, Zone};
use akit_runtime::upstream::EngineSlot;
use akit_runtime::{Env, Graph, GraphCell, HandlerParams, Registry, compile};
use akit_tls::{FileWatcher, Report};
use flags::{Spec, Usage};

const USAGE: &str = "\
akit: a reverse proxy, load balancer and API gateway on akio

Usage:
  akit <command> [flags]

Commands:
  run        run a config
  validate   check a config, without running it
  version    print akit's version and the akio commit it is built on
  help       print this help

Run akit <command> -h for a command's flags.
";

const CONFIG_USAGE: &str = "a config file, or a directory of them, to use; give -c again for more, which are merged (fit.json when none is given). A file ending in .yaml or .yml is YAML, any other JSON (comments and trailing commas allowed); fit's .conf format comes later; a directory's files are those, in its subdirectories too";

const RUN_FLAGS: &[Spec] = &[
    Spec {
        name: "c",
        bool: false,
        usage: CONFIG_USAGE,
    },
    Spec {
        name: "shutdown-timeout",
        bool: false,
        usage: "how long shutdown waits for requests in flight before closing their connections (default 30s)",
    },
    Spec {
        name: "watch",
        bool: true,
        usage: "apply the config again each time one of its files changes, or a directory's files are added or taken out, as SIGHUP does",
    },
    Spec {
        name: "watch-interval",
        bool: false,
        usage: "how often watched files are looked at: the config's, with -watch, and certificate files, with -watch-certs (default 1s)",
    },
    Spec {
        name: "watch-certs",
        bool: true,
        usage: "apply the running config again each time the certificate or key files it loaded change (default true)",
    },
    // fit's, not supported yet: refused when given.
    Spec {
        name: "admin",
        bool: true,
        usage: "not supported yet (the Admin API, I26)",
    },
    Spec {
        name: "timeout",
        bool: false,
        usage: "not supported yet (the Admin API, I26)",
    },
];

const VALIDATE_FLAGS: &[Spec] = &[Spec {
    name: "c",
    bool: false,
    usage: CONFIG_USAGE,
}];

fn version() -> String {
    format!(
        "akit {}\nakio {}\n{}/{}",
        env!("CARGO_PKG_VERSION"),
        akit_runtime::AKIO_REV
            .get(..12)
            .unwrap_or(akit_runtime::AKIO_REV),
        std::env::consts::OS,
        std::env::consts::ARCH,
    )
}

/// Prints a config's errors as fit does: each with its file, or with `name`, the files' names,
/// when it has none.
fn print_errors(name: &str, errs: &Errors) {
    for e in errs.as_slice() {
        if e.file.is_some() {
            eprintln!("{e}");
        } else {
            eprintln!("{name}: {e}");
        }
    }
}

/// The config files the flags name, `fit.json` when none.
fn config_paths(flags: &[(&str, String)]) -> Vec<String> {
    let paths: Vec<String> = flags
        .iter()
        .filter(|(n, _)| *n == "c")
        .map(|(_, v)| v.clone())
        .collect();
    if paths.is_empty() {
        vec!["fit.json".to_owned()]
    } else {
        paths
    }
}

/// Why a config could not be loaded.
enum Failure {
    /// The config's errors.
    Config(Errors),
    /// The module registry could not be made (a bug in akit, not in the config).
    Registry(String),
}

impl Failure {
    /// Prints the errors, as fit prints them; `name` names the files.
    fn print(&self, name: &str) {
        match self {
            Failure::Config(errs) => print_errors(name, errs),
            Failure::Registry(e) => eprintln!("akit: {e}"),
        }
    }
}

/// A config read: the config, as one JSON document too (what the Admin API keeps of it), and the
/// zone its access log's `time_zone` names, which is loaded here, as fit's validation loads it.
type Loaded = (Config, String, Option<Zone>);

/// Reads the config at `paths`, as [`load`] reads it, with the files it was read from (what
/// `-watch` compares the files with).
fn read_config(paths: &[String]) -> Result<(Loaded, Vec<ConfigFile>), Failure> {
    let files = akit_config::read_config_files(paths).map_err(Failure::Config)?;
    Ok((load(&files)?, files))
}

/// The config `files` hold.
fn load(files: &[ConfigFile]) -> Result<Loaded, Failure> {
    let (config, json) = akit_config::load_config_files(files).map_err(Failure::Config)?;
    let zone = time_zone(&config)?;
    Ok((config, json, zone))
}

/// The zone `config`'s access log's `time_zone` names, loaded.
fn time_zone(config: &Config) -> Result<Option<Zone>, Failure> {
    Ok(match &config.access_log {
        None => None,
        Some(a) => Some(Zone::load(&a.time_zone).map_err(|e| {
            Failure::Config(Errors(vec![Error {
                file: None,
                path: "access_log.time_zone".into(),
                line: 0,
                col: 0,
                msg: e.to_string(),
            }]))
        })?),
    })
}

/// Compiles `config`, its upstream pools dialing through the engine `engine` will hold, its
/// requests written to `log`.
fn compile_config(
    config: &Config,
    engine: &EngineSlot,
    log: Option<&AccessLog>,
    metrics: &Metrics,
    acme: Option<&akit_tls::Acme>,
) -> Result<Graph, Failure> {
    let registry: Registry<HandlerParams> = Registry::new(&[
        akit_handler::MODULES,
        akit_middleware::MODULES,
        akit_cache::MODULES,
        akit_proxy::MODULES,
    ])
    .map_err(|e| Failure::Registry(e.to_string()))?;
    let env = Env::new(engine.clone())
        .with_access_log(log.cloned())
        .with_metrics(Some(metrics.clone()))
        .with_acme(acme.cloned());
    compile(config, &registry, &env).map_err(Failure::Config)
}

/// Reads and compiles the config at `paths`, as [`read_config`] and [`compile_config`].
fn read(paths: &[String], engine: &EngineSlot, log: Option<&AccessLog>) -> Result<Graph, Failure> {
    let ((config, _, _), _) = read_config(paths)?;
    compile_config(&config, engine, log, &Metrics::new(), None)
}

/// Opens the access log `c` describes, its times in `zone`, as fit's `NewAccessLogFiles`.
fn open_access_log(c: &akit_config::AccessLog, zone: Zone) -> std::io::Result<AccessLog> {
    let out = akit_observe::open_output(&c.output)?;
    let mut opts = akit_observe::Options::new();
    opts.format = match c.format {
        LogFormat::Json => Format::Json,
        LogFormat::Text => Format::Text,
        LogFormat::Line => Format::Line,
    };
    opts.zone = zone;
    opts.min_status = c.min_status();
    opts.flush_interval = c.flush_interval;
    opts.buffer_bytes = c.buffer_bytes;
    AccessLog::new(out, opts)
}

/// The certificate, key and authority files a config loaded, with their SHA-256, by path.
type CertFiles = std::collections::BTreeMap<String, akit_tls::Sum>;

/// What applying a config takes: the engine slot its pools dial through, the listeners it moves,
/// the access log opened at start (with the config it was opened from), and the `admin` akit
/// started with; and what the running config loaded from certificate files, which
/// `-watch-certs` compares the files with.
struct Runner<'a> {
    slot: &'a EngineSlot,
    listeners: &'a mut akit_listener::Listeners,
    log: Option<&'a (AccessLog, akit_config::AccessLog)>,
    admin: Option<&'a akit_config::Admin>,
    cert_files: CertFiles,
    /// The process's metrics registry, which each graph is compiled with (I37).
    metrics: &'a Metrics,
    /// The ACME manager made from the `acme` akit started with (I31), and that `acme`: each
    /// config applied has it manage the names its listeners list in `tls.acme`.
    acme: &'a akit_tls::Acme,
    acme_config: Option<&'a akit_config::Acme>,
    /// The configs applied after the first, and those that failed, for the metrics: every
    /// reload goes through [`Runner::apply`] or fails before it, here (SIGHUP, the Admin API,
    /// the certificate files).
    stats: &'a ConfigStats,
}

impl Runner<'_> {
    /// Compiles `config` and moves the server to it (I22's reload), as one step: on any failure
    /// the running config stays. The access log takes the new config's level and time zone
    /// (`zone`); its other settings, and `admin`, take a restart, as fit's, and are warned of.
    fn apply(&mut self, config: &Config, zone: Option<Zone>) -> Result<Applied, ApplyError> {
        let r = self.swap(config);
        self.stats.reloaded(r.is_ok());
        let (r, files) = r?;
        self.cert_files = files;
        let warnings = self.warnings(config, zone, true);
        Ok(applied(&r, warnings))
    }

    /// Checks `config` as [`apply`](Runner::apply) would apply it, changing nothing (a dry run of
    /// the Admin API, I27, as fit's): compiled with no engine, so that its pools dial nothing
    /// and probe nothing, and its new listeners' addresses tried (`Listeners::plan`). Not counted
    /// as a reload.
    fn check(&self, config: &Config, zone: Option<Zone>) -> Result<Applied, ApplyError> {
        let graph = compile_config(config, &EngineSlot::new(), None, &Metrics::new(), None)
            .map_err(failure)?;
        let r = self
            .listeners
            .plan(&graph)
            .map_err(|e| ApplyError::Failed(e.to_string()))?;
        Ok(applied(&r, self.warnings(config, zone, false)))
    }

    /// The changes in `config` that take a restart, as fit warns of them. With `set`, the access
    /// log takes `config`'s level and time zone (`zone`).
    fn warnings(&self, config: &Config, zone: Option<Zone>, set: bool) -> Vec<String> {
        let mut warnings = Vec::new();
        match (self.log, &config.access_log, zone) {
            (Some((l, first)), Some(c), Some(zone)) => {
                if set {
                    l.set_min_status(c.min_status());
                    l.set_zone(zone);
                }
                if (&c.output, c.format, c.flush_interval, c.buffer_bytes)
                    != (
                        &first.output,
                        first.format,
                        first.flush_interval,
                        first.buffer_bytes,
                    )
                {
                    warnings.push(
                        "access_log's output, format, flush_interval and buffer_bytes take a restart; the running ones stay"
                            .to_owned(),
                    );
                }
            }
            (None, Some(_), _) => {
                warnings.push(
                    "access_log takes a restart: requests are not logged until then".to_owned(),
                );
            }
            _ => {}
        }
        warnings.extend(akit_admin::admin_warning(self.admin, config.admin.as_ref()));
        if config.acme.as_ref() != self.acme_config {
            warnings.push(
                "acme's settings take a restart; the running ones stay (its names follow every reload)"
                    .to_owned(),
            );
        }
        warnings
    }

    /// Compiles `config` and moves the listeners to it; the files its certificates came from.
    fn swap(
        &mut self,
        config: &Config,
    ) -> Result<(akit_listener::Reloaded, CertFiles), ApplyError> {
        let graph = compile_config(
            config,
            self.slot,
            self.log.map(|l| &l.0),
            self.metrics,
            Some(self.acme),
        )
        .map_err(failure)?;
        let files = graph.cert_files().clone();
        let names = graph.acme_names().to_vec();
        let r = self
            .listeners
            .reload(graph)
            .map_err(|e| ApplyError::Failed(e.to_string()))?;
        // Once the graph is current, as fit's manageACME.
        self.acme.manage(&names);
        Ok((r, files))
    }

    /// Applies `text`, a config as standard JSON from the Admin API, or with [`Mode::DryRun`]
    /// only checks it.
    fn apply_text(&mut self, text: &str, mode: Mode) -> Result<Applied, ApplyError> {
        let read = akit_config::from_json(text, None)
            .map_err(ApplyError::Config)
            .and_then(|config| {
                let zone = time_zone(&config).map_err(failure)?;
                Ok((config, zone))
            });
        match (read, mode) {
            (Ok((config, zone)), Mode::Apply) => self.apply(&config, zone),
            (Ok((config, zone)), Mode::DryRun) => self.check(&config, zone),
            (Err(e), Mode::Apply) => {
                self.stats.reloaded(false);
                Err(e)
            }
            (Err(e), Mode::DryRun) => Err(e),
        }
    }
}

/// A failure to compile, as the Admin API reports it.
fn failure(f: Failure) -> ApplyError {
    match f {
        Failure::Config(errs) => ApplyError::Config(errs),
        Failure::Registry(e) => ApplyError::Failed(e),
    }
}

/// What a reload changed, as the Admin API reports it, with `warnings`.
fn applied(r: &akit_listener::Reloaded, warnings: Vec<String>) -> Applied {
    let change = |l: &akit_listener::Served| ListenerChange {
        name: l.name.clone(),
        address: l.address.to_string(),
        protocols: l.protocols.iter().map(|p| p.as_str().to_owned()).collect(),
    };
    Applied {
        added: r.added.iter().map(change).collect(),
        removed: r.removed.iter().map(change).collect(),
        warnings,
    }
}

/// SIGHUP: reads the config at `paths` again and, when it compiles and its listeners can be
/// served, applies it as a new version; otherwise the running config stays. Says what happened
/// as fit says it. The file provider, when there is one, is told what was read, so that it does
/// not apply the same again.
fn reload(
    paths: &[String],
    configs: &Configs,
    runner: &mut Runner<'_>,
    watcher: Option<&mut Watcher>,
) {
    let name = paths.join(", ");
    println!("akit: reloading {name}");
    let files = match akit_config::read_config_files(paths) {
        Ok(f) => f,
        Err(errs) => {
            runner.stats.reloaded(false);
            eprintln!("akit: reload failed; the running config stays");
            print_errors(&name, &errs);
            return;
        }
    };
    if let Some(w) = watcher {
        w.applied(&files);
    }
    apply_files(&name, &files, configs, runner);
}

/// Applies the config `files` hold, named `name`, as a new version from the files, for SIGHUP and
/// `-watch` (I29); on any failure the running config stays. Says what happened as fit says it.
fn apply_files(name: &str, files: &[ConfigFile], configs: &Configs, runner: &mut Runner<'_>) {
    let (config, json, zone) = match load(files) {
        Ok(r) => r,
        Err(f) => {
            runner.stats.reloaded(false);
            eprintln!("akit: reload failed; the running config stays");
            f.print(name);
            return;
        }
    };
    match configs.update(akit_admin::SOURCE_FILE, None, json, |_| {
        runner.apply(&config, zone)
    }) {
        Ok(o) => {
            print_applied(&o);
            println!("akit: reloaded {name} (config version {})", o.version);
        }
        Err(Refused::Apply(ApplyError::Config(errs))) => {
            eprintln!("akit: reload failed; the running config stays");
            print_errors(name, &errs);
        }
        Err(Refused::Apply(ApplyError::Failed(e))) => {
            eprintln!("akit: reload failed; the running config stays: {e}");
        }
        // No If-Match was given, and nothing but the config is changed.
        Err(Refused::Changed | Refused::Request { .. }) => {}
    }
}

/// `-watch-certs`: the certificate files changed, so the running config is applied again as a
/// new version, as fit's `Configs.Reapply`, loading them anew; on failure the running config, and
/// its certificates, stay. Says what happened as fit says it.
fn reload_certs(configs: &Configs, runner: &mut Runner<'_>) -> Result<(), ()> {
    println!("akit: certificate files changed; reloading");
    let data = configs.current().data().to_owned();
    match configs.update(akit_admin::SOURCE_CERTIFICATES, None, data, |text| {
        runner.apply_text(text, Mode::Apply)
    }) {
        Ok(o) => {
            print_applied(&o);
            println!(
                "akit: reloaded certificate files (config version {})",
                o.version
            );
            Ok(())
        }
        Err(Refused::Apply(ApplyError::Config(errs))) => {
            eprintln!("akit: reload failed; the running config stays");
            print_errors("the running config", &errs);
            Err(())
        }
        Err(Refused::Apply(ApplyError::Failed(e))) => {
            eprintln!("akit: reload failed; the running config stays: {e}");
            Err(())
        }
        // No If-Match was given, and nothing but the config is changed.
        Err(Refused::Changed | Refused::Request { .. }) => Err(()),
    }
}

/// What applying a config changed: the listeners closed and opened, and its warnings.
fn print_applied(o: &Outcome) {
    for l in &o.applied.removed {
        println!("akit: listener {:?} on {} removed", l.name, l.address);
    }
    for l in &o.applied.added {
        println!(
            "akit: listener {:?} on {} ({})",
            l.name,
            l.address,
            l.protocols.join(", ")
        );
    }
    for w in &o.applied.warnings {
        eprintln!("akit: {w}");
    }
}

/// `akit: listener "web" on 127.0.0.1:8080 (http/1.1, h2c)`.
fn print_listener(l: &akit_listener::Served) {
    let protocols: Vec<&str> = l.protocols.iter().map(|p| p.as_str()).collect();
    println!(
        "akit: listener {:?} on {} ({})",
        l.name,
        l.address,
        protocols.join(", ")
    );
}

/// How often the control thread retires replaced graphs while some are still in use.
const COLLECT_EVERY: Duration = Duration::from_millis(100);

fn usage_error(u: &Usage) -> ExitCode {
    eprintln!("{}", u.0);
    ExitCode::from(2)
}

/// `akit validate`: the config is read and compiled (every module provisioned, which checks its
/// params) and released.
fn validate(args: &[String]) -> ExitCode {
    let flags = match flags::parse("validate", args, VALIDATE_FLAGS) {
        Ok(f) => f,
        Err(u) => return usage_error(&u),
    };
    let paths = config_paths(&flags);
    // No engine: an upstream's pool is made, and never dials.
    match read(&paths, &EngineSlot::new(), None).map_err(|f| f.print(&paths.join(", "))) {
        Ok(_) => {
            println!("{}: valid", paths.join(", "));
            ExitCode::SUCCESS
        }
        Err(()) => ExitCode::FAILURE,
    }
}

/// What the control thread is asked to do: by the signal thread, and by the Admin API.
enum Control {
    /// SIGINT or SIGTERM.
    Stop,
    /// SIGHUP.
    Hangup,
    /// A config loaded through the Admin API.
    Admin(akit_admin::Job),
}

/// How many requests may wait for the control thread: the Admin API's connections, each with
/// one request at a time, and signals. Past it, a load is answered 503.
const CONTROL_QUEUE: usize = 64;

/// Delivers SIGINT, SIGTERM and SIGHUP on `tx`, read from akio's signal pipe by a thread of its
/// own (the handlers only write a byte).
fn signals(tx: mpsc::SyncSender<Control>) -> std::io::Result<()> {
    use akio_sys::process::{SIGHUP, SIGINT, SIGTERM, signal_pipe};
    let mut pipe = signal_pipe(&[SIGINT, SIGTERM, SIGHUP])?;
    std::thread::Builder::new()
        .name("akit-signals".into())
        .spawn(move || {
            let mut b = [0u8; 1];
            while pipe.read_exact(&mut b).is_ok() {
                let sig = i32::from(b[0]);
                let s = if sig == SIGHUP {
                    Control::Hangup
                } else {
                    Control::Stop
                };
                if tx.send(s).is_err() {
                    break;
                }
            }
        })?;
    Ok(())
}

/// `akit run`.
fn run(args: &[String]) -> ExitCode {
    let flags = match flags::parse("run", args, RUN_FLAGS) {
        Ok(f) => f,
        Err(u) => return usage_error(&u),
    };
    let mut grace = Duration::from_secs(30);
    let mut watch = false;
    let mut watch_certs = true;
    let mut watch_interval = akit_tls::DEFAULT_WATCH_INTERVAL;
    for (name, value) in &flags {
        match *name {
            "c" => {}
            "watch" => watch = value == "true",
            "watch-certs" => watch_certs = value == "true",
            "watch-interval" => match flags::parse_duration(value) {
                Some(d) if !d.is_zero() => watch_interval = d,
                _ => {
                    return usage_error(&Usage(format!(
                        "invalid value {value:?} for flag -watch-interval: parse error\n{}",
                        flags::usage("run", RUN_FLAGS)
                    )));
                }
            },
            "shutdown-timeout" => match flags::parse_duration(value) {
                Some(d) => grace = d,
                None => {
                    return usage_error(&Usage(format!(
                        "invalid value {value:?} for flag -shutdown-timeout: parse error\n{}",
                        flags::usage("run", RUN_FLAGS)
                    )));
                }
            },
            other => {
                eprintln!("akit run: not supported yet: -{other}");
                return ExitCode::from(2);
            }
        }
    }
    let paths = config_paths(&flags);
    // The pools are made with the graph, before the engine they dial through, which is built
    // from the graph's listeners: they reach it through this slot, filled once it is built.
    let slot = EngineSlot::new();
    let name = paths.join(", ");
    let ((config, json, zone), files) = match read_config(&paths) {
        Ok(c) => c,
        Err(f) => {
            f.print(&name);
            return ExitCode::FAILURE;
        }
    };
    // Opened before the graph, which writes to it, and closed after the engine, so that every
    // response finished has its line written.
    let log = match (&config.access_log, zone) {
        (Some(c), Some(zone)) => match open_access_log(c, zone) {
            Ok(l) => {
                if !matches!(c.output.as_str(), "stdout" | "stderr") {
                    eprintln!(
                        "akit: access log file {} has no rotate: it will grow without bound",
                        c.output
                    );
                }
                Some((l, c.clone()))
            }
            Err(e) => {
                eprintln!("akit run: access log: {e}");
                return ExitCode::FAILURE;
            }
        },
        _ => None,
    };
    // One registry for the process, as fit's: the graphs share their upstreams' counters through
    // it, and with `metrics` count their requests in it (I37).
    let metrics = Metrics::new();
    let stats = ConfigStats::new();
    // One ACME manager for the process, from the `acme` akit starts with, as fit's (I31): it runs
    // no thread until a listener lists a name in `tls.acme`.
    let acme_config = config.acme.clone();
    let acme = match akit_tls::Acme::new(&acme_config.clone().unwrap_or_default()) {
        Ok(a) => a,
        Err(e) => {
            eprintln!("akit run: {e}");
            return ExitCode::FAILURE;
        }
    };
    let graph = match compile_config(
        &config,
        &slot,
        log.as_ref().map(|l| &l.0),
        &metrics,
        Some(&acme),
    ) {
        Ok(g) => g,
        Err(f) => {
            f.print(&name);
            return ExitCode::FAILURE;
        }
    };
    let admin = config.admin.clone();
    drop(config);
    let cert_files = graph.cert_files().clone();
    let acme_names = graph.acme_names().to_vec();
    let configs = Configs::new(
        json,
        akit_admin::SOURCE_FILE,
        admin.as_ref().map_or(1, |a| a.history),
    );
    // Before the listeners: a signal from then on shuts down gracefully.
    let (tx, control) = mpsc::sync_channel(CONTROL_QUEUE);
    if let Err(e) = signals(tx.clone()) {
        eprintln!("akit run: signals: {e}");
        return ExitCode::FAILURE;
    }
    let cell = GraphCell::new(graph);
    let mut opts = akit_listener::Options::default();
    opts.shutdown_timeout = grace;
    let engine = match akit_listener::engine(&cell, &opts) {
        Ok(e) => Arc::new(e),
        Err(e) => {
            eprintln!("akit run: {e}");
            return ExitCode::FAILURE;
        }
    };
    if let Err(e) = engine.start() {
        eprintln!("akit run: listening: {e}");
        return ExitCode::FAILURE;
    }
    // Upstream connections are dialed on the listeners' threads.
    slot.set(Arc::clone(&engine));
    let mut listeners = akit_listener::Listeners::new(Arc::clone(&engine), cell.clone(), &opts);
    for l in listeners.served() {
        print_listener(&l);
    }
    acme.manage(&acme_names);
    let api = match &admin {
        None => None,
        Some(a) => {
            let tx = tx.clone();
            let submit: akit_admin::Submit = Box::new(move |job| {
                match tx.try_send(Control::Admin(job)) {
                    Ok(()) => Ok(()),
                    Err(
                        mpsc::TrySendError::Full(Control::Admin(job))
                        | mpsc::TrySendError::Disconnected(Control::Admin(job)),
                    ) => Err(job),
                    // Only a job is sent here.
                    Err(_) => Ok(()),
                }
            });
            // Every metric, at GET /metrics, as fit's Admin API serves them (I37).
            let scrape = akit_listener::metrics::Scrape::new(
                cell.clone(),
                Arc::clone(&engine),
                stats.clone(),
            )
            .with_access_log(log.as_ref().map(|l| l.0.clone()));
            let mut api_opts = akit_admin::Options::default();
            api_opts.metrics = Some(Arc::new(move |out: &mut Vec<u8>| scrape.write(out)));
            match akit_admin::Server::start(a, configs.clone(), submit, &api_opts) {
                Ok(s) => {
                    println!("akit: admin API on {}", s.addr());
                    Some(s)
                }
                Err(e) => {
                    eprintln!("akit run: admin: {e}");
                    akit_listener::shutdown(&engine, grace);
                    return ExitCode::FAILURE;
                }
            }
        }
    };
    drop(tx);
    let mut runner = Runner {
        slot: &slot,
        listeners: &mut listeners,
        log: log.as_ref(),
        admin: admin.as_ref(),
        cert_files,
        acme: &acme,
        acme_config: acme_config.as_ref(),
        metrics: &metrics,
        stats: &stats,
    };
    // The certificate files watched (fit's -watch-certs), on this thread, every interval: the
    // running config's version number and the files it loaded tell the watcher what it loaded.
    let mut watcher = watch_certs.then(|| FileWatcher::new(akit_tls::DEFAULT_WATCH_DEBOUNCE));
    // The config's files watched (fit's -watch, I29), on this thread, every interval, each change
    // applied through the same `Configs` as SIGHUP and the Admin API.
    let mut provider = watch.then(|| {
        let mut opts = WatchOptions::default();
        opts.expand = Some(Box::new(|paths: &[String]| {
            akit_config::config_files(paths)
                .map(|fs| fs.iter().map(|f| f.display().to_string()).collect())
                .map_err(|e| e.to_string())
        }));
        Watcher::new(&paths, &files, opts)
    });
    drop(files);
    if provider.is_some() {
        println!("akit: watching {name}");
    }
    let mut at = Instant::now();
    loop {
        // Graphs replaced by a reload are retired from here once no request uses them
        // (decision 004): after each swap, and on a timer while any is left.
        let mut wait = (cell.retiring() > 0).then_some(COLLECT_EVERY);
        if watcher.is_some() || provider.is_some() {
            let until = at.saturating_duration_since(Instant::now());
            wait = Some(wait.map_or(until, |w| w.min(until)));
        }
        let next = match wait {
            None => control.recv().ok(),
            Some(wait) => match control.recv_timeout(wait) {
                Ok(c) => Some(c),
                Err(mpsc::RecvTimeoutError::Timeout) => {
                    let now = Instant::now();
                    if now >= at && (watcher.is_some() || provider.is_some()) {
                        at = now.checked_add(watch_interval).unwrap_or(now);
                        if let Some(p) = &mut provider {
                            let r = p.tick(now, |files| {
                                println!("akit: {name} changed; reloading");
                                apply_files(&name, files, &configs, &mut runner);
                                Ok::<(), ()>(()) // apply_files said what happened
                            });
                            if let Some(WatchReport::Unreadable(e)) = r {
                                eprintln!("akit: watching {name}: {e}");
                            }
                        }
                        if let Some(w) = &mut watcher {
                            let files = runner.cert_files.clone();
                            let version = configs.current().number;
                            if let Some(Report::Unreadable(e)) =
                                w.tick(now, version, &files, || reload_certs(&configs, &mut runner))
                            {
                                eprintln!(
                                    "akit: certificate files: {e}; the running certificates stay"
                                );
                            }
                            // A failed reload said why.
                        }
                    }
                    if cell.retiring() > 0 {
                        cell.collect();
                    }
                    continue;
                }
                Err(mpsc::RecvTimeoutError::Disconnected) => None,
            },
        };
        match next {
            Some(Control::Stop) | None => break,
            Some(Control::Hangup) => {
                reload(&paths, &configs, &mut runner, provider.as_mut());
                cell.collect();
            }
            Some(Control::Admin(job)) => {
                if let Some(o) = job.run(&configs, |text, mode| runner.apply_text(text, mode)) {
                    print_applied(&o);
                    println!(
                        "akit: loaded a config from the admin API (config version {})",
                        o.version
                    );
                }
                cell.collect();
            }
        }
    }
    // The API first: no more loads. Those still waiting are refused.
    if let Some(api) = &api {
        api.shutdown();
    }
    // No more certificates obtained: its work under way is given up.
    acme.close();
    // A second SIGINT or SIGTERM does not wait for the shutdown, as fit's.
    std::thread::spawn(move || {
        while let Ok(c) = control.recv() {
            match c {
                Control::Stop => {
                    eprintln!("akit run: stopped by a second signal");
                    std::process::exit(1);
                }
                Control::Admin(job) => job.refuse(503, "akit is shutting down"),
                Control::Hangup => {}
            }
        }
    });
    println!("akit: shutting down");
    if !akit_listener::shutdown(&engine, grace) {
        eprintln!(
            "akit run: shutdown took longer than {}; connections still open were closed",
            flags::format_duration(grace)
        );
        if let Some((l, _)) = &log {
            l.close();
        }
        return ExitCode::FAILURE;
    }
    drop(listeners);
    drop(engine);
    drop(cell);
    if let Some((l, _)) = &log {
        l.close();
    }
    println!("akit: stopped");
    ExitCode::SUCCESS
}

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let Some((cmd, rest)) = args.split_first() else {
        eprint!("{USAGE}");
        return ExitCode::from(2);
    };
    match cmd.as_str() {
        "run" => run(rest),
        "validate" => validate(rest),
        "version" | "-V" | "--version" => {
            if let Err(u) = flags::parse("version", rest, &[]) {
                return usage_error(&u);
            }
            println!("{}", version());
            ExitCode::SUCCESS
        }
        "help" | "-h" | "-help" | "--help" => {
            print!("{USAGE}");
            ExitCode::SUCCESS
        }
        other => {
            eprint!("akit: unknown command {other:?}\n\n{USAGE}");
            ExitCode::from(2)
        }
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn version_names_akit_akio_and_the_platform() {
        let v = super::version();
        let lines: Vec<&str> = v.lines().collect();
        assert!(lines[0].starts_with("akit "));
        assert_eq!(lines[1], "akio e9891b52c0e9");
        assert_eq!(
            lines[2],
            format!("{}/{}", std::env::consts::OS, std::env::consts::ARCH)
        );
    }
}
