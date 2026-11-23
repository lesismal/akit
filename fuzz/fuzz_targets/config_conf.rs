#![no_main]
//! akit-config: reading a `.conf` config never panics, whatever the bytes (the lexer, the parser,
//! `def` and `use`, sites and the conversion by fit's field types, then the decoder); a config
//! that reads is read the same way again from its own source.

libfuzzer_sys::fuzz_target!(|data: &[u8]| {
    let Ok(src) = std::str::from_utf8(data) else {
        return;
    };
    let first = akit_config::from_conf(src, None);
    let again = akit_config::from_conf(src, None);
    assert_eq!(first, again);
});
