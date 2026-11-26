//! fit's `tunnel/internal/bodycrypt`, byte for byte (decision 038): the bodies of link frames
//! sealed with AES-256-GCM under keys derived from the body password, or from a token and the
//! body password.
//!
//! The password's PBKDF2 (or the token's HKDF) is the [`Coder`]. Each link opens with both sides
//! sending a hello of [`RANDOM_SIZE`] fresh random bytes, and a [`Session`] of its own: each
//! direction has a key, HKDF-SHA256 of the coder's secret under the dialer's random bytes followed
//! by the listener's, the direction in the info; it numbers its frames from zero, the number being
//! the GCM nonce. A frame that does not open as the next one closes the link.

use std::sync::Arc;

use akio_crypto::gcm::AesGcm;
use akio_crypto::hash::Sha256;

const SALT: &str = "fit-tunnel-link-body";
const ITER: u32 = 10_000;
const INFO_TO_LISTENER: &str = "fit-tunnel-link v3 dialer to listener";
const INFO_TO_DIALER: &str = "fit-tunnel-link v3 listener to dialer";
const SALT_TOKEN: &str = "fit-tunnel-link-body token";
const INFO_TOKEN_TO_LISTENER: &str = "fit-tunnel-link v4 token dialer to listener";
const INFO_TOKEN_TO_DIALER: &str = "fit-tunnel-link v4 token listener to dialer";

/// The random bytes of a side's hello.
pub const RANDOM_SIZE: usize = 16;
/// How many more bytes a sealed body has than a plain one: its tag.
pub const OVERHEAD: usize = 16;

/// The coder of a token by its ID, on a listener; or why the token is refused.
pub type TokenCoders = Arc<dyn Fn(&str) -> Result<Coder, String> + Send + Sync>;

/// A password's PBKDF2, or a token's key, from which every link's keys derive, for the side that
/// listened or dialed.
#[derive(Clone)]
pub struct Coder {
    secret: [u8; 32],
    dialer: bool,
    token: String,
    tokens: Option<TokenCoders>,
}

impl std::fmt::Debug for Coder {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Coder")
            .field("dialer", &self.dialer)
            .field("token", &self.token)
            .finish_non_exhaustive()
    }
}

impl Coder {
    /// The coder of `password`, for the listening side; `None` for an empty one.
    #[must_use]
    pub fn new(password: &str) -> Option<Coder> {
        if password.is_empty() {
            return None;
        }
        let mut secret = [0u8; 32];
        akio_crypto::kdf::pbkdf2_hmac::<Sha256>(
            password.as_bytes(),
            SALT.as_bytes(),
            ITER,
            &mut secret,
        );
        Some(Coder {
            secret,
            dialer: false,
            token: String::new(),
            tokens: None,
        })
    }

    /// The coder of a link registered with token `id`, whose body key is `key`, and of the body
    /// password: HKDF, not PBKDF2.
    #[must_use]
    pub fn token(id: &str, key: &[u8], password: &str) -> Coder {
        let mut ikm = key.to_vec();
        ikm.extend_from_slice(password.as_bytes());
        let mut secret = [0u8; 32];
        let _ = akio_crypto::kdf::hkdf::<Sha256>(
            SALT_TOKEN.as_bytes(),
            &ikm,
            id.as_bytes(),
            &mut secret,
        );
        Coder {
            secret,
            dialer: false,
            token: id.to_owned(),
            tokens: None,
        }
    }

    /// The same coder for the side that dialed.
    #[must_use]
    pub fn dialer(&self) -> Coder {
        Coder {
            dialer: true,
            tokens: None,
            ..self.clone()
        }
    }

    /// The token's ID, or "" for a password's coder.
    #[must_use]
    pub fn token_id(&self) -> &str {
        &self.token
    }

    /// This coder for a listener that also takes the links of the tokens it issued.
    #[must_use]
    pub fn with_tokens(&self, of: TokenCoders) -> Coder {
        Coder {
            tokens: Some(of),
            ..self.clone()
        }
    }

    /// The coder of the link whose hello named token `id`, on a listener.
    ///
    /// # Errors
    ///
    /// A side that takes no tokens, or a token refused.
    pub fn for_token(&self, id: &str) -> Result<Coder, String> {
        match &self.tokens {
            Some(f) if !self.dialer => f(id),
            _ => Err(
                "bodycrypt: the peer's hello names a token, which this side does not take"
                    .to_owned(),
            ),
        }
    }

    /// The session of a link on which this side's hello carried `own` and the peer's `peer`.
    ///
    /// # Errors
    ///
    /// A hello of the wrong length, or one that is this side's own sent back.
    pub fn session(&self, own: &[u8], peer: &[u8]) -> Result<Session, String> {
        if own.len() != RANDOM_SIZE || peer.len() != RANDOM_SIZE {
            return Err("bodycrypt: a hello of the wrong length".to_owned());
        }
        if own == peer {
            return Err("bodycrypt: the peer's hello is this side's own, sent back".to_owned());
        }
        let (to_listener, to_dialer) = if self.token.is_empty() {
            (INFO_TO_LISTENER, INFO_TO_DIALER)
        } else {
            (INFO_TOKEN_TO_LISTENER, INFO_TOKEN_TO_DIALER)
        };
        let (dialer, listener, send_info, recv_info) = if self.dialer {
            (own, peer, to_listener, to_dialer)
        } else {
            (peer, own, to_dialer, to_listener)
        };
        let mut salt = dialer.to_vec();
        salt.extend_from_slice(listener);
        let aead = |info: &str| {
            let mut key = [0u8; 32];
            let _ =
                akio_crypto::kdf::hkdf::<Sha256>(&salt, &self.secret, info.as_bytes(), &mut key);
            AesGcm::new(&key).map_err(|e| format!("bodycrypt: {e}"))
        };
        Ok(Session {
            send: aead(send_info)?,
            recv: aead(recv_info)?,
            send_n: 0,
            recv_n: 0,
        })
    }
}

/// The keys and counters of one link, as one side sees it.
pub struct Session {
    send: AesGcm,
    recv: AesGcm,
    send_n: u64,
    recv_n: u64,
}

impl std::fmt::Debug for Session {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Session").finish_non_exhaustive()
    }
}

fn nonce(n: u64) -> [u8; 12] {
    let mut b = [0u8; 12];
    if let Some(tail) = b.get_mut(4..) {
        tail.copy_from_slice(&n.to_be_bytes());
    }
    b
}

/// What a frame that does not open says.
pub const ERR_OPEN: &str = "bodycrypt: a frame failed to decrypt: forged, replayed, reordered, or sealed under another password";

impl Session {
    /// Seals `body`, the next frame's, with `header` as the additional data, appending the tag.
    pub fn seal(&mut self, body: &mut Vec<u8>, header: &[u8]) {
        let tag = self
            .send
            .encrypt_in_place_detached(&nonce(self.send_n), header, body);
        self.send_n = self.send_n.wrapping_add(1);
        body.extend_from_slice(&tag);
    }

    /// Seals the body that is `frame[at..]`, its tag already reserved at its end.
    pub fn seal_in_place(&mut self, frame: &mut [u8], header: std::ops::Range<usize>, at: usize) {
        let header_bytes: Vec<u8> = frame.get(header).map(<[u8]>::to_vec).unwrap_or_default();
        let len = frame.len();
        let Some(body) = frame.get_mut(at..len.saturating_sub(OVERHEAD)) else {
            return;
        };
        let tag = self
            .send
            .encrypt_in_place_detached(&nonce(self.send_n), &header_bytes, body);
        self.send_n = self.send_n.wrapping_add(1);
        if let Some(t) = frame.get_mut(len.saturating_sub(OVERHEAD)..) {
            t.copy_from_slice(&tag);
        }
    }

    /// Opens `body`, the next frame's sealed body, with `header` as the additional data; the plain
    /// body is its first bytes.
    ///
    /// # Errors
    ///
    /// [`ERR_OPEN`].
    pub fn open<'b>(
        &mut self,
        body: &'b mut [u8],
        header: &[u8],
    ) -> Result<&'b [u8], &'static str> {
        let Some(split) = body.len().checked_sub(OVERHEAD) else {
            return Err(ERR_OPEN);
        };
        let (data, tag) = body.split_at_mut(split);
        self.recv
            .decrypt_in_place_detached(&nonce(self.recv_n), header, data, tag)
            .map_err(|_| ERR_OPEN)?;
        self.recv_n = self.recv_n.wrapping_add(1);
        Ok(data)
    }
}

/// Fresh random bytes of a hello.
#[must_use]
pub fn random() -> [u8; RANDOM_SIZE] {
    let mut b = [0u8; RANDOM_SIZE];
    let _ = akio_crypto::rand::fill_random(&mut b);
    b
}
