//! `akit store export` and `akit store import` (I93), as fit's `fit store`: the config the store
//! `admin.store` names holds, printed as JSON, and a config sealed as the store, checked first.
//! The store is fit's `fitstore` file ([`akit_admin::store`]): fit's and akit's open each other's.

use std::process::ExitCode;

use akit_admin::store::{self, Store};
use akit_config::value::{Value, parse};
use akit_runtime::upstream::EngineSlot;

use crate::flags::{self, Spec};

const USAGE: &str = "\
Usage:
	akit store export -c <config file>
	        print the config the store holds, as JSON
	akit store import -c <config file> <config file or directory>...
	        check the config the files hold and seal it as the store,
	        replacing it, with a new salt

The config file is the one akit run is started with: its admin.store says
where the store is and its password. To change the password, export the
store, change the password, and import what was exported.
";

const FLAGS: &[Spec] = &[Spec {
    name: "c",
    bool: false,
    usage: "the config file akit run is started with, whose admin.store names the store; give -c again for more, which are merged",
}];

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
    let paths = super::config_paths(&parsed);
    let shown = paths.join(", ");
    let bootstrap = match akit_config::read_config_files(&paths)
        .and_then(|f| akit_config::load_config_files(&f))
    {
        Ok(b) => b,
        Err(errs) => {
            super::print_errors(&shown, &errs);
            return ExitCode::FAILURE;
        }
    };
    let Some(st) = bootstrap.0.admin.as_ref().and_then(|a| a.store.clone()) else {
        eprintln!("{name}: {shown} names no store: no admin.store");
        return ExitCode::FAILURE;
    };
    let password = match st.password() {
        Ok(p) => p,
        Err(e) => {
            eprintln!("{name}: {e}");
            return ExitCode::FAILURE;
        }
    };
    let r = if cmd == "export" {
        export(&st.path, &password)
    } else {
        import(&st.path, &password, &bootstrap.1, files)
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

/// Prints the config the store at `path` holds.
fn export(path: &str, password: &str) -> Result<(), Option<String>> {
    let (_, data) = Store::open(path, password).map_err(|e| Some(e.to_string()))?;
    let Some(data) = data else {
        return Err(Some(format!("there is no store at {path} yet")));
    };
    use std::io::Write;
    let mut out = std::io::stdout().lock();
    out.write_all(&data)
        .and_then(|()| out.flush())
        .map_err(|e| Some(e.to_string()))
}

/// Seals the config `files` hold as the store at `path`, after checking it with the bootstrap
/// file's `admin` (`bootstrap`, the bootstrap config as JSON): with a new salt, keeping the
/// console users the old store kept when it opens with the password.
fn import(
    path: &str,
    password: &str,
    bootstrap: &str,
    files: &[String],
) -> Result<(), Option<String>> {
    let users = match Store::open(path, password) {
        Ok((s, _)) => s.users().to_vec(),
        // A store that does not open, as after the password was changed, is no reason to stop.
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
    let mut full = imported;
    if let (Value::Object(members), Some(admin)) = (
        &mut full,
        parse(bootstrap).ok().and_then(|b| b.get("admin").cloned()),
    ) {
        members.push(("admin".to_owned(), admin));
    }
    let checked = akit_config::from_json(&full.to_json(), None).and_then(|c| {
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
    let mut st = Store::create(path, password).map_err(|e| Some(e.to_string()))?;
    st.write(data.into_bytes())
        .map_err(|e| Some(e.to_string()))?;
    if !users.is_empty() {
        st.set_users(users).map_err(|e| Some(e.to_string()))?;
    }
    println!("{shown}: sealed into {path}");
    Ok(())
}
