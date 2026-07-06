//! Decoding an arbitrary CVSP batch stream must never panic or OOM.
#![no_main]
use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    let mut reader = data;
    let _ = cavs_proto::decode_stream(&mut reader, |_item| Ok(()));
});
