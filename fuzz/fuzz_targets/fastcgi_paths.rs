#![no_main]
//! `akit_handler::fuzzing::fastcgi_paths`: what it checks is said there.

libfuzzer_sys::fuzz_target!(|data: &[u8]| akit_handler::fuzzing::fastcgi_paths(data));
