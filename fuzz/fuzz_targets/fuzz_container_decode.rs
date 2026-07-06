//! Opening + fully verifying an arbitrary .cavs must never panic.
#![no_main]
use libfuzzer_sys::fuzz_target;
use std::io::Write as _;

fuzz_target!(|data: &[u8]| {
    let mut tmp = tempfile::NamedTempFile::new().unwrap();
    tmp.write_all(data).unwrap();
    if let Ok(mut reader) = cavs_format::Reader::open(tmp.path()) {
        let _ = reader.verify();
    }
});
