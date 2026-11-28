//! A streamed body cut into events, fit's `framer`: server-sent events, each its event name and
//! data, or NDJSON's lines, each a JSON value with no name. A line, and an event's data, may be
//! at most `max` bytes; what waits for the rest of a line is the only part of the body kept
//! between pieces.

use super::ErrorKind;

/// Takes each event: its name and its data, valid for the call only.
pub(crate) type Handle<'h> = dyn FnMut(&str, &[u8]) -> Result<(), ErrorKind> + 'h;

#[derive(Debug, Default)]
pub(crate) struct Framer {
    pub(crate) ndjson: bool,
    pub(crate) max: usize,
    /// A line not ended yet.
    line: Vec<u8>,
    /// The SSE event's name.
    name: String,
    /// The SSE event's data so far.
    data: Vec<u8>,
    has_data: bool,
}

fn trim_cr(l: &[u8]) -> &[u8] {
    l.strip_suffix(b"\r").unwrap_or(l)
}

impl Framer {
    pub(crate) fn new(ndjson: bool, max: usize) -> Framer {
        Framer {
            ndjson,
            max,
            ..Framer::default()
        }
    }

    /// Takes a piece of the body, which it does not keep but for a line not ended.
    pub(crate) fn write(&mut self, mut p: &[u8], h: &mut Handle<'_>) -> Result<(), ErrorKind> {
        while !p.is_empty() {
            let Some(i) = p.iter().position(|b| *b == b'\n') else {
                if self.line.len().saturating_add(p.len()) > self.max {
                    return Err(ErrorKind::TooLarge);
                }
                self.line.extend_from_slice(p);
                return Ok(());
            };
            let (line, rest) = p.split_at(i);
            p = rest.get(1..).unwrap_or(&[]);
            if self.line.len().saturating_add(line.len()) > self.max {
                return Err(ErrorKind::TooLarge);
            }
            let r = if self.line.is_empty() {
                self.line_done(trim_cr(line), h)
            } else {
                let mut whole = std::mem::take(&mut self.line);
                whole.extend_from_slice(line);
                let r = self.line_done(trim_cr(&whole), h);
                whole.clear();
                self.line = whole;
                r
            };
            r?;
        }
        Ok(())
    }

    /// Takes the end of the body: a last line without its newline, and an event without its
    /// blank line, count as ended.
    pub(crate) fn end(&mut self, h: &mut Handle<'_>) -> Result<(), ErrorKind> {
        if !self.line.is_empty() {
            let line = std::mem::take(&mut self.line);
            self.line_done(trim_cr(&line), h)?;
        }
        if self.ndjson {
            Ok(())
        } else {
            self.dispatch(h)
        }
    }

    fn line_done(&mut self, line: &[u8], h: &mut Handle<'_>) -> Result<(), ErrorKind> {
        if self.ndjson {
            if line.iter().all(u8::is_ascii_whitespace) {
                return Ok(());
            }
            return h("", line);
        }
        if line.is_empty() {
            return self.dispatch(h);
        }
        if line.first() == Some(&b':') {
            // A comment, as a keep-alive.
            return Ok(());
        }
        let (field, value) = match line.iter().position(|b| *b == b':') {
            Some(i) => {
                let (f, v) = line.split_at(i);
                let v = v.get(1..).unwrap_or(&[]);
                (f, v.strip_prefix(b" ").unwrap_or(v))
            }
            None => (line, &[][..]),
        };
        match field {
            b"event" => self.name = String::from_utf8_lossy(value).into_owned(),
            b"data" => {
                // A line is within max already; the lines of an event together must be too.
                if self.has_data {
                    if self
                        .data
                        .len()
                        .saturating_add(1)
                        .saturating_add(value.len())
                        > self.max
                    {
                        return Err(ErrorKind::TooLarge);
                    }
                    self.data.push(b'\n');
                }
                self.data.extend_from_slice(value);
                self.has_data = true;
            }
            _ => {}
        }
        Ok(())
    }

    /// Hands on the event the lines so far made, if it has data.
    fn dispatch(&mut self, h: &mut Handle<'_>) -> Result<(), ErrorKind> {
        let name = std::mem::take(&mut self.name);
        let has = std::mem::replace(&mut self.has_data, false);
        if !has {
            self.data.clear();
            return Ok(());
        }
        let data = std::mem::take(&mut self.data);
        let r = h(&name, &data);
        let mut data = data;
        data.clear();
        self.data = data;
        r
    }
}
