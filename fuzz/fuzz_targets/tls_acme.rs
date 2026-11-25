#![no_main]
//! `akit_tls::fuzzing::tls_acme`: what it checks is said there.

libfuzzer_sys::fuzz_target!(|data: &[u8]| akit_tls::fuzzing::tls_acme(data));
