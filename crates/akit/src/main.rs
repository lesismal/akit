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
//!   new pool reads them again; so are the Admin API's and the console's certificate, key and
//!   client CA files (I94), loaded once settled and served to new connections without a
//!   restart, the running certificate kept when they do not load (SIGHUP reads them again too).
//!   With `-watch`
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
mod boot;
mod build_cli;
pub(crate) mod flags;
mod hash_password;
mod store_cli;
mod token;

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
  token      issue, list, change and revoke the link tokens of a running akit's
             tunnel hub and LLM roles, through its Admin API
  store      print the config the encrypted store holds, or seal a config as
             the store (fit's fitstore files)
  version    print akit's version and the akio commit it is built on
  help       print this help

Run akit <command> -h for a command's flags.
";

const CONFIG_USAGE: &str = "a config file, or a directory of them, to use; give -c again for more, which are merged (akit.json when none is given). A file ending in .conf is fit's .conf format, one ending in .yaml or .yml YAML, any other JSON (comments and trailing commas allowed); a directory's files are its .conf, .yaml, .yml and .json files, in its subdirectories too";

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
    Spec {
        name: "admin",
        bool: true,
        usage: "serve the Admin API and the console with their defaults where the config leaves them out, and keep the config in a store; with no console user in the config, the user admin logs in with the password admin, which must be changed within -timeout",
    },
    Spec {
        name: "timeout",
        bool: false,
        usage: "with -admin, how long the console's default password, admin, works: it must be changed within that time, before the console or the Admin API can be used, or akit must be restarted (default 30m0s)",
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
pub(crate) fn print_errors(name: &str, errs: &Errors) {
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

/// The config files the flags name, `akit.json` when none (fit's `fit.json`, renamed: decision 037).
pub(crate) fn config_paths(flags: &[(&str, String)]) -> Vec<String> {
    let paths: Vec<String> = flags
        .iter()
        .filter(|(n, _)| *n == "c")
        .map(|(_, v)| v.clone())
        .collect();
    if paths.is_empty() {
        vec!["akit.json".to_owned()]
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

/// Refuses a config that sets a member of fit's roles akit does not run ([`crate::ROLES`]):
/// `akit validate` checks every role (I98), and the iterations after I100 run them; until a role
/// runs, applying it would be a config that says more than akit does.
fn roles_not_running(config: &Config) -> Result<(), Failure> {
    let unknown = akit_roles::host::unknown(config, crate::ROLES);
    let mut errs: Vec<Error> = unknown
        .into_iter()
        .map(|path| Error {
            file: None,
            path: path.to_owned(),
            line: 0,
            col: 0,
            msg: format!(
                "akit run does not run this role yet ({}); akit validate checks it",
                match path {
                    p if p.starts_with("tunnel.") => "I101-I103",
                    "webrtc.media" => "I113",
                    "webrtc.proxy" => "I114",
                    _ => "I104-I109",
                }
            ),
        })
        .collect();
    errs.extend(webrtc_handlers_not_served(config));
    // Cluster management (I130): read and checked by akit validate; the manager runs since
    // I132, a member links out from I139b (decision row 74).
    let admin = config.admin.as_ref();
    for (set, path, when) in [(
        admin.is_some_and(|a| a.cluster_member.is_some()),
        "admin.cluster_member",
        "I139b",
    )] {
        if set {
            errs.push(Error {
                file: None,
                path: path.to_owned(),
                line: 0,
                col: 0,
                msg: format!("akit run does not run this yet ({when}); akit validate checks it"),
            });
        }
    }
    if errs.is_empty() {
        Ok(())
    } else {
        Err(Failure::Config(Errors(errs)))
    }
}

/// Refuses the routes whose handler is `webrtc_proxy` until its role runs (I114): `akit validate` checks them, and they answer 503 as fit's do without their role,
/// but a config that sets them would say more than akit does.
fn webrtc_handlers_not_served(config: &Config) -> Vec<Error> {
    config
        .routes
        .iter()
        .enumerate()
        .filter_map(|(i, r)| {
            let iteration = match &r.handler {
                akit_config::Handler::WebRtcProxy(_) => "I114",
                _ => return None,
            };
            Some(Error {
                file: None,
                path: format!("routes[{i}].handler"),
                line: 0,
                col: 0,
                msg: format!(
                    "akit run does not serve the {} handler yet ({iteration}); akit validate \
                     checks it",
                    r.handler.module()
                ),
            })
        })
        .collect()
}

/// The store, as the cluster manager keeps its members in it (I132): its section `cluster`,
/// written on the manager's pool.
struct ClusterStore(boot::Shared);

impl akit_cluster::Store for ClusterStore {
    fn section(&self, name: &str) -> Option<Vec<u8>> {
        boot::lock(&self.0).section(name).map(<[u8]>::to_vec)
    }

    fn set_section(&self, name: &str, data: &[u8]) -> Result<(), String> {
        boot::lock(&self.0)
            .set_section(name, data)
            .map_err(|e| e.to_string())
    }
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
    /// The roles (I100), moved to every config applied after the listeners.
    roles: &'a mut akit_roles::host::Manager,
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
        let mut warnings = self.warnings(config, json, zone, true);
        // The roles after the listeners, as fit's: a role's failure is a warning (I100).
        warnings.extend(self.roles.apply(config));
        // The files the roles read (their listeners' certificates, I102d), watched with the
        // listeners' own.
        self.cert_files.extend(self.roles.files());
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
        roles_not_running(config).map_err(failure)?;
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
        // A role akit does not run is refused here, not by `validate` (I98, I100).
        roles_not_running(config).map_err(failure)?;
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

/// SIGHUP with a store (I93b): reads the store again, as `akit store import` or another akit may
/// have written it, and applies what it holds as a new version from the store; on any failure the
/// running config stays. Says what happened as fit says it.
fn reload_store(b: &boot::Boot, configs: &Configs, runner: &mut Runner<'_>) {
    println!("akit: reloading the store {}", b.path);
    let read = boot::lock(&b.store)
        .read()
        .map_err(|e| e.to_string())
        .and_then(|d| {
            String::from_utf8(d).map_err(|_| "the store's config is not UTF-8".to_owned())
        });
    let data = match read.map(|d| akit_config::value::standardize(&d)) {
        Ok(Ok(d)) => d,
        Ok(Err(errs)) => {
            runner.stats.reloaded(false);
            eprintln!("akit: reload failed; the running config stays");
            print_errors(&b.path, &errs);
            return;
        }
        Err(e) => {
            runner.stats.reloaded(false);
            eprintln!("akit: reload failed; the running config stays");
            eprintln!("{}: {e}", b.path);
            return;
        }
    };
    match configs.update(akit_admin::SOURCE_STORE, None, data, |text| {
        runner.apply_text(text, Mode::Apply)
    }) {
        Ok(o) => {
            print_applied(&o);
            println!(
                "akit: reloaded the store {} (config version {})",
                b.path, o.version
            );
        }
        Err(Refused::Apply(ApplyError::Config(errs))) => {
            eprintln!("akit: reload failed; the running config stays");
            print_errors(&b.path, &errs);
        }
        Err(Refused::Apply(ApplyError::Failed(e))) => {
            eprintln!("akit: reload failed; the running config stays: {e}");
        }
        Err(Refused::Changed | Refused::Request { .. }) => {}
    }
}

/// The config `std`, standard JSON, with `admin` and `admin.console` set, as `-admin` asks, and
/// the default console user, whose password is temporary, when the config names none (I93c).
fn with_default_admin(std: &str) -> Result<(String, Option<String>), Errors> {
    let hash = akio_crypto::bcrypt::hash(
        akit_admin::bootstrap::DEFAULT_CONSOLE_PASSWORD.as_bytes(),
        akio_crypto::bcrypt::DEFAULT_COST,
    )
    .map_err(|e| {
        Errors(vec![Error {
            file: None,
            path: String::new(),
            line: 0,
            col: 0,
            msg: format!("hashing the default password: {e}"),
        }])
    })?;
    akit_admin::bootstrap::with_default_console(std, &hash)
}

/// `t` as RFC 3339 in UTC, as Go's `time.RFC3339`.
fn rfc3339(t: std::time::SystemTime) -> String {
    let secs = t
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| i64::try_from(d.as_secs()).unwrap_or(i64::MAX));
    akit_roles::linktoken::rfc3339(secs)
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
/// Applies a role's change of its own member (I100, fit's `Host.Edit`): the running config with
/// the member changed, applied as any config from the role's pages, the member's path its source;
/// the role is told the warnings, or why it was not applied.
fn role_edit(edit: akit_roles::host::Edit, configs: &Configs, runner: &mut Runner<'_>) {
    let source = edit.id;
    let mut done = None;
    let r = configs.change(
        source,
        None,
        Mode::Apply,
        |cur| {
            let mut root = akit_config::value::parse(cur.data()).map_err(|_| Refused::Request {
                status: 500,
                msg: "the running config is not JSON".into(),
            })?;
            match edit.apply_to(&mut root) {
                Ok(d) => {
                    done = Some(d);
                    Ok(root.to_json())
                }
                Err((d, msg)) => {
                    done = Some(d);
                    Err(Refused::Request { status: 400, msg })
                }
            }
        },
        |text, mode| runner.apply_text(text, mode),
    );
    let told = match r {
        Ok(o) => {
            print_applied(&o);
            println!(
                "akit: {source} changed its config (config version {})",
                o.version
            );
            Ok(o.applied.warnings)
        }
        Err(e) => {
            let msg = match e {
                Refused::Changed => "the config changed meanwhile".to_owned(),
                Refused::Request { msg, .. } => msg,
                Refused::Apply(ApplyError::Config(errs)) => errs.to_string(),
                Refused::Apply(ApplyError::Failed(msg)) => msg,
            };
            eprintln!("akit: {source}'s change of its config was not applied: {msg}");
            Err(msg)
        }
    };
    if let Some(d) = done {
        d(told);
    }
}

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
    /// A role changes its own member (I100).
    RoleEdit(akit_roles::host::Edit),
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
    let mut admin_flag = false;
    let mut timeout: Option<Duration> = None;
    for (name, value) in &flags {
        match *name {
            "c" => {}
            "admin" => admin_flag = value == "true",
            "timeout" => match flags::parse_duration(value) {
                Some(d) => timeout = Some(d),
                None => {
                    return usage_error(&Usage(format!(
                        "invalid value {value:?} for flag -timeout: parse error\n{}",
                        flags::usage("run", RUN_FLAGS)
                    )));
                }
            },
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
    if timeout.is_some() && !admin_flag {
        eprintln!("akit run: -timeout is the default password's time, which needs -admin");
        return ExitCode::from(2);
    }
    if timeout.is_some_and(|t| t.is_zero()) {
        eprintln!("akit run: -timeout must be more than 0");
        return ExitCode::from(2);
    }
    let timeout = timeout.unwrap_or(Duration::from_secs(30 * 60));
    let started = std::time::SystemTime::now();
    let given = flags.iter().any(|(n, _)| *n == "c");
    let paths = config_paths(&flags);
    // The pools are made with the graph, before the engine they dial through, which is built
    // from the graph's listeners: they reach it through this slot, filled once it is built.
    let slot = EngineSlot::new();
    let mut name = paths.join(", ");
    // The config files, a .store among them aside, and the store that goes with them (I93b,
    // I93c): the one admin.store names, a .store given with -c, or the one beside the files, when
    // it exists or -admin is given. With a store, the config runs from what it holds merged onto
    // the files'.
    let (boot_files, spec) = match boot::load_boot(&paths, given, admin_flag) {
        Ok(b) => b,
        Err(e) => {
            eprintln!("akit run: {e}");
            return ExitCode::FAILURE;
        }
    };
    let Ok((std, files)) = store_cli::files_json(&boot_files) else {
        return ExitCode::FAILURE;
    };
    let mut temp_user = None;
    let std = if admin_flag {
        match with_default_admin(&std) {
            Ok((s, t)) => {
                temp_user = t;
                s
            }
            Err(errs) => {
                print_errors(&name, &errs);
                return ExitCode::FAILURE;
            }
        }
    } else {
        std
    };
    let boot = match boot::open_store(&std, &spec) {
        Ok(b) => b,
        Err(e) => {
            eprintln!("{name}: {e}");
            return ExitCode::FAILURE;
        }
    };
    let loaded = match &boot {
        None => load(&files).map(|(c, j, z)| (c, j.clone(), j, z)),
        Some(b) => {
            let full = b.full();
            akit_config::from_json(&full, None)
                .map_err(Failure::Config)
                .and_then(|c| {
                    let zone = time_zone(&c)?;
                    Ok((c, full, b.base.clone(), zone))
                })
        }
    };
    let (config, full_json, json, zone) = match (loaded, &boot) {
        (Ok(l), _) => l,
        // A store that did not exist holds the files' config: its errors are the files'.
        (Err(f), Some(b)) if b.seeded => {
            match load(&files) {
                Err(ff) if !files.is_empty() => ff.print(&name),
                _ => f.print(&name),
            }
            return ExitCode::FAILURE;
        }
        (Err(f), Some(b)) => {
            f.print(&b.path);
            return ExitCode::FAILURE;
        }
        (Err(f), None) => {
            f.print(&name);
            return ExitCode::FAILURE;
        }
    };
    if let Some(b) = &boot {
        if watch {
            eprintln!(
                "{name}: -watch watches config files, but the config is kept in the store {}; send SIGHUP to read the store again",
                b.path
            );
            return ExitCode::FAILURE;
        }
        if !b.seeded {
            name.clone_from(&b.path);
        }
    }
    print_warnings(&name, &config.warnings);
    // Secret files open to others, credentials in the clear (I80b).
    for w in akit_config::hardening::warnings(&config, &full_json) {
        eprintln!("akit: {w}");
    }
    // A store seeded from the files, or merged onto them, is written before akit serves, and put
    // back as it was when the start fails after it (I93b).
    let mut restore = None;
    let mut store_note = None;
    if let Some(b) = boot.as_ref().filter(|b| b.seeded || b.merged) {
        restore = Some(boot::Restore::of(&b.path));
        if let Err(e) = boot::lock(&b.store).write(b.base.clone().into_bytes()) {
            eprintln!("akit run: {e}");
            return ExitCode::FAILURE;
        }
        let files = boot_files.join(", ");
        store_note = Some(if b.seeded {
            format!(
                "akit: the config of {files} is kept in the store {} from now on",
                b.path
            )
        } else {
            format!(
                "akit: the store {} merged onto {files}, the store's values winning",
                b.path
            )
        });
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
    let graph = match roles_not_running(&config).and_then(|()| {
        compile_config(
            &config,
            &slot,
            log.as_ref().map(|l| &l.0),
            &metrics,
            Some(&acme),
            tracer.as_ref(),
            None,
        )
    }) {
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
    // The roles take the config once the listeners serve it (I100).
    let start_config = config;
    let mut cert_files = graph.cert_files().clone();
    let acme_names = graph.acme_names().to_vec();
    let configs = Configs::new(
        json,
        if boot.is_some() {
            akit_admin::SOURCE_STORE
        } else {
            akit_admin::SOURCE_FILE
        },
        admin.as_ref().map_or(1, |a| a.history),
    );
    // With a store, every config applied is kept in it, admin coming from the bootstrap file;
    // the console's passwords changed since that file was written are kept beside the config;
    // the store's key seals and opens the Admin API's store exports and imports (I93b).
    if let Some(b) = &boot {
        let st = Arc::clone(&b.store);
        configs.use_store(
            b.admin.clone(),
            Box::new(move |data: &str| {
                boot::lock(&st)
                    .write(data.as_bytes().to_vec())
                    .map_err(|e| e.to_string())
            }),
        );
        let st = Arc::clone(&b.store);
        configs.use_console_users(
            b.users.clone(),
            Box::new(move |users: &[akit_admin::store::User]| {
                boot::lock(&st)
                    .set_users(users.to_vec())
                    .map_err(|e| e.to_string())
            }),
        );
        if let Some(s) = boot::lock(&b.store).sealer() {
            configs.use_sealer(s);
        }
        // The default user's password is temporary until it is changed, which keeps the changed
        // hash in the store (I93c).
        if let Some(user) = temp_user
            .as_ref()
            .filter(|u| !b.users.iter().any(|s| &s.username == *u))
        {
            let until = started.checked_add(timeout).unwrap_or(started);
            configs.use_temporary_password(user, until);
            eprintln!(
                "akit: the console user {user:?} has the password {:?} until it is changed, which must be by {}; until then the console asks for that change and the Admin API refuses every request",
                akit_admin::bootstrap::DEFAULT_CONSOLE_PASSWORD,
                rfc3339(until)
            );
        }
    }
    // Who this akit is, for a manager of many (I131, fit's NodeID): admin.node_id, or the ID
    // kept in the store, or in the file beside the config, made the first time.
    let (node_id, id_kept) = match admin.as_ref().and_then(|a| a.node_id.clone()) {
        Some(id) => (id, akit_admin::node::ID_IN_CONFIG),
        None => {
            let prefix = paths
                .first()
                .and_then(|p| akit_roles::host::beside_prefix(p));
            let (id, kept, err) = match &boot {
                Some(b) => {
                    akit_admin::node::node_id(Some(&mut boot::lock(&b.store)), prefix.as_deref())
                }
                None => akit_admin::node::node_id(None, prefix.as_deref()),
            };
            if let Some(e) = err {
                eprintln!(
                    "akit run: the node ID is kept nowhere and made again at the next start: {e}"
                );
            }
            (id, kept)
        }
    };
    configs.use_node(akit_admin::node::Node {
        id: node_id,
        id_kept,
        hostname: String::new(),
        version: BUILD
            .get()
            .map_or(crate::VERSION, |(v, _)| v.as_str())
            .to_owned(),
        engine: format!(
            "akio {}",
            akit_runtime::AKIO_REV
                .get(..12)
                .unwrap_or(akit_runtime::AKIO_REV)
        ),
        started,
    });
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
    // The roles (I100), after the listeners, on their engine: their edits of their own members
    // reach this thread through the control queue, as the Admin API's loads do.
    let editor: akit_roles::host::Editor = {
        let tx = tx.clone();
        Arc::new(move |edit| match tx.try_send(Control::RoleEdit(edit)) {
            Ok(()) => Ok(()),
            Err(
                mpsc::TrySendError::Full(Control::RoleEdit(edit))
                | mpsc::TrySendError::Disconnected(Control::RoleEdit(edit)),
            ) => Err(edit),
            // Only an edit is sent here.
            Err(_) => Ok(()),
        })
    };
    let mut roles = akit_roles::host::Manager::new(crate::ROLES, Arc::clone(&engine), Some(editor));
    roles.set_drain(grace);
    // The roles' own consoles (I109), moved with their roles.
    roles.set_consoles(Box::new(akit_admin::console::RoleConsoles::new(
        &akit_admin::Options::default(),
    )));
    // Without a store, what the roles make and must keep goes beside the config; with one, it
    // goes into the config, which the store keeps.
    if let Some(first) = paths.first().filter(|_| boot.is_none()) {
        roles.keep_beside(first);
    }
    for w in roles.apply(&start_config) {
        eprintln!("akit run: {w}");
    }
    // The files the roles read (their listeners' certificates, I102d), watched with the
    // listeners' own.
    cert_files.extend(roles.files());
    for id in roles.running() {
        println!("akit: running {id}");
    }
    drop(start_config);
    acme.manage(&acme_names);
    let mut cluster: Option<akit_cluster::Manager> = None;
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
            // The running roles' metrics after the rest (I111: the TURN server's).
            let meters = roles.meters();
            api_opts.metrics = Some(Arc::new(move |out: &mut Vec<u8>| {
                scrape.write(out);
                akit_roles::host::write_meters(&meters, out);
            }));
            // The log files, at GET /logs and POST /logs/rotate and /logs/cleanup (I83).
            api_opts.logs = Some(log_files.clone());
            // The running roles' link tokens, at GET /tokens (I100).
            let registries = roles.registries();
            api_opts.tokens = Some(Arc::new(move |id: &str| {
                akit_roles::host::token_views(&registries, id)
            }));
            // The running roles' modules, which the console serves beside the config's (I109).
            api_opts.modules = Some(roles.modules());
            // What GET /node reads of the running akit (I131): the roles running, and whether
            // the running config counts requests.
            let (modules, graph) = (roles.modules(), cell.clone());
            api_opts.node = Some(Arc::new(move || akit_admin::node::NodeLive {
                roles: modules.ids().iter().map(|s| (*s).to_owned()).collect(),
                counts_requests: graph.with(|g| g.metrics().is_some()),
            }));
            // The cluster manager (I132), its members kept in the store, served by the console.
            if let Some(c) = &a.cluster {
                let Some(b) = &boot else {
                    eprintln!(
                        "akit run: admin.cluster keeps its members in the store: set admin.store, \
                         or run with -admin"
                    );
                    akit_listener::shutdown(&engine, grace);
                    return ExitCode::FAILURE;
                };
                let o = akit_cluster::Options {
                    engine: Arc::clone(&engine),
                    settings: c.clone(),
                    store: Arc::new(ClusterStore(Arc::clone(&b.store))),
                    log: Arc::new(|msg: String| {
                        akit_observe::logger::log(
                            akit_observe::logger::Logger::Admin,
                            akit_observe::logger::Level::Info,
                            format_args!("{msg}"),
                        );
                    }),
                    system_roots: None,
                    schema: akit_admin::node::schema(),
                };
                match akit_cluster::Manager::new(o) {
                    Ok(m) => {
                        api_opts.cluster = Some(m.module());
                        api_opts.cluster_max_body =
                            usize::try_from(c.response_bytes()).unwrap_or(usize::MAX);
                        cluster = Some(m);
                    }
                    Err(e) => {
                        eprintln!("akit run: admin.cluster: {e}");
                        akit_listener::shutdown(&engine, grace);
                        return ExitCode::FAILURE;
                    }
                }
            }
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
    // Started: the store written from the files stays, and says so.
    if let Some(r) = &mut restore {
        r.disarm();
    }
    if let Some(note) = &store_note {
        println!("{note}");
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
        roles: &mut roles,
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
                            // The Admin API's and the console's certificate files (I94): what
                            // happened is in the admin logger.
                            if let Some(api) = runner.api {
                                let _ = api.watch_certs(now);
                            }
                            // The roles' consoles' (I109b): in the admin logger too.
                            runner.roles.watch_console_certs(now);
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
                match &boot {
                    Some(b) => reload_store(b, &configs, &mut runner),
                    None => reload(&paths, &configs, &mut runner, provider.as_mut()),
                }
                // The Admin API's and the console's certificate files read again (I94); what
                // happened is in the admin logger.
                if let Some(api) = runner.api {
                    let _ = api.reload_certs();
                }
                runner.roles.reload_console_certs();
                cell.collect();
            }
            // Taken at the top of the loop.
            Some(Control::Provide) => {}
            Some(Control::RoleEdit(edit)) => {
                role_edit(edit, &configs, &mut runner);
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
    // The API first: no more loads. Those still waiting are refused. No provider's part is
    // applied either.
    if let Some(api) = &api {
        api.shutdown();
    }
    // The cluster manager's members logged out, before the engine its calls go out on (I132).
    if let Some(m) = &cluster {
        m.close();
    }
    updates.close();
    for p in &mut providers {
        p.stop();
    }
    // No more certificates obtained: its work under way is given up.
    acme.close();
    // The roles before the engine they serve on (I100).
    runner.roles.stop(grace);
    // A second SIGINT or SIGTERM does not wait for the shutdown, as fit's.
    std::thread::spawn(move || {
        while let Ok(c) = control.recv() {
            match c {
                Control::Stop => {
                    eprintln!("akit run: stopped by a second signal");
                    std::process::exit(1);
                }
                Control::Admin(job) => job.refuse(503, "akit is shutting down"),
                Control::RoleEdit(edit) => (edit.done)(Err("akit is shutting down".to_owned())),
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
        "token" => token::token(rest),
        "store" => store_cli::store(rest),
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

    /// A role akit validate checks is refused by akit run until its iteration runs it (I98):
    /// every role of fit's runs now (the tunnel's since I101, the LLM feature's since I105-I107).
    #[test]
    fn roles_are_refused_until_they_run() {
        let c = akit_config::from_json(r#"{"tunnel": {"hub": {}}, "llm": {"proxy": {}}}"#, None)
            .unwrap();
        assert!(super::roles_not_running(&c).is_ok());
        let c = akit_config::from_json(
            r#"{"llm": {"hub": {}, "node": {}, "proxy": {}}, "tunnel": {"node": {}}}"#,
            None,
        )
        .unwrap();
        assert!(super::roles_not_running(&c).is_ok());
        let c = akit_config::from_json(r#"{"tunnel": {"hub": {}}}"#, None).unwrap();
        assert!(super::roles_not_running(&c).is_ok());
        let c = akit_config::from_json(r#"{"tunnel": {}, "llm": {}}"#, None).unwrap();
        assert!(super::roles_not_running(&c).is_ok());
        // The BitTorrent roles (I150): read and checked, run since I151 and I152.
        let c = akit_config::from_json(
            r#"{"bittorrent": {"client": {"listen": "127.0.0.1:0", "data_dir": "d"}, "tracker": {}}}"#,
            None,
        )
        .unwrap();
        assert!(super::roles_not_running(&c).is_ok());
        let c = akit_config::from_json(r#"{"bittorrent": {}}"#, None).unwrap();
        assert!(super::roles_not_running(&c).is_ok());
    }

    /// The WebRTC roles and handlers are refused by akit run until their iterations run them
    /// (I110): the TURN server runs (I111), the media server and whip and whep (I113).
    #[test]
    fn webrtc_is_refused_until_it_runs() {
        let c = akit_config::from_json(
            r#"{"listeners": [{"name": "web", "address": "127.0.0.1:0"}],
                "upstreams": [{"name": "m", "addresses": ["127.0.0.1:9"]}],
                "routes": [{"handler": {"module": "respond"}}, {"handler": {"module": "whip"}}, {"handler": {"module": "webrtc_proxy", "params": {"upstream": "m"}}}],
                "webrtc": {"turn": {"listen": ["127.0.0.1:0"], "realm": "r", "secret": "s", "relay_ip": "127.0.0.1"}, "media": {"listen": "127.0.0.1:0"}}}"#,
            None,
        )
        .unwrap();
        let Err(super::Failure::Config(errs)) = super::roles_not_running(&c) else {
            panic!("webrtc let through");
        };
        let got: Vec<(&str, bool)> = errs
            .0
            .iter()
            .map(|e| (e.path.as_str(), e.msg.contains("I11")))
            .collect();
        assert_eq!(got, [("routes[2].handler", true)]);
    }

    /// Cluster management is checked by akit validate; akit run runs the manager (I132) and
    /// refuses a member linking out until I139b (decision row 74); the node ID is taken.
    #[test]
    fn cluster_member_is_refused_until_it_runs() {
        let doc = r#"{"admin": {"node_id": "edge-1",
            "console": {"users": [{"username": "a", "password_hash": "$2a$10$x"}]},
            "cluster": {},
            "cluster_member": {"manager": "wss://m:7444/cluster/link", "fingerprint": "0a0a0a0a0a0a0a0a0a0a0a0a0a0a0a0a0a0a0a0a0a0a0a0a0a0a0a0a0a0a0a0a", "token_file": "/t"}}}"#;
        let c = akit_config::from_json(doc, None).unwrap();
        let Err(super::Failure::Config(errs)) = super::roles_not_running(&c) else {
            panic!("not refused");
        };
        let paths: Vec<&str> = errs.as_slice().iter().map(|e| e.path.as_str()).collect();
        assert_eq!(paths, ["admin.cluster_member"]);
        let c = akit_config::from_json(
            r#"{"admin": {"node_id": "edge-1", "console": {"users": [{"username": "a", "password_hash": "$2a$10$x"}]}, "cluster": {}}}"#,
            None,
        )
        .unwrap();
        assert!(super::roles_not_running(&c).is_ok());
    }

    /// Every role's pages are served by akit run: the hub's (I109, its console_socket with
    /// them), the LLM roles' (I109c), the tunnel's node's and proxy's (I109b); a proxy's
    /// web_proxies run (I103b).
    #[test]
    fn role_pages_are_served() {
        let console = r#"{"address": "127.0.0.1:7100", "users": [{"username": "a", "password_hash": "$2a$10$N9qo8uLOickgx2ZMRZoMyeIjZAgcfl7p92ldGxad68LJZdL17lhWy"}]}"#;
        let doc = format!(
            r#"{{"tunnel": {{"hub": {{"password": "pw", "console": {console}, "console_socket": {{"address": "127.0.0.1:7101"}}}},
                "node": {{"console": {}}},
                "proxy": {{"console": {}, "web_proxies": [{{"listen": "127.0.0.1:7103", "url": "ws://127.0.0.1:7101/", "password": "pw"}}]}}}}}}"#,
            console.replace("7100", "7104"),
            console.replace("7100", "7102")
        );
        let c = akit_config::from_json(&doc, None).unwrap();
        assert!(super::roles_not_running(&c).is_ok());
        let llm = format!(r#"{{"llm": {{"node": {{"console": {console}}}}}}}"#);
        let c = akit_config::from_json(&llm, None).unwrap_or_else(|e| panic!("{llm}\n{e}"));
        assert!(super::roles_not_running(&c).is_ok());
    }
}
