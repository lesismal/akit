#![no_main]
//! akit-config: reading a JSON config never panics, whatever the bytes; a config that reads is
//! read the same way again from its own source.

libfuzzer_sys::fuzz_target!(|data: &[u8]| {
    let Ok(src) = std::str::from_utf8(data) else {
        return;
    };
    let first = akit_config::from_json(src, None);
    let again = akit_config::from_json(src, None);
    assert_eq!(first, again);
});
