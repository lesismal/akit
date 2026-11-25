#![no_main]
//! `akit_cache::fuzzing::cache_semantics`: what it checks is said there.

libfuzzer_sys::fuzz_target!(|data: &[u8]| akit_cache::fuzzing::cache_semantics(data));
