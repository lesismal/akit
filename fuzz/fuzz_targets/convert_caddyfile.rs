#![no_main]
//! `akit_config::fuzzing::convert_caddyfile`: what it checks is said there.

libfuzzer_sys::fuzz_target!(|data: &[u8]| akit_config::fuzzing::convert_caddyfile(data));
