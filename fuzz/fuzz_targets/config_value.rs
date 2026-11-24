#![no_main]
//! akit-config's `value` (the Admin API's JSON, I26): standardizing never panics, keeps the
//! length and yields what parses to the same value; a value written out parses back to itself.

libfuzzer_sys::fuzz_target!(|data: &[u8]| {
    let Ok(src) = std::str::from_utf8(data) else {
        return;
    };
    let Ok(v) = akit_config::value::parse(src) else {
        assert!(akit_config::value::standardize(src).is_err());
        return;
    };
    let std = akit_config::value::standardize(src).unwrap();
    assert_eq!(std.len(), src.len());
    assert_eq!(akit_config::value::parse(&std).unwrap(), v);
    assert_eq!(akit_config::value::parse(&v.to_json()).unwrap(), v);
});
