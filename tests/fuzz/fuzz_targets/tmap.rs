#![no_main]
use libfuzzer_sys::fuzz_target;

#[path = "../common.rs"]
mod common;

// data = the payload of a `tmap` box (FullBox version and flags included).
fuzz_target!(|data: &[u8]| {
    let _ = isobmff::__fuzz::tmap(data, &common::small_limits());
});
