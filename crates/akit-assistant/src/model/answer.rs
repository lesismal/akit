//! An answer gathered as its events are parsed, fit's `answer`: each piece handed on, the tool
//! calls assembled and bounded.

use std::collections::HashMap;

use super::{Answer as Res, ErrorKind, Event, ToolCall};

/// Hands an event on; false once the call has ended, and the parse stops.
pub(crate) type Emit = dyn FnMut(Event) -> bool + Send;

/// The parse stopped because the call ended: its cause ends it.
pub(crate) const STOPPED: &str = "the call has ended";

pub(crate) struct Gather {
    pub(crate) res: Res,
    pub(crate) emit: Box<Emit>,
    pub(crate) max_tools: usize,
    /// Whether the API has said the answer is complete.
    pub(crate) done: bool,
    /// The tool calls whose arguments are still arriving, by the key the API's parts name them
    /// with, in the order they began.
    partial: HashMap<String, Partial>,
    order: Vec<String>,
}

#[derive(Default)]
pub(crate) struct Partial {
    id: String,
    name: String,
    pub(crate) args: String,
}

fn stopped() -> ErrorKind {
    ErrorKind::Other(STOPPED.to_owned())
}

impl Gather {
    pub(crate) fn new(res: Res, emit: Box<Emit>, max_tools: usize) -> Gather {
        Gather {
            res,
            emit,
            max_tools,
            done: false,
            partial: HashMap::new(),
            order: Vec::new(),
        }
    }

    pub(crate) fn text(&mut self, s: &str) -> Result<(), ErrorKind> {
        if s.is_empty() {
            return Ok(());
        }
        self.res.text.push_str(s);
        self.send(Event::Text(s.to_owned()))
    }

    pub(crate) fn thinking(&mut self, s: &str) -> Result<(), ErrorKind> {
        if s.is_empty() {
            return Ok(());
        }
        self.res.thinking.push_str(s);
        self.send(Event::Thinking(s.to_owned()))
    }

    fn send(&mut self, e: Event) -> Result<(), ErrorKind> {
        if (self.emit)(e) {
            Ok(())
        } else {
            Err(stopped())
        }
    }

    /// Starts the tool call `key`, or adds to it: `id` and `name` when the API gives them.
    pub(crate) fn begin(
        &mut self,
        key: &str,
        id: &str,
        name: &str,
    ) -> Result<&mut Partial, ErrorKind> {
        if !self.partial.contains_key(key) {
            if self.res.tool_calls.len().saturating_add(self.partial.len()) >= self.max_tools {
                return Err(ErrorKind::TooLarge);
            }
            self.partial.insert(key.to_owned(), Partial::default());
            self.order.push(key.to_owned());
        }
        let pc = self.partial.get_mut(key).ok_or(ErrorKind::TooLarge)?;
        if !id.is_empty() {
            id.clone_into(&mut pc.id);
        }
        if !name.is_empty() {
            name.clone_into(&mut pc.name);
        }
        Ok(pc)
    }

    /// Finishes the tool call `key`, if it has begun, and hands it on.
    pub(crate) fn end(&mut self, key: &str) -> Result<(), ErrorKind> {
        let Some(pc) = self.partial.remove(key) else {
            return Ok(());
        };
        self.order.retain(|k| k != key);
        self.call(ToolCall {
            id: pc.id,
            name: pc.name,
            arguments: pc.args,
            invalid: false,
            signature: String::new(),
        })
    }

    /// Finishes every tool call that has begun, in order.
    pub(crate) fn end_all(&mut self) -> Result<(), ErrorKind> {
        while let Some(k) = self.order.first().cloned() {
            self.end(&k)?;
        }
        Ok(())
    }

    /// Hands on a whole tool call, its arguments made a JSON object: none is `{}`, and what is
    /// not one becomes a string, marked invalid.
    pub(crate) fn call(&mut self, mut tc: ToolCall) -> Result<(), ErrorKind> {
        if self.res.tool_calls.len() >= self.max_tools {
            return Err(ErrorKind::TooLarge);
        }
        if tc.id.is_empty() {
            tc.id = format!("call_{}", self.res.tool_calls.len().saturating_add(1));
        }
        let args = tc.arguments.trim();
        if args.is_empty() || args == "null" {
            tc.arguments = "{}".to_owned();
        } else if args.starts_with('{') && strict_json(args) {
            tc.arguments = args.to_owned();
        } else {
            tc.arguments = crate::json::quoted(args);
            tc.invalid = true;
        }
        self.res.tool_calls.push(tc.clone());
        self.send(Event::ToolCall(tc))
    }
}

/// Whether `s` is JSON as Go's `json.Valid` takes it: akit's reader, which also takes comments
/// and trailing commas, and none of them (a `//` in a string, as a URL's, is not a comment).
pub(crate) fn strict_json(s: &str) -> bool {
    super::json_value(s).is_some() && !lenient(s)
}

/// Whether `s` has, outside its strings, a comment or a trailing comma.
fn lenient(s: &str) -> bool {
    let mut in_str = false;
    let mut esc = false;
    let mut last = b' ';
    for b in s.bytes() {
        if in_str {
            if esc {
                esc = false;
            } else if b == b'\\' {
                esc = true;
            } else if b == b'"' {
                in_str = false;
            }
            last = b'"';
            continue;
        }
        match b {
            b'"' => in_str = true,
            b'}' | b']' if last == b',' => return true,
            b'/' | b'*' if last == b'/' => return true,
            _ => {}
        }
        if !b.is_ascii_whitespace() {
            last = b;
        }
    }
    false
}
