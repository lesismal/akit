#![no_main]
//! `akit_observe::fuzzing::observe_tzif`: what it checks is said there.

libfuzzer_sys::fuzz_target!(|data: &[u8]| akit_observe::fuzzing::observe_tzif(data));
