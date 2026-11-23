//! The isobmff-rs side of the parity gate (docs/parity.md). Output format, escape rules and line
//! order are those of `tests/parity/c/heif_oracle.c` and `avif_oracle.c`.
//!
//! ```text
//! oracle heif [--threads N] [--read] <in-dir> <out-dir>   graph.txt (+ item-<id>.bin, icc-<id>.bin)
//! oracle avif [--threads N] <in-dir> <out-dir>            avif.txt (+ avif-icc/exif/xmp.bin), AVIF files only
//! oracle gen <in-dir> <out-dir>                           deterministic mutations of files < 1 MiB
//! oracle stats <in-dir> <out.tsv>                         one line of corpus statistics per file
//! ```
//!
//! Every regular file below `<in-dir>` is read (name = relative path with `/` replaced by `__`).
//! `--read` goes through `isobmff::read` over a `BufReader<File>` and reads item bytes through the
//! file (seek + read_exact per extent); the output is identical to the slice path. Every `Err` of
//! the library is an output line, never a panic. `unwrap` is used for output I/O only.
//!
//! Known gaps against libheif: a `meta` line counts the raw item bytes (libheif inflates a
//! compressed mime item). Non-UTF-8 names are lossy on both sides (docs/parity.md). Every error
//! also goes to stderr as `name<TAB>what: message`.

use isobmff::{
    Av1c, Axis, Codec, CodecConfig, Colour, Derived, Extent, FourCc, Fraction, GainMapChannel,
    GainMapMetadata, GainMapSource, Heif, Image, ImageKind, ItemError, Limits, PropValue,
    Transform,
};
use std::fmt::Write as _;
use std::fs::{self, File};
use std::io::{BufReader, Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicBool;

const MAX_FILE: u64 = 512 << 20;
/// Item types that libheif does not expose as metadata (`item_type_is_image` and `rgan`).
const NOT_METADATA: [&[u8; 4]; 13] = [
    b"hvc1", b"av01", b"grid", b"tili", b"iden", b"iovl", b"avc1", b"unci", b"vvc1", b"jpeg",
    b"j2k1", b"mski", b"rgan",
];

fn usage() -> ! {
    eprintln!(
        "usage: oracle heif [--threads N] [--read] <in-dir> <out-dir>\n       oracle avif [--threads N] <in-dir> <out-dir>\n       oracle gen <in-dir> <out-dir>\n       oracle stats <in-dir> <out.tsv>"
    );
    std::process::exit(2)
}

// ---- directory walk, escapes ----

fn walk(root: &Path, dir: &Path, out: &mut Vec<(String, PathBuf)>) {
    let Ok(rd) = fs::read_dir(dir) else {
        eprintln!("oracle: cannot read {}", dir.display());
        return;
    };
    let mut paths: Vec<PathBuf> = rd.filter_map(|e| e.ok().map(|e| e.path())).collect();
    paths.sort();
    for p in paths {
        if fs::symlink_metadata(&p).is_ok_and(|m| m.is_dir()) {
            walk(root, &p, out);
            continue;
        }
        let Ok(m) = fs::metadata(&p) else { continue };
        if !m.is_file() {
            continue;
        }
        if m.len() > MAX_FILE {
            eprintln!("oracle: {}: larger than 512 MiB, skipped", p.display());
            continue;
        }
        let Ok(rel) = p.strip_prefix(root) else {
            continue;
        };
        let parts: Vec<String> = rel
            .components()
            .map(|c| c.as_os_str().to_string_lossy().into_owned())
            .collect();
        out.push((parts.join("__"), p));
    }
}

fn files(dir: &str) -> Vec<(String, PathBuf)> {
    let root = Path::new(dir);
    let mut v = Vec::new();
    walk(root, root, &mut v);
    v
}

/// Bytes 0x21..0x7e except `%` as is, the rest `%XX`; empty is `""`.
fn esc(o: &mut String, s: &[u8]) {
    if s.is_empty() {
        o.push_str("\"\"");
        return;
    }
    for &c in s {
        if (0x21..=0x7e).contains(&c) && c != b'%' {
            o.push(char::from(c));
        } else {
            let _ = write!(o, "%{c:02X}");
        }
    }
}

fn fcc(o: &mut String, f: FourCc) {
    esc(o, f.as_bytes());
}

fn ids(o: &mut String, v: &[u32]) {
    if v.is_empty() {
        o.push('-');
        return;
    }
    for (i, x) in v.iter().enumerate() {
        if i > 0 {
            o.push(',');
        }
        let _ = write!(o, "{x}");
    }
}

fn axis(a: Axis) -> u8 {
    match a {
        Axis::TopBottom => 0,
        Axis::LeftRight => 1,
    }
}

// ---- options ----

struct Opts {
    threads: Option<usize>,
    read: bool,
    pos: Vec<String>,
}

fn opts(args: &[String]) -> Opts {
    let mut o = Opts {
        threads: None,
        read: false,
        pos: Vec::new(),
    };
    let mut it = args.iter();
    while let Some(a) = it.next() {
        match a.as_str() {
            "--threads" => {
                o.threads = Some(
                    it.next()
                        .and_then(|v| v.parse().ok())
                        .unwrap_or_else(|| usage()),
                );
            }
            "--read" => o.read = true,
            _ => o.pos.push(a.clone()),
        }
    }
    o
}

fn limits(o: &Opts) -> Limits {
    let mut l = Limits::default();
    if let Some(t) = o.threads {
        l.max_threads = t;
    }
    l
}

// ---- where item bytes come from ----

enum Src<'a> {
    Mem(&'a [u8]),
    File(File, u64),
}

impl Src<'_> {
    /// The bytes of one extent; `None` when it is not inside the file.
    fn extent(&mut self, e: Extent) -> Option<Vec<u8>> {
        match self {
            Src::Mem(d) => Heif::slice(d, e).map(<[u8]>::to_vec),
            Src::File(f, len) => {
                if e.end()? > *len {
                    return None;
                }
                let mut v = vec![0u8; usize::try_from(e.length).ok()?];
                f.seek(SeekFrom::Start(e.offset)).ok()?;
                f.read_exact(&mut v).ok()?;
                Some(v)
            }
        }
    }

    /// `Heif::item_bytes` (raw extents concatenated); the file path has the same rules.
    fn item_bytes(&mut self, h: &Heif, id: u32, lim: &Limits) -> Option<Vec<u8>> {
        if let Src::Mem(d) = self {
            let cancel = AtomicBool::new(false);
            return h
                .item_bytes(d, id, lim, &cancel)
                .ok()
                .map(std::borrow::Cow::into_owned);
        }
        let ex = h.item(id)?.data.as_ref().ok()?;
        match ex.as_slice() {
            [] => Some(Vec::new()),
            [e] => self.extent(*e),
            many => {
                let mut total = 0u64;
                for e in many {
                    total = total.checked_add(e.length)?;
                }
                if usize::try_from(total).ok()? > lim.max_alloc {
                    return None;
                }
                let mut v = Vec::new();
                for e in many {
                    v.extend_from_slice(&self.extent(*e)?);
                }
                Some(v)
            }
        }
    }
}

// ---- heif: graph.txt ----

/// The last `nclx` (libheif keeps the last `colr` of each kind); (2, 2, 2, full range) is "no
/// profile" in libheif.
fn first_nclx(im: &Image) -> Option<(u16, u16, u16, bool)> {
    im.colour
        .iter()
        .rev()
        .find_map(|c| match c {
            Colour::Nclx {
                primaries,
                transfer,
                matrix,
                full_range,
            } => Some((*primaries, *transfer, *matrix, *full_range)),
            Colour::Icc(_) => None,
        })
        .filter(|&n| n != (2, 2, 2, true))
}

/// The last ICC profile.
fn first_icc(im: &Image) -> Option<Extent> {
    im.colour.iter().rev().find_map(|c| match c {
        Colour::Icc(e) => Some(*e),
        Colour::Nclx { .. } => None,
    })
}

fn image_block(o: &mut String, h: &Heif, src: &mut Src<'_>, lim: &Limits, dir: &Path, img: &Image) {
    let id = img.id;
    let ty = h.item(id).map_or([0; 4], |i| i.item_type.0);
    let (sw, sh) = img.display_size().unwrap_or((0, 0));
    let _ = write!(o, "image {id} ispe=");
    if img.width == 0 && img.height == 0 {
        o.push('-');
    } else {
        let _ = write!(o, "{}x{}", img.width, img.height);
    }
    // libheif reports alpha for a grid when any tile has alpha.
    let alpha = img.alpha.is_some()
        || matches!(&img.kind, ImageKind::Grid(g)
            if g.tiles.iter().any(|t| h.image(*t).is_ok_and(|i| i.alpha.is_some())));
    let _ = write!(
        o,
        " size={sw}x{sh} alpha={} prem={} bits=",
        u8::from(alpha),
        u8::from(img.premultiplied)
    );
    match (&img.kind, &ty) {
        (
            ImageKind::Coded {
                codec: Codec::Hevc,
                config: CodecConfig::Hevc(c),
            },
            b"hvc1",
        ) => {
            let _ = write!(o, "{}/{}", c.bit_depth_luma, c.bit_depth_chroma);
        }
        (
            ImageKind::Coded {
                codec: Codec::Av1,
                config: CodecConfig::Av1(a),
            },
            b"av01",
        ) => {
            let _ = write!(o, "{0}/{0}", a.depth());
        }
        _ => o.push('-'),
    }
    o.push('\n');

    // A grid without a profile of its own takes the one of its first tile (libheif).
    let tile0 = match &img.kind {
        ImageKind::Grid(g) => g.tiles.first().and_then(|t| h.image(*t).ok()),
        _ => None,
    };
    let _ = write!(o, "colr {id} ");
    match first_nclx(img).or_else(|| tile0.as_ref().and_then(first_nclx)) {
        Some((p, t, m, f)) => {
            let _ = write!(o, "nclx={p},{t},{m},{}", u8::from(f));
        }
        None => o.push('-'),
    }
    match first_icc(img)
        .or_else(|| tile0.as_ref().and_then(first_icc))
        .filter(|e| e.length > 0)
    {
        Some(e) => {
            let _ = writeln!(o, " icc={}", e.length);
            if let Some(b) = src.extent(e) {
                fs::write(dir.join(format!("icc-{id}.bin")), b).unwrap();
            }
        }
        None => o.push_str(" icc=-\n"),
    }

    for a in &img.aux {
        let _ = write!(o, "aux {id} {} ", a.id);
        // libheif cannot open an aux image it cannot set up: no type.
        if h.image(a.id).is_ok() {
            esc(o, a.aux_type.as_bytes());
        } else {
            o.push('-');
        }
        o.push('\n');
    }
    if let Some(d) = img.depth {
        let _ = writeln!(o, "depth {id} {d}");
    }
    for t in &img.thumbnails {
        let _ = writeln!(o, "thumb {id} {t}");
    }

    let mut metas: Vec<_> = h
        .items
        .iter()
        .filter(|m| {
            let mime_jpeg = m.item_type.0 == *b"mime" && m.content_type == "image/jpeg";
            !mime_jpeg
                && !NOT_METADATA.iter().any(|t| **t == m.item_type.0)
                && h.references
                    .iter()
                    .any(|r| r.kind.0 == *b"cdsc" && r.from == m.id && r.to.contains(&id))
        })
        .collect();
    metas.sort_by_key(|m| m.id);
    for m in metas {
        let _ = write!(o, "meta {id} {} ", m.id);
        fcc(o, m.item_type);
        o.push(' ');
        esc(o, m.content_type.as_bytes());
        let len = src.item_bytes(h, m.id, lim).map_or(0, |b| b.len());
        let _ = writeln!(o, " {len}");
    }

    if let (b"grid", ImageKind::Grid(g)) = (&ty, &img.kind) {
        let (tw, th) = g
            .tiles
            .first()
            .and_then(|t| h.image(*t).ok())
            .and_then(|t| t.display_size())
            .unwrap_or((0, 0));
        let _ = write!(
            o,
            "grid {id} {}x{} tile={tw}x{th} out={}x{} tiles=",
            g.columns, g.rows, g.width, g.height
        );
        ids(o, &g.tiles);
        o.push('\n');
    }
}

/// The error line on stderr (`name<TAB>what: message`) and the `error` output text.
fn fail(name: &str, what: &str, e: impl std::fmt::Display) -> String {
    eprintln!("{name}\t{what}: {e}");
    "error\n".into()
}

fn graph(name: &str, h: &Heif, src: &mut Src<'_>, lim: &Limits, dir: &Path) -> String {
    let mut o = String::from("ok\n");
    o.push_str("brand ");
    if h.ftyp.major.0 == [0; 4] {
        o.push_str("\"\"");
    } else {
        fcc(&mut o, h.ftyp.major);
    }
    o.push(' ');
    if h.ftyp.compatible.is_empty() {
        o.push('-');
    }
    for (i, b) in h.ftyp.compatible.iter().enumerate() {
        if i > 0 {
            o.push(',');
        }
        fcc(&mut o, *b);
    }
    o.push('\n');
    if h.primary == 0 {
        o.push_str("primary -\n");
    } else {
        let _ = writeln!(o, "primary {}", h.primary);
    }

    let mut items: Vec<_> = h.items.iter().collect();
    items.sort_by_key(|i| i.id);

    for it in &items {
        let _ = write!(o, "item {} ", it.id);
        fcc(&mut o, it.item_type);
        let _ = write!(o, " hidden={} name=", u8::from(it.hidden));
        esc(&mut o, it.name.as_bytes());
        let mime = it.item_type.0 == *b"mime";
        for (label, on, s) in [
            (" ctype=", mime, &it.content_type),
            (" cenc=", mime, &it.content_encoding),
            (" uri=", it.item_type.0 == *b"uri ", &it.uri_type),
        ] {
            o.push_str(label);
            if on {
                esc(&mut o, s.as_bytes());
            } else {
                o.push('-');
            }
        }
        o.push('\n');
    }

    for it in &items {
        match src.item_bytes(h, it.id, lim) {
            Some(b) => {
                let _ = writeln!(o, "data {} {}", it.id, b.len());
                fs::write(dir.join(format!("item-{}.bin", it.id)), b).unwrap();
            }
            None => {
                let _ = writeln!(o, "data {} error", it.id);
                let why = it.data.as_ref().err().map_or_else(
                    || "extent outside the file".to_string(),
                    ToString::to_string,
                );
                eprintln!("{name}\tdata {}: {why}", it.id);
            }
        }
    }

    for it in &items {
        for r in h.references.iter().filter(|r| r.from == it.id) {
            let _ = write!(o, "ref {} ", it.id);
            fcc(&mut o, r.kind);
            o.push(' ');
            ids(&mut o, &r.to);
            o.push('\n');
        }
    }

    for it in &items {
        if it
            .associations
            .iter()
            .any(|a| usize::from(a.index) > h.properties.len())
        {
            continue;
        }
        for (k, a) in it.associations.iter().enumerate() {
            let Some(p) = usize::from(a.index)
                .checked_sub(1)
                .and_then(|i| h.properties.get(i))
            else {
                continue;
            };
            let _ = write!(o, "prop {} {} ", it.id, k + 1);
            // libheif lists a property that failed to parse as type `ERR `.
            if matches!(p.value, PropValue::Invalid) {
                o.push_str("ERR%20");
            } else {
                fcc(&mut o, p.kind);
            }
            match &p.value {
                PropValue::Irot { angle } => {
                    let _ = write!(o, " rot={angle}");
                }
                PropValue::Imir { axis: a } => {
                    let _ = write!(o, " axis={}", axis(*a));
                }
                _ => {}
            }
            o.push('\n');
        }
    }

    // libheif only exposes `altr`, `ster` and `pymd` groups.
    for g in h
        .groups
        .iter()
        .filter(|g| [b"altr", b"ster", b"pymd"].contains(&&g.kind.0))
    {
        o.push_str("group ");
        fcc(&mut o, g.kind);
        let _ = write!(o, " {} ", g.id);
        ids(&mut o, &g.entities);
        o.push('\n');
    }

    for id in h.top_level_images() {
        match h.image(id) {
            Ok(img) => image_block(&mut o, h, src, lim, dir, &img),
            Err(e) => eprintln!("{name}\timage {id}: {e}"),
        }
    }
    o
}

fn graph_file(name: &str, path: &Path, dir: &Path, read: bool, lim: &Limits) -> String {
    let cancel = AtomicBool::new(false);
    if read {
        let (f, f2) = match (File::open(path), File::open(path)) {
            (Ok(f), Ok(f2)) => (f, f2),
            (Err(e), _) | (_, Err(e)) => return fail(name, "open", e),
        };
        let len = match f2.metadata() {
            Ok(m) => m.len(),
            Err(e) => return fail(name, "stat", e),
        };
        match isobmff::read(BufReader::new(f), lim, &cancel) {
            Ok(h) => graph(name, &h, &mut Src::File(f2, len), lim, dir),
            Err(e) => fail(name, "parse", e),
        }
    } else {
        let data = match fs::read(path) {
            Ok(d) => d,
            Err(e) => return fail(name, "open", e),
        };
        match isobmff::parse(&data, lim, &cancel) {
            Ok(h) => graph(name, &h, &mut Src::Mem(&data), lim, dir),
            Err(e) => fail(name, "parse", e),
        }
    }
}

fn run_heif(args: &[String]) {
    let o = opts(args);
    let [inp, out] = &o.pos[..] else { usage() };
    // libheif runs with its security limits off in the C oracle (libavif keeps its size limit).
    let lim = Limits {
        max_pixels: u64::MAX,
        ..limits(&o)
    };
    for (name, path) in files(inp) {
        let dir = Path::new(out).join(&name);
        fs::create_dir_all(&dir).unwrap();
        let text = graph_file(&name, &path, &dir, o.read, &lim);
        fs::write(dir.join("graph.txt"), text).unwrap();
    }
}

// ---- avif: avif.txt ----

fn be32(d: &[u8], o: usize) -> Option<u32> {
    Some(u32::from_be_bytes(
        d.get(o..o.checked_add(4)?)?.try_into().ok()?,
    ))
}

/// The `avif_oracle.c` rule: the first box is an `ftyp` with `avif` as major or compatible brand.
fn has_avif_brand(d: &[u8]) -> bool {
    fn brand(d: &[u8]) -> Option<bool> {
        let n = d.len() as u64;
        if n < 16 || d.get(4..8)? != b"ftyp" {
            return Some(false);
        }
        let mut size = u64::from(be32(d, 0)?);
        let mut off = 8u64;
        if size == 1 {
            if n < 24 {
                return Some(false);
            }
            size = (u64::from(be32(d, 8)?) << 32) | u64::from(be32(d, 12)?);
            off = 16;
        } else if size == 0 {
            size = n;
        }
        let end = size.min(n);
        if end < off + 8 {
            return Some(false);
        }
        let at = |p: u64| d.get(usize::try_from(p).ok()?..usize::try_from(p + 4).ok()?);
        if at(off)? == b"avif" {
            return Some(true);
        }
        let mut p = off + 8;
        while p + 4 <= end {
            if at(p)? == b"avif" {
                return Some(true);
            }
            p += 4;
        }
        Some(false)
    }
    brand(d).unwrap_or(false)
}

/// libavif `avifGetExifTiffHeaderOffset`: the first TIFF header in `b`.
fn tiff_offset(b: &[u8]) -> Option<usize> {
    (0..b.len().saturating_sub(4))
        .find(|&o| matches!(b.get(o..o + 4), Some(h) if h == b"MM\0*" || h == b"II*\0"))
}

/// The bounding range of the data of one item: `(offset, size)`, `(0, 0)` in `idat`.
fn item_range(h: &Heif, id: u32) -> Option<(u64, u64)> {
    let it = h.item(id)?;
    if it.construction_method == 1 {
        return Some((0, 0));
    }
    // The avif oracle keeps method 2 an item error (libavif was not checked for it).
    if it.construction_method == 2 {
        return None;
    }
    let ex = it.data.as_ref().ok()?;
    let lo = ex.iter().map(|e| e.offset).min()?;
    let hi = ex.iter().map(|e| e.offset.saturating_add(e.length)).max()?;
    Some((lo, hi - lo))
}

/// libavif `avifExtentMerge`.
fn merge(acc: &mut (u64, u64), r: (u64, u64)) {
    if acc.1 == 0 {
        *acc = r;
    } else if r.1 != 0 {
        let lo = acc.0.min(r.0);
        let hi = acc.0.saturating_add(acc.1).max(r.0.saturating_add(r.1));
        *acc = (lo, hi - lo);
    }
}

fn av1c_of(h: &Heif, img: &Image) -> Option<Av1c> {
    fn own(img: &Image) -> Option<Av1c> {
        match &img.kind {
            ImageKind::Coded {
                config: CodecConfig::Av1(a),
                ..
            } => Some(*a),
            _ => None,
        }
    }
    match &img.kind {
        ImageKind::Grid(g) => own(&h.image(*g.tiles.first()?).ok()?),
        _ => own(img),
    }
}

fn fr(f: Fraction) -> String {
    format!("{}/{}", f.numerator, f.denominator)
}

fn finish_gain(mut o: String, gain: Option<(u32, GainMapMetadata)>) -> String {
    let Some((_, m)) = gain else {
        o.push_str("gainmap 0\n");
        return o;
    };
    let ch = |sel: fn(&GainMapChannel) -> Fraction| {
        m.channels
            .iter()
            .map(|c| fr(sel(c)))
            .collect::<Vec<_>>()
            .join(",")
    };
    o.push_str("gainmap 1\n");
    let _ = writeln!(
        o,
        "gainmap-meta base={} alt={} min={} max={} gamma={} baseoff={} altoff={} base_cs={}",
        fr(m.base_headroom),
        fr(m.alternate_headroom),
        ch(|c| c.min),
        ch(|c| c.max),
        ch(|c| c.gamma),
        ch(|c| c.base_offset),
        ch(|c| c.alternate_offset),
        u8::from(m.use_base_colour_space)
    );
    o
}

fn avif_text(name: &str, data: &[u8], dir: &Path, lim: &Limits) -> String {
    let cancel = AtomicBool::new(false);
    let h = match isobmff::parse(data, lim, &cancel) {
        Ok(h) => h,
        Err(e) => return fail(name, "parse", e),
    };
    // libavif fails the parse when a pixi, pasp or lsel in ipco does not parse (read.c:2949-2964).
    if let Some(p) = h
        .properties
        .iter()
        .find(|p| matches!(&p.kind.0, b"pixi" | b"pasp" | b"lsel") && p.value == PropValue::Invalid)
    {
        return fail(name, "ipco", format!("malformed {}", p.kind));
    }
    let img = match h.primary_image() {
        Ok(i) => i,
        Err(e) => return fail(name, "primary image", e),
    };
    let Some(av1c) = av1c_of(&h, &img) else {
        return fail(name, "av1C", "no AV1 configuration");
    };
    let (w, hh) = match &img.kind {
        ImageKind::Grid(g) => (g.width, g.height),
        _ => (img.width, img.height),
    };
    let mut o = String::from("ok\n");
    let _ = writeln!(
        o,
        "size {w}x{hh} depth={} yuv={}",
        av1c.depth(),
        av1c.yuv_format()
    );
    // libavif: a grid has alpha when every tile has an alpha item (and reads their extents).
    let tile_alphas: Vec<u32> = match &img.kind {
        ImageKind::Grid(g) if img.alpha.is_none() && !g.tiles.is_empty() => g
            .tiles
            .iter()
            .map_while(|t| h.image(*t).ok().and_then(|i| i.alpha))
            .collect(),
        _ => Vec::new(),
    };
    let tiles_alpha = matches!(&img.kind, ImageKind::Grid(g) if tile_alphas.len() == g.tiles.len());
    let alpha = img.alpha.is_some() || (tiles_alpha && !tile_alphas.is_empty());
    let _ = writeln!(
        o,
        "alpha {} prem {}",
        u8::from(alpha),
        u8::from(alpha && img.premultiplied)
    );

    match first_icc(&img).filter(|e| e.length > 0) {
        Some(e) => {
            let Some(b) = Heif::slice(data, e) else {
                return fail(name, "icc", "profile outside the file");
            };
            let _ = writeln!(o, "icc {}", e.length);
            fs::write(dir.join("avif-icc.bin"), b).unwrap();
        }
        None => o.push_str("icc 0\n"),
    }

    // libavif walks the items in iinf order: the last Exif and the last XMP item win; an
    // exif_tiff_header_offset that is not where the TIFF header is gives `exif error` (the C
    // oracle parses again without Exif). libavif keeps everything after the 4-byte offset field.
    let mut exif: Vec<u8> = Vec::new();
    let mut exif_error = false;
    let mut xmp: Vec<u8> = Vec::new();
    for it in &h.items {
        let is_exif = img.exif.contains(&it.id);
        if !is_exif && !img.xmp.contains(&it.id) {
            continue;
        }
        match &it.data {
            Ok(v) if v.iter().all(|e| e.length == 0) => continue,
            Err(ItemError::NoLocation) => continue,
            _ => {}
        }
        if is_exif && exif_error {
            continue;
        }
        if it.construction_method == 2 {
            return fail(
                name,
                &format!("item {}", it.id),
                ItemError::Unsupported("construction method 2"),
            );
        }
        let b = match h.item_bytes(data, it.id, lim, &cancel) {
            Ok(b) => b,
            Err(e) => return fail(name, &format!("item {}", it.id), e),
        };
        if is_exif {
            let good = b.get(4..).filter(|rest| {
                be32(&b, 0).is_some_and(|off| {
                    tiff_offset(rest).is_some_and(|e| u64::try_from(e) == Ok(u64::from(off)))
                })
            });
            match good {
                Some(rest) => exif = rest.to_vec(),
                None => exif_error = true,
            }
        } else {
            xmp = b.into_owned();
        }
    }
    if exif_error {
        o.push_str("exif error\n");
    } else {
        let _ = writeln!(o, "exif {}", exif.len());
        if !exif.is_empty() {
            fs::write(dir.join("avif-exif.bin"), &exif).unwrap();
        }
    }
    let _ = writeln!(o, "xmp {}", xmp.len());
    if !xmp.is_empty() {
        fs::write(dir.join("avif-xmp.bin"), &xmp).unwrap();
    }

    match img.transforms.iter().find_map(|t| match t {
        Transform::Rotate(d) => Some(*d),
        _ => None,
    }) {
        Some(d) => {
            let _ = writeln!(o, "irot {d}");
        }
        None => o.push_str("irot -\n"),
    }
    match img.transforms.iter().find_map(|t| match t {
        Transform::Mirror(a) => Some(*a),
        _ => None,
    }) {
        Some(a) => {
            let _ = writeln!(o, "imir {}", axis(a));
        }
        None => o.push_str("imir -\n"),
    }
    match img.transforms.iter().find_map(|t| match t {
        Transform::Clap(c) => Some(*c),
        _ => None,
    }) {
        Some(c) => {
            let _ = writeln!(
                o,
                "clap {}/{},{}/{},{}/{},{}/{}",
                c.width_n,
                c.width_d,
                c.height_n,
                c.height_d,
                c.horiz_off_n,
                c.horiz_off_d,
                c.vert_off_n,
                c.vert_off_d
            );
        }
        None => o.push_str("clap -\n"),
    }
    match img.pasp {
        Some((hs, vs)) => {
            let _ = writeln!(o, "pasp {hs}/{vs}");
        }
        None => o.push_str("pasp -\n"),
    }

    let gain = if h.ftyp.has_brand(b"tmap") {
        img.gain_map
            .as_ref()
            .filter(|g| g.source == GainMapSource::Tmap)
            .and_then(|g| g.metadata.map(|m| (g.image, m)))
    } else {
        None
    };

    // The merged range over the colour, alpha and gain map items (every tile of a grid).
    let mut want = vec![img.id];
    want.extend(img.alpha);
    if tiles_alpha {
        want.extend(&tile_alphas);
    }
    want.extend(gain.map(|g| g.0));
    let mut acc = (0u64, 0u64);
    for id in want {
        let leaves = match h.image(id) {
            Ok(Image {
                kind: ImageKind::Grid(g),
                ..
            }) => g.tiles,
            _ => vec![id],
        };
        for leaf in leaves {
            match item_range(&h, leaf) {
                Some(r) => merge(&mut acc, r),
                None => {
                    o.push_str("extent error\n");
                    return finish_gain(o, gain);
                }
            }
        }
    }
    let _ = writeln!(o, "extent {} {}", acc.0, acc.1);
    finish_gain(o, gain)
}

fn run_avif(args: &[String]) {
    let o = opts(args);
    let [inp, out] = &o.pos[..] else { usage() };
    let lim = limits(&o);
    for (name, path) in files(inp) {
        let Ok(data) = fs::read(&path) else { continue };
        if !has_avif_brand(&data) {
            continue;
        }
        let dir = Path::new(out).join(&name);
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join("avif.txt"), avif_text(&name, &data, &dir, &lim)).unwrap();
    }
}

// ---- gen: deterministic mutations ----

const GEN_TOTAL: usize = 4000;
const GEN_PER_FILE: usize = 16;

fn xorshift(s: &mut u64) -> u64 {
    let mut x = *s;
    x ^= x << 13;
    x ^= x >> 7;
    x ^= x << 17;
    *s = x;
    x
}

fn seed(name: &str) -> u64 {
    let mut h = 0xcbf2_9ce4_8422_2325u64;
    for b in name.bytes() {
        h ^= u64::from(b);
        h = h.wrapping_mul(0x0100_0000_01b3);
    }
    h | 1
}

/// The boxes of `d[off..end]` as `(offset, size, type)`, the last one clamped to `end`.
fn boxes_in(d: &[u8], mut off: usize, end: usize) -> Vec<(usize, usize, [u8; 4])> {
    let mut v = Vec::new();
    while off.saturating_add(8) <= end {
        let (Some(s32), Some(kind)) = (
            be32(d, off),
            d.get(off + 4..off + 8)
                .and_then(|k| <[u8; 4]>::try_from(k).ok()),
        ) else {
            break;
        };
        let size = match s32 {
            0 => end - off,
            1 => {
                let (Some(hi), Some(lo)) = (be32(d, off + 8), be32(d, off + 12)) else {
                    break;
                };
                usize::try_from((u64::from(hi) << 32) | u64::from(lo)).unwrap_or(usize::MAX)
            }
            n => usize::try_from(n).unwrap_or(usize::MAX),
        };
        if size < 8 {
            break;
        }
        let size = size.min(end - off);
        v.push((off, size, kind));
        off += size;
    }
    v
}

enum Mutation {
    Truncate(usize),
    Flip(usize, u8),
    /// Box at offset, its size, edit: 0 = size 0, 1 = size 1, 2 = half, 3 = double.
    Size(usize, usize, u8),
}

fn candidates(d: &[u8], rng: &mut u64) -> Vec<Mutation> {
    let len = d.len();
    let top = boxes_in(d, 0, len);
    let mut all = top.clone();
    for &(off, size, kind) in &top {
        if kind == *b"meta" {
            all.extend(boxes_in(d, off + 12, off + size));
        }
    }
    let mut c = Vec::new();
    // truncations at box boundaries (up to 5, evenly) and at fixed fractions
    let mut bounds: Vec<usize> = all.iter().map(|b| b.0).filter(|&o| o > 0).collect();
    bounds.sort_unstable();
    bounds.dedup();
    let n = bounds.len().min(5);
    for i in 0..n {
        if let Some(&b) = bounds.get(i * bounds.len() / n) {
            c.push(Mutation::Truncate(b));
        }
    }
    for cut in [len / 4, len / 2, len / 4 * 3, len - 1] {
        if cut > 0 && cut < len {
            c.push(Mutation::Truncate(cut));
        }
    }
    // bit flips: half in the first 2 KiB (where `meta` usually is), half anywhere
    for i in 0..6 {
        let span = if i % 2 == 0 { len.min(2048) } else { len };
        let pos = usize::try_from(xorshift(rng) % span as u64).unwrap_or(0);
        c.push(Mutation::Flip(pos, (xorshift(rng) % 8) as u8));
    }
    // box-size edits of the first 12 boxes
    for &(off, size, _) in all.iter().take(12) {
        for e in 0..4 {
            c.push(Mutation::Size(off, size, e));
        }
    }
    c
}

fn apply(d: &[u8], m: &Mutation) -> (String, Vec<u8>) {
    match *m {
        Mutation::Truncate(n) => (format!("t{n}"), d.get(..n).unwrap_or(d).to_vec()),
        Mutation::Flip(p, b) => {
            let mut v = d.to_vec();
            if let Some(x) = v.get_mut(p) {
                *x ^= 1 << b;
            }
            (format!("f{p}b{b}"), v)
        }
        Mutation::Size(off, size, e) => {
            let (tag, val) = match e {
                0 => ("z", 0),
                1 => ("o", 1),
                2 => ("h", u32::try_from(size / 2).unwrap_or(0)),
                _ => (
                    "d",
                    u32::try_from(size.saturating_mul(2)).unwrap_or(u32::MAX),
                ),
            };
            let mut v = d.to_vec();
            if let Some(f) = v.get_mut(off..off + 4) {
                f.copy_from_slice(&val.to_be_bytes());
            }
            (format!("s{off}{tag}"), v)
        }
    }
}

fn run_gen(args: &[String]) {
    let [inp, out] = args else { usage() };
    fs::create_dir_all(out).unwrap();
    let small: Vec<_> = files(inp)
        .into_iter()
        .filter(|(_, p)| fs::metadata(p).is_ok_and(|m| (8..1 << 20).contains(&m.len())))
        .collect();
    let per = (GEN_TOTAL / small.len().max(1)).clamp(1, GEN_PER_FILE);
    let mut total = 0;
    for (name, path) in small {
        let Ok(d) = fs::read(&path) else { continue };
        let mut rng = seed(&name);
        let c = candidates(&d, &mut rng);
        let take = per.min(c.len());
        for i in 0..take {
            if total >= GEN_TOTAL {
                return;
            }
            let Some(m) = c.get(i * c.len() / take) else {
                continue;
            };
            let (tag, bytes) = apply(&d, m);
            fs::write(Path::new(out).join(format!("{name}.{tag}")), bytes).unwrap();
            total += 1;
        }
    }
}

// ---- stats ----

fn run_stats(args: &[String]) {
    let [inp, out] = args else { usage() };
    let lim = Limits::default();
    let cancel = AtomicBool::new(false);
    let mut o = String::new();
    for (name, path) in files(inp) {
        let Ok(data) = fs::read(&path) else { continue };
        let Ok(h) = isobmff::parse(&data, &lim, &cancel) else {
            continue;
        };
        let refs = h.references.iter().map(|r| r.to.len()).max().unwrap_or(0);
        let grid = h
            .items
            .iter()
            .filter_map(|i| match &i.derived {
                Some(Ok(Derived::Grid(g))) => Some(u64::from(g.rows) * u64::from(g.columns)),
                _ => None,
            })
            .max()
            .unwrap_or(0);
        let ext = h
            .items
            .iter()
            .filter_map(|i| i.data.as_ref().ok().map(Vec::len))
            .max()
            .unwrap_or(0);
        let _ = writeln!(
            o,
            "{name}\t{}\t{}\t{}\t{refs}\t{grid}\t{ext}\t{}",
            h.stats.boxes,
            h.stats.max_depth,
            h.items.len(),
            h.properties.len()
        );
    }
    fs::write(out, o).unwrap();
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match args.split_first() {
        Some((c, rest)) if c == "heif" => run_heif(rest),
        Some((c, rest)) if c == "avif" => run_avif(rest),
        Some((c, rest)) if c == "gen" => run_gen(rest),
        Some((c, rest)) if c == "stats" => run_stats(rest),
        _ => usage(),
    }
}
