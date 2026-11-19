#![no_main]
use libfuzzer_sys::fuzz_target;

#[path = "../common.rs"]
mod common;

// data = the payload of a `iinf` box (FullBox version and flags included).
fuzz_target!(|data: &[u8]| {
    let _ = isobmff::__fuzz::iinf(data, &common::small_limits());
});
