#![no_main]
//! `akit_observe::fuzzing::observe_escapes`: what it checks is said there.

libfuzzer_sys::fuzz_target!(|data: &[u8]| akit_observe::fuzzing::observe_escapes(data));
