//! Hand-built HEIF and AVIF files (tests/common) through `parse`, `Heif::image`, `item_bytes` and
//! `metadata`.

mod common;

use std::sync::atomic::AtomicBool;

use common::*;
use isobmff::{
    Axis, Clap, Codec, CodecConfig, Colour, Derived, Error, Extent, FourCc, Fraction,
    GainMapSource, Heif, ImageKind, ItemError, Limits, Metadata, OverlayInput, Rect, Transform,
};

fn go(file: &[u8]) -> Heif {
    parse_with(file, &Limits::default()).unwrap()
}

fn parse_with(file: &[u8], limits: &Limits) -> Result<Heif, Error> {
    isobmff::parse(file, limits, &AtomicBool::new(false))
}

fn bytes(h: &Heif, file: &[u8], id: u32) -> Result<Vec<u8>, Error> {
    h.item_bytes(file, id, &Limits::default(), &AtomicBool::new(false))
        .map(|b| b.into_owned())
}

fn meta<'a>(h: &Heif, file: &'a [u8], id: u32) -> Result<Metadata<'a>, Error> {
    h.metadata(file, id, &Limits::default(), &AtomicBool::new(false))
}

#[test]
fn single_hevc_image() {
    let file = single();
    let h = go(&file);
    assert_eq!(h.ftyp.major, FourCc(*b"heic"));
    assert!(h.ftyp.has_brand(b"mif1"));
    assert_eq!(h.primary, 1);
    assert_eq!(h.properties.len(), 6);
    assert!(!h.has_moov);
    let img = h.primary_image().unwrap();
    assert_eq!((img.width, img.height), (64, 48));
    let ImageKind::Coded {
        codec: Codec::Hevc,
        config: CodecConfig::Hevc(c),
    } = &img.kind
    else {
        panic!("not an hevc image: {:?}", img.kind)
    };
    assert_eq!(
        (c.chroma_format_idc, c.bit_depth_luma, c.bit_depth_chroma),
        (1, 8, 8)
    );
    assert_eq!(c.arrays.len(), 1);
    let a = &c.arrays[0];
    assert_eq!((a.complete, a.nal_type), (true, 32));
    assert_eq!(Heif::slice(&file, a.nals[0]), Some(&VPS[..]));
    assert_eq!(img.colour.len(), 2);
    assert_eq!(
        img.colour[0],
        Colour::Nclx {
            primaries: 1,
            transfer: 13,
            matrix: 6,
            full_range: true
        }
    );
    let Colour::Icc(e) = img.colour[1] else {
        panic!("second colr is not an ICC profile")
    };
    assert_eq!(Heif::slice(&file, e), Some(ICC));
    assert_eq!(img.pixi, Some(vec![8, 8, 8]));
    assert_eq!(img.pasp, Some((1, 1)));
    assert!(img.transforms.is_empty());
    assert_eq!(img.display_size(), Some((64, 48)));
    assert_eq!(bytes(&h, &file, 1), Ok(vec![1, 2, 3, 4]));
    assert_eq!(h.top_level_images(), vec![1]);
    assert_eq!(meta(&h, &file, 1), Ok(Metadata::default()));
}

#[test]
fn avif_image() {
    let file = avif();
    let h = go(&file);
    assert!(h.ftyp.has_brand(b"avif"));
    let img = h.primary_image().unwrap();
    let ImageKind::Coded {
        codec: Codec::Av1,
        config: CodecConfig::Av1(c),
    } = &img.kind
    else {
        panic!("not an av1 image: {:?}", img.kind)
    };
    assert_eq!(
        (c.depth(), c.yuv_format(), c.seq_level_idx_0),
        (10, 420, 12)
    );
    assert_eq!(Heif::slice(&file, c.config_obus), Some(&[0x0a, 0x00][..]));
    assert_eq!(img.colour.len(), 1);
}

#[test]
fn grid_of_48_tiles_parses_with_default_limits() {
    let file = grid();
    let h = go(&file);
    assert_eq!(h.items.len(), 49);
    assert_eq!(h.references.len(), 1);
    assert_eq!(h.references[0].to.len(), 48);
    let img = h.primary_image().unwrap();
    assert_eq!((img.width, img.height), (4096, 3072));
    let ImageKind::Grid(g) = &img.kind else {
        panic!("not a grid: {:?}", img.kind)
    };
    assert_eq!((g.rows, g.columns, g.width, g.height), (6, 8, 4096, 3072));
    assert_eq!(g.tiles, (2..=49).collect::<Vec<u32>>());
    assert_eq!(h.top_level_images(), vec![1]);
    assert_eq!(bytes(&h, &file, 1).unwrap().len(), 8);
    assert_eq!(bytes(&h, &file, 49), Ok(vec![49; 4]));
}

#[test]
fn grid_fails_when_the_caps_are_lowered() {
    let file = grid();
    let tiles = |n: usize| Limits {
        max_grid_tiles: n,
        ..Limits::default()
    };
    let h = parse_with(&file, &tiles(47)).unwrap();
    assert_eq!(
        h.image(1).err(),
        Some(Error::LimitExceeded(isobmff::Limit::GridTiles))
    );
    assert!(parse_with(&file, &tiles(48)).unwrap().image(1).is_ok());
    let refs = |n: usize| Limits {
        max_references: n,
        ..Limits::default()
    };
    assert_eq!(
        parse_with(&file, &refs(47)).err(),
        Some(Error::LimitExceeded(isobmff::Limit::References))
    );
    assert!(parse_with(&file, &refs(48)).is_ok());
}

#[test]
fn overlay_with_two_inputs() {
    let file = overlay();
    let h = go(&file);
    let img = h.primary_image().unwrap();
    let ImageKind::Overlay(o) = &img.kind else {
        panic!("not an overlay: {:?}", img.kind)
    };
    assert_eq!(o.fill, [0x1111, 0x2222, 0x3333, 0xffff]);
    assert_eq!((o.width, o.height), (100, 80));
    assert_eq!(
        o.inputs,
        vec![
            OverlayInput { id: 2, x: 0, y: 0 },
            OverlayInput {
                id: 3,
                x: -10,
                y: 20
            }
        ]
    );
    assert_eq!(h.top_level_images(), vec![1]);
}

#[test]
fn alpha_and_depth_auxiliary_images() {
    let file = alpha_depth();
    let h = go(&file);
    let img = h.primary_image().unwrap();
    assert_eq!((img.alpha, img.depth), (Some(2), Some(3)));
    let ids: Vec<u32> = img.aux.iter().map(|a| a.id).collect();
    assert_eq!(ids, vec![2, 3, 4]);
    assert_eq!(&*img.aux[0].aux_type, ALPHA);
    assert_eq!(&*img.aux[1].aux_type, DEPTH);
    assert_eq!(&*img.aux[2].aux_type, "urn:x:other");
    assert_eq!(img.aux[0].subtype.length, 0);
    assert_eq!(h.top_level_images(), vec![1]);
    // The auxiliary images have no auxiliary images of their own.
    assert_eq!(h.image(2).unwrap().alpha, None);
}

#[test]
fn thumbnails_point_at_their_master() {
    let file = thumbnails();
    let h = go(&file);
    let img = h.primary_image().unwrap();
    assert_eq!(img.thumbnails, vec![2, 3]);
    // The thumbnails are sources of `thmb` references, so they are not top-level images.
    assert_eq!(h.top_level_images(), vec![1]);
    let t = h.image(2).unwrap();
    assert_eq!((t.width, t.height), (16, 12));
    assert!(t.thumbnails.is_empty());
}

#[test]
fn apple_gain_map_is_an_auxl_image() {
    let file = apple_gain_map();
    let h = go(&file);
    let img = h.primary_image().unwrap();
    let gm = img.gain_map.as_ref().unwrap();
    assert_eq!(gm.image, 2);
    assert_eq!(gm.source, GainMapSource::Apple);
    assert_eq!((gm.tmap, gm.metadata), (None, None));
    assert_eq!(gm.xmp, vec![3]);
    assert_eq!(img.aux.len(), 1);
    assert_eq!(&*img.aux[0].aux_type, APPLE_GAIN_MAP);
    assert_eq!(img.alpha, None);
}

#[test]
fn tmap_item_in_an_altr_group() {
    let file = tmap_altr();
    let h = go(&file);
    assert_eq!(h.groups.len(), 1);
    assert_eq!(h.groups[0].kind, FourCc(*b"altr"));
    assert_eq!(h.groups[0].entities, vec![3, 1]);
    // The primary item is the tmap item itself; the gain map hangs off the base image.
    assert_eq!(
        h.primary_image().unwrap().kind,
        ImageKind::Unsupported(FourCc(*b"tmap"))
    );
    let base = h.image(1).unwrap();
    let gm = base.gain_map.as_ref().unwrap();
    assert_eq!(
        (gm.image, gm.tmap, gm.source),
        (2, Some(3), GainMapSource::Tmap)
    );
    assert!(gm.xmp.is_empty());
    let m = gm.metadata.unwrap();
    assert_eq!((m.minimum_version, m.writer_version, m.flags), (0, 0, 0x08));
    assert!(m.common_denominator && !m.is_multichannel);
    assert!(!m.use_base_colour_space && !m.backward_direction);
    let f = |numerator, denominator| Fraction {
        numerator,
        denominator,
    };
    assert_eq!(m.base_headroom, f(0, 1000));
    assert_eq!(m.alternate_headroom, f(3000, 1000));
    let ch = m.channels[0];
    assert_eq!(
        (ch.min, ch.max, ch.gamma),
        (f(0, 1000), f(2000, 1000), f(1000, 1000))
    );
    assert_eq!(
        (ch.base_offset, ch.alternate_offset),
        (f(15, 1000), f(25, 1000))
    );
    // A single-channel map repeats channel 0.
    assert_eq!(m.channels[1], ch);
    assert_eq!(m.channels[2], ch);
    assert_eq!(h.item(3).unwrap().derived, Some(Ok(Derived::Tmap(Some(m)))));
    // The gain map image has no gain map of its own.
    assert_eq!(h.image(2).unwrap().gain_map, None);
}

#[test]
fn exif_with_offset_header_and_xmp() {
    let file = exif_xmp();
    let h = go(&file);
    let img = h.primary_image().unwrap();
    assert_eq!((img.exif.clone(), img.xmp.clone()), (vec![2], vec![3]));
    assert_eq!(h.item(3).unwrap().content_type, XMP_TYPE);
    let m = meta(&h, &file, 1).unwrap();
    // The 2 filler bytes of the offset header are gone.
    assert_eq!(m.exif.as_deref(), Some(&TIFF[..]));
    assert_eq!(m.xmp.as_deref(), Some(XMP));
    // The raw item still has the header.
    assert_eq!(bytes(&h, &file, 2), Ok(exif_item(2)));
}

#[test]
fn exif_with_a_bad_offset_is_malformed() {
    let file = exif_bad_offset();
    let h = go(&file);
    assert_eq!(meta(&h, &file, 1).err(), Some(Error::Malformed("Exif")));
    // Only the metadata call fails.
    assert!(h.primary_image().is_ok());
    assert!(bytes(&h, &file, 2).is_ok());
}

#[test]
fn idat_construction_method_1() {
    let file = idat_file();
    let h = go(&file);
    let exif = h.item(4).unwrap();
    assert_eq!(exif.construction_method, 1);
    let Ok(ext) = &exif.data else {
        panic!("no extents: {:?}", exif.data)
    };
    assert_eq!(ext.len(), 1);
    // The extent is an absolute file offset into the idat payload.
    assert_eq!(Heif::slice(&file, ext[0]), Some(&exif_item(0)[..]));
    assert_eq!(h.item(1).unwrap().construction_method, 1);
    // The grid payload was read from idat.
    let img = h.primary_image().unwrap();
    let ImageKind::Grid(g) = &img.kind else {
        panic!("not a grid: {:?}", img.kind)
    };
    assert_eq!((g.rows, g.columns, g.width, g.height), (1, 2, 128, 64));
    assert_eq!(g.tiles, vec![2, 3]);
    assert_eq!(h.item(2).unwrap().construction_method, 0);
    assert_eq!(meta(&h, &file, 1).unwrap().exif.as_deref(), Some(&TIFF[..]));
}

#[test]
fn iloc_versions_and_field_sizes() {
    for f in ILOC_FORMATS {
        let file = iloc_file(f);
        let h = go(&file);
        let what = format!("iloc {f:?}");
        assert_eq!(bytes(&h, &file, 1), Ok(vec![1; 4]), "{what}");
        assert_eq!(bytes(&h, &file, 2), Ok(exif_item(0)), "{what}");
        assert_eq!(
            h.item(2).unwrap().construction_method,
            u8::from(f.version >= 1),
            "{what}"
        );
        assert_eq!(
            meta(&h, &file, 1).unwrap().exif.as_deref(),
            Some(&TIFF[..]),
            "{what}"
        );
    }
}

#[test]
fn construction_method_2_is_an_item_error_not_a_parse_error() {
    let file = method2_file();
    let h = go(&file);
    let it = h.item(2).unwrap();
    assert_eq!(it.construction_method, 2);
    assert_eq!(
        it.data,
        Err(ItemError::Unsupported("construction method 2"))
    );
    assert_eq!(
        bytes(&h, &file, 2),
        Err(Error::Unsupported("construction method 2"))
    );
    assert_eq!(
        meta(&h, &file, 1).err(),
        Some(Error::Unsupported("construction method 2"))
    );
    assert!(h.primary_image().is_ok());
    assert_eq!(bytes(&h, &file, 1), Ok(vec![1; 4]));
}

#[test]
fn largesize_and_size_zero_boxes() {
    for (name, file) in [("largesize", largesize()), ("size_zero", size_zero())] {
        let h = go(&file);
        assert_eq!(bytes(&h, &file, 1), Ok(vec![1; 4]), "{name}");
        assert_eq!(h.items.len(), 1, "{name}");
    }
}

#[test]
fn transforms_keep_ipma_order_and_display_size_applies_them() {
    let h = go(&transforms());
    let crop = Clap {
        width_n: 40,
        width_d: 1,
        height_n: 20,
        height_d: 1,
        horiz_off_n: 0,
        horiz_off_d: 1,
        vert_off_n: 0,
        vert_off_d: 1,
    };
    assert_eq!(
        crop.crop(100, 50),
        Some(Rect {
            x: 30,
            y: 15,
            width: 40,
            height: 20
        })
    );
    let a = h.image(1).unwrap();
    assert_eq!(
        a.transforms,
        vec![
            Transform::Clap(crop),
            Transform::Rotate(90),
            Transform::Mirror(Axis::LeftRight)
        ]
    );
    // clap: 100 x 50 -> 40 x 20; irot 90 swaps; imir keeps.
    assert_eq!(a.display_size(), Some((20, 40)));
    let b = h.image(2).unwrap();
    assert_eq!(
        b.transforms,
        vec![
            Transform::Rotate(90),
            Transform::Clap(crop),
            Transform::Mirror(Axis::TopBottom)
        ]
    );
    // irot first: 100 x 50 -> 50 x 100; the 40 x 20 clap fits; imir keeps.
    assert_eq!(b.display_size(), Some((40, 20)));
    assert_eq!((a.width, a.height), (100, 50));
}

#[test]
fn essential_unknown_property_makes_the_image_unsupported() {
    let h = go(&essential_unknown());
    let a = h.image(1).unwrap();
    assert_eq!(a.kind, ImageKind::Unsupported(FourCc(*b"hvc1")));
    assert_eq!((a.width, a.height), (64, 48));
    let b = h.image(2).unwrap();
    assert!(matches!(b.kind, ImageKind::Coded { .. }));
}

#[test]
fn errors_of_a_missing_item() {
    let file = single();
    let h = go(&file);
    assert_eq!(h.image(9).err(), Some(Error::NoSuchItem(9)));
    assert_eq!(bytes(&h, &file, 9), Err(Error::NoSuchItem(9)));
    assert_eq!(meta(&h, &file, 9).err(), Some(Error::NoSuchItem(9)));
    assert_eq!(
        Heif::slice(
            &file,
            Extent {
                offset: u64::MAX,
                length: 2
            }
        ),
        None
    );
}

#[test]
fn tmap_is_not_a_top_level_image() {
    let h = go(&tmap_altr());
    assert_eq!(h.top_level_images(), vec![1, 2]);
}

#[test]
fn tmap_after_the_base_in_the_altr_group_is_ignored() {
    let mut s = Spec::new(b"avif");
    s.primary = 3;
    let size = s.prop(ispe(64, 48));
    let small = s.prop(ispe(32, 24));
    let cfg = s.prop(av1c());
    s.av1(1, size, cfg);
    s.av1(2, small, cfg);
    s.item(3, b"tmap").data(&tmap_payload());
    s.link(b"dimg", 3, &[1, 2]);
    s.group(b"altr", 100, &[1, 3]);
    let h = go(&s.build());
    assert!(h.image(1).unwrap().gain_map.is_none());
}

#[test]
fn prem_is_read_from_the_source_item() {
    let mut s = Spec::new(b"heic");
    let size = s.prop(ispe(64, 48));
    let cfg = s.prop(hvcc());
    let alpha = s.prop(auxc(ALPHA));
    s.hevc(1, size, cfg);
    s.hevc(2, size, cfg).assoc(&[alpha]);
    s.link(b"auxl", 2, &[1]);
    s.link(b"prem", 1, &[2]);
    let h = go(&s.build());
    assert!(h.image(1).unwrap().premultiplied);
    assert!(!h.image(2).unwrap().premultiplied);
}

#[test]
fn essential_unknown_property_still_needs_the_codec_config() {
    let mut s = Spec::new(b"heic");
    let size = s.prop(ispe(64, 48));
    let zzzz = s.prop(unknown(b"zzzz"));
    s.item(1, b"hvc1").assoc(&[size]).essential(zzzz);
    let h = go(&s.build());
    assert_eq!(h.image(1).err(), Some(Error::Malformed("hvcC")));
}

#[test]
fn thumbnails_and_aux_come_only_from_image_items() {
    let mut s = Spec::new(b"heic");
    let size = s.prop(ispe(64, 48));
    let cfg = s.prop(hvcc());
    s.hevc(1, size, cfg);
    s.item(2, b"Exif").data(&[0; 8]);
    s.link(b"thmb", 2, &[1]);
    s.link(b"auxl", 2, &[1]);
    let img = go(&s.build()).image(1).unwrap();
    assert!(img.thumbnails.is_empty() && img.aux.is_empty());
}
