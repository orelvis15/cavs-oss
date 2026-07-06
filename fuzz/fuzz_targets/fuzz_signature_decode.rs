//! Decoding an arbitrary .cavssig must never panic, OOM or loop; a decoded
//! signature must re-encode to the exact same bytes (canonical form).
#![no_main]
use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    if let Ok(sig) = cavs_signature::CavsSignature::decode(data) {
        assert_eq!(sig.encode(), data, "decode/encode must be canonical");
    }
});
