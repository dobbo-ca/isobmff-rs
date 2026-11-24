//! Applies the transformative properties of an item (`clap`, `irot`, `imir`) to decoded planes.
//!
//! The decoders (hevc-rs, av1-rs, gainmap-rs) give planes; [`apply_transforms`] applies the list
//! [`Image::transforms`](crate::Image::transforms) in order (the ipma order, which libheif also
//! applies in) and [`transformed_size`] gives the size without planes. Pure safe Rust, serial,
//! bounded by [`Limits`] and the cancel flag. See `docs/transforms.md`.

use std::sync::atomic::AtomicBool;

use crate::budget::Budget;
use crate::{Axis, Error, Limit, Limits, Rect, Transform};

/// The side of a rotation tile. A 90 degree rotation reads and writes in tiles of this many
/// samples square, so a large image does not stride over the whole plane for every sample.
const TILE: usize = 64;

const PLANES: Error = Error::Malformed("planes");
const ALLOC: Error = Error::LimitExceeded(Limit::Alloc);

/// The chroma format of a set of [`Planes`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Chroma {
    /// Luma only.
    Mono,
    /// Chroma is half the width and half the height of luma (rounded up).
    Yuv420,
    /// Chroma is half the width of luma (rounded up), full height.
    Yuv422,
    /// Chroma is as large as luma.
    Yuv444,
}

/// The samples of one plane, rows packed (stride = width).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Samples {
    /// 8-bit samples.
    U8(Vec<u8>),
    /// 9 to 16-bit samples, in the low bits.
    U16(Vec<u16>),
}

impl Samples {
    fn count(&self) -> usize {
        match self {
            Samples::U8(v) => v.len(),
            Samples::U16(v) => v.len(),
        }
    }

    fn bytes(&self) -> usize {
        match self {
            Samples::U8(v) => v.len(),
            Samples::U16(v) => v.len().saturating_mul(2),
        }
    }
}

/// One plane: `width` x `height` samples, `samples.len() == width * height`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Plane {
    /// Width in samples.
    pub width: u32,
    /// Height in samples.
    pub height: u32,
    /// The samples; each plane picks 8 or 16 bits on its own.
    pub samples: Samples,
}

/// A decoded image. `planes[0]` is luma (`width` x `height`). Unless `chroma` is
/// [`Chroma::Mono`], `planes[1]` and `planes[2]` are chroma, `ceil(width / sx)` x
/// `ceil(height / sy)` (`sx`, `sy` are 2, 2 for 4:2:0; 2, 1 for 4:2:2; 1, 1 for 4:4:4). Every
/// further plane (alpha, depth) is `width` x `height`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Planes {
    /// Luma width.
    pub width: u32,
    /// Luma height.
    pub height: u32,
    /// The chroma format.
    pub chroma: Chroma,
    /// The planes, in the order above.
    pub planes: Vec<Plane>,
}

fn us(v: u32) -> usize {
    usize::try_from(v).unwrap_or(usize::MAX)
}

/// The subsampling `(sx, sy)` of plane `index`.
fn sampling(chroma: Chroma, index: usize) -> (u32, u32) {
    match (chroma, index) {
        (Chroma::Yuv420, 1 | 2) => (2, 2),
        (Chroma::Yuv422, 1 | 2) => (2, 1),
        _ => (1, 1),
    }
}

fn fdiv(a: u32, d: u32) -> u32 {
    a.checked_div(d).unwrap_or(0)
}

/// Checks the layout described on [`Planes`].
fn validate(p: &Planes) -> Result<(), Error> {
    let need = if p.chroma == Chroma::Mono { 1 } else { 3 };
    if p.width == 0 || p.height == 0 || p.planes.len() < need {
        return Err(PLANES);
    }
    for (i, pl) in p.planes.iter().enumerate() {
        let (sx, sy) = sampling(p.chroma, i);
        let (w, h) = (p.width.div_ceil(sx), p.height.div_ceil(sy));
        let n = us(w).checked_mul(us(h));
        if pl.width != w || pl.height != h || n != Some(pl.samples.count()) {
            return Err(PLANES);
        }
    }
    Ok(())
}

fn step_size(w: u32, h: u32, t: &Transform) -> Result<(u32, u32), Error> {
    match t {
        Transform::Clap(c) => c
            .crop_rounded(w, h)
            .map(|r| (r.width, r.height))
            .ok_or(Error::Malformed("clap")),
        Transform::Rotate(0 | 180) | Transform::Mirror(_) => Ok((w, h)),
        Transform::Rotate(90 | 270) => Ok((h, w)),
        Transform::Rotate(_) => Err(Error::Malformed("irot")),
    }
}

/// The size [`apply_transforms`] gives for an image of `width` x `height`, without planes. The
/// same rules: each `clap` is cropped on the size after the transforms before it, with
/// [`Clap::crop_rounded`](crate::Clap::crop_rounded); `irot` 90 and 270 swap the sides. Both the
/// input size and the result are checked against `limits.max_pixels`.
///
/// # Errors
///
/// `Malformed("clap")` for a `clap` that [`Clap::crop_rounded`](crate::Clap::crop_rounded)
/// rejects, `Malformed("irot")` for an angle that is not 0, 90, 180 or 270,
/// `LimitExceeded(Pixels)` when the input or the result has more than `limits.max_pixels`.
pub fn transformed_size(
    width: u32,
    height: u32,
    transforms: &[Transform],
    limits: &Limits,
) -> Result<(u32, u32), Error> {
    let over = |w: u32, h: u32| u64::from(w).saturating_mul(u64::from(h)) > limits.max_pixels;
    if over(width, height) {
        return Err(Error::LimitExceeded(Limit::Pixels));
    }
    let (mut w, mut h) = (width, height);
    for t in transforms {
        (w, h) = step_size(w, h, t)?;
    }
    if over(w, h) {
        return Err(Error::LimitExceeded(Limit::Pixels));
    }
    Ok((w, h))
}

/// Applies `transforms` to `planes`, in slice order (the ipma order of
/// [`Image::transforms`](crate::Image::transforms)). An empty list returns `planes` unchanged.
///
/// - `irot` rotates anticlockwise. `imir` with [`Axis::TopBottom`] swaps rows, with
///   [`Axis::LeftRight`] swaps columns. Rotation by 180 and mirrors work in place; a rotation by
///   90 or 270 and a crop allocate new planes.
/// - Each `clap` is cropped with [`Clap::crop_rounded`](crate::Clap::crop_rounded) on the size at
///   that point. The luma crop is the rectangle; a chroma plane with factors `sx`, `sy` takes the
///   origin `floor(x / sx), floor(y / sy)` and the size `ceil(w / sx) x ceil(h / sy)`. The chroma
///   format is kept, so for an odd offset the chroma moves by one luma sample (libheif converts to
///   4:4:4 there instead).
/// - 4:2:2 with a rotation by 90 or 270 has no 4:2:2 result: the chroma planes are made 4:4:4 by
///   copying each sample into two columns (trimmed to the luma width) as they are rotated, and the
///   result is [`Chroma::Yuv444`]. For 4:2:0 and an odd side, each plane is rotated or mirrored at
///   its own size, so the chroma keeps its `ceil` size.
/// - `limits.max_pixels` is checked against the input size and the output size, before any work
///   (not for an empty list). Every new buffer, the plane list included, is charged to
///   `limits.max_alloc` before it is allocated; a buffer that a later step replaces is given back
///   after that step. The input planes are the caller's and are not charged. The cancel flag is
///   checked between steps and at least every 64 rows. Nothing is spawned.
/// - A second `irot`, `imir` or `clap` in the list is applied like the first. libheif rejects a
///   second one of a kind (also with `ignore_transformations`); this is a known difference, so
///   that [`Image::transforms`](crate::Image::transforms) can keep every property as stored.
///
/// # Errors
///
/// `Malformed("planes")` when `planes` do not have the layout of [`Planes`];
/// `Malformed("clap")`, `Malformed("irot")` and `LimitExceeded(Pixels)` as for
/// [`transformed_size`]; `LimitExceeded(Alloc)`, `OutOfMemory`, `Cancelled`.
pub fn apply_transforms(
    mut planes: Planes,
    transforms: &[Transform],
    limits: &Limits,
    cancel: &AtomicBool,
) -> Result<Planes, Error> {
    validate(&planes)?;
    if transforms.is_empty() {
        return Ok(planes);
    }
    transformed_size(planes.width, planes.height, transforms, limits)?;
    let budget = Budget::new(limits, cancel);
    // Bytes charged for the buffers that `planes` holds now (the caller's input is not charged).
    let mut owned = 0usize;
    for t in transforms {
        budget.check_cancel()?;
        let fresh = match *t {
            Transform::Clap(c) => {
                let r = c
                    .crop_rounded(planes.width, planes.height)
                    .ok_or(Error::Malformed("clap"))?;
                crop(&mut planes, r, &budget)?
            }
            Transform::Rotate(deg) => rotate(&mut planes, deg, &budget)?,
            Transform::Mirror(axis) => {
                mirror(&mut planes, axis, &budget)?;
                None
            }
        };
        if let Some(bytes) = fresh {
            budget.release(owned);
            owned = bytes;
        }
    }
    Ok(planes)
}

/// Crops every plane. Returns the bytes charged for the new planes.
fn crop(p: &mut Planes, r: Rect, b: &Budget<'_>) -> Result<Option<usize>, Error> {
    let mut out = b.vec::<Plane>(p.planes.len())?;
    let mut bytes = p.planes.len().saturating_mul(size_of::<Plane>());
    for (i, pl) in p.planes.iter().enumerate() {
        let (sx, sy) = sampling(p.chroma, i);
        let (x, y) = (us(fdiv(r.x, sx)), us(fdiv(r.y, sy)));
        let (w, h) = (r.width.div_ceil(sx), r.height.div_ceil(sy));
        let sw = us(pl.width);
        let samples = match &pl.samples {
            Samples::U8(v) => Samples::U8(crop_plane(v, sw, (x, y), (us(w), us(h)), b)?),
            Samples::U16(v) => Samples::U16(crop_plane(v, sw, (x, y), (us(w), us(h)), b)?),
        };
        bytes = bytes.saturating_add(samples.bytes());
        out.push(Plane {
            width: w,
            height: h,
            samples,
        });
    }
    *p = Planes {
        width: r.width,
        height: r.height,
        chroma: p.chroma,
        planes: out,
    };
    Ok(Some(bytes))
}

fn crop_plane<T: Copy>(
    src: &[T],
    sw: usize,
    (x, y): (usize, usize),
    (w, h): (usize, usize),
    b: &Budget<'_>,
) -> Result<Vec<T>, Error> {
    if sw == 0 {
        return Err(PLANES);
    }
    let n = w.checked_mul(h).ok_or(ALLOC)?;
    let end = x.checked_add(w).ok_or(PLANES)?;
    let mut out = b.vec::<T>(n)?;
    for (i, row) in src.chunks_exact(sw).skip(y).take(h).enumerate() {
        if i & 63 == 0 {
            b.check_cancel()?;
        }
        out.extend_from_slice(row.get(x..end).ok_or(PLANES)?);
    }
    if out.len() != n {
        return Err(PLANES);
    }
    Ok(out)
}

/// Rotates anticlockwise. Returns the bytes charged when new planes were made (90, 270).
fn rotate(p: &mut Planes, deg: u16, b: &Budget<'_>) -> Result<Option<usize>, Error> {
    match deg {
        0 => Ok(None),
        180 => {
            mirror(p, Axis::LeftRight, b)?;
            mirror(p, Axis::TopBottom, b)?;
            Ok(None)
        }
        90 | 270 => {
            let to444 = p.chroma == Chroma::Yuv422;
            let ccw90 = deg == 90;
            let mut out = b.vec::<Plane>(p.planes.len())?;
            let mut bytes = p.planes.len().saturating_mul(size_of::<Plane>());
            for (i, pl) in p.planes.iter().enumerate() {
                // A 4:2:2 chroma plane is read as if each sample were two columns wide.
                let rep = to444 && (i == 1 || i == 2);
                let lw = if rep { p.width } else { pl.width };
                let geo = Geo {
                    sw: us(pl.width),
                    sh: us(pl.height),
                    lw: us(lw),
                    rep,
                    ccw90,
                };
                let samples = match &pl.samples {
                    Samples::U8(v) => Samples::U8(rotate_plane(v, geo, b)?),
                    Samples::U16(v) => Samples::U16(rotate_plane(v, geo, b)?),
                };
                bytes = bytes.saturating_add(samples.bytes());
                out.push(Plane {
                    width: pl.height,
                    height: lw,
                    samples,
                });
            }
            *p = Planes {
                width: p.height,
                height: p.width,
                chroma: if to444 { Chroma::Yuv444 } else { p.chroma },
                planes: out,
            };
            Ok(Some(bytes))
        }
        _ => Err(Error::Malformed("irot")),
    }
}

/// The geometry of one 90 or 270 degree plane rotation: the stored size `sw` x `sh`, the logical
/// width `lw` (twice `sw`, trimmed, when `rep`), and the direction.
#[derive(Clone, Copy)]
struct Geo {
    sw: usize,
    sh: usize,
    lw: usize,
    rep: bool,
    ccw90: bool,
}

/// Rotates one plane in tiles. The output is `sh` wide and `lw` high. For 90 degrees output row
/// `lw - 1 - c` is input column `c`, top to bottom; for 270 degrees output row `c` is input
/// column `c`, bottom to top. Each output row segment is written contiguously while the input
/// column is read with a stride; a tile keeps those reads in cache for the next `TILE` columns.
fn rotate_plane<T: Copy + Default>(src: &[T], g: Geo, b: &Budget<'_>) -> Result<Vec<T>, Error> {
    if g.sw == 0 || g.sh == 0 {
        return Err(PLANES);
    }
    let n = g.lw.checked_mul(g.sh).ok_or(ALLOC)?;
    let mut dst = b.vec::<T>(n)?;
    dst.resize(n, T::default());
    let mut r0 = 0usize;
    while r0 < g.sh {
        let r1 = r0.saturating_add(TILE).min(g.sh);
        let mut c0 = 0usize;
        while c0 < g.lw {
            b.check_cancel()?;
            let c1 = c0.saturating_add(TILE).min(g.lw);
            for c in c0..c1 {
                let sc = if g.rep { c / 2 } else { c };
                let (oy, a, z) = if g.ccw90 {
                    (g.lw.saturating_sub(1).saturating_sub(c), r0, r1)
                } else {
                    (c, g.sh.saturating_sub(r1), g.sh.saturating_sub(r0))
                };
                let seg = dst
                    .chunks_exact_mut(g.sh)
                    .nth(oy)
                    .and_then(|row| row.get_mut(a..z))
                    .ok_or(PLANES)?;
                let col = src
                    .iter()
                    .skip(r0.saturating_mul(g.sw).saturating_add(sc))
                    .step_by(g.sw)
                    .take(r1.saturating_sub(r0));
                if g.ccw90 {
                    for (d, s) in seg.iter_mut().zip(col) {
                        *d = *s;
                    }
                } else {
                    for (d, s) in seg.iter_mut().rev().zip(col) {
                        *d = *s;
                    }
                }
            }
            c0 = c1;
        }
        r0 = r1;
    }
    Ok(dst)
}

/// Mirrors every plane in place.
fn mirror(p: &mut Planes, axis: Axis, b: &Budget<'_>) -> Result<(), Error> {
    for pl in &mut p.planes {
        let w = us(pl.width);
        match &mut pl.samples {
            Samples::U8(v) => mirror_plane(v, w, axis, b)?,
            Samples::U16(v) => mirror_plane(v, w, axis, b)?,
        }
    }
    Ok(())
}

fn mirror_plane<T>(v: &mut [T], w: usize, axis: Axis, b: &Budget<'_>) -> Result<(), Error> {
    if w == 0 {
        return Err(PLANES);
    }
    b.check_cancel()?;
    match axis {
        Axis::LeftRight => {
            for (i, row) in v.chunks_exact_mut(w).enumerate() {
                if i & 63 == 0 {
                    b.check_cancel()?;
                }
                row.reverse();
            }
        }
        Axis::TopBottom => {
            let half = v.len().checked_div(w).ok_or(PLANES)? / 2;
            let cut = half.saturating_mul(w);
            let (top, rest) = v.split_at_mut_checked(cut).ok_or(PLANES)?;
            let middle = rest.len().saturating_sub(cut);
            let (_, bottom) = rest.split_at_mut_checked(middle).ok_or(PLANES)?;
            let pairs = top
                .chunks_exact_mut(w)
                .zip(bottom.chunks_exact_mut(w).rev());
            // Each pair swaps two rows, so 32 pairs are 64 rows.
            for (i, (a, c)) in pairs.enumerate() {
                if i & 31 == 0 {
                    b.check_cancel()?;
                }
                a.swap_with_slice(c);
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Clap;

    fn lim() -> Limits {
        Limits {
            max_threads: 1,
            ..Limits::default()
        }
    }

    fn run(p: Planes, t: &[Transform]) -> Result<Planes, Error> {
        apply_transforms(p, t, &lim(), &AtomicBool::new(false))
    }

    fn val(plane: usize, x: usize, y: usize, wide: bool) -> u32 {
        let base = u32::try_from((plane * 7919 + y * 131 + x * 17) % 251).unwrap_or(0) + 1;
        if wide {
            base + 1000 * u32::try_from(plane + 1).unwrap_or(0)
        } else {
            base
        }
    }

    fn mk_plane(index: usize, w: u32, h: u32, wide: bool) -> Plane {
        let (wu, hu) = (w as usize, h as usize);
        let at = |x: usize, y: usize| val(index, x, y, wide);
        let samples = if wide {
            Samples::U16(
                (0..hu)
                    .flat_map(|y| (0..wu).map(move |x| (x, y)))
                    .map(|(x, y)| at(x, y) as u16)
                    .collect(),
            )
        } else {
            Samples::U8(
                (0..hu)
                    .flat_map(|y| (0..wu).map(move |x| (x, y)))
                    .map(|(x, y)| at(x, y) as u8)
                    .collect(),
            )
        };
        Plane {
            width: w,
            height: h,
            samples,
        }
    }

    fn mk(w: u32, h: u32, chroma: Chroma, wide: bool, extra: bool) -> Planes {
        let mut planes = vec![mk_plane(0, w, h, wide)];
        if chroma != Chroma::Mono {
            let (sx, sy) = sampling(chroma, 1);
            planes.push(mk_plane(1, w.div_ceil(sx), h.div_ceil(sy), wide));
            planes.push(mk_plane(2, w.div_ceil(sx), h.div_ceil(sy), wide));
        }
        if extra {
            let i = planes.len();
            planes.push(mk_plane(i, w, h, wide));
        }
        Planes {
            width: w,
            height: h,
            chroma,
            planes,
        }
    }

    type Grid = Vec<Vec<u32>>;

    fn grid(p: &Plane) -> Grid {
        let all: Vec<u32> = match &p.samples {
            Samples::U8(v) => v.iter().map(|&s| u32::from(s)).collect(),
            Samples::U16(v) => v.iter().map(|&s| u32::from(s)).collect(),
        };
        all.chunks(p.width as usize).map(<[u32]>::to_vec).collect()
    }

    /// The libheif mappings, one sample at a time.
    fn ref_rot(g: &Grid, deg: u16) -> Grid {
        let (h, w) = (g.len(), g[0].len());
        match deg {
            90 => (0..w)
                .map(|y| (0..h).map(|x| g[x][w - 1 - y]).collect())
                .collect(),
            270 => (0..w)
                .map(|y| (0..h).map(|x| g[h - 1 - x][y]).collect())
                .collect(),
            180 => (0..h)
                .map(|y| (0..w).map(|x| g[h - 1 - y][w - 1 - x]).collect())
                .collect(),
            _ => g.clone(),
        }
    }

    fn ref_mirror(g: &Grid, axis: Axis) -> Grid {
        let (h, w) = (g.len(), g[0].len());
        match axis {
            Axis::LeftRight => (0..h)
                .map(|y| (0..w).map(|x| g[y][w - 1 - x]).collect())
                .collect(),
            Axis::TopBottom => (0..h)
                .map(|y| (0..w).map(|x| g[h - 1 - y][x]).collect())
                .collect(),
        }
    }

    fn ref_crop(g: &Grid, x: usize, y: usize, w: usize, h: usize) -> Grid {
        (y..y + h).map(|r| g[r][x..x + w].to_vec()).collect()
    }

    fn is_wide(p: &Plane) -> bool {
        matches!(p.samples, Samples::U16(_))
    }

    const SIZES: [(u32, u32); 9] = [
        (1, 1),
        (2, 2),
        (3, 2),
        (2, 3),
        (5, 4),
        (7, 7),
        (8, 6),
        (70, 67),
        (130, 3),
    ];
    const CHROMAS: [Chroma; 4] = [Chroma::Mono, Chroma::Yuv420, Chroma::Yuv422, Chroma::Yuv444];

    fn check_planes(out: &Planes, expect: &[Grid], wide: bool) {
        assert_eq!(validate(out), Ok(()));
        assert_eq!(out.planes.len(), expect.len());
        for (pl, e) in out.planes.iter().zip(expect) {
            assert_eq!(&grid(pl), e);
            assert_eq!(is_wide(pl), wide);
        }
    }

    #[test]
    fn every_rotation_matches_the_naive_reference() {
        for chroma in CHROMAS {
            for wide in [false, true] {
                for (w, h) in SIZES {
                    for deg in [0u16, 90, 180, 270] {
                        let input = mk(w, h, chroma, wide, true);
                        let quarter = deg == 90 || deg == 270;
                        let to444 = quarter && chroma == Chroma::Yuv422;
                        let expect: Vec<Grid> = input
                            .planes
                            .iter()
                            .enumerate()
                            .map(|(i, pl)| {
                                let mut g = grid(pl);
                                if to444 && (i == 1 || i == 2) {
                                    g = g
                                        .iter()
                                        .map(|r| (0..w as usize).map(|x| r[x / 2]).collect())
                                        .collect();
                                }
                                ref_rot(&g, deg)
                            })
                            .collect();
                        let out = run(input, &[Transform::Rotate(deg)]).unwrap();
                        let (ew, eh) = if quarter { (h, w) } else { (w, h) };
                        assert_eq!((out.width, out.height), (ew, eh));
                        assert_eq!(
                            out.chroma,
                            if to444 { Chroma::Yuv444 } else { chroma },
                            "{chroma:?} {w}x{h} {deg}"
                        );
                        check_planes(&out, &expect, wide);
                    }
                }
            }
        }
    }

    #[test]
    fn every_mirror_matches_the_naive_reference() {
        for chroma in CHROMAS {
            for wide in [false, true] {
                for (w, h) in SIZES {
                    for axis in [Axis::LeftRight, Axis::TopBottom] {
                        let input = mk(w, h, chroma, wide, true);
                        let expect: Vec<Grid> = input
                            .planes
                            .iter()
                            .map(|pl| ref_mirror(&grid(pl), axis))
                            .collect();
                        let out = run(input, &[Transform::Mirror(axis)]).unwrap();
                        assert_eq!((out.width, out.height, out.chroma), (w, h, chroma));
                        check_planes(&out, &expect, wide);
                    }
                }
            }
        }
    }

    #[test]
    fn four_rotations_and_two_mirrors_are_the_identity() {
        for chroma in [Chroma::Mono, Chroma::Yuv420, Chroma::Yuv444] {
            for (w, h) in SIZES {
                let input = mk(w, h, chroma, true, true);
                let r = run(input.clone(), &[Transform::Rotate(90); 4]).unwrap();
                assert_eq!(r, input, "{chroma:?} {w}x{h}");
                let r = run(input.clone(), &[Transform::Rotate(270); 4]).unwrap();
                assert_eq!(r, input);
                let r = run(input.clone(), &[Transform::Rotate(180); 2]).unwrap();
                assert_eq!(r, input);
                for axis in [Axis::LeftRight, Axis::TopBottom] {
                    let r = run(input.clone(), &[Transform::Mirror(axis); 2]).unwrap();
                    assert_eq!(r, input);
                }
            }
        }
    }

    #[test]
    fn mixed_depths_per_plane_are_kept() {
        let mut input = mk(5, 4, Chroma::Yuv420, false, false);
        input.planes[1] = mk_plane(1, 3, 2, true);
        let out = run(input, &[Transform::Rotate(90)]).unwrap();
        assert!(!is_wide(&out.planes[0]));
        assert!(is_wide(&out.planes[1]));
        assert!(!is_wide(&out.planes[2]));
        assert_eq!(validate(&out), Ok(()));
    }

    fn clap_at(cw: u32, ch: u32, hon: i32, von: i32) -> Clap {
        Clap {
            width_n: cw,
            width_d: 1,
            height_n: ch,
            height_d: 1,
            horiz_off_n: hon,
            horiz_off_d: 1,
            vert_off_n: von,
            vert_off_d: 1,
        }
    }

    #[test]
    fn clap_with_odd_and_even_offsets_keeps_the_chroma_format() {
        let cases = [
            (3, 3, 0, 0),
            (3, 3, 1, 1),
            (4, 2, -1, 2),
            (9, 7, 0, 0),
            (1, 1, 2, -2),
            (5, 4, 3, 3),
            (2, 2, 0, 0),
        ];
        for chroma in CHROMAS {
            for wide in [false, true] {
                for (cw, ch, hon, von) in cases {
                    let c = clap_at(cw, ch, hon, von);
                    let r = c.crop_rounded(9, 7).unwrap();
                    let input = mk(9, 7, chroma, wide, true);
                    let expect: Vec<Grid> = input
                        .planes
                        .iter()
                        .enumerate()
                        .map(|(i, pl)| {
                            let (sx, sy) = sampling(chroma, i);
                            ref_crop(
                                &grid(pl),
                                (r.x / sx) as usize,
                                (r.y / sy) as usize,
                                r.width.div_ceil(sx) as usize,
                                r.height.div_ceil(sy) as usize,
                            )
                        })
                        .collect();
                    let out = run(input, &[Transform::Clap(c)]).unwrap();
                    assert_eq!((out.width, out.height), (r.width, r.height));
                    assert_eq!(out.chroma, chroma);
                    check_planes(&out, &expect, wide);
                }
            }
        }
    }

    #[test]
    fn clap_is_computed_on_the_size_after_the_transforms_before_it() {
        let c = clap_at(4, 2, 0, 0);
        let input = mk(8, 6, Chroma::Yuv420, false, false);
        let both = run(input.clone(), &[Transform::Rotate(90), Transform::Clap(c)]).unwrap();
        let step = run(input.clone(), &[Transform::Rotate(90)]).unwrap();
        let step = run(step, &[Transform::Clap(c)]).unwrap();
        assert_eq!(both, step);
        assert_eq!((both.width, both.height), (4, 2));
        // And a different list gives the other size order.
        let other = run(input, &[Transform::Clap(c), Transform::Rotate(90)]).unwrap();
        assert_eq!((other.width, other.height), (2, 4));
    }

    #[test]
    fn clap_outside_empty_or_hostile_is_malformed() {
        let input = || mk(10, 10, Chroma::Yuv420, false, false);
        let bad = |c: Clap| run(input(), &[Transform::Clap(c)]);
        assert_eq!(bad(clap_at(4, 4, 20, 0)), Err(Error::Malformed("clap")));
        assert_eq!(bad(clap_at(0, 4, 0, 0)), Err(Error::Malformed("clap")));
        let huge = Clap {
            width_d: u32::MAX,
            ..clap_at(4, 4, 0, 0)
        };
        assert_eq!(bad(huge), Err(Error::Malformed("clap")));
        let zero = Clap {
            height_d: 0,
            ..clap_at(4, 4, 0, 0)
        };
        assert_eq!(bad(zero), Err(Error::Malformed("clap")));
        // i32::MIN offsets: libheif's clamping gives the full width and height.
        let out = bad(clap_at(4, 4, i32::MIN, i32::MIN)).unwrap();
        assert_eq!((out.width, out.height), (10, 10));
        assert_eq!(
            run(input(), &[Transform::Rotate(45)]),
            Err(Error::Malformed("irot"))
        );
    }

    #[test]
    fn rotating_422_makes_444_by_replication() {
        let luma = Plane {
            width: 3,
            height: 2,
            samples: Samples::U8(vec![10, 11, 12, 13, 14, 15]),
        };
        let chroma = |a: u8, b: u8, c: u8, d: u8| Plane {
            width: 2,
            height: 2,
            samples: Samples::U8(vec![a, b, c, d]),
        };
        let input = Planes {
            width: 3,
            height: 2,
            chroma: Chroma::Yuv422,
            planes: vec![luma, chroma(1, 2, 3, 4), chroma(5, 6, 7, 8)],
        };
        let out = run(input, &[Transform::Rotate(90)]).unwrap();
        assert_eq!((out.width, out.height, out.chroma), (2, 3, Chroma::Yuv444));
        assert_eq!(
            out.planes[0].samples,
            Samples::U8(vec![12, 15, 11, 14, 10, 13])
        );
        assert_eq!(out.planes[1].samples, Samples::U8(vec![2, 4, 1, 3, 1, 3]));
        assert_eq!(out.planes[2].samples, Samples::U8(vec![6, 8, 5, 7, 5, 7]));
        assert_eq!((out.planes[1].width, out.planes[1].height), (2, 3));
    }

    #[test]
    fn alpha_is_a_fourth_full_size_plane() {
        let input = mk(5, 3, Chroma::Yuv420, false, true);
        assert_eq!(input.planes.len(), 4);
        let a = grid(&input.planes[3]);
        let out = run(input, &[Transform::Rotate(270)]).unwrap();
        assert_eq!((out.planes[3].width, out.planes[3].height), (3, 5));
        assert_eq!(grid(&out.planes[3]), ref_rot(&a, 270));
    }

    #[test]
    fn bad_layouts_are_malformed_planes() {
        let t = [Transform::Mirror(Axis::LeftRight)];
        let good = || mk(5, 4, Chroma::Yuv420, false, false);
        assert!(run(good(), &t).is_ok());
        let mut p = good();
        p.planes[1].width = 2; // chroma is 3 x 2
        assert_eq!(run(p, &t), Err(PLANES));
        let mut p = good();
        p.planes[0].samples = Samples::U8(vec![0; 19]);
        assert_eq!(run(p, &t), Err(PLANES));
        let mut p = good();
        p.planes.pop();
        assert_eq!(run(p, &t), Err(PLANES));
        let mut p = good();
        p.width = 6;
        assert_eq!(run(p, &t), Err(PLANES));
        // An empty list validates too.
        let mut p = good();
        p.planes.pop();
        assert_eq!(run(p, &[]), Err(PLANES));
        let mut p = mk(1, 1, Chroma::Mono, false, false);
        p.width = 0;
        p.height = 0;
        p.planes[0].width = 0;
        p.planes[0].height = 0;
        p.planes[0].samples = Samples::U8(Vec::new());
        assert_eq!(run(p, &t), Err(PLANES));
    }

    #[test]
    fn empty_list_returns_the_input() {
        let input = mk(5, 4, Chroma::Yuv422, true, true);
        assert_eq!(run(input.clone(), &[]), Ok(input.clone()));
        // Rotation by 0 changes nothing either.
        assert_eq!(run(input.clone(), &[Transform::Rotate(0)]), Ok(input));
    }

    #[test]
    fn limits_and_cancel() {
        let input = || mk(8, 6, Chroma::Yuv420, false, false);
        let flag = AtomicBool::new(false);
        let rot = [Transform::Rotate(90)];
        let tight = Limits {
            max_alloc: 0,
            ..lim()
        };
        assert_eq!(
            apply_transforms(input(), &rot, &tight, &flag),
            Err(Error::LimitExceeded(Limit::Alloc))
        );
        // Mirror works in place: no allocation at all.
        assert!(
            apply_transforms(
                input(),
                &[Transform::Mirror(Axis::TopBottom)],
                &tight,
                &flag
            )
            .is_ok()
        );
        let few = Limits {
            max_pixels: 47,
            ..lim()
        };
        assert_eq!(
            apply_transforms(input(), &rot, &few, &flag),
            Err(Error::LimitExceeded(Limit::Pixels))
        );
        let enough = Limits {
            max_pixels: 48,
            ..lim()
        };
        assert!(apply_transforms(input(), &rot, &enough, &flag).is_ok());
        let stop = AtomicBool::new(true);
        assert_eq!(
            apply_transforms(input(), &rot, &lim(), &stop),
            Err(Error::Cancelled)
        );
        assert_eq!(
            apply_transforms(
                input(),
                &[Transform::Mirror(Axis::LeftRight)],
                &lim(),
                &stop
            ),
            Err(Error::Cancelled)
        );
    }

    struct Xorshift(u64);

    impl Xorshift {
        fn rand(&mut self) -> u64 {
            self.0 ^= self.0 << 13;
            self.0 ^= self.0 >> 7;
            self.0 ^= self.0 << 17;
            self.0
        }

        fn below(&mut self, n: u64) -> u64 {
            self.rand() % n
        }
    }

    #[test]
    fn transformed_size_equals_the_output_size() {
        let mut rng = Xorshift(0x9E37_79B9_7F4A_7C15);
        for _ in 0..400 {
            let (w, h) = (rng.below(40) as u32 + 1, rng.below(40) as u32 + 1);
            let chroma = CHROMAS[rng.below(4) as usize];
            let mut list = Vec::new();
            for _ in 0..rng.below(6) {
                list.push(match rng.below(3) {
                    0 => Transform::Rotate([0, 90, 180, 270][rng.below(4) as usize]),
                    1 => Transform::Mirror(if rng.below(2) == 0 {
                        Axis::LeftRight
                    } else {
                        Axis::TopBottom
                    }),
                    _ => Transform::Clap(Clap {
                        width_n: rng.below(50) as u32,
                        width_d: rng.below(4) as u32,
                        height_n: rng.below(50) as u32,
                        height_d: rng.below(4) as u32,
                        horiz_off_n: rng.below(30) as i32 - 15,
                        horiz_off_d: rng.below(4) as u32,
                        vert_off_n: rng.below(30) as i32 - 15,
                        vert_off_d: rng.below(4) as u32,
                    }),
                });
            }
            let size = transformed_size(w, h, &list, &lim());
            let out = run(
                mk(w, h, chroma, rng.below(2) == 0, rng.below(2) == 0),
                &list,
            );
            match (size, out) {
                (Ok((sw, sh)), Ok(o)) => {
                    assert_eq!((o.width, o.height), (sw, sh));
                    assert_eq!(validate(&o), Ok(()));
                }
                (Err(a), Err(b)) => assert_eq!(a, b),
                (s, o) => panic!("size {s:?} but output {:?}", o.map(|o| (o.width, o.height))),
            }
        }
    }

    #[test]
    fn a_large_plane_rotates_across_tile_borders() {
        let input = mk(200, 130, Chroma::Yuv420, true, false);
        let g = grid(&input.planes[0]);
        let out = run(input, &[Transform::Rotate(90), Transform::Rotate(270)]).unwrap();
        assert_eq!(grid(&out.planes[0]), g);
    }
}
