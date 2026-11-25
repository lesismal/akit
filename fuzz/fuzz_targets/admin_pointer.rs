#![no_main]
//! `akit_admin::fuzzing::admin_pointer`: what it checks is said there.

libfuzzer_sys::fuzz_target!(|data: &[u8]| akit_admin::fuzzing::admin_pointer(data));
