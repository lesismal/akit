//! fit's frames (`llm/internal/wire/frame.go`), byte for byte: one per WebSocket message,
//!
//! ```text
//! type:u8  flags:u8  id:u32 (big endian)  [method_len:u8 method]  payload
//! ```
//!
//! the method only on requests, stream opens and pushes; a payload of 1 KiB or more gzipped when
//! that makes it smaller (`FLAG_GZIP`); 16 MiB at most, encoded or inflated.

use akio_deflate::{Format, Level};

use super::Error;

/// A frame's type.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum Type {
    /// A call.
    Request = 1,
    /// A call's answer.
    Response = 2,
    /// A stream opened.
    StreamOpen = 3,
    /// A stream's data.
    StreamData = 4,
    /// A stream's end (the opener's half close, or the responder's result).
    StreamEnd = 5,
    /// A call or stream given up.
    StreamCancel = 6,
    /// A one-way message.
    Push = 7,
    /// A ping, its sequence the payload.
    Ping = 8,
    /// A ping's answer.
    Pong = 9,
    /// A stream's credit given back: 4 bytes, big endian.
    Credit = 10,
    /// A stream's idle timeout reset, no data.
    StreamKeepalive = 11,
    /// The sender is going away: nothing new on this link.
    Drain = 12,
}

impl Type {
    /// The type of byte `b`.
    #[must_use]
    pub fn of(b: u8) -> Option<Type> {
        Some(match b {
            1 => Type::Request,
            2 => Type::Response,
            3 => Type::StreamOpen,
            4 => Type::StreamData,
            5 => Type::StreamEnd,
            6 => Type::StreamCancel,
            7 => Type::Push,
            8 => Type::Ping,
            9 => Type::Pong,
            10 => Type::Credit,
            11 => Type::StreamKeepalive,
            12 => Type::Drain,
            _ => return None,
        })
    }

    /// fit's name of the type.
    #[must_use]
    pub fn name(self) -> &'static str {
        match self {
            Type::Request => "request",
            Type::Response => "response",
            Type::StreamOpen => "stream-open",
            Type::StreamData => "stream-data",
            Type::StreamEnd => "stream-end",
            Type::StreamCancel => "stream-cancel",
            Type::Push => "push",
            Type::Ping => "ping",
            Type::Pong => "pong",
            Type::Credit => "credit",
            Type::StreamKeepalive => "stream-keepalive",
            Type::Drain => "drain",
        }
    }

    /// Whether frames of the type carry a method.
    #[must_use]
    pub fn has_head(self) -> bool {
        matches!(self, Type::Request | Type::StreamOpen | Type::Push)
    }
}

/// The payload is gzipped.
pub const FLAG_GZIP: u8 = 1;
/// The payload is an error, `{"code":…,"message":…}`.
pub const FLAG_ERROR: u8 = 1 << 1;
/// The payload is binary, not JSON.
pub const FLAG_BINARY: u8 = 1 << 2;

/// The header's size: type, flags, stream ID.
pub const HEADER_SIZE: usize = 6;
/// The longest method: its length is one byte.
pub const MAX_HEAD_FIELD: usize = 255;
/// The largest frame, encoded or inflated, by default.
pub const DEFAULT_MAX_FRAME: usize = 16 << 20;
/// The smallest payload compressed, by default.
pub const DEFAULT_COMPRESS: usize = 1 << 10;

/// One frame.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Frame {
    /// Its type.
    pub kind: Type,
    /// Its flags, `FLAG_GZIP` aside.
    pub flags: u8,
    /// Its stream ID.
    pub id: u32,
    /// Its method (requests, stream opens and pushes).
    pub method: String,
    /// Its payload, inflated.
    pub payload: Vec<u8>,
}

impl Frame {
    /// A frame of `kind` for stream `id`, without method or payload.
    #[must_use]
    pub fn new(kind: Type, id: u32) -> Frame {
        Frame {
            kind,
            flags: 0,
            id,
            method: String::new(),
            payload: Vec::new(),
        }
    }
}

/// Encodes a frame. A payload of at least `compress_min` bytes (0: never) is gzipped when that
/// makes it smaller; the frame must not pass `max_frame` (0: no bound).
///
/// # Errors
///
/// [`Error::FrameTooLarge`]; [`Error::BadFrame`] for a method longer than 255 bytes.
pub fn marshal(
    kind: Type,
    flags: u8,
    id: u32,
    method: &str,
    payload: &[u8],
    compress_min: usize,
    max_frame: usize,
) -> Result<Vec<u8>, Error> {
    let mut flags = flags & !FLAG_GZIP;
    let mut zipped = None;
    if compress_min > 0 && payload.len() >= compress_min {
        let z = akio_deflate::compress(Format::Gzip, Level::DEFAULT, payload);
        if z.len() < payload.len() {
            flags |= FLAG_GZIP;
            zipped = Some(z);
        }
    }
    let body: &[u8] = zipped.as_deref().unwrap_or(payload);
    let mut size = HEADER_SIZE.saturating_add(body.len());
    if kind.has_head() {
        if method.len() > MAX_HEAD_FIELD {
            return Err(Error::BadFrame(format!(
                "method longer than {MAX_HEAD_FIELD} bytes"
            )));
        }
        size = size.saturating_add(1).saturating_add(method.len());
    }
    if max_frame > 0 && size > max_frame {
        return Err(Error::FrameTooLarge);
    }
    let mut b = Vec::with_capacity(size);
    b.push(kind as u8);
    b.push(flags);
    b.extend_from_slice(&id.to_be_bytes());
    if kind.has_head() {
        b.push(u8::try_from(method.len()).unwrap_or(u8::MAX));
        b.extend_from_slice(method.as_bytes());
    }
    b.extend_from_slice(body);
    Ok(b)
}

/// Decodes a frame; a gzipped payload is inflated, and must not pass `max_frame` (0: no bound).
///
/// # Errors
///
/// [`Error::FrameTooLarge`], [`Error::BadFrame`].
pub fn unmarshal(b: &[u8], max_frame: usize) -> Result<Frame, Error> {
    if max_frame > 0 && b.len() > max_frame {
        return Err(Error::FrameTooLarge);
    }
    let (Some(&t), Some(&flags), Some(id)) = (b.first(), b.get(1), b.get(2..HEADER_SIZE)) else {
        return Err(Error::BadFrame(String::new()));
    };
    let Some(kind) = Type::of(t) else {
        return Err(Error::BadFrame(format!("unknown type {t}")));
    };
    let id = u32::from_be_bytes([
        id.first().copied().unwrap_or(0),
        id.get(1).copied().unwrap_or(0),
        id.get(2).copied().unwrap_or(0),
        id.get(3).copied().unwrap_or(0),
    ]);
    let mut rest = b.get(HEADER_SIZE..).unwrap_or(&[]);
    let mut method = String::new();
    if kind.has_head() {
        let n = usize::from(*rest.first().ok_or(Error::BadFrame(String::new()))?);
        let m = rest
            .get(1..n.saturating_add(1))
            .ok_or(Error::BadFrame(String::new()))?;
        method = String::from_utf8_lossy(m).into_owned();
        rest = rest.get(n.saturating_add(1)..).unwrap_or(&[]);
    }
    let payload = if flags & FLAG_GZIP != 0 {
        let limit = if max_frame > 0 { max_frame } else { usize::MAX };
        match akio_deflate::decompress(Format::Gzip, rest, limit) {
            Ok(p) if max_frame > 0 && p.len() > max_frame => return Err(Error::FrameTooLarge),
            Ok(p) => p,
            Err(akio_deflate::Error::TooLarge) => return Err(Error::FrameTooLarge),
            Err(e) => return Err(Error::BadFrame(e.to_string())),
        }
    } else {
        rest.to_vec()
    };
    Ok(Frame {
        kind,
        flags: flags & !FLAG_GZIP,
        id,
        method,
        payload,
    })
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::indexing_slicing)]
    use super::*;

    #[test]
    fn round_trip() {
        for (kind, method) in [
            (Type::Request, "/node/info"),
            (Type::StreamData, ""),
            (Type::Push, "/node/update"),
        ] {
            for payload in [Vec::new(), b"{}".to_vec(), b"x".repeat(4096)] {
                let b =
                    marshal(kind, FLAG_BINARY, 7, method, &payload, DEFAULT_COMPRESS, 0).unwrap();
                let f = unmarshal(&b, DEFAULT_MAX_FRAME).unwrap();
                assert_eq!(f.kind, kind);
                assert_eq!(f.id, 7);
                assert_eq!(f.flags, FLAG_BINARY);
                assert_eq!(f.payload, payload);
                assert_eq!(f.method, if kind.has_head() { method } else { "" });
                if payload.len() >= DEFAULT_COMPRESS {
                    assert!(b.len() < payload.len(), "compressed");
                    assert_eq!(b[1] & FLAG_GZIP, FLAG_GZIP);
                }
            }
        }
        // fit's bytes: a request of /x with {} on stream 3.
        assert_eq!(
            marshal(Type::Request, 0, 3, "/x", b"{}", DEFAULT_COMPRESS, 0).unwrap(),
            [1, 0, 0, 0, 0, 3, 2, b'/', b'x', b'{', b'}']
        );
    }

    #[test]
    fn limits() {
        assert_eq!(
            marshal(Type::StreamData, 0, 1, "", &[0; 100], 0, 50),
            Err(Error::FrameTooLarge)
        );
        assert!(marshal(Type::Request, 0, 1, &"m".repeat(256), b"", 0, 0).is_err());
        assert!(unmarshal(&[1, 0, 0, 0], 0).is_err());
        assert!(unmarshal(&[0, 0, 0, 0, 0, 0], 0).is_err());
        assert!(unmarshal(&[13, 0, 0, 0, 0, 0], 0).is_err());
        assert!(unmarshal(&[1, 0, 0, 0, 0, 0, 5, b'a'], 0).is_err());
        assert_eq!(unmarshal(&[0; 100], 50), Err(Error::FrameTooLarge));
        // A gzip bomb: inflates past the bound.
        let big = vec![0u8; 1 << 20];
        let b = marshal(Type::StreamData, 0, 1, "", &big, 1, 0).unwrap();
        assert!(b.len() < 64 << 10);
        assert_eq!(unmarshal(&b, 64 << 10), Err(Error::FrameTooLarge));
        // Not gzip at all.
        assert!(unmarshal(&[4, FLAG_GZIP, 0, 0, 0, 1, 1, 2, 3], 0).is_err());
    }
}
