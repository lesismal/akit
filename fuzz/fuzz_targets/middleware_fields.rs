#![no_main]
//! `akit_middleware::fuzzing::middleware_fields`: what it checks is said there.

libfuzzer_sys::fuzz_target!(|data: &[u8]| akit_middleware::fuzzing::middleware_fields(data));
