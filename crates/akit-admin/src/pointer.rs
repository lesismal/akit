//! JSON pointers (RFC 6901) into a config, as fit's Admin API takes them after `/config`.

use akit_config::value::Value;

/// A request the API refuses: its status and message.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Problem {
    pub(crate) status: u16,
    pub(crate) msg: String,
}

impl Problem {
    pub(crate) fn new(status: u16, msg: String) -> Problem {
        Problem { status, msg }
    }
}

/// The reference tokens of pointer `p`, `""` or starting with `/`.
pub(crate) fn parse(p: &str) -> Result<Vec<String>, Problem> {
    if p.is_empty() {
        return Ok(Vec::new());
    }
    let Some(rest) = p.strip_prefix('/') else {
        return Err(Problem::new(
            400,
            format!("{p:?} is not a JSON pointer: it must start with '/'"),
        ));
    };
    rest.split('/')
        .map(|t| {
            let mut out = String::with_capacity(t.len());
            let mut chars = t.chars();
            while let Some(c) = chars.next() {
                if c != '~' {
                    out.push(c);
                    continue;
                }
                match chars.next() {
                    Some('0') => out.push('~'),
                    Some('1') => out.push('/'),
                    _ => {
                        return Err(Problem::new(
                            400,
                            format!("{p:?} is not a JSON pointer: '~' must be followed by 0 or 1"),
                        ));
                    }
                }
            }
            Ok(out)
        })
        .collect()
}

/// `tokens` written back as a JSON pointer, for messages.
fn string(tokens: &[String]) -> String {
    let mut b = String::new();
    for t in tokens {
        b.push('/');
        b.push_str(&t.replace('~', "~0").replace('/', "~1"));
    }
    b
}

/// The value `tokens` point to in `root`, or 404 with fit's message.
pub(crate) fn lookup<'v>(root: &'v Value, tokens: &[String]) -> Result<&'v Value, Problem> {
    let mut v = root;
    for (i, t) in tokens.iter().enumerate() {
        let upto = tokens.get(..=i).unwrap_or(tokens);
        v = match v {
            Value::Object(_) => v.get(t).ok_or_else(|| {
                Problem::new(404, format!("{}: there is no member {t:?}", string(upto)))
            })?,
            Value::Array(a) => {
                let not_index = || {
                    Problem::new(
                        404,
                        format!("{}: {t:?} is not an array index", string(upto)),
                    )
                };
                if t.is_empty()
                    || !t.bytes().all(|b| b.is_ascii_digit())
                    || (t.len() > 1 && t.starts_with('0'))
                {
                    return Err(not_index());
                }
                let j: usize = t.parse().map_err(|_| not_index())?;
                a.get(j).ok_or_else(|| {
                    Problem::new(
                        404,
                        format!(
                            "{}: index {j} is past the end of an array of {}",
                            string(upto),
                            a.len()
                        ),
                    )
                })?
            }
            _ => {
                return Err(Problem::new(
                    404,
                    format!(
                        "{}: {} is neither an object nor an array",
                        string(upto),
                        string(tokens.get(..i).unwrap_or_default())
                    ),
                ));
            }
        };
    }
    Ok(v)
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]

    use super::{lookup, parse};
    use akit_config::value::{self, Value};

    #[test]
    fn pointers() {
        assert_eq!(parse("").unwrap(), Vec::<String>::new());
        assert_eq!(parse("/a~1b/~0/0").unwrap(), ["a/b", "~", "0"]);
        assert_eq!(parse("/").unwrap(), [""]);
        assert_eq!(parse("a").unwrap_err().status, 400);
        assert!(
            parse("/a~2")
                .unwrap_err()
                .msg
                .contains("'~' must be followed")
        );

        let root = value::parse(r#"{"a/b": [10, {"c": true}], "n": 1}"#).unwrap();
        let get = |p: &str| lookup(&root, &parse(p).unwrap());
        assert_eq!(get("/a~1b/1/c").unwrap(), &Value::Bool(true));
        assert_eq!(get("").unwrap(), &root);
        for (p, want) in [
            ("/x", r#"/x: there is no member "x""#),
            (
                "/a~1b/2",
                "/a~1b/2: index 2 is past the end of an array of 2",
            ),
            ("/a~1b/01", r#"/a~1b/01: "01" is not an array index"#),
            ("/a~1b/-", r#"/a~1b/-: "-" is not an array index"#),
            ("/n/x", "/n/x: /n is neither an object nor an array"),
        ] {
            let e = get(p).unwrap_err();
            assert_eq!((e.status, e.msg.as_str()), (404, want), "{p}");
        }
    }
}
