//! `oracle gen-transforms <out-dir> <known.tsv>`: minimal HEIF files with `unci` (ISO/IEC
//! 23001-17, uncompressed) items for the transform oracle (docs/parity.md, "Transforms").
//!
//! The files are deterministic (own xorshift, no clock). Each has the main image as item 1, and
//! an alpha item or an Apple gain map item as item 2 (`auxl` to item 1) in the alpha and gain
//! map cases. `known.tsv` lists the (file, image id, `libheif-444`) rows where libheif changes
//! the chroma format to 4:4:4 (the rules of docs/parity.md), predicted from the geometry alone.

use std::fmt::Write as _;
use std::fs;
use std::path::Path;

#[derive(Clone, Copy, PartialEq)]
enum Ch {
    C400,
    C420,
    C422,
    C444,
}

impl Ch {
    fn name(self) -> &'static str {
        match self {
            Ch::C400 => "c400",
            Ch::C420 => "c420",
            Ch::C422 => "c422",
            Ch::C444 => "c444",
        }
    }
    /// `uncC` sampling_type.
    fn sampling(self) -> u8 {
        match self {
            Ch::C400 | Ch::C444 => 0,
            Ch::C422 => 1,
            Ch::C420 => 2,
        }
    }
    /// The chroma plane size as the `unci` decoder of libheif reads it (floor, docs/parity.md).
    fn chroma_dims(self, w: usize, h: usize) -> (usize, usize) {
        match self {
            Ch::C400 => (0, 0),
            Ch::C444 => (w, h),
            Ch::C422 => (w / 2, h),
            Ch::C420 => (w / 2, h / 2),
        }
    }
}

/// A clean aperture, as a rectangle of the size at that point of the transform list.
#[derive(Clone, Copy)]
enum Cl {
    Even,
    OddX,
    OddY,
    OddXY,
    OddSize,
    OutLeft,
    OutRight,
    OutsideRight,
    OutsideLeft,
    /// Raw clap numbers (width n/d, height n/d, horizontal offset n/d, vertical offset n/d); only
    /// as the last transform and only on 4:0:0 and 4:4:4 (the resulting size is not tracked).
    Raw(&'static [i32; 8]),
}

#[derive(Clone, Copy)]
enum Tf {
    /// `irot`: 1, 2 or 3 quarter turns anticlockwise.
    Rot(u8),
    /// `imir`: the axis byte, 0 = top/bottom, 1 = left/right.
    Mir(u8),
    Clap(Cl),
}

use Cl::*;
use Tf::*;

/// (name, transforms): the transform sets of the sweep, in `ipma` order.
const SETS: &[(&str, &[Tf])] = &[
    ("none", &[]),
    ("rot90", &[Rot(1)]),
    ("rot180", &[Rot(2)]),
    ("rot270", &[Rot(3)]),
    ("mirtb", &[Mir(0)]),
    ("mirlr", &[Mir(1)]),
    ("clapeven", &[Clap(Even)]),
    ("clapoddx", &[Clap(OddX)]),
    ("clapoddy", &[Clap(OddY)]),
    ("clapoddxy", &[Clap(OddXY)]),
    ("clapoddsize", &[Clap(OddSize)]),
    ("clapoutleft", &[Clap(OutLeft)]),
    ("clapoutright", &[Clap(OutRight)]),
    ("clapoutsideright", &[Clap(OutsideRight)]),
    ("clapoutsideleft", &[Clap(OutsideLeft)]),
    ("clapoddx-rot90-mirlr", &[Clap(OddX), Rot(1), Mir(1)]),
    ("clapeven-rot270-mirtb", &[Clap(Even), Rot(3), Mir(0)]),
    ("rot90-mirlr", &[Rot(1), Mir(1)]),
    ("mirlr-rot90", &[Mir(1), Rot(1)]),
    ("rot180-mirtb", &[Rot(2), Mir(0)]),
    ("rot90-clapoddx", &[Rot(1), Clap(OddX)]),
    ("rot270-clapoddxy", &[Rot(3), Clap(OddXY)]),
];

fn set(name: &str) -> &'static [Tf] {
    SETS.iter()
        .find(|(n, _)| *n == name)
        .map_or(&[][..], |(_, t)| *t)
}

const ALPHA_URN: &str = "urn:mpeg:mpegB:cicp:systems:auxiliary:alpha";
const GAIN_URN: &str = "urn:com:apple:photo:2020:aux:hdrgainmap";

// ---- geometry ----

/// The rectangle (left, top, width, height) of a clean aperture on a w x h image.
fn rect(c: Cl, w: i64, h: i64) -> (i64, i64, i64, i64) {
    let odd = |v: i64| if v % 2 == 1 { v } else { v - 1 };
    match c {
        Even => (2, 2, (w - 4).max(1), (h - 4).max(1)),
        OddX => (1, 0, (w - 2).max(1), h),
        OddY => (0, 1, w, (h - 2).max(1)),
        OddXY => (1, 1, (w - 2).max(1), (h - 2).max(1)),
        OddSize => (2, 2, odd((w - 4).max(1)).max(1), odd((h - 4).max(1)).max(1)),
        OutLeft => (-2, -1, (w - 1).max(1), (h - 1).max(1)),
        OutRight => (w / 2, h / 2, w, h),
        OutsideRight => (w + 2, h + 2, 3, 3),
        OutsideLeft => (-6, -6, 3, 3),
        Raw(_) => (0, 0, 1, 1),
    }
}

/// The crop libheif makes: left and top clamped to 0, a right or bottom edge that is past the end
/// (or negative) taken as the last column or row; `None` when it is empty.
fn crop(c: Cl, w: i64, h: i64) -> Option<(i64, i64, i64, i64)> {
    if let Raw(_) = c {
        return Some((0, 0, 0, 0));
    }
    let (l, t, cw, ch) = rect(c, w, h);
    let clamp = |v: i64, n: i64| if (0..n).contains(&v) { v } else { n - 1 };
    let (r, b) = (clamp(l + cw - 1, w), clamp(t + ch - 1, h));
    let (l, t) = (l.max(0), t.max(0));
    (l <= r && t <= b).then_some((l, t, r, b))
}

/// Whether libheif converts the image to 4:4:4 on the way (the table of docs/parity.md); `None`
/// when a crop is empty (the decode fails on both sides).
fn converts(ch: Ch, w: u32, h: u32, tfs: &[Tf]) -> Option<bool> {
    let (mut w, mut h) = (i64::from(w), i64::from(h));
    let mut conv = false;
    let sub = matches!(ch, Ch::C420 | Ch::C422);
    for t in tfs {
        match *t {
            Rot(k) => {
                if sub && !conv {
                    conv = match (ch, k) {
                        (Ch::C422, 2) => h % 2 == 1,
                        (Ch::C422, _) => true,
                        (_, 1) => w % 2 == 1,
                        (_, 3) => h % 2 == 1,
                        _ => w % 2 == 1 || h % 2 == 1,
                    };
                }
                if k != 2 {
                    std::mem::swap(&mut w, &mut h);
                }
            }
            Mir(a) => {
                if sub && !conv {
                    conv = if ch == Ch::C422 {
                        a == 1 && w % 2 == 1
                    } else {
                        w % 2 == 1 || h % 2 == 1
                    };
                }
            }
            Clap(c) => {
                let (l, t, r, b) = crop(c, w, h)?;
                if sub && !conv {
                    conv = if ch == Ch::C422 {
                        l % 2 == 1
                    } else {
                        l % 2 == 1 || t % 2 == 1
                    };
                }
                (w, h) = (r - l + 1, b - t + 1);
            }
        }
    }
    Some(conv)
}

// ---- bytes ----

struct Rng(u64);

impl Rng {
    fn new(name: &str) -> Rng {
        let mut h = 0xcbf2_9ce4_8422_2325u64;
        for b in name.bytes() {
            h = (h ^ u64::from(b)).wrapping_mul(0x0100_0000_01b3);
        }
        Rng(h | 1)
    }
    fn step(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }
}

fn bx(t: &[u8; 4], body: &[u8]) -> Vec<u8> {
    let mut o = u32::try_from(body.len() + 8)
        .unwrap_or(u32::MAX)
        .to_be_bytes()
        .to_vec();
    o.extend_from_slice(t);
    o.extend_from_slice(body);
    o
}

/// A full box, version 0, flags 0.
fn fbx(t: &[u8; 4], body: &[u8]) -> Vec<u8> {
    let mut b = vec![0u8; 4];
    b.extend_from_slice(body);
    bx(t, &b)
}

fn u16b(v: usize) -> [u8; 2] {
    u16::try_from(v).unwrap_or(u16::MAX).to_be_bytes()
}

/// One image item: the geometry and the transforms; `aux` is the `auxC` type of an auxiliary item.
struct Spec {
    id: u16,
    w: u32,
    h: u32,
    ch: Ch,
    depth: u8,
    tfs: &'static [Tf],
    aux: Option<&'static str>,
    /// Chroma constant along each row, so libheif's bilinear 4:2:2 upsampling equals replication
    /// and the chroma of a rotated 4:2:2 image can be compared exactly (even widths only).
    rowconst: bool,
}

fn pixels(rng: &mut Rng, n: usize, depth: u8, out: &mut Vec<u8>) {
    let mask = (1u64 << depth) - 1;
    for _ in 0..n {
        let v = (rng.step() >> 16) & mask;
        if depth <= 8 {
            out.push(v as u8);
        } else {
            out.extend_from_slice(&(v as u16).to_be_bytes());
        }
    }
}

/// The item data (component interleave: Y, Cb, Cr planes) and the properties of the item as
/// (box, essential) in `ipma` order.
fn item(s: &Spec, rng: &mut Rng) -> (Vec<u8>, Vec<(Vec<u8>, bool)>) {
    let (w, h) = (s.w as usize, s.h as usize);
    let (cw, chh) = s.ch.chroma_dims(w, h);
    let mut data = Vec::new();
    pixels(rng, w * h, s.depth, &mut data);
    if s.rowconst {
        for _ in 0..chh * 2 {
            let mut one = Vec::new();
            pixels(rng, 1, s.depth, &mut one);
            for _ in 0..cw {
                data.extend_from_slice(&one);
            }
        }
    } else {
        pixels(rng, cw * chh * 2, s.depth, &mut data);
    }

    let ncomp: u32 = if s.ch == Ch::C400 { 1 } else { 3 };
    // 10 and 12 bits: each sample in a big-endian 16-bit container (align_size 2); 8 and 16 bits
    // fill their bytes (align_size 0).
    let align = if s.depth == 10 || s.depth == 12 { 2 } else { 0 };
    let mut uncc = 0u32.to_be_bytes().to_vec();
    uncc.extend_from_slice(&ncomp.to_be_bytes());
    for i in 0..ncomp {
        uncc.extend_from_slice(&u16b(i as usize));
        uncc.extend_from_slice(&[s.depth - 1, 0, align]);
    }
    uncc.extend_from_slice(&[s.ch.sampling(), 0, 0, 0]);
    uncc.extend_from_slice(&[0u8; 20]);
    let mut cmpd = ncomp.to_be_bytes().to_vec();
    if s.ch == Ch::C400 {
        cmpd.extend_from_slice(&0u16.to_be_bytes());
    } else {
        for t in 1u16..=3 {
            cmpd.extend_from_slice(&t.to_be_bytes());
        }
    }

    let mut ispe = s.w.to_be_bytes().to_vec();
    ispe.extend_from_slice(&s.h.to_be_bytes());
    let mut props = vec![
        (fbx(b"ispe", &ispe), false),
        (fbx(b"uncC", &uncc), true),
        (bx(b"cmpd", &cmpd), false),
    ];
    if let Some(urn) = s.aux {
        let mut a = urn.as_bytes().to_vec();
        a.push(0);
        props.push((fbx(b"auxC", &a), false));
    }
    let (mut w, mut h) = (i64::from(s.w), i64::from(s.h));
    for t in s.tfs {
        match *t {
            Rot(k) => {
                props.push((bx(b"irot", &[k]), true));
                if k != 2 {
                    std::mem::swap(&mut w, &mut h);
                }
            }
            Mir(a) => props.push((bx(b"imir", &[a]), true)),
            Clap(Raw(n)) => {
                let mut b = Vec::new();
                for v in n {
                    b.extend_from_slice(&v.to_be_bytes());
                }
                props.push((bx(b"clap", &b), true));
            }
            Clap(c) => {
                let (l, t, cw, ch) = rect(c, w, h);
                let mut b = Vec::new();
                for v in [cw, 1, ch, 1, 2 * l - (w - cw), 2, 2 * t - (h - ch), 2] {
                    b.extend_from_slice(&i32::try_from(v).unwrap_or(0).to_be_bytes());
                }
                props.push((bx(b"clap", &b), true));
                if let Some((l, t, r, bt)) = crop(c, w, h) {
                    (w, h) = (r - l + 1, bt - t + 1);
                }
            }
        }
    }
    (data, props)
}

/// The whole file; item 1 is the primary image, the other items are `auxl` to it.
fn build(specs: &[Spec], seed: &str) -> Vec<u8> {
    let mut rng = Rng::new(seed);
    let items: Vec<_> = specs.iter().map(|s| item(s, &mut rng)).collect();
    let ftyp = bx(b"ftyp", b"mif1\0\0\0\0mif1");

    let meta = |base: u32| -> Vec<u8> {
        let mut hdlr = vec![0u8; 4];
        hdlr.extend_from_slice(b"pict");
        hdlr.extend_from_slice(&[0u8; 12]);
        hdlr.push(0);
        let mut iloc = vec![0x44, 0x00];
        iloc.extend_from_slice(&u16b(specs.len()));
        let mut off = base;
        let mut iinf = u16b(specs.len()).to_vec();
        let mut ipco = Vec::new();
        let mut ipma = (specs.len() as u32).to_be_bytes().to_vec();
        let mut nprops = 0usize;
        let mut iref = Vec::new();
        for (s, (data, props)) in specs.iter().zip(&items) {
            let len = data.len() as u32;
            iloc.extend_from_slice(&s.id.to_be_bytes());
            iloc.extend_from_slice(&[0, 0]);
            iloc.extend_from_slice(&1u16.to_be_bytes());
            iloc.extend_from_slice(&off.to_be_bytes());
            iloc.extend_from_slice(&len.to_be_bytes());
            off += len;
            // infe version 2; flags 1 = hidden for an auxiliary image
            let mut infe = vec![2, 0, 0, u8::from(s.aux.is_some())];
            infe.extend_from_slice(&s.id.to_be_bytes());
            infe.extend_from_slice(&[0, 0]);
            infe.extend_from_slice(b"unci\0");
            iinf.extend_from_slice(&bx(b"infe", &infe));
            ipma.extend_from_slice(&s.id.to_be_bytes());
            ipma.push(props.len() as u8);
            for (b, ess) in props {
                nprops += 1;
                ipco.extend_from_slice(b);
                ipma.push((u8::from(*ess) << 7) | nprops as u8);
            }
            if s.aux.is_some() {
                let mut r = s.id.to_be_bytes().to_vec();
                r.extend_from_slice(&1u16.to_be_bytes());
                r.extend_from_slice(&1u16.to_be_bytes());
                iref.extend_from_slice(&bx(b"auxl", &r));
            }
        }
        let mut m = fbx(b"hdlr", &hdlr);
        m.extend_from_slice(&fbx(b"pitm", &1u16.to_be_bytes()));
        m.extend_from_slice(&fbx(b"iloc", &iloc));
        m.extend_from_slice(&fbx(b"iinf", &iinf));
        if !iref.is_empty() {
            m.extend_from_slice(&fbx(b"iref", &iref));
        }
        let mut iprp = bx(b"ipco", &ipco);
        iprp.extend_from_slice(&fbx(b"ipma", &ipma));
        m.extend_from_slice(&bx(b"iprp", &iprp));
        fbx(b"meta", &m)
    };

    let base = (ftyp.len() + meta(0).len() + 8) as u32;
    let mut out = ftyp;
    out.extend_from_slice(&meta(base));
    let mut mdat = Vec::new();
    for (d, _) in &items {
        mdat.extend_from_slice(d);
    }
    out.extend_from_slice(&bx(b"mdat", &mdat));
    out
}

// ---- the set of files ----

struct Out<'a> {
    dir: &'a Path,
    known: String,
    n: usize,
}

impl Out<'_> {
    /// Writes one file; the main image (item 1) of a file in which libheif converts the chroma to
    /// 4:4:4 goes to the known differences.
    fn add(&mut self, name: &str, specs: &[Spec]) {
        let bytes = build(specs, name);
        fs::write(self.dir.join(format!("{name}.heic")), bytes).unwrap();
        self.n += 1;
        if specs
            .first()
            .is_some_and(|m| !m.rowconst && converts(m.ch, m.w, m.h, m.tfs) == Some(true))
        {
            let _ = writeln!(self.known, "{name}.heic\t1\tlibheif-444");
        }
    }
}

fn main_spec(ch: Ch, depth: u8, w: u32, h: u32, tfs: &'static [Tf]) -> Spec {
    Spec {
        id: 1,
        w,
        h,
        ch,
        depth,
        tfs,
        aux: None,
        rowconst: false,
    }
}

fn aux_spec(w: u32, h: u32, tfs: &'static [Tf], urn: &'static str) -> Spec {
    Spec {
        id: 2,
        w,
        h,
        ch: Ch::C400,
        depth: 8,
        tfs,
        aux: Some(urn),
        rowconst: false,
    }
}

pub fn run_gen(args: &[String]) {
    let [dir, known] = args else { super::usage() };
    fs::create_dir_all(dir).unwrap();
    let mut o = Out {
        dir: Path::new(dir),
        known: String::from("# generated by oracle gen-transforms: file\timage id\treason\n"),
        n: 0,
    };
    let chs = [Ch::C400, Ch::C420, Ch::C422, Ch::C444];
    // geometry sweep: every chroma format, size and transform set at 8 bits
    for ch in chs {
        for (w, h) in [(7, 5), (16, 12), (33, 18)] {
            for (sn, tfs) in SETS {
                let name = format!("unci_{}_d8_{w}x{h}_{sn}", ch.name());
                o.add(&name, &[main_spec(ch, 8, w, h, tfs)]);
            }
        }
    }
    // fractional clap geometry where no chroma conversion hides the result: an odd W with an even
    // clapW (origin x.5: left truncates, top rounds), a fractional width, denominators of 3, and a
    // negative half offset that is partly outside
    const FRAC: &[(&str, &[Tf])] = &[
        ("fraczero", &[Clap(Raw(&[4, 1, 4, 1, 0, 1, 0, 1]))]),
        ("fracwidth", &[Clap(Raw(&[5, 2, 5, 2, 0, 1, 0, 1]))]),
        ("fracthirds", &[Clap(Raw(&[3, 1, 3, 1, 1, 3, 1, 3]))]),
        ("fracneg", &[Clap(Raw(&[8, 1, 6, 1, -1, 2, -1, 2]))]),
    ];
    for ch in [Ch::C400, Ch::C444] {
        for (w, h) in [(7, 5), (16, 12), (33, 18)] {
            for (sn, tfs) in FRAC {
                let name = format!("unci_{}_d8_{w}x{h}_{sn}", ch.name());
                o.add(&name, &[main_spec(ch, 8, w, h, tfs)]);
            }
        }
    }
    // 4:2:2 rotated by 90 and 270: the chroma becomes 4:4:4 by replication; row-constant chroma
    // makes libheif's bilinear conversion identical, so the chroma planes are compared exactly
    for sn in ["rot90", "rot270"] {
        for (w, h) in [(16, 12), (34, 18)] {
            let name = format!("unci_c422_rowconst_{w}x{h}_{sn}");
            let mut m = main_spec(Ch::C422, 8, w, h, set(sn));
            m.rowconst = true;
            o.add(&name, &[m]);
        }
    }
    // depth sweep: 10, 12 and 16 bits
    for depth in [10u8, 12, 16] {
        for ch in chs {
            for (w, h) in [(7, 5), (16, 12)] {
                for sn in ["rot90", "clapoddxy", "mirlr", "clapoddx-rot90-mirlr"] {
                    let name = format!("unci_{}_d{depth}_{w}x{h}_{sn}", ch.name());
                    o.add(&name, &[main_spec(ch, depth, w, h, set(sn))]);
                }
            }
        }
    }
    // alpha: the alpha item has the size of the main image and its own transforms
    for ch in [Ch::C400, Ch::C420, Ch::C444] {
        for sn in [
            "rot90",
            "rot270",
            "mirlr",
            "clapoddxy",
            "clapoddx-rot90-mirlr",
        ] {
            let name = format!("unci_alpha_{}_same_{sn}", ch.name());
            o.add(
                &name,
                &[
                    main_spec(ch, 8, 16, 12, set(sn)),
                    aux_spec(16, 12, set(sn), ALPHA_URN),
                ],
            );
        }
        for sn in ["mirlr", "rot180"] {
            let name = format!("unci_alpha_{}_own_{sn}", ch.name());
            o.add(
                &name,
                &[
                    main_spec(ch, 8, 16, 12, set("none")),
                    aux_spec(16, 12, set(sn), ALPHA_URN),
                ],
            );
        }
        for sn in ["rot90", "mirtb"] {
            let name = format!("unci_alpha_{}_mainonly_{sn}", ch.name());
            o.add(
                &name,
                &[
                    main_spec(ch, 8, 12, 12, set(sn)),
                    aux_spec(12, 12, set("none"), ALPHA_URN),
                ],
            );
        }
    }
    // Apple gain map: a half-size auxiliary image with its own transforms
    for ch in [Ch::C420, Ch::C444] {
        for (mn, gn) in [
            ("none", "rot90"),
            ("none", "clapoddx"),
            ("rot90", "mirlr"),
            ("clapeven", "clapeven"),
            ("rot90", "rot90"),
        ] {
            let name = format!("unci_gain_{}_{mn}_{gn}", ch.name());
            o.add(
                &name,
                &[
                    main_spec(ch, 8, 16, 12, set(mn)),
                    aux_spec(8, 6, set(gn), GAIN_URN),
                ],
            );
        }
    }
    fs::write(known, &o.known).unwrap();
    eprintln!("gen-transforms: {} files", o.n);
}
