//! The payloads of derived items: `grid`, `iovl` and `tmap`.

use crate::boxes::Cur;
use crate::budget::Budget;
use crate::{Error, Limit, Limits};

/// A `grid` image: `columns` x `rows` tiles (the `dimg` references, row-major) on an output of
/// `width` x `height`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Grid {
    /// Number of tile rows.
    pub rows: u32,
    /// Number of tile columns.
    pub columns: u32,
    /// Output width in pixels (before transforms).
    pub width: u32,
    /// Output height in pixels (before transforms).
    pub height: u32,
    /// The tile item ids, row-major.
    pub tiles: Vec<u32>,
}

/// One input of an `iovl` overlay.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct OverlayInput {
    /// The input item id (a `dimg` reference).
    pub id: u32,
    /// Horizontal offset of the input on the canvas.
    pub x: i32,
    /// Vertical offset of the input on the canvas.
    pub y: i32,
}

/// An `iovl` overlay: `inputs` painted in order on a canvas.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Overlay {
    /// Canvas fill value, R G B A, 16 bits each.
    pub fill: [u16; 4],
    /// Canvas width in pixels.
    pub width: u32,
    /// Canvas height in pixels.
    pub height: u32,
    /// The inputs, in `dimg` order.
    pub inputs: Vec<OverlayInput>,
}

/// A fraction. The numerator is signed where the spec says so and widened to `i64`; the
/// denominator is never 0 in a parsed value.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Fraction {
    /// Numerator.
    pub numerator: i64,
    /// Denominator.
    pub denominator: u32,
}

/// The gain map parameters of one colour channel (ISO 21496-1).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct GainMapChannel {
    /// `gain_map_min` (signed numerator).
    pub min: Fraction,
    /// `gain_map_max` (signed numerator).
    pub max: Fraction,
    /// `gain_map_gamma`.
    pub gamma: Fraction,
    /// `base_offset` (signed numerator).
    pub base_offset: Fraction,
    /// `alternate_offset` (signed numerator).
    pub alternate_offset: Fraction,
}

/// The payload of a `tmap` item: ISO 21496-1 gain map metadata. This parses the superset that
/// libavif and libultrahdr read: flag bit 3 selects the common-denominator layout, and the raw
/// flags byte is kept because bits 2 and 3 are not settled between the two libraries.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct GainMapMetadata {
    /// `minimum_version` (always 0 here: a larger value is not parsed).
    pub minimum_version: u16,
    /// `writer_version`.
    pub writer_version: u16,
    /// The raw flags byte.
    pub flags: u8,
    /// Flags bit 7: three channels, else one (channels 1 and 2 then repeat channel 0).
    pub is_multichannel: bool,
    /// Flags bit 6.
    pub use_base_colour_space: bool,
    /// Flags bit 2 (`backward_direction` in libultrahdr).
    pub backward_direction: bool,
    /// Flags bit 3 (`use_common_denominator` in libultrahdr): every denominator is the one shared
    /// value, and the numerators follow it.
    pub common_denominator: bool,
    /// `base_hdr_headroom`.
    pub base_headroom: Fraction,
    /// `alternate_hdr_headroom`.
    pub alternate_headroom: Fraction,
    /// The three channels (R, G, B).
    pub channels: [GainMapChannel; 3],
}

/// The `grid` payload (8 or 12 bytes); the tiles are filled in from the `dimg` references.
pub(crate) fn parse_grid(bytes: &[u8], limits: &Limits) -> Result<Grid, Error> {
    let mut c = Cur::new("grid", bytes, 0);
    if c.u8()? != 0 {
        return Err(Error::Malformed("grid"));
    }
    let large = c.u8()? & 1 != 0;
    let rows = u32::from(c.u8()?).saturating_add(1);
    let columns = u32::from(c.u8()?).saturating_add(1);
    let (width, height) = if large {
        (c.u32()?, c.u32()?)
    } else {
        (u32::from(c.u16()?), u32::from(c.u16()?))
    };
    if c.remaining() != 0 || width == 0 || height == 0 {
        return Err(Error::Malformed("grid"));
    }
    if u64::from(rows).saturating_mul(u64::from(columns)) > limits.max_grid_tiles as u64 {
        return Err(Error::LimitExceeded(Limit::GridTiles));
    }
    check_pixels(width, height, limits)?;
    Ok(Grid {
        rows,
        columns,
        width,
        height,
        tiles: Vec::new(),
    })
}

fn check_pixels(width: u32, height: u32, limits: &Limits) -> Result<(), Error> {
    if u64::from(width).saturating_mul(u64::from(height)) > limits.max_pixels {
        return Err(Error::LimitExceeded(Limit::Pixels));
    }
    Ok(())
}

/// The `iovl` payload. The number of inputs is what the offsets that follow the header hold;
/// their ids are filled in from the `dimg` references.
pub(crate) fn parse_iovl(
    bytes: &[u8],
    limits: &Limits,
    budget: &Budget<'_>,
) -> Result<Overlay, Error> {
    let mut c = Cur::new("iovl", bytes, 0);
    if c.u8()? != 0 {
        return Err(Error::Malformed("iovl"));
    }
    let large = c.u8()? & 1 != 0;
    let mut fill = [0u16; 4];
    for f in &mut fill {
        *f = c.u16()?;
    }
    let (width, height) = if large {
        (c.u32()?, c.u32()?)
    } else {
        (u32::from(c.u16()?), u32::from(c.u16()?))
    };
    check_pixels(width, height, limits)?;
    let pair: usize = if large { 8 } else { 4 };
    let n = c.remaining().checked_div(pair).unwrap_or(0);
    if n.saturating_mul(pair) != c.remaining() {
        return Err(Error::Malformed("iovl"));
    }
    if n > limits.max_grid_tiles {
        return Err(Error::LimitExceeded(Limit::GridTiles));
    }
    let mut inputs = budget.vec::<OverlayInput>(n)?;
    for _ in 0..n {
        let (x, y) = if large {
            (c.i32()?, c.i32()?)
        } else {
            (i32::from(c.u16()? as i16), i32::from(c.u16()? as i16))
        };
        inputs.push(OverlayInput { id: 0, x, y });
    }
    Ok(Overlay {
        fill,
        width,
        height,
        inputs,
    })
}

fn frac(c: &mut Cur<'_>, common: Option<u32>, signed: bool) -> Result<Fraction, Error> {
    let numerator = if signed {
        i64::from(c.i32()?)
    } else {
        i64::from(c.u32()?)
    };
    let denominator = match common {
        Some(d) => d,
        None => c.u32()?,
    };
    Ok(Fraction {
        numerator,
        denominator,
    })
}

/// `a <= b` for two fractions with positive denominators.
fn le(a: Fraction, b: Fraction) -> Option<bool> {
    let l = i128::from(a.numerator).checked_mul(i128::from(b.denominator))?;
    let r = i128::from(b.numerator).checked_mul(i128::from(a.denominator))?;
    Some(l <= r)
}

/// The `tmap` payload. `Ok(None)` when the version is one this crate does not read (the item
/// version or `minimum_version` is not 0), as libavif ignores such a gain map. Zero
/// denominators, a zero gamma numerator, `max < min` and trailing bytes after a version-0 writer
/// are `Malformed("tmap")`.
pub(crate) fn parse_tmap(bytes: &[u8]) -> Result<Option<GainMapMetadata>, Error> {
    let mut c = Cur::new("tmap", bytes, 0);
    if c.u8()? != 0 {
        return Ok(None);
    }
    let minimum_version = c.u16()?;
    if minimum_version != 0 {
        return Ok(None);
    }
    let writer_version = c.u16()?;
    let flags = c.u8()?;
    let is_multichannel = flags & 0x80 != 0;
    let common_denominator = flags & 0x08 != 0;
    let common = if common_denominator {
        Some(c.u32()?)
    } else {
        None
    };
    let base_headroom = frac(&mut c, common, false)?;
    let alternate_headroom = frac(&mut c, common, false)?;
    let count = if is_multichannel { 3 } else { 1 };
    let mut channels = [GainMapChannel {
        min: base_headroom,
        max: base_headroom,
        gamma: base_headroom,
        base_offset: base_headroom,
        alternate_offset: base_headroom,
    }; 3];
    for i in 0..count {
        let ch = GainMapChannel {
            min: frac(&mut c, common, true)?,
            max: frac(&mut c, common, true)?,
            gamma: frac(&mut c, common, false)?,
            base_offset: frac(&mut c, common, true)?,
            alternate_offset: frac(&mut c, common, true)?,
        };
        if let Some(slot) = channels.get_mut(i) {
            *slot = ch;
        }
    }
    if let Some(first) = channels.first().copied() {
        for slot in channels.iter_mut().skip(count) {
            *slot = first;
        }
    }
    if writer_version == 0 && c.remaining() != 0 {
        return Err(Error::Malformed("tmap"));
    }
    let bad = |f: Fraction| f.denominator == 0;
    if bad(base_headroom) || bad(alternate_headroom) {
        return Err(Error::Malformed("tmap"));
    }
    for ch in &channels {
        let any_zero = [
            ch.min,
            ch.max,
            ch.gamma,
            ch.base_offset,
            ch.alternate_offset,
        ]
        .into_iter()
        .any(bad);
        if any_zero || ch.gamma.numerator == 0 || le(ch.min, ch.max) != Some(true) {
            return Err(Error::Malformed("tmap"));
        }
    }
    Ok(Some(GainMapMetadata {
        minimum_version,
        writer_version,
        flags,
        is_multichannel,
        use_base_colour_space: flags & 0x40 != 0,
        backward_direction: flags & 0x04 != 0,
        common_denominator,
        base_headroom,
        alternate_headroom,
        channels,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::AtomicBool;

    fn be(v: &[i64]) -> Vec<u8> {
        v.iter().flat_map(|&n| (n as u32).to_be_bytes()).collect()
    }

    #[test]
    fn grid_16_and_32_bit() {
        let l = Limits::default();
        let g = parse_grid(&[0, 0, 1, 2, 0x04, 0x00, 0x03, 0x00], &l).unwrap();
        assert_eq!((g.rows, g.columns, g.width, g.height), (2, 3, 1024, 768));
        let mut b = vec![0, 1, 0, 0];
        b.extend_from_slice(&be(&[70000, 10]));
        let g = parse_grid(&b, &l).unwrap();
        assert_eq!((g.rows, g.columns, g.width, g.height), (1, 1, 70000, 10));
    }

    #[test]
    fn grid_rejects_bad_input() {
        let l = Limits::default();
        assert!(parse_grid(&[1, 0, 0, 0, 0, 1, 0, 1], &l).is_err()); // version
        assert!(parse_grid(&[0, 0, 0, 0, 0, 0, 0, 1], &l).is_err()); // zero width
        assert!(parse_grid(&[0, 0, 0, 0, 0, 1, 0, 1, 9], &l).is_err()); // trailing byte
        assert!(parse_grid(&[0, 0, 0, 0, 0, 1], &l).is_err()); // short
        let few = Limits {
            max_grid_tiles: 3,
            ..Limits::default()
        };
        assert_eq!(
            parse_grid(&[0, 0, 1, 1, 0, 1, 0, 1], &few),
            Err(Error::LimitExceeded(Limit::GridTiles))
        );
        let small = Limits {
            max_pixels: 100,
            ..Limits::default()
        };
        assert_eq!(
            parse_grid(&[0, 0, 0, 0, 0, 11, 0, 10], &small),
            Err(Error::LimitExceeded(Limit::Pixels))
        );
    }

    #[test]
    fn iovl_with_signed_offsets() {
        let l = Limits::default();
        let cancel = AtomicBool::new(false);
        let budget = Budget::new(&l, &cancel);
        let mut b = vec![0, 0, 0xff, 0xff, 0, 0, 0, 0, 0xff, 0xff, 0, 100, 0, 50];
        b.extend_from_slice(&[0, 5, 0xff, 0xfe]); // (5, -2)
        b.extend_from_slice(&[0, 0, 0, 7]); // (0, 7)
        let o = parse_iovl(&b, &l, &budget).unwrap();
        assert_eq!((o.fill, o.width, o.height), ([65535, 0, 0, 65535], 100, 50));
        assert_eq!(
            o.inputs,
            vec![
                OverlayInput { id: 0, x: 5, y: -2 },
                OverlayInput { id: 0, x: 0, y: 7 }
            ]
        );
        b.push(0);
        assert!(parse_iovl(&b, &l, &budget).is_err()); // offsets that are not whole pairs
    }

    #[test]
    fn tmap_common_denominator_single_channel() {
        // version 0, minimum 0, writer 0, flags: common denominator (bit 3) and base colour space.
        let mut b = vec![0, 0, 0, 0, 0, 0x48];
        b.extend_from_slice(&be(&[10, 0, 30, -10, 20, 10, 0, 0]));
        let m = parse_tmap(&b).unwrap().unwrap();
        assert!(m.common_denominator && m.use_base_colour_space && !m.is_multichannel);
        assert_eq!(m.flags, 0x48);
        assert_eq!(
            m.alternate_headroom,
            Fraction {
                numerator: 30,
                denominator: 10
            }
        );
        let c = m.channels.first().unwrap();
        assert_eq!(
            c.min,
            Fraction {
                numerator: -10,
                denominator: 10
            }
        );
        assert_eq!(
            c.max,
            Fraction {
                numerator: 20,
                denominator: 10
            }
        );
        assert_eq!(m.channels.get(2), m.channels.first()); // single channel repeats
    }

    #[test]
    fn tmap_separate_denominators_multichannel() {
        let mut b = vec![0, 0, 0, 0, 0, 0x80];
        b.extend_from_slice(&be(&[0, 1, 2, 1]));
        for _ in 0..3 {
            b.extend_from_slice(&be(&[-1, 2, 3, 2, 1, 1, 0, 1, 0, 1]));
        }
        let m = parse_tmap(&b).unwrap().unwrap();
        assert!(m.is_multichannel && !m.common_denominator);
        let c = m.channels.get(1).unwrap();
        assert_eq!(
            c.gamma,
            Fraction {
                numerator: 1,
                denominator: 1
            }
        );
        assert_eq!(
            c.min,
            Fraction {
                numerator: -1,
                denominator: 2
            }
        );
    }

    #[test]
    fn tmap_unknown_version_and_invalid_values() {
        assert_eq!(parse_tmap(&[1, 0, 0, 0, 0, 0]), Ok(None));
        assert_eq!(parse_tmap(&[0, 0, 1, 0, 1, 0]), Ok(None));
        // zero common denominator
        let mut b = vec![0, 0, 0, 0, 0, 0x08];
        b.extend_from_slice(&be(&[0, 0, 0, 0, 0, 1, 0, 0]));
        assert!(parse_tmap(&b).is_err());
        // max < min
        let mut b = vec![0, 0, 0, 0, 0, 0x00];
        b.extend_from_slice(&be(&[0, 1, 0, 1, 5, 1, 1, 1, 1, 1, 0, 1, 0, 1]));
        assert!(parse_tmap(&b).is_err());
        // truncated
        assert!(parse_tmap(&[0, 0, 0, 0, 0]).is_err());
    }
}
