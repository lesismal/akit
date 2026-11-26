#![no_main]
//! `akit_handler::fuzzing::static_path`: what it checks is said there.

libfuzzer_sys::fuzz_target!(|data: &[u8]| akit_handler::fuzzing::static_path(data));
