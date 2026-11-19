//! The small property boxes: `ispe pixi pasp irot imir clap colr auxC lsel a1op a1lx`.

use crate::boxes::{Cur, full_header};
use crate::budget::Budget;
use crate::{Error, Extent};

use super::PropValue;

/// The axis of an `imir` mirror.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Axis {
    /// Axis 0: the top and bottom swap (a vertical flip).
    TopBottom,
    /// Axis 1: the left and right swap (a horizontal flip).
    LeftRight,
}

/// A `colr` property.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Colour {
    /// `nclx`: CICP colour description.
    Nclx {
        /// `colour_primaries`.
        primaries: u16,
        /// `transfer_characteristics`.
        transfer: u16,
        /// `matrix_coefficients`.
        matrix: u16,
        /// `full_range_flag`.
        full_range: bool,
    },
    /// `prof` or `rICC`: an ICC profile, as an extent of the file (zero copy).
    Icc(Extent),
}

/// A clean aperture (`clap`): the raw fractions. [`Clap::crop`] gives the rectangle.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Clap {
    /// `cleanApertureWidthN`.
    pub width_n: u32,
    /// `cleanApertureWidthD`.
    pub width_d: u32,
    /// `cleanApertureHeightN`.
    pub height_n: u32,
    /// `cleanApertureHeightD`.
    pub height_d: u32,
    /// `horizOffN` (signed).
    pub horiz_off_n: i32,
    /// `horizOffD`.
    pub horiz_off_d: u32,
    /// `vertOffN` (signed).
    pub vert_off_n: i32,
    /// `vertOffD`.
    pub vert_off_d: u32,
}

/// A crop rectangle in pixels.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Rect {
    /// Left edge.
    pub x: u32,
    /// Top edge.
    pub y: u32,
    /// Width.
    pub width: u32,
    /// Height.
    pub height: u32,
}

/// `(dim - size) / 2 + off_n / off_d` as an exact integer, or `None` when it is not an integer
/// or is negative. `dim`, `size` and the offsets are small enough that i128 never overflows; the
/// operations are checked anyway.
fn crop_origin(dim: i128, size: i128, off_n: i128, off_d: i128) -> Option<i128> {
    let t = dim
        .checked_sub(size)?
        .checked_mul(off_d)?
        .checked_add(off_n.checked_mul(2)?)?;
    let d = off_d.checked_mul(2)?;
    if t.checked_rem(d)? != 0 {
        return None;
    }
    t.checked_div(d).filter(|&v| v >= 0)
}

impl Clap {
    /// The crop rectangle of this aperture on an image of `width` x `height`, with exact
    /// fractions (the rule of libavif `avifCropRectFromCleanApertureBox`, `src/avif.c`, BSD-2):
    /// every denominator is positive and below 2^31, the width and height numerators are not
    /// negative, the clap width and height are integers and nonzero, the left edge
    /// `(W - clapW) / 2 + horizOff` and the top edge are integers and not negative, and the
    /// rectangle is inside the image. Anything else gives `None`; nothing is rounded.
    pub fn crop(&self, width: u32, height: u32) -> Option<Rect> {
        let pos = |d: u32| i32::try_from(d).ok().filter(|&d| d > 0).map(i128::from);
        let nonneg = |n: u32| i32::try_from(n).ok().filter(|&n| n >= 0).map(i128::from);
        let (wd, hd) = (pos(self.width_d)?, pos(self.height_d)?);
        let (hod, vod) = (pos(self.horiz_off_d)?, pos(self.vert_off_d)?);
        let (wn, hn) = (nonneg(self.width_n)?, nonneg(self.height_n)?);
        if wn.checked_rem(wd)? != 0 || hn.checked_rem(hd)? != 0 {
            return None;
        }
        let cw = wn.checked_div(wd)?;
        let ch = hn.checked_div(hd)?;
        let w = i128::from(i32::try_from(width).ok()?);
        let h = i128::from(i32::try_from(height).ok()?);
        let x = crop_origin(w, cw, i128::from(self.horiz_off_n), hod)?;
        let y = crop_origin(h, ch, i128::from(self.vert_off_n), vod)?;
        if cw == 0 || ch == 0 || x.checked_add(cw)? > w || y.checked_add(ch)? > h {
            return None;
        }
        Some(Rect {
            x: u32::try_from(x).ok()?,
            y: u32::try_from(y).ok()?,
            width: u32::try_from(cw).ok()?,
            height: u32::try_from(ch).ok()?,
        })
    }
}

/// A FullBox payload: (version, cursor after the 4-byte header).
fn full<'a>(what: &'static str, bytes: &'a [u8], base: u64) -> Result<(u8, Cur<'a>), Error> {
    let (v, _, rest) = full_header(what, bytes)?;
    Ok((v, Cur::new(what, rest, base.saturating_add(4))))
}

pub(crate) fn ispe(bytes: &[u8], base: u64) -> Result<PropValue, Error> {
    let (_, mut c) = full("ispe", bytes, base)?;
    Ok(PropValue::Ispe {
        width: c.u32()?,
        height: c.u32()?,
    })
}

pub(crate) fn pixi(bytes: &[u8], base: u64, budget: &Budget<'_>) -> Result<PropValue, Error> {
    let (_, mut c) = full("pixi", bytes, base)?;
    let n = usize::from(c.u8()?);
    let bits = budget.copy(c.take(n)?)?;
    Ok(PropValue::Pixi { bits })
}

pub(crate) fn pasp(bytes: &[u8], base: u64) -> Result<PropValue, Error> {
    let mut c = Cur::new("pasp", bytes, base);
    Ok(PropValue::Pasp {
        h_spacing: c.u32()?,
        v_spacing: c.u32()?,
    })
}

pub(crate) fn irot(bytes: &[u8], base: u64) -> Result<PropValue, Error> {
    let b = Cur::new("irot", bytes, base).u8()?;
    Ok(PropValue::Irot {
        angle: u16::from(b & 3).saturating_mul(90),
    })
}

pub(crate) fn imir(bytes: &[u8], base: u64) -> Result<PropValue, Error> {
    let b = Cur::new("imir", bytes, base).u8()?;
    Ok(PropValue::Imir {
        axis: if b & 1 == 0 {
            Axis::TopBottom
        } else {
            Axis::LeftRight
        },
    })
}

pub(crate) fn clap(bytes: &[u8], base: u64) -> Result<PropValue, Error> {
    let mut c = Cur::new("clap", bytes, base);
    Ok(PropValue::Clap(Clap {
        width_n: c.u32()?,
        width_d: c.u32()?,
        height_n: c.u32()?,
        height_d: c.u32()?,
        horiz_off_n: c.i32()?,
        horiz_off_d: c.u32()?,
        vert_off_n: c.i32()?,
        vert_off_d: c.u32()?,
    }))
}

pub(crate) fn colr(bytes: &[u8], base: u64) -> Result<PropValue, Error> {
    let mut c = Cur::new("colr", bytes, base);
    let kind = c.fourcc()?;
    match &kind.0 {
        b"nclx" => {
            let primaries = c.u16()?;
            let transfer = c.u16()?;
            let matrix = c.u16()?;
            let full_range = c.u8()? & 0x80 != 0;
            Ok(PropValue::Colr(Colour::Nclx {
                primaries,
                transfer,
                matrix,
                full_range,
            }))
        }
        b"prof" | b"rICC" => Ok(PropValue::Colr(Colour::Icc(c.rest().1))),
        _ => Ok(PropValue::Other),
    }
}

pub(crate) fn auxc(bytes: &[u8], base: u64, budget: &Budget<'_>) -> Result<PropValue, Error> {
    let (_, mut c) = full("auxC", bytes, base)?;
    let aux_type = budget.arc_str(c.cstr())?;
    let (_, subtype) = c.rest();
    Ok(PropValue::AuxC { aux_type, subtype })
}

pub(crate) fn lsel(bytes: &[u8], base: u64) -> Result<PropValue, Error> {
    Ok(PropValue::Lsel {
        layer_id: Cur::new("lsel", bytes, base).u16()?,
    })
}

pub(crate) fn a1op(bytes: &[u8], base: u64) -> Result<PropValue, Error> {
    Ok(PropValue::A1op {
        op_index: Cur::new("a1op", bytes, base).u8()?,
    })
}

pub(crate) fn a1lx(bytes: &[u8], base: u64) -> Result<PropValue, Error> {
    let mut c = Cur::new("a1lx", bytes, base);
    let large = c.u8()? & 1 != 0;
    let mut sizes = [0u32; 3];
    for s in &mut sizes {
        *s = if large { c.u32()? } else { u32::from(c.u16()?) };
    }
    Ok(PropValue::A1lx { sizes })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[allow(clippy::too_many_arguments)]
    fn clap(wn: u32, wd: u32, hn: u32, hd: u32, xn: i32, xd: u32, yn: i32, yd: u32) -> Clap {
        Clap {
            width_n: wn,
            width_d: wd,
            height_n: hn,
            height_d: hd,
            horiz_off_n: xn,
            horiz_off_d: xd,
            vert_off_n: yn,
            vert_off_d: yd,
        }
    }

    #[test]
    fn centred_crop() {
        let r = clap(100, 1, 50, 1, 0, 1, 0, 1).crop(200, 100).unwrap();
        assert_eq!(
            r,
            Rect {
                x: 50,
                y: 25,
                width: 100,
                height: 50
            }
        );
    }

    #[test]
    fn offset_crop() {
        let r = clap(100, 1, 50, 1, 10, 1, -5, 1).crop(200, 100).unwrap();
        assert_eq!((r.x, r.y), (60, 20));
    }

    #[test]
    fn odd_image_with_half_offset_is_an_integer() {
        // W = 101, clapW = 100: (101 - 100) / 2 = 1/2, plus 1/2 = 1.
        let r = clap(100, 1, 100, 1, 1, 2, 1, 2).crop(101, 101).unwrap();
        assert_eq!((r.x, r.y), (1, 1));
    }

    #[test]
    fn fractional_origin_is_rejected() {
        assert_eq!(clap(100, 1, 100, 1, 0, 1, 0, 1).crop(101, 101), None);
    }

    #[test]
    fn rejects_bad_fractions_and_bounds() {
        assert_eq!(clap(100, 0, 50, 1, 0, 1, 0, 1).crop(200, 100), None); // D = 0
        assert_eq!(clap(100, 3, 50, 1, 0, 1, 0, 1).crop(200, 100), None); // width not an integer
        assert_eq!(clap(0, 1, 50, 1, 0, 1, 0, 1).crop(200, 100), None); // empty
        assert_eq!(clap(100, 1, 50, 1, -100, 1, 0, 1).crop(200, 100), None); // x < 0
        assert_eq!(clap(100, 1, 50, 1, 60, 1, 0, 1).crop(200, 100), None); // past the right edge
        assert_eq!(clap(300, 1, 50, 1, 0, 1, 0, 1).crop(200, 100), None); // wider than the image
        assert_eq!(clap(u32::MAX, 1, 50, 1, 0, 1, 0, 1).crop(200, 100), None); // negative as i32
        assert_eq!(clap(100, 1, 50, 1, 0, u32::MAX, 0, 1).crop(200, 100), None);
    }

    #[test]
    fn full_image_crop() {
        let r = clap(200, 1, 100, 1, 0, 1, 0, 1).crop(200, 100).unwrap();
        assert_eq!(
            r,
            Rect {
                x: 0,
                y: 0,
                width: 200,
                height: 100
            }
        );
    }

    #[test]
    fn irot_masks_and_imir_axis() {
        assert_eq!(irot(&[0xFF], 0), Ok(PropValue::Irot { angle: 270 }));
        assert_eq!(
            imir(&[0], 0),
            Ok(PropValue::Imir {
                axis: Axis::TopBottom
            })
        );
        assert_eq!(
            imir(&[3], 0),
            Ok(PropValue::Imir {
                axis: Axis::LeftRight
            })
        );
    }

    #[test]
    fn a1lx_sizes() {
        assert_eq!(
            a1lx(&[0, 0, 1, 0, 2, 0, 3], 0),
            Ok(PropValue::A1lx { sizes: [1, 2, 3] })
        );
        let mut b = vec![1];
        for n in [1u32, 2, 3] {
            b.extend_from_slice(&n.to_be_bytes());
        }
        assert_eq!(a1lx(&b, 0), Ok(PropValue::A1lx { sizes: [1, 2, 3] }));
    }
}
