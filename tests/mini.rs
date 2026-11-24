//! The `mini` box (ISO/IEC 23008-12 Annex O): files built with the bit writer of tests/common,
//! through `parse`, `read`, `Heif::image`, `item_bytes` and `metadata`.

mod common;

use std::io::Cursor;
use std::sync::atomic::AtomicBool;

use common::*;
use isobmff::{
    Axis, Codec, CodecConfig, Colour, Derived, Error, Extent, FourCc, GainMapSource, Heif,
    ImageKind, Limit, Limits, MiniAmve, MiniCclv, MiniClli, MiniHdr, MiniMdcv, MiniReve, PropValue,
    Transform,
};

fn go(file: &[u8]) -> Heif {
    isobmff::parse(file, &Limits::default(), &AtomicBool::new(false)).unwrap()
}

fn fail(file: &[u8]) -> Error {
    isobmff::parse(file, &Limits::default(), &AtomicBool::new(false)).unwrap_err()
}

fn bytes(h: &Heif, file: &[u8], id: u32) -> Vec<u8> {
    h.item_bytes(file, id, &Limits::default(), &AtomicBool::new(false))
        .unwrap()
        .into_owned()
}

/// The (4cc, essential) of the associations of an item, in order.
fn props(h: &Heif, id: u32) -> Vec<([u8; 4], bool)> {
    h.item(id)
        .unwrap()
        .associations
        .iter()
        .map(|a| (h.properties[usize::from(a.index) - 1].kind.0, a.essential))
        .collect()
}

fn kinds(h: &Heif) -> Vec<[u8; 4]> {
    h.properties.iter().map(|p| p.kind.0).collect()
}

fn chunk(file: &[u8], e: Extent) -> &[u8] {
    Heif::slice(file, e).unwrap()
}

#[test]
fn av1_with_alpha_icc_exif_and_xmp() {
    let m = MiniSpec::full();
    let file = mini_file(&m);
    let h = go(&file);
    assert_eq!(h.ftyp.major, FourCc(*b"mif3"));
    assert_eq!(h.primary, 1);
    assert!(!h.has_moov && h.other_boxes.is_empty() && h.groups.is_empty());
    let ids: Vec<u32> = h.items.iter().map(|i| i.id).collect();
    assert_eq!(ids, [1, 2, 6, 7]);
    let types: Vec<[u8; 4]> = h.items.iter().map(|i| i.item_type.0).collect();
    assert_eq!(types, [*b"av01", *b"av01", *b"Exif", *b"mime"]);
    assert!(
        h.items
            .iter()
            .all(|i| !i.hidden && i.construction_method == 0)
    );
    assert_eq!(h.item(7).unwrap().content_type, XMP_TYPE);
    assert_eq!(h.item(7).unwrap().content_encoding, "");
    let refs: Vec<(FourCc, u32, Vec<u32>)> = h
        .references
        .iter()
        .map(|r| (r.kind, r.from, r.to.clone()))
        .collect();
    assert_eq!(
        refs,
        vec![
            (FourCc(*b"auxl"), 2, vec![1]),
            (FourCc(*b"prem"), 1, vec![2]),
            (FourCc(*b"cdsc"), 6, vec![1]),
            (FourCc(*b"cdsc"), 7, vec![1]),
        ]
    );
    assert_eq!(
        kinds(&h),
        [
            *b"av1C", *b"ispe", *b"pixi", *b"colr", *b"colr", *b"av1C", *b"auxC", *b"irot"
        ]
    );
    assert_eq!(
        props(&h, 1),
        [
            (*b"av1C", true),
            (*b"ispe", false),
            (*b"pixi", false),
            (*b"colr", true),
            (*b"colr", true),
            (*b"irot", true)
        ]
    );
    assert_eq!(
        props(&h, 2),
        [
            (*b"av1C", true),
            (*b"ispe", false),
            (*b"auxC", true),
            (*b"irot", true)
        ]
    );
    assert!(props(&h, 6).is_empty() && props(&h, 7).is_empty());

    let img = h.primary_image().unwrap();
    assert_eq!((img.width, img.height), (64, 48));
    assert_eq!(img.transforms, [Transform::Rotate(270)]);
    assert_eq!(img.display_size(), Some((48, 64)));
    assert_eq!(img.pixi, Some(vec![10, 10, 10]));
    assert_eq!((img.alpha, img.premultiplied), (Some(2), true));
    assert_eq!(img.exif, [6]);
    assert_eq!(img.xmp, [7]);
    assert_eq!(&*img.aux[0].aux_type, ALPHA);
    assert_eq!(
        img.colour[0],
        Colour::Nclx {
            primaries: 9,
            transfer: 16,
            matrix: 9,
            full_range: true
        }
    );
    let Colour::Icc(icc) = img.colour[1] else {
        panic!("no ICC profile: {:?}", img.colour)
    };
    assert_eq!(chunk(&file, icc), &m.icc[..]);
    let ImageKind::Coded {
        codec: Codec::Av1,
        config: CodecConfig::Av1(c),
    } = &img.kind
    else {
        panic!("not an av1 image: {:?}", img.kind)
    };
    assert_eq!(c.depth(), 10);
    assert_eq!(chunk(&file, c.raw), &m.main_cfg[..]);
    let alpha = h.image(2).unwrap();
    assert!(matches!(
        alpha.kind,
        ImageKind::Coded {
            codec: Codec::Av1,
            ..
        }
    ));
    assert_eq!((alpha.width, alpha.height), (64, 48));
    assert_eq!(alpha.transforms, [Transform::Rotate(270)]);
    assert_eq!(h.top_level_images(), [1]);

    // The header fields and the chunk ranges.
    let mini = h.mini.as_ref().unwrap();
    assert_eq!(chunk(&file, mini.range)[4..8], *b"mini");
    assert_eq!(mini.range.offset + 8, mini.payload.offset);
    assert_eq!(mini.range.end(), mini.payload.end());
    assert_eq!((mini.orientation, mini.bit_depth), (6, 10));
    assert_eq!(
        (mini.width, mini.height, mini.chroma_subsampling),
        (64, 48, 1)
    );
    assert_eq!(
        (
            mini.colour_primaries,
            mini.transfer_characteristics,
            mini.matrix_coefficients
        ),
        (9, 16, 9)
    );
    assert!(mini.explicit_cicp && mini.alpha && mini.alpha_premultiplied && mini.full_range);
    assert!(mini.icc && mini.exif && mini.xmp && !mini.hdr && !mini.float);
    assert!(!mini.explicit_codec_types && !mini.exif_xmp_compressed);
    assert_eq!(mini.infe_type, FourCc(*b"av01"));
    assert_eq!(mini.codec_config_type, FourCc(*b"av1C"));
    assert!(mini.gain_map.is_none());
    assert_eq!(chunk(&file, mini.main_config), &m.main_cfg[..]);
    // An alpha configuration of size 0 is the main one.
    assert_eq!(mini.alpha_config, mini.main_config);
    assert_eq!(chunk(&file, mini.icc_profile), &m.icc[..]);
    assert_eq!(chunk(&file, mini.alpha_data), &m.alpha_data[..]);
    assert_eq!(chunk(&file, mini.main_data), &m.main_data[..]);
    assert_eq!(chunk(&file, mini.exif_data), &m.exif[..]);
    assert_eq!(chunk(&file, mini.xmp_data), &m.xmp[..]);
    assert_eq!(mini.gain_map_data.length, 0);

    // The item data are the chunks, in the file.
    assert_eq!(
        h.item(1).unwrap().data,
        Ok(vec![Extent {
            offset: find(&file, &m.main_data),
            length: m.main_data.len() as u64
        }])
    );
    assert_eq!(bytes(&h, &file, 1), m.main_data);
    assert_eq!(bytes(&h, &file, 2), m.alpha_data);
    assert_eq!(bytes(&h, &file, 6), m.exif);
    assert_eq!(bytes(&h, &file, 7), m.xmp);
    // The Exif of a mini box has no `exif_tiff_header_offset`.
    let md = h
        .metadata(&file, 1, &Limits::default(), &AtomicBool::new(false))
        .unwrap();
    assert_eq!(md.exif.as_deref(), Some(&TIFF[..]));
    assert_eq!(md.xmp.as_deref(), Some(XMP));
}

#[test]
fn heic_by_the_minor_version() {
    let file = mini_file(&MiniSpec::hevc());
    let h = go(&file);
    assert_eq!(h.items.len(), 1);
    assert_eq!(h.items[0].item_type, FourCc(*b"hvc1"));
    assert_eq!(kinds(&h), [*b"hvcC", *b"ispe", *b"pixi", *b"colr"]);
    let img = h.primary_image().unwrap();
    let ImageKind::Coded {
        codec: Codec::Hevc,
        config: CodecConfig::Hevc(c),
    } = &img.kind
    else {
        panic!("not an hevc image: {:?}", img.kind)
    };
    assert_eq!((c.chroma_format_idc, c.bit_depth_luma), (1, 8));
    // Without explicit CICP: 1 / 13 / 6 and the full range flag.
    assert_eq!(
        img.colour,
        [Colour::Nclx {
            primaries: 1,
            transfer: 13,
            matrix: 6,
            full_range: true
        }]
    );
    assert_eq!(h.mini.as_ref().unwrap().codec_config_type, FourCc(*b"hvcC"));
    // The configuration is too large for 3 bits: `large_codec_config_flag` is set.
    assert!(h.mini.as_ref().unwrap().large_codec_config);
}

#[test]
fn explicit_codec_types_win_over_the_brand() {
    let m = MiniSpec {
        minor: *b"heic",
        codec: Some((*b"av01", *b"av1C")),
        ..MiniSpec::av1()
    };
    let h = go(&mini_file(&m));
    assert_eq!(h.items[0].item_type, FourCc(*b"av01"));
    assert!(h.mini.as_ref().unwrap().explicit_codec_types);
    assert!(matches!(
        h.primary_image().unwrap().kind,
        ImageKind::Coded {
            codec: Codec::Av1,
            ..
        }
    ));

    // A codec this crate does not decode: the image is unsupported, the graph is there.
    let m = MiniSpec {
        minor: *b"vvi3",
        codec: Some((*b"vvc1", *b"vvcC")),
        ..MiniSpec::av1()
    };
    let file = mini_file(&m);
    let h = go(&file);
    assert_eq!(h.items[0].item_type, FourCc(*b"vvc1"));
    assert_eq!(kinds(&h)[0], *b"vvcC");
    assert_eq!(
        h.primary_image().unwrap().kind,
        ImageKind::Unsupported(FourCc(*b"vvc1"))
    );
    assert_eq!(bytes(&h, &file, 1), m.main_data);
}

#[test]
fn a_brand_that_is_not_avif_or_heic_needs_explicit_codec_types() {
    for minor in [*b"vvi3", [0; 4], *b"mif3"] {
        let m = MiniSpec {
            minor,
            ..MiniSpec::av1()
        };
        assert_eq!(fail(&mini_file(&m)), Error::Unsupported("mini codec"));
    }
}

#[test]
fn dimensions_small_and_large() {
    for (w, h, large) in [
        (1, 1, false),
        (128, 128, false),
        (129, 1, true),
        (20000, 30, true),
    ] {
        let file = mini_file(&MiniSpec {
            width: w,
            height: h,
            ..MiniSpec::av1()
        });
        let heif = go(&file);
        let img = heif.primary_image().unwrap();
        assert_eq!((img.width, img.height), (w, h));
        assert_eq!(heif.mini.as_ref().unwrap().large_dimensions, large);
        assert_eq!(bytes(&heif, &file, 1), b"MAIN-DATA-0123");
    }
}

#[test]
fn chroma_depth_and_float() {
    for (chroma, channels) in [(0u8, 1usize), (1, 3), (2, 3), (3, 3)] {
        for depth in 8..=16u8 {
            let file = mini_file(&MiniSpec {
                chroma,
                depth,
                chroma_h: chroma == 1 || chroma == 2,
                chroma_v: chroma == 1,
                ..MiniSpec::av1()
            });
            let h = go(&file);
            let mini = h.mini.as_ref().unwrap();
            assert_eq!((mini.chroma_subsampling, mini.bit_depth), (chroma, depth));
            assert_eq!(
                (
                    mini.chroma_horizontally_centred,
                    mini.chroma_vertically_centred
                ),
                (chroma == 1 || chroma == 2, chroma == 1)
            );
            assert_eq!(h.primary_image().unwrap().pixi, Some(vec![depth; channels]));
            assert_eq!(bytes(&h, &file, 1), b"MAIN-DATA-0123");
        }
    }
    // Float: 16, 32 or 64 bits; the value 3 is reserved.
    for (log2, depth) in [(0u8, 16u8), (1, 32), (2, 64)] {
        let file = mini_file(&MiniSpec {
            float: true,
            float_log2: log2,
            ..MiniSpec::av1()
        });
        let h = go(&file);
        let mini = h.mini.as_ref().unwrap();
        assert!(mini.float);
        assert_eq!(mini.bit_depth, depth);
        assert_eq!(bytes(&h, &file, 1), b"MAIN-DATA-0123");
    }
    let m = MiniSpec {
        float: true,
        float_log2: 3,
        ..MiniSpec::av1()
    };
    assert_eq!(fail(&mini_file(&m)), Error::Malformed("mini"));
}

#[test]
fn cicp_defaults_and_explicit_values() {
    let cicp = |m: &MiniSpec| {
        let h = go(&mini_file(m));
        let mini = h.mini.as_ref().unwrap();
        (
            mini.colour_primaries,
            mini.transfer_characteristics,
            mini.matrix_coefficients,
        )
    };
    assert_eq!(cicp(&MiniSpec::av1()), (1, 13, 6));
    let mono = MiniSpec {
        chroma: 0,
        ..MiniSpec::av1()
    };
    assert_eq!(cicp(&mono), (1, 13, 2));
    let icc = MiniSpec {
        icc: b"ICC".to_vec(),
        ..MiniSpec::av1()
    };
    assert_eq!(cicp(&icc), (2, 2, 6));
    // `matrix_coefficients` is 8 bits even for 4:0:0 (libheif 1.23.7; libavif v1.4.2 skips it):
    // the data after the header is only found when the header is read this way.
    let explicit = MiniSpec {
        chroma: 0,
        cicp: Some((9, 16, 9)),
        ..MiniSpec::av1()
    };
    assert_eq!(cicp(&explicit), (9, 16, 9));
    let file = mini_file(&explicit);
    assert_eq!(bytes(&go(&file), &file, 1), explicit.main_data);
}

#[test]
fn orientations_are_irot_and_imir() {
    let mirror_lr = Transform::Mirror(Axis::LeftRight);
    let mirror_tb = Transform::Mirror(Axis::TopBottom);
    let want = [
        (1u8, vec![]),
        (2, vec![mirror_lr]),
        (3, vec![Transform::Rotate(180)]),
        (4, vec![mirror_tb]),
        // libheif: irot 270 and imir axis 1; libavif: irot 90 and imir axis 0 (the same transform).
        (5, vec![Transform::Rotate(270), mirror_lr]),
        (6, vec![Transform::Rotate(270)]),
        (7, vec![Transform::Rotate(270), mirror_tb]),
        (8, vec![Transform::Rotate(90)]),
    ];
    for (orientation, transforms) in want {
        let file = mini_file(&MiniSpec {
            orientation,
            ..MiniSpec::av1()
        });
        let h = go(&file);
        let img = h.primary_image().unwrap();
        assert_eq!(img.transforms, transforms, "orientation {orientation}");
        let size = if orientation <= 4 { (64, 48) } else { (48, 64) };
        assert_eq!(img.display_size(), Some(size), "orientation {orientation}");
        assert_eq!(h.mini.as_ref().unwrap().orientation, orientation);
    }
}

#[test]
fn hdr_properties_are_parsed_and_kept() {
    let hdr = MiniHdr {
        clli: Some(MiniClli {
            max_content_light_level: 1000,
            max_pic_average_light_level: 400,
        }),
        mdcv: Some(MiniMdcv {
            primaries: [(1, 2), (3, 4), (5, 6)],
            white_point: (7, 8),
            max_luminance: 9,
            min_luminance: 10,
        }),
        cclv: Some(MiniCclv {
            primaries: Some([(-1, 2), (3, -4), (5, 6)]),
            min_luminance: Some(7),
            max_luminance: None,
            avg_luminance: Some(9),
        }),
        amve: Some(MiniAmve {
            ambient_illumination: 100,
            ambient_light_x: 3,
            ambient_light_y: 4,
        }),
        reve: Some(MiniReve {
            values: [(1, 2, 3), (4, 5, 6)],
        }),
        ndwt: Some(203),
    };
    let m = MiniSpec {
        hdr: true,
        hdr_main: hdr,
        ..MiniSpec::av1()
    };
    let file = mini_file(&m);
    let h = go(&file);
    let mini = h.mini.as_ref().unwrap();
    assert!(mini.hdr && mini.gain_map.is_none());
    assert_eq!(mini.hdr_properties, hdr);
    // `reve` has no property (libheif skips it).
    assert_eq!(
        props(&h, 1)[4..],
        [
            (*b"clli", false),
            (*b"mdcv", false),
            (*b"cclv", false),
            (*b"amve", false),
            (*b"ndwt", false)
        ]
    );
    assert_eq!(h.properties[4].value, PropValue::Other);
    assert_eq!(bytes(&h, &file, 1), m.main_data);
}

#[test]
fn alpha_item_only_when_it_has_data() {
    let m = MiniSpec {
        alpha: true,
        premultiplied: true,
        ..MiniSpec::av1()
    };
    let file = mini_file(&m);
    let h = go(&file);
    assert_eq!(h.items.len(), 1);
    assert!(h.references.is_empty());
    let img = h.primary_image().unwrap();
    assert_eq!((img.alpha, img.premultiplied), (None, false));
    let mini = h.mini.as_ref().unwrap();
    assert!(mini.alpha && mini.alpha_premultiplied);
    assert_eq!(mini.alpha_data.length, 0);
    assert_eq!(bytes(&h, &file, 1), m.main_data);
}

#[test]
fn alpha_with_its_own_codec_configuration() {
    let alpha_cfg = vec![0x81, 0x0c, 0x0c, 0x00, 0x0a, 0x00];
    let m = MiniSpec {
        alpha: true,
        alpha_data: b"ALPHA-DATA-4567".to_vec(),
        alpha_cfg: alpha_cfg.clone(),
        ..MiniSpec::av1()
    };
    let file = mini_file(&m);
    let h = go(&file);
    let mini = h.mini.as_ref().unwrap();
    assert_ne!(mini.alpha_config, mini.main_config);
    assert_eq!(chunk(&file, mini.alpha_config), &alpha_cfg[..]);
    let first = h.item(2).unwrap().associations[0].index;
    let PropValue::Av1c(a) = h.properties[usize::from(first) - 1].value else {
        panic!("not an av1C: {:?}", h.properties)
    };
    assert_eq!(a.depth(), 8);
    assert_eq!(bytes(&h, &file, 2), m.alpha_data);
    assert_eq!(bytes(&h, &file, 1), m.main_data);
    // Not premultiplied: no `prem` reference.
    assert_eq!(h.references.len(), 1);
}

#[test]
fn xmp_may_be_compressed() {
    let m = MiniSpec {
        xmp: XMP.to_vec(),
        compressed: true,
        ..MiniSpec::av1()
    };
    let file = mini_file(&m);
    let h = go(&file);
    assert_eq!(h.item(7).unwrap().content_encoding, "deflate");
    assert!(h.mini.as_ref().unwrap().exif_xmp_compressed);
    // The bytes are the deflated data as they are.
    assert_eq!(bytes(&h, &file, 7), XMP);
    assert_eq!(h.primary_image().unwrap().xmp, [7]);
}

#[test]
fn exif_may_be_compressed() {
    let m = MiniSpec {
        exif: TIFF.to_vec(),
        compressed: true,
        ..MiniSpec::av1()
    };
    let file = mini_file(&m);
    let h = go(&file);
    assert_eq!(h.item(6).unwrap().content_encoding, "deflate");
}

#[test]
fn large_chunks() {
    let m = MiniSpec {
        main_data: vec![7; 40000],
        icc: vec![5; 2000],
        ..MiniSpec::hevc()
    };
    let file = mini_file(&m);
    let h = go(&file);
    let mini = h.mini.as_ref().unwrap();
    assert!(mini.large_item_data && mini.large_metadata && mini.large_codec_config);
    assert_eq!(mini.main_data.length, 40000);
    assert_eq!(chunk(&file, mini.icc_profile), &m.icc[..]);
    assert_eq!(bytes(&h, &file, 1).len(), 40000);
    // Every large flag set by hand, with small data.
    let m = MiniSpec {
        large: true,
        ..MiniSpec::full()
    };
    let file = mini_file(&m);
    let h = go(&file);
    let mini = h.mini.as_ref().unwrap();
    assert!(
        mini.large_dimensions
            && mini.large_metadata
            && mini.large_codec_config
            && mini.large_item_data
    );
    assert_eq!(bytes(&h, &file, 1), m.main_data);
    assert_eq!(bytes(&h, &file, 7), m.xmp);
}

#[test]
fn gain_map_makes_tmap_and_gain_map_items() {
    let m = MiniSpec::gain_map();
    let file = mini_file(&m);
    let h = go(&file);
    let ids: Vec<u32> = h.items.iter().map(|i| i.id).collect();
    assert_eq!(ids, [1, 3, 4]);
    let types: Vec<[u8; 4]> = h.items.iter().map(|i| i.item_type.0).collect();
    assert_eq!(types, [*b"av01", *b"tmap", *b"av01"]);
    let hidden: Vec<bool> = h.items.iter().map(|i| i.hidden).collect();
    assert_eq!(hidden, [false, false, true]);
    assert_eq!(h.references.len(), 1);
    assert_eq!(h.references[0].kind, FourCc(*b"dimg"));
    assert_eq!(
        (h.references[0].from, &h.references[0].to[..]),
        (3, &[1, 4][..])
    );
    assert_eq!(h.groups.len(), 1);
    assert_eq!(h.groups[0].kind, FourCc(*b"altr"));
    assert_eq!(h.groups[0].entities, [3, 1]);
    assert_eq!(
        kinds(&h),
        [
            *b"av1C", *b"ispe", *b"pixi", *b"colr", *b"clli", *b"av1C", *b"ispe", *b"pixi",
            *b"colr", *b"ispe", *b"colr", *b"ndwt"
        ]
    );
    assert_eq!(
        props(&h, 1),
        [
            (*b"av1C", true),
            (*b"ispe", false),
            (*b"pixi", false),
            (*b"colr", true),
            (*b"clli", false)
        ]
    );
    assert_eq!(
        props(&h, 3),
        [(*b"ispe", false), (*b"colr", true), (*b"ndwt", false)]
    );
    assert_eq!(
        props(&h, 4),
        [
            (*b"av1C", true),
            (*b"ispe", false),
            (*b"pixi", false),
            (*b"colr", true)
        ]
    );
    // The tmap item is as big as the base image; its ICC profile is the `tmap` one.
    assert_eq!(
        h.properties[9].value,
        PropValue::Ispe {
            width: 64,
            height: 48
        }
    );
    let PropValue::Colr(Colour::Icc(tmap_icc)) = h.properties[10].value else {
        panic!("not an ICC profile: {:?}", h.properties[10])
    };
    assert_eq!(chunk(&file, tmap_icc), &m.tmap_icc[..]);

    let base = h.image(1).unwrap();
    let gm = base.gain_map.as_ref().unwrap();
    assert_eq!(
        (gm.image, gm.tmap, gm.source),
        (4, Some(3), GainMapSource::Tmap)
    );
    let md = gm.metadata.unwrap();
    assert_eq!(
        (md.minimum_version, md.writer_version, md.flags),
        (0, 0, 0x08)
    );
    assert!(md.common_denominator);
    assert_eq!(md.alternate_headroom.numerator, 3000);
    // The `tmap` item data is the metadata chunk; its payload gets the version byte 0.
    assert_eq!(bytes(&h, &file, 3), m.gain_meta);
    assert!(matches!(
        h.item(3).unwrap().derived,
        Some(Ok(Derived::Tmap(Some(_))))
    ));
    assert_eq!(
        h.item(3).unwrap().data,
        Ok(vec![Extent {
            offset: find(&file, &m.gain_meta),
            length: m.gain_meta.len() as u64
        }])
    );
    assert_eq!(bytes(&h, &file, 4), m.gain_data);
    assert_eq!(bytes(&h, &file, 1), m.main_data);
    let gain = h.image(4).unwrap();
    assert_eq!((gain.width, gain.height), (32, 24));
    assert_eq!(gain.pixi, Some(vec![8]));
    assert_eq!(
        gain.colour,
        [Colour::Nclx {
            primaries: 2,
            transfer: 2,
            matrix: 6,
            full_range: true
        }]
    );
    assert!(matches!(
        gain.kind,
        ImageKind::Coded {
            codec: Codec::Av1,
            ..
        }
    ));
    assert_eq!(h.top_level_images(), [1]);

    let mini = h.mini.as_ref().unwrap();
    let g = mini.gain_map.as_ref().unwrap();
    assert!(mini.hdr && !g.same_dimensions);
    assert_eq!((g.width, g.height, g.matrix_coefficients), (32, 24, 6));
    assert!(g.full_range && !g.float);
    assert_eq!((g.chroma_subsampling, g.bit_depth), (0, 8));
    assert!(g.tmap_icc && !g.tmap_explicit_cicp && g.tmap_full_range);
    assert_eq!(
        (
            g.tmap_colour_primaries,
            g.tmap_transfer_characteristics,
            g.tmap_matrix_coefficients
        ),
        (1, 13, 6)
    );
    assert_eq!(g.hdr.ndwt, Some(203));
    assert_eq!(
        mini.hdr_properties.clli,
        Some(MiniClli {
            max_content_light_level: 1000,
            max_pic_average_light_level: 400
        })
    );
    assert_eq!(chunk(&file, mini.gain_map_metadata), &m.gain_meta[..]);
    assert_eq!(chunk(&file, mini.tmap_icc_profile), &m.tmap_icc[..]);
    assert_eq!(chunk(&file, mini.gain_map_data), &m.gain_data[..]);
    // A gain map configuration of size 0 is the main one.
    assert_eq!(mini.gain_map_config, mini.main_config);
}

#[test]
fn gain_map_dimension_same_as_main_flag_both_ways() {
    for same in [true, false] {
        let m = MiniSpec {
            gain: Some(GainSpec {
                same_dims: same,
                ..GainSpec::small()
            }),
            ..MiniSpec::gain_map()
        };
        let file = mini_file(&m);
        let h = go(&file);
        let g = h.mini.as_ref().unwrap().gain_map.unwrap();
        let want = if same { (64, 48) } else { (32, 24) };
        assert_eq!(g.same_dimensions, same);
        assert_eq!((g.width, g.height), want);
        let gain = h.image(4).unwrap();
        assert_eq!((gain.width, gain.height), want);
        // The fields after the dimensions are only found when the bits were counted right.
        assert_eq!(bytes(&h, &file, 4), m.gain_data);
        assert_eq!(bytes(&h, &file, 1), m.main_data);
        assert!(h.image(1).unwrap().gain_map.is_some());
    }
}

#[test]
fn gain_map_with_a_rotated_base_and_explicit_tmap_cicp() {
    let m = MiniSpec {
        orientation: 6,
        tmap_icc: Vec::new(),
        gain: Some(GainSpec {
            tmap_cicp: Some((9, 16, 9, false)),
            ..GainSpec::small()
        }),
        ..MiniSpec::gain_map()
    };
    let file = mini_file(&m);
    let h = go(&file);
    assert_eq!(
        kinds(&h),
        [
            *b"av1C", *b"ispe", *b"pixi", *b"colr", *b"clli", *b"irot", *b"av1C", *b"ispe",
            *b"pixi", *b"colr", *b"ispe", *b"colr"
        ]
    );
    // The gain map image is rotated like the base image; the tmap item is already rotated: its
    // size is swapped.
    assert_eq!(
        props(&h, 4),
        [
            (*b"av1C", true),
            (*b"ispe", false),
            (*b"pixi", false),
            (*b"colr", true),
            (*b"irot", true)
        ]
    );
    assert_eq!(props(&h, 3), [(*b"ispe", false), (*b"colr", true)]);
    assert_eq!(
        h.properties[10].value,
        PropValue::Ispe {
            width: 48,
            height: 64
        }
    );
    assert_eq!(
        h.properties[11].value,
        PropValue::Colr(Colour::Nclx {
            primaries: 9,
            transfer: 16,
            matrix: 9,
            full_range: false
        })
    );
    assert_eq!(h.image(4).unwrap().transforms, [Transform::Rotate(270)]);
    let g = h.mini.as_ref().unwrap().gain_map.unwrap();
    assert!(!g.tmap_icc && g.tmap_explicit_cicp && !g.tmap_full_range);
}

#[test]
fn gain_map_without_gain_map_data() {
    let m = MiniSpec {
        gain_data: Vec::new(),
        ..MiniSpec::gain_map()
    };
    let file = mini_file(&m);
    let h = go(&file);
    let ids: Vec<u32> = h.items.iter().map(|i| i.id).collect();
    assert_eq!(ids, [1, 3]);
    assert_eq!(h.references[0].to, [1]);
    // A tmap item with one input is no gain map.
    assert!(h.image(1).unwrap().gain_map.is_none());
}

#[test]
fn version_and_sizes_are_checked() {
    let v1 = MiniSpec {
        version: 1,
        ..MiniSpec::av1()
    };
    assert_eq!(fail(&mini_file(&v1)), Error::Unsupported("mini"));

    // Chunk sizes past the end of the payload.
    let mut p = mini_payload(&MiniSpec::full());
    p.pop();
    let file = mini_file_of(*b"avif", &bx(b"mini", &p));
    assert_eq!(fail(&file), Error::Malformed("mini"));
    assert_eq!(
        isobmff::read(
            Cursor::new(&file[..]),
            &Limits::default(),
            &AtomicBool::new(false)
        )
        .err(),
        Some(Error::Malformed("mini"))
    );
    // A header that ends early, and an empty one.
    let short: [&[u8]; 2] = [&[0, 0], &[]];
    for p in short {
        let file = mini_file_of(*b"avif", &bx(b"mini", p));
        assert_eq!(fail(&file), Error::Malformed("mini"));
    }
    // A box that runs past the end of the file.
    let mut file = mini_file(&MiniSpec::av1());
    file.pop();
    assert_eq!(fail(&file), Error::Truncated);
    assert_eq!(
        isobmff::read(
            Cursor::new(&file[..]),
            &Limits::default(),
            &AtomicBool::new(false)
        )
        .err(),
        Some(Error::Truncated)
    );
}

#[test]
fn trailing_bytes_are_tolerated() {
    let m = MiniSpec {
        trailing: vec![0xEE; 9],
        ..MiniSpec::full()
    };
    let file = mini_file(&m);
    let h = go(&file);
    assert_eq!(bytes(&h, &file, 1), m.main_data);
    assert_eq!(bytes(&h, &file, 7), m.xmp);
    let mini = h.mini.as_ref().unwrap();
    assert_eq!(
        mini.xmp_data.end().unwrap() + 9,
        mini.payload.end().unwrap()
    );
}

#[test]
fn one_mini_box_and_no_meta() {
    let p = mini_payload(&MiniSpec::av1());
    let one = bx(b"mini", &p);
    let meta = bx(b"meta", &[0; 4]);
    let two = cat(&[&one, &one]);
    assert_eq!(
        fail(&mini_file_of(*b"avif", &two)),
        Error::Malformed("mini")
    );
    for rest in [cat(&[&one, &meta]), cat(&[&meta, &one])] {
        let file = mini_file_of(*b"avif", &rest);
        assert_eq!(fail(&file), Error::Malformed("mini"));
        assert_eq!(
            isobmff::read(
                Cursor::new(&file[..]),
                &Limits::default(),
                &AtomicBool::new(false)
            )
            .err(),
            Some(Error::Malformed("mini"))
        );
    }
    // Boxes around it are skipped, and the brand is not checked: libheif takes any `mini`.
    let free = bx(b"free", &[0; 4]);
    let file = cat(&[
        &bx(b"ftyp", b"avifavifmif1"),
        &free,
        &one,
        &bx(b"mdat", &[1, 2]),
    ]);
    let h = go(&file);
    assert_eq!(h.ftyp.major, FourCc(*b"avif"));
    assert_eq!(h.primary_image().unwrap().width, 64);
}

#[test]
fn read_equals_parse_and_limits_apply() {
    let c = AtomicBool::new(false);
    let d = Limits::default();
    for m in [MiniSpec::full(), MiniSpec::hevc(), MiniSpec::gain_map()] {
        let file = mini_file(&m);
        let a = isobmff::parse(&file, &d, &c).unwrap();
        let b = isobmff::read(Cursor::new(&file[..]), &d, &c).unwrap();
        assert_eq!(a, b);
        assert_eq!(a.stats, b.stats);
        for it in &a.items {
            assert_eq!(a.image(it.id), b.image(it.id));
        }
        // max_alloc, max_meta_bytes (the mini box is copied like `meta`) and cancel.
        let tiny = Limits { max_alloc: 0, ..d };
        assert_eq!(
            isobmff::parse(&file, &tiny, &c).err(),
            Some(Error::LimitExceeded(Limit::Alloc))
        );
        assert_eq!(
            isobmff::read(Cursor::new(&file[..]), &tiny, &c).err(),
            Some(Error::LimitExceeded(Limit::Alloc))
        );
        let small = Limits {
            max_meta_bytes: 10,
            ..d
        };
        assert_eq!(
            isobmff::read(Cursor::new(&file[..]), &small, &c).err(),
            Some(Error::LimitExceeded(Limit::MetaBytes))
        );
        let stop = AtomicBool::new(true);
        assert_eq!(
            isobmff::parse(&file, &d, &stop).err(),
            Some(Error::Cancelled)
        );
        assert_eq!(
            isobmff::read(Cursor::new(&file[..]), &d, &stop).err(),
            Some(Error::Cancelled)
        );
    }
}

#[test]
fn no_panic_on_damaged_files() {
    let c = AtomicBool::new(false);
    let d = Limits::default();
    for m in [MiniSpec::full(), MiniSpec::gain_map()] {
        let file = mini_file(&m);
        let check = |f: &[u8]| {
            if let Ok(h) = isobmff::parse(f, &d, &c) {
                for it in &h.items {
                    let _ = h.image(it.id);
                    let _ = h.item_bytes(f, it.id, &d, &c);
                    let _ = h.metadata(f, it.id, &d, &c);
                }
            }
            let _ = isobmff::read(Cursor::new(f), &d, &c);
        };
        for n in 0..file.len() {
            check(&file[..n]);
        }
        for at in 0..file.len() * 8 {
            let mut f = file.clone();
            f[at / 8] ^= 0x80u8 >> (at % 8);
            check(&f);
        }
    }
}

/// The seeds of the `mini` fuzz target were assembled by hand, bit by bit.
#[test]
fn the_fuzz_seeds_parse() {
    let file = |seed: &[u8]| mini_file_of(*b"avif", &bx(b"mini", seed));

    let f = file(include_bytes!("fuzz/payload-seeds/mini/av1"));
    let h = go(&f);
    assert_eq!(h.items.len(), 1);
    assert_eq!(bytes(&h, &f, 1), b"data");
    let img = h.primary_image().unwrap();
    assert_eq!(
        (img.width, img.height, img.pixi),
        (32, 32, Some(vec![8, 8, 8]))
    );

    let f = file(include_bytes!("fuzz/payload-seeds/mini/alpha"));
    let h = go(&f);
    let ids: Vec<u32> = h.items.iter().map(|i| i.id).collect();
    assert_eq!(ids, [1, 2]);
    assert_eq!(bytes(&h, &f, 1), b"DATA");
    assert_eq!(bytes(&h, &f, 2), b"AA");
    let img = h.primary_image().unwrap();
    assert_eq!((img.width, img.height, img.alpha), (16, 16, Some(2)));
    assert_eq!(h.mini.as_ref().unwrap().chroma_subsampling, 3);

    let f = file(include_bytes!("fuzz/payload-seeds/mini/gain-map"));
    let h = go(&f);
    let ids: Vec<u32> = h.items.iter().map(|i| i.id).collect();
    assert_eq!(ids, [1, 3, 4]);
    assert_eq!(bytes(&h, &f, 1), b"DATA");
    assert_eq!(bytes(&h, &f, 4), b"GG");
    // An empty gain map metadata chunk: the tmap payload is the version byte alone.
    assert!(bytes(&h, &f, 3).is_empty());
    assert_eq!(
        h.item(3).unwrap().derived,
        Some(Err(Error::Malformed("tmap")))
    );
    let g = h.mini.as_ref().unwrap().gain_map.unwrap();
    assert!(g.same_dimensions);
    assert_eq!((g.width, g.height, g.matrix_coefficients), (16, 16, 6));
}
