#![no_main]
use libfuzzer_sys::fuzz_target;
use std::io::Cursor;
use std::sync::atomic::AtomicBool;

#[path = "../common.rs"]
mod common;

// data = the whole file; read_movie over a Cursor gives the same result as parse_movie, and the
// first samples of every track are the same through both.
fuzz_target!(|data: &[u8]| {
    let lim = common::small_limits();
    let cancel = AtomicBool::new(false);
    let a = isobmff::parse_movie(data, &lim, &cancel);
    let b = isobmff::read_movie(Cursor::new(data), &lim, &cancel);
    // read_movie also holds a copy of moov, so it can run into max_alloc and max_moov_bytes first.
    let skip = matches!(&a, Err(e) if common::read_only_limit(e))
        || matches!(&b, Err(e) if common::read_only_limit(e));
    if !skip {
        assert_eq!(a, b, "read_movie and parse_movie differ");
    }
    if let (Ok(x), Ok(y)) = (&a, &b) {
        for (s, t) in x.tracks.iter().zip(&y.tracks) {
            let w1: Vec<_> = s
                .samples(Cursor::new(data), &lim, &cancel)
                .take(256)
                .collect();
            let w2: Vec<_> = t
                .samples(Cursor::new(data), &lim, &cancel)
                .take(256)
                .collect();
            assert_eq!(w1, w2, "walks differ");
        }
    }
});
