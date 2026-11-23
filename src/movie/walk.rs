//! The sample walk: sample number to chunk to file offset, with `stts`, `ctts` and `stss` read
//! forward through small windows. A walk allocates six windows of at most 4 KiB, charged once,
//! and nothing else.

use std::io::{Read, Seek, SeekFrom};
use std::sync::atomic::{AtomicBool, Ordering};

use super::stbl::{SizeKind, Sizes, Stbl, Table};
use super::{Sample, Track};
use crate::budget::Budget;
use crate::{Error, Extent, Limits, read_at};

/// The most bytes of one table that a window holds.
const WINDOW: u64 = 4096;
/// A walk checks the cancel flag at least every 4096 samples (this mask plus one).
const TICK_MASK: u32 = 4095;

/// A forward window over the entries of one table, read with `read_at`.
struct Win<'a> {
    base: u64,
    len: u64,
    buf: Vec<u8>,
    /// Offset in the table of `buf[0]`, and how many bytes of `buf` are valid.
    at: u64,
    have: usize,
    cancel: &'a AtomicBool,
}

impl<'a> Win<'a> {
    fn new(budget: &Budget<'_>, cancel: &'a AtomicBool, e: Extent) -> Result<Self, Error> {
        let cap = usize::try_from(e.length.min(WINDOW)).unwrap_or(0);
        let mut buf = budget.vec::<u8>(cap)?;
        buf.resize(cap, 0);
        Ok(Self {
            base: e.offset,
            len: e.length,
            buf,
            at: 0,
            have: 0,
            cancel,
        })
    }

    /// `n` bytes (at most 16) at offset `rel` of the table. A read that leaves the window
    /// refills it, and checks the cancel flag.
    fn bytes<R: Read + Seek>(&mut self, r: &mut R, rel: u64, n: usize) -> Result<&[u8], Error> {
        let end = rel
            .checked_add(n as u64)
            .filter(|&e| e <= self.len)
            .ok_or(Error::Malformed("stbl"))?;
        if rel < self.at || end > self.at.saturating_add(self.have as u64) {
            if self.cancel.load(Ordering::Relaxed) {
                return Err(Error::Cancelled);
            }
            let want = self.len.saturating_sub(rel).min(self.buf.len() as u64);
            let want = usize::try_from(want).unwrap_or(0);
            let pos = self.base.checked_add(rel).ok_or(Error::Truncated)?;
            let dst = self.buf.get_mut(..want).ok_or(Error::Malformed("stbl"))?;
            read_at(r, pos, dst)?;
            self.at = rel;
            self.have = want;
        }
        let s = usize::try_from(rel.saturating_sub(self.at)).unwrap_or(usize::MAX);
        self.buf
            .get(s..s.saturating_add(n))
            .ok_or(Error::Malformed("stbl"))
    }

    fn arr<R: Read + Seek, const N: usize>(
        &mut self,
        r: &mut R,
        rel: u64,
    ) -> Result<[u8; N], Error> {
        <[u8; N]>::try_from(self.bytes(r, rel, N)?).map_err(|_| Error::Malformed("stbl"))
    }

    fn u32_at<R: Read + Seek>(&mut self, r: &mut R, rel: u64) -> Result<u32, Error> {
        Ok(u32::from_be_bytes(self.arr::<R, 4>(r, rel)?))
    }

    fn u64_at<R: Read + Seek>(&mut self, r: &mut R, rel: u64) -> Result<u64, Error> {
        Ok(u64::from_be_bytes(self.arr::<R, 8>(r, rel)?))
    }
}

/// `entry index * size`, or `Malformed(what)`.
fn at(idx: u64, each: u64, what: &'static str) -> Result<u64, Error> {
    idx.checked_mul(each).ok_or(Error::Malformed(what))
}

/// Decode times from `stts`, for increasing sample numbers.
struct Time<'a> {
    win: Win<'a>,
    count: u64,
    next: u64,
    /// The first sample number of the current run, its length, delta and decode time.
    first: u64,
    run: u64,
    delta: u64,
    dts: u64,
    loaded: bool,
    /// Past the last run: its delta repeats.
    forever: bool,
}

impl<'a> Time<'a> {
    fn new(win: Win<'a>, count: u64) -> Self {
        Self {
            win,
            count,
            next: 0,
            first: 1,
            run: 0,
            delta: 0,
            dts: 0,
            loaded: false,
            forever: false,
        }
    }

    /// The decode time of sample `n` (at least 1, not below the one asked before).
    fn dts<R: Read + Seek>(&mut self, r: &mut R, n: u64) -> Result<i64, Error> {
        const BAD: Error = Error::Malformed("stts");
        loop {
            let end = self.first.checked_add(self.run).ok_or(BAD)?;
            if self.forever || n < end {
                break;
            }
            // The current run lies before `n`: its length moves the run start time.
            let total = self
                .run
                .checked_mul(self.delta)
                .and_then(|x| self.dts.checked_add(x))
                .filter(|&x| i64::try_from(x).is_ok())
                .ok_or(BAD)?;
            self.dts = total;
            self.first = end;
            if self.next < self.count {
                let off = at(self.next, 8, "stts")?;
                self.run = u64::from(self.win.u32_at(r, off)?);
                self.delta = u64::from(self.win.u32_at(r, off.saturating_add(4))?);
                self.next = self.next.saturating_add(1);
                self.loaded = true;
            } else if self.loaded {
                self.forever = true;
            } else {
                return Err(BAD);
            }
        }
        let v = n
            .checked_sub(self.first)
            .and_then(|k| k.checked_mul(self.delta))
            .and_then(|x| self.dts.checked_add(x))
            .ok_or(BAD)?;
        i64::try_from(v).map_err(|_| BAD)
    }
}

/// Composition offsets from `ctts`, read as signed for both versions.
struct Cto<'a> {
    win: Win<'a>,
    count: u64,
    next: u64,
    first: u64,
    run: u64,
    off: i32,
    done: bool,
}

impl<'a> Cto<'a> {
    fn new(win: Win<'a>, count: u64) -> Self {
        Self {
            win,
            count,
            next: 0,
            first: 1,
            run: 0,
            off: 0,
            done: false,
        }
    }

    /// The offset of sample `n`; 0 past the last run.
    fn cto<R: Read + Seek>(&mut self, r: &mut R, n: u64) -> Result<i32, Error> {
        loop {
            if self.done {
                return Ok(0);
            }
            let end = self.first.saturating_add(self.run);
            if n < end {
                return Ok(self.off);
            }
            if self.next >= self.count {
                self.done = true;
                continue;
            }
            let o = at(self.next, 8, "ctts")?;
            self.run = u64::from(self.win.u32_at(r, o)?);
            self.off = i32::from_be_bytes(self.win.arr::<R, 4>(r, o.saturating_add(4))?);
            self.first = end;
            self.next = self.next.saturating_add(1);
        }
    }
}

/// The listed sync samples of `stss`.
struct Stss<'a> {
    win: Win<'a>,
    count: u64,
    idx: u64,
    prev: u64,
    /// The last entry read.
    head: Option<u64>,
}

impl<'a> Stss<'a> {
    fn new(win: Win<'a>, count: u64) -> Self {
        Self {
            win,
            count,
            idx: 0,
            prev: 0,
            head: None,
        }
    }

    /// The next listed sample number; entries must be at least 1 and strictly increasing.
    fn next_entry<R: Read + Seek>(&mut self, r: &mut R) -> Result<Option<u64>, Error> {
        if self.idx >= self.count {
            return Ok(None);
        }
        let v = u64::from(self.win.u32_at(r, at(self.idx, 4, "stss")?)?);
        if v == 0 || v <= self.prev {
            return Err(Error::Malformed("stss"));
        }
        self.prev = v;
        self.idx = self.idx.saturating_add(1);
        self.head = Some(v);
        Ok(Some(v))
    }

    /// True when sample `n` is listed (`n` does not decrease between calls).
    fn is_sync<R: Read + Seek>(&mut self, r: &mut R, n: u64) -> Result<bool, Error> {
        loop {
            if let Some(h) = self.head
                && h >= n
            {
                return Ok(h == n);
            }
            if self.next_entry(r)?.is_none() {
                return Ok(false);
            }
        }
    }
}

/// The size of sample `n`, from `stsz` or `stz2`.
struct Size<'a> {
    kind: SizeKind,
    win: Win<'a>,
}

impl Size<'_> {
    /// The size of sample number `n` (1-based).
    fn get<R: Read + Seek>(&mut self, r: &mut R, n: u64) -> Result<u64, Error> {
        let i = n.checked_sub(1).ok_or(Error::Malformed("stsz"))?;
        match self.kind {
            SizeKind::Constant(s) => Ok(u64::from(s)),
            SizeKind::Stsz => Ok(u64::from(self.win.u32_at(r, at(i, 4, "stsz")?)?)),
            SizeKind::Stz2(16) => Ok(u64::from(u16::from_be_bytes(
                self.win.arr::<R, 2>(r, at(i, 2, "stz2")?)?,
            ))),
            SizeKind::Stz2(8) => Ok(u64::from(u8::from_be_bytes(self.win.arr::<R, 1>(r, i)?))),
            SizeKind::Stz2(_) => {
                let [b] = self.win.arr::<R, 1>(r, i.checked_div(2).unwrap_or(0))?;
                // 4-bit fields, high nibble first.
                let v = if i & 1 == 0 {
                    b.checked_shr(4).unwrap_or(0)
                } else {
                    b & 0x0f
                };
                Ok(u64::from(v))
            }
        }
    }
}

/// Sample number to file offset: the `stsc` runs, the chunk offsets and the sizes.
struct Loc<'a> {
    stsc: Win<'a>,
    stsc_count: u64,
    co: Win<'a>,
    wide: bool,
    what: &'static str,
    chunks: u64,
    size: Size<'a>,
    started: bool,
    /// The `stsc` entry after the current run: (first chunk, samples per chunk, description).
    ahead: Option<(u64, u64, u32)>,
    next_run: u64,
    /// The current run.
    spc: u64,
    desc: u32,
    /// The current chunk (1-based), the samples already taken from it, and the file offset of
    /// the next sample.
    c: u64,
    k: u64,
    off: u64,
    /// The samples taken so far.
    n: u64,
}

impl Loc<'_> {
    fn entry<R: Read + Seek>(&mut self, r: &mut R, i: u64) -> Result<(u64, u64, u32), Error> {
        let o = at(i, 12, "stsc")?;
        Ok((
            u64::from(self.stsc.u32_at(r, o)?),
            u64::from(self.stsc.u32_at(r, o.saturating_add(4))?),
            self.stsc.u32_at(r, o.saturating_add(8))?,
        ))
    }

    /// Reads the first `stsc` entry; the first chunk must be 1.
    fn start<R: Read + Seek>(&mut self, r: &mut R) -> Result<(), Error> {
        self.started = true;
        if self.stsc_count == 0 {
            return Err(Error::Malformed("stsc"));
        }
        let e = self.entry(r, 0)?;
        if e.0 != 1 {
            return Err(Error::Malformed("stsc"));
        }
        self.ahead = Some(e);
        self.next_run = 1;
        Ok(())
    }

    /// Makes the entry in `ahead` the current run and reads the one after it.
    fn load_run<R: Read + Seek>(&mut self, r: &mut R) -> Result<(), Error> {
        let (first, spc, desc) = self.ahead.ok_or(Error::Malformed("stsc"))?;
        self.spc = spc;
        self.desc = desc;
        self.ahead = if self.next_run < self.stsc_count {
            let e = self.entry(r, self.next_run)?;
            if e.0 <= first {
                return Err(Error::Malformed("stsc"));
            }
            self.next_run = self.next_run.saturating_add(1);
            Some(e)
        } else {
            None
        };
        Ok(())
    }

    fn chunk_offset<R: Read + Seek>(&mut self, r: &mut R, c: u64) -> Result<u64, Error> {
        let i = c.checked_sub(1).ok_or(Error::Malformed(self.what))?;
        if self.wide {
            self.co.u64_at(r, at(i, 8, self.what)?)
        } else {
            Ok(u64::from(self.co.u32_at(r, at(i, 4, self.what)?)?))
        }
    }

    /// Moves to the start of the next chunk that holds samples.
    fn next_chunk<R: Read + Seek>(&mut self, r: &mut R) -> Result<(), Error> {
        if !self.started {
            self.start(r)?;
        }
        loop {
            self.c = self.c.saturating_add(1);
            match self.ahead {
                Some((f, _, _)) if f == self.c => self.load_run(r)?,
                _ => {}
            }
            if self.c > self.chunks {
                return Err(Error::Malformed(self.what));
            }
            if self.spc != 0 {
                break;
            }
            // A run without samples: go to its last chunk, the next step ends it.
            match self.ahead {
                Some((f, _, _)) => self.c = f.saturating_sub(1),
                None => return Err(Error::Malformed(self.what)),
            }
        }
        self.k = 0;
        self.off = self.chunk_offset(r, self.c)?;
        Ok(())
    }

    /// Adds the sizes of the next `m` samples to the offset.
    fn add_sizes<R: Read + Seek>(&mut self, r: &mut R, m: u64) -> Result<(), Error> {
        let sum = if let SizeKind::Constant(s) = self.size.kind {
            m.checked_mul(u64::from(s))
                .ok_or(Error::Malformed(self.what))?
        } else {
            let mut sum: u64 = 0;
            for i in 0..m {
                let n = self.n.saturating_add(1).saturating_add(i);
                sum = sum
                    .checked_add(self.size.get(r, n)?)
                    .ok_or(Error::Malformed(self.what))?;
            }
            sum
        };
        self.off = self
            .off
            .checked_add(sum)
            .ok_or(Error::Malformed(self.what))?;
        self.k = self.k.saturating_add(m);
        self.n = self.n.saturating_add(m);
        Ok(())
    }

    /// The next sample: its offset, size and description index.
    fn next_sample<R: Read + Seek>(&mut self, r: &mut R) -> Result<(u64, u64, u32), Error> {
        if self.k >= self.spc {
            self.next_chunk(r)?;
        }
        let size = self.size.get(r, self.n.saturating_add(1))?;
        let off = self.off;
        self.off = off.checked_add(size).ok_or(Error::Malformed(self.what))?;
        self.k = self.k.saturating_add(1);
        self.n = self.n.saturating_add(1);
        Ok((off, size, self.desc))
    }

    /// Moves past `rem` samples: whole chunks by run lengths, without reading `stsz`, then the
    /// sizes of the samples before the target in its chunk.
    fn skip<R: Read + Seek>(&mut self, r: &mut R, mut rem: u64) -> Result<(), Error> {
        let avail = self.spc.saturating_sub(self.k);
        if rem < avail {
            return self.add_sizes(r, rem);
        }
        self.n = self.n.saturating_add(avail);
        self.k = self.spc;
        rem = rem.saturating_sub(avail);
        while rem > 0 {
            self.next_chunk(r)?;
            let last = self.ahead.map_or(self.chunks, |(f, _, _)| {
                f.saturating_sub(1).min(self.chunks)
            });
            let left = last.saturating_sub(self.c).saturating_add(1);
            let q = rem.checked_div(self.spc).unwrap_or(0).min(left);
            if q == 0 {
                return self.add_sizes(r, rem);
            }
            let took = q.checked_mul(self.spc).ok_or(Error::Malformed(self.what))?;
            self.n = self.n.saturating_add(took);
            rem = rem.saturating_sub(took);
            self.c = self.c.saturating_add(q.saturating_sub(1));
            self.k = self.spc;
        }
        Ok(())
    }
}

/// One walk over the samples of a track, forward only.
pub(crate) struct Walk<'a> {
    t: &'a Track,
    cancel: &'a AtomicBool,
    file_len: u64,
    loc: Loc<'a>,
    time: Time<'a>,
    cto: Option<Cto<'a>>,
    sync: Option<Stss<'a>>,
    ticks: u32,
}

impl<'a> Walk<'a> {
    pub(crate) fn new<R: Read + Seek>(
        t: &'a Track,
        r: &mut R,
        budget: &Budget<'_>,
        cancel: &'a AtomicBool,
    ) -> Result<Self, Error> {
        budget.check_cancel()?;
        let file_len = r.seek(SeekFrom::End(0)).map_err(crate::io_err)?;
        let st: &Stbl = &t.st;
        let win = |tb: &Table| Win::new(budget, cancel, tb.entries);
        let Sizes { kind, table } = st.sizes;
        let loc = Loc {
            stsc: win(&st.stsc)?,
            stsc_count: st.stsc.count,
            co: win(&st.chunks.table)?,
            wide: st.chunks.wide,
            what: if st.chunks.wide { "co64" } else { "stco" },
            chunks: st.chunks.table.count,
            size: Size {
                kind,
                win: win(&table)?,
            },
            started: false,
            ahead: None,
            next_run: 0,
            spc: 0,
            desc: 0,
            c: 0,
            k: 0,
            off: 0,
            n: 0,
        };
        let cto = match &st.ctts {
            Some(tb) => Some(Cto::new(win(tb)?, tb.count)),
            None => None,
        };
        let sync = match &st.stss {
            Some(tb) => Some(Stss::new(win(tb)?, tb.count)),
            None => None,
        };
        Ok(Self {
            t,
            cancel,
            file_len,
            loc,
            time: Time::new(win(&st.stts)?, st.stts.count),
            cto,
            sync,
            ticks: 0,
        })
    }

    /// True when the track lists its sync samples (`stss`).
    pub(crate) fn has_stss(&self) -> bool {
        self.sync.is_some()
    }

    /// The next sample number listed in `stss`, `None` at the end (and without `stss`).
    pub(crate) fn next_listed<R: Read + Seek>(&mut self, r: &mut R) -> Result<Option<u64>, Error> {
        match &mut self.sync {
            Some(s) => s.next_entry(r),
            None => Ok(None),
        }
    }

    /// Sample number `n` (1-based), which must come after the one asked before. Samples in
    /// between are skipped without being read.
    pub(crate) fn sample<R: Read + Seek>(&mut self, r: &mut R, n: u64) -> Result<Sample, Error> {
        self.ticks = self.ticks.wrapping_add(1);
        if self.ticks & TICK_MASK == 0 && self.cancel.load(Ordering::Relaxed) {
            return Err(Error::Cancelled);
        }
        let gap = n.saturating_sub(self.loc.n.saturating_add(1));
        if gap > 0 {
            self.loc.skip(r, gap)?;
        }
        let (off, size, desc) = self.loc.next_sample(r)?;
        off.checked_add(size)
            .filter(|&e| e <= self.file_len)
            .ok_or(Error::Truncated)?;
        let entry = usize::try_from(desc.saturating_sub(1))
            .ok()
            .and_then(|i| self.t.entries.get(i))
            .filter(|_| desc > 0)
            .ok_or(Error::Malformed("stsc"))?;
        let external = entry
            .data_reference_index
            .checked_sub(1)
            .is_some_and(|i| self.t.st.drefs.get(usize::from(i)) == Some(&false));
        if external {
            return Err(Error::Unsupported("data reference"));
        }
        let dts = self.time.dts(r, n)?;
        let cto = match &mut self.cto {
            Some(c) => c.cto(r, n)?,
            None => 0,
        };
        let sync = match &mut self.sync {
            Some(s) => s.is_sync(r, n)?,
            None => true,
        };
        Ok(Sample {
            number: n,
            range: Extent {
                offset: off,
                length: size,
            },
            dts,
            cto,
            sync,
            entry: desc,
            flags: None,
        })
    }
}

/// The samples of a track in decode order (see [`Track::samples`]). The first `Err` ends it.
pub struct Samples<'a, R> {
    t: &'a Track,
    r: R,
    budget: Budget<'a>,
    cancel: &'a AtomicBool,
    walk: Option<Walk<'a>>,
    next: u64,
    done: bool,
}

impl<R: Read + Seek> Iterator for Samples<'_, R> {
    type Item = Result<Sample, Error>;

    fn next(&mut self) -> Option<Self::Item> {
        if self.done || self.next > self.t.sample_count {
            return None;
        }
        if self.walk.is_none() {
            match Walk::new(self.t, &mut self.r, &self.budget, self.cancel) {
                Ok(w) => self.walk = Some(w),
                Err(e) => {
                    self.done = true;
                    return Some(Err(e));
                }
            }
        }
        let w = self.walk.as_mut()?;
        match w.sample(&mut self.r, self.next) {
            Ok(s) => {
                self.next = self.next.saturating_add(1);
                Some(Ok(s))
            }
            Err(e) => {
                self.done = true;
                Some(Err(e))
            }
        }
    }
}

impl Track {
    /// The samples of the track in decode order, with their file ranges, decode and composition
    /// times and sync flags. `r` reads the same file that the track was parsed from; the walk
    /// reads only the table bytes it passes over. The first error ends the iteration. Limits:
    /// `max_alloc` (the walk windows, 4 KiB each), cancel (every 4096 samples and table windows).
    ///
    /// # Errors
    ///
    /// Each item is an `Err` on a bad table ([`Error::Malformed`]), a sample past the end of the
    /// file ([`Error::Truncated`]), an external data reference ([`Error::Unsupported`]), a
    /// reader error, or a limit.
    pub fn samples<'a, R: Read + Seek>(
        &'a self,
        r: R,
        limits: &Limits,
        cancel: &'a AtomicBool,
    ) -> Samples<'a, R> {
        Samples {
            t: self,
            r,
            budget: Budget::new(limits, cancel),
            cancel,
            walk: None,
            next: 1,
            done: false,
        }
    }

    /// The first `n` sync samples, in sample order. Without `stss` every sample is a sync
    /// sample. The result is charged for at most `min(n, sync sample count)` samples.
    ///
    /// # Errors
    ///
    /// As [`samples`](Self::samples), and [`Error::LimitExceeded`] for the result.
    pub fn sync_samples<R: Read + Seek>(
        &self,
        mut r: R,
        n: usize,
        limits: &Limits,
        cancel: &AtomicBool,
    ) -> Result<Vec<Sample>, Error> {
        let budget = Budget::new(limits, cancel);
        budget.check_cancel()?;
        let bound = self
            .st
            .stss
            .map_or(self.sample_count, |s| s.count.min(self.sample_count));
        let want = usize::try_from(bound).unwrap_or(usize::MAX).min(n);
        let mut out = budget.vec::<Sample>(want)?;
        if want == 0 {
            return Ok(out);
        }
        let mut w = Walk::new(self, &mut r, &budget, cancel)?;
        let mut next: u64 = 1;
        while out.len() < want {
            let v = if w.has_stss() {
                match w.next_listed(&mut r)? {
                    Some(v) if v <= self.sample_count => v,
                    _ => break,
                }
            } else {
                next
            };
            out.push(w.sample(&mut r, v)?);
            next = next.saturating_add(1);
        }
        Ok(out)
    }

    /// The sync sample with the greatest composition time `pts <= t` (`t` in the media
    /// timescale, no edit list applied); the lowest number among equals. When no sync sample
    /// has `pts <= t`, the lowest-numbered sync sample. `None` when the track has no sync
    /// sample. It visits every sync sample, because composition times are not monotone.
    ///
    /// # Errors
    ///
    /// As [`samples`](Self::samples).
    pub fn sync_sample_at<R: Read + Seek>(
        &self,
        mut r: R,
        t: i64,
        limits: &Limits,
        cancel: &AtomicBool,
    ) -> Result<Option<Sample>, Error> {
        let budget = Budget::new(limits, cancel);
        budget.check_cancel()?;
        if self.sample_count == 0 {
            return Ok(None);
        }
        let mut w = Walk::new(self, &mut r, &budget, cancel)?;
        let mut first: Option<Sample> = None;
        let mut best: Option<Sample> = None;
        let mut next: u64 = 1;
        loop {
            let v = if w.has_stss() {
                match w.next_listed(&mut r)? {
                    Some(v) if v <= self.sample_count => v,
                    _ => break,
                }
            } else if next <= self.sample_count {
                next
            } else {
                break;
            };
            next = next.saturating_add(1);
            let s = w.sample(&mut r, v)?;
            first.get_or_insert(s);
            if s.pts() <= t && best.is_none_or(|b| s.pts() > b.pts()) {
                best = Some(s);
            }
        }
        Ok(best.or(first))
    }
}
