#![no_main]
//! `akit_router::fuzzing::router_find`: what it checks is said there.

libfuzzer_sys::fuzz_target!(|data: &[u8]| akit_router::fuzzing::router_find(data));
