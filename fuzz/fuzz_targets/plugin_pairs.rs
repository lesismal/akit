#![no_main]
//! `akit_plugin::fuzzing::plugin_pairs`: what it checks is said there.

libfuzzer_sys::fuzz_target!(|data: &[u8]| akit_plugin::fuzzing::plugin_pairs(data));
