#![no_main]
use libfuzzer_sys::fuzz_target;

#[path = "../common.rs"]
mod common;

// data = the payload of a `ipco` box (FullBox version and flags included).
fuzz_target!(|data: &[u8]| {
    let _ = isobmff::__fuzz::ipco(data, &common::small_limits());
});
