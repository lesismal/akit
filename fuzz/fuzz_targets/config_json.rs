#![no_main]
//! `akit_config::fuzzing::config_json`: what it checks is said there.

libfuzzer_sys::fuzz_target!(|data: &[u8]| akit_config::fuzzing::config_json(data));
