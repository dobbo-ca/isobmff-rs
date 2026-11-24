//! MP4/MOV tracks: `parse_movie`, `read_movie` and the sample walks.

mod common;

use std::io::Cursor;
use std::sync::atomic::AtomicBool;

use common::*;
use isobmff::{
    Edit, Error, Limit, Limits, Movie, PropValue, Sample, Track, parse_movie, read_movie,
};

fn parse(file: &[u8]) -> Movie {
    parse_movie(file, &Limits::default(), &AtomicBool::new(false)).unwrap()
}

fn parse_err(file: &[u8]) -> Error {
    parse_movie(file, &Limits::default(), &AtomicBool::new(false)).unwrap_err()
}

fn walk(t: &Track, file: &[u8]) -> Vec<Result<Sample, Error>> {
    let c = AtomicBool::new(false);
    t.samples(Cursor::new(file), &Limits::default(), &c)
        .collect()
}

fn samples(t: &Track, file: &[u8]) -> Vec<Sample> {
    walk(t, file).into_iter().map(Result::unwrap).collect()
}

fn sync(t: &Track, file: &[u8], n: usize) -> Result<Vec<Sample>, Error> {
    let c = AtomicBool::new(false);
    t.sync_samples(Cursor::new(file), n, &Limits::default(), &c)
}

fn at(t: &Track, file: &[u8], time: i64) -> Option<u64> {
    let c = AtomicBool::new(false);
    t.sync_sample_at(Cursor::new(file), time, &Limits::default(), &c)
        .unwrap()
        .map(|s| s.number)
}

fn spans(v: &[Sample]) -> Vec<(u64, u64)> {
    v.iter().map(|s| (s.range.offset, s.range.length)).collect()
}

/// One video track with `stsd` and these tables.
fn file(tables: &[Vec<u8>]) -> Vec<u8> {
    mp4(&moov(&[Trak::video(1, tables).build()], &[]), 400)
}

/// The tables of 4 samples (sizes 3, 4, 5, 6) in two chunks of two, at 28 and 128.
fn four(sizes: Vec<u8>, chunks: Vec<u8>) -> Vec<Vec<u8>> {
    vec![stts(&[(4, 10)]), stsc(&[(1, 2, 1)]), sizes, chunks]
}

/// The samples of `movie_runs`: 7 samples in chunks 1, 2, 5, 6 and 7.
const RUNS: [(u64, u64); 7] = [
    (28, 10),
    (38, 11),
    (128, 12),
    (140, 13),
    (428, 14),
    (528, 15),
    (628, 16),
];

#[test]
fn offsets_follow_the_stsc_runs() {
    let f = movie_runs();
    let m = parse(&f);
    let t = &m.tracks[0];
    assert_eq!(t.sample_count, 7);
    let got = samples(t, &f);
    assert_eq!(spans(&got), RUNS);
    for (i, s) in got.iter().enumerate() {
        assert_eq!(s.number, i as u64 + 1);
        assert_eq!(s.dts, i as i64 * 100);
        assert_eq!((s.cto, s.sync, s.entry, s.flags), (0, true, 1, None));
        assert_eq!(s.pts(), s.dts);
    }
}

#[test]
fn skipping_gives_the_same_offsets_as_walking() {
    for k in 1..=7u32 {
        let f = movie_runs_with(&[stss(&[k])]);
        let m = parse(&f);
        let t = &m.tracks[0];
        let got = sync(t, &f, 5).unwrap();
        assert_eq!(got.len(), 1, "sample {k}");
        let want = RUNS[k as usize - 1];
        assert_eq!(spans(&got), [want], "sample {k}");
        assert_eq!(got[0].number, u64::from(k));
        assert_eq!(got[0].dts, i64::from(k - 1) * 100);
    }
    // Several listed samples, skipping across chunks and the empty run.
    let f = movie_runs_with(&[stss(&[2, 5, 7])]);
    let m = parse(&f);
    let got = sync(&m.tracks[0], &f, 10).unwrap();
    assert_eq!(spans(&got), [RUNS[1], RUNS[4], RUNS[6]]);
    // No stss: the first n samples.
    let f = movie_runs();
    let m = parse(&f);
    let t = &m.tracks[0];
    assert_eq!(spans(&sync(t, &f, 3).unwrap()), RUNS[..3]);
    assert_eq!(sync(t, &f, 100).unwrap().len(), 7);
    assert!(sync(t, &f, 0).unwrap().is_empty());
}

#[test]
fn size_and_chunk_table_forms() {
    let want: [(u64, u64); 4] = [(28, 3), (31, 4), (128, 5), (133, 6)];
    for sizes in [
        stsz(&[3, 4, 5, 6]),
        stz2(8, &[3, 4, 5, 6]),
        stz2(16, &[3, 4, 5, 6]),
        stz2(4, &[3, 4, 5, 6]),
    ] {
        for chunks in [stco(&[28, 128]), co64(&[28, 128])] {
            let f = file(&four(sizes.clone(), chunks));
            let m = parse(&f);
            assert_eq!(spans(&samples(&m.tracks[0], &f)), want);
        }
    }
    // An odd number of 4-bit fields leaves half a byte.
    let f = file(&[
        stts(&[(3, 10)]),
        stsc(&[(1, 3, 1)]),
        stz2(4, &[3, 4, 5]),
        stco(&[28]),
    ]);
    let m = parse(&f);
    assert_eq!(
        spans(&samples(&m.tracks[0], &f)),
        [(28, 3), (31, 4), (35, 5)]
    );
    // A constant sample size.
    let f = file(&four(stsz_const(7, 4), stco(&[28, 128])));
    let m = parse(&f);
    let got = samples(&m.tracks[0], &f);
    assert_eq!(spans(&got), [(28, 7), (35, 7), (128, 7), (135, 7)]);
    let skipped = sync(&m.tracks[0], &f, 4).unwrap();
    assert_eq!(skipped.len(), 4);
    let f = file(&[
        stts(&[(4, 10)]),
        stsc(&[(1, 2, 1)]),
        stsz_const(7, 4),
        stco(&[28, 128]),
        stss(&[4]),
    ]);
    let m = parse(&f);
    assert_eq!(spans(&sync(&m.tracks[0], &f, 1).unwrap()), [(135, 7)]);
}

#[test]
fn sync_flags() {
    let tables = |extra: Vec<u8>| {
        let mut t = four(stsz(&[3, 4, 5, 6]), stco(&[28, 128]));
        t.push(extra);
        file(&t)
    };
    // No stss: every sample is a sync sample.
    let f = file(&four(stsz(&[3, 4, 5, 6]), stco(&[28, 128])));
    let m = parse(&f);
    assert!(samples(&m.tracks[0], &f).iter().all(|s| s.sync));
    // stss lists 2 and 3.
    let f = tables(stss(&[2, 3]));
    let m = parse(&f);
    let flags: Vec<bool> = samples(&m.tracks[0], &f).iter().map(|s| s.sync).collect();
    assert_eq!(flags, [false, true, true, false]);
    // An empty stss: no sync sample.
    let f = tables(stss(&[]));
    let m = parse(&f);
    assert!(samples(&m.tracks[0], &f).iter().all(|s| !s.sync));
    assert!(sync(&m.tracks[0], &f, 4).unwrap().is_empty());
    assert_eq!(at(&m.tracks[0], &f, 0), None);
    // Entries: 0, not increasing, equal.
    for bad in [stss(&[0, 2]), stss(&[3, 2]), stss(&[2, 2])] {
        let f = tables(bad);
        let m = parse(&f);
        assert_eq!(
            sync(&m.tracks[0], &f, 4).err(),
            Some(Error::Malformed("stss"))
        );
        let w = walk(&m.tracks[0], &f);
        assert_eq!(
            w.iter().find_map(|r| r.as_ref().err()),
            Some(&Error::Malformed("stss"))
        );
    }
}

#[test]
fn decode_and_composition_times() {
    let t5 = |stts_box: Vec<u8>, ctts_box: Option<Vec<u8>>| {
        let mut t = vec![stts_box, stsc(&[(1, 5, 1)]), stsz_const(1, 5), stco(&[28])];
        t.extend(ctts_box);
        file(&t)
    };
    // A short stts repeats its last delta.
    let f = t5(stts(&[(2, 10), (1, 5)]), None);
    let m = parse(&f);
    let dts: Vec<i64> = samples(&m.tracks[0], &f).iter().map(|s| s.dts).collect();
    assert_eq!(dts, [0, 10, 20, 25, 30]);
    // An empty stts with samples.
    let f = t5(stts(&[]), None);
    let m = parse(&f);
    assert_eq!(walk(&m.tracks[0], &f)[0], Err(Error::Malformed("stts")));
    // Runs of 0 samples are skipped.
    let f = t5(stts(&[(0, 99), (5, 3)]), None);
    let m = parse(&f);
    let dts: Vec<i64> = samples(&m.tracks[0], &f).iter().map(|s| s.dts).collect();
    assert_eq!(dts, [0, 3, 6, 9, 12]);
    // ctts version 1, negative offsets.
    let f = t5(stts(&[(5, 10)]), Some(ctts(1, &[(2, -5), (3, 20)])));
    let m = parse(&f);
    let got = samples(&m.tracks[0], &f);
    let cto: Vec<i32> = got.iter().map(|s| s.cto).collect();
    assert_eq!(cto, [-5, -5, 20, 20, 20]);
    assert_eq!(got[0].pts(), -5);
    assert_eq!(got[4].pts(), 60);
    // Version 0 is read as signed too (as ffmpeg and gpac do).
    let f = t5(
        stts(&[(5, 10)]),
        Some(ctts(0, &[(1, -10), (1, 0x7fff_ffff)])),
    );
    let m = parse(&f);
    let cto: Vec<i32> = samples(&m.tracks[0], &f).iter().map(|s| s.cto).collect();
    assert_eq!(cto, [-10, i32::MAX, 0, 0, 0]);
    // A short ctts: 0 after its last run.
    let f = t5(stts(&[(5, 10)]), Some(ctts(0, &[(2, 7)])));
    let m = parse(&f);
    let cto: Vec<i32> = samples(&m.tracks[0], &f).iter().map(|s| s.cto).collect();
    assert_eq!(cto, [7, 7, 0, 0, 0]);
    // pts saturates instead of overflowing.
    let s = Sample {
        number: 1,
        range: Default::default(),
        dts: i64::MAX,
        cto: 5,
        sync: true,
        entry: 1,
        flags: None,
    };
    assert_eq!(s.pts(), i64::MAX);
}

#[test]
fn nearest_sync_sample() {
    // stss 1, 4, 6; dts 0, 10, ..., 50; cto 20, -5, -5, 0, 0, 0: pts 20, 30 and 50 for the
    // sync samples.
    let f = movie_sync();
    let m = parse(&f);
    let t = &m.tracks[0];
    let got = samples(t, &f);
    assert_eq!(
        got.iter().map(Sample::pts).collect::<Vec<_>>(),
        [20, 5, 15, 30, 40, 50]
    );
    assert_eq!(
        got.iter().map(|s| s.sync).collect::<Vec<_>>(),
        [true, false, false, true, false, true]
    );
    // Before the first sync sample: the lowest-numbered one.
    assert_eq!(at(t, &f, i64::MIN), Some(1));
    assert_eq!(at(t, &f, 10), Some(1));
    assert_eq!(at(t, &f, 20), Some(1));
    assert_eq!(at(t, &f, 29), Some(1));
    assert_eq!(at(t, &f, 30), Some(4));
    assert_eq!(at(t, &f, 49), Some(4));
    assert_eq!(at(t, &f, 50), Some(6));
    assert_eq!(at(t, &f, i64::MAX), Some(6));
    let s = t
        .sync_sample_at(
            Cursor::new(&f[..]),
            45,
            &Limits::default(),
            &AtomicBool::new(false),
        )
        .unwrap()
        .unwrap();
    assert_eq!((s.number, s.range.length), (4, 7));
    // Equal pts: the lowest number wins (no stss, so every sample is sync).
    let f = file(&[
        stts(&[(2, 10)]),
        ctts(0, &[(1, 10), (1, 0)]),
        stsc(&[(1, 2, 1)]),
        stsz(&[1, 1]),
        stco(&[28]),
    ]);
    let m = parse(&f);
    assert_eq!(at(&m.tracks[0], &f, 10), Some(1));
    assert_eq!(at(&m.tracks[0], &f, 9), Some(1));
    assert_eq!(at(&m.tracks[0], &f, 100), Some(1));
    // A track without samples has no sync sample.
    let f = file(&[stts(&[]), stsc(&[]), stsz(&[]), stco(&[])]);
    let m = parse(&f);
    assert_eq!(at(&m.tracks[0], &f, 0), None);
    assert!(samples(&m.tracks[0], &f).is_empty());
}

#[test]
fn overflow_and_end_of_file() {
    let f = file(&[
        stts(&[(2, 10)]),
        stsc(&[(1, 2, 1)]),
        stsz_const(10, 2),
        co64(&[u64::MAX - 5]),
    ]);
    let m = parse(&f);
    assert_eq!(walk(&m.tracks[0], &f)[0], Err(Error::Malformed("co64")));
    // Past the end of the file: Truncated, and the walk ends there.
    let f = file(&[
        stts(&[(3, 10)]),
        stsc(&[(1, 1, 1)]),
        stsz_const(4, 3),
        stco(&[28, 1_000_000, 28]),
    ]);
    let m = parse(&f);
    let w = walk(&m.tracks[0], &f);
    assert_eq!(w.len(), 2);
    assert!(w[0].is_ok());
    assert_eq!(w[1], Err(Error::Truncated));
    assert_eq!(sync(&m.tracks[0], &f, 3).err(), Some(Error::Truncated));
    // More samples than chunks: the sample table and chunk table disagree.
    let f = file(&[
        stts(&[(4, 10)]),
        stsc(&[(1, 1, 1)]),
        stsz_const(4, 4),
        stco(&[28, 128]),
    ]);
    let m = parse(&f);
    let w = walk(&m.tracks[0], &f);
    assert_eq!(w.len(), 3);
    assert_eq!(w[2], Err(Error::Malformed("stco")));
    // stsc must start at chunk 1 and increase.
    for bad in [stsc(&[(2, 1, 1)]), stsc(&[(1, 1, 1), (1, 2, 1)]), stsc(&[])] {
        let f = file(&[stts(&[(2, 10)]), bad, stsz_const(4, 2), stco(&[28])]);
        let m = parse(&f);
        assert_eq!(walk(&m.tracks[0], &f)[0], Err(Error::Malformed("stsc")));
    }
    // A sample description index that does not exist.
    let f = file(&[
        stts(&[(1, 10)]),
        stsc(&[(1, 1, 2)]),
        stsz_const(4, 1),
        stco(&[28]),
    ]);
    let m = parse(&f);
    assert_eq!(walk(&m.tracks[0], &f)[0], Err(Error::Malformed("stsc")));
}

#[test]
fn counts_that_do_not_fit_are_malformed() {
    for (name, bad) in [
        ("stts", fbx(b"stts", 0, 0, &[0, 0, 0, 9, 0, 0, 0, 1])),
        ("stts", fbx(b"stts", 1, 0, &[0, 0, 0, 0])),
        ("stsc", fbx(b"stsc", 0, 0, &[0xff; 4])),
        ("stss", fbx(b"stss", 0, 0, &[0, 0, 1, 0])),
        ("stco", fbx(b"stco", 0, 0, &[0, 0, 0, 2, 0, 0, 0, 1])),
        ("co64", fbx(b"co64", 0, 0, &[0, 0, 0, 1, 0, 0, 0, 1])),
        (
            "stsz",
            fbx(b"stsz", 0, 0, &[0, 0, 0, 0, 0, 0, 0, 3, 0, 0, 0, 1]),
        ),
        ("stz2", fbx(b"stz2", 0, 0, &[0, 0, 0, 5, 0, 0, 0, 1])),
    ] {
        let mut t = vec![
            stts(&[(1, 10)]),
            stsc(&[(1, 1, 1)]),
            stsz_const(4, 1),
            stco(&[28]),
        ];
        match t.iter().position(|b| b[4..8] == *name.as_bytes()) {
            Some(i) => t[i] = bad,
            None => t.push(bad),
        }
        assert_eq!(parse_err(&file(&t)), Error::Malformed(name), "{name}");
    }
}

#[test]
fn structure_errors() {
    let f = movie_runs();
    // No moov.
    let ftyp = bx(b"ftyp", b"isom\0\0\0\0isom");
    assert_eq!(parse_err(&ftyp), Error::NoMoov);
    assert_eq!(parse_err(&cat(&[&ftyp, &bx(b"mdat", &[1])])), Error::NoMoov);
    // The first box.
    assert_eq!(parse_err(&bx(b"meta", &[0; 8])), Error::UnknownFormat);
    assert_eq!(parse_err(&[]), Error::UnknownFormat);
    // Two moov boxes.
    let m = moov(&[], &[]);
    assert_eq!(parse_err(&cat(&[&ftyp, &m, &m])), Error::Malformed("moov"));
    // cmov.
    assert_eq!(
        parse_err(&cat(&[&ftyp, &moov(&[], &[bx(b"cmov", &[0; 8])])])),
        Error::Unsupported("cmov")
    );
    // A QuickTime file without ftyp.
    let mov = cat(&[&bx(b"mdat", &[0; 64]), &moov(&[], &[])]);
    let p = parse(&mov);
    assert!(p.ftyp.is_none());
    assert!(p.tracks.is_empty());
    assert_eq!(p.timescale, 1000);
    assert_eq!(p.duration, 5000);
    let wide = cat(&[&bx(b"wide", &[]), &bx(b"mdat", &[0; 64]), &moov(&[], &[])]);
    assert!(parse(&wide).ftyp.is_none());
    // A damaged tail after moov ends the walk; before moov it is Truncated.
    let mut tail = f.clone();
    tail.extend_from_slice(&[0, 0, 0, 0x40, b'f', b'r', b'e', b'e']);
    assert_eq!(parse(&tail).tracks.len(), 1);
    let cut = f[..30].to_vec();
    assert_eq!(parse_err(&cut), Error::Truncated);
    // Duplicate track ids, missing and duplicate boxes.
    let a = Trak::video(5, &[stts(&[]), stsc(&[]), stsz(&[]), stco(&[])]).build();
    let b = Trak::video(5, &[stts(&[]), stsc(&[]), stsz(&[]), stco(&[])]).build();
    assert_eq!(
        parse_err(&mp4(&moov(&[a, b], &[]), 10)),
        Error::Malformed("tkhd")
    );
    let all = [stts(&[]), stsc(&[]), stsz(&[]), stco(&[])];
    for (i, name) in ["stts", "stsc", "stsz", "stco"].into_iter().enumerate() {
        let mut t = all.to_vec();
        t.remove(i);
        let e = parse_err(&file(&t));
        assert_eq!(e, Error::Malformed(name));
    }
    let mut t = Trak::video(1, &all);
    t.stbl.remove(0);
    assert_eq!(
        parse_err(&mp4(&moov(&[t.build()], &[]), 10)),
        Error::Malformed("stsd")
    );
    for dup in [
        stts(&[]),
        stsz(&[]),
        stz2(8, &[]),
        stco(&[]),
        co64(&[]),
        stss(&[]),
    ] {
        let mut t = all.to_vec();
        t.push(dup.clone());
        if &dup[4..8] == b"stss" {
            t.push(dup);
        }
        assert_eq!(parse_err(&file(&t)), Error::Malformed("stbl"));
    }
    // A track without mdia.
    let bare = bx(b"trak", &tkhd(1, 1, 1, IDENTITY));
    assert_eq!(
        parse_err(&mp4(&moov(&[bare], &[]), 10)),
        Error::Malformed("mdia")
    );
    // Track id 0.
    let zero = Trak::video(0, &all).build();
    assert_eq!(
        parse_err(&mp4(&moov(&[zero], &[]), 10)),
        Error::Malformed("tkhd")
    );
}

#[test]
fn external_data_references() {
    let dref = |flags: u32| {
        let url = fbx(b"url ", 0, flags, b"http://example.invalid/x\0");
        bx(
            b"dinf",
            &fbx(b"dref", 0, 0, &cat(&[&1u32.to_be_bytes(), &url])),
        )
    };
    let tables = [
        stts(&[(1, 10)]),
        stsc(&[(1, 1, 1)]),
        stsz_const(4, 1),
        stco(&[28]),
    ];
    let make = |dinf: Option<Vec<u8>>| {
        let mut t = Trak::video(1, &tables);
        t.minf_extra.extend(dinf);
        mp4(&moov(&[t.build()], &[]), 100)
    };
    // Self-contained entry, no dref: fine.
    for f in [make(Some(dref(1))), make(None)] {
        let m = parse(&f);
        assert_eq!(samples(&m.tracks[0], &f).len(), 1);
    }
    // The entry points at an external file.
    let f = make(Some(dref(0)));
    let m = parse(&f);
    assert_eq!(
        walk(&m.tracks[0], &f)[0],
        Err(Error::Unsupported("data reference"))
    );
    // The dinf is kept raw.
    assert!(m.tracks[0].other_boxes.iter().any(|(k, _)| k.0 == *b"dinf"));
}

#[test]
fn unparsed_boxes_are_kept_once() {
    let mut t = Trak::video(
        1,
        &[
            stts(&[(1, 10)]),
            stsc(&[(1, 1, 1)]),
            stsz_const(4, 1),
            stco(&[28]),
            bx(b"sdtp", &[0; 8]),
            bx(b"sgpd", &[0; 8]),
        ],
    );
    t.trak_extra.push(bx(b"udta", &[1, 2]));
    t.trak_extra.push(bx(b"tref", &[0; 8]));
    t.minf_extra.push(hdlr(b"dhlr"));
    t.minf_extra.push(bx(b"vmhd", &[0; 12]));
    t.mdia_extra.push(bx(b"uuid", &[7; 20]));
    let f = mp4(
        &moov(
            &[t.build()],
            &[
                bx(b"udta", &[9; 4]),
                bx(b"meta", &[0; 12]),
                bx(b"mvex", &[]),
            ],
        ),
        100,
    );
    let m = parse(&f);
    assert!(m.fragmented);
    let kinds = |v: &[(isobmff::FourCc, isobmff::Extent)]| -> Vec<String> {
        v.iter().map(|(k, _)| k.to_string()).collect()
    };
    assert_eq!(kinds(&m.other_boxes), ["mdat", "udta", "meta", "mvex"]);
    // Every range is a whole box of its type.
    for (k, e) in m.other_boxes.iter().chain(&m.tracks[0].other_boxes) {
        let b = isobmff::Heif::slice(&f, *e).unwrap();
        assert_eq!(&b[4..8], k.as_bytes());
        assert_eq!(
            u32::from_be_bytes(b[..4].try_into().unwrap()) as usize,
            b.len()
        );
    }
    let tr = &m.tracks[0];
    let mut k = kinds(&tr.other_boxes);
    k.sort();
    assert_eq!(k, ["hdlr", "sdtp", "sgpd", "tref", "udta", "uuid", "vmhd"]);
    assert_eq!(kinds(&tr.tables), ["stts", "stsc", "stsz", "stco"]);
}

#[test]
fn track_header_entries_and_edits() {
    let f = movie_two_tracks();
    let m = parse(&f);
    assert_eq!((m.timescale, m.duration), (1000, 5000));
    assert_eq!(m.tracks.len(), 2);
    let v = m.video_track().unwrap();
    assert_eq!(v.id, 1);
    assert_eq!(m.track(2).unwrap().handler.0, *b"soun");
    assert!(m.track(3).is_none());
    assert_eq!((v.timescale, v.duration), (30000, 90000));
    assert_eq!((v.width, v.height), (64 << 16, 48 << 16));
    assert_eq!(v.flags, 3);
    assert_eq!(v.matrix, IDENTITY);
    assert_eq!(
        v.edits,
        [
            Edit {
                segment_duration: 2000,
                media_time: -1,
                rate_integer: 1,
                rate_fraction: 0
            },
            Edit {
                segment_duration: 3000,
                media_time: 0,
                rate_integer: 1,
                rate_fraction: 0
            }
        ]
    );
    assert_eq!(v.entries.len(), 1);
    let e = &v.entries[0];
    assert_eq!(e.kind.0, *b"hvc1");
    assert_eq!((e.data_reference_index, e.size), (1, Some((64, 48))));
    assert_eq!(e.boxes.len(), 2);
    let cfg = e.config().unwrap();
    assert_eq!(cfg.kind.0, *b"hvcC");
    assert!(matches!(cfg.value, PropValue::Hvcc(_)));
    assert_eq!(e.boxes[1].kind.0, *b"pasp");
    // An audio entry is not parsed past its first 8 bytes.
    let a = m.track(2).unwrap();
    assert_eq!(a.entries[0].kind.0, *b"mp4a");
    assert!(a.entries[0].size.is_none() && a.entries[0].boxes.is_empty());
    assert!(a.entries[0].config().is_none());
    assert_eq!(a.sample_count, 3);
    assert_eq!(spans(&samples(a, &f)), [(68, 1), (69, 2), (71, 3)]);
    assert!(m.video_track().is_some_and(|t| std::ptr::eq(t, v)));
    // Version 1 elst, and a rotation matrix.
    let mut t = Trak::video(1, &[stts(&[]), stsc(&[]), stsz(&[]), stco(&[])]);
    t.trak_extra.push(edts_v1(&[(u64::MAX, i64::MIN)]));
    t.matrix = [0, 0x10000, 0, -0x10000, 0, 0, 0, 0, 0x4000_0000];
    let f = mp4(&moov(&[t.build()], &[]), 10);
    let m = parse(&f);
    assert_eq!(
        m.tracks[0].edits,
        [Edit {
            segment_duration: u64::MAX,
            media_time: i64::MIN,
            rate_integer: 1,
            rate_fraction: 0
        }]
    );
    assert_eq!(m.tracks[0].matrix[1], 0x10000);
    assert_eq!(m.tracks[0].matrix[3], -0x10000);
}

#[test]
fn sample_entry_tails() {
    let tables = [stts(&[]), stsc(&[]), stsz(&[]), stco(&[])];
    let with = |stsd: Vec<u8>| {
        let mut t = Trak::video(1, &tables);
        t.stbl[0] = stsd;
        mp4(&moov(&[t.build()], &[]), 10)
    };
    // The 4-byte terminator after the children (QuickTime) is ignored.
    let f = with(stsd_visual(
        b"avc1",
        8,
        8,
        &cat(&[&unknown(b"avcC"), &[0; 4]]),
    ));
    let m = parse(&f);
    assert_eq!(m.tracks[0].entries[0].boxes.len(), 1);
    let cfg = m.tracks[0].entries[0].config().unwrap();
    assert_eq!((cfg.kind.0, &cfg.value), (*b"avcC", &PropValue::Unknown));
    // A tail of 8 or more bytes is a box and must be one.
    assert_eq!(
        parse_err(&with(stsd_visual(b"avc1", 8, 8, &[0, 0, 0, 3, 0, 0, 0, 0]))),
        Error::Malformed("box size")
    );
    // A visual entry shorter than its 78 fixed bytes.
    let short = fbx(
        b"stsd",
        0,
        0,
        &cat(&[&1u32.to_be_bytes(), &bx(b"avc1", &[0; 20])]),
    );
    assert_eq!(parse_err(&with(short)), Error::Malformed("stsd"));
    // max_properties counts the children of all entries together.
    let f = with(stsd_visual(
        b"hvc1",
        8,
        8,
        &cat(&[&hvcc(), &pasp(1, 1), &unknown(b"abcd")]),
    ));
    let l = Limits {
        max_properties: 2,
        ..Limits::default()
    };
    assert_eq!(
        parse_movie(&f, &l, &AtomicBool::new(false)).err(),
        Some(Error::LimitExceeded(Limit::Properties))
    );
}

#[test]
fn limits_and_cancel() {
    let f = movie_runs();
    let c = AtomicBool::new(false);
    let d = Limits::default();
    let moov_cap = Limits {
        max_moov_bytes: 16,
        ..d
    };
    // Only `read_movie` copies the moov.
    assert!(parse_movie(&f, &moov_cap, &c).is_ok());
    assert_eq!(
        read_movie(Cursor::new(&f[..]), &moov_cap, &c).err(),
        Some(Error::LimitExceeded(Limit::MoovBytes))
    );
    assert_eq!(
        Error::LimitExceeded(Limit::MoovBytes).class(),
        "limit-moov-bytes"
    );
    assert_eq!(Error::NoMoov.class(), "no-moov");
    let none = Limits { max_alloc: 0, ..d };
    assert_eq!(
        parse_movie(&f, &none, &c).err(),
        Some(Error::LimitExceeded(Limit::Alloc))
    );
    let few = Limits { max_boxes: 5, ..d };
    assert_eq!(
        parse_movie(&f, &few, &c).err(),
        Some(Error::LimitExceeded(Limit::Boxes))
    );
    let flat = Limits { max_depth: 3, ..d };
    assert_eq!(
        parse_movie(&f, &flat, &c).err(),
        Some(Error::LimitExceeded(Limit::Depth))
    );
    let stop = AtomicBool::new(true);
    assert_eq!(parse_movie(&f, &d, &stop).err(), Some(Error::Cancelled));
    let m = parse(&f);
    let t = &m.tracks[0];
    // The walk charges its windows to max_alloc.
    let tiny = Limits { max_alloc: 4, ..d };
    let first = t.samples(Cursor::new(&f[..]), &tiny, &c).next().unwrap();
    assert_eq!(first, Err(Error::LimitExceeded(Limit::Alloc)));
    assert_eq!(
        t.sync_samples(Cursor::new(&f[..]), 3, &tiny, &c).err(),
        Some(Error::LimitExceeded(Limit::Alloc))
    );
    // A cancel flag that is set before a walk.
    assert_eq!(
        t.samples(Cursor::new(&f[..]), &d, &stop).next(),
        Some(Err(Error::Cancelled))
    );
    assert_eq!(
        t.sync_samples(Cursor::new(&f[..]), 3, &d, &stop).err(),
        Some(Error::Cancelled)
    );
    assert_eq!(
        t.sync_sample_at(Cursor::new(&f[..]), 0, &d, &stop).err(),
        Some(Error::Cancelled)
    );
    // The iterator ends after its first error.
    let mut it = t.samples(Cursor::new(&f[..]), &tiny, &c);
    assert!(it.next().is_some_and(|r| r.is_err()));
    assert!(it.next().is_none());
}

#[test]
fn read_equals_parse_and_walks_through_a_reader() {
    let d = Limits::default();
    let c = AtomicBool::new(false);
    for (name, f) in all_movie_files() {
        let a = parse_movie(&f, &d, &c).unwrap();
        let b = read_movie(Cursor::new(&f[..]), &d, &c).unwrap();
        assert_eq!(a, b, "{name}");
        assert_eq!(a.stats, b.stats, "{name}");
        assert!(a.stats.boxes > 10, "{name}");
        // A walk through `&mut reader` is the same as through a `Cursor` by value.
        let mut r = Cursor::new(&f[..]);
        for t in &a.tracks {
            let by_ref: Vec<_> = t.samples(&mut r, &d, &c).collect();
            assert_eq!(by_ref, walk(t, &f), "{name}");
        }
    }
    // read and parse fail alike.
    let ftyp = bx(b"ftyp", b"isom\0\0\0\0isom");
    let m = moov(&[], &[]);
    let cases = [
        (ftyp.clone(), Error::NoMoov),
        (cat(&[&ftyp, &m, &m]), Error::Malformed("moov")),
        (bx(b"meta", &[0; 8]), Error::UnknownFormat),
        (Vec::new(), Error::UnknownFormat),
        (f_cut(), Error::Truncated),
    ];
    for (file, want) in cases {
        assert_eq!(parse_movie(&file, &d, &c).err(), Some(want.clone()));
        assert_eq!(read_movie(Cursor::new(&file[..]), &d, &c).err(), Some(want));
    }
    // A damaged tail after moov.
    let mut tail = movie_runs();
    tail.extend_from_slice(&[0, 0, 0, 0x40, b'f', b'r', b'e', b'e']);
    assert_eq!(
        parse_movie(&tail, &d, &c).unwrap(),
        read_movie(Cursor::new(&tail[..]), &d, &c).unwrap()
    );
}

/// The movie file cut inside `moov`.
fn f_cut() -> Vec<u8> {
    let f = movie_runs();
    let at = f.windows(4).position(|w| w == b"moov").unwrap();
    f[..at + 12].to_vec()
}

// ---- fragments ----

fn numbers(v: &[Sample]) -> Vec<u64> {
    v.iter().map(|s| s.number).collect()
}

/// A fragmented file with one track and 4-byte samples by default (`trex`), whose `moof` holds
/// `trafs`.
fn one_moof(trafs: Vec<Vec<u8>>) -> Vec<u8> {
    frag_file(1, &[trex(1, 1, 10, 4, 0)], move |_, _| moof(&trafs))
}

#[test]
fn fragment_fields_and_offsets() {
    let f = movie_frag();
    let m = parse(&f);
    assert!(m.fragmented);
    let d = mdat0(&f);
    let (t1, t2) = (m.track(1).unwrap(), m.track(2).unwrap());
    assert_eq!((t1.sample_count, t2.sample_count), (6, 2));
    // Track 1: tfhd over trex, trun over tfhd, default-base-is-moof, a second run without
    // data_offset (continues after the first), an explicit base, tfdt v1, first_sample_flags.
    let s1 = samples(t1, &f);
    assert_eq!(
        spans(&s1),
        [
            (d, 20),
            (d + 20, 20),
            (d + 40, 7),
            (d + 47, 8),
            (d + 104, 6),
            (d + 110, 7)
        ]
    );
    assert_eq!(numbers(&s1), [1, 2, 3, 4, 5, 6]);
    let dts: Vec<i64> = s1.iter().map(|s| s.dts).collect();
    assert_eq!(dts, [0, 200, 400, 450, 5000, 5100]);
    let cto: Vec<i32> = s1.iter().map(|s| s.cto).collect();
    assert_eq!(cto, [0, 0, 0, 0, -5, 3]);
    assert_eq!(s1[4].pts(), 4995);
    let sync_flags: Vec<bool> = s1.iter().map(|s| s.sync).collect();
    assert_eq!(sync_flags, [true, true, true, true, false, true]);
    let flags: Vec<Option<u32>> = s1.iter().map(|s| s.flags).collect();
    let tfhd_flags = Some(0x0200_0000);
    assert_eq!(
        flags,
        [
            tfhd_flags,
            tfhd_flags,
            tfhd_flags,
            tfhd_flags,
            Some(0x10000),
            Some(0)
        ]
    );
    assert!(s1.iter().all(|s| s.entry == 1));
    // Track 2: the trex defaults, and an implicit base after the last run of track 1.
    let s2 = samples(t2, &f);
    assert_eq!(spans(&s2), [(d + 55, 10), (d + 65, 10)]);
    assert_eq!(s2.iter().map(|s| s.dts).collect::<Vec<_>>(), [0, 100]);
    assert!(s2.iter().all(|s| !s.sync && s.flags == Some(0x10000)));
    // Keyframes: fragment samples count like the samples of moov.
    assert_eq!(numbers(&sync(t1, &f, 10).unwrap()), [1, 2, 3, 4, 6]);
    assert_eq!(numbers(&sync(t1, &f, 2).unwrap()), [1, 2]);
    assert!(sync(t2, &f, 5).unwrap().is_empty());
    for (time, want) in [(-10, 1), (399, 2), (449, 3), (450, 4), (5102, 4), (5103, 6)] {
        assert_eq!(at(t1, &f, time), Some(want), "t = {time}");
    }
    assert_eq!(at(t1, &f, i64::MAX), Some(6));
    assert_eq!(at(t2, &f, 0), None);
    // mvex, the mdat and the two mfhd are kept; the moof boxes themselves are parsed.
    let kinds: Vec<String> = m.other_boxes.iter().map(|(k, _)| k.to_string()).collect();
    assert_eq!(kinds, ["mvex", "mdat", "mfhd", "mfhd"]);
}

#[test]
fn tfdt_versions_and_continuation() {
    // Version 0 in the first moof; the second has no tfdt and continues at the end of the first.
    let m1 = |off: u32| {
        moof(&[traf(&[
            tfhd(1, 0x2_0000, &[]),
            tfdt(0, 777),
            trun(0, 0x1, 2, &[off], &[]),
        ])])
    };
    let m2 = |off: u32| {
        moof(&[traf(&[
            tfhd(1, 0x2_0000, &[]),
            trun(0, 0x1, 1, &[off], &[]),
        ])])
    };
    let len1 = m1(0).len() as u64;
    let f = frag_file(1, &[trex(1, 1, 10, 4, 0)], |at, data| {
        cat(&[
            &m1((data - at) as u32),
            &m2((data as i64 + 8 - (at + len1) as i64) as u32),
        ])
    });
    let m = parse(&f);
    let d = mdat0(&f);
    let s = samples(&m.tracks[0], &f);
    assert_eq!(spans(&s), [(d, 4), (d + 4, 4), (d + 8, 4)]);
    assert_eq!(s.iter().map(|s| s.dts).collect::<Vec<_>>(), [777, 787, 797]);
    // Version 1 above i64::MAX is malformed.
    for t in [u64::MAX, 1 << 63] {
        let f = one_moof(vec![traf(&[tfhd(1, 0x2_0000, &[]), tfdt(1, t)])]);
        assert_eq!(parse_err(&f), Error::Malformed("tfdt"));
    }
    // So is a run whose end passes i64::MAX.
    let f = one_moof(vec![traf(&[
        tfhd(1, 0x2_0000, &[]),
        tfdt(1, i64::MAX as u64),
        trun(0, 0x1, 1, &[0], &[]),
    ])]);
    assert_eq!(parse_err(&f), Error::Malformed("trun"));
}

#[test]
fn first_sample_flags_and_per_sample_flags() {
    let f = frag_file(1, &[trex(1, 1, 10, 4, 0)], |at, data| {
        let off = (data - at) as u32;
        moof(&[traf(&[
            tfhd(1, 0x2_0000, &[]),
            // The first sample is not a sync sample; the others have the default (sync).
            trun(0, 0x1 | 0x4, 3, &[off, 0x10000], &[]),
            // A flags field per sample wins over first_sample_flags.
            trun(0, 0x4 | 0x400, 2, &[0x10000], &[0, 0x10000]),
        ])])
    });
    let m = parse(&f);
    let s = samples(&m.tracks[0], &f);
    let d = mdat0(&f);
    assert_eq!(s.len(), 5);
    assert!(
        s.iter()
            .enumerate()
            .all(|(i, x)| x.range.offset == d + 4 * i as u64)
    );
    let flags: Vec<u32> = s.iter().map(|s| s.flags.unwrap()).collect();
    assert_eq!(flags, [0x10000, 0, 0, 0, 0x10000]);
    let sync_flags: Vec<bool> = s.iter().map(|s| s.sync).collect();
    assert_eq!(sync_flags, [false, true, true, true, false]);
}

#[test]
fn trun_cto_is_signed_in_both_versions() {
    for version in [0, 1] {
        let f = one_moof(vec![traf(&[
            tfhd(1, 0x2_0000, &[]),
            trun(version, 0x800, 2, &[], &[-5i32 as u32, 7]),
        ])]);
        let m = parse(&f);
        let s = samples(&m.tracks[0], &f);
        assert_eq!(s.iter().map(|s| s.cto).collect::<Vec<_>>(), [-5, 7]);
        assert_eq!(s[0].pts(), -5);
    }
}

#[test]
fn unknown_track_traf_is_kept_raw() {
    let f = frag_file(1, &[trex(1, 1, 10, 4, 0)], |at, data| {
        let off = (data - at) as u32;
        moof(&[
            traf(&[tfhd(9, 0x2_0000, &[]), trun(0, 0x1, 1, &[off], &[])]),
            traf(&[
                tfhd(1, 0x2_0000, &[]),
                bx(b"senc", &[0; 4]),
                trun(0, 0x1, 1, &[off], &[]),
            ]),
        ])
    });
    let m = parse(&f);
    assert_eq!(m.tracks[0].sample_count, 1);
    let traf_at = f.windows(4).position(|w| w == b"traf").unwrap() as u64 - 4;
    assert!(
        m.other_boxes
            .iter()
            .any(|(k, e)| k.0 == *b"traf" && e.offset == traf_at)
    );
    assert_eq!(
        m.other_boxes
            .iter()
            .filter(|(k, _)| k.0 == *b"traf")
            .count(),
        1
    );
    // A child of the traf of a known track goes to that track.
    assert!(m.tracks[0].other_boxes.iter().any(|(k, _)| k.0 == *b"senc"));
}

#[test]
fn moof_before_moov() {
    let ftyp = bx(b"ftyp", b"isom\0\0\0\0isom");
    let tr = Trak::video(1, &[stts(&[]), stsc(&[]), stsz(&[]), stco(&[])]).build();
    let m = moov(&[tr], &[bx(b"mvex", &trex(1, 1, 10, 4, 0))]);
    let mk = |at: u64, data: u64| {
        moof(&[traf(&[
            tfhd(1, 0x2_0000, &[]),
            trun(0, 0x1, 2, &[(data - at) as u32], &[]),
        ])])
    };
    let at = ftyp.len() as u64;
    let data = at + mk(0, 0).len() as u64 + m.len() as u64 + 8;
    let mdat = bx(b"mdat", &[0; 400]);
    let f = cat(&[&ftyp, &mk(at, data), &m, &mdat]);
    let a = parse(&f);
    assert_eq!(
        spans(&samples(&a.tracks[0], &f)),
        [(data, 4), (data + 4, 4)]
    );
    let b = read_movie(
        Cursor::new(&f[..]),
        &Limits::default(),
        &AtomicBool::new(false),
    )
    .unwrap();
    assert_eq!(a, b);
}

#[test]
fn moof_inside_moov_is_malformed() {
    let tr = Trak::video(1, &[stts(&[]), stsc(&[]), stsz(&[]), stco(&[])]).build();
    let m = moov(
        &[tr],
        &[
            bx(b"mvex", &trex(1, 1, 10, 4, 0)),
            moof(&[traf(&[tfhd(1, 0, &[])])]),
        ],
    );
    assert_eq!(parse_err(&mp4(&m, 4)), Error::Malformed("moov"));
}

#[test]
fn fragment_errors() {
    let id = |id, flags: u32| tfhd(id, flags, &[]);
    let run = || trun(0, 0, 1, &[], &[]);
    let cases = [
        // A moof when moov has no mvex.
        (
            cat(&[
                &mp4(
                    &moov(
                        &[Trak::video(1, &[stts(&[]), stsc(&[]), stsz(&[]), stco(&[])]).build()],
                        &[],
                    ),
                    10,
                ),
                &moof(&[]),
            ]),
            Error::Malformed("mvex"),
        ),
        // A traf of a track without trex.
        (
            frag_file(1, &[], |_, _| moof(&[traf(&[id(1, 0), run()])])),
            Error::Malformed("trex"),
        ),
        (
            frag_file(1, &[trex(1, 1, 1, 1, 0), trex(1, 1, 1, 1, 0)], |_, _| {
                moof(&[])
            }),
            Error::Malformed("trex"),
        ),
        // Sample description 0 and past the entries.
        (
            one_moof(vec![traf(&[tfhd(1, 0x2, &[2]), run()])]),
            Error::Malformed("tfhd"),
        ),
        (
            one_moof(vec![traf(&[tfhd(1, 0x2, &[0]), run()])]),
            Error::Malformed("tfhd"),
        ),
        (
            one_moof(vec![traf(&[tfhd(0, 0, &[])])]),
            Error::Malformed("tfhd"),
        ),
        // tfhd must be the first child.
        (one_moof(vec![traf(&[run()])]), Error::Malformed("traf")),
        (one_moof(vec![traf(&[])]), Error::Malformed("traf")),
        // A count that its records cannot hold.
        (
            one_moof(vec![traf(&[id(1, 0), trun(0, 0x200, 1000, &[], &[1])])]),
            Error::Malformed("trun"),
        ),
        // Samples without a size anywhere.
        (
            frag_file(1, &[trex(1, 1, 10, 0, 0)], |_, _| {
                moof(&[traf(&[id(1, 0), trun(0, 0, 3, &[], &[])])])
            }),
            Error::Malformed("trun"),
        ),
        // base_data_offset + data_offset past u64.
        (
            one_moof(vec![traf(&[
                tfhd(1, 0x1, &[u32::MAX, u32::MAX]),
                trun(0, 0x1, 1, &[5], &[]),
            ])]),
            Error::Malformed("trun"),
        ),
        // A short box.
        (
            one_moof(vec![traf(&[id(1, 0), bx(b"trun", &[0; 6])])]),
            Error::Malformed("trun"),
        ),
    ];
    let (d, c) = (Limits::default(), AtomicBool::new(false));
    for (i, (f, want)) in cases.into_iter().enumerate() {
        assert_eq!(
            parse_movie(&f, &d, &c).err(),
            Some(want.clone()),
            "case {i}"
        );
        assert_eq!(
            read_movie(Cursor::new(&f[..]), &d, &c).err(),
            Some(want),
            "case {i} read"
        );
    }
}

#[test]
fn fragment_sample_past_the_end_is_truncated() {
    let f = frag_file(1, &[trex(1, 1, 10, 100_000, 0)], |_, _| {
        moof(&[traf(&[tfhd(1, 0x2_0000, &[]), trun(0, 0, 2, &[], &[])])])
    });
    let m = parse(&f);
    let w = walk(&m.tracks[0], &f);
    assert_eq!(w, [Err(Error::Truncated)]);
    assert_eq!(sync(&m.tracks[0], &f, 1).err(), Some(Error::Truncated));
}

#[test]
fn moov_samples_continue_into_fragments() {
    let ftyp = bx(b"ftyp", b"isom\0\0\0\0isom");
    let mdat = bx(b"mdat", &[0; 400]);
    let t = Trak::video(
        1,
        &[
            stts(&[(4, 10)]),
            stsc(&[(1, 2, 1)]),
            stsz(&[3, 4, 5, 6]),
            stco(&[28, 128]),
            stss(&[1, 3]),
        ],
    );
    let m = moov(&[t.build()], &[bx(b"mvex", &trex(1, 1, 10, 8, 0))]);
    let moof_at = (ftyp.len() + mdat.len() + m.len()) as u64;
    // The fragment data are 3 samples of 8 bytes at 300, before the moof: a negative data_offset.
    let off = (300 - moof_at as i64) as i32 as u32;
    let frag = moof(&[traf(&[
        tfhd(1, 0x2_0000, &[]),
        trun(0, 0x1 | 0x400, 3, &[off], &[0x10000, 0, 0x10000]),
    ])]);
    let f = cat(&[&ftyp, &mdat, &m, &frag]);
    let m = parse(&f);
    let t = &m.tracks[0];
    assert_eq!(t.sample_count, 7);
    let s = samples(t, &f);
    assert_eq!(numbers(&s), [1, 2, 3, 4, 5, 6, 7]);
    assert_eq!(
        spans(&s),
        [
            (28, 3),
            (31, 4),
            (128, 5),
            (133, 6),
            (300, 8),
            (308, 8),
            (316, 8)
        ]
    );
    // The first fragment sample starts where the stts deltas end.
    assert_eq!(
        s.iter().map(|s| s.dts).collect::<Vec<_>>(),
        [0, 10, 20, 30, 40, 50, 60]
    );
    assert_eq!(s[3].flags, None);
    assert_eq!(s[4].flags, Some(0x10000));
    assert_eq!(numbers(&sync(t, &f, 10).unwrap()), [1, 3, 6]);
    assert_eq!(numbers(&sync(t, &f, 2).unwrap()), [1, 3]);
    assert_eq!(numbers(&sync(t, &f, 3).unwrap()), [1, 3, 6]);
    for (time, want) in [(-1, 1), (19, 1), (20, 3), (49, 3), (50, 6), (1000, 6)] {
        assert_eq!(at(t, &f, time), Some(want), "t = {time}");
    }
    let r = read_movie(
        Cursor::new(&f[..]),
        &Limits::default(),
        &AtomicBool::new(false),
    )
    .unwrap();
    assert_eq!(m, r);
}

#[test]
fn fragment_limits_and_cancel() {
    let (d, c) = (Limits::default(), AtomicBool::new(false));
    // A moof larger than the moov: max_moov_bytes applies to each copied box.
    let big = frag_file(1, &[trex(1, 1, 10, 4, 0)], |at, data| {
        moof(&[traf(&[
            tfhd(1, 0x2_0000, &[]),
            trun(0, 0x1 | 0x100, 400, &[(data - at) as u32], &[10; 400]),
        ])])
    });
    let at = big.windows(4).position(|w| w == b"moov").unwrap();
    let moov_len = u32::from_be_bytes(big[at - 4..at].try_into().unwrap()) as usize;
    let cap = Limits {
        max_moov_bytes: moov_len,
        ..d
    };
    assert!(parse_movie(&big, &cap, &c).is_ok());
    assert_eq!(
        read_movie(Cursor::new(&big[..]), &cap, &c).err(),
        Some(Error::LimitExceeded(Limit::MoovBytes))
    );
    let roomy = Limits {
        max_moov_bytes: moov_len + 3000,
        ..d
    };
    assert_eq!(
        parse_movie(&big, &roomy, &c).unwrap(),
        read_movie(Cursor::new(&big[..]), &roomy, &c).unwrap()
    );
    // The runs are charged to max_alloc: fewer bytes make it fail, never anything else.
    let plain = frag_file(1, &[trex(1, 1, 10, 4, 0)], |_, _| Vec::new());
    let need = |f: &[u8]| {
        (0..1usize << 16).step_by(8).find(|&a| {
            let l = Limits { max_alloc: a, ..d };
            match parse_movie(f, &l, &c) {
                Ok(_) => true,
                Err(e) => {
                    assert_eq!(e, Error::LimitExceeded(Limit::Alloc), "max_alloc {a}");
                    false
                }
            }
        })
    };
    assert!(need(&big).unwrap() > need(&plain).unwrap());
    // A walk charges its window of trun records.
    let f = movie_frag();
    let m = parse(&f);
    let t = &m.tracks[0];
    let tiny = Limits { max_alloc: 4, ..d };
    assert_eq!(
        t.samples(Cursor::new(&f[..]), &tiny, &c).next(),
        Some(Err(Error::LimitExceeded(Limit::Alloc)))
    );
    assert_eq!(
        t.sync_samples(Cursor::new(&f[..]), 2, &tiny, &c).err(),
        Some(Error::LimitExceeded(Limit::Alloc))
    );
    // A cancel flag that is set stops the parse and the walks.
    let stop = AtomicBool::new(true);
    assert_eq!(parse_movie(&f, &d, &stop).err(), Some(Error::Cancelled));
    assert_eq!(
        read_movie(Cursor::new(&f[..]), &d, &stop).err(),
        Some(Error::Cancelled)
    );
    assert_eq!(
        t.samples(Cursor::new(&f[..]), &d, &stop).next(),
        Some(Err(Error::Cancelled))
    );
}

#[test]
fn fuzz_seed_is_a_fragmented_file() {
    for dir in ["movie", "movie_read"] {
        let path = format!(
            "{}/tests/fuzz/payload-seeds/{dir}/fragmented",
            env!("CARGO_MANIFEST_DIR")
        );
        let f = std::fs::read(path).unwrap();
        let m = parse(&f);
        assert!(m.fragmented);
        let t = &m.tracks[0];
        assert_eq!(t.sample_count, 2);
        let s = samples(t, &f);
        assert_eq!(spans(&s), [(468, 4), (472, 4)]);
        assert_eq!(s.iter().map(|s| s.dts).collect::<Vec<_>>(), [100, 110]);
    }
}
