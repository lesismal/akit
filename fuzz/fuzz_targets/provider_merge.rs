#![no_main]
//! `akit_provider::fuzzing::provider_merge`: what it checks is said there.

libfuzzer_sys::fuzz_target!(|data: &[u8]| akit_provider::fuzzing::provider_merge(data));
