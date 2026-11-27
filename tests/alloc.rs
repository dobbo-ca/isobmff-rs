//! The real heap use of `Heif::image` and `Heif::metadata` on files with a huge reference count.
//!
//! The id lists of an image (`dimg` inputs, thumbnails, auxiliary images, Exif and XMP items) are
//! bounded by the items of the file, not by the count of its references: a file with a million
//! `thmb` references from one item has one thumbnail, and the call must not hold a million ids on
//! the way to it. A grid, identity or overlay whose `dimg` count is wrong is an error before any
//! list is made.
//!
//! One test in its own binary with a counting global allocator, so no other test allocates while
//! it measures.

mod common;

use std::alloc::{GlobalAlloc, Layout, System};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

use common::*;
use isobmff::{Error, Heif, Limits};

struct Counting;

static NOW: AtomicUsize = AtomicUsize::new(0);
static PEAK: AtomicUsize = AtomicUsize::new(0);

fn grow(n: usize) {
    let now = NOW.fetch_add(n, Ordering::Relaxed) + n;
    PEAK.fetch_max(now, Ordering::Relaxed);
}

// SAFETY: every call goes to `System` with the same arguments; the counters only observe.
unsafe impl GlobalAlloc for Counting {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        // SAFETY: the caller's contract for `alloc` is passed on unchanged.
        let p = unsafe { System.alloc(layout) };
        if !p.is_null() {
            grow(layout.size());
        }
        p
    }

    unsafe fn alloc_zeroed(&self, layout: Layout) -> *mut u8 {
        // SAFETY: as for `alloc`.
        let p = unsafe { System.alloc_zeroed(layout) };
        if !p.is_null() {
            grow(layout.size());
        }
        p
    }

    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        // SAFETY: as for `alloc`.
        unsafe { System.dealloc(ptr, layout) };
        NOW.fetch_sub(layout.size(), Ordering::Relaxed);
    }

    unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
        // SAFETY: as for `alloc`.
        let p = unsafe { System.realloc(ptr, layout, new_size) };
        if !p.is_null() {
            NOW.fetch_sub(layout.size(), Ordering::Relaxed);
            grow(new_size);
        }
        p
    }
}

#[global_allocator]
static GLOBAL: Counting = Counting;

/// The most bytes `f` holds at once, above what was held before it.
fn peak<R>(f: impl FnOnce() -> R) -> (R, usize) {
    let base = NOW.load(Ordering::Relaxed);
    PEAK.store(base, Ordering::Relaxed);
    let r = f();
    (r, PEAK.load(Ordering::Relaxed).saturating_sub(base))
}

/// Far below the ids of the references in these files (4 bytes each), far above what the calls
/// need for the few items they have.
const SMALL: usize = 64 << 10;

/// Ids per `iref` box (its count is 16 bits) and boxes: 983 025 `dimg` ids, under the default
/// `max_references`.
const IDS: usize = 65_535;
const BOXES: usize = 15;
/// One-id reference boxes from one item to the image.
const MANY: usize = 200_000;

fn parse(file: &[u8]) -> Heif {
    isobmff::parse(file, &Limits::default(), &AtomicBool::new(false)).unwrap()
}

/// A grid of 48 tiles with 983 073 `dimg` ids.
fn grid_dimg() -> Vec<u8> {
    let mut s = grid_spec();
    for _ in 0..BOXES {
        s.link(b"dimg", 1, &[2; IDS]);
    }
    s.build()
}

/// An `iden` image (2) with 983 025 `dimg` ids to image 1.
fn iden_dimg() -> Vec<u8> {
    let mut s = Spec::new(b"heic");
    let size = s.prop(ispe(64, 48));
    let cfg = s.prop(hvcc());
    s.hevc(1, size, cfg).hidden();
    s.item(2, b"iden").assoc(&[size]);
    s.primary = 2;
    for _ in 0..BOXES {
        s.link(b"dimg", 2, &[1; IDS]);
    }
    s.build()
}

/// Image 1 and item 2 (of type `kind`, an image for `thmb` and `auxl`), with `MANY` references
/// of type `link` from 2 to 1.
fn many_links(link: &[u8; 4], kind: &[u8; 4]) -> Vec<u8> {
    let mut s = Spec::new(b"heic");
    let size = s.prop(ispe(64, 48));
    let cfg = s.prop(hvcc());
    s.hevc(1, size, cfg);
    if kind == b"hvc1" {
        s.hevc(2, size, cfg);
    } else {
        s.item(2, kind).data(&exif_item(0));
    }
    for _ in 0..MANY {
        s.link(link, 2, &[1]);
    }
    s.build()
}

#[test]
fn id_lists_are_bounded_by_the_items_not_the_references() {
    let file = grid_dimg();
    let heif = parse(&file);
    let (r, used) = peak(|| heif.image(1));
    assert_eq!(r.err(), Some(Error::Malformed("grid")));
    assert!(used < SMALL, "grid with 983 073 dimg ids: {used} bytes");

    let file = iden_dimg();
    let heif = parse(&file);
    let (r, used) = peak(|| heif.image(2));
    assert_eq!(r.err(), Some(Error::Malformed("iden")));
    assert!(used < SMALL, "iden with 983 025 dimg ids: {used} bytes");

    let file = many_links(b"thmb", b"hvc1");
    let heif = parse(&file);
    let (r, used) = peak(|| heif.image(1));
    assert_eq!(r.unwrap().thumbnails, [2]);
    assert!(used < SMALL, "{MANY} thmb references: {used} bytes");

    let file = many_links(b"auxl", b"hvc1");
    let heif = parse(&file);
    let (r, used) = peak(|| heif.image(1));
    assert_eq!(r.unwrap().aux.iter().map(|a| a.id).collect::<Vec<_>>(), [2]);
    assert!(used < SMALL, "{MANY} auxl references: {used} bytes");

    let file = many_links(b"cdsc", b"Exif");
    let heif = parse(&file);
    let (r, used) = peak(|| heif.image(1));
    assert_eq!(r.unwrap().exif, [2]);
    assert!(used < SMALL, "{MANY} cdsc references: {used} bytes");
    let c = AtomicBool::new(false);
    let (r, used) = peak(|| {
        heif.metadata(&file, 1, &Limits::default(), &c)
            .map(|m| m.exif)
    });
    assert!(r.unwrap().is_some());
    assert!(
        used < SMALL,
        "metadata, {MANY} cdsc references: {used} bytes"
    );
}
