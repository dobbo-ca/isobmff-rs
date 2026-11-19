//! Shared by the targets (`#[path = "../common.rs"] mod common;`): the small limits they run
//! under, and calls of every graph accessor with the invariants that hold for any input.
#![allow(dead_code)]

use std::sync::atomic::AtomicBool;

use isobmff::{Error, Heif, ImageKind, Limit, Limits};

/// Limits that keep one input well under the `-malloc_limit_mb=512` of the fuzz run, so an
/// allocation outside the limits is a crash.
pub fn small_limits() -> Limits {
    Limits {
        max_alloc: 64 << 20,
        max_threads: 1,
        max_boxes: 200_000,
        ..Limits::default()
    }
}

/// Items looked at per input (a file can have 65 535).
const ITEMS: usize = 256;

/// Calls every accessor of `h`, parsed from `data` under `lim`.
pub fn use_heif(h: &Heif, data: &[u8], lim: &Limits) {
    let cancel = AtomicBool::new(false);
    assert!(h.stats.boxes <= lim.max_boxes, "more boxes than max_boxes");
    assert!(h.stats.max_depth <= lim.max_depth, "deeper than max_depth");
    assert!(h.items.len() <= lim.max_items, "more items than max_items");
    assert!(
        h.properties.len() <= lim.max_properties,
        "more properties than max_properties"
    );
    let _ = h.primary_image();
    let tops = h.top_level_images();
    assert!(
        tops.windows(2).all(|w| w[0] < w[1]),
        "top-level ids not ascending"
    );
    for t in tops {
        assert!(h.item(t).is_some(), "top-level image without an item");
    }
    for it in h.items.iter().take(ITEMS) {
        let id = it.id;
        assert_eq!(h.item(id).map(|i| i.id), Some(id), "item lookup");
        if let Ok(img) = h.image(id) {
            assert_eq!(img.id, id);
            assert_eq!(
                img.display_size().is_none(),
                img.width == 0 && img.height == 0,
                "display_size and ispe disagree"
            );
            match &img.kind {
                ImageKind::Grid(g) => {
                    let n = u64::from(g.rows) * u64::from(g.columns);
                    assert_eq!(g.tiles.len() as u64, n, "grid tile count");
                }
                ImageKind::Overlay(o) => {
                    assert!(o.inputs.len() <= lim.max_grid_tiles, "overlay inputs");
                }
                _ => {}
            }
            for t in &img.transforms {
                if let isobmff::Transform::Clap(c) = t {
                    if let Some(r) = c.crop(img.width, img.height) {
                        assert!(
                            u64::from(r.x) + u64::from(r.width) <= u64::from(img.width)
                                && u64::from(r.y) + u64::from(r.height) <= u64::from(img.height),
                            "crop outside the image"
                        );
                    }
                }
            }
            let _ = h.metadata(data, id, lim, &cancel);
        }
        match (&it.data, h.item_bytes(data, id, lim, &cancel)) {
            (Err(_), Ok(_)) => panic!("item_bytes gave bytes for an item with a data error"),
            (Ok(ext), Ok(b)) => {
                let want: u64 = ext.iter().map(|e| e.length).sum();
                assert_eq!(b.len() as u64, want, "item_bytes length");
            }
            (_, Err(e)) => assert_ne!(e, Error::NoSuchItem(id), "item exists"),
        }
    }
}

/// `parse` then every accessor; the result is `parse`'s.
pub fn parse_all(data: &[u8]) -> Result<Heif, Error> {
    let lim = small_limits();
    let cancel = AtomicBool::new(false);
    let r = isobmff::parse(data, &lim, &cancel);
    if let Ok(h) = &r {
        use_heif(h, data, &lim);
    }
    r
}

/// True for the errors that only one of `parse` and `read` can give (`read` copies `meta`).
pub fn read_only_limit(e: &Error) -> bool {
    matches!(e, Error::LimitExceeded(Limit::Alloc | Limit::MetaBytes))
}
