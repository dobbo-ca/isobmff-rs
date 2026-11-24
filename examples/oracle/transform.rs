//! `oracle transform` and `oracle transform-cmp` (docs/parity.md, "Transforms").
//!
//! `transform <in-dir> <raw-dir> <out-dir>` reads the raw planes that `heif-transform-oracle`
//! wrote (`<raw-dir>/<name>/<id>.planes`, no transforms applied), applies the transforms of item
//! `<id>` with `isobmff::apply_transforms` and writes `<out-dir>/<name>/<id>.planes` (or `.err`).
//! An alpha plane (the plane after the luma and chroma planes) takes the transforms of the alpha
//! item, as libheif decodes the alpha item on its own, not those of the main image. Without an
//! alpha item (a grid with alpha in the tiles) it takes the transforms of the main image.
//!
//! `transform-cmp <raw-dir> <libheif-dir> <out-dir> <known.tsv> <graph-known.tsv> <report-dir>`
//! compares the two result trees and writes `transform-counts.md` and `transform.diff` into `<report-dir>`.

use super::{files, limits, opts, usage};
use isobmff::{
    Chroma, Heif, Image, ImageKind, Limits, Plane, PlaneSamples as Samples, Planes,
    apply_transforms,
};
use std::collections::{BTreeMap, BTreeSet};
use std::fmt::Write as _;
use std::fs;
use std::path::Path;
use std::sync::atomic::AtomicBool;

// ---- the .planes format ----

struct Pl<'a> {
    w: u32,
    h: u32,
    bps: u8,
    data: &'a [u8],
}

struct Parsed<'a> {
    chroma: &'a str,
    w: u32,
    h: u32,
    planes: Vec<Pl<'a>>,
}

fn line<'a>(d: &'a [u8], pos: &mut usize) -> Option<&'a str> {
    let rest = d.get(*pos..)?;
    let n = rest.iter().position(|&b| b == b'\n')?;
    *pos = pos.checked_add(n)?.checked_add(1)?;
    std::str::from_utf8(rest.get(..n)?).ok()
}

fn dims(s: &str) -> Option<(u32, u32)> {
    let (a, b) = s.split_once('x')?;
    Some((a.parse().ok()?, b.parse().ok()?))
}

fn parse_planes(d: &[u8]) -> Option<Parsed<'_>> {
    let mut pos = 0;
    let head = line(d, &mut pos)?;
    let mut it = head.split(' ');
    if it.next()? != "planes" {
        return None;
    }
    let chroma = it.next()?;
    let (w, h) = dims(it.next()?)?;
    let n: usize = it.next()?.parse().ok()?;
    if n > 4 || it.next().is_some() {
        return None;
    }
    let mut infos = Vec::new();
    for _ in 0..n {
        let l = line(d, &mut pos)?;
        let (dm, b) = l.split_once(' ')?;
        let (pw, ph) = dims(dm)?;
        let bps: u8 = b.parse().ok()?;
        if bps != 1 && bps != 2 {
            return None;
        }
        infos.push((pw, ph, bps));
    }
    let mut planes = Vec::new();
    for (w, h, bps) in infos {
        let len = usize::try_from(w)
            .ok()?
            .checked_mul(usize::try_from(h).ok()?)?
            .checked_mul(usize::from(bps))?;
        let end = pos.checked_add(len)?;
        planes.push(Pl {
            w,
            h,
            bps,
            data: d.get(pos..end)?,
        });
        pos = end;
    }
    if pos != d.len() {
        return None;
    }
    Some(Parsed {
        chroma,
        w,
        h,
        planes,
    })
}

fn to_planes(p: &Parsed) -> Option<Planes> {
    let chroma = match p.chroma {
        "400" => Chroma::Mono,
        "420" => Chroma::Yuv420,
        "422" => Chroma::Yuv422,
        "444" => Chroma::Yuv444,
        _ => return None,
    };
    let planes = p
        .planes
        .iter()
        .map(|q| Plane {
            width: q.w,
            height: q.h,
            samples: if q.bps == 1 {
                Samples::U8(q.data.to_vec())
            } else {
                let (pairs, _) = q.data.as_chunks::<2>();
                Samples::U16(
                    pairs
                        .iter()
                        .map(|&[a, b]| u16::from_le_bytes([a, b]))
                        .collect(),
                )
            },
        })
        .collect();
    Some(Planes {
        width: p.w,
        height: p.h,
        chroma,
        planes,
    })
}

fn write_planes(chroma: Chroma, w: u32, h: u32, planes: &[Plane]) -> Vec<u8> {
    let cn = match chroma {
        Chroma::Mono => "400",
        Chroma::Yuv420 => "420",
        Chroma::Yuv422 => "422",
        Chroma::Yuv444 => "444",
    };
    let mut o = format!("planes {cn} {w}x{h} {}\n", planes.len()).into_bytes();
    for p in planes {
        let b = match p.samples {
            Samples::U8(_) => 1,
            Samples::U16(_) => 2,
        };
        o.extend_from_slice(format!("{}x{} {b}\n", p.width, p.height).as_bytes());
    }
    for p in planes {
        match &p.samples {
            Samples::U8(v) => o.extend_from_slice(v),
            Samples::U16(v) => {
                for s in v {
                    o.extend_from_slice(&s.to_le_bytes());
                }
            }
        }
    }
    o
}

// ---- transform ----

/// The token of an `.err` file: `clap` for the one error libheif has a counterpart of.
fn token(e: &isobmff::Error) -> &'static str {
    match e {
        isobmff::Error::Malformed("clap") => "clap",
        _ => "error",
    }
}

/// libheif scales an alpha plane to the size of the main image with nearest neighbour,
/// `src = dst * src_size / dst_size`.
fn scale(a: &Plane, dw: u32, dh: u32) -> Option<Plane> {
    fn go<T: Copy>(s: &[T], sw: u32, sh: u32, dw: u32, dh: u32) -> Option<Vec<T>> {
        let mut o = Vec::new();
        for y in 0..dh {
            let sy = u64::from(y) * u64::from(sh) / u64::from(dh);
            for x in 0..dw {
                let sx = u64::from(x) * u64::from(sw) / u64::from(dw);
                let i = usize::try_from(sy * u64::from(sw) + sx).ok()?;
                o.push(*s.get(i)?);
            }
        }
        Some(o)
    }
    if dw == 0 || dh == 0 || a.width == 0 || a.height == 0 {
        return None;
    }
    let samples = match &a.samples {
        Samples::U8(s) => Samples::U8(go(s, a.width, a.height, dw, dh)?),
        Samples::U16(s) => Samples::U16(go(s, a.width, a.height, dw, dh)?),
    };
    Some(Plane {
        width: dw,
        height: dh,
        samples,
    })
}

/// The inputs of a derived image (iden, grid tiles, overlay inputs).
fn inputs(img: &Image) -> Vec<u32> {
    match &img.kind {
        ImageKind::Identity { input } => vec![*input],
        ImageKind::Grid(g) => g.tiles.clone(),
        ImageKind::Overlay(o) => o.inputs.iter().map(|i| i.id).collect(),
        _ => Vec::new(),
    }
}

/// True when an input of `img`, at any depth, has transforms. libheif passes
/// `ignore_transformations` on to grid tiles, iden inputs and overlay inputs (grid.cc:543 and
/// :677, iden.cc:98, overlay.cc:392), so the raw planes of such an image also lack the
/// transforms of its inputs and are not what `img.transforms` applies to. Not compared.
fn inputs_transformed(h: &Heif, img: &Image) -> bool {
    let mut seen = BTreeSet::new();
    let mut todo = inputs(img);
    while let Some(id) = todo.pop() {
        if !seen.insert(id) {
            continue;
        }
        if let Ok(x) = h.image(id) {
            if !x.transforms.is_empty() {
                return true;
            }
            todo.extend(inputs(&x));
        }
    }
    false
}

fn transform_one(
    h: &Heif,
    id: u32,
    raw: &[u8],
    lim: &Limits,
    cancel: &AtomicBool,
) -> Result<Vec<u8>, &'static str> {
    let mut planes = parse_planes(raw).and_then(|p| to_planes(&p)).ok_or("raw")?;
    let img = h.image(id).map_err(|_| "image")?;
    if inputs_transformed(h, &img) {
        return Err("inputs");
    }
    let n_main = if planes.chroma == Chroma::Mono { 1 } else { 3 };
    if planes.planes.len() != n_main && planes.planes.len() != n_main + 1 {
        return Err("raw");
    }
    // An alpha plane without an alpha item of the image (a grid whose tiles have the alpha items)
    // is part of the main decode: libheif assembles it into the grid image (grid.cc:575-604, the
    // alpha plane is filled and each tile's alpha is copied in) and applies the transforms of the
    // grid item to every channel of that image (image_item.cc:1406-1426). So it stays an extra
    // full-size plane of the one `apply_transforms` call below.
    let alpha = match img.alpha {
        Some(aid) if planes.planes.len() == n_main + 1 => planes.planes.pop().map(|a| (a, aid)),
        _ => None,
    };
    let mut res = apply_transforms(planes, &img.transforms, lim, cancel).map_err(|e| token(&e))?;
    if let Some((a, aid)) = alpha {
        let atr = h.image(aid).map_err(|_| "alpha")?.transforms;
        let ap = Planes {
            width: a.width,
            height: a.height,
            chroma: Chroma::Mono,
            planes: vec![a],
        };
        let ap = apply_transforms(ap, &atr, lim, cancel).map_err(|e| token(&e))?;
        let a = ap.planes.into_iter().next().ok_or("alpha")?;
        let a = if a.width == res.width && a.height == res.height {
            a
        } else {
            scale(&a, res.width, res.height).ok_or("alpha")?
        };
        res.planes.push(a);
    }
    Ok(write_planes(res.chroma, res.width, res.height, &res.planes))
}

pub fn run_transform(args: &[String]) {
    let o = opts(args);
    let [inp, raw, out] = &o.pos[..] else { usage() };
    let lim = Limits {
        max_pixels: u64::MAX,
        max_alloc: 2 << 30,
        ..limits(&o)
    };
    let cancel = AtomicBool::new(false);
    for (name, path) in files(inp) {
        let rd = Path::new(raw).join(&name);
        let Ok(entries) = fs::read_dir(&rd) else {
            continue;
        };
        let mut ids: Vec<u32> = entries
            .filter_map(|e| {
                let n = e.ok()?.file_name();
                n.to_str()?.strip_suffix(".planes")?.parse().ok()
            })
            .collect();
        if ids.is_empty() {
            continue;
        }
        ids.sort_unstable();
        let od = Path::new(out).join(&name);
        fs::create_dir_all(&od).unwrap();
        let heif = fs::read(&path)
            .ok()
            .and_then(|d| isobmff::parse(&d, &lim, &cancel).ok());
        for id in ids {
            let res = match (&heif, fs::read(rd.join(format!("{id}.planes")))) {
                (Some(h), Ok(raw)) => transform_one(h, id, &raw, &lim, &cancel),
                (None, _) => Err("parse"),
                (_, Err(_)) => Err("raw"),
            };
            match res {
                Ok(b) => fs::write(od.join(format!("{id}.planes")), b).unwrap(),
                Err(t) => fs::write(od.join(format!("{id}.err")), format!("{t}\n")).unwrap(),
            }
        }
    }
}

// ---- transform-cmp ----

#[derive(Default)]
struct Counts {
    visited: u32,
    inputs: u32,
    raw_err: u32,
    rejected: u32,
    derived: u32,
    images: u32,
    planes: u32,
    exact: u32,
    known: u32,
    failures: u32,
}

/// The first difference of two plane sets, as text.
fn plane_diff(a: &Pl, b: &Pl, k: usize) -> Option<String> {
    if (a.w, a.h, a.bps) != (b.w, b.h, b.bps) {
        return Some(format!(
            "plane {k}: libheif {}x{} {}B, isobmff-rs {}x{} {}B",
            a.w, a.h, a.bps, b.w, b.h, b.bps
        ));
    }
    let i = a.data.iter().zip(b.data).position(|(x, y)| x != y)?;
    Some(format!("plane {k}: first difference at byte {i}"))
}

/// Compares the isobmff-rs result `b` with the libheif result `a`; `Ok` is the number of planes
/// compared. A known entry (libheif changed the chroma format to 4:4:4) compares the luma plane
/// and the alpha plane only.
fn compare(a: &Parsed, b: &Parsed, raw: &Parsed, known: bool) -> Result<u32, String> {
    if known {
        if a.chroma != "444" || !matches!(raw.chroma, "420" | "422") {
            return Err(format!(
                "stale known difference: libheif gives {} for {} input",
                a.chroma, raw.chroma
            ));
        }
        if (a.w, a.h) != (b.w, b.h) || a.planes.len() != b.planes.len() {
            return Err("size or plane count".into());
        }
        let mut n = 0;
        for (k, (x, y)) in a.planes.iter().zip(&b.planes).enumerate() {
            if k == 1 || k == 2 {
                continue;
            }
            if let Some(d) = plane_diff(x, y, k) {
                return Err(d);
            }
            n += 1;
        }
        return Ok(n);
    }
    if a.chroma != b.chroma {
        return Err(format!(
            "chroma libheif {}, isobmff-rs {}",
            a.chroma, b.chroma
        ));
    }
    if (a.w, a.h) != (b.w, b.h) || a.planes.len() != b.planes.len() {
        return Err(format!(
            "size libheif {}x{} ({} planes), isobmff-rs {}x{} ({} planes)",
            a.w,
            a.h,
            a.planes.len(),
            b.w,
            b.h,
            b.planes.len()
        ));
    }
    let mut n = 0;
    for (k, (x, y)) in a.planes.iter().zip(&b.planes).enumerate() {
        if let Some(d) = plane_diff(x, y, k) {
            return Err(d);
        }
        n += 1;
    }
    Ok(n)
}

fn list_ids(dir: &Path) -> Vec<u32> {
    let Ok(rd) = fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut ids: Vec<u32> = rd
        .filter_map(|e| {
            let n = e.ok()?.file_name();
            let n = n.to_str()?;
            n.strip_suffix(".planes")
                .or_else(|| n.strip_suffix(".err"))?
                .parse()
                .ok()
        })
        .collect();
    ids.sort_unstable();
    ids.dedup();
    ids
}

fn token_of(dir: &Path, id: u32) -> Option<String> {
    Some(
        fs::read_to_string(dir.join(format!("{id}.err")))
            .ok()?
            .trim()
            .to_string(),
    )
}

pub fn run_cmp(args: &[String]) {
    let [raw, lh, rs, known_tsv, graph_tsv, rep] = args else {
        usage()
    };
    // files that are a `heif` known difference of the graph gate (tests/parity/known-differences.tsv)
    let graph_known: Vec<String> = fs::read_to_string(graph_tsv)
        .unwrap_or_default()
        .lines()
        .filter_map(|l| {
            let mut f = l.split('\t');
            let (n, side) = (f.next()?, f.next()?);
            (side == "heif" && !n.starts_with('#')).then(|| n.to_string())
        })
        .collect();
    // (name, id) -> reason; the value is set to true once the entry was compared
    let mut known: BTreeMap<(String, u32), bool> = BTreeMap::new();
    for l in fs::read_to_string(known_tsv).unwrap_or_default().lines() {
        let mut f = l.split('\t');
        let (Some(n), Some(Ok(id))) = (f.next(), f.next().map(str::parse::<u32>)) else {
            continue;
        };
        if !n.starts_with('#') {
            known.insert((n.to_string(), id), false);
        }
    }
    let mut c = Counts::default();
    let mut diff = String::new();
    let mut names: Vec<String> = fs::read_dir(raw)
        .map(|rd| {
            rd.filter_map(|e| e.ok()?.file_name().into_string().ok())
                .collect()
        })
        .unwrap_or_default();
    names.sort();
    for name in names {
        c.visited += 1;
        let (rd, ld, sd) = (
            Path::new(raw).join(&name),
            Path::new(lh).join(&name),
            Path::new(rs).join(&name),
        );
        let ids = list_ids(&rd);
        for id in &ids {
            if token_of(&rd, *id).is_some() {
                c.raw_err += 1;
            }
        }
        let ids: Vec<u32> = ids
            .into_iter()
            .filter(|id| rd.join(format!("{id}.planes")).is_file())
            .collect();
        if ids.is_empty() {
            continue;
        }
        // isobmff::parse rejects the file and the graph gate lists it: not a transform difference
        if graph_known.contains(&name)
            && ids
                .iter()
                .any(|id| token_of(&sd, *id).as_deref() == Some("parse"))
        {
            c.rejected += 1;
            continue;
        }
        c.inputs += 1;
        for id in ids {
            if token_of(&sd, id).as_deref() == Some("inputs") {
                c.derived += 1;
                continue;
            }
            c.images += 1;
            let is_known = known.get_mut(&(name.clone(), id)).is_some_and(|u| {
                *u = true;
                true
            });
            let (lf, rf) = (
                fs::read(ld.join(format!("{id}.planes"))),
                fs::read(sd.join(format!("{id}.planes"))),
            );
            let res: Result<(u32, bool), String> = match (lf, rf) {
                (Ok(a), Ok(b)) => {
                    let rawb = fs::read(rd.join(format!("{id}.planes"))).unwrap_or_default();
                    match (parse_planes(&a), parse_planes(&b), parse_planes(&rawb)) {
                        (Some(pa), Some(pb), Some(pr)) => {
                            compare(&pa, &pb, &pr, is_known).map(|n| (n, is_known))
                        }
                        _ => Err("unreadable .planes file".into()),
                    }
                }
                (Err(_), Err(_)) => match (token_of(&ld, id), token_of(&sd, id)) {
                    (Some(a), Some(b)) if a == b && !is_known => Ok((0, false)),
                    (Some(a), Some(b)) => Err(format!("libheif error {a}, isobmff-rs error {b}")),
                    (a, b) => Err(format!("no result: libheif {a:?}, isobmff-rs {b:?}")),
                },
                (Ok(_), Err(_)) => Err(format!(
                    "libheif planes, isobmff-rs {}",
                    token_of(&sd, id).unwrap_or_else(|| "nothing".into())
                )),
                (Err(_), Ok(_)) => Err(format!(
                    "libheif {}, isobmff-rs planes",
                    token_of(&ld, id).unwrap_or_else(|| "nothing".into())
                )),
            };
            match res {
                Ok((n, k)) => {
                    c.planes += n;
                    if k {
                        c.known += 1;
                    } else {
                        c.exact += 1;
                    }
                }
                Err(e) => {
                    c.failures += 1;
                    let _ = writeln!(diff, "{name}\t{id}\t{e}");
                }
            }
        }
    }
    for ((name, id), used) in &known {
        if !used {
            c.failures += 1;
            let _ = writeln!(
                diff,
                "{name}\t{id}\tstale known difference: was not compared"
            );
        }
    }
    let t = format!(
        "| transform: files libheif read / files with a decoded image | {} / {} |\n\
         | transform: images libheif cannot decode or write as planes (not compared) | {} |\n\
         | transform: files isobmff-rs rejects (graph known difference) (not compared) | {} |\n\
         | transform: derived images whose inputs have transforms (not compared) | {} |\n\
         | transform: images compared | {} |\n\
         | transform: planes compared | {} |\n\
         | transform: exact matches (planes or the same error) | {} |\n\
         | transform: known differences (libheif-444: luma and alpha exact, chroma skipped) | {} |\n\
         | transform: failures | {} |\n",
        c.visited,
        c.inputs,
        c.raw_err,
        c.rejected,
        c.derived,
        c.images,
        c.planes,
        c.exact,
        c.known,
        c.failures
    );
    fs::create_dir_all(rep).unwrap();
    fs::write(Path::new(rep).join("transform-counts.md"), t).unwrap();
    fs::write(Path::new(rep).join("transform.diff"), diff).unwrap();
    fs::write(
        Path::new(rep).join("transform-images"),
        format!("{}\n", c.images),
    )
    .unwrap();
}
