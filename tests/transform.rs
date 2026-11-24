//! `apply_transforms`: the `max_alloc` ledger (the peak of buffers alive at once), `max_pixels`,
//! the cancel flag, and `transformed_size` against the `apply_transforms` output.

use std::sync::atomic::AtomicBool;

use isobmff::transform::Samples;
use isobmff::{
    Axis, Chroma, Clap, Error, Limit, Limits, Plane, Planes, Transform, apply_transforms,
    transformed_size,
};

/// The plane list is charged too: `size_of::<Plane>()` per plane of each new step.
const PL: usize = size_of::<Plane>();

fn plane(width: u32, height: u32, wide: bool) -> Plane {
    let n = (width * height) as usize;
    let samples = if wide {
        Samples::U16((0..n).map(|i| i as u16).collect())
    } else {
        Samples::U8((0..n).map(|i| i as u8).collect())
    };
    Plane {
        width,
        height,
        samples,
    }
}

/// 64 x 48: luma 3072 samples; 4:2:0 chroma 768 each; 4:2:2 chroma 1536 each.
fn planes(chroma: Chroma, wide: bool) -> Planes {
    let (cw, ch) = match chroma {
        Chroma::Yuv420 => (32, 24),
        Chroma::Yuv422 => (32, 48),
        _ => (64, 48),
    };
    Planes {
        width: 64,
        height: 48,
        chroma,
        planes: vec![
            plane(64, 48, wide),
            plane(cw, ch, wide),
            plane(cw, ch, wide),
        ],
    }
}

fn limits(max_alloc: usize) -> Limits {
    Limits {
        max_alloc,
        max_threads: 1,
        ..Limits::default()
    }
}

/// `needed` bytes pass, one byte less is `LimitExceeded(Alloc)`.
fn assert_peak(input: Planes, t: &[Transform], needed: usize) {
    let c = AtomicBool::new(false);
    let ok = apply_transforms(input.clone(), t, &limits(needed), &c);
    assert!(ok.is_ok(), "{needed} bytes should pass: {:?}", ok.err());
    assert_eq!(
        apply_transforms(input, t, &limits(needed - 1), &c),
        Err(Error::LimitExceeded(Limit::Alloc)),
        "{} bytes should fail",
        needed - 1
    );
}

fn centre_clap() -> Clap {
    Clap {
        width_n: 32,
        width_d: 1,
        height_n: 24,
        height_d: 1,
        horiz_off_n: 0,
        horiz_off_d: 1,
        vert_off_n: 0,
        vert_off_d: 1,
    }
}

#[test]
fn a_rotation_charges_its_new_planes() {
    let rot = [Transform::Rotate(90)];
    assert_peak(planes(Chroma::Yuv420, false), &rot, 3072 + 2 * 768 + 3 * PL);
    assert_peak(
        planes(Chroma::Yuv420, true),
        &rot,
        2 * (3072 + 2 * 768) + 3 * PL,
    );
    // 4:2:2 chroma becomes 4:4:4: three full planes.
    assert_peak(planes(Chroma::Yuv422, false), &rot, 3 * 3072 + 3 * PL);
    assert_peak(
        planes(Chroma::Yuv444, false),
        &[Transform::Rotate(270)],
        3 * 3072 + 3 * PL,
    );
}

#[test]
fn mirrors_and_180_work_in_place() {
    let c = AtomicBool::new(false);
    for t in [
        Transform::Mirror(Axis::LeftRight),
        Transform::Mirror(Axis::TopBottom),
        Transform::Rotate(180),
        Transform::Rotate(0),
    ] {
        let out = apply_transforms(planes(Chroma::Yuv420, true), &[t], &limits(0), &c);
        assert!(out.is_ok(), "{t:?}");
    }
}

#[test]
fn a_crop_charges_the_cropped_planes() {
    let t = [Transform::Clap(centre_clap())];
    assert_peak(
        planes(Chroma::Yuv420, false),
        &t,
        32 * 24 + 2 * 16 * 12 + 3 * PL,
    );
}

#[test]
fn a_replaced_buffer_is_given_back_after_the_next_step() {
    let rot = Transform::Rotate(90);
    let c = Transform::Clap(centre_clap());
    // The second rotation holds the first result and its own output: twice the planes.
    assert_peak(
        planes(Chroma::Yuv420, false),
        &[rot, rot],
        2 * (4608 + 3 * PL),
    );
    // Three rotations: the first is released before the third is charged, so the peak stays.
    assert_peak(
        planes(Chroma::Yuv420, false),
        &[rot, rot, rot],
        2 * (4608 + 3 * PL),
    );
    // An in-place step keeps what the rotation charged.
    assert_peak(
        planes(Chroma::Yuv420, false),
        &[rot, Transform::Mirror(Axis::LeftRight)],
        4608 + 3 * PL,
    );
    // Rotate, then crop 32 x 24 out of 48 x 64: the rotation (4608) and the crop (1152) coexist.
    assert_peak(
        planes(Chroma::Yuv420, false),
        &[rot, c],
        4608 + 1152 + 6 * PL,
    );
    // Crop, then rotate the small image: the crop (1152) and its rotation (1152).
    assert_peak(planes(Chroma::Yuv420, false), &[c, rot], 2 * 1152 + 6 * PL);
}

#[test]
fn pixels_and_cancel() {
    let c = AtomicBool::new(false);
    let rot = [Transform::Rotate(90)];
    let l = Limits {
        max_pixels: 3071,
        ..limits(1 << 30)
    };
    assert_eq!(
        apply_transforms(planes(Chroma::Yuv420, false), &rot, &l, &c),
        Err(Error::LimitExceeded(Limit::Pixels))
    );
    // The input counts too, even when the crop output fits.
    let l = Limits {
        max_pixels: 768,
        ..limits(1 << 30)
    };
    assert_eq!(
        apply_transforms(
            planes(Chroma::Yuv420, false),
            &[Transform::Clap(centre_clap())],
            &l,
            &c
        ),
        Err(Error::LimitExceeded(Limit::Pixels))
    );
    assert_eq!(
        transformed_size(64, 48, &[Transform::Clap(centre_clap())], &l),
        Err(Error::LimitExceeded(Limit::Pixels))
    );
    let l = Limits {
        max_pixels: 3072,
        ..limits(1 << 30)
    };
    assert!(
        apply_transforms(
            planes(Chroma::Yuv420, false),
            &[Transform::Clap(centre_clap())],
            &l,
            &c
        )
        .is_ok()
    );
    let stop = AtomicBool::new(true);
    assert_eq!(
        apply_transforms(planes(Chroma::Yuv420, false), &rot, &limits(1 << 30), &stop),
        Err(Error::Cancelled)
    );
}

#[test]
fn transformed_size_follows_the_list() {
    let t = [
        Transform::Clap(centre_clap()),
        Transform::Rotate(270),
        Transform::Mirror(Axis::TopBottom),
    ];
    assert_eq!(transformed_size(64, 48, &t, &limits(0)), Ok((24, 32)));
    let out = apply_transforms(
        planes(Chroma::Yuv420, false),
        &t,
        &limits(1 << 30),
        &AtomicBool::new(false),
    )
    .unwrap();
    assert_eq!((out.width, out.height), (24, 32));
    assert_eq!((out.planes[1].width, out.planes[1].height), (12, 16));
}
