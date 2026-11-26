//! fit's mesh protocol (`llm/internal/wire`), byte for byte: frames carried one per WebSocket
//! message ([`frame`]), and a [`Session`] that runs calls, flow-controlled streams and pushes over
//! any "send a frame, receive a frame" transport ([`session`]); the transport over akio-ws, with
//! the handshake that follows the upgrade, the heartbeat and the link's pacing ([`conn`]).
//!
//! fit runs each call's handler in a goroutine of its own and lets a stream's ends wait in `Send`
//! and `Recv`; akit has no goroutines and blocks no I/O thread (rule 1), so here everything is a
//! callback: a call's handler answers through a [`Reply`] whenever it is ready, a stream's data
//! arrives at [`Stream::on_data`] and is given back with [`Stream::release`], and a sender that
//! finds no credit ([`Stream::try_send`]) is told when to try again ([`Stream::on_credit`]).

pub mod conn;
pub mod frame;
pub mod session;

pub use conn::{Conn, Credential, DialConfig, Listener, ListenerConfig, Tokens};
pub use frame::{Frame, Type};
pub use session::{Event, Handler, Options, Reply, Request, Session, Stream, pump};

use std::fmt;

use akio_crypto::hash::Sha256;
use akio_crypto::mac::Hmac;

use crate::json;
use crate::model::{CODE_INTERNAL, CODE_NOT_FOUND};

/// Why something failed: a business error a peer sent ([`Error::Remote`], fit's `*wire.Error`,
/// whose code crosses the mesh unchanged), or a local failure, which crosses it as an internal
/// error with its message.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Error {
    /// A business error: a code of [`crate::model`]'s and its message.
    Remote {
        /// The code.
        code: i64,
        /// The message.
        msg: String,
    },
    /// The session closed.
    Closed,
    /// The stream was canceled.
    Canceled,
    /// The stream saw no data either way for its idle timeout.
    StreamIdle,
    /// The stream had ended already.
    StreamEnded,
    /// The peer sent more than the stream's window.
    FlowControl,
    /// The peer broke the protocol.
    Protocol(String),
    /// A frame passed the bound.
    FrameTooLarge,
    /// A frame that does not decode.
    BadFrame(String),
    /// A deadline passed.
    Timeout,
    /// The peer sent nothing for three ping intervals.
    Heartbeat,
    /// A ping waited its slow timeout for its pong.
    Slow,
    /// The listener turned the link away before authentication.
    Refused(String),
    /// The handshake failed.
    Auth(String),
    /// The transport failed to send.
    Send(String),
    /// Anything else.
    Other(String),
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let with = |f: &mut fmt::Formatter<'_>, base: &str, s: &str| {
            if s.is_empty() {
                f.write_str(base)
            } else {
                write!(f, "{base}: {s}")
            }
        };
        match self {
            Error::Remote { msg, .. } => f.write_str(msg),
            Error::Closed => f.write_str("wire: session closed"),
            Error::Canceled => f.write_str("wire: stream canceled"),
            Error::StreamIdle => f.write_str("wire: stream idle timeout"),
            Error::StreamEnded => f.write_str("wire: stream already ended"),
            Error::FlowControl => f.write_str("wire: peer exceeded stream window"),
            Error::Protocol(s) => with(f, "wire: protocol violation", s),
            Error::FrameTooLarge => f.write_str("wire: frame too large"),
            Error::BadFrame(s) => with(f, "wire: malformed frame", s),
            Error::Timeout => f.write_str("context deadline exceeded"),
            Error::Heartbeat => f.write_str("wire: heartbeat timeout"),
            Error::Slow => f.write_str("wire: connection too slow"),
            Error::Refused(s) => with(f, "wire: connection refused by the listener", s),
            Error::Auth(s) => with(f, "wire: authentication rejected", s),
            Error::Send(s) => with(f, "wire: send", s),
            Error::Other(s) => f.write_str(s),
        }
    }
}

impl std::error::Error for Error {}

impl Error {
    /// A business error.
    #[must_use]
    pub fn code(code: i64, msg: impl Into<String>) -> Error {
        Error::Remote {
            code,
            msg: msg.into(),
        }
    }

    /// The error's code and message as a method's caller gets them (fit's `mesh.ErrCode`): a
    /// business error's own, a timeout's `timeout`, any other's message as an internal error.
    #[must_use]
    pub fn code_msg(&self) -> (i64, String) {
        match self {
            Error::Remote { code, msg } => (*code, msg.clone()),
            Error::Timeout => (CODE_INTERNAL, "timeout".to_owned()),
            e => (CODE_INTERNAL, e.to_string()),
        }
    }

    /// As a method's error crosses the mesh (fit's `wireErr`).
    #[must_use]
    pub fn to_remote(&self) -> Error {
        let (code, msg) = self.code_msg();
        Error::Remote { code, msg }
    }
}

/// An error as a frame's payload: `{"code":…,"message":…}` (fit's `encodeError`: a business
/// error keeps its code, any other is an internal error with its message).
#[must_use]
pub fn encode_error(e: &Error) -> Vec<u8> {
    let (code, msg) = match e {
        Error::Remote { code, msg } => (*code, msg.clone()),
        e => (CODE_INTERNAL, e.to_string()),
    };
    json::Obj::new()
        .int("code", code)
        .str("message", &msg)
        .end()
        .into_bytes()
}

/// A frame's error payload (fit's `decodeError`).
#[must_use]
pub fn decode_error(p: &[u8]) -> Error {
    if let Ok(v) = json::parse(p) {
        let msg = json::get_str(&v, "message");
        if !msg.is_empty() {
            return Error::Remote {
                code: json::get_int(&v, "code"),
                msg,
            };
        }
    }
    Error::code(CODE_INTERNAL, "wire: malformed error payload")
}

/// A refusal of a call or stream no handler serves.
#[must_use]
pub fn not_supported(what: &str) -> Error {
    Error::code(CODE_NOT_FOUND, format!("{what} not supported"))
}

// ---- Passwords (fit's `auth.go`) ----

/// The size of a dialer's nonce after its proof, when it proves a password both ways.
pub const NONCE_SIZE: usize = 32;
/// The size of a password's proof.
pub const PROOF_SIZE: usize = 32;
/// The size of the listener's challenge.
pub const CHALLENGE_SIZE: usize = 32;

/// Sets the listener's proof apart from a dialer's.
const LISTENER_LABEL: &str = "fit llm wire listener\x00";

/// The dialer's proof of `password` for `challenge`: HMAC-SHA256 of the challenge keyed by it.
#[must_use]
pub fn password_proof(password: &str, challenge: &[u8]) -> [u8; 32] {
    let mut m = Hmac::<Sha256>::new(password.as_bytes());
    m.update(challenge);
    m.finalize()
}

/// The listener's proof of `password` for its challenge and the dialer's nonce.
#[must_use]
pub fn password_listener_proof(password: &str, challenge: &[u8], nonce: &[u8]) -> [u8; 32] {
    let mut m = Hmac::<Sha256>::new(password.as_bytes());
    m.update(LISTENER_LABEL.as_bytes());
    m.update(challenge);
    m.update(nonce);
    m.finalize()
}

// ---- Tokens (fit's `token.go`) ----

/// Starts a dialer's answer when it proves a token:
/// `magic:4 idlen:u8 id nonce:32 proof:32`.
pub const TOKEN_MAGIC: &[u8] = b"FTK1";
/// Names the mesh's handshake in a token's proofs.
pub const TOKEN_LABEL: &str = "fit llm mesh v1";

/// The transcript of a token's proofs.
#[must_use]
pub fn token_transcript<'a>(
    id: &'a str,
    nonce: &'a [u8],
    challenge: &'a [u8],
) -> akit_roles::linktoken::Transcript<'a> {
    akit_roles::linktoken::Transcript {
        label: TOKEN_LABEL,
        token: id,
        dialer: nonce,
        listener: challenge,
        extra: &[],
    }
}

/// A dialer's answer to `challenge` with token `sec`, and its nonce.
///
/// # Errors
///
/// No random bytes.
pub fn token_answer(
    sec: &akit_roles::linktoken::Secret,
    challenge: &[u8],
) -> std::io::Result<(Vec<u8>, Vec<u8>)> {
    let nonce = akit_roles::linktoken::nonce()?;
    let id = sec.id();
    let mut b = TOKEN_MAGIC.to_vec();
    b.push(u8::try_from(id.len()).unwrap_or(u8::MAX));
    b.extend_from_slice(id.as_bytes());
    b.extend_from_slice(&nonce);
    b.extend_from_slice(&sec.prove(&token_transcript(id, &nonce, challenge)));
    Ok((b, nonce.to_vec()))
}

/// Splits an answer [`token_answer`] made: the token's ID, the nonce and the proof.
#[must_use]
pub fn parse_token_answer(data: &[u8]) -> Option<(String, &[u8], &[u8])> {
    use akit_roles::linktoken::{NONCE_SIZE as N, PROOF_SIZE as P};
    let rest = data.strip_prefix(TOKEN_MAGIC)?;
    let n = usize::from(*rest.first()?);
    if n == 0 || rest.len() != 1usize.saturating_add(n).saturating_add(N).saturating_add(P) {
        return None;
    }
    let id = rest.get(1..=n)?;
    let nonce = rest.get(n.saturating_add(1)..n.saturating_add(1).saturating_add(N))?;
    let proof = rest.get(n.saturating_add(1).saturating_add(N)..)?;
    Some((String::from_utf8_lossy(id).into_owned(), nonce, proof))
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]
    use super::*;

    #[test]
    fn errors_cross_as_fit_writes_them() {
        let e = Error::code(4003, "peer is offline");
        let b = encode_error(&e);
        assert_eq!(b, br#"{"code":4003,"message":"peer is offline"}"#);
        assert_eq!(decode_error(&b), e);
        assert_eq!(
            decode_error(&encode_error(&Error::Closed)),
            Error::code(5000, "wire: session closed")
        );
        assert_eq!(
            decode_error(b"{}"),
            Error::code(5000, "wire: malformed error payload")
        );
        assert_eq!(Error::Timeout.code_msg(), (5000, "timeout".to_owned()));
    }

    #[test]
    fn token_answers() {
        let sec = akit_roles::linktoken::Secret::new().unwrap();
        let ch = [7u8; CHALLENGE_SIZE];
        let (a, nonce) = token_answer(&sec, &ch).unwrap();
        let (id, n, proof) = parse_token_answer(&a).unwrap();
        assert_eq!(id, sec.id());
        assert_eq!(n, nonce.as_slice());
        let t = token_transcript(&id, n, &ch);
        assert!(akit_roles::linktoken::verify(&sec.stored(), &t, proof));
        assert!(parse_token_answer(&a[..a.len() - 1]).is_none());
        assert!(parse_token_answer(&[0u8; 32]).is_none());
        assert!(parse_token_answer(b"FTK1\x00").is_none());
    }

    #[test]
    fn password_proofs_differ_by_side() {
        let ch = [1u8; 32];
        let d = password_proof("pw", &ch);
        let l = password_listener_proof("pw", &ch, &[2u8; 32]);
        assert_ne!(d, l);
        assert_ne!(password_proof("other", &ch), d);
    }
}
