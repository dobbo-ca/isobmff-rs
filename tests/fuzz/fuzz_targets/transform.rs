#![no_main]
use std::sync::atomic::AtomicBool;

use isobmff::transform::Samples;
use isobmff::{Axis, Chroma, Clap, Plane, Planes, Transform, apply_transforms, transformed_size};
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

// data = chroma, sizes (<= 64 x 64), extra planes, up to 6 transforms, then sample bytes.
fuzz_target!(|data: &[u8]| {
    let mut r = Rd(data);
    let chroma =
        [Chroma::Mono, Chroma::Yuv420, Chroma::Yuv422, Chroma::Yuv444][usize::from(r.u8() % 4)];
    let (w, h) = (u32::from(r.u8() % 64) + 1, u32::from(r.u8() % 64) + 1);
    let extra = r.u8() % 3;
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
    let size = transformed_size(w, h, &transforms);
    match apply_transforms(input, &transforms, &common::small_limits(), &cancel) {
        Ok(out) => {
            assert_eq!(size, Ok((out.width, out.height)));
            assert_eq!(out.planes.len(), count);
            for (i, p) in out.planes.iter().enumerate() {
                let (sx, sy) = match (out.chroma, i) {
                    (Chroma::Yuv420, 1 | 2) => (2, 2),
                    (Chroma::Yuv422, 1 | 2) => (2, 1),
                    _ => (1, 1),
                };
                assert_eq!(p.width, out.width.div_ceil(sx));
                assert_eq!(p.height, out.height.div_ceil(sy));
                let n = match &p.samples {
                    Samples::U8(v) => v.len(),
                    Samples::U16(v) => v.len(),
                };
                assert_eq!(n, (p.width * p.height) as usize);
            }
        }
        // The size check comes first, so its error is the error of the call.
        Err(e) => {
            if let Err(s) = size {
                assert_eq!(e, s);
            }
        }
    }
});
