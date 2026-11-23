#![no_main]
//! akit-config: reading a YAML config never panics, whatever the bytes, and stays within its
//! bounds on aliases and merge keys; a config that reads is read the same way again.

libfuzzer_sys::fuzz_target!(|data: &[u8]| {
    let Ok(src) = std::str::from_utf8(data) else {
        return;
    };
    let first = akit_config::from_yaml(src, None);
    let again = akit_config::from_yaml(src, None);
    assert_eq!(first, again);
});
