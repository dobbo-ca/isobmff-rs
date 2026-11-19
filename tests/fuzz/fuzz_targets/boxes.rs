#![no_main]
use libfuzzer_sys::fuzz_target;

#[path = "../common.rs"]
mod common;

// data = a whole box tree (the file as is).
fuzz_target!(|data: &[u8]| {
    let _ = isobmff::__fuzz::boxes(data, &common::small_limits());
});
