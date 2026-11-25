#![no_main]
//! `akit_config::fuzzing::config_adapt`: what it checks is said there.

libfuzzer_sys::fuzz_target!(|data: &[u8]| akit_config::fuzzing::config_adapt(data));
