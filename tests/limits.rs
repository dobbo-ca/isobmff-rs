//! Every `Limits` field, the `max_alloc` ledger, the cancel flag and `max_threads`.

mod common;

use std::io::Cursor;
use std::sync::atomic::AtomicBool;

use common::*;
use isobmff::{Error, Heif, Limit, Limits};

fn exceeded(l: Limit) -> Option<Error> {
    Some(Error::LimitExceeded(l))
}

fn box_len(file: &[u8], pos: usize) -> usize {
    let head: [u8; 4] = file[pos..pos + 4].try_into().unwrap();
    u32::from_be_bytes(head) as usize
}

#[test]
fn every_limit_has_its_own_variant() {
    let c = AtomicBool::new(false);
    let d = Limits::default();
    let (one, grid, ex, over) = (single(), grid(), exif_xmp(), overlay());
    let parse = |file: &[u8], l: Limits| isobmff::parse(file, &l, &c);

    let alloc = Limits { max_alloc: 0, ..d };
    assert_eq!(parse(&one, alloc).err(), exceeded(Limit::Alloc));
    let boxes = Limits { max_boxes: 3, ..d };
    assert_eq!(parse(&one, boxes).err(), exceeded(Limit::Boxes));
    let depth = Limits { max_depth: 1, ..d };
    assert_eq!(parse(&one, depth).err(), exceeded(Limit::Depth));
    let items = Limits { max_items: 1, ..d };
    assert_eq!(parse(&ex, items).err(), exceeded(Limit::Items));
    let props = Limits {
        max_properties: 1,
        ..d
    };
    assert_eq!(parse(&one, props).err(), exceeded(Limit::Properties));
    let refs = Limits {
        max_references: 47,
        ..d
    };
    assert_eq!(parse(&grid, refs).err(), exceeded(Limit::References));
    let extents = Limits {
        max_extents: 1,
        ..d
    };
    assert_eq!(parse(&ex, extents).err(), exceeded(Limit::Extents));

    // A grid or overlay over the cap, or an image over max_pixels, fails that image only.
    let tiles = Limits {
        max_grid_tiles: 47,
        ..d
    };
    assert_eq!(
        parse(&grid, tiles).unwrap().image(1).err(),
        exceeded(Limit::GridTiles)
    );
    let inputs = Limits {
        max_grid_tiles: 1,
        ..d
    };
    assert_eq!(
        parse(&over, inputs).unwrap().image(1).err(),
        exceeded(Limit::GridTiles)
    );
    let pixels = Limits {
        max_pixels: 64 * 48 - 1,
        ..d
    };
    let h = parse(&one, pixels).unwrap();
    assert_eq!(h.image(1).err(), exceeded(Limit::Pixels));
    let pixels = Limits {
        max_pixels: 64 * 48,
        ..d
    };
    assert!(parse(&one, pixels).unwrap().image(1).is_ok());
    let pixels = Limits {
        max_pixels: 4096 * 3072 - 1,
        ..d
    };
    assert_eq!(
        parse(&grid, pixels).unwrap().image(1).err(),
        exceeded(Limit::Pixels)
    );

    // read: the copy of meta.
    let meta = Limits {
        max_meta_bytes: 16,
        ..d
    };
    assert_eq!(
        isobmff::read(Cursor::new(&one[..]), &meta, &c).err(),
        exceeded(Limit::MetaBytes)
    );

    // metadata: one Exif or XMP item.
    let h = parse(&ex, d).unwrap();
    let small = Limits {
        max_metadata_bytes: 1,
        ..d
    };
    assert_eq!(
        h.metadata(&ex, 1, &small, &c).err(),
        exceeded(Limit::MetadataBytes)
    );
    assert!(h.metadata(&ex, 1, &d, &c).is_ok());
}

#[test]
fn limit_classes() {
    let all = [
        (Limit::Alloc, "limit-alloc"),
        (Limit::Boxes, "limit-boxes"),
        (Limit::Depth, "limit-depth"),
        (Limit::Items, "limit-items"),
        (Limit::Properties, "limit-properties"),
        (Limit::References, "limit-references"),
        (Limit::Extents, "limit-extents"),
        (Limit::GridTiles, "limit-grid-tiles"),
        (Limit::Pixels, "limit-pixels"),
        (Limit::MetaBytes, "limit-meta-bytes"),
        (Limit::MetadataBytes, "limit-metadata-bytes"),
        (Limit::MoovBytes, "limit-moov-bytes"),
    ];
    for (l, class) in all {
        assert_eq!(Error::LimitExceeded(l).class(), class);
    }
}

/// The smallest `max_alloc` with which `f` succeeds (it fails with `Alloc` below that).
fn alloc_threshold(f: impl Fn(&Limits) -> Result<Heif, Error>) -> usize {
    let at = |n: usize| {
        f(&Limits {
            max_alloc: n,
            ..Limits::default()
        })
    };
    let (mut lo, mut hi) = (0, 256 << 20);
    assert_eq!(at(lo).err(), exceeded(Limit::Alloc));
    assert!(at(hi).is_ok());
    while hi - lo > 1 {
        let mid = lo + (hi - lo) / 2;
        match at(mid) {
            Ok(_) => hi = mid,
            Err(e) => {
                assert_eq!(e, Error::LimitExceeded(Limit::Alloc));
                lo = mid;
            }
        }
    }
    hi
}

#[test]
fn max_alloc_is_the_peak_of_the_ledger() {
    let c = AtomicBool::new(false);
    let (small, big) = (single(), grid());
    let t_small = alloc_threshold(|l| isobmff::parse(&small, l, &c));
    let t_big = alloc_threshold(|l| isobmff::parse(&big, l, &c));
    assert!(t_small > 0, "a parse charges something");
    assert!(
        t_small < t_big,
        "48 tiles need more than one image: {t_small} {t_big}"
    );
    assert!(t_big < 1 << 20, "a 49-item file needs {t_big} bytes");

    // At the threshold the result is the same as with the default; one byte less fails.
    let want = isobmff::parse(&big, &Limits::default(), &c).unwrap();
    let at = Limits {
        max_alloc: t_big,
        ..Limits::default()
    };
    assert_eq!(isobmff::parse(&big, &at, &c).unwrap(), want);
    let below = Limits {
        max_alloc: t_big - 1,
        ..Limits::default()
    };
    assert_eq!(
        isobmff::parse(&big, &below, &c).err(),
        exceeded(Limit::Alloc)
    );

    // read also holds a copy of meta, so it needs at least that much more.
    let meta_len = box_len(&big, box_len(&big, 0));
    let t_read = alloc_threshold(|l| isobmff::read(Cursor::new(&big[..]), l, &c));
    assert!(
        t_read >= t_big + meta_len,
        "read {t_read}, parse {t_big}, meta {meta_len}"
    );
}

#[test]
fn cancel_flag_gives_cancelled() {
    let file = exif_xmp();
    let d = Limits::default();
    let stop = AtomicBool::new(true);
    let go_on = AtomicBool::new(false);
    assert_eq!(
        isobmff::parse(&file, &d, &stop).err(),
        Some(Error::Cancelled)
    );
    assert_eq!(
        isobmff::read(Cursor::new(&file[..]), &d, &stop).err(),
        Some(Error::Cancelled)
    );
    let h = isobmff::parse(&file, &d, &go_on).unwrap();
    assert_eq!(
        h.item_bytes(&file, 1, &d, &stop).err(),
        Some(Error::Cancelled)
    );
    assert_eq!(
        h.metadata(&file, 1, &d, &stop).err(),
        Some(Error::Cancelled)
    );
    assert!(h.item_bytes(&file, 1, &d, &go_on).is_ok());
}

#[test]
fn one_thread_and_the_default_give_equal_results() {
    let c = AtomicBool::new(false);
    let d = Limits::default();
    let one = Limits {
        max_threads: 1,
        ..d
    };
    for (name, file) in all_files() {
        let a = isobmff::parse(&file, &d, &c).unwrap();
        let b = isobmff::parse(&file, &one, &c).unwrap();
        assert_eq!(a, b, "{name}");
        assert_eq!(
            isobmff::read(Cursor::new(&file[..]), &one, &c).unwrap(),
            a,
            "{name}"
        );
        for it in &a.items {
            assert_eq!(a.image(it.id), b.image(it.id), "{name} image {}", it.id);
            assert_eq!(
                a.item_bytes(&file, it.id, &d, &c),
                b.item_bytes(&file, it.id, &one, &c),
                "{name} item {}",
                it.id
            );
        }
        assert_eq!(a.top_level_images(), b.top_level_images(), "{name}");
    }
}
