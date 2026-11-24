//! The sample-table boxes of `stbl`: only their headers are read here. The entries stay in the
//! file; `walk.rs` reads them through a small window.

use crate::boxes::{be32, full_header};
use crate::{Error, Extent};

/// A table of fixed-size entries: where the entries start and how many there are.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub(crate) struct Table {
    pub(crate) entries: Extent,
    pub(crate) count: u64,
}

/// How `stsz` or `stz2` gives the size of a sample.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum SizeKind {
    /// `stsz` with a non-zero `sample_size`: every sample has it.
    Constant(u32),
    /// `stsz` with one u32 per sample.
    Stsz,
    /// `stz2` with this field size in bits (4, 8 or 16).
    Stz2(u8),
}

/// `stsz` or `stz2`: `table.count` is the sample count.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Sizes {
    pub(crate) kind: SizeKind,
    pub(crate) table: Table,
}

/// `stco` or `co64`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Chunks {
    pub(crate) wide: bool,
    pub(crate) table: Table,
}

/// The tables of one track, as ranges of the file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Stbl {
    pub(crate) stts: Table,
    pub(crate) ctts: Option<Table>,
    pub(crate) stss: Option<Table>,
    pub(crate) stsc: Table,
    pub(crate) sizes: Sizes,
    pub(crate) chunks: Chunks,
    /// Per `dref` entry: true when its media data is in this file (`flags & 1`). Empty without
    /// a `dref`.
    pub(crate) drefs: Vec<bool>,
}

/// A table box with a FullBox version in `versions`, an `entry_count` and `each` bytes per
/// entry. `payload` is the box payload, `start` its absolute offset.
pub(crate) fn table(
    what: &'static str,
    payload: &[u8],
    start: u64,
    each: u64,
    versions: &[u8],
) -> Result<Table, Error> {
    let (v, _, rest) = full_header(what, payload)?;
    if !versions.contains(&v) {
        return Err(Error::Malformed(what));
    }
    let count = u64::from(be32(rest, 0).ok_or(Error::Malformed(what))?);
    let avail = (rest.len() as u64).saturating_sub(4);
    let length = count
        .checked_mul(each)
        .filter(|&l| l <= avail)
        .ok_or(Error::Malformed(what))?;
    Ok(Table {
        entries: Extent {
            offset: start.saturating_add(8),
            length,
        },
        count,
    })
}

/// `stsz`: `sample_size`, `sample_count`, then one u32 per sample when `sample_size` is 0.
pub(crate) fn stsz(payload: &[u8], start: u64) -> Result<Sizes, Error> {
    const BAD: Error = Error::Malformed("stsz");
    let (v, _, rest) = full_header("stsz", payload)?;
    if v != 0 {
        return Err(BAD);
    }
    let size = be32(rest, 0).ok_or(BAD)?;
    let count = u64::from(be32(rest, 4).ok_or(BAD)?);
    let offset = start.saturating_add(12);
    if size != 0 {
        return Ok(Sizes {
            kind: SizeKind::Constant(size),
            table: Table {
                entries: Extent { offset, length: 0 },
                count,
            },
        });
    }
    let avail = (rest.len() as u64).saturating_sub(8);
    let length = count.checked_mul(4).filter(|&l| l <= avail).ok_or(BAD)?;
    Ok(Sizes {
        kind: SizeKind::Stsz,
        table: Table {
            entries: Extent { offset, length },
            count,
        },
    })
}

/// `stz2`: three reserved bytes, `field_size`, `sample_count`, then packed sizes.
pub(crate) fn stz2(payload: &[u8], start: u64) -> Result<Sizes, Error> {
    const BAD: Error = Error::Malformed("stz2");
    let (v, _, rest) = full_header("stz2", payload)?;
    if v != 0 {
        return Err(BAD);
    }
    let field = *rest.get(3).ok_or(BAD)?;
    if !matches!(field, 4 | 8 | 16) {
        return Err(BAD);
    }
    let count = u64::from(be32(rest, 4).ok_or(BAD)?);
    let avail = (rest.len() as u64).saturating_sub(8);
    let length = count
        .checked_mul(u64::from(field))
        .map(|bits| bits.div_ceil(8))
        .filter(|&l| l <= avail)
        .ok_or(BAD)?;
    Ok(Sizes {
        kind: SizeKind::Stz2(field),
        table: Table {
            entries: Extent {
                offset: start.saturating_add(12),
                length,
            },
            count,
        },
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn counts_are_checked_against_the_payload() {
        // v0, count 2, two 8-byte entries.
        let mut b = vec![0, 0, 0, 0, 0, 0, 0, 2];
        b.extend_from_slice(&[0; 16]);
        let t = table("stts", &b, 100, 8, &[0]).unwrap();
        assert_eq!((t.count, t.entries.offset, t.entries.length), (2, 108, 16));
        assert_eq!(
            table("stts", &b[..23], 100, 8, &[0]),
            Err(Error::Malformed("stts"))
        );
        // A count of 2^32 - 1 cannot fit.
        let huge = [0, 0, 0, 0, 0xff, 0xff, 0xff, 0xff];
        assert_eq!(
            table("stts", &huge, 0, 8, &[0]),
            Err(Error::Malformed("stts"))
        );
        // Version 1 only where listed.
        let mut v1 = b.clone();
        v1[0] = 1;
        assert!(table("ctts", &v1, 0, 8, &[0, 1]).is_ok());
        assert!(table("stts", &v1, 0, 8, &[0]).is_err());
    }

    #[test]
    fn stsz_and_stz2() {
        let constant = stsz(&[0, 0, 0, 0, 0, 0, 0, 7, 0, 0, 0, 9], 0).unwrap();
        assert_eq!(constant.kind, SizeKind::Constant(7));
        assert_eq!(constant.table.count, 9);
        let var = stsz(&[0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 1, 0, 0, 0, 5], 0).unwrap();
        assert_eq!(var.kind, SizeKind::Stsz);
        assert_eq!(var.table.entries.length, 4);
        assert!(stsz(&[0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 2, 0, 0, 0, 5], 0).is_err());
        // stz2, 4-bit fields, 3 samples: 2 bytes.
        let z = stz2(&[0, 0, 0, 0, 0, 0, 0, 4, 0, 0, 0, 3, 0x12, 0x30], 0).unwrap();
        assert_eq!(z.kind, SizeKind::Stz2(4));
        assert_eq!(z.table.entries.length, 2);
        assert!(stz2(&[0, 0, 0, 0, 0, 0, 0, 5, 0, 0, 0, 3, 0x12, 0x30], 0).is_err());
        assert!(stz2(&[0, 0, 0, 0, 0, 0, 0, 4, 0, 0, 0, 5, 0x12, 0x30], 0).is_err());
    }
}
