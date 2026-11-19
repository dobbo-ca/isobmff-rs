#![no_main]
use libfuzzer_sys::fuzz_target;

#[path = "../common.rs"]
mod common;

// data = the payload of a `grid` item, and also of an `iovl` item.
fuzz_target!(|data: &[u8]| {
    let lim = common::small_limits();
    let _ = isobmff::__fuzz::grid(data, &lim);
    let _ = isobmff::__fuzz::iovl(data, &lim);
});
