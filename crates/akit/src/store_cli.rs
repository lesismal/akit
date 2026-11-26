//! `akit store export` and `akit store import` (I93), as fit's `fit store`: the config the store
//! holds, printed as JSON, and a config sealed as the store, checked first. The store is the one
//! `admin.store` names, a `.store` file `-c` gives, or the one beside the config file, as `akit
//! run` finds it (`boot`, I93c). The store is fit's `fitstore` file
//! ([`akit_admin::store`]): fit's and akit's open each other's.

use std::process::ExitCode;

use akit_admin::bootstrap::{split_admin, with_admin};
use akit_admin::store::{self, Store};
use akit_config::value::parse;
use akit_runtime::upstream::EngineSlot;

use super::boot;
use crate::flags::{self, Spec};

const USAGE: &str = "\
Usage:
	akit store export -c <config file or .store>
	        print the config the store holds, as JSON
	akit store import -c <config file or .store> <config file or directory>...
	        check the config the files hold and seal it as the store,
	        replacing it, with a new salt

The config file is the one akit run is started with: its admin.store says
where the store is and its password. Without admin.store, the store is the
.store file -c names, or the one beside the config file (akit.json gives
akit.store), and its password is $AKIT_STORE_PASSWORD or the key file beside
it, the store's path with .key added. To change the password, export the
store, change the password, and import what was exported.
";

const FLAGS: &[Spec] = &[Spec {
    name: "c",
    bool: false,
    usage: "the config file akit run is started with, whose admin.store names the store, or the .store file itself; give -c again for more, which are merged",
}];

/// The files' config as JSON, not checked: `{}` with none. Prints its errors.
pub(crate) fn files_json(files: &[String]) -> Result<(String, Vec<akit_config::ConfigFile>), ()> {
    if files.is_empty() {
        return Ok(("{}".to_owned(), Vec::new()));
    }
    let shown = files.join(", ");
    let read =
        akit_config::read_config_files(files).map_err(|e| super::print_errors(&shown, &e))?;
    let json = akit_config::json_of_files(&read).map_err(|e| super::print_errors(&shown, &e))?;
    Ok((json, read))
}

/// `akit store export|import`: 0, 1 when it fails, 2 for a usage error.
pub(crate) fn store(args: &[String]) -> ExitCode {
    let Some((cmd, rest)) = args.split_first() else {
        eprint!("{USAGE}");
        return ExitCode::from(2);
    };
    if cmd != "export" && cmd != "import" {
        eprint!("{USAGE}");
        return ExitCode::from(2);
    }
    let name = format!("akit store {cmd}");
    // The flags come first, the files to import after them, as Go's flag package takes them.
    let (flag_args, files) = rest.split_at(flags_end(rest));
    let parsed = match flags::parse(&format!("store {cmd}"), flag_args, FLAGS) {
        Ok(f) => f,
        Err(u) => {
            eprintln!("{}", u.0);
            return ExitCode::from(2);
        }
    };
    if (cmd == "export" && !files.is_empty()) || (cmd == "import" && files.is_empty()) {
        eprint!("{USAGE}");
        return ExitCode::from(2);
    }
    let given = parsed.iter().any(|(n, _)| *n == "c");
    let paths = super::config_paths(&parsed);
    let shown = paths.join(", ");
    let (boot_files, mut spec) = match boot::load_boot(&paths, given, false) {
        Ok(b) => b,
        Err(e) => {
            eprintln!("{name}: {e}");
            return ExitCode::FAILURE;
        }
    };
    let Ok((std, _)) = files_json(&boot_files) else {
        return ExitCode::FAILURE;
    };
    // An import makes the store when there is none yet.
    spec.create = cmd == "import";
    let admin = split_admin(&std).ok().and_then(|(a, _)| a);
    let r = if cmd == "export" {
        export(&std, &spec, &shown)
    } else {
        import(admin.as_deref(), &spec, files)
    };
    match r {
        Ok(()) => ExitCode::SUCCESS,
        Err(Some(e)) => {
            eprintln!("{name}: {e}");
            ExitCode::FAILURE
        }
        Err(None) => ExitCode::FAILURE,
    }
}

/// Where the flags of `args` end: at the first argument that is not a flag (nor `-c`'s value),
/// or past `--`.
fn flags_end(args: &[String]) -> usize {
    let mut i = 0;
    while let Some(a) = args.get(i) {
        if a == "--" {
            return i;
        }
        if !a.starts_with('-') {
            return i;
        }
        let takes_value = (a == "-c" || a == "--c") && args.get(i.saturating_add(1)).is_some();
        i = i.saturating_add(if takes_value { 2 } else { 1 });
    }
    i
}

/// Prints the config the store of `std`, the bootstrap config, holds.
fn export(std: &str, spec: &boot::Spec, shown: &str) -> Result<(), Option<String>> {
    let b = boot::open_store(std, spec).map_err(Some)?.ok_or_else(|| {
        Some(format!(
            "{shown} names no store: no admin.store, and no {}",
            spec.path
        ))
    })?;
    let Some(data) = &b.stored else {
        return Err(Some(format!("there is no store at {} yet", b.path)));
    };
    use std::io::Write;
    let mut out = std::io::stdout().lock();
    out.write_all(data.as_bytes())
        .and_then(|()| out.flush())
        .map_err(|e| Some(e.to_string()))
}

/// Seals the config `files` hold as the store `admin` (the bootstrap file's, as JSON) and `spec`
/// describe, after checking it with that `admin`: with a new salt, keeping the console users the
/// old store kept when it opens with the password.
fn import(admin: Option<&str>, spec: &boot::Spec, files: &[String]) -> Result<(), Option<String>> {
    let Some((path, password)) = boot::resolve_store(admin, spec).map_err(Some)? else {
        return Err(Some("no store".to_owned()));
    };
    // An import replaces the config, not the console's passwords: the users the old store keeps
    // stay, when it opens; one that does not, as after the password was changed, is no reason to
    // stop.
    let users = match Store::open(&path, &password) {
        Ok((s, _)) => s.users().to_vec(),
        Err(store::Error::Open(_)) => Vec::new(),
        Err(e) => return Err(Some(e.to_string())),
    };
    let shown = files.join(", ");
    let fail = |errs: &akit_config::Errors| {
        super::print_errors(&shown, errs);
        None
    };
    let (_, data) = akit_config::read_config_files(files)
        .and_then(|f| akit_config::load_config_files(&f))
        .map_err(|e| fail(&e))?;
    let imported = parse(&data).map_err(|e| fail(&e))?;
    if imported.get("admin").is_some() {
        return Err(Some(format!(
            "{shown}: admin: admin stays in the bootstrap file: leave it out of what is imported"
        )));
    }
    // Checked as akit run would take it: with the bootstrap file's admin.
    let full = match admin {
        Some(a) => with_admin(&data, a),
        None => data.clone(),
    };
    let checked = akit_config::from_json(&full, None).and_then(|c| {
        let modules = super::BUILD
            .get()
            .map_or(crate::STANDARD, |(_, m)| m.as_slice());
        let registry: akit_runtime::Registry<akit_runtime::HandlerParams> =
            akit_runtime::Registry::new(modules).map_err(|e| {
                akit_config::Errors(vec![akit_config::Error {
                    file: None,
                    path: String::new(),
                    line: 0,
                    col: 0,
                    msg: e.to_string(),
                }])
            })?;
        akit_runtime::compile(&c, &registry, &akit_runtime::Env::new(EngineSlot::new()))
    });
    if let Err(e) = checked {
        return Err(fail(&e));
    }
    let mut st = Store::create(&path, &password).map_err(|e| Some(e.to_string()))?;
    st.write(data.into_bytes())
        .map_err(|e| Some(e.to_string()))?;
    if !users.is_empty() {
        st.set_users(users).map_err(|e| Some(e.to_string()))?;
    }
    println!("{shown}: sealed into {path}");
    Ok(())
}
