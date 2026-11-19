//! The peak thread count of the calls stays within `max_threads` (the crate spawns none).
//!
//! One test in its own binary, so no other test thread starts or ends while it counts. The OS
//! thread count is read from `/proc/self/task`, so the test runs on Linux only.
#![cfg(target_os = "linux")]

mod common;

use std::io::Cursor;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::time::Duration;

use common::*;
use isobmff::Limits;

fn threads() -> usize {
    std::fs::read_dir("/proc/self/task").map_or(0, Iterator::count)
}

#[test]
fn peak_threads_stay_within_max_threads() {
    let c = AtomicBool::new(false);
    let done = AtomicBool::new(false);
    let peak = AtomicUsize::new(0);
    std::thread::scope(|s| {
        s.spawn(|| {
            while !done.load(Ordering::Relaxed) {
                peak.fetch_max(threads(), Ordering::Relaxed);
                std::thread::sleep(Duration::from_micros(50));
            }
        });
        // Main, the test thread and the sampler.
        while peak.load(Ordering::Relaxed) == 0 {
            std::thread::yield_now();
        }
        let baseline = threads();
        for max_threads in [1, 2, Limits::default().max_threads] {
            let l = Limits {
                max_threads,
                ..Limits::default()
            };
            for (name, file) in all_files() {
                let h = isobmff::parse(&file, &l, &c).unwrap();
                isobmff::read(Cursor::new(&file[..]), &l, &c).unwrap();
                for it in &h.items {
                    let _ = h.image(it.id);
                    let _ = h.item_bytes(&file, it.id, &l, &c);
                    let _ = h.metadata(&file, it.id, &l, &c);
                }
                let _ = h.primary_image();
                assert!(
                    threads() <= baseline,
                    "{name}: {} threads, {baseline} before",
                    threads()
                );
            }
            assert!(
                peak.load(Ordering::Relaxed) <= baseline,
                "max_threads {max_threads}: peak {} over {baseline}",
                peak.load(Ordering::Relaxed)
            );
        }
        done.store(true, Ordering::Relaxed);
    });
}
