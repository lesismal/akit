#![no_main]
//! `akit_provider::fuzzing::provider_docker`: what it checks is said there.

libfuzzer_sys::fuzz_target!(|data: &[u8]| akit_provider::fuzzing::provider_docker(data));
