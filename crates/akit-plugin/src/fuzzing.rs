//! Fuzz entry points (feature `fuzzing`; not a public API): the `fuzz/` cargo-fuzz project calls
//! them, and the tests below replay `fuzz/corpus/<target>` on stable. What a plugin hands the
//! host: Proxy-Wasm's header maps, serialized in the guest's memory, and the field names it
//! sets. The WebAssembly runtime itself is akio-wasm's, fuzzed in akio (`wasm_decode`,
//! `wasm_exec`, `wasm_diff`).
#![allow(
    clippy::indexing_slicing,
    clippy::arithmetic_side_effects,
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic
)]

use crate::abi::{MAX_OPS, canonical, is_tchar};
use crate::proxywasm::{deserialize_pairs, serialize_pairs, window};

/// `plugin_pairs`: `data[0..8]` a window's start and size; the rest a header map as a guest
/// serializes it. A map that reads holds at most [`MAX_OPS`] pairs and serializes back to what
/// reads the same; the window is a part of the data; canonical names stay tokens.
pub fn plugin_pairs(data: &[u8]) {
    if data.len() < 8 {
        return;
    }
    let start = u32::from_le_bytes(data[..4].try_into().unwrap());
    let size = u32::from_le_bytes(data[4..8].try_into().unwrap());
    let b = &data[8..];
    if let Some(w) = window(b, start, size) {
        assert!(w.len() <= b.len() && w.len() as u64 <= u64::from(size));
    } else {
        assert!(start as usize > b.len());
    }
    let Some(pairs) = deserialize_pairs(b) else {
        return;
    };
    assert!(pairs.len() <= MAX_OPS);
    let again = serialize_pairs(&pairs);
    assert_eq!(deserialize_pairs(&again).as_ref(), Some(&pairs));
    for (k, _) in &pairs {
        if !k.is_empty() && k.bytes().all(is_tchar) {
            let c = canonical(k);
            assert!(c.bytes().all(is_tchar) && c.eq_ignore_ascii_case(k));
            assert_eq!(canonical(&c), c);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Runs `f` on every file of `fuzz/corpus/<target>`.
    fn replay(target: &str, f: fn(&[u8])) {
        let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../fuzz/corpus")
            .join(target);
        let mut n = 0;
        for e in std::fs::read_dir(&dir).unwrap() {
            f(&std::fs::read(e.unwrap().path()).unwrap());
            n += 1;
        }
        assert!(n > 0, "{} is empty", dir.display());
    }

    #[test]
    fn corpus_plugin_pairs() {
        replay("plugin_pairs", plugin_pairs);
    }
}
