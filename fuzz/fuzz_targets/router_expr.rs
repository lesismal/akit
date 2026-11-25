#![no_main]
//! `akit_router::fuzzing::router_expr`: what it checks is said there.

libfuzzer_sys::fuzz_target!(|data: &[u8]| akit_router::fuzzing::router_expr(data));
