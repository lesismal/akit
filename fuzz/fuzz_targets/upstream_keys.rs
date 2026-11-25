#![no_main]
//! `akit_upstream::fuzzing::upstream_keys`: what it checks is said there.

libfuzzer_sys::fuzz_target!(|data: &[u8]| akit_upstream::fuzzing::upstream_keys(data));
