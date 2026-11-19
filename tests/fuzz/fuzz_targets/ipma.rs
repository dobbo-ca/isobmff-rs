#![no_main]
use libfuzzer_sys::fuzz_target;

#[path = "../common.rs"]
mod common;

// data = the payload of a `ipma` box (FullBox version and flags included).
fuzz_target!(|data: &[u8]| {
    let _ = isobmff::__fuzz::ipma(data, &common::small_limits());
});
