//! `hvcC` and `av1C`: the codec configuration records. The parsed header is for callers that
//! need the bit depth or format; the decoder crates take the raw payload ([`Hvcc::raw`],
//! [`Av1c::raw`]).

use crate::boxes::Cur;
use crate::budget::Budget;
use crate::{Error, Extent};

/// One array of NAL units of an `hvcC`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NalArray {
    /// `array_completeness`.
    pub complete: bool,
    /// `NAL_unit_type` (6 bits).
    pub nal_type: u8,
    /// Each NAL unit, as an extent of the file (without its 2-byte length).
    pub nals: Vec<Extent>,
}

/// An HEVC decoder configuration record (`hvcC`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Hvcc {
    /// `configurationVersion` (1).
    pub version: u8,
    /// `general_profile_space` (2 bits).
    pub profile_space: u8,
    /// `general_tier_flag`.
    pub tier: bool,
    /// `general_profile_idc` (5 bits).
    pub profile_idc: u8,
    /// `chromaFormat` (0 mono, 1 = 4:2:0, 2 = 4:2:2, 3 = 4:4:4).
    pub chroma_format_idc: u8,
    /// Luma bit depth in bits (`bitDepthLumaMinus8` + 8).
    pub bit_depth_luma: u8,
    /// Chroma bit depth in bits (`bitDepthChromaMinus8` + 8).
    pub bit_depth_chroma: u8,
    /// `lengthSizeMinusOne` (the NAL length prefix size in the item data is this + 1).
    pub length_size_minus_one: u8,
    /// The parameter set arrays.
    pub arrays: Vec<NalArray>,
    /// The whole payload of the box.
    pub raw: Extent,
}

/// An AV1 codec configuration record (`av1C`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Av1c {
    /// `seq_profile` (3 bits).
    pub seq_profile: u8,
    /// `seq_level_idx_0` (5 bits).
    pub seq_level_idx_0: u8,
    /// `seq_tier_0`.
    pub tier: bool,
    /// `high_bitdepth`.
    pub high_bitdepth: bool,
    /// `twelve_bit`.
    pub twelve_bit: bool,
    /// `monochrome`.
    pub monochrome: bool,
    /// `chroma_subsampling_x`.
    pub subsampling_x: bool,
    /// `chroma_subsampling_y`.
    pub subsampling_y: bool,
    /// `chroma_sample_position` (2 bits).
    pub chroma_sample_position: u8,
    /// The `configOBUs` (a sequence header OBU).
    pub config_obus: Extent,
    /// The whole payload of the box.
    pub raw: Extent,
}

impl Av1c {
    /// The bit depth: 12 if `twelve_bit`, 10 if `high_bitdepth`, else 8.
    pub fn depth(&self) -> u8 {
        if self.twelve_bit {
            12
        } else if self.high_bitdepth {
            10
        } else {
            8
        }
    }

    /// The chroma format as 400 (monochrome), 420, 422 or 444.
    pub fn yuv_format(&self) -> u16 {
        if self.monochrome {
            400
        } else if self.subsampling_x && self.subsampling_y {
            420
        } else if self.subsampling_x {
            422
        } else {
            444
        }
    }
}

pub(crate) fn parse_hvcc(bytes: &[u8], base: u64, budget: &Budget<'_>) -> Result<Hvcc, Error> {
    let mut c = Cur::new("hvcC", bytes, base);
    let version = c.u8()?;
    if version != 1 {
        return Err(Error::Malformed("hvcC"));
    }
    let b = c.u8()?;
    // compatibility flags (4), constraint flags (6), level (1), segmentation (2), parallelism (1)
    c.skip(14)?;
    let chroma_format_idc = c.u8()? & 3;
    let bit_depth_luma = (c.u8()? & 7).saturating_add(8);
    let bit_depth_chroma = (c.u8()? & 7).saturating_add(8);
    c.skip(2)?; // avgFrameRate
    let length_size_minus_one = c.u8()? & 3;
    let num_arrays = usize::from(c.u8()?);
    let mut arrays = budget.vec::<NalArray>(num_arrays)?;
    for _ in 0..num_arrays {
        let a = c.u8()?;
        let n = usize::from(c.u16()?);
        if c.remaining() / 2 < n {
            return Err(Error::Malformed("hvcC"));
        }
        let mut nals = budget.vec::<Extent>(n)?;
        for _ in 0..n {
            let len = usize::from(c.u16()?);
            nals.push(c.extent(len)?);
        }
        arrays.push(NalArray {
            complete: a & 0x80 != 0,
            nal_type: a & 0x3f,
            nals,
        });
    }
    Ok(Hvcc {
        version,
        profile_space: b >> 6,
        tier: b & 0x20 != 0,
        profile_idc: b & 0x1f,
        chroma_format_idc,
        bit_depth_luma,
        bit_depth_chroma,
        length_size_minus_one,
        arrays,
        raw: Extent {
            offset: base,
            length: bytes.len() as u64,
        },
    })
}

pub(crate) fn parse_av1c(bytes: &[u8], base: u64) -> Result<Av1c, Error> {
    let mut c = Cur::new("av1C", bytes, base);
    if c.u8()? != 0x81 {
        return Err(Error::Malformed("av1C"));
    }
    let b1 = c.u8()?;
    let b2 = c.u8()?;
    c.skip(1)?; // initial_presentation_delay
    let (_, config_obus) = c.rest();
    Ok(Av1c {
        seq_profile: b1 >> 5,
        seq_level_idx_0: b1 & 0x1f,
        tier: b2 & 0x80 != 0,
        high_bitdepth: b2 & 0x40 != 0,
        twelve_bit: b2 & 0x20 != 0,
        monochrome: b2 & 0x10 != 0,
        subsampling_x: b2 & 0x08 != 0,
        subsampling_y: b2 & 0x04 != 0,
        chroma_sample_position: b2 & 3,
        config_obus,
        raw: Extent {
            offset: base,
            length: bytes.len() as u64,
        },
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::AtomicBool;

    #[test]
    fn av1c_depth_and_format() {
        // marker/version, profile 0 level 8, high_bitdepth + 4:2:0, delay, one OBU byte
        let a = parse_av1c(&[0x81, 0x08, 0x4c, 0x00, 0x0a], 10).unwrap();
        assert_eq!((a.depth(), a.yuv_format()), (10, 420));
        assert_eq!(
            a.config_obus,
            Extent {
                offset: 14,
                length: 1
            }
        );
        let m = parse_av1c(&[0x81, 0, 0x10, 0], 0).unwrap();
        assert_eq!((m.depth(), m.yuv_format()), (8, 400));
        let t = parse_av1c(&[0x81, 0, 0x60 | 0x08, 0], 0).unwrap();
        assert_eq!((t.depth(), t.yuv_format()), (12, 422));
        assert!(parse_av1c(&[0x01, 0, 0, 0], 0).is_err());
    }

    #[test]
    fn hvcc_header_and_arrays() {
        let mut b = vec![1, 0x01, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 93, 0xF0, 0, 0xFC];
        b.extend_from_slice(&[0xFD, 0xFA, 0xFA, 0, 0, 0x03, 1]); // 4:2:0, depth 10/10, 1 array
        b.extend_from_slice(&[0xA0, 0, 1, 0, 2, 0x40, 0x01]); // VPS, one NAL of 2 bytes
        let cancel = AtomicBool::new(false);
        let budget = Budget::new(&crate::Limits::default(), &cancel);
        let h = parse_hvcc(&b, 100, &budget).unwrap();
        assert_eq!(
            (h.chroma_format_idc, h.bit_depth_luma, h.bit_depth_chroma),
            (1, 10, 10)
        );
        assert_eq!(h.length_size_minus_one, 3);
        assert_eq!(h.arrays.len(), 1);
        let a = h.arrays.first().unwrap();
        assert_eq!((a.complete, a.nal_type), (true, 32));
        assert_eq!(
            a.nals,
            vec![Extent {
                offset: 100 + 23 + 5,
                length: 2
            }]
        );
    }
}
