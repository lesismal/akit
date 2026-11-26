//! The console (I89–I91): web pages behind logins, from which the running config is read and
//! changed through the Admin API, called in process ([decision 036](../../../../docs/decisions/036-console.md)).
//!
//! - [`login`]: users, password checks, lockouts and sessions, as fit's package `login` (I89).
//! - `server`: the console's server and its endpoints, as fit's `app/console` and
//!   `admin/console.go` (I90).
//! - `pages`: the files it serves (I91).

pub mod login;
pub(crate) mod pages;
pub(crate) mod server;

pub use server::{ADMIN_MODULE, COOKIE, CSP, CSRF_HEADER, IDLE_TIMEOUT, MAX_LOGIN_BYTES};

/// The source of a config the console applied, as `/versions` names it: fit's `SourceConsole`.
pub const SOURCE_CONSOLE: &str = "console";

pub(crate) use crate::server::accept_filter;
