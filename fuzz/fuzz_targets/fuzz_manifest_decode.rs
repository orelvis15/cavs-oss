//! Decoding an arbitrary manifest must never panic, OOM or loop.
#![no_main]
use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    let _ = cavs_manifest::read_manifest(data);
});
