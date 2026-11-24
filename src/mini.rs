//! The `mini` box (ISO/IEC 23008-12 Annex O, brand `mif3`): a whole image in one top-level box,
//! with no `meta`. [`parse`] reads its bit-packed header and synthesises the same `Meta` that a
//! `meta` box would give, so the item graph code is shared.
//!
//! The bit syntax is the one of libheif 1.23.7 `mini.cc` (the current working draft), which
//! differs from libavif v1.4.2 in two places (docs/design.md, "The `mini` box"): `matrix_coefficients`
//! is always 8 bits when `explicit_cicp_flag` is set, and `gainmap_dimension_same_as_main_item_flag`
//! is present. The item and property layout follows libheif, plus the gain map items of libavif
//! (`avifParseMiniBox`, BSD-2-Clause, see NOTICE; no code of either library is copied).

use crate::boxes::{BoxHeader, Ctx};
use crate::budget::Budget;
use crate::ftyp::Ftyp;
use crate::meta::iinf::ItemInfo;
use crate::meta::iloc::IlocItem;
use crate::meta::iprp::Entries;
use crate::meta::{Association, EntityGroup, Meta, Reference};
use crate::props::{Axis, Colour, PropValue, Property, parse_value};
use crate::{Error, Extent, FourCc};

const BAD: Error = Error::Malformed("mini");
const ALPHA_URN: &str = "urn:mpeg:mpegB:cicp:systems:auxiliary:alpha";
const XMP_TYPE: &str = "application/rdf+xml";

/// `clli` of a mini header.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MiniClli {
    /// `max_content_light_level`.
    pub max_content_light_level: u16,
    /// `max_pic_average_light_level`.
    pub max_pic_average_light_level: u16,
}

/// `mdcv` of a mini header.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MiniMdcv {
    /// `display_primaries` (x, y) of the three primaries.
    pub primaries: [(u16, u16); 3],
    /// `white_point` (x, y).
    pub white_point: (u16, u16),
    /// `max_display_mastering_luminance`.
    pub max_luminance: u32,
    /// `min_display_mastering_luminance`.
    pub min_luminance: u32,
}

/// `cclv` of a mini header; each part is present when its flag is set.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MiniCclv {
    /// `ccv_primaries` (x, y) of the three primaries.
    pub primaries: Option<[(i32, i32); 3]>,
    /// `ccv_min_luminance_value`.
    pub min_luminance: Option<u32>,
    /// `ccv_max_luminance_value`.
    pub max_luminance: Option<u32>,
    /// `ccv_avg_luminance_value`.
    pub avg_luminance: Option<u32>,
}

/// `amve` of a mini header.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MiniAmve {
    /// `ambient_illuminance`.
    pub ambient_illumination: u32,
    /// `ambient_light_x`.
    pub ambient_light_x: u16,
    /// `ambient_light_y`.
    pub ambient_light_y: u16,
}

/// `reve` of a mini header: two (illuminance, x, y) triples, kept raw (libheif skips them).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MiniReve {
    /// The two triples in bit order.
    pub values: [(u32, u16, u16); 2],
}

/// The HDR property block of a mini header: each entry is present when its flag was set.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct MiniHdr {
    /// `clli`.
    pub clli: Option<MiniClli>,
    /// `mdcv`.
    pub mdcv: Option<MiniMdcv>,
    /// `cclv`.
    pub cclv: Option<MiniCclv>,
    /// `amve`.
    pub amve: Option<MiniAmve>,
    /// `reve`.
    pub reve: Option<MiniReve>,
    /// `ndwt` (`diffuse_white_luminance`).
    pub ndwt: Option<u32>,
}

/// The gain map part of a mini header (`hdr_flag` and `gainmap_flag`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MiniGainMap {
    /// `gainmap_dimension_same_as_main_item_flag`.
    pub same_dimensions: bool,
    /// Gain map image width.
    pub width: u32,
    /// Gain map image height.
    pub height: u32,
    /// `gainmap_matrix_coefficients`.
    pub matrix_coefficients: u8,
    /// `gainmap_full_range_flag`.
    pub full_range: bool,
    /// `gainmap_chroma_subsampling` (0 = 4:0:0, 1 = 4:2:0, 2 = 4:2:2, 3 = 4:4:4).
    pub chroma_subsampling: u8,
    /// `gainmap_chroma_is_horizontally_centered` (read for 4:2:0 and 4:2:2).
    pub chroma_horizontally_centred: bool,
    /// `gainmap_chroma_is_vertically_centered` (read for 4:2:0).
    pub chroma_vertically_centred: bool,
    /// `gainmap_float_flag`.
    pub float: bool,
    /// Gain map bits per sample (8, 9..16, or 16/32/64 when `float`).
    pub bit_depth: u8,
    /// `tmap_icc_flag`.
    pub tmap_icc: bool,
    /// `tmap_explicit_cicp_flag`.
    pub tmap_explicit_cicp: bool,
    /// `tmap_colour_primaries` (1 when not explicit).
    pub tmap_colour_primaries: u8,
    /// `tmap_transfer_characteristics` (13 when not explicit).
    pub tmap_transfer_characteristics: u8,
    /// `tmap_matrix_coefficients` (6 when not explicit).
    pub tmap_matrix_coefficients: u8,
    /// `tmap_full_range_flag` (set when not explicit).
    pub tmap_full_range: bool,
    /// The HDR properties of the `tmap` item.
    pub hdr: MiniHdr,
}

/// A parsed `mini` box: every header field and the file range of every chunk. A chunk that is
/// absent has length 0. Absolute file offsets throughout.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Mini {
    /// The whole box.
    pub range: Extent,
    /// The payload after the box header.
    pub payload: Extent,
    /// `explicit_codec_types_flag`.
    pub explicit_codec_types: bool,
    /// `float_flag`.
    pub float: bool,
    /// `full_range_flag`.
    pub full_range: bool,
    /// `alpha_flag`.
    pub alpha: bool,
    /// `explicit_cicp_flag`.
    pub explicit_cicp: bool,
    /// `hdr_flag`.
    pub hdr: bool,
    /// `icc_flag`.
    pub icc: bool,
    /// `exif_flag`.
    pub exif: bool,
    /// `xmp_flag`.
    pub xmp: bool,
    /// `chroma_subsampling` (0 = 4:0:0, 1 = 4:2:0, 2 = 4:2:2, 3 = 4:4:4).
    pub chroma_subsampling: u8,
    /// `orientation_minus1` + 1: the Exif orientation 1..=8.
    pub orientation: u8,
    /// `large_dimensions_flag`.
    pub large_dimensions: bool,
    /// Image width.
    pub width: u32,
    /// Image height.
    pub height: u32,
    /// `chroma_is_horizontally_centered` (read for 4:2:0 and 4:2:2).
    pub chroma_horizontally_centred: bool,
    /// `chroma_is_vertically_centered` (read for 4:2:0).
    pub chroma_vertically_centred: bool,
    /// Bits per sample (8, 9..16, or 16/32/64 when `float`).
    pub bit_depth: u8,
    /// `alpha_is_premultiplied` (read when `alpha`).
    pub alpha_premultiplied: bool,
    /// `colour_primaries` (the default when not explicit).
    pub colour_primaries: u8,
    /// `transfer_characteristics` (the default when not explicit).
    pub transfer_characteristics: u8,
    /// `matrix_coefficients` (the default when not explicit).
    pub matrix_coefficients: u8,
    /// The item type of the coded items: `infe_type` when explicit, else from the `ftyp` minor
    /// version (`avif` gives `av01`, `heic` gives `hvc1`).
    pub infe_type: FourCc,
    /// The type of the codec configuration property (`codec_config_type`, `av1C` or `hvcC`).
    pub codec_config_type: FourCc,
    /// The gain map, when `hdr` and `gainmap_flag` are set.
    pub gain_map: Option<MiniGainMap>,
    /// The HDR properties of the main item (read when `hdr`).
    pub hdr_properties: MiniHdr,
    /// `large_metadata_flag`.
    pub large_metadata: bool,
    /// `large_codec_config_flag`.
    pub large_codec_config: bool,
    /// `large_item_data_flag`.
    pub large_item_data: bool,
    /// `exif_xmp_compressed_flag`.
    pub exif_xmp_compressed: bool,
    /// The codec configuration of the main item.
    pub main_config: Extent,
    /// The codec configuration of the alpha item (the main one when its size is 0).
    pub alpha_config: Extent,
    /// The codec configuration of the gain map item (the main one when its size is 0).
    pub gain_map_config: Extent,
    /// The ICC profile of the main item.
    pub icc_profile: Extent,
    /// The ICC profile of the `tmap` item.
    pub tmap_icc_profile: Extent,
    /// The gain map metadata (the `tmap` item payload after its version byte).
    pub gain_map_metadata: Extent,
    /// The alpha item data.
    pub alpha_data: Extent,
    /// The gain map item data.
    pub gain_map_data: Extent,
    /// The main item data.
    pub main_data: Extent,
    /// The Exif item data (no `exif_tiff_header_offset`).
    pub exif_data: Extent,
    /// The XMP item data.
    pub xmp_data: Extent,
}

/// A most-significant-bit-first reader. A read past the end is `Malformed("mini")`.
struct Bits<'a> {
    buf: &'a [u8],
    byte: usize,
    bit: usize,
}

const MASKS: [u8; 8] = [0x80, 0x40, 0x20, 0x10, 0x08, 0x04, 0x02, 0x01];

impl<'a> Bits<'a> {
    fn new(buf: &'a [u8]) -> Self {
        Self {
            buf,
            byte: 0,
            bit: 0,
        }
    }

    fn flag(&mut self) -> Result<bool, Error> {
        let b = *self.buf.get(self.byte).ok_or(BAD)?;
        let m = *MASKS.get(self.bit).ok_or(BAD)?;
        self.bit = self.bit.saturating_add(1);
        if self.bit >= 8 {
            self.bit = 0;
            self.byte = self.byte.saturating_add(1);
        }
        Ok(b & m != 0)
    }

    /// The next `n` bits (at most 32) as a number.
    fn bits(&mut self, n: u32) -> Result<u32, Error> {
        let mut v: u32 = 0;
        for _ in 0..n {
            let b = u32::from(self.flag()?);
            v = v.checked_mul(2).and_then(|x| x.checked_add(b)).ok_or(BAD)?;
        }
        Ok(v)
    }

    fn u8(&mut self, n: u32) -> Result<u8, Error> {
        u8::try_from(self.bits(n)?).map_err(|_| BAD)
    }

    fn u16(&mut self) -> Result<u16, Error> {
        u16::try_from(self.bits(16)?).map_err(|_| BAD)
    }

    fn skip(&mut self, n: u32) -> Result<(), Error> {
        for _ in 0..n {
            self.flag()?;
        }
        Ok(())
    }

    /// Moves to the next byte boundary (the padding bits are not checked) and gives the byte index.
    fn align(&mut self) -> usize {
        if self.bit != 0 {
            self.bit = 0;
            self.byte = self.byte.saturating_add(1);
        }
        self.byte
    }
}

/// Width and height minus one, 7 or 15 bits each.
fn dims(b: &mut Bits<'_>, large: bool) -> Result<(u32, u32), Error> {
    let n = if large { 15 } else { 7 };
    let w = b.bits(n)?.saturating_add(1);
    let h = b.bits(n)?.saturating_add(1);
    Ok((w, h))
}

/// The chroma centring flags: horizontal for 4:2:0 and 4:2:2, vertical for 4:2:0.
fn centred(b: &mut Bits<'_>, chroma_subsampling: u8) -> Result<(bool, bool), Error> {
    let h = if chroma_subsampling == 1 || chroma_subsampling == 2 {
        b.flag()?
    } else {
        false
    };
    let v = if chroma_subsampling == 1 {
        b.flag()?
    } else {
        false
    };
    Ok((h, v))
}

/// The bit depth: log2 minus 4 in 2 bits for a float (value 3 is reserved), else a high bit depth
/// flag and minus 9 in 3 bits.
fn depth(b: &mut Bits<'_>, float: bool) -> Result<u8, Error> {
    if float {
        match b.bits(2)? {
            0 => Ok(16),
            1 => Ok(32),
            2 => Ok(64),
            _ => Err(BAD),
        }
    } else if b.flag()? {
        u8::try_from(b.bits(3)?.saturating_add(9)).map_err(|_| BAD)
    } else {
        Ok(8)
    }
}

fn opt32(b: &mut Bits<'_>, present: bool) -> Result<Option<u32>, Error> {
    if present {
        Ok(Some(b.bits(32)?))
    } else {
        Ok(None)
    }
}

/// Six flags (`clli mdcv cclv amve reve ndwt`), then the present payloads in that order.
fn hdr_block(b: &mut Bits<'_>) -> Result<MiniHdr, Error> {
    let clli = b.flag()?;
    let mdcv = b.flag()?;
    let cclv = b.flag()?;
    let amve = b.flag()?;
    let reve = b.flag()?;
    let ndwt = b.flag()?;
    let mut h = MiniHdr::default();
    if clli {
        h.clli = Some(MiniClli {
            max_content_light_level: b.u16()?,
            max_pic_average_light_level: b.u16()?,
        });
    }
    if mdcv {
        let mut primaries = [(0u16, 0u16); 3];
        for p in &mut primaries {
            *p = (b.u16()?, b.u16()?);
        }
        h.mdcv = Some(MiniMdcv {
            primaries,
            white_point: (b.u16()?, b.u16()?),
            max_luminance: b.bits(32)?,
            min_luminance: b.bits(32)?,
        });
    }
    if cclv {
        b.skip(2)?; // ccv_cancel_flag, ccv_persistence_flag
        let primaries_present = b.flag()?;
        let min_present = b.flag()?;
        let max_present = b.flag()?;
        let avg_present = b.flag()?;
        b.skip(2)?;
        let primaries = if primaries_present {
            let mut p = [(0i32, 0i32); 3];
            for e in &mut p {
                *e = (b.bits(32)? as i32, b.bits(32)? as i32);
            }
            Some(p)
        } else {
            None
        };
        h.cclv = Some(MiniCclv {
            primaries,
            min_luminance: opt32(b, min_present)?,
            max_luminance: opt32(b, max_present)?,
            avg_luminance: opt32(b, avg_present)?,
        });
    }
    if amve {
        h.amve = Some(MiniAmve {
            ambient_illumination: b.bits(32)?,
            ambient_light_x: b.u16()?,
            ambient_light_y: b.u16()?,
        });
    }
    if reve {
        let mut values = [(0u32, 0u16, 0u16); 2];
        for v in &mut values {
            *v = (b.bits(32)?, b.u16()?, b.u16()?);
        }
        h.reve = Some(MiniReve { values });
    }
    if ndwt {
        h.ndwt = Some(b.bits(32)?);
    }
    Ok(h)
}

/// The `irot` angle and `imir` axis of an Exif orientation (libheif: 5 is irot 270 and imir axis
/// 1, where libavif has irot 90 and imir axis 0, the same transform).
fn orientation_transforms(o: u8) -> (Option<u16>, Option<Axis>) {
    match o {
        2 => (None, Some(Axis::LeftRight)),
        3 => (Some(180), None),
        4 => (None, Some(Axis::TopBottom)),
        5 => (Some(270), Some(Axis::LeftRight)),
        6 => (Some(270), None),
        7 => (Some(270), Some(Axis::TopBottom)),
        8 => (Some(90), None),
        _ => (None, None),
    }
}

/// Adds a property and gives its 1-based index.
fn push(
    props: &mut Vec<Property>,
    kind: &[u8; 4],
    range: Extent,
    value: PropValue,
) -> Result<u16, Error> {
    props.push(Property {
        kind: FourCc(*kind),
        range,
        value,
    });
    u16::try_from(props.len()).map_err(|_| BAD)
}

/// Adds the codec configuration property that is the chunk `e`, parsed as the 4cc `kind` says.
fn push_config(
    ctx: &Ctx<'_>,
    props: &mut Vec<Property>,
    kind: FourCc,
    e: Extent,
) -> Result<u16, Error> {
    let bytes = ctx.slice(e.offset..e.end().ok_or(BAD)?)?;
    let value = parse_value(kind, bytes, e.offset, ctx.budget)?;
    props.push(Property {
        kind,
        range: e,
        value,
    });
    u16::try_from(props.len()).map_err(|_| BAD)
}

fn add(list: &mut Vec<Association>, index: u16, essential: bool) {
    list.push(Association { index, essential });
}

/// The properties of an HDR block that libheif and libavif know (`reve` has none), not essential.
fn push_hdr(
    props: &mut Vec<Property>,
    h: &MiniHdr,
    at: Extent,
    list: &mut Vec<Association>,
) -> Result<(), Error> {
    for (present, kind) in [
        (h.clli.is_some(), b"clli"),
        (h.mdcv.is_some(), b"mdcv"),
        (h.cclv.is_some(), b"cclv"),
        (h.amve.is_some(), b"amve"),
        (h.ndwt.is_some(), b"ndwt"),
    ] {
        if present {
            let i = push(props, kind, at, PropValue::Other)?;
            add(list, i, false);
        }
    }
    Ok(())
}

fn pixi_bits(
    budget: &Budget<'_>,
    chroma_subsampling: u8,
    sample_bits: u8,
) -> Result<Vec<u8>, Error> {
    let n = if chroma_subsampling == 0 { 1 } else { 3 };
    let mut v = budget.vec::<u8>(n)?;
    v.resize(n, sample_bits);
    Ok(v)
}

fn info(id: u32, kind: &[u8; 4]) -> ItemInfo {
    ItemInfo {
        id,
        item_type: FourCc(*kind),
        name: String::new(),
        content_type: String::new(),
        content_encoding: String::new(),
        uri_type: String::new(),
        hidden: false,
        protection_index: 0,
    }
}

fn loc(budget: &Budget<'_>, id: u32, e: Extent) -> Result<IlocItem, Error> {
    // An empty chunk has no extent: an extent of length 0 would mean "to the end of the file".
    let mut extents = budget.vec::<Extent>(1)?;
    if e.length > 0 {
        extents.push(e);
    }
    Ok(IlocItem {
        id,
        method: 0,
        data_reference_index: 0,
        base_offset: 0,
        extents,
        extent_index: Vec::new(),
    })
}

fn ids(budget: &Budget<'_>, v: &[u32]) -> Result<Vec<u32>, Error> {
    let mut o = budget.vec::<u32>(v.len())?;
    o.extend_from_slice(v);
    Ok(o)
}

fn reference(
    budget: &Budget<'_>,
    kind: &[u8; 4],
    from: u32,
    to: &[u32],
) -> Result<Reference, Error> {
    Ok(Reference {
        kind: FourCc(*kind),
        from,
        to: ids(budget, to)?,
    })
}

/// Parses the top-level `mini` box `hdr` and synthesises its item graph input. `ftyp` gives the
/// codec when the box has no explicit codec types.
///
/// Item 1 is the primary image, item 2 its alpha plane (when it has data), items 3 (`tmap`) and 4
/// (gain map image) with an `altr` group, item 6 `Exif`, item 7 XMP (`mime`): the ids of libheif.
pub(crate) fn parse(ctx: &Ctx<'_>, hdr: &BoxHeader, ftyp: &Ftyp) -> Result<(Meta, Mini), Error> {
    let budget = ctx.budget;
    let payload = hdr.payload();
    let bytes = ctx.slice(payload.clone())?;
    let mut b = Bits::new(bytes);

    if b.bits(2)? != 0 {
        return Err(Error::Unsupported("mini"));
    }
    let explicit_codec_types = b.flag()?;
    let float = b.flag()?;
    let full_range = b.flag()?;
    let alpha = b.flag()?;
    let explicit_cicp = b.flag()?;
    let has_hdr = b.flag()?;
    let icc = b.flag()?;
    let exif = b.flag()?;
    let xmp = b.flag()?;
    let chroma_subsampling = b.u8(2)?;
    let orientation = b.u8(3)?.saturating_add(1);
    let large_dimensions = b.flag()?;
    let (width, height) = dims(&mut b, large_dimensions)?;
    let (chroma_h, chroma_v) = centred(&mut b, chroma_subsampling)?;
    let bit_depth = depth(&mut b, float)?;
    let alpha_premultiplied = if alpha { b.flag()? } else { false };
    let (cp, tc, mc) = if explicit_cicp {
        (b.u8(8)?, b.u8(8)?, b.u8(8)?)
    } else {
        (
            if icc { 2 } else { 1 },
            if icc { 2 } else { 13 },
            if chroma_subsampling == 0 { 2 } else { 6 },
        )
    };
    let (explicit_infe, explicit_config) = if explicit_codec_types {
        (
            FourCc(b.bits(32)?.to_be_bytes()),
            FourCc(b.bits(32)?.to_be_bytes()),
        )
    } else {
        (FourCc([0; 4]), FourCc([0; 4]))
    };

    let mut gain_map: Option<MiniGainMap> = None;
    let mut hdr_properties = MiniHdr::default();
    if has_hdr && b.flag()? {
        let same_dimensions = b.flag()?;
        let (gw, gh) = if same_dimensions {
            (width, height)
        } else {
            dims(&mut b, large_dimensions)?
        };
        let matrix_coefficients = b.u8(8)?;
        let g_full_range = b.flag()?;
        let g_chroma = b.u8(2)?;
        let (g_h, g_v) = centred(&mut b, g_chroma)?;
        let g_float = b.flag()?;
        let g_depth = depth(&mut b, g_float)?;
        let tmap_icc = b.flag()?;
        let tmap_explicit_cicp = b.flag()?;
        let (tcp, ttc, tmc, tfr) = if tmap_explicit_cicp {
            (b.u8(8)?, b.u8(8)?, b.u8(8)?, b.flag()?)
        } else {
            (1, 13, 6, true)
        };
        gain_map = Some(MiniGainMap {
            same_dimensions,
            width: gw,
            height: gh,
            matrix_coefficients,
            full_range: g_full_range,
            chroma_subsampling: g_chroma,
            chroma_horizontally_centred: g_h,
            chroma_vertically_centred: g_v,
            float: g_float,
            bit_depth: g_depth,
            tmap_icc,
            tmap_explicit_cicp,
            tmap_colour_primaries: tcp,
            tmap_transfer_characteristics: ttc,
            tmap_matrix_coefficients: tmc,
            tmap_full_range: tfr,
            hdr: MiniHdr::default(),
        });
    }
    if has_hdr {
        hdr_properties = hdr_block(&mut b)?;
        if let Some(g) = gain_map.as_mut() {
            g.hdr = hdr_block(&mut b)?;
        }
    }

    let large_metadata = if icc || exif || xmp || gain_map.is_some() {
        b.flag()?
    } else {
        false
    };
    let large_codec_config = b.flag()?;
    let large_item_data = b.flag()?;
    let meta_bits = if large_metadata { 20 } else { 10 };
    let config_bits = if large_codec_config { 12 } else { 3 };
    let data_bits = if large_item_data { 28 } else { 15 };
    let icc_size = if icc {
        b.bits(meta_bits)?.saturating_add(1)
    } else {
        0
    };
    let tmap_icc_size = match &gain_map {
        Some(g) if g.tmap_icc => b.bits(meta_bits)?.saturating_add(1),
        _ => 0,
    };
    let (gain_meta_size, gain_data_size, gain_config_size) = if gain_map.is_some() {
        let m = b.bits(meta_bits)?;
        let d = b.bits(data_bits)?;
        let c = if d > 0 { b.bits(config_bits)? } else { 0 };
        (m, d, c)
    } else {
        (0, 0, 0)
    };
    let main_config_size = b.bits(config_bits)?;
    let main_data_size = b.bits(data_bits)?.saturating_add(1);
    let alpha_data_size = if alpha { b.bits(data_bits)? } else { 0 };
    let alpha_config_size = if alpha_data_size > 0 {
        b.bits(config_bits)?
    } else {
        0
    };
    let exif_xmp_compressed = if exif || xmp { b.flag()? } else { false };
    let exif_size = if exif {
        b.bits(meta_bits)?.saturating_add(1)
    } else {
        0
    };
    let xmp_size = if xmp {
        b.bits(meta_bits)?.saturating_add(1)
    } else {
        0
    };
    let header_len = u64::try_from(b.align()).map_err(|_| BAD)?;

    // The chunks follow the header in this order; together they must fit in the payload (bytes
    // after the last one are ignored).
    let mut pos = payload.start.checked_add(header_len).ok_or(BAD)?;
    let mut take = |n: u32| -> Result<Extent, Error> {
        let e = Extent {
            offset: pos,
            length: u64::from(n),
        };
        pos = e.end().filter(|&end| end <= payload.end).ok_or(BAD)?;
        Ok(e)
    };
    let main_config = take(main_config_size)?;
    let alpha_config = if alpha_data_size == 0 {
        Extent::default()
    } else if alpha_config_size == 0 {
        main_config
    } else {
        take(alpha_config_size)?
    };
    let gain_map_config = if gain_data_size == 0 {
        Extent::default()
    } else if gain_config_size == 0 {
        main_config
    } else {
        take(gain_config_size)?
    };
    let icc_profile = take(icc_size)?;
    let tmap_icc_profile = take(tmap_icc_size)?;
    let gain_map_metadata = take(gain_meta_size)?;
    let alpha_data = take(alpha_data_size)?;
    let gain_map_data = take(gain_data_size)?;
    let main_data = take(main_data_size)?;
    let exif_data = take(exif_size)?;
    let xmp_data = take(xmp_size)?;

    let (infe_type, codec_config_type) = if explicit_codec_types {
        (explicit_infe, explicit_config)
    } else {
        match &ftyp.minor_version.to_be_bytes() {
            b"avif" => (FourCc(*b"av01"), FourCc(*b"av1C")),
            b"heic" => (FourCc(*b"hvc1"), FourCc(*b"hvcC")),
            _ => return Err(Error::Unsupported("mini codec")),
        }
    };

    // ---- the graph ----

    // A property that has no bytes of its own points at the start of the payload.
    let at = Extent {
        offset: payload.start,
        length: 0,
    };
    let have_alpha = alpha_data_size > 0;
    let have_gain_item = gain_data_size > 0;
    let (rot, mir) = orientation_transforms(orientation);

    let mut props = budget.vec::<Property>(32)?;
    let mut main_list = budget.vec::<Association>(16)?;
    if main_config.length > 0 {
        let i = push_config(ctx, &mut props, codec_config_type, main_config)?;
        add(&mut main_list, i, true);
    }
    let ispe = push(&mut props, b"ispe", at, PropValue::Ispe { width, height })?;
    add(&mut main_list, ispe, false);
    let pixi = push(
        &mut props,
        b"pixi",
        at,
        PropValue::Pixi {
            bits: pixi_bits(budget, chroma_subsampling, bit_depth)?,
        },
    )?;
    add(&mut main_list, pixi, false);
    let nclx = push(
        &mut props,
        b"colr",
        at,
        PropValue::Colr(Colour::Nclx {
            primaries: u16::from(cp),
            transfer: u16::from(tc),
            matrix: u16::from(mc),
            full_range,
        }),
    )?;
    add(&mut main_list, nclx, true);
    if icc {
        let i = push(
            &mut props,
            b"colr",
            icc_profile,
            PropValue::Colr(Colour::Icc(icc_profile)),
        )?;
        add(&mut main_list, i, true);
    }
    let mut alpha_list = budget.vec::<Association>(8)?;
    if have_alpha {
        if alpha_config.length > 0 {
            let i = push_config(ctx, &mut props, codec_config_type, alpha_config)?;
            add(&mut alpha_list, i, true);
        }
        add(&mut alpha_list, ispe, false);
        let i = push(
            &mut props,
            b"auxC",
            at,
            PropValue::AuxC {
                aux_type: budget.arc_str(ALPHA_URN.as_bytes())?,
                subtype: at,
            },
        )?;
        add(&mut alpha_list, i, true);
    }
    push_hdr(&mut props, &hdr_properties, at, &mut main_list)?;
    let mut rot_index: Option<u16> = None;
    let mut mir_index: Option<u16> = None;
    if let Some(angle) = rot {
        let i = push(&mut props, b"irot", at, PropValue::Irot { angle })?;
        add(&mut main_list, i, true);
        if have_alpha {
            add(&mut alpha_list, i, true);
        }
        rot_index = Some(i);
    }
    if let Some(axis) = mir {
        let i = push(&mut props, b"imir", at, PropValue::Imir { axis })?;
        add(&mut main_list, i, true);
        if have_alpha {
            add(&mut alpha_list, i, true);
        }
        mir_index = Some(i);
    }

    let mut gain_list = budget.vec::<Association>(8)?;
    let mut tmap_list = budget.vec::<Association>(8)?;
    if let Some(g) = &gain_map {
        if have_gain_item {
            if gain_map_config.length > 0 {
                let i = push_config(ctx, &mut props, codec_config_type, gain_map_config)?;
                add(&mut gain_list, i, true);
            }
            let i = push(
                &mut props,
                b"ispe",
                at,
                PropValue::Ispe {
                    width: g.width,
                    height: g.height,
                },
            )?;
            add(&mut gain_list, i, false);
            let i = push(
                &mut props,
                b"pixi",
                at,
                PropValue::Pixi {
                    bits: pixi_bits(budget, g.chroma_subsampling, g.bit_depth)?,
                },
            )?;
            add(&mut gain_list, i, false);
            let i = push(
                &mut props,
                b"colr",
                at,
                PropValue::Colr(Colour::Nclx {
                    primaries: 2,
                    transfer: 2,
                    matrix: u16::from(g.matrix_coefficients),
                    full_range: g.full_range,
                }),
            )?;
            add(&mut gain_list, i, true);
            // The same transforms as the base image, shared with it.
            for i in [rot_index, mir_index].into_iter().flatten() {
                add(&mut gain_list, i, true);
            }
        }
        // The `tmap` item is the oriented image: its size is swapped for orientations 5 to 8.
        let (tw, th) = if orientation <= 4 {
            (width, height)
        } else {
            (height, width)
        };
        let i = push(
            &mut props,
            b"ispe",
            at,
            PropValue::Ispe {
                width: tw,
                height: th,
            },
        )?;
        add(&mut tmap_list, i, false);
        if g.tmap_explicit_cicp || !g.tmap_icc {
            let i = push(
                &mut props,
                b"colr",
                at,
                PropValue::Colr(Colour::Nclx {
                    primaries: u16::from(g.tmap_colour_primaries),
                    transfer: u16::from(g.tmap_transfer_characteristics),
                    matrix: u16::from(g.tmap_matrix_coefficients),
                    full_range: g.tmap_full_range,
                }),
            )?;
            add(&mut tmap_list, i, true);
        }
        if g.tmap_icc {
            let i = push(
                &mut props,
                b"colr",
                tmap_icc_profile,
                PropValue::Colr(Colour::Icc(tmap_icc_profile)),
            )?;
            add(&mut tmap_list, i, true);
        }
        push_hdr(&mut props, &g.hdr, at, &mut tmap_list)?;
    }

    let mut assoc: Entries = budget.vec::<(u32, Vec<Association>)>(4)?;
    assoc.push((1, main_list));
    if have_alpha {
        assoc.push((2, alpha_list));
    }
    if gain_map.is_some() {
        assoc.push((3, tmap_list));
    }
    if have_gain_item {
        assoc.push((4, gain_list));
    }

    let mut items = budget.vec::<ItemInfo>(6)?;
    let mut iloc = budget.vec::<IlocItem>(6)?;
    let mut refs = budget.vec::<Reference>(6)?;
    items.push(info(1, &infe_type.0));
    iloc.push(loc(budget, 1, main_data)?);
    if have_alpha {
        items.push(info(2, &infe_type.0));
        iloc.push(loc(budget, 2, alpha_data)?);
        refs.push(reference(budget, b"auxl", 2, &[1])?);
        if alpha_premultiplied {
            refs.push(reference(budget, b"prem", 1, &[2])?);
        }
    }
    let mut groups = budget.vec::<EntityGroup>(1)?;
    if gain_map.is_some() {
        items.push(info(3, b"tmap"));
        iloc.push(loc(budget, 3, gain_map_metadata)?);
        if have_gain_item {
            let mut m = info(4, &infe_type.0);
            m.hidden = true;
            items.push(m);
            iloc.push(loc(budget, 4, gain_map_data)?);
            refs.push(reference(budget, b"dimg", 3, &[1, 4])?);
        } else {
            refs.push(reference(budget, b"dimg", 3, &[1])?);
        }
        groups.push(EntityGroup {
            kind: FourCc(*b"altr"),
            id: 0,
            entities: ids(budget, &[3, 1])?,
        });
    }
    if exif {
        let mut m = info(6, b"Exif");
        if exif_xmp_compressed {
            m.content_encoding = budget.string(b"deflate")?;
        }
        items.push(m);
        iloc.push(loc(budget, 6, exif_data)?);
        refs.push(reference(budget, b"cdsc", 6, &[1])?);
    }
    if xmp {
        let mut m = info(7, b"mime");
        m.content_type = budget.string(XMP_TYPE.as_bytes())?;
        if exif_xmp_compressed {
            m.content_encoding = budget.string(b"deflate")?;
        }
        items.push(m);
        iloc.push(loc(budget, 7, xmp_data)?);
        refs.push(reference(budget, b"cdsc", 7, &[1])?);
    }

    let meta = Meta {
        primary: 1,
        items,
        iloc,
        idat: None,
        refs,
        props,
        assoc,
        groups,
        other: Vec::new(),
        iprp_other: Vec::new(),
    };
    let mini = Mini {
        range: Extent {
            offset: hdr.start,
            length: hdr.end.saturating_sub(hdr.start),
        },
        payload: Extent {
            offset: payload.start,
            length: payload.end.saturating_sub(payload.start),
        },
        explicit_codec_types,
        float,
        full_range,
        alpha,
        explicit_cicp,
        hdr: has_hdr,
        icc,
        exif,
        xmp,
        chroma_subsampling,
        orientation,
        large_dimensions,
        width,
        height,
        chroma_horizontally_centred: chroma_h,
        chroma_vertically_centred: chroma_v,
        bit_depth,
        alpha_premultiplied,
        colour_primaries: cp,
        transfer_characteristics: tc,
        matrix_coefficients: mc,
        infe_type,
        codec_config_type,
        gain_map,
        hdr_properties,
        large_metadata,
        large_codec_config,
        large_item_data,
        exif_xmp_compressed,
        main_config,
        alpha_config,
        gain_map_config,
        icc_profile,
        tmap_icc_profile,
        gain_map_metadata,
        alpha_data,
        gain_map_data,
        main_data,
        exif_data,
        xmp_data,
    };
    Ok((meta, mini))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Limits;
    use crate::boxes::Src;
    use std::sync::atomic::AtomicBool;

    /// A header decoded by hand: version 0, full range, 4:2:0, orientation 1, 32 x 32, 8 bit,
    /// `av1C` of 4 bytes, 4 bytes of data.
    #[test]
    fn a_header_written_by_hand() {
        let mut payload = vec![0x08, 0x08, 0x1F, 0x3E, 0x08, 0x00, 0x0C];
        payload.extend_from_slice(&[0x81, 0x00, 0x0C, 0x00]);
        payload.extend_from_slice(b"data");
        let limits = Limits::default();
        let cancel = AtomicBool::new(false);
        let budget = Budget::new(&limits, &cancel);
        let end = payload.len() as u64;
        let src = Src {
            bytes: &payload,
            base: 0,
            file_len: end,
        };
        let ctx = Ctx::new(&src, &budget, &limits);
        let hdr = BoxHeader {
            kind: FourCc(*b"mini"),
            uuid: None,
            start: 0,
            header_len: 0,
            end,
        };
        let ftyp = Ftyp {
            major: FourCc(*b"mif3"),
            minor_version: u32::from_be_bytes(*b"avif"),
            compatible: Vec::new(),
        };
        let (meta, mini) = parse(&ctx, &hdr, &ftyp).unwrap();
        assert_eq!((mini.width, mini.height), (32, 32));
        assert_eq!((mini.chroma_subsampling, mini.orientation), (1, 1));
        assert!(mini.full_range && !mini.alpha && !mini.hdr && !mini.icc);
        assert_eq!((mini.bit_depth, mini.matrix_coefficients), (8, 6));
        assert_eq!(
            mini.main_config,
            Extent {
                offset: 7,
                length: 4
            }
        );
        assert_eq!(
            mini.main_data,
            Extent {
                offset: 11,
                length: 4
            }
        );
        assert_eq!(meta.primary, 1);
        assert_eq!(meta.items.len(), 1);
        assert_eq!(meta.props.len(), 4);
    }

    #[test]
    fn bits_are_msb_first_and_bounded() {
        let mut b = Bits::new(&[0b1010_0000, 0xff]);
        assert_eq!(b.bits(3), Ok(0b101));
        assert_eq!(b.bits(5), Ok(0));
        assert_eq!(b.bits(8), Ok(0xff));
        assert_eq!(b.flag(), Err(BAD));
        let mut b = Bits::new(&[0xff, 0xff, 0xff, 0xff]);
        assert_eq!(b.bits(32), Ok(u32::MAX));
    }

    #[test]
    fn align_skips_to_the_next_byte() {
        let mut b = Bits::new(&[0, 0, 0]);
        b.skip(3).unwrap();
        assert_eq!(b.align(), 1);
        assert_eq!(b.align(), 1);
    }

    #[test]
    fn depth_rules() {
        // not float: 0 -> 8; 1 then 3 bits 111 -> 16
        assert_eq!(depth(&mut Bits::new(&[0x00]), false), Ok(8));
        assert_eq!(depth(&mut Bits::new(&[0b1111_0000]), false), Ok(16));
        // float: 00 -> 16, 10 -> 64, 11 reserved
        assert_eq!(depth(&mut Bits::new(&[0b0000_0000]), true), Ok(16));
        assert_eq!(depth(&mut Bits::new(&[0b1000_0000]), true), Ok(64));
        assert_eq!(depth(&mut Bits::new(&[0b1100_0000]), true), Err(BAD));
    }

    #[test]
    fn orientation_5_is_irot_270_and_imir_axis_1() {
        assert_eq!(
            orientation_transforms(5),
            (Some(270), Some(Axis::LeftRight))
        );
        assert_eq!(orientation_transforms(1), (None, None));
        assert_eq!(orientation_transforms(8), (Some(90), None));
    }
}
