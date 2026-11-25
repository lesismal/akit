#![no_main]
//! `akit_provider::fuzzing::provider_consul`: what it checks is said there.

libfuzzer_sys::fuzz_target!(|data: &[u8]| akit_provider::fuzzing::provider_consul(data));
