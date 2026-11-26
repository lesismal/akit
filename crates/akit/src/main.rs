#![forbid(unsafe_code)]
//! The `akit` command line, which the library holds as its `cli` module and calls from
//! [`main`] (the binary is `src/bin/akit.rs`, and one `akit build` makes is the same with modules
//! of its own besides), with fit's commands, flags and exit codes: 0 when it worked, 1 when it did
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
//!   With `telemetry` (I64), requests are traced and their spans, with the metrics, exported over
//!   OTLP/HTTP, by threads of their own, from the `telemetry` akit starts with: a change takes a
//!   restart.
//! - `validate`: reads and compiles the config, as `run` would, and says whether it is valid.
//! - `adapt` and `fmt` (I73): convert a config between `.conf`, YAML and JSON, and write a
//!   config's files again in their own formats, as fit writes them ([`adapt`]).
//! - `build` (I70): builds an akit with more modules, or fewer, into a binary ([`build_cli`],
//!   [`crate::build`]), as fit's `fit build`.
//! - `version`, `help`.

mod adapt;
mod build_cli;
pub(crate) mod flags;
mod hash_password;

use std::io::Read;
use std::process::ExitCode;
use std::sync::{Arc, OnceLock, mpsc};
use std::time::{Duration, Instant};

use akit_admin::{
    Applied, ApplyError, Configs, ListenerChange, Mode, Outcome, Refused, WatchOptions,
    WatchReport, Watcher,
};
use akit_config::{Config, ConfigFile, Error, Errors, LogFormat};
use akit_observe::metrics::{ConfigStats, Metrics};
use akit_observe::{AccessLog, Files, Format, LogFile, MetricsExporter, Otlp, Tracer, Zone};
use akit_provider::{Kinds, Provider, Updates};
use akit_runtime::upstream::EngineSlot;
use akit_runtime::{Env, Graph, GraphCell, HandlerParams, Info, Previous, Registry, compile};
use akit_tls::{FileWatcher, Report};
use flags::{Spec, Usage};

const USAGE: &str = "\
akit: a reverse proxy, load balancer and API gateway on akio

Usage:
  akit <command> [flags]

Commands:
  run        run a config
  validate   check a config, without running it
  adapt      convert a config between .conf, YAML and native JSON, or an
             nginx config or a Caddyfile into one of them
  fmt        write a config's files again, each in its own format
  build      build an akit with more modules, or fewer, as akit build -h says
  hash-password
             print the bcrypt hash of a password read from standard input, for
             admin.console's users and basic_auth
  version    print akit's version and the akio commit it is built on
  help       print this help

Run akit <command> -h for a command's flags.
";

const CONFIG_USAGE: &str = "a config file, or a directory of them, to use; give -c again for more, which are merged (fit.json when none is given). A file ending in .conf is fit's .conf format, one ending in .yaml or .yml YAML, any other JSON (comments and trailing commas allowed); a directory's files are its .conf, .yaml, .yml and .json files, in its subdirectories too";

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

/// The version [`main`] was given, and the module lists the registry is made from: set once,
/// when the process starts, before any config is read.
static BUILD: OnceLock<(String, Vec<&'static [Info<HandlerParams>]>)> = OnceLock::new();

fn version() -> String {
    format!(
        "akit {}\nakio {}\n{}/{}",
        BUILD.get().map_or(crate::VERSION, |(v, _)| v.as_str()),
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

/// Prints a config's warnings (I79): the old names of renamed fields, at their files, lines and
/// columns, as fit prints them; `name` names the files of one without a file.
fn print_warnings(name: &str, w: &akit_config::Warnings) {
    for e in &w.0 {
        if e.file.is_some() {
            eprintln!("{e}");
        } else {
            eprintln!("{name}: {e}");
        }
    }
}

/// The config files the flags name, `fit.json` when none.
pub(crate) fn config_paths(flags: &[(&str, String)]) -> Vec<String> {
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

/// The zone `config`'s `log.time_zone` names, loaded (I82): the local zone when it has no `log`.
fn log_zone(config: &Config) -> Result<Zone, Failure> {
    let tz = config.log.as_ref().map_or("", |l| l.time_zone.as_str());
    Zone::load(tz).map_err(|e| {
        Failure::Config(Errors(vec![Error {
            file: None,
            path: "log.time_zone".into(),
            line: 0,
            col: 0,
            msg: e.to_string(),
        }]))
    })
}

/// The zone `config`'s access log's `time_zone` names, loaded; `log.time_zone` is checked too.
fn time_zone(config: &Config) -> Result<Option<Zone>, Failure> {
    log_zone(config)?;
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
    tracer: Option<&Tracer>,
    previous: Option<Previous>,
) -> Result<Graph, Failure> {
    let modules = BUILD.get().map_or(crate::STANDARD, |(_, m)| m.as_slice());
    let registry: Registry<HandlerParams> =
        Registry::new(modules).map_err(|e| Failure::Registry(e.to_string()))?;
    let env = Env::new(engine.clone())
        .with_access_log(log.cloned())
        .with_metrics(Some(metrics.clone()))
        .with_acme(acme.cloned())
        .with_tracer(tracer.cloned())
        .with_previous(previous);
    compile(config, &registry, &env).map_err(Failure::Config)
}

/// Opens the access log `c` describes, its times in `zone`, as fit's `NewAccessLogFiles`: a file
/// output through the sink `files` keeps for its path (I81), rotated as `c.rotate` says; with
/// the file's sink, which a reload changes the rotation and zone of.
fn open_access_log(
    c: &akit_config::AccessLog,
    zone: Zone,
    files: &Files,
) -> std::io::Result<(AccessLog, Option<LogFile>)> {
    let mut opts = akit_observe::Options::new();
    opts.format = match c.format {
        LogFormat::Json => Format::Json,
        LogFormat::Text => Format::Text,
        LogFormat::Line => Format::Line,
    };
    opts.zone = zone.clone();
    opts.min_status = c.min_status();
    opts.flush_interval = c.flush_interval;
    opts.buffer_bytes = c.buffer_bytes;
    if !akit_config::is_file_output(&c.output) {
        let out = akit_observe::open_output(&c.output)?;
        return Ok((AccessLog::new(out, opts)?, None));
    }
    let file = files.open(&c.output, akit_config::rotation(c.rotate.as_ref()), zone)?;
    let held = files.get(&c.output);
    Ok((AccessLog::to_file(file, opts)?, held))
}

/// Each logger's output, cleaned, and format: what a restart, not a reload, changes.
fn log_shape(l: Option<&akit_config::Log>) -> Vec<(String, LogFormat)> {
    let default = akit_config::Log::default();
    let l = l.unwrap_or(&default);
    akit_config::LOGGERS
        .iter()
        .map(|n| {
            let out = l.output_of(n);
            let out = if akit_config::is_file_output(out) {
                akit_config::output_path(out)
            } else {
                out.to_owned()
            };
            (out, l.format_of(n))
        })
        .collect()
}

/// The certificate, key and authority files a config loaded, with their SHA-256, by path.
type CertFiles = std::collections::BTreeMap<String, akit_tls::Sum>;

/// What applying a config takes: the engine slot its pools dial through, the listeners it moves,
/// the access log opened at start (with the config it was opened from), and the `admin` akit
/// started with; and what the running config loaded from certificate files, which
/// `-watch-certs` compares the files with.
struct Runner<'a> {
    slot: &'a EngineSlot,
    /// The running graph, whose unchanged upstreams' pools the next one shares.
    cell: &'a GraphCell,
    listeners: &'a mut akit_listener::Listeners,
    log: Option<&'a (AccessLog, akit_config::AccessLog)>,
    /// The access log's file, when it writes one (I81): a reload changes its rotation and zone.
    access_file: Option<&'a LogFile>,
    /// akit's own log (I82), and the `log` it was made from: a reload changes its levels, zone
    /// and files' rotation in place (I83); its outputs and formats take a restart.
    root: &'a akit_observe::logger::Root,
    log_config: Option<&'a akit_config::Log>,
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
    /// The `telemetry` akit started with, and the tracer made from it (I64): each graph samples
    /// its requests with it; a config's `telemetry` takes a restart, as fit's.
    telemetry: Option<&'a akit_config::Telemetry>,
    tracer: Option<&'a Tracer>,
    /// The `providers` akit started with (I58, I59): a config's take a restart, as fit's.
    providers: &'a [akit_config::Provider],
    /// The `limits` and `engine` akit started with (I79): `max_conns`, `max_pending_bytes`
    /// (I79b), `max_write_queue_bytes` and `engine` take a restart, as fit's.
    limits: akit_config::Limits,
    engine: akit_config::Engine,
    /// The Admin API akit started with, whose access lists follow every config applied (I87).
    api: Option<&'a akit_admin::Server>,
}

impl Runner<'_> {
    /// Compiles `config` and moves the server to it (I22's reload), as one step: on any failure
    /// the running config stays. The access log takes the new config's level and time zone
    /// (`zone`); its other settings, and `admin`, take a restart, as fit's, and are warned of.
    fn apply(
        &mut self,
        config: &Config,
        json: &str,
        zone: Option<Zone>,
    ) -> Result<Applied, ApplyError> {
        let r = self.swap(config);
        self.stats.reloaded(r.is_ok());
        let (r, files) = r?;
        self.cert_files = files;
        let warnings = self.warnings(config, json, zone, true);
        Ok(applied(&r, warnings))
    }

    /// Checks `config` as [`apply`](Runner::apply) would apply it, changing nothing (a dry run of
    /// the Admin API, I27, as fit's): compiled with no engine, so that its pools dial nothing
    /// and probe nothing, and its new listeners' addresses tried (`Listeners::plan`). Not counted
    /// as a reload.
    fn check(
        &self,
        config: &Config,
        json: &str,
        zone: Option<Zone>,
    ) -> Result<Applied, ApplyError> {
        let graph = compile_config(
            config,
            &EngineSlot::new(),
            None,
            &Metrics::new(),
            None,
            None,
            None,
        )
        .map_err(failure)?;
        let r = self
            .listeners
            .plan(&graph)
            .map_err(|e| ApplyError::Failed(e.to_string()))?;
        Ok(applied(&r, self.warnings(config, json, zone, false)))
    }

    /// The changes in `config` (`json` as standard JSON) that take a restart, as fit warns of
    /// them, and what it exposes (I80b). With `set`, the access log takes `config`'s level and
    /// time zone (`zone`).
    fn warnings(&self, config: &Config, json: &str, zone: Option<Zone>, set: bool) -> Vec<String> {
        // The old names of renamed fields first, as fit's load returns them (I79).
        let mut warnings: Vec<String> = config.warnings.0.iter().map(ToString::to_string).collect();
        // Secret files open to others, credentials in the clear (I80b).
        warnings.extend(akit_config::hardening::warnings(config, json));
        // akit's own log (I83, fit's Root.SetConfig): levels, zone, rotation and retention in
        // place; a logger's output or format takes a restart.
        if set {
            if let Ok(z) = log_zone(config) {
                self.root.set_config(config.log.as_ref(), z);
            }
        }
        if log_shape(config.log.as_ref()) != log_shape(self.log_config) {
            warnings
                .push("log's outputs and formats take a restart; the running ones stay".to_owned());
        }
        match (self.log, &config.access_log, zone) {
            (Some((l, first)), Some(c), Some(zone)) => {
                if set {
                    l.set_min_status(c.min_status());
                    l.set_zone(zone.clone());
                    // The file's rotation and zone change in place, as fit's (I81).
                    if let Some(f) = self.access_file {
                        f.set_zone(zone);
                        f.set_rotation(akit_config::rotation(c.rotate.as_ref()));
                    }
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
        if config.telemetry.as_ref() != self.telemetry {
            warnings.push("telemetry takes a restart; the running one stays".to_owned());
        }
        if config.providers != self.providers {
            warnings.push("providers take a restart; the running ones stay".to_owned());
        }
        let l = &config.limits;
        if (l.max_conns, l.max_write_queue_bytes)
            != (self.limits.max_conns, self.limits.max_write_queue_bytes)
        {
            warnings.push(
                "limits.max_conns and limits.max_write_queue_bytes take a restart; the running ones stay"
                    .to_owned(),
            );
        }
        if l.max_pending_bytes != self.limits.max_pending_bytes {
            warnings
                .push("limits.max_pending_bytes takes a restart; the running one stays".to_owned());
        }
        if config.engine != self.engine {
            warnings.push("engine takes a restart; the running one stays".to_owned());
        }
        warnings
    }

    /// Compiles `config` and moves the listeners to it; the files its certificates came from.
    fn swap(
        &mut self,
        config: &Config,
    ) -> Result<(akit_listener::Reloaded, CertFiles), ApplyError> {
        // The running graph's pools, for the upstreams that stay as they are (fit's
        // `Env.Previous`): their connections and health carry over the swap.
        let previous = self.cell.with(Previous::of);
        let graph = compile_config(
            config,
            self.slot,
            self.log.map(|l| &l.0),
            self.metrics,
            Some(self.acme),
            self.tracer,
            Some(previous),
        )
        .map_err(failure)?;
        let files = graph.cert_files().clone();
        let names = graph.acme_names().to_vec();
        let admin_access = graph.admin_access().cloned();
        let console_access = graph.console_access().cloned();
        let r = self
            .listeners
            .reload(graph)
            .map_err(|e| ApplyError::Failed(e.to_string()))?;
        // Once the graph is current, as fit's manageACME.
        self.acme.manage(&names);
        // The Admin API's access lists follow the config applied, as fit's `setAccess` (I87).
        if let Some(api) = self.api {
            api.set_access(admin_access.as_ref());
            // The console's too (I90).
            api.set_console_access(console_access.as_ref());
        }
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
            (Ok((config, zone)), Mode::Apply) => self.apply(&config, text, zone),
            (Ok((config, zone)), Mode::DryRun) => self.check(&config, text, zone),
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
    match configs.update(akit_admin::SOURCE_FILE, None, json, |text| {
        runner.apply(&config, text, zone)
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
    match configs.reapply(
        akit_admin::SOURCE_CERTIFICATES,
        None,
        Mode::Apply,
        |text, mode| runner.apply_text(text, mode),
    ) {
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

/// Makes and starts the providers `pcs` lists, as fit's `startProviders`: each named to
/// `configs`, in their order of precedence, and sending its parts through `updates`.
fn start_providers(
    pcs: &[akit_config::Provider],
    configs: &Configs,
    updates: &Updates,
) -> Result<Vec<Box<dyn Provider>>, String> {
    let kinds = Kinds::new(&[
        akit_provider::docker::KINDS,
        akit_provider::kubernetes::KINDS,
        akit_provider::API_KINDS,
    ])?;
    let mut out: Vec<Box<dyn Provider>> = Vec::with_capacity(pcs.len());
    for pc in pcs {
        let name = &pc.name;
        let kind = pc.kind.name();
        let Some(make) = kinds.lookup(kind) else {
            return Err(format!(
                "provider {name}: this akit was built without the {kind} provider (it has {})",
                kinds.names().join(", ")
            ));
        };
        let (mut p, watching) = make(name, pc)?;
        println!("akit: provider {name} {watching}");
        configs.add_provider(name)?;
        p.start(updates.update(name))
            .map_err(|e| format!("provider {name}: {e}"))?;
        out.push(p);
    }
    Ok(out)
}

/// Applies the parts providers sent (I57), each merged into the running base config as a new
/// version whose source is the provider, at most one per provider that has one waiting, so that
/// the control thread goes back to its other work in between; a part refused leaves the
/// provider's last in place. Says what happened as fit says it.
fn provide(updates: &Updates, configs: &Configs, runner: &mut Runner<'_>) {
    for _ in 0..updates.pending() {
        let Some((name, data)) = updates.take() else {
            break;
        };
        match configs.provide(&name, &data, |text, mode| runner.apply_text(text, mode)) {
            Ok(o) if o.unchanged => {}
            Ok(o) => {
                print_applied(&o);
                println!(
                    "akit: applied what provider {name} provides (config version {})",
                    o.version
                );
            }
            Err(Refused::Apply(ApplyError::Config(errs))) => {
                eprintln!("akit: provider {name}: refused; what it provided before stays");
                print_errors(&format!("provider {name}"), &errs);
            }
            Err(Refused::Apply(ApplyError::Failed(e))) => {
                eprintln!("akit: provider {name}: refused; what it provided before stays: {e}");
            }
            Err(Refused::Request { msg, .. }) => eprintln!("akit: provider {name}: {msg}"),
            // No If-Match is given.
            Err(Refused::Changed) => {}
        }
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
    let checked = read_config(&paths).and_then(|((config, json, _), _)| {
        print_warnings(&paths.join(", "), &config.warnings);
        for w in akit_config::hardening::warnings(&config, &json) {
            eprintln!("{}: {w}", paths.join(", "));
        }
        compile_config(
            &config,
            &EngineSlot::new(),
            None,
            &Metrics::new(),
            None,
            None,
            None,
        )
    });
    match checked.map_err(|f| f.print(&paths.join(", "))) {
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
    /// Providers' parts wait (I57).
    Provide,
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
    print_warnings(&name, &config.warnings);
    // Secret files open to others, credentials in the clear (I80b).
    for w in akit_config::hardening::warnings(&config, &json) {
        eprintln!("akit: {w}");
    }
    // The log files the process writes, a sink a path (I81), and akit's own log (I82), which
    // akio's records go to as the engine logger's; the access log shares its files.
    let log_files = Files::new();
    let root = match log_zone(&config)
        .map_err(|f| {
            f.print(&name);
        })
        .and_then(|zone| {
            akit_observe::logger::Root::new(config.log.as_ref(), zone, &log_files).map_err(|e| {
                eprintln!("akit run: log: {e}");
            })
        }) {
        Ok(r) => Arc::new(r),
        Err(()) => return ExitCode::FAILURE,
    };
    akit_observe::logger::install(Arc::clone(&root));
    akit_observe::logger::bridge_akio();
    // Closed last, on every way out of `run`.
    let _log_guard = LogGuard(Arc::clone(&root));
    if let Some(l) = &config.log {
        for out in l.file_outputs() {
            if out.rotate.is_none() {
                akit_observe::warn!(
                    Server,
                    "log file {} has no rotate: it will grow without bound",
                    out.path
                );
            }
        }
    }
    // Opened before the graph, which writes to it, and closed after the engine, so that every
    // response finished has its line written.
    let mut access_file = None;
    let log = match (&config.access_log, zone) {
        (Some(c), Some(zone)) => match open_access_log(c, zone, &log_files) {
            Ok((l, f)) => {
                if akit_config::is_file_output(&c.output) && c.rotate.is_none() {
                    akit_observe::warn!(
                        Server,
                        "access log file {} has no rotate: it will grow without bound",
                        c.output
                    );
                }
                access_file = f;
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
    // OpenTelemetry (I64), from the `telemetry` akit starts with, as fit's: its exports run on
    // threads of their own; the tracer is closed after the engine, so that every span finished is
    // exported.
    let telemetry = config.telemetry.clone();
    let otlp = match &telemetry {
        None => None,
        Some(t) => match Otlp::new(t, crate::VERSION, akit_observe::otlp::stderr_log()) {
            Ok(o) => Some(o),
            Err(e) => {
                eprintln!("akit run: telemetry: {e}");
                return ExitCode::FAILURE;
            }
        },
    };
    let traces = telemetry.as_ref().and_then(|t| t.traces.as_ref());
    let tracer = match (&otlp, traces) {
        (Some(o), Some(c)) => match Tracer::new(Arc::clone(o), c) {
            Ok(t) => Some(t),
            Err(e) => {
                eprintln!("akit run: telemetry: {e}");
                return ExitCode::FAILURE;
            }
        },
        _ => None,
    };
    let graph = match compile_config(
        &config,
        &slot,
        log.as_ref().map(|l| &l.0),
        &metrics,
        Some(&acme),
        tracer.as_ref(),
        None,
    ) {
        Ok(g) => g,
        Err(f) => {
            f.print(&name);
            return ExitCode::FAILURE;
        }
    };
    let admin = config.admin.clone();
    let log_config = config.log.clone();
    let provider_configs = config.providers.clone();
    let (start_limits, start_engine) = (config.limits, config.engine);
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
            .with_access_log(log.as_ref().map(|l| l.0.clone()))
            .with_listeners(listeners.view())
            .with_tracer(tracer.clone());
            let mut api_opts = akit_admin::Options::default();
            // The running graph's `admin.access`, then its global access (I87).
            api_opts.access = cell.with(|g| g.admin_access().cloned());
            // And the console's: `admin.console.access`, then the global access (I90).
            api_opts.console_access = cell.with(|g| g.console_access().cloned());
            // What akit is doing now, at GET /status, which the dashboard reads (I63).
            let status = scrape.clone();
            api_opts.status = Some(Arc::new(move |out: &mut String| status.write_status(out)));
            api_opts.metrics = Some(Arc::new(move |out: &mut Vec<u8>| scrape.write(out)));
            // The log files, at GET /logs and POST /logs/rotate and /logs/cleanup (I83).
            api_opts.logs = Some(log_files.clone());
            match akit_admin::Server::start(a, configs.clone(), submit, &api_opts) {
                Ok(s) => {
                    println!("akit: admin API on {}", s.address());
                    if let Some(url) = s.console_url() {
                        println!("akit: console on {url}");
                    }
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
    // Providers' parts (I57) wake the control thread through the same queue; one it could not
    // be given is made up for after the next thing it does, when it takes the parts waiting. The
    // providers' kinds and their config come with I58-I61: none runs yet.
    let updates = {
        let tx = tx.clone();
        Updates::new(move || {
            let _ = tx.try_send(Control::Provide);
        })
    };
    drop(tx);
    // The providers the config lists (I58, I59, I61), in their order of precedence, as fit's
    // startProviders; read when akit starts.
    let mut providers = match start_providers(&provider_configs, &configs, &updates) {
        Ok(p) => p,
        Err(e) => {
            eprintln!("akit run: {e}");
            if let Some(api) = &api {
                api.shutdown();
            }
            akit_listener::shutdown(&engine, grace);
            return ExitCode::FAILURE;
        }
    };
    // The metrics /metrics serves, exported every telemetry.metrics.interval (I64).
    let exporter = match (&otlp, telemetry.as_ref().and_then(|t| t.metrics.as_ref())) {
        (Some(o), Some(m)) => {
            let scrape = akit_listener::metrics::Scrape::new(
                cell.clone(),
                Arc::clone(&engine),
                stats.clone(),
            )
            .with_access_log(log.as_ref().map(|l| l.0.clone()))
            .with_tracer(tracer.clone());
            let write = Box::new(move |w: &mut dyn akit_observe::MetricWriter| {
                scrape.write_metrics(w);
            });
            match MetricsExporter::new(Arc::clone(o), m.interval, write) {
                Ok(e) => Some(e),
                Err(e) => {
                    eprintln!("akit run: telemetry: {e}");
                    akit_listener::shutdown(&engine, grace);
                    return ExitCode::FAILURE;
                }
            }
        }
        _ => None,
    };
    if let Some(t) = &telemetry {
        println!("akit: exporting telemetry to {}", t.endpoint);
    }
    let mut runner = Runner {
        slot: &slot,
        cell: &cell,
        listeners: &mut listeners,
        log: log.as_ref(),
        access_file: access_file.as_ref(),
        root: &root,
        log_config: log_config.as_ref(),
        admin: admin.as_ref(),
        cert_files,
        acme: &acme,
        acme_config: acme_config.as_ref(),
        metrics: &metrics,
        stats: &stats,
        telemetry: telemetry.as_ref(),
        tracer: tracer.as_ref(),
        providers: &provider_configs,
        limits: start_limits,
        engine: start_engine,
        api: api.as_ref(),
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
        if updates.pending() > 0 {
            provide(&updates, &configs, &mut runner);
            cell.collect();
        }
        // Graphs replaced by a reload are retired from here once no request uses them
        // (decision 004): after each swap, and on a timer while any is left.
        let mut wait = (cell.retiring() > 0).then_some(COLLECT_EVERY);
        if updates.pending() > 0 {
            wait = Some(Duration::ZERO);
        }
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
            // Taken at the top of the loop.
            Some(Control::Provide) => {}
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
    // The API first: no more loads. Those still waiting are refused. No provider's part is
    // applied either.
    if let Some(api) = &api {
        api.shutdown();
    }
    updates.close();
    for p in &mut providers {
        p.stop();
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
                Control::Hangup | Control::Provide => {}
            }
        }
    });
    println!("akit: shutting down");
    if !akit_listener::shutdown(&engine, grace) {
        eprintln!(
            "akit run: shutdown took longer than {}; connections still open were closed",
            flags::format_duration(grace)
        );
        close_telemetry(exporter.as_ref(), tracer.as_ref());
        if let Some((l, _)) = &log {
            l.close();
        }
        return ExitCode::FAILURE;
    }
    drop(listeners);
    drop(engine);
    drop(cell);
    close_telemetry(exporter.as_ref(), tracer.as_ref());
    if let Some((l, _)) = &log {
        l.close();
    }
    println!("akit: stopped");
    ExitCode::SUCCESS
}

/// Lets go of akit's own log's files when `run` returns, last: what is logged afterwards goes to
/// standard error, as fit's `Root.Close`.
struct LogGuard(Arc<akit_observe::logger::Root>);

impl Drop for LogGuard {
    fn drop(&mut self) {
        akit_observe::logger::uninstall();
        self.0.close();
    }
}

/// Exports the metrics once more and the spans that wait, and stops their threads, as fit's
/// deferred `Close`s do.
fn close_telemetry(exporter: Option<&MetricsExporter>, tracer: Option<&Tracer>) {
    if let Some(e) = exporter {
        e.close();
    }
    if let Some(t) = tracer {
        t.close();
    }
}

/// Runs the command the process's arguments name, as an akit whose version is `akit_version` and
/// whose registry has the modules of `modules` (the binary's: [`crate::STANDARD`]; one `akit
/// build` makes adds its own, I70), and returns the exit code: 0 when it worked, 1 when it did
/// not, 2 when it was used wrongly. Called once, by the binary's `main`.
pub fn main(akit_version: &str, modules: &[&'static [Info<HandlerParams>]]) -> ExitCode {
    let _ = BUILD.set((akit_version.to_owned(), modules.to_vec()));
    let args: Vec<String> = std::env::args().skip(1).collect();
    let Some((cmd, rest)) = args.split_first() else {
        eprint!("{USAGE}");
        return ExitCode::from(2);
    };
    match cmd.as_str() {
        "run" => run(rest),
        "validate" => validate(rest),
        "adapt" => adapt::adapt(rest),
        "fmt" => adapt::fmt(rest),
        "build" => build_cli::build(rest),
        "hash-password" => hash_password::hash_password(rest),
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
        // The akio commit akit is pinned at, short (rule 22: one pin, `AKIO_REV`).
        assert_eq!(lines[1], format!("akio {}", &akit_runtime::AKIO_REV[..12]));
        assert_eq!(
            lines[2],
            format!("{}/{}", std::env::consts::OS, std::env::consts::ARCH)
        );
    }
}
