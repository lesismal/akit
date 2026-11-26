#![no_main]
//! `akit_proxy::fuzzing::proxy_grpc_timeout`: what it checks is said there.

libfuzzer_sys::fuzz_target!(|data: &[u8]| akit_proxy::fuzzing::proxy_grpc_timeout(data));
