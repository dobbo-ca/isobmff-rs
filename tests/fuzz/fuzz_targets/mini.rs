#![no_main]
use libfuzzer_sys::fuzz_target;

#[path = "../common.rs"]
mod common;

// data = the payload of a `mini` box (no box header); the `ftyp` minor version is `avif` for an
// even length and `heic` for an odd one.
fuzz_target!(|data: &[u8]| {
    let _ = isobmff::__fuzz::mini(data, &common::small_limits());
});
