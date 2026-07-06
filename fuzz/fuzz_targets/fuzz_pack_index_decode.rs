//! Reading an arbitrary .cavsindex must never panic or over-allocate.
#![no_main]
use libfuzzer_sys::fuzz_target;
use std::io::Write as _;

fuzz_target!(|data: &[u8]| {
    let mut tmp = tempfile::NamedTempFile::new().unwrap();
    tmp.write_all(data).unwrap();
    let _ = cavs_store::packfile::read_pack_index(tmp.path());
});
