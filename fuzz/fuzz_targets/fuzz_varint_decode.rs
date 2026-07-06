//! Strict LEB128 decoding must reject anything malformed without panicking.
#![no_main]
use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    let mut slice = data;
    while cavs_manifest::varint::read_varuint(&mut slice).is_ok() && !slice.is_empty() {}
});
