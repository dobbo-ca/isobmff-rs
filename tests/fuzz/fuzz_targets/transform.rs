#![no_main]
use std::sync::atomic::AtomicBool;

use isobmff::transform::Samples;
use isobmff::{
    Axis, Chroma, Clap, Error, Limits, Plane, Planes, Transform, apply_transforms, transformed_size,
};
use libfuzzer_sys::fuzz_target;

#[path = "../common.rs"]
mod common;

/// Bytes of the input, then zeros.
struct Rd<'a>(&'a [u8]);

impl Rd<'_> {
    fn u8(&mut self) -> u8 {
        let (b, rest) = self.0.split_first().map_or((0, &[][..]), |(b, r)| (*b, r));
        self.0 = rest;
        b
    }

    fn u32(&mut self) -> u32 {
        u32::from_le_bytes([self.u8(), self.u8(), self.u8(), self.u8()])
    }
}

fn plane(r: &mut Rd<'_>, w: u32, h: u32) -> Plane {
    let n = (w * h) as usize;
    let samples = if r.u8() & 1 == 0 {
        Samples::U8((0..n).map(|_| r.u8()).collect())
    } else {
        Samples::U16(
            (0..n)
                .map(|_| u16::from_le_bytes([r.u8(), r.u8()]))
                .collect(),
        )
    };
    Plane {
        width: w,
        height: h,
        samples,
    }
}

fn clap(r: &mut Rd<'_>) -> Clap {
    if r.u8() & 1 == 0 {
        // Small fields: crops that often fit the image.
        Clap {
            width_n: u32::from(r.u8() % 80),
            width_d: u32::from(r.u8() % 4),
            height_n: u32::from(r.u8() % 80),
            height_d: u32::from(r.u8() % 4),
            horiz_off_n: i32::from(r.u8() as i8),
            horiz_off_d: u32::from(r.u8() % 4),
            vert_off_n: i32::from(r.u8() as i8),
            vert_off_d: u32::from(r.u8() % 4),
        }
    } else {
        Clap {
            width_n: r.u32(),
            width_d: r.u32(),
            height_n: r.u32(),
            height_d: r.u32(),
            horiz_off_n: r.u32() as i32,
            horiz_off_d: r.u32(),
            vert_off_n: r.u32() as i32,
            vert_off_d: r.u32(),
        }
    }
}

/// Two bytes in 1..=160, so a plane spans several 64 x 64 rotation tiles.
fn side(r: &mut Rd<'_>) -> u32 {
    u32::from(u16::from_le_bytes([r.u8(), r.u8()]) % 160) + 1
}

fn samples_len(s: &Samples) -> usize {
    match s {
        Samples::U8(v) => v.len(),
        Samples::U16(v) => v.len(),
    }
}

fn run(input: &Planes, t: &[Transform], l: &Limits, c: &AtomicBool) -> Result<Planes, Error> {
    apply_transforms(input.clone(), t, l, c)
}

// data = chroma, sizes (<= 160 x 160), extra planes, limits, up to 6 transforms, then sample bytes.
fuzz_target!(|data: &[u8]| {
    let mut r = Rd(data);
    let chroma =
        [Chroma::Mono, Chroma::Yuv420, Chroma::Yuv422, Chroma::Yuv444][usize::from(r.u8() % 4)];
    let (w, h) = (side(&mut r), side(&mut r));
    let extra = r.u8() % 3;
    // Generous limits (the target's own, cancel unset) or tiny ones.
    let generous = r.u8() & 1 == 0;
    let limits = if generous {
        common::small_limits()
    } else {
        Limits {
            max_alloc: usize::try_from(r.u32() % (1 << 18)).unwrap_or(0),
            max_pixels: u64::from(r.u32() % (1 << 15)),
            ..common::small_limits()
        }
    };
    let count = r.u8() % 7;
    let mut transforms = Vec::new();
    for _ in 0..count {
        transforms.push(match r.u8() % 3 {
            0 => Transform::Rotate(u16::from(r.u8() % 6) * 90),
            1 => Transform::Mirror(if r.u8() & 1 == 0 {
                Axis::LeftRight
            } else {
                Axis::TopBottom
            }),
            _ => Transform::Clap(clap(&mut r)),
        });
    }
    let mut planes = vec![plane(&mut r, w, h)];
    if chroma != Chroma::Mono {
        let (cw, ch) = match chroma {
            Chroma::Yuv420 => (w.div_ceil(2), h.div_ceil(2)),
            Chroma::Yuv422 => (w.div_ceil(2), h),
            _ => (w, h),
        };
        planes.push(plane(&mut r, cw, ch));
        planes.push(plane(&mut r, cw, ch));
    }
    for _ in 0..extra {
        planes.push(plane(&mut r, w, h));
    }
    let count = planes.len();
    let input = Planes {
        width: w,
        height: h,
        chroma,
        planes,
    };
    let cancel = AtomicBool::new(false);
    let size = transformed_size(w, h, &transforms, &limits);
    // The layout is valid, so an error is the size error or a limit; with generous limits and a
    // size that is Ok there is none.
    let out = match run(&input, &transforms, &limits, &cancel) {
        Ok(out) => out,
        Err(e) => {
            match &size {
                Err(s) => assert_eq!(&e, s),
                Ok(_) => assert!(
                    !generous && matches!(e, Error::LimitExceeded(_)),
                    "{e:?} with size {size:?}, generous {generous}"
                ),
            }
            return;
        }
    };
    // The empty list returns the input, which the pixel limit does not check.
    if !transforms.is_empty() {
        assert_eq!(size, Ok((out.width, out.height)));
    }
    assert_eq!(out.planes.len(), count);
    for (i, p) in out.planes.iter().enumerate() {
        let (sx, sy) = match (out.chroma, i) {
            (Chroma::Yuv420, 1 | 2) => (2, 2),
            (Chroma::Yuv422, 1 | 2) => (2, 1),
            _ => (1, 1),
        };
        assert_eq!(p.width, out.width.div_ceil(sx));
        assert_eq!(p.height, out.height.div_ceil(sy));
        assert_eq!(samples_len(&p.samples), (p.width * p.height) as usize);
    }
    // Rotations and mirrors only: four quarter turns (not 4:2:2, which becomes 4:4:4) and two
    // mirrors of one axis restore the input.
    if generous && transforms.iter().all(|t| !matches!(t, Transform::Clap(_))) {
        let quarter = [Transform::Rotate(90); 4];
        if chroma != Chroma::Yuv422 {
            assert_eq!(run(&input, &quarter, &limits, &cancel).as_ref(), Ok(&input));
        }
        for axis in [Axis::LeftRight, Axis::TopBottom] {
            let twice = [Transform::Mirror(axis); 2];
            assert_eq!(run(&input, &twice, &limits, &cancel).as_ref(), Ok(&input));
        }
    }
});
