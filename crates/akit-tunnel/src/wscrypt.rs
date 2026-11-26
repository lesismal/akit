//! The sealing of the WebSocket a hub's console is reached through (I103b, fit's
//! `tunnel/internal/wscrypt`, byte for byte): every binary message is sealed with AES-256-GCM
//! under a key both sides derive from the password they share.
//!
//! The server (the hub's `console_socket`) opens the session with a message of [`HELLO_LEN`]
//! random bytes, and the client (a proxy's web proxy) answers with as many of its own. Each
//! direction then has a key of its own, HKDF-SHA256 of the password's PBKDF2 under both sets of
//! bytes, and numbers its messages from zero, the number being the GCM nonce. A message that does
//! not open under the next number (forged, replayed, reordered, or sealed under another password)
//! ends the session.

use akio_crypto::gcm::AesGcm;
use akio_crypto::hash::Sha256;

/// The length of the random bytes each side opens with.
pub const HELLO_LEN: usize = 16;
/// How many more bytes a sealed message has than what it carries: its tag.
pub const OVERHEAD: usize = 16;

const PBKDF2_SALT: &str = "fit-tunnel-ws";
const PBKDF2_ITER: u32 = 10_000;
const INFO_C2S: &str = "fit-tunnel-ws c2s";
const INFO_S2C: &str = "fit-tunnel-ws s2c";

/// What a message that does not open says.
pub const ERR_OPEN: &str = "wscrypt: message failed to decrypt";

/// A password's PBKDF2, derived once for every session: a role derives it when its config is
/// applied, on the control thread, not as a connection opens.
#[derive(Clone, PartialEq, Eq)]
pub struct Secret([u8; 32]);

impl std::fmt::Debug for Secret {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Secret(..)")
    }
}

impl Secret {
    /// The secret of `password`.
    #[must_use]
    pub fn new(password: &str) -> Secret {
        let mut b = [0u8; 32];
        akio_crypto::kdf::pbkdf2_hmac::<Sha256>(
            password.as_bytes(),
            PBKDF2_SALT.as_bytes(),
            PBKDF2_ITER,
            &mut b,
        );
        Secret(b)
    }
}

/// Fresh random bytes one side opens with.
#[must_use]
pub fn hello() -> [u8; HELLO_LEN] {
    let mut b = [0u8; HELLO_LEN];
    let _ = akio_crypto::rand::fill_random(&mut b);
    b
}

/// Seals what one side sends and opens what it receives.
pub struct Session {
    send: AesGcm,
    send_n: u64,
    recv: AesGcm,
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

fn aead(secret: &Secret, salt: &[u8], info: &str) -> Result<AesGcm, String> {
    let mut key = [0u8; 32];
    akio_crypto::kdf::hkdf::<Sha256>(salt, &secret.0, info.as_bytes(), &mut key)
        .map_err(|e| format!("wscrypt: {e}"))?;
    AesGcm::new(&key).map_err(|e| format!("wscrypt: {e}"))
}

impl Session {
    /// The session of the server's side, when `server`, or the client's, once `server_hello`
    /// and `client_hello` are exchanged.
    ///
    /// # Errors
    ///
    /// A hello of the wrong length.
    pub fn new(
        secret: &Secret,
        server_hello: &[u8],
        client_hello: &[u8],
        server: bool,
    ) -> Result<Session, String> {
        if server_hello.len() != HELLO_LEN || client_hello.len() != HELLO_LEN {
            return Err("wscrypt: bad hello".to_owned());
        }
        let mut salt = server_hello.to_vec();
        salt.extend_from_slice(client_hello);
        let c2s = aead(secret, &salt, INFO_C2S)?;
        let s2c = aead(secret, &salt, INFO_S2C)?;
        let (send, recv) = if server { (s2c, c2s) } else { (c2s, s2c) };
        Ok(Session {
            send,
            send_n: 0,
            recv,
            recv_n: 0,
        })
    }

    /// The message carrying `plain`: its bytes sealed, the tag after them.
    pub fn seal(&mut self, plain: &[u8]) -> Vec<u8> {
        let mut out = Vec::with_capacity(plain.len().saturating_add(OVERHEAD));
        out.extend_from_slice(plain);
        let tag = self
            .send
            .encrypt_in_place_detached(&nonce(self.send_n), &[], &mut out);
        self.send_n = self.send_n.wrapping_add(1);
        out.extend_from_slice(&tag);
        out
    }

    /// What the next message the other side sent carries.
    ///
    /// # Errors
    ///
    /// [`ERR_OPEN`]: a message that does not open as the next one.
    pub fn open(&mut self, message: &[u8]) -> Result<Vec<u8>, &'static str> {
        let Some(split) = message.len().checked_sub(OVERHEAD) else {
            return Err(ERR_OPEN);
        };
        let (data, tag) = message.split_at(split);
        let mut out = data.to_vec();
        self.recv
            .decrypt_in_place_detached(&nonce(self.recv_n), &[], &mut out, tag)
            .map_err(|_| ERR_OPEN)?;
        self.recv_n = self.recv_n.wrapping_add(1);
        Ok(out)
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::indexing_slicing)]
mod tests {
    use super::*;

    /// fit's `TestSession`.
    #[test]
    fn session() {
        let secret = Secret::new("pw");
        let (sh, ch) = (hello(), hello());
        let mut server = Session::new(&secret, &sh, &ch, true).unwrap();
        let mut client = Session::new(&secret, &sh, &ch, false).unwrap();
        for msg in ["one", "two"] {
            let got = server.open(&client.seal(msg.as_bytes())).unwrap();
            assert_eq!(got, msg.as_bytes());
        }
        let sealed = server.seal(b"x");
        client.open(&sealed).unwrap();
        assert!(client.open(&sealed).is_err(), "a replayed message opened");
        let other = Secret::new("other");
        let mut wrong = Session::new(&other, &sh, &ch, false).unwrap();
        assert!(
            server.open(&wrong.seal(b"x")).is_err(),
            "a message of another password opened"
        );
        // An empty message, a web proxy's proof of the password, opens empty.
        assert_eq!(server.open(&client.seal(b"")).unwrap(), b"");
        assert!(server.open(b"short").is_err());
    }

    /// fit's `TestNewSessionBadHello`.
    #[test]
    fn bad_hello() {
        let secret = Secret::new("pw");
        assert!(Session::new(&secret, &hello()[..HELLO_LEN - 1], &hello(), true).is_err());
        assert!(Session::new(&secret, &hello(), &[], false).is_err());
    }

    /// What fit's `wscrypt` seals, byte for byte: a session of fixed hellos under password `pw`,
    /// computed by fit's `wscrypt.go` itself, copied into a scratch module (Go's `crypto/pbkdf2`, `crypto/hkdf` and AES-GCM, as fit's
    /// `wscrypt.NewSession` and `Seal`).
    #[test]
    fn fits_bytes() {
        let secret = Secret::new("pw");
        let sh: Vec<u8> = (0u8..16).collect();
        let ch: Vec<u8> = (16u8..32).collect();
        let mut client = Session::new(&secret, &sh, &ch, false).unwrap();
        let mut server = Session::new(&secret, &sh, &ch, true).unwrap();
        let hex = |b: &[u8]| b.iter().map(|x| format!("{x:02x}")).collect::<String>();
        assert_eq!(hex(&client.seal(b"hello")), FIT_C2S_0);
        assert_eq!(hex(&client.seal(b"")), FIT_C2S_1);
        assert_eq!(hex(&server.seal(b"console")), FIT_S2C_0);
    }

    const FIT_C2S_0: &str = "c9fbd1208e98fc59ba4d7e9388f95291a36a71ce68";
    const FIT_C2S_1: &str = "54ab4e563087d0d42d91791aa5ab7855";
    const FIT_S2C_0: &str = "43bd4b5684a1b1220f97c85339f89b9e4a3c774dcda662";
}
