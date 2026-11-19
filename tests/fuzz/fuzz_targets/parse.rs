#![no_main]
use libfuzzer_sys::fuzz_target;

#[path = "../common.rs"]
mod common;

// data = the whole file; parse, then every graph accessor and metadata().
fuzz_target!(|data: &[u8]| {
    let _ = common::parse_all(data);
});
