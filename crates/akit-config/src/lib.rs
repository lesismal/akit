#![forbid(unsafe_code)]
#![warn(clippy::unwrap_used)]
//! The akit config model: fit's config (decision 002), read strictly, with every problem
//! reported with its file, line, column and path, as fit reports it.
//!
//! I02 gives the model's first part ([`Config`]: `listeners`, `routes` and the `respond`
//! handler) and the JSON reader: JSON with `//` and `/* */` comments and trailing commas, as fit
//! reads it; an unknown field, a key given twice or a wrong type is an error. A field fit reads
//! and akit does not support yet is refused as "not supported yet", never read and ignored
//! ([`Error::is_not_supported_yet`]). I09 adds listeners' `tls` ([`Tls`]), and I14 `upstreams`
//! ([`Upstream`]): their servers and the limits on the connections to them; I19 their
//! `weights` and `load_balancing` ([`LoadBalancing`]). I16 reads `limits.max_body_bytes`
//! ([`Limits`]), which every listener carries.
//!
//! I23 adds fit's own format, `.conf` ([`from_conf`]), and I24 YAML ([`from_yaml`], decision
//! 005): each read into the same tree as JSON and decoded by the same code, so the model reads
//! all three alike and problems point into the file as written.
//!
//! Reading a file blocks: this is control plane code, never called on an I/O thread
//! (rules.md rule 1).

mod conf;
mod decode;
pub mod duration;
mod error;
mod json;
mod model;
pub mod value;
mod yaml;

use std::path::{Path, PathBuf};

pub use error::{Error, Errors, NOT_SUPPORTED_YET};
pub use json::MAX_DEPTH;
pub use model::{
    AccessLevel, AccessLog, Acme, Admin, BasicAuth, BasicUser, BodyLimit, Certificate, Challenge,
    ClientAuth, ClientAuthMode, ClientCert, ConcurrencyLimit, Config, DEFAULT_ACME_DIRECTORY,
    DEFAULT_ACME_MAX_CONCURRENT, DEFAULT_ACME_REQUEST_TIMEOUT, DEFAULT_ACME_TIMEOUT,
    DEFAULT_ADMIN_ADDRESS, DEFAULT_ADMIN_HISTORY, DEFAULT_ADMIN_MAX_CONFIG_BYTES,
    DEFAULT_ADMIN_MAX_CONNS, DEFAULT_BROWSE_MAX_ENTRIES, DEFAULT_BUFFER_BYTES,
    DEFAULT_CHECK_INTERVAL, DEFAULT_CONNECT_TIMEOUT, DEFAULT_FAIL_TIMEOUT, DEFAULT_FLUSH_INTERVAL,
    DEFAULT_HEALTH_INTERVAL, DEFAULT_HEALTH_THRESHOLD, DEFAULT_HEALTH_TIMEOUT,
    DEFAULT_IDLE_TIMEOUT, DEFAULT_MAX_BODY_BYTES, DEFAULT_MAX_CONNS, DEFAULT_MAX_FAILS,
    DEFAULT_MAX_FILE_BYTES, DEFAULT_MAX_IDLE_CONNS, DEFAULT_MAX_PENDING, DEFAULT_PROTOCOLS,
    DEFAULT_RATE_MAX_KEYS, DEFAULT_REALM, DEFAULT_RESPONSE_TIMEOUT, DEFAULT_STICKY_COOKIE_NAME,
    DEFAULT_TLS_PROTOCOLS, Dotfiles, Encoding, Handler, HeaderOps, Headers, HealthCheck,
    HealthCheckType, Limits, Listener, LoadBalancing, LogFormat, MAX_WEIGHT, Match, Metrics,
    Network, Protocol, Proxy, RateKey, RateLimit, Redirect, Respond, Rewrite, Route,
    SCHEMA_VERSION, Static, StickyCookie, Tls, TlsVersion, Upstream, UpstreamProtocol,
};

/// The largest config document read, in bytes: fit's own configs are kilobytes; this bounds
/// what a mistaken path (a log file, a device) can make akit read.
pub const MAX_CONFIG_BYTES: usize = 64 << 20;

/// The most files a config is read from, as fit's.
pub const MAX_CONFIG_FILES: usize = 10_000;

/// One config file: the name its problems give, its format and what it holds.
#[derive(Debug, Clone, Copy)]
pub struct Source<'a> {
    /// The file's name, named in every problem found in it.
    pub name: Option<&'a str>,
    /// Its format.
    pub format: Format,
    /// Its text.
    pub src: &'a str,
}

/// The format a config file is written in.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
#[non_exhaustive]
pub enum Format {
    /// JSON, with comments and trailing commas as fit reads it.
    #[default]
    Json,
    /// YAML, as fit reads it (decision 005).
    Yaml,
    /// fit's own format, `.conf` (fit's `docs/reference/fit.md`).
    Conf,
}

impl Format {
    /// The format a file's extension says, as fit picks it: `.conf` is fit's own format,
    /// `.yaml` and `.yml` are YAML, any other file JSON.
    #[must_use]
    pub fn of(path: &Path) -> Format {
        match extension(path).as_deref() {
            Some("conf") => Format::Conf,
            Some("yaml" | "yml") => Format::Yaml,
            _ => Format::Json,
        }
    }
}

/// Reads a config written in JSON. `file`, when given, is named in every problem.
pub fn from_json(src: &str, file: Option<&str>) -> Result<Config, Errors> {
    from_sources(&[Source {
        name: file,
        format: Format::Json,
        src,
    }])
}

/// Reads a config written in YAML. `file`, when given, is named in every problem.
pub fn from_yaml(src: &str, file: Option<&str>) -> Result<Config, Errors> {
    from_sources(&[Source {
        name: file,
        format: Format::Yaml,
        src,
    }])
}

/// Reads a config written in fit's own format, `.conf` (fit's `docs/reference/fit.md`): each
/// directive a field, as its JSON twin has it, with `site`, the shorthands, `def` and `use`;
/// `{$NAME}` is the environment variable's value. `file`, when given, is named in every problem,
/// each at its line and column in the `.conf` text. `include` and `import`, which read other
/// files, are refused as not supported yet: name the files on the command line instead.
pub fn from_conf(src: &str, file: Option<&str>) -> Result<Config, Errors> {
    from_sources(&[Source {
        name: file,
        format: Format::Conf,
        src,
    }])
}

/// Reads a config from several files, each in its format, merged as fit merges a config's files:
/// the lists at the top (`listeners`, `routes`) are joined in the files' order, `schema_version`
/// may be given by each if they agree, and the checks across elements (unique names, routes
/// naming listeners that exist) run on the whole. Every problem names the file it is in, with its line, column and
/// its path in that file.
pub fn from_sources(srcs: &[Source<'_>]) -> Result<Config, Errors> {
    let mut ds: Vec<decode::Decoder<'_>> = srcs
        .iter()
        .map(|s| decode::Decoder::new(s.src, s.name))
        .collect();
    let total: usize = srcs.iter().map(|s| s.src.len()).sum();
    if total > MAX_CONFIG_BYTES {
        if let Some(d) = ds.first_mut() {
            let what = if srcs.len() > 1 {
                "the config's files hold"
            } else {
                "the config is"
            };
            d.error(0, "", format!("{what} more than {MAX_CONFIG_BYTES} bytes"));
        }
        return Err(collect(ds));
    }
    let mut parts = Vec::with_capacity(srcs.len());
    for (i, (s, d)) in srcs.iter().zip(ds.iter_mut()).enumerate() {
        let parsed = match s.format {
            Format::Json => json::parse(s.src),
            Format::Yaml => yaml::parse(s.src),
            Format::Conf => conf::parse(s.src, &|name| std::env::var(name).ok()),
        };
        match parsed {
            Ok(node) => match model::part(d, &node, i) {
                Some(p) => parts.push(p),
                None => {
                    if d.errors.is_empty() {
                        d.error(node.off, "", "the config could not be read");
                    }
                }
            },
            Err(e) => d.error(e.off, "", e.msg),
        }
    }
    if parts.len() < srcs.len() || srcs.is_empty() {
        if ds.iter().all(|d| d.errors.is_empty()) {
            return Err(Errors(vec![Error {
                file: None,
                path: String::new(),
                line: 0,
                col: 0,
                msg: "no config files".into(),
            }]));
        }
        return Err(collect(ds));
    }
    let config = model::merge(&mut ds, parts);
    let errs = collect(ds);
    if errs.0.is_empty() {
        Ok(config)
    } else {
        Err(errs)
    }
}

/// Every decoder's problems, file by file.
fn collect(ds: Vec<decode::Decoder<'_>>) -> Errors {
    Errors(ds.into_iter().flat_map(|d| d.errors).collect())
}

/// The config files `paths` name, as fit finds them: each file as it is, and each directory's
/// files ending in `.json`, `.conf`, `.yaml` or `.yml`, in its subdirectories too, in lexical
/// order (files and directories whose names start with a dot left out); a file named twice is
/// read once; at most [`MAX_CONFIG_FILES`].
///
/// # Errors
///
/// A path that cannot be read, a directory with no config files, or too many files.
pub fn config_files<P: AsRef<Path>>(paths: &[P]) -> Result<Vec<PathBuf>, Errors> {
    let mut files: Vec<PathBuf> = Vec::new();
    for p in paths {
        let p = p.as_ref();
        let meta = std::fs::metadata(p).map_err(|e| file_error(p, &e.to_string()))?;
        if meta.is_dir() {
            let before = files.len();
            walk(p, p, &mut files)?;
            if files.len() == before {
                return Err(file_error(
                    p,
                    "no config files in the directory (.conf, .yaml, .yml, .json)",
                ));
            }
        } else {
            add(p, &mut files)?;
        }
    }
    if files.is_empty() {
        return Err(Errors(vec![Error {
            file: None,
            path: String::new(),
            line: 0,
            col: 0,
            msg: "no config files".into(),
        }]));
    }
    Ok(files)
}

const DIR_EXTS: &[&str] = &["conf", "yaml", "yml", "json"];

fn walk(root: &Path, dir: &Path, files: &mut Vec<PathBuf>) -> Result<(), Errors> {
    let mut entries: Vec<_> = std::fs::read_dir(dir)
        .map_err(|e| file_error(dir, &e.to_string()))?
        .collect::<Result<_, _>>()
        .map_err(|e| file_error(dir, &e.to_string()))?;
    // Lexical order, as Go's filepath.WalkDir.
    entries.sort_by_key(std::fs::DirEntry::file_name);
    for e in entries {
        let path = e.path();
        if e.file_name().as_encoded_bytes().starts_with(b".") && path != root {
            continue;
        }
        let ty = e
            .file_type()
            .map_err(|err| file_error(&path, &err.to_string()))?;
        // A symbolic link is followed, as WalkDir does for the root only: elsewhere it is
        // read as a file when it ends in one of the extensions.
        if ty.is_dir() {
            walk(root, &path, files)?;
        } else if extension(&path).is_some_and(|x| DIR_EXTS.contains(&x.as_str())) {
            add(&path, files)?;
        }
    }
    Ok(())
}

fn add(p: &Path, files: &mut Vec<PathBuf>) -> Result<(), Errors> {
    // As Go's filepath.Clean, for telling a file named twice: no "." or empty components.
    let clean: PathBuf = p.components().collect();
    if files.contains(&clean) {
        return Ok(());
    }
    if files.len() == MAX_CONFIG_FILES {
        return Err(Errors(vec![Error {
            file: None,
            path: String::new(),
            line: 0,
            col: 0,
            msg: format!("a config is read from at most {MAX_CONFIG_FILES} files"),
        }]));
    }
    files.push(clean);
    Ok(())
}

fn extension(p: &Path) -> Option<String> {
    p.extension()
        .map(|x| x.to_string_lossy().to_ascii_lowercase())
}

fn file_error(p: &Path, msg: &str) -> Errors {
    Errors(vec![Error {
        file: Some(p.display().to_string()),
        path: String::new(),
        line: 0,
        col: 0,
        msg: msg.to_owned(),
    }])
}

/// Reads the config `paths` name ([`config_files`]), merged ([`from_sources`]); at most
/// [`MAX_CONFIG_BYTES`] in all. Each file is read in the format its extension says
/// ([`Format::of`]): fit's own format for `.conf`, YAML for `.yaml` and `.yml`, JSON otherwise.
///
/// # Errors
///
/// Every problem found, each naming its file.
pub fn from_files<P: AsRef<Path>>(paths: &[P]) -> Result<Config, Errors> {
    let files = read_config_files(paths)?;
    from_sources(&sources(&files)?)
}

/// [`from_files`], with the config as one JSON document too ([`value::json_of`]): what the
/// Admin API keeps of a config read from files and answers `GET /config` with.
///
/// # Errors
///
/// Every problem found, each naming its file.
pub fn load_files<P: AsRef<Path>>(paths: &[P]) -> Result<(Config, String), Errors> {
    load_config_files(&read_config_files(paths)?)
}

/// A config file as it was read: its path and its bytes. What the file provider (I29) compares
/// with what it reads at its next look.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConfigFile {
    /// Its path, as [`config_files`] gives it.
    pub path: String,
    /// What it held.
    pub data: Vec<u8>,
}

/// Reads the files `paths` name ([`config_files`]), at most [`MAX_CONFIG_BYTES`] in all.
///
/// # Errors
///
/// A path that cannot be read, or files past the limit, each naming its file.
pub fn read_config_files<P: AsRef<Path>>(paths: &[P]) -> Result<Vec<ConfigFile>, Errors> {
    let files = config_files(paths)?;
    let mut read = Vec::with_capacity(files.len());
    let mut total: usize = 0;
    for f in &files {
        let data = read_limited(f, MAX_CONFIG_BYTES.saturating_sub(total), files.len() > 1)?;
        total = total.saturating_add(data.len());
        read.push(ConfigFile {
            path: f.display().to_string(),
            data,
        });
    }
    Ok(read)
}

/// The config `files` hold, as [`load_files`] reads it from them: each file in the format its
/// extension says, merged, with the config as one JSON document too.
///
/// # Errors
///
/// Every problem found, each naming its file; a file that is not UTF-8.
pub fn load_config_files(files: &[ConfigFile]) -> Result<(Config, String), Errors> {
    let srcs = sources(files)?;
    let config = from_sources(&srcs)?;
    Ok((config, value::json_of(&srcs)?))
}

/// `files` as sources, each in the format its extension says.
fn sources(files: &[ConfigFile]) -> Result<Vec<Source<'_>>, Errors> {
    files
        .iter()
        .map(|f| {
            let path = Path::new(&f.path);
            let src = std::str::from_utf8(&f.data).map_err(|e| not_utf8(path, e.valid_up_to()))?;
            Ok(Source {
                name: Some(&f.path),
                format: Format::of(path),
                src,
            })
        })
        .collect()
}

fn not_utf8(path: &Path, at: usize) -> Errors {
    file_error(path, &format!("the config is not UTF-8 (at byte {at})"))
}

fn read_limited(path: &Path, limit: usize, several: bool) -> Result<Vec<u8>, Errors> {
    use std::io::Read;
    let too_big = || {
        let what = if several {
            "the config's files hold"
        } else {
            "the config is"
        };
        file_error(path, &format!("{what} more than {MAX_CONFIG_BYTES} bytes"))
    };
    let f = std::fs::File::open(path).map_err(|e| file_error(path, &e.to_string()))?;
    let mut bytes = Vec::new();
    let cap = u64::try_from(limit).unwrap_or(u64::MAX).saturating_add(1);
    f.take(cap)
        .read_to_end(&mut bytes)
        .map_err(|e| file_error(path, &e.to_string()))?;
    if bytes.len() > limit {
        return Err(too_big());
    }
    Ok(bytes)
}

/// Reads a JSON config from a file, at most [`MAX_CONFIG_BYTES`] long.
pub fn from_json_file(path: &Path) -> Result<Config, Errors> {
    let data = read_limited(path, MAX_CONFIG_BYTES, false)?;
    let src = String::from_utf8(data).map_err(|e| not_utf8(path, e.utf8_error().valid_up_to()))?;
    from_json(&src, Some(&path.display().to_string()))
}
