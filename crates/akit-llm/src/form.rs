//! fit's `model/form.go` and the body rewrites of `node/proxy.go`: the model a call names, in a
//! JSON body or a `multipart/form-data` one (OpenAI's uploads), read and replaced in place.

use akit_config::value::Value;

use crate::json;

/// The longest boundary RFC 2046 allows.
pub const MAX_BOUNDARY: usize = 70;

/// A media type's type, lower case, and its parameters (Go's `mime.ParseMediaType`, enough of
/// it): `None` when it is not one.
#[must_use]
pub fn media_type(ct: &str) -> Option<(String, Vec<(String, String)>)> {
    let mut parts = ct.split(';');
    let mt = parts.next()?.trim().to_ascii_lowercase();
    if mt.is_empty() || !mt.contains('/') {
        return None;
    }
    let mut params = Vec::new();
    for p in parts {
        let p = p.trim();
        if p.is_empty() {
            continue;
        }
        let (k, v) = p.split_once('=')?;
        let v = v.trim();
        let v = v
            .strip_prefix('"')
            .and_then(|v| v.strip_suffix('"'))
            .unwrap_or(v);
        params.push((k.trim().to_ascii_lowercase(), v.to_owned()));
    }
    Some((mt, params))
}

/// The boundary of a `multipart/form-data` media type, `None` when `ct` is not one or its
/// boundary is missing or too long.
#[must_use]
pub fn form_boundary(ct: &str) -> Option<String> {
    let (mt, params) = media_type(ct)?;
    if mt != "multipart/form-data" {
        return None;
    }
    let b = params.into_iter().find(|(k, _)| k == "boundary")?.1;
    (!b.is_empty() && b.len() <= MAX_BOUNDARY).then_some(b)
}

fn find(hay: &[u8], needle: &[u8], from: usize) -> Option<usize> {
    if needle.is_empty() {
        return Some(from);
    }
    hay.get(from..)?
        .windows(needle.len())
        .position(|w| w == needle)
        .map(|i| i.saturating_add(from))
}

/// Where the value of the first part named `name` of a `multipart/form-data` body is:
/// `body[start..end]`.
///
/// # Errors
///
/// A body that is not one of its boundary, or has no such field.
pub fn form_field(body: &[u8], boundary: &str, name: &str) -> Result<(usize, usize), String> {
    let dash = format!("--{boundary}").into_bytes();
    let mut delim = b"\r\n".to_vec();
    delim.extend_from_slice(&dash);
    let i = find(body, &dash, 0)
        .filter(|&i| i == 0 || (i >= 2 && body.get(i.saturating_sub(2)..i) == Some(b"\r\n")))
        .ok_or("the body is not multipart/form-data of its boundary")?;
    let mut pos = i.saturating_add(dash.len());
    loop {
        while matches!(body.get(pos), Some(b' ' | b'\t')) {
            pos = pos.saturating_add(1);
        }
        let rest = body.get(pos..).unwrap_or(&[]);
        if rest.starts_with(b"--") {
            return Err(format!("no {name} field in the form"));
        }
        if !rest.starts_with(b"\r\n") {
            return Err("a malformed multipart boundary line".to_owned());
        }
        let hdr_start = pos.saturating_add(2);
        let hdr_end = find(body, b"\r\n\r\n", hdr_start)
            .ok_or("a multipart part without the end of its header")?;
        let content_start = hdr_end.saturating_add(4);
        let content_end = next_delim(body, content_start, &delim)
            .ok_or("a multipart part without its closing boundary")?;
        let hdr = body.get(hdr_start..hdr_end).unwrap_or(&[]);
        if part_name(hdr).as_deref() == Some(name) {
            return Ok((content_start, content_end));
        }
        pos = content_end.saturating_add(delim.len());
    }
}

fn next_delim(body: &[u8], mut from: usize, delim: &[u8]) -> Option<usize> {
    loop {
        let at = find(body, delim, from)?;
        let mut rest = body.get(at.saturating_add(delim.len())..).unwrap_or(&[]);
        while let Some((b' ' | b'\t', r)) = rest.split_first() {
            rest = r;
        }
        if rest.starts_with(b"\r\n") || rest.starts_with(b"--") {
            return Some(at);
        }
        from = at.saturating_add(1);
    }
}

fn part_name(hdr: &[u8]) -> Option<String> {
    let text = String::from_utf8_lossy(hdr);
    for line in text.split("\r\n") {
        let Some((k, v)) = line.split_once(':') else {
            continue;
        };
        if !k.trim().eq_ignore_ascii_case("content-disposition") {
            continue;
        }
        let (disp, params) = media_type(&format!("x/{}", v.trim()))?;
        if disp != "x/form-data" {
            return None;
        }
        return params.into_iter().find(|(k, _)| k == "name").map(|p| p.1);
    }
    None
}

/// Sets the top-level `model` of a JSON object body to `upstream`; the other members keep their
/// values.
///
/// # Errors
///
/// A body that is not a JSON object.
pub fn rewrite_model(body: &[u8], upstream: &str) -> Result<Vec<u8>, String> {
    let mut v =
        json::parse(body).map_err(|_| "the request body must be a JSON object".to_owned())?;
    let Value::Object(ms) = &mut v else {
        return Err("the request body must be a JSON object".to_owned());
    };
    set_member(ms, "model", Value::String(upstream.to_owned()));
    Ok(json::write(&v).into_bytes())
}

fn set_member(ms: &mut Vec<(String, Value)>, k: &str, v: Value) {
    match ms.iter_mut().find(|(n, _)| n == k) {
        Some((_, x)) => *x = v,
        None => ms.push((k.to_owned(), v)),
    }
}

/// Sets the `model` of every request of a Gemini `batchEmbedContents` body to
/// `models/<upstream>`.
///
/// # Errors
///
/// A body that is not a JSON object with requests.
pub fn rewrite_batch_models(body: &[u8], upstream: &str) -> Result<Vec<u8>, String> {
    let bad = || "the request body must be a JSON object with requests".to_owned();
    let mut v = json::parse(body).map_err(|_| bad())?;
    let Value::Object(ms) = &mut v else {
        return Err(bad());
    };
    let Some((_, Value::Array(reqs))) = ms.iter_mut().find(|(k, _)| k == "requests") else {
        return Err(bad());
    };
    for r in reqs.iter_mut() {
        if let Value::Object(f) = r {
            set_member(f, "model", Value::String(format!("models/{upstream}")));
        }
    }
    Ok(json::write(&v).into_bytes())
}

/// Sets the `model` field of a `multipart/form-data` body to `upstream`; every other byte, the
/// files included, stays as it is.
///
/// # Errors
///
/// A body without a model field.
pub fn rewrite_form(body: &[u8], boundary: &str, upstream: &str) -> Result<Vec<u8>, String> {
    let (start, end) = form_field(body, boundary, "model")?;
    let mut out = Vec::with_capacity(body.len().saturating_add(upstream.len()));
    out.extend_from_slice(body.get(..start).unwrap_or(&[]));
    out.extend_from_slice(upstream.as_bytes());
    out.extend_from_slice(body.get(end..).unwrap_or(&[]));
    Ok(out)
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]
    use super::*;

    const FORM: &[u8] = b"--XX\r\nContent-Disposition: form-data; name=\"file\"; filename=\"a.wav\"\r\n\r\nRIFF--XX-not\r\n--XX\r\nContent-Disposition: form-data; name=\"model\"\r\n\r\nwhisper\r\n--XX--\r\n";

    #[test]
    fn forms() {
        assert_eq!(
            form_boundary("multipart/form-data; boundary=XX").as_deref(),
            Some("XX")
        );
        assert_eq!(
            form_boundary("Multipart/Form-Data; boundary=\"a b\"").as_deref(),
            Some("a b")
        );
        assert!(form_boundary("application/json").is_none());
        assert!(
            form_boundary(&format!("multipart/form-data; boundary={}", "x".repeat(71))).is_none()
        );
        let (s, e) = form_field(FORM, "XX", "model").unwrap();
        assert_eq!(&FORM[s..e], b"whisper");
        assert!(form_field(FORM, "XX", "nope").is_err());
        assert!(form_field(b"junk", "XX", "model").is_err());
        let out = rewrite_form(FORM, "XX", "whisper-1").unwrap();
        let (s, e) = form_field(&out, "XX", "model").unwrap();
        assert_eq!(&out[s..e], b"whisper-1");
        assert!(out.starts_with(&FORM[..60]));
    }

    #[test]
    fn json_bodies() {
        let b = rewrite_model(br#"{"model":"a","stream":true,"n":1.50}"#, "b").unwrap();
        assert_eq!(b, br#"{"model":"b","stream":true,"n":1.50}"#);
        assert!(rewrite_model(b"[1]", "b").is_err());
        let b = rewrite_batch_models(br#"{"requests":[{"model":"models/a","x":1}]}"#, "b").unwrap();
        assert_eq!(b, br#"{"requests":[{"model":"models/b","x":1}]}"#);
    }
}
