//! MP4/MOV tracks: `moov` with its sample tables, kept as ranges of the file and walked on
//! demand ([`Track::samples`], [`Track::sync_samples`], [`Track::sync_sample_at`]). Non-fragmented
//! files only: `mvex` sets [`Movie::fragmented`] and `moof` boxes stay in [`Movie::other_boxes`].

mod stbl;
mod walk;

use std::io::{Read, Seek, SeekFrom};
use std::sync::atomic::AtomicBool;

use crate::boxes::{BoxHeader, Boxes, Ctx, Cur, Src, decode_header, full_header};
use crate::budget::Budget;
use crate::props::{Property, parse_property};
use crate::{Error, Extent, FourCc, Ftyp, Limit, Limits, Stats, ftyp, io_err, read_at, read_vec};

use stbl::{Chunks, Sizes, Stbl, Table};

pub use walk::Samples;

/// The first top-level box of a movie file (QuickTime files need no `ftyp`).
const FIRST: [&[u8; 4]; 7] = [
    b"ftyp", b"moov", b"mdat", b"free", b"skip", b"wide", b"pnot",
];

/// The tracks of an MP4, MOV, 3GP or AVIS file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Movie {
    /// The first `ftyp`; `None` for a QuickTime file without one.
    pub ftyp: Option<Ftyp>,
    /// `mvhd` timescale (ticks per second); 0 without `mvhd`.
    pub timescale: u32,
    /// `mvhd` duration in `timescale` ticks (version 0 widened).
    pub duration: u64,
    /// One entry per `trak`, in file order.
    pub tracks: Vec<Track>,
    /// `moov` has an `mvex`: the file has movie fragments. This version does not read them.
    pub fragmented: bool,
    /// Every top-level box and `moov` child that is not parsed (`mdat`, `free`, `udta`, `meta`,
    /// `mvex`, `moof`, `uuid`, ...), type and whole-box range, in file order. The first `ftyp`
    /// and the `moov` box itself are not listed.
    pub other_boxes: Vec<(FourCc, Extent)>,
    /// Boxes visited and the deepest nesting.
    pub stats: Stats,
}

impl Movie {
    /// The track with this `tkhd` track id.
    pub fn track(&self, id: u32) -> Option<&Track> {
        self.tracks.iter().find(|t| t.id == id)
    }

    /// The first track whose handler is `vide` or `pict`.
    pub fn video_track(&self) -> Option<&Track> {
        self.tracks
            .iter()
            .find(|t| matches!(&t.handler.0, b"vide" | b"pict"))
    }
}

/// One `trak`. The sample tables are not decoded: they stay in the file and the walks of
/// [`samples`](Self::samples), [`sync_samples`](Self::sync_samples) and
/// [`sync_sample_at`](Self::sync_sample_at) read them, so a track costs the same memory whatever
/// its sample count.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Track {
    /// `tkhd` track id (not 0, unique in the file).
    pub id: u32,
    /// `tkhd` flags.
    pub flags: u32,
    /// The `tkhd` payload.
    pub tkhd: Extent,
    /// `tkhd` matrix (nine 16.16 or 2.30 fixed-point values, row major): the rotation of a
    /// phone video.
    pub matrix: [i32; 9],
    /// `tkhd` width, raw 16.16 fixed point (the pixel width is `width >> 16`).
    pub width: u32,
    /// `tkhd` height, raw 16.16 fixed point.
    pub height: u32,
    /// `mdia/hdlr` handler type (`vide`, `pict`, `soun`, ...).
    pub handler: FourCc,
    /// `mdhd` timescale. 0 is kept: check it before dividing.
    pub timescale: u32,
    /// `mdhd` duration in `timescale` ticks.
    pub duration: u64,
    /// The `stsd` entries; the sample description index `i` is `entries[i - 1]`.
    pub entries: Vec<SampleEntry>,
    /// The `elst` entries (none without `elst`). The walks do not apply them.
    pub edits: Vec<Edit>,
    /// The `stsz` or `stz2` sample count.
    pub sample_count: u64,
    /// The parsed `stbl` tables (`stts ctts stss stsc stsz stz2 stco co64`), type and whole-box
    /// range, in file order.
    pub tables: Vec<(FourCc, Extent)>,
    /// Every other box under `trak` that is not parsed (`udta`, `tref`, `dinf`, `vmhd`, `sdtp`,
    /// `sgpd`, ...), type and whole-box range, in file order.
    pub other_boxes: Vec<(FourCc, Extent)>,
    st: Stbl,
}

/// One `stsd` entry.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SampleEntry {
    /// The entry type (`hvc1`, `avc1`, `av01`, `mp4a`, `encv`, ...).
    pub kind: FourCc,
    /// The whole box.
    pub range: Extent,
    /// Index into the `dref` of the track (1-based).
    pub data_reference_index: u16,
    /// Width and height of a visual entry (handler `vide`, `pict` or `auxv`).
    pub size: Option<(u16, u16)>,
    /// The children of a visual entry after its 78 fixed bytes (`hvcC`, `colr`, `pasp`, ...),
    /// parsed like the properties of `ipco`. Empty for other entries.
    pub boxes: Vec<Property>,
}

impl SampleEntry {
    /// The first child of type `hvcC`, `avcC`, `av1C` or `vvcC`. `hvcC` and `av1C` are parsed;
    /// `avcC` and `vvcC` are [`PropValue::Unknown`](crate::PropValue::Unknown) with the payload
    /// range.
    pub fn config(&self) -> Option<&Property> {
        self.boxes
            .iter()
            .find(|p| matches!(&p.kind.0, b"hvcC" | b"avcC" | b"av1C" | b"vvcC"))
    }
}

/// One `elst` entry.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Edit {
    /// Duration in the movie timescale.
    pub segment_duration: u64,
    /// Start in the media timescale; -1 is an empty edit.
    pub media_time: i64,
    /// `media_rate_integer`.
    pub rate_integer: i16,
    /// `media_rate_fraction`.
    pub rate_fraction: i16,
}

/// One sample of a track.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Sample {
    /// 1-based decode order.
    pub number: u64,
    /// The sample bytes, as an absolute range of the file.
    pub range: Extent,
    /// Decode time in the media timescale, from the first sample (0) and no edit list.
    pub dts: i64,
    /// Composition offset (`ctts`, read as signed for both versions), 0 when there is none.
    pub cto: i32,
    /// A sync sample: listed in `stss`, or any sample when there is no `stss`.
    pub sync: bool,
    /// Sample description index (1-based) into [`Track::entries`].
    pub entry: u32,
    /// Resolved fragment sample flags. Always `None` for the samples of `moov`.
    pub flags: Option<u32>,
}

impl Sample {
    /// Composition time: `dts + cto`.
    pub fn pts(&self) -> i64 {
        self.dts.saturating_add(i64::from(self.cto))
    }
}

/// What a top-level box is for the walk.
enum Kind {
    Ftyp,
    Moov,
    Other,
}

#[derive(Default)]
struct Top {
    ftyp: Option<Ftyp>,
    ftyp_seen: bool,
    moov: Option<Moov>,
    moov_seen: bool,
    other: Vec<(FourCc, Extent)>,
}

impl Top {
    /// Sorts `h`: the first `ftyp` and the first `moov` are for the caller to parse; anything
    /// else is kept as a range. A second `moov` is `Malformed`.
    fn note(&mut self, budget: &Budget<'_>, h: &BoxHeader) -> Result<Kind, Error> {
        match &h.kind.0 {
            b"ftyp" if !self.ftyp_seen => {
                self.ftyp_seen = true;
                Ok(Kind::Ftyp)
            }
            b"moov" => {
                if self.moov_seen {
                    return Err(Error::Malformed("moov"));
                }
                self.moov_seen = true;
                Ok(Kind::Moov)
            }
            _ => {
                keep(budget, &mut self.other, h)?;
                Ok(Kind::Other)
            }
        }
    }

    fn finish(self, stats: Stats) -> Result<Movie, Error> {
        let moov = self.moov.ok_or(Error::NoMoov)?;
        Ok(Movie {
            ftyp: self.ftyp,
            timescale: moov.timescale,
            duration: moov.duration,
            tracks: moov.tracks,
            fragmented: moov.fragmented,
            other_boxes: self.other,
            stats,
        })
    }
}

fn first_box_ok(kind: Option<&[u8]>) -> bool {
    kind.is_some_and(|k| FIRST.iter().any(|f| f.as_slice() == k))
}

/// Parses the tracks of an MP4, MOV, 3GP or AVIS file held in memory: `moov` with every `trak`,
/// its sample entries, edit list and sample tables. The tables stay in the file; the walks on
/// [`Track`] read them. `parse` and `read` (the HEIF item graph) are unchanged: call both for an
/// AVIS file when both views are wanted.
///
/// Limits enforced: `max_alloc`, `max_boxes`, `max_depth`, `max_properties` (children of sample
/// entries, all together); the cancel flag every 1024 boxes.
///
/// # Errors
///
/// [`Error::UnknownFormat`] when the first box is not one of `ftyp moov mdat free skip wide
/// pnot`; [`Error::NoMoov`] without a `moov`; [`Error::Truncated`] when a top-level box before
/// `moov` runs past the end of `data` (after `moov`, a damaged tail ends the walk);
/// [`Error::Unsupported`] for `cmov`; [`Error::Malformed`] for an invalid box (two `moov`, a
/// missing or duplicate required box, a table whose count does not fit its box, ...);
/// [`Error::LimitExceeded`]; [`Error::OutOfMemory`]; [`Error::Cancelled`].
pub fn parse_movie(data: &[u8], limits: &Limits, cancel: &AtomicBool) -> Result<Movie, Error> {
    let budget = Budget::new(limits, cancel);
    budget.check_cancel()?;
    if !first_box_ok(data.get(4..8)) {
        return Err(Error::UnknownFormat);
    }
    let src = Src {
        bytes: data,
        base: 0,
        file_len: data.len() as u64,
    };
    let ctx = Ctx::new(&src, &budget, limits);
    let mut boxes = Boxes::new(&ctx, 0..src.file_len, 0, true)?;
    let mut top = Top::default();
    loop {
        let h = match boxes.next_box() {
            Ok(Some(h)) => h,
            Ok(None) => break,
            // After `moov`, a damaged tail ends the walk (as for `meta`).
            Err(Error::Truncated) if top.moov.is_some() => break,
            Err(e) => return Err(e),
        };
        match top.note(&budget, &h)? {
            Kind::Ftyp => top.ftyp = Some(ftyp::parse(ctx.slice(h.payload())?, &budget)?),
            Kind::Moov => top.moov = Some(parse_moov(&ctx, &h, &mut top.other)?),
            Kind::Other => {}
        }
    }
    let stats = Stats {
        boxes: ctx.boxes.get(),
        max_depth: ctx.max_depth.get(),
    };
    top.finish(stats)
}

/// Parses the tracks of an MP4, MOV, 3GP or AVIS file from a reader. It reads the top-level box
/// headers, `ftyp` and the whole `moov` box (at most `limits.max_moov_bytes`, charged to
/// `max_alloc` while it is parsed) and seeks over `mdat`. The result equals [`parse_movie`] on
/// the same bytes. The walks on [`Track`] need the reader again.
///
/// Limits enforced: those of [`parse_movie`] and `max_moov_bytes`.
///
/// # Errors
///
/// Those of [`parse_movie`], and [`Error::LimitExceeded`] with `Limit::MoovBytes` for a larger
/// `moov` (or `ftyp`); [`Error::Truncated`] when the reader ends early; [`Error::Io`] for another
/// I/O error.
pub fn read_movie<R: Read + Seek>(
    mut r: R,
    limits: &Limits,
    cancel: &AtomicBool,
) -> Result<Movie, Error> {
    let budget = Budget::new(limits, cancel);
    budget.check_cancel()?;
    let file_len = r.seek(SeekFrom::End(0)).map_err(io_err)?;
    if file_len < 8 {
        return Err(Error::UnknownFormat);
    }
    let none = Src {
        bytes: &[],
        base: 0,
        file_len,
    };
    let walk = Ctx::new(&none, &budget, limits);
    let mut top = Top::default();
    let mut pos: u64 = 0;
    let mut head = [0u8; 32];
    while pos < file_len {
        walk.tick()?;
        let avail = usize::try_from(file_len.saturating_sub(pos).min(32)).unwrap_or(32);
        let buf = head.get_mut(..avail).ok_or(Error::Truncated)?;
        read_at(&mut r, pos, buf)?;
        if pos == 0 && !first_box_ok(buf.get(4..8)) {
            return Err(Error::UnknownFormat);
        }
        let h = match decode_header(buf, pos, file_len, true) {
            Ok(h) => h,
            Err(Error::Truncated) if top.moov.is_some() => break,
            Err(e) => return Err(e),
        };
        match top.note(&budget, &h)? {
            Kind::Ftyp => {
                let p = h.payload();
                let len = copy_len(p.end.saturating_sub(p.start), limits)?;
                let bytes = read_vec(&mut r, &budget, p.start, len)?;
                top.ftyp = Some(ftyp::parse(&bytes, &budget)?);
                budget.release(len);
            }
            Kind::Moov => {
                let len = copy_len(h.end.saturating_sub(h.start), limits)?;
                let bytes = read_vec(&mut r, &budget, h.start, len)?;
                let src = Src {
                    bytes: &bytes,
                    base: h.start,
                    file_len,
                };
                let ctx = Ctx::new(&src, &budget, limits);
                ctx.boxes.set(walk.boxes.get());
                ctx.max_depth.set(walk.max_depth.get());
                top.moov = Some(parse_moov(&ctx, &h, &mut top.other)?);
                walk.boxes.set(ctx.boxes.get());
                walk.max_depth.set(ctx.max_depth.get());
                budget.release(len);
            }
            Kind::Other => {}
        }
        pos = h.end;
    }
    let stats = Stats {
        boxes: walk.boxes.get(),
        max_depth: walk.max_depth.get(),
    };
    top.finish(stats)
}

/// The size of a box that `read_movie` copies, within `max_moov_bytes`.
fn copy_len(len: u64, limits: &Limits) -> Result<usize, Error> {
    usize::try_from(len)
        .ok()
        .filter(|&l| l <= limits.max_moov_bytes)
        .ok_or(Error::LimitExceeded(Limit::MoovBytes))
}

/// Appends to a charged `Vec`, growing by doubling; the whole new capacity is charged first.
fn push<T>(budget: &Budget<'_>, v: &mut Vec<T>, x: T) -> Result<(), Error> {
    if v.len() == v.capacity() {
        let add = v.capacity().max(4);
        let bytes = add
            .checked_mul(size_of::<T>())
            .ok_or(Error::LimitExceeded(Limit::Alloc))?;
        budget.charge(bytes)?;
        v.try_reserve_exact(add).map_err(|_| Error::OutOfMemory)?;
    }
    v.push(x);
    Ok(())
}

/// Keeps `h` as a (type, whole-box range) entry.
fn keep(budget: &Budget<'_>, v: &mut Vec<(FourCc, Extent)>, h: &BoxHeader) -> Result<(), Error> {
    push(budget, v, (h.kind, whole(h)))
}

fn whole(h: &BoxHeader) -> Extent {
    Extent {
        offset: h.start,
        length: h.end.saturating_sub(h.start),
    }
}

fn once<T>(slot: &mut Option<T>, v: T, what: &'static str) -> Result<(), Error> {
    if slot.is_some() {
        return Err(Error::Malformed(what));
    }
    *slot = Some(v);
    Ok(())
}

struct Moov {
    timescale: u32,
    duration: u64,
    tracks: Vec<Track>,
    fragmented: bool,
}

/// `moov` (depth 0). Children that are not parsed are appended to `other`.
fn parse_moov(
    ctx: &Ctx<'_>,
    h: &BoxHeader,
    other: &mut Vec<(FourCc, Extent)>,
) -> Result<Moov, Error> {
    let mut m = Moov {
        timescale: 0,
        duration: 0,
        tracks: Vec::new(),
        fragmented: false,
    };
    let mut mvhd = None;
    let mut props: usize = 0;
    let mut boxes = Boxes::new(ctx, h.payload(), 1, false)?;
    while let Some(c) = boxes.next_box()? {
        match &c.kind.0 {
            b"mvhd" => once(
                &mut mvhd,
                time_header("mvhd", ctx.slice(c.payload())?)?,
                "mvhd",
            )?,
            b"trak" => {
                let t = parse_trak(ctx, &c, &mut props)?;
                push(ctx.budget, &mut m.tracks, t)?;
            }
            b"cmov" => return Err(Error::Unsupported("cmov")),
            b"mvex" => {
                m.fragmented = true;
                keep(ctx.budget, other, &c)?;
            }
            _ => keep(ctx.budget, other, &c)?,
        }
    }
    (m.timescale, m.duration) = mvhd.unwrap_or((0, 0));
    unique_ids(ctx.budget, &m.tracks)?;
    Ok(m)
}

/// A duplicate track id is `Malformed("tkhd")`.
fn unique_ids(budget: &Budget<'_>, tracks: &[Track]) -> Result<(), Error> {
    let mut ids = budget.vec::<u32>(tracks.len())?;
    ids.extend(tracks.iter().map(|t| t.id));
    ids.sort_unstable();
    let dup = ids.windows(2).any(|w| w.first() == w.get(1));
    budget.release(tracks.len().saturating_mul(size_of::<u32>()));
    if dup {
        return Err(Error::Malformed("tkhd"));
    }
    Ok(())
}

/// `mvhd` and `mdhd` (timescale, duration), version 0 or 1.
fn time_header(what: &'static str, bytes: &[u8]) -> Result<(u32, u64), Error> {
    let (v, _, rest) = full_header(what, bytes)?;
    let mut c = Cur::new(what, rest, 0);
    match v {
        0 => {
            c.skip(8)?;
            let ts = c.u32()?;
            Ok((ts, u64::from(c.u32()?)))
        }
        1 => {
            c.skip(16)?;
            let ts = c.u32()?;
            Ok((ts, c.u64()?))
        }
        _ => Err(Error::Malformed(what)),
    }
}

/// The handler type of an `hdlr` payload.
fn handler(bytes: &[u8]) -> Result<FourCc, Error> {
    let (_, _, rest) = full_header("hdlr", bytes)?;
    let mut c = Cur::new("hdlr", rest, 0);
    c.skip(4)?; // pre_defined
    c.fourcc()
}

struct Tkhd {
    id: u32,
    flags: u32,
    matrix: [i32; 9],
    width: u32,
    height: u32,
    range: Extent,
}

fn parse_tkhd(ctx: &Ctx<'_>, h: &BoxHeader) -> Result<Tkhd, Error> {
    let p = h.payload();
    let (v, flags, rest) = full_header("tkhd", ctx.slice(p.clone())?)?;
    let mut c = Cur::new("tkhd", rest, 0);
    match v {
        0 => c.skip(8)?,
        1 => c.skip(16)?,
        _ => return Err(Error::Malformed("tkhd")),
    }
    let id = c.u32()?;
    c.skip(4)?; // reserved
    c.skip(if v == 1 { 8 } else { 4 })?; // duration
    c.skip(16)?; // reserved, layer, alternate_group, volume, reserved
    let mut matrix = [0i32; 9];
    for m in &mut matrix {
        *m = c.i32()?;
    }
    let width = c.u32()?;
    let height = c.u32()?;
    if id == 0 {
        return Err(Error::Malformed("tkhd"));
    }
    Ok(Tkhd {
        id,
        flags,
        matrix,
        width,
        height,
        range: Extent {
            offset: p.start,
            length: p.end.saturating_sub(p.start),
        },
    })
}

fn parse_elst(ctx: &Ctx<'_>, h: &BoxHeader) -> Result<Vec<Edit>, Error> {
    let (v, _, rest) = full_header("elst", ctx.slice(h.payload())?)?;
    let mut c = Cur::new("elst", rest, 0);
    let n = usize::try_from(c.u32()?).map_err(|_| Error::Malformed("elst"))?;
    let each = match v {
        0 => 12,
        1 => 20,
        _ => return Err(Error::Malformed("elst")),
    };
    if !c.fits(n, each) {
        return Err(Error::Malformed("elst"));
    }
    let mut out = ctx.budget.vec::<Edit>(n)?;
    for _ in 0..n {
        let (segment_duration, media_time) = if v == 1 {
            (c.u64()?, c.u64()? as i64)
        } else {
            (u64::from(c.u32()?), i64::from(c.i32()?))
        };
        out.push(Edit {
            segment_duration,
            media_time,
            rate_integer: c.u16()? as i16,
            rate_fraction: c.u16()? as i16,
        });
    }
    Ok(out)
}

/// `edts` (depth 2 box): its `elst`; other children are kept as ranges.
fn parse_edts(
    ctx: &Ctx<'_>,
    h: &BoxHeader,
    edits: &mut Option<Vec<Edit>>,
    other: &mut Vec<(FourCc, Extent)>,
) -> Result<(), Error> {
    let mut boxes = Boxes::new(ctx, h.payload(), 3, false)?;
    while let Some(c) = boxes.next_box()? {
        match &c.kind.0 {
            b"elst" => once(edits, parse_elst(ctx, &c)?, "elst")?,
            _ => keep(ctx.budget, other, &c)?,
        }
    }
    Ok(())
}

/// `dinf` (depth 4 box): per `dref` entry, true when its data is in this file.
fn parse_dinf(ctx: &Ctx<'_>, h: &BoxHeader) -> Result<Vec<bool>, Error> {
    let mut drefs = None;
    let mut boxes = Boxes::new(ctx, h.payload(), 5, false)?;
    while let Some(c) = boxes.next_box()? {
        if &c.kind.0 == b"dref" {
            once(&mut drefs, parse_dref(ctx, &c)?, "dref")?;
        }
    }
    Ok(drefs.unwrap_or_default())
}

fn parse_dref(ctx: &Ctx<'_>, h: &BoxHeader) -> Result<Vec<bool>, Error> {
    let p = h.payload();
    let bytes = ctx.slice(p.clone())?;
    full_header("dref", bytes)?;
    if bytes.len() < 8 {
        return Err(Error::Malformed("dref"));
    }
    let mut out = Vec::new();
    let mut boxes = Boxes::new(ctx, p.start.saturating_add(8)..p.end, 6, false)?;
    while let Some(e) = boxes.next_box()? {
        let (_, flags, _) = full_header("dref", ctx.slice(e.payload())?)?;
        push(ctx.budget, &mut out, flags & 1 != 0)?;
    }
    Ok(out)
}

#[derive(Default)]
struct StblParts {
    stsd: Option<BoxHeader>,
    stts: Option<Table>,
    ctts: Option<Table>,
    stss: Option<Table>,
    stsc: Option<Table>,
    sizes: Option<Sizes>,
    chunks: Option<Chunks>,
}

/// `stbl` (depth 4 box): the tables, whole-box ranges in `tables`; the other children in
/// `other`. Returns the tables and the `stsd` header (read once the handler is known).
fn parse_stbl(
    ctx: &Ctx<'_>,
    h: &BoxHeader,
    tables: &mut Vec<(FourCc, Extent)>,
    other: &mut Vec<(FourCc, Extent)>,
) -> Result<(Stbl, BoxHeader), Error> {
    let mut s = StblParts::default();
    let mut boxes = Boxes::new(ctx, h.payload(), 5, false)?;
    while let Some(c) = boxes.next_box()? {
        let p = c.payload();
        let st = p.start;
        match &c.kind.0 {
            b"stsd" => {
                once(&mut s.stsd, c, "stbl")?;
                continue;
            }
            b"stts" => once(
                &mut s.stts,
                stbl::table("stts", ctx.slice(p)?, st, 8, &[0])?,
                "stbl",
            )?,
            b"ctts" => once(
                &mut s.ctts,
                stbl::table("ctts", ctx.slice(p)?, st, 8, &[0, 1])?,
                "stbl",
            )?,
            b"stss" => once(
                &mut s.stss,
                stbl::table("stss", ctx.slice(p)?, st, 4, &[0])?,
                "stbl",
            )?,
            b"stsc" => once(
                &mut s.stsc,
                stbl::table("stsc", ctx.slice(p)?, st, 12, &[0])?,
                "stbl",
            )?,
            b"stsz" => once(&mut s.sizes, stbl::stsz(ctx.slice(p)?, st)?, "stbl")?,
            b"stz2" => once(&mut s.sizes, stbl::stz2(ctx.slice(p)?, st)?, "stbl")?,
            b"stco" => once(
                &mut s.chunks,
                Chunks {
                    wide: false,
                    table: stbl::table("stco", ctx.slice(p)?, st, 4, &[0])?,
                },
                "stbl",
            )?,
            b"co64" => once(
                &mut s.chunks,
                Chunks {
                    wide: true,
                    table: stbl::table("co64", ctx.slice(p)?, st, 8, &[0])?,
                },
                "stbl",
            )?,
            _ => {
                keep(ctx.budget, other, &c)?;
                continue;
            }
        }
        keep(ctx.budget, tables, &c)?;
    }
    let stsd = s.stsd.ok_or(Error::Malformed("stsd"))?;
    let st = Stbl {
        stts: s.stts.ok_or(Error::Malformed("stts"))?,
        ctts: s.ctts,
        stss: s.stss,
        stsc: s.stsc.ok_or(Error::Malformed("stsc"))?,
        sizes: s.sizes.ok_or(Error::Malformed("stsz"))?,
        chunks: s.chunks.ok_or(Error::Malformed("stco"))?,
        drefs: Vec::new(),
    };
    Ok((st, stsd))
}

/// `minf` (depth 3 box).
fn parse_minf(
    ctx: &Ctx<'_>,
    h: &BoxHeader,
    tables: &mut Vec<(FourCc, Extent)>,
    other: &mut Vec<(FourCc, Extent)>,
) -> Result<(Stbl, BoxHeader), Error> {
    let mut stbl = None;
    let mut drefs = None;
    let mut boxes = Boxes::new(ctx, h.payload(), 4, false)?;
    while let Some(c) = boxes.next_box()? {
        match &c.kind.0 {
            b"stbl" => once(&mut stbl, parse_stbl(ctx, &c, tables, other)?, "stbl")?,
            b"dinf" => {
                keep(ctx.budget, other, &c)?;
                once(&mut drefs, parse_dinf(ctx, &c)?, "dinf")?;
            }
            _ => keep(ctx.budget, other, &c)?,
        }
    }
    let (mut st, stsd) = stbl.ok_or(Error::Malformed("stbl"))?;
    st.drefs = drefs.unwrap_or_default();
    Ok((st, stsd))
}

/// `stsd` (depth 5 box): one entry per child. `visual` entries have the 78 fixed bytes of a
/// `VisualSampleEntry` and children that parse like `ipco` properties.
fn parse_stsd(
    ctx: &Ctx<'_>,
    h: &BoxHeader,
    visual: bool,
    props: &mut usize,
) -> Result<Vec<SampleEntry>, Error> {
    let p = h.payload();
    let bytes = ctx.slice(p.clone())?;
    full_header("stsd", bytes)?;
    if bytes.len() < 8 {
        return Err(Error::Malformed("stsd"));
    }
    let mut out = Vec::new();
    let mut boxes = Boxes::new(ctx, p.start.saturating_add(8)..p.end, 6, false)?;
    while let Some(e) = boxes.next_box()? {
        let entry = parse_entry(ctx, &e, visual, props)?;
        push(ctx.budget, &mut out, entry)?;
    }
    Ok(out)
}

fn parse_entry(
    ctx: &Ctx<'_>,
    h: &BoxHeader,
    visual: bool,
    props: &mut usize,
) -> Result<SampleEntry, Error> {
    let p = h.payload();
    let mut c = Cur::new("stsd", ctx.slice(p.clone())?, p.start);
    c.skip(6)?; // reserved
    let data_reference_index = c.u16()?;
    let mut size = None;
    let mut boxes = Vec::new();
    if visual {
        c.skip(16)?; // pre_defined, reserved, pre_defined[3]
        let width = c.u16()?;
        let height = c.u16()?;
        c.skip(50)?; // resolutions, reserved, frame_count, compressorname, depth, pre_defined
        size = Some((width, height));
        let mut kids = Boxes::new(ctx, c.offset()..p.end, 7, false)?.lenient_tail();
        while let Some(k) = kids.next_box()? {
            *props = props.saturating_add(1);
            if *props > ctx.limits.max_properties {
                return Err(Error::LimitExceeded(Limit::Properties));
            }
            push(ctx.budget, &mut boxes, parse_property(ctx, &k)?)?;
        }
    }
    Ok(SampleEntry {
        kind: h.kind,
        range: whole(h),
        data_reference_index,
        size,
        boxes,
    })
}

struct Mdia {
    timescale: u32,
    duration: u64,
    handler: FourCc,
    st: Stbl,
    stsd: BoxHeader,
}

/// `mdia` (depth 2 box).
fn parse_mdia(
    ctx: &Ctx<'_>,
    h: &BoxHeader,
    tables: &mut Vec<(FourCc, Extent)>,
    other: &mut Vec<(FourCc, Extent)>,
) -> Result<Mdia, Error> {
    let mut mdhd = None;
    let mut hdlr = None;
    let mut minf = None;
    let mut boxes = Boxes::new(ctx, h.payload(), 3, false)?;
    while let Some(c) = boxes.next_box()? {
        match &c.kind.0 {
            b"mdhd" => once(
                &mut mdhd,
                time_header("mdhd", ctx.slice(c.payload())?)?,
                "mdhd",
            )?,
            b"hdlr" => once(&mut hdlr, handler(ctx.slice(c.payload())?)?, "hdlr")?,
            b"minf" => once(&mut minf, parse_minf(ctx, &c, tables, other)?, "minf")?,
            _ => keep(ctx.budget, other, &c)?,
        }
    }
    let (timescale, duration) = mdhd.ok_or(Error::Malformed("mdhd"))?;
    let (st, stsd) = minf.ok_or(Error::Malformed("minf"))?;
    Ok(Mdia {
        timescale,
        duration,
        handler: hdlr.ok_or(Error::Malformed("hdlr"))?,
        st,
        stsd,
    })
}

/// `trak` (depth 1 box).
fn parse_trak(ctx: &Ctx<'_>, h: &BoxHeader, props: &mut usize) -> Result<Track, Error> {
    let mut tkhd = None;
    let mut mdia = None;
    let mut edits = None;
    let mut tables = Vec::new();
    let mut other = Vec::new();
    let mut boxes = Boxes::new(ctx, h.payload(), 2, false)?;
    while let Some(c) = boxes.next_box()? {
        match &c.kind.0 {
            b"tkhd" => once(&mut tkhd, parse_tkhd(ctx, &c)?, "tkhd")?,
            b"mdia" => once(
                &mut mdia,
                parse_mdia(ctx, &c, &mut tables, &mut other)?,
                "mdia",
            )?,
            b"edts" => parse_edts(ctx, &c, &mut edits, &mut other)?,
            _ => keep(ctx.budget, &mut other, &c)?,
        }
    }
    let tk: Tkhd = tkhd.ok_or(Error::Malformed("tkhd"))?;
    let md: Mdia = mdia.ok_or(Error::Malformed("mdia"))?;
    let visual = matches!(&md.handler.0, b"vide" | b"pict" | b"auxv");
    let entries = parse_stsd(ctx, &md.stsd, visual, props)?;
    Ok(Track {
        id: tk.id,
        flags: tk.flags,
        tkhd: tk.range,
        matrix: tk.matrix,
        width: tk.width,
        height: tk.height,
        handler: md.handler,
        timescale: md.timescale,
        duration: md.duration,
        entries,
        edits: edits.unwrap_or_default(),
        sample_count: md.st.sizes.table.count,
        tables,
        other_boxes: other,
        st: md.st,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn bx(kind: &[u8; 4], body: &[u8]) -> Vec<u8> {
        let mut v = ((body.len() as u32) + 8).to_be_bytes().to_vec();
        v.extend_from_slice(kind);
        v.extend_from_slice(body);
        v
    }

    #[test]
    fn first_box_rule() {
        assert!(first_box_ok(Some(b"ftyp")));
        assert!(first_box_ok(Some(b"wide")));
        assert!(!first_box_ok(Some(b"meta")));
        assert!(!first_box_ok(None));
    }

    #[test]
    fn mvhd_versions() {
        let mut v0 = vec![0, 0, 0, 0, 0, 0, 0, 1, 0, 0, 0, 2];
        v0.extend_from_slice(&600u32.to_be_bytes());
        v0.extend_from_slice(&1200u32.to_be_bytes());
        assert_eq!(time_header("mvhd", &v0), Ok((600, 1200)));
        let mut v1 = vec![1, 0, 0, 0];
        v1.extend_from_slice(&[0; 16]);
        v1.extend_from_slice(&90000u32.to_be_bytes());
        v1.extend_from_slice(&(1u64 << 40).to_be_bytes());
        assert_eq!(time_header("mdhd", &v1), Ok((90000, 1 << 40)));
        assert_eq!(
            time_header("mvhd", &[2, 0, 0, 0]),
            Err(Error::Malformed("mvhd"))
        );
    }

    #[test]
    fn no_moov_and_unknown_format() {
        let l = Limits::default();
        let c = AtomicBool::new(false);
        let ftyp = bx(b"ftyp", b"isom\0\0\0\0isom");
        assert_eq!(parse_movie(&ftyp, &l, &c).err(), Some(Error::NoMoov));
        assert_eq!(
            parse_movie(&bx(b"meta", &[0; 8]), &l, &c).err(),
            Some(Error::UnknownFormat)
        );
        assert_eq!(parse_movie(&[], &l, &c).err(), Some(Error::UnknownFormat));
        let two = [bx(b"moov", &[]), bx(b"moov", &[])].concat();
        assert_eq!(
            parse_movie(&two, &l, &c).err(),
            Some(Error::Malformed("moov"))
        );
    }
}
