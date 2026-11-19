#![no_main]
use libfuzzer_sys::fuzz_target;
use std::io::Cursor;
use std::sync::atomic::AtomicBool;

#[path = "../common.rs"]
mod common;

// data = the whole file; read over a Cursor gives the same result as parse.
fuzz_target!(|data: &[u8]| {
    let lim = common::small_limits();
    let cancel = AtomicBool::new(false);
    let a = isobmff::parse(data, &lim, &cancel);
    let b = isobmff::read(Cursor::new(data), &lim, &cancel);
    // read also holds a copy of meta, so it can run into max_alloc and max_meta_bytes first.
    let skip = matches!(&a, Err(e) if common::read_only_limit(e))
        || matches!(&b, Err(e) if common::read_only_limit(e));
    if !skip {
        assert_eq!(a, b, "read and parse differ");
    }
    if let Ok(h) = &b {
        common::use_heif(h, data, &lim);
    }
});
