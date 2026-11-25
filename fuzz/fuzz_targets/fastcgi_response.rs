#![no_main]
//! `akit_handler::fuzzing::fastcgi_response`: what it checks is said there.

libfuzzer_sys::fuzz_target!(|data: &[u8]| akit_handler::fuzzing::fastcgi_response(data));
