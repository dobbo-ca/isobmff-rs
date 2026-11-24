//! The small property boxes: `ispe pixi pasp irot imir clap colr auxC lsel rref a1op a1lx`.

use crate::boxes::{Cur, full_header};
use crate::budget::Budget;
use crate::{Error, Extent, FourCc};

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

/// `(n + d / 2) / d`, truncating toward zero: the `round` of libheif's `Fraction`. `d` is positive.
fn round_div(n: i128, d: i128) -> Option<i128> {
    n.checked_add(d.checked_div(2)?)?.checked_div(d)
}

/// One axis of [`Clap::crop_rounded`]: the first and last sample, inclusive, after libheif's
/// clamping, or `None`. `lead` is true for the horizontal axis, whose first sample is truncated;
/// the vertical axis rounds it (libheif's asymmetry). Every fraction is exact.
fn rounded_span(
    dim: u32,
    (size_n, size_d): (u32, u32),
    (off_n, off_d): (i32, u32),
    lead: bool,
) -> Option<(i128, i128)> {
    let int32 = |v: u32| i32::try_from(v).ok().map(i128::from);
    let (dim, size_n, size_d, off_d) = (
        i128::from(dim),
        int32(size_n)?,
        int32(size_d).filter(|&d| d > 0)?,
        int32(off_d).filter(|&d| d > 0)?,
    );
    if !(1..=2_147_483_648).contains(&dim) || round_div(size_n, size_d)? < 1 {
        return None;
    }
    // first = off_n/off_d + (dim - 1)/2 - (size_n - size_d)/(2 size_d), over one denominator.
    let span = size_n.checked_sub(size_d)?;
    let den = off_d.checked_mul(size_d)?.checked_mul(2)?;
    let num = i128::from(off_n)
        .checked_mul(size_d)?
        .checked_mul(2)?
        .checked_add(
            dim.checked_sub(1)?
                .checked_mul(off_d)?
                .checked_mul(size_d)?,
        )?
        .checked_sub(span.checked_mul(off_d)?)?;
    let first = if lead {
        num.checked_div(den)?
    } else {
        round_div(num, den)?
    };
    // last = round((size - 1) + first), from the unclamped first.
    let last = round_div(span.checked_add(first.checked_mul(size_d)?)?, size_d)?;
    let first = first.max(0);
    // libheif casts to uint32 before comparing: a negative last is clamped like one past the edge.
    let last = if last < 0 || last >= dim {
        dim.checked_sub(1)?
    } else {
        last
    };
    (first <= last).then_some((first, last))
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

    /// The crop rectangle with the rounding of libheif (v1.23.7, `Box_clap::get_crop` and the
    /// clamping in `ImageItem::decode_and_postprocess`), used as a design reference: no code is
    /// copied. It accepts the files that [`crop`](Self::crop) rejects. With `W` x `H` the image
    /// size at this point of the transform list:
    ///
    /// ```text
    /// left   = trunc(offX + (W - 1)/2 - (clapW - 1)/2)       right  = round((clapW - 1) + left)
    /// top    = round(offY + (H - 1)/2 - (clapH - 1)/2)       bottom = round((clapH - 1) + top)
    /// ```
    ///
    /// `trunc` is division toward zero and `round(n/d) = (n + d/2)/d` truncated, so the two axes
    /// differ on a half. `right` and `bottom` use the unclamped `left` and `top`. Then `left` and
    /// `top` are raised to 0, a `right` or `bottom` that is negative or at least `W` or `H` becomes
    /// `W - 1` or `H - 1` (libheif compares as unsigned, so a rectangle entirely left of or above
    /// the image becomes the full width or height), and `left > right` or `top > bottom` (entirely
    /// right of or below the image) gives `None`.
    ///
    /// Fractions are exact (`i128`, checked, never a panic). libheif's `Fraction`
    /// loses precision (a denominator above 65 536, a numerator above 65 536 with a denominator
    /// above 1, `W - 1` or `H - 1` above 65 536, intermediate products beyond `i32`), so the result
    /// can differ from libheif there. `None` also for: a zero or negative-as-`i32` denominator, a width or
    /// height numerator above `i32::MAX`, a rounded clap width or height below 1 (libheif fails the
    /// load), and `W` or `H` of 0 or above 2^31.
    pub fn crop_rounded(&self, width: u32, height: u32) -> Option<Rect> {
        let (x0, x1) = rounded_span(
            width,
            (self.width_n, self.width_d),
            (self.horiz_off_n, self.horiz_off_d),
            true,
        )?;
        let (y0, y1) = rounded_span(
            height,
            (self.height_n, self.height_d),
            (self.vert_off_n, self.vert_off_d),
            false,
        )?;
        Some(Rect {
            x: u32::try_from(x0).ok()?,
            y: u32::try_from(y0).ok()?,
            width: u32::try_from(x1.checked_sub(x0)?.checked_add(1)?).ok()?,
            height: u32::try_from(y1.checked_sub(y0)?.checked_add(1)?).ok()?,
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

pub(crate) fn rref(bytes: &[u8], base: u64, budget: &Budget<'_>) -> Result<PropValue, Error> {
    let (version, mut c) = full("rref", bytes, base)?;
    if version > 1 {
        return Err(Error::Malformed("rref"));
    }
    // libheif reads the count as u32 when the payload is a non-empty multiple of 4 (box.cc:4274-4288).
    let rem = c.remaining();
    let n = if rem > 0 && rem.is_multiple_of(4) {
        usize::try_from(c.u32()?).map_err(|_| Error::Malformed("rref"))?
    } else {
        usize::from(c.u8()?)
    };
    if !c.fits(n, 4) {
        return Err(Error::Malformed("rref"));
    }
    let mut types = budget.vec::<FourCc>(n)?;
    for _ in 0..n {
        types.push(c.fourcc()?);
    }
    Ok(PropValue::Rref { types })
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

    #[test]
    fn rref_counts() {
        let l = crate::Limits::default();
        let stop = std::sync::atomic::AtomicBool::new(false);
        let b = Budget::new(&l, &stop);
        let want = PropValue::Rref {
            types: vec![FourCc(*b"pred")],
        };
        assert_eq!(rref(b"\0\0\0\0\x01pred", 0, &b), Ok(want.clone()));
        assert_eq!(rref(b"\0\0\0\0\0\0\0\x01pred", 0, &b), Ok(want));
    }

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

    fn rr(c: Clap, w: u32, h: u32) -> Option<(u32, u32, u32, u32)> {
        c.crop_rounded(w, h).map(|r| (r.x, r.y, r.width, r.height))
    }

    #[test]
    fn crop_rounded_matches_hand_computed_libheif_values() {
        // Even sizes: the exact centre.
        assert_eq!(
            rr(clap(50, 1, 50, 1, 0, 1, 0, 1), 100, 100),
            Some((25, 25, 50, 50))
        );
        // 5 x 5, 2 x 2: left = trunc(1.5) = 1, top = round(1.5) = 2.
        assert_eq!(rr(clap(2, 1, 2, 1, 0, 1, 0, 1), 5, 5), Some((1, 2, 2, 2)));
        // A half offset: both origins are exactly 2.
        assert_eq!(rr(clap(2, 1, 2, 1, 1, 2, 1, 2), 5, 5), Some((2, 2, 2, 2)));
        // Partly outside on the left: left -5 is raised to 0, right = 44.
        assert_eq!(
            rr(clap(50, 1, 50, 1, -30, 1, 0, 1), 100, 100),
            Some((0, 25, 45, 50))
        );
        // Partly outside on the right: left 75, right 124 is clamped to 99.
        assert_eq!(
            rr(clap(50, 1, 50, 1, 50, 1, 0, 1), 100, 100),
            Some((75, 25, 25, 50))
        );
        // A fractional width, 5/2: round(2.5) = 3 wide, left = trunc(4.5 - 0.75) = 3, right = 5.
        assert_eq!(rr(clap(5, 2, 4, 1, 0, 1, 0, 1), 10, 10), Some((3, 3, 3, 4)));
        // The largest denominators: width 1, centred.
        assert_eq!(
            rr(
                clap(i32::MAX as u32, i32::MAX as u32, 1, 1, 0, 1, 0, 1),
                10,
                10
            ),
            Some((4, 5, 1, 1))
        );
    }

    #[test]
    fn crop_rounded_libheif_quirks_and_rejections() {
        // Entirely right of the image: left 23 > right 9.
        assert_eq!(rr(clap(4, 1, 4, 1, 20, 1, 0, 1), 10, 10), None);
        // Entirely left of the image: libheif clamps the negative right to W - 1, the full width.
        assert_eq!(
            rr(clap(4, 1, 4, 1, -20, 1, 0, 1), 10, 10),
            Some((0, 3, 10, 4))
        );
        // i32::MIN offsets do not overflow.
        assert_eq!(
            rr(clap(4, 1, 4, 1, i32::MIN, 1, i32::MIN, 1), 10, 10),
            Some((0, 0, 10, 10))
        );
        assert!(
            clap(4, 1, 4, 1, i32::MAX, 1, i32::MAX, 1)
                .crop_rounded(10, 10)
                .is_none()
        );
        // Zero, huge and negative-as-i32 denominators.
        assert_eq!(rr(clap(4, 0, 4, 1, 0, 1, 0, 1), 10, 10), None);
        assert_eq!(rr(clap(4, 1, 4, 0, 0, 1, 0, 1), 10, 10), None);
        assert_eq!(rr(clap(4, 1, 4, 1, 0, 0, 0, 1), 10, 10), None);
        assert_eq!(rr(clap(4, 1, 4, 1, 0, 1, 0, 0), 10, 10), None);
        assert_eq!(rr(clap(4, u32::MAX, 4, 1, 0, 1, 0, 1), 10, 10), None);
        assert_eq!(rr(clap(4, 1, 4, 1, 0, u32::MAX, 0, 1), 10, 10), None);
        // A width numerator above i32::MAX, a width that rounds to 0, an empty width.
        assert_eq!(rr(clap(u32::MAX, 1, 4, 1, 0, 1, 0, 1), 10, 10), None);
        assert_eq!(rr(clap(1, i32::MAX as u32, 4, 1, 0, 1, 0, 1), 10, 10), None);
        assert_eq!(rr(clap(0, 1, 4, 1, 0, 1, 0, 1), 10, 10), None);
        // An empty image, and one larger than 2^31.
        assert_eq!(rr(clap(4, 1, 4, 1, 0, 1, 0, 1), 0, 10), None);
        assert_eq!(rr(clap(4, 1, 4, 1, 0, 1, 0, 1), 10, u32::MAX), None);
        // The widest allowed image, with the widest clap, does not overflow.
        assert!(
            clap(
                i32::MAX as u32,
                1,
                i32::MAX as u32,
                1,
                i32::MIN,
                1,
                i32::MIN,
                1
            )
            .crop_rounded(1 << 31, 1 << 31)
            .is_some()
        );
    }

    #[test]
    fn crop_rounded_equals_crop_when_the_origin_is_an_integer() {
        for (w, h, cw, ch) in [
            (200u32, 100u32, 100u32, 50u32),
            (101, 101, 99, 51),
            (64, 48, 64, 48),
        ] {
            let c = clap(cw, 1, ch, 1, 0, 1, 0, 1);
            if let Some(r) = c.crop(w, h) {
                assert_eq!(c.crop_rounded(w, h), Some(r), "{w}x{h} {cw}x{ch}");
            }
        }
    }
}
