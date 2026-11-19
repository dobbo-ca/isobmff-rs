//! The general ISO-BMFF box reader, used by every parser (and by the later MP4/MOV work).

use std::cell::Cell;
use std::ops::Range;

use crate::budget::Budget;
use crate::{Error, FourCc, Limit, Limits};

/// A range of the input file, in absolute file offsets.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub struct Extent {
    /// Offset of the first byte from the start of the file.
    pub offset: u64,
    /// Number of bytes.
    pub length: u64,
}

impl Extent {
    /// `offset + length`, or `None` on overflow.
    pub fn end(&self) -> Option<u64> {
        self.offset.checked_add(self.length)
    }
}

/// The bytes of the input that a parse can see. `bytes[0]` is file offset `base`; `read` parses
/// a copy of `meta` with `base` = the offset of the `meta` box. `file_len` is the length of the
/// whole file.
pub(crate) struct Src<'a> {
    pub(crate) bytes: &'a [u8],
    pub(crate) base: u64,
    pub(crate) file_len: u64,
}

impl<'a> Src<'a> {
    /// The bytes of an absolute range, or `None` when they are not all visible.
    pub(crate) fn get(&self, r: Range<u64>) -> Option<&'a [u8]> {
        let s = usize::try_from(r.start.checked_sub(self.base)?).ok()?;
        let e = usize::try_from(r.end.checked_sub(self.base)?).ok()?;
        self.bytes.get(s..e)
    }
}

/// What one parse shares: the visible bytes, the budget, the limits and the box counters.
pub(crate) struct Ctx<'a> {
    pub(crate) src: &'a Src<'a>,
    pub(crate) budget: &'a Budget<'a>,
    pub(crate) limits: &'a Limits,
    pub(crate) boxes: Cell<usize>,
    pub(crate) max_depth: Cell<usize>,
}

impl<'a> Ctx<'a> {
    pub(crate) fn new(src: &'a Src<'a>, budget: &'a Budget<'a>, limits: &'a Limits) -> Self {
        Self {
            src,
            budget,
            limits,
            boxes: Cell::new(0),
            max_depth: Cell::new(0),
        }
    }

    /// Counts one box against `max_boxes`; checks the cancel flag every 1024 boxes.
    pub(crate) fn tick(&self) -> Result<(), Error> {
        let n = self.boxes.get().saturating_add(1);
        self.boxes.set(n);
        if n > self.limits.max_boxes {
            return Err(Error::LimitExceeded(Limit::Boxes));
        }
        if n & 1023 == 0 {
            self.budget.check_cancel()?;
        }
        Ok(())
    }

    /// The bytes of an absolute range, or `Truncated`.
    pub(crate) fn slice(&self, r: Range<u64>) -> Result<&'a [u8], Error> {
        self.src.get(r).ok_or(Error::Truncated)
    }
}

/// One box header.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct BoxHeader {
    pub(crate) kind: FourCc,
    #[allow(dead_code)] // the extended type; read by the later MP4 parsers
    pub(crate) uuid: Option<[u8; 16]>,
    pub(crate) start: u64,
    pub(crate) header_len: u8,
    pub(crate) end: u64,
}

impl BoxHeader {
    /// The absolute range of the bytes after the header.
    pub(crate) fn payload(&self) -> Range<u64> {
        self.start.saturating_add(u64::from(self.header_len))..self.end
    }
}

pub(crate) fn be32(b: &[u8], off: usize) -> Option<u32> {
    let s = b.get(off..off.checked_add(4)?)?;
    Some(u32::from_be_bytes(s.try_into().ok()?))
}

fn be64(b: &[u8], off: usize) -> Option<u64> {
    let s = b.get(off..off.checked_add(8)?)?;
    Some(u64::from_be_bytes(s.try_into().ok()?))
}

/// Decodes the header of the box at `start`. `buf` is the file bytes from `start` (at least the
/// first 32 or up to `parent_end`). `top` is true for a top-level box, where a box that runs past
/// the end is `Truncated` and not `Malformed`.
pub(crate) fn decode_header(
    buf: &[u8],
    start: u64,
    parent_end: u64,
    top: bool,
) -> Result<BoxHeader, Error> {
    let short = move || {
        if top {
            Error::Truncated
        } else {
            Error::Malformed("box size")
        }
    };
    let size32 = be32(buf, 0).ok_or_else(short)?;
    let kind = FourCc(
        buf.get(4..8)
            .and_then(|s| s.try_into().ok())
            .ok_or_else(short)?,
    );
    let mut hlen: usize = 8;
    let mut size = u64::from(size32);
    if size32 == 1 {
        size = be64(buf, 8).ok_or_else(short)?;
        hlen = 16;
    }
    let uuid = if kind.0 == *b"uuid" {
        let end = hlen.checked_add(16).ok_or_else(short)?;
        let u: [u8; 16] = buf
            .get(hlen..end)
            .and_then(|s| s.try_into().ok())
            .ok_or_else(short)?;
        hlen = end;
        Some(u)
    } else {
        None
    };
    if size32 == 0 {
        size = parent_end.saturating_sub(start);
    }
    if size < hlen as u64 {
        return Err(Error::Malformed("box size"));
    }
    let end = start
        .checked_add(size)
        .filter(|&e| e <= parent_end)
        .ok_or_else(short)?;
    Ok(BoxHeader {
        kind,
        uuid,
        start,
        header_len: hlen as u8,
        end,
    })
}

/// The children of a range of the input, one header at a time.
pub(crate) struct Boxes<'c, 'a> {
    ctx: &'c Ctx<'a>,
    pos: u64,
    end: u64,
    top: bool,
    counted: bool,
}

impl<'c, 'a> Boxes<'c, 'a> {
    /// Children of `range`, nested `depth` deep (top level = 0). `top` is true for the
    /// top-level walk of the file.
    pub(crate) fn new(
        ctx: &'c Ctx<'a>,
        range: Range<u64>,
        depth: usize,
        top: bool,
    ) -> Result<Self, Error> {
        if depth > ctx.limits.max_depth {
            return Err(Error::LimitExceeded(Limit::Depth));
        }
        if depth > ctx.max_depth.get() {
            ctx.max_depth.set(depth);
        }
        Ok(Self {
            ctx,
            pos: range.start,
            end: range.end,
            top,
            counted: true,
        })
    }

    /// The next child, `None` at the end of the range. Each header counts one box.
    pub(crate) fn next_box(&mut self) -> Result<Option<BoxHeader>, Error> {
        if self.pos >= self.end {
            return Ok(None);
        }
        if self.counted {
            self.ctx.tick()?;
        }
        let avail = self.end.saturating_sub(self.pos).min(32);
        let buf = self
            .ctx
            .src
            .get(self.pos..self.pos.saturating_add(avail))
            .ok_or(Error::Truncated)?;
        let h = decode_header(buf, self.pos, self.end, self.top)?;
        self.pos = h.end;
        Ok(Some(h))
    }
}

/// The number of children of `range` (a first pass that sizes a `Vec` before it is allocated).
/// Counts nothing against `max_boxes` except that it stops with `LimitExceeded(Boxes)` past it.
pub(crate) fn count_children(
    ctx: &Ctx<'_>,
    range: Range<u64>,
    depth: usize,
) -> Result<usize, Error> {
    let mut b = Boxes::new(ctx, range, depth, false)?;
    b.counted = false;
    let mut n: usize = 0;
    while b.next_box()?.is_some() {
        n = n.saturating_add(1);
        if n > ctx.limits.max_boxes {
            return Err(Error::LimitExceeded(Limit::Boxes));
        }
    }
    Ok(n)
}

/// Splits a FullBox payload into (version, flags, the bytes after the 4-byte header).
pub(crate) fn full_header<'a>(
    what: &'static str,
    payload: &'a [u8],
) -> Result<(u8, u32, &'a [u8]), Error> {
    let v = be32(payload, 0).ok_or(Error::Malformed(what))?;
    let rest = payload.get(4..).ok_or(Error::Malformed(what))?;
    Ok(((v >> 24) as u8, v & 0x00ff_ffff, rest))
}

/// A bounds-checked big-endian cursor over the bytes of one box. Every read that runs past the
/// end is `Malformed(what)`.
pub(crate) struct Cur<'a> {
    buf: &'a [u8],
    pos: usize,
    base: u64,
    what: &'static str,
}

impl<'a> Cur<'a> {
    /// `base` is the absolute file offset of `buf[0]`.
    pub(crate) fn new(what: &'static str, buf: &'a [u8], base: u64) -> Self {
        Self {
            buf,
            pos: 0,
            base,
            what,
        }
    }

    pub(crate) fn remaining(&self) -> usize {
        self.buf.len().saturating_sub(self.pos)
    }

    /// True when `n` entries of `each` bytes still fit in what is left (always for `each` = 0).
    pub(crate) fn fits(&self, n: usize, each: usize) -> bool {
        n.checked_mul(each).is_some_and(|t| t <= self.remaining())
    }

    /// The absolute file offset of the next byte.
    pub(crate) fn offset(&self) -> u64 {
        self.base.saturating_add(self.pos as u64)
    }

    pub(crate) fn take(&mut self, n: usize) -> Result<&'a [u8], Error> {
        let end = self.pos.checked_add(n).ok_or(Error::Malformed(self.what))?;
        let s = self
            .buf
            .get(self.pos..end)
            .ok_or(Error::Malformed(self.what))?;
        self.pos = end;
        Ok(s)
    }

    pub(crate) fn skip(&mut self, n: usize) -> Result<(), Error> {
        self.take(n).map(|_| ())
    }

    /// The next `n` bytes as an extent of the file.
    pub(crate) fn extent(&mut self, n: usize) -> Result<Extent, Error> {
        let offset = self.offset();
        self.skip(n)?;
        Ok(Extent {
            offset,
            length: n as u64,
        })
    }

    /// Everything that is left, as bytes and as an extent.
    pub(crate) fn rest(&mut self) -> (&'a [u8], Extent) {
        let ext = Extent {
            offset: self.offset(),
            length: self.remaining() as u64,
        };
        let s = self.buf.get(self.pos..).unwrap_or_default();
        self.pos = self.buf.len();
        (s, ext)
    }

    fn arr<const N: usize>(&mut self) -> Result<[u8; N], Error> {
        let s = self.take(N)?;
        <[u8; N]>::try_from(s).map_err(|_| Error::Malformed(self.what))
    }

    pub(crate) fn u8(&mut self) -> Result<u8, Error> {
        Ok(u8::from_be_bytes(self.arr()?))
    }

    pub(crate) fn u16(&mut self) -> Result<u16, Error> {
        Ok(u16::from_be_bytes(self.arr()?))
    }

    pub(crate) fn u32(&mut self) -> Result<u32, Error> {
        Ok(u32::from_be_bytes(self.arr()?))
    }

    pub(crate) fn i32(&mut self) -> Result<i32, Error> {
        Ok(i32::from_be_bytes(self.arr()?))
    }

    pub(crate) fn u64(&mut self) -> Result<u64, Error> {
        Ok(u64::from_be_bytes(self.arr()?))
    }

    pub(crate) fn fourcc(&mut self) -> Result<FourCc, Error> {
        Ok(FourCc(self.arr()?))
    }

    /// An unsigned integer of `n` bytes: 0 (value 0), 4 or 8.
    pub(crate) fn uint(&mut self, n: usize) -> Result<u64, Error> {
        match n {
            0 => Ok(0),
            4 => Ok(u64::from(self.u32()?)),
            8 => self.u64(),
            _ => Err(Error::Malformed(self.what)),
        }
    }

    /// A NUL-terminated string; without the NUL it runs to the end of the box.
    pub(crate) fn cstr(&mut self) -> &'a [u8] {
        let rest = self.buf.get(self.pos..).unwrap_or_default();
        match rest.iter().position(|&b| b == 0) {
            Some(i) => {
                self.pos = self.pos.saturating_add(i).saturating_add(1);
                rest.get(..i).unwrap_or_default()
            }
            None => {
                self.pos = self.buf.len();
                rest
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hdr(buf: &[u8], top: bool) -> Result<BoxHeader, Error> {
        decode_header(buf, 0, buf.len() as u64, top)
    }

    #[test]
    fn plain_box() {
        let h = hdr(&[0, 0, 0, 12, b'f', b'r', b'e', b'e', 1, 2, 3, 4], true).unwrap();
        assert_eq!(h.kind, FourCc(*b"free"));
        assert_eq!((h.header_len, h.end), (8, 12));
        assert_eq!(h.payload(), 8..12);
    }

    #[test]
    fn size_zero_runs_to_parent_end() {
        let h = hdr(&[0, 0, 0, 0, b'm', b'd', b'a', b't', 9, 9], true).unwrap();
        assert_eq!(h.end, 10);
    }

    #[test]
    fn largesize() {
        let mut b = vec![0, 0, 0, 1, b'm', b'd', b'a', b't'];
        b.extend_from_slice(&18u64.to_be_bytes());
        b.extend_from_slice(&[0, 0]);
        let h = hdr(&b, true).unwrap();
        assert_eq!((h.header_len, h.end), (16, 18));
    }

    #[test]
    fn uuid_box() {
        let mut b = vec![0, 0, 0, 24, b'u', b'u', b'i', b'd'];
        b.extend_from_slice(&[7; 16]);
        let h = hdr(&b, false).unwrap();
        assert_eq!(h.header_len, 24);
        assert_eq!(h.uuid, Some([7; 16]));
    }

    #[test]
    fn size_errors() {
        // Smaller than the header.
        assert_eq!(
            hdr(&[0, 0, 0, 4, b'f', b'r', b'e', b'e'], true),
            Err(Error::Malformed("box size"))
        );
        // Past the parent: Truncated at the top level, Malformed below.
        let b = [0, 0, 0, 20, b'f', b'r', b'e', b'e'];
        assert_eq!(hdr(&b, true), Err(Error::Truncated));
        assert_eq!(hdr(&b, false), Err(Error::Malformed("box size")));
    }

    #[test]
    fn cursor_reads_and_fails() {
        let mut c = Cur::new("x", &[0, 1, 0, 0, 0, 2, b'a', 0, b'b'], 100);
        assert_eq!(c.u16().unwrap(), 1);
        assert_eq!(c.u32().unwrap(), 2);
        assert_eq!(c.offset(), 106);
        assert_eq!(c.cstr(), b"a");
        assert_eq!(c.cstr(), b"b");
        assert_eq!(c.u8(), Err(Error::Malformed("x")));
    }
}
