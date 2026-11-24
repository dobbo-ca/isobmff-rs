#![no_main]
use libfuzzer_sys::fuzz_target;

#[path = "../common.rs"]
mod common;

// data = the whole file; parse_movie, then every walk over every track (all samples,
// sync_samples, sync_sample_at).
fuzz_target!(|data: &[u8]| {
    let _ = isobmff::__fuzz::movie(data, &common::small_limits());
});
