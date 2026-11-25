#![no_main]
//! `akit_l4::fuzzing::l4_hello`: what it checks is said there.

libfuzzer_sys::fuzz_target!(|data: &[u8]| akit_l4::fuzzing::l4_hello(data));
