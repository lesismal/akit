//! The role `media_auth` (I121), as fit's: it moves the policies of its engine to every config
//! applied ([`crate::auth::registry::apply`]), and removes them when a config leaves
//! `media_auth` out, so that a handler or a listener still holding one refuses as unavailable.

use std::time::Duration;

use akit_config::Config;
use akit_roles::host::{Host, Info, Role};

use crate::auth::registry;

/// The role `media_auth`, which `akit run` registers before the roles that name its policies.
pub const MEDIA_AUTH: Info = Info {
    id: "media_auth",
    set: |c| !c.media_auth.is_empty(),
    new: |host| Ok(Box::new(AuthRole::new(&host))),
};

/// The role.
#[derive(Debug)]
pub struct AuthRole {
    key: usize,
}

impl AuthRole {
    fn new(host: &Host) -> AuthRole {
        AuthRole {
            key: registry::key(&host.engine),
        }
    }
}

impl Role for AuthRole {
    fn apply(&mut self, config: &Config) -> Result<(), String> {
        registry::apply(self.key, &config.media_auth)
    }

    fn stop(&mut self, _grace: Duration) {
        registry::remove(self.key);
    }

    fn files(&self) -> Vec<(String, [u8; 32])> {
        registry::files(self.key)
    }
}
