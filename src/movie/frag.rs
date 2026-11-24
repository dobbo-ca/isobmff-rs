//! Movie fragments: the `trex` defaults of `mvex`, then every `moof`. A `trun` becomes one
//! `Run`: where its samples start in the file, their first decode time, the range of its
//! per-sample records and the defaults that apply. The records stay in the file; `walk.rs` reads
//! them. Parsing sums the durations and sizes of each run once (constant memory, the cancel flag
//! every 4096 records) to know where the next run starts.

use super::stbl::Table;
use super::{Track, keep, push};
use crate::boxes::{BoxHeader, Boxes, Ctx, Cur, be32, full_header};
use crate::budget::Budget;
use crate::{Error, Extent, FourCc};

/// `trex`: the defaults of one track.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Trex {
    id: u32,
    desc: u32,
    dur: u32,
    size: u32,
    flags: u32,
}

/// One `trun`: a run of samples of a track.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Run {
    /// `sample_count`, at least 1.
    pub(crate) count: u32,
    /// File offset of the first sample.
    pub(crate) start: u64,
    /// Decode time of the first sample.
    pub(crate) dts: i64,
    /// The per-sample records.
    pub(crate) entries: Extent,
    /// The `trun` flags: which fields a record has.
    pub(crate) flags: u32,
    /// `first_sample_flags`.
    pub(crate) first_flags: Option<u32>,
    /// The defaults (`tfhd`, else `trex`) for a field a record does not have.
    pub(crate) dur: u32,
    pub(crate) size: u32,
    pub(crate) sflags: u32,
    /// The sample description index, checked against the entries of the track.
    pub(crate) desc: u32,
}

/// Bytes per record of a `trun` with these flags: 4 for each of duration, size, flags, cto.
pub(crate) fn rec_len(f: u32) -> usize {
    ((f & 0xF00).count_ones() as usize).saturating_mul(4)
}

/// Where the field `bit` (`0x100` duration, `0x200` size, `0x400` flags, `0x800` cto) sits in a
/// record, `None` when the records do not have it.
pub(crate) fn field_at(f: u32, bit: u32) -> Option<usize> {
    (f & bit != 0).then(|| rec_len(f & bit.saturating_sub(1)))
}

fn parse_trex(bytes: &[u8]) -> Result<Trex, Error> {
    let (_, _, rest) = full_header("trex", bytes)?;
    let mut c = Cur::new("trex", rest, 0);
    Ok(Trex {
        id: c.u32()?,
        desc: c.u32()?,
        dur: c.u32()?,
        size: c.u32()?,
        flags: c.u32()?,
    })
}

/// The `trex` children of an `mvex` (depth 1 box).
pub(super) fn parse_mvex(ctx: &Ctx<'_>, h: &BoxHeader, out: &mut Vec<Trex>) -> Result<(), Error> {
    let mut boxes = Boxes::new(ctx, h.payload(), 2, false)?;
    while let Some(c) = boxes.next_box()? {
        if &c.kind.0 == b"trex" {
            push(ctx.budget, out, parse_trex(ctx.slice(c.payload())?)?)?;
        }
    }
    Ok(())
}

/// The sum of the `stts` deltas: where the fragments of a track start when no `tfdt` says.
fn stts_total(ctx: &Ctx<'_>, t: &Table) -> Result<i64, Error> {
    const BAD: Error = Error::Malformed("stts");
    let end = t.entries.end().ok_or(BAD)?;
    let bytes = ctx.slice(t.entries.offset..end)?;
    let mut total: u64 = 0;
    for (i, e) in bytes.as_chunks::<8>().0.iter().enumerate() {
        if i & 4095 == 4095 {
            ctx.budget.check_cancel()?;
        }
        let (Some(n), Some(d)) = (be32(e, 0), be32(e, 4)) else {
            return Err(BAD);
        };
        total = u64::from(n)
            .checked_mul(u64::from(d))
            .and_then(|x| total.checked_add(x))
            .ok_or(BAD)?;
    }
    i64::try_from(total).map_err(|_| BAD)
}

/// After the `moov` walk: gives every track its `trex` (a duplicate is `Malformed("trex")`) and
/// the decode time where its moov samples end.
pub(super) fn finish_moov(
    ctx: &Ctx<'_>,
    mut trexes: Vec<Trex>,
    tracks: &mut [Track],
) -> Result<(), Error> {
    trexes.sort_unstable_by_key(|t| t.id);
    if trexes
        .windows(2)
        .any(|w| w.first().map(|a| a.id) == w.get(1).map(|b| b.id))
    {
        return Err(Error::Malformed("trex"));
    }
    for t in tracks {
        t.trex = trexes
            .binary_search_by_key(&t.id, |x| x.id)
            .ok()
            .and_then(|i| trexes.get(i))
            .copied();
        t.end_dts = stts_total(ctx, &t.st.stts)?;
    }
    Ok(())
}

struct Tfhd {
    id: u32,
    base: Option<u64>,
    desc: Option<u32>,
    dur: Option<u32>,
    size: Option<u32>,
    flags: Option<u32>,
    moof_base: bool,
}

fn parse_tfhd(bytes: &[u8]) -> Result<Tfhd, Error> {
    let (_, f, rest) = full_header("tfhd", bytes)?;
    let mut c = Cur::new("tfhd", rest, 0);
    let id = c.u32()?;
    let base = if f & 0x01 != 0 { Some(c.u64()?) } else { None };
    let desc = if f & 0x02 != 0 { Some(c.u32()?) } else { None };
    let dur = if f & 0x08 != 0 { Some(c.u32()?) } else { None };
    let size = if f & 0x10 != 0 { Some(c.u32()?) } else { None };
    let flags = if f & 0x20 != 0 { Some(c.u32()?) } else { None };
    if id == 0 {
        return Err(Error::Malformed("tfhd"));
    }
    Ok(Tfhd {
        id,
        base,
        desc,
        dur,
        size,
        flags,
        moof_base: f & 0x02_0000 != 0,
    })
}

/// `tfdt`: `baseMediaDecodeTime`, version 0 (u32) or 1 (u64).
fn parse_tfdt(bytes: &[u8]) -> Result<i64, Error> {
    const BAD: Error = Error::Malformed("tfdt");
    let (v, _, rest) = full_header("tfdt", bytes)?;
    let mut c = Cur::new("tfdt", rest, 0);
    let t = match v {
        0 => u64::from(c.u32()?),
        1 => c.u64()?,
        _ => return Err(BAD),
    };
    i64::try_from(t).map_err(|_| BAD)
}

/// What a `traf` has settled when its `trun` boxes are read.
struct Traf {
    /// The base data offset.
    base: u64,
    /// Where the next run starts without a `data_offset`: the end of the previous run.
    cursor: u64,
    /// The decode time of the next run.
    dts: i64,
    desc: u32,
    dur: u32,
    size: u32,
    flags: u32,
}

/// The sum of one field over the records of a `trun`: from the records when they have it, else
/// `count * default`.
fn sum(
    ctx: &Ctx<'_>,
    recs: &[u8],
    rec: usize,
    pos: Option<usize>,
    count: u32,
    default: u32,
) -> Result<u64, Error> {
    const BAD: Error = Error::Malformed("trun");
    let Some(pos) = pos else {
        return u64::from(count).checked_mul(u64::from(default)).ok_or(BAD);
    };
    let mut total: u64 = 0;
    for (i, r) in recs.chunks_exact(rec.max(1)).enumerate() {
        if i & 4095 == 4095 {
            ctx.budget.check_cancel()?;
        }
        let v = be32(r, pos).ok_or(BAD)?;
        total = total.checked_add(u64::from(v)).ok_or(BAD)?;
    }
    Ok(total)
}

/// One `trun`: the run, where its data ends and the decode time after it.
fn parse_trun(ctx: &Ctx<'_>, h: &BoxHeader, tf: &Traf) -> Result<(Run, u64, i64), Error> {
    const BAD: Error = Error::Malformed("trun");
    let p = h.payload();
    let (_, f, rest) = full_header("trun", ctx.slice(p.clone())?)?;
    let mut c = Cur::new("trun", rest, p.start.saturating_add(4));
    let count = c.u32()?;
    let data_offset = if f & 0x01 != 0 { Some(c.i32()?) } else { None };
    let first_flags = if f & 0x04 != 0 { Some(c.u32()?) } else { None };
    let rec = rec_len(f);
    let total = usize::try_from(count)
        .ok()
        .and_then(|n| n.checked_mul(rec))
        .filter(|&t| t <= c.remaining())
        .ok_or(BAD)?;
    let at = c.offset();
    let recs = c.take(total)?;
    let (dpos, spos) = (field_at(f, 0x100), field_at(f, 0x200));
    // A run whose samples have no size would be as long as `count` says, whatever the file holds.
    if spos.is_none() && tf.size == 0 && count > 0 {
        return Err(BAD);
    }
    let dur = sum(ctx, recs, rec, dpos, count, tf.dur)?;
    let size = sum(ctx, recs, rec, spos, count, tf.size)?;
    let start = match data_offset {
        Some(o) => tf.base.checked_add_signed(i64::from(o)).ok_or(BAD)?,
        None => tf.cursor,
    };
    let end = start.checked_add(size).ok_or(BAD)?;
    let dts_end = i64::try_from(dur)
        .ok()
        .and_then(|d| tf.dts.checked_add(d))
        .ok_or(BAD)?;
    let run = Run {
        count,
        start,
        dts: tf.dts,
        entries: Extent {
            offset: at,
            length: total as u64,
        },
        flags: f,
        first_flags,
        dur: tf.dur,
        size: tf.size,
        sflags: tf.flags,
        desc: tf.desc,
    };
    Ok((run, end, dts_end))
}

/// `traf` (depth 1 box). `tfhd` must come first. A `traf` of a track that is not in `moov` is
/// kept whole in `other`. `implicit` is where the data of the last run of this `moof` ended.
fn parse_traf(
    ctx: &Ctx<'_>,
    h: &BoxHeader,
    moof: u64,
    implicit: &mut u64,
    tracks: &mut [Track],
    index: &[(u32, usize)],
    other: &mut Vec<(FourCc, Extent)>,
) -> Result<(), Error> {
    let mut boxes = Boxes::new(ctx, h.payload(), 2, false)?;
    let first = boxes
        .next_box()?
        .filter(|c| &c.kind.0 == b"tfhd")
        .ok_or(Error::Malformed("traf"))?;
    let tf = parse_tfhd(ctx.slice(first.payload())?)?;
    let at = index.partition_point(|&(id, _)| id < tf.id);
    let found = index
        .get(at)
        .filter(|&&(id, _)| id == tf.id)
        .and_then(|&(_, i)| tracks.get_mut(i));
    let Some(t) = found else {
        return keep(ctx.budget, other, h);
    };
    let trex = t.trex.ok_or(Error::Malformed("trex"))?;
    let desc = tf.desc.unwrap_or(trex.desc);
    let known = usize::try_from(desc)
        .ok()
        .is_some_and(|d| d != 0 && d <= t.entries.len());
    if !known {
        return Err(Error::Malformed("tfhd"));
    }
    let base = tf
        .base
        .unwrap_or(if tf.moof_base { moof } else { *implicit });
    let mut st = Traf {
        base,
        cursor: base,
        dts: t.end_dts,
        desc,
        dur: tf.dur.unwrap_or(trex.dur),
        size: tf.size.unwrap_or(trex.size),
        flags: tf.flags.unwrap_or(trex.flags),
    };
    let mut ran = false;
    while let Some(c) = boxes.next_box()? {
        match &c.kind.0 {
            b"tfdt" => {
                let v = parse_tfdt(ctx.slice(c.payload())?)?;
                if !ran {
                    st.dts = v;
                }
            }
            b"trun" => {
                let (run, end, dts) = parse_trun(ctx, &c, &st)?;
                if run.count > 0 {
                    t.sample_count = t
                        .sample_count
                        .checked_add(u64::from(run.count))
                        .ok_or(Error::Malformed("trun"))?;
                    push(ctx.budget, &mut t.runs, run)?;
                    st.cursor = end;
                    st.dts = dts;
                    t.end_dts = dts;
                    *implicit = end;
                    ran = true;
                }
            }
            _ => keep(ctx.budget, &mut t.other_boxes, &c)?,
        }
    }
    Ok(())
}

/// (id, index) of every track sorted by id then index, charged to the budget: `parse_traf` finds
/// a track by binary search. The caller gives the charge back (`index_bytes`) after the `moof`s.
pub(super) fn track_index(
    budget: &Budget<'_>,
    tracks: &[Track],
) -> Result<Vec<(u32, usize)>, Error> {
    let mut v = budget.vec(tracks.len())?;
    v.extend(tracks.iter().enumerate().map(|(i, t)| (t.id, i)));
    v.sort_unstable();
    Ok(v)
}

/// Bytes `track_index` charged for `n` tracks.
pub(super) fn index_bytes(n: usize) -> usize {
    n.saturating_mul(size_of::<(u32, usize)>())
}

/// `moof` (depth 0 box): every `traf` adds runs to its track; the other children are appended to
/// `other`. A `moof` in a file whose `moov` has no `mvex` is `Malformed("mvex")`.
pub(super) fn parse_moof(
    ctx: &Ctx<'_>,
    h: &BoxHeader,
    fragmented: bool,
    tracks: &mut [Track],
    index: &[(u32, usize)],
    other: &mut Vec<(FourCc, Extent)>,
) -> Result<(), Error> {
    if !fragmented {
        return Err(Error::Malformed("mvex"));
    }
    let mut implicit = h.start;
    let mut boxes = Boxes::new(ctx, h.payload(), 1, false)?;
    while let Some(c) = boxes.next_box()? {
        if &c.kind.0 == b"traf" {
            parse_traf(ctx, &c, h.start, &mut implicit, tracks, index, other)?;
        } else {
            keep(ctx.budget, other, &c)?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn record_layout() {
        assert_eq!(rec_len(0), 0);
        assert_eq!(rec_len(0xF00), 16);
        assert_eq!(field_at(0x100, 0x100), Some(0));
        assert_eq!(field_at(0x300, 0x200), Some(4));
        assert_eq!(field_at(0xF00, 0x800), Some(12));
        assert_eq!(field_at(0x100, 0x200), None);
    }
}
