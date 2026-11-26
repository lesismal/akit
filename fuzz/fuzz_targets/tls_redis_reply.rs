#![no_main]
//! `akit_tls::fuzzing::tls_redis_reply`: what it checks is said there.

libfuzzer_sys::fuzz_target!(|data: &[u8]| akit_tls::fuzzing::tls_redis_reply(data));
