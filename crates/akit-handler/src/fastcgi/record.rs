//! FastCGI 1.0's records, as akit uses them (fit's `record.go`): one request at a time on a
//! connection, always with id 1, as PHP-FPM serves them.

/// The record types akit sends and reads.
pub(crate) const BEGIN_REQUEST: u8 = 1;
pub(crate) const END_REQUEST: u8 = 3;
pub(crate) const PARAMS: u8 = 4;
pub(crate) const STDIN: u8 = 5;
pub(crate) const STDOUT: u8 = 6;
pub(crate) const STDERR: u8 = 7;

const VERSION: u8 = 1;
const RESPONDER: u8 = 1;
const KEEP_CONN: u8 = 1;
/// The one request id akit uses.
pub(crate) const REQUEST_ID: u16 = 1;
const HEADER_LEN: usize = 8;
/// The most content one record carries.
const MAX_CONTENT: usize = 65535;

/// Appends records of type `t` carrying `content`, split as one record carries at most 65535
/// bytes; an empty `content` makes one empty record, which ends a stream.
pub(crate) fn append_record(b: &mut Vec<u8>, t: u8, mut content: &[u8]) {
    loop {
        let n = content.len().min(MAX_CONTENT);
        let pad = n.wrapping_neg() % 8;
        let [hi, lo] = u16::try_from(n).unwrap_or(u16::MAX).to_be_bytes();
        let [id_hi, id_lo] = REQUEST_ID.to_be_bytes();
        // `pad` is below 8.
        let pad_byte = u8::try_from(pad).unwrap_or(0);
        b.extend_from_slice(&[VERSION, t, id_hi, id_lo, hi, lo, pad_byte, 0]);
        let (now, rest) = content.split_at(n);
        b.extend_from_slice(now);
        b.resize(b.len().saturating_add(pad), 0);
        content = rest;
        if content.is_empty() {
            return;
        }
    }
}

/// The record that starts a request as a responder, the connection kept for the next.
pub(crate) fn append_begin_request(b: &mut Vec<u8>) {
    append_record(b, BEGIN_REQUEST, &[0, RESPONDER, KEEP_CONN, 0, 0, 0, 0, 0]);
}

/// Appends a name-value pair as FastCGI encodes them: each length in one byte below 128, in four
/// with the high bit set past it. A name or value past 2^31 bytes cannot be encoded: it is cut
/// (no config or request holds one; the listener bounds a request's header).
pub(crate) fn append_param(b: &mut Vec<u8>, name: &[u8], value: &[u8]) {
    let name = name.get(..name.len().min(0x7fff_ffff)).unwrap_or(name);
    let value = value.get(..value.len().min(0x7fff_ffff)).unwrap_or(value);
    for n in [name.len(), value.len()] {
        match u8::try_from(n) {
            Ok(small) if small < 128 => b.push(small),
            _ => {
                let n = u32::try_from(n).unwrap_or(0x7fff_ffff) | 1 << 31;
                b.extend_from_slice(&n.to_be_bytes());
            }
        }
    }
    b.extend_from_slice(name);
    b.extend_from_slice(value);
}

/// One record read: its type, request id and content.
#[derive(Debug)]
pub(crate) struct Record<'a> {
    pub(crate) t: u8,
    pub(crate) id: u16,
    pub(crate) content: &'a [u8],
}

/// The application broke FastCGI's framing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Protocol;

/// Reads records out of what a connection delivers, a piece at a time, keeping at most one
/// record's worth (64 KiB and its header and padding) between pieces.
#[derive(Debug, Default)]
pub(crate) struct Reader {
    buf: Vec<u8>,
}

impl Reader {
    /// Whether part of a record waits for the rest.
    pub(crate) fn is_empty(&self) -> bool {
        self.buf.is_empty()
    }

    /// Adds `data` and calls `f` with each whole record in it, until `f` says stop (`false`);
    /// what follows is kept. The content `f` gets is valid only during the call. Returns
    /// whether bytes are left over after the record `f` stopped at (or a partial record).
    pub(crate) fn feed(
        &mut self,
        data: &[u8],
        mut f: impl FnMut(Record<'_>) -> bool,
    ) -> Result<bool, Protocol> {
        let mut kept = std::mem::take(&mut self.buf);
        let all: &[u8] = if kept.is_empty() {
            data
        } else {
            kept.extend_from_slice(data);
            &kept
        };
        let mut rest = all;
        let mut stopped = false;
        while let Some(h) = rest.get(..HEADER_LEN) {
            if h.first() != Some(&VERSION) {
                return Err(Protocol);
            }
            let byte = |i: usize| h.get(i).copied().unwrap_or(0);
            let n = usize::from(u16::from_be_bytes([byte(4), byte(5)]));
            let total = HEADER_LEN
                .saturating_add(n)
                .saturating_add(usize::from(byte(6)));
            let Some(content) = rest.get(HEADER_LEN..HEADER_LEN.saturating_add(n)) else {
                break;
            };
            if rest.len() < total {
                break;
            }
            let rec = Record {
                t: byte(1),
                id: u16::from_be_bytes([byte(2), byte(3)]),
                content,
            };
            rest = rest.get(total..).unwrap_or_default();
            if !f(rec) {
                stopped = true;
                break;
            }
        }
        let left = !rest.is_empty();
        if !stopped {
            // Part of a record: kept, in the reader's own buffer, since `data` is the
            // connection's.
            let mut buf = Vec::new();
            buf.extend_from_slice(rest);
            self.buf = buf;
        }
        Ok(left)
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::indexing_slicing)]

    use super::*;

    /// fit's `TestRecords`: fed a byte at a time and in one piece, the same comes out.
    #[test]
    fn records() {
        let big = vec![b'x'; 70000];
        let mut b = Vec::new();
        append_record(&mut b, STDOUT, &big);
        append_record(&mut b, END_REQUEST, &[0; 8]);
        for step in [1, b.len()] {
            let mut r = Reader::default();
            let (mut got, mut ended) = (Vec::new(), false);
            for chunk in b.chunks(step) {
                r.feed(chunk, |rec| {
                    match rec.t {
                        STDOUT => got.extend_from_slice(rec.content),
                        END_REQUEST => ended = true,
                        _ => {}
                    }
                    true
                })
                .unwrap();
            }
            assert!(got == big && ended, "step {step}");
            assert!(r.is_empty());
        }
        let mut r = Reader::default();
        assert_eq!(
            r.feed(&[2, 6, 0, 1, 0, 0, 0, 0], |_| true),
            Err(Protocol),
            "version 2"
        );
        let mut p = Vec::new();
        append_param(&mut p, b"N", &[b'v'; 200]);
        assert_eq!(p.len(), 1 + 4 + 1 + 200);
        assert_ne!(p[1] & 0x80, 0, "a long value's length: {:x?}", &p[..6]);
    }

    #[test]
    fn stops_where_told() {
        let mut b = Vec::new();
        append_record(&mut b, STDOUT, b"a");
        append_record(&mut b, END_REQUEST, &[0; 8]);
        append_record(&mut b, STDOUT, b"late");
        let mut r = Reader::default();
        let mut n = 0;
        let left = r
            .feed(&b, |rec| {
                n += 1;
                rec.t != END_REQUEST
            })
            .unwrap();
        assert_eq!(n, 2);
        assert!(left);
    }
}
