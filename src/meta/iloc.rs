//! `iloc` versions 0 to 2, resolved to absolute file extents.

use crate::boxes::{Ctx, Cur, full_header};
use crate::budget::Budget;
use crate::error::ItemError;
use crate::{Error, Extent, Limit, Limits};
use std::ops::Range;

/// One `iloc` item as read: the extents are still relative to `base` and the construction
/// method.
#[derive(Debug)]
pub(crate) struct IlocItem {
    pub(crate) id: u32,
    pub(crate) method: u8,
    pub(crate) data_reference_index: u16,
    pub(crate) base_offset: u64,
    pub(crate) extents: Vec<Extent>,
}

fn size_ok(n: usize) -> bool {
    matches!(n, 0 | 4 | 8)
}

/// Parses the payload of an `iloc` box (starting at its version byte).
pub(crate) fn parse_bytes(
    bytes: &[u8],
    limits: &Limits,
    budget: &Budget<'_>,
) -> Result<Vec<IlocItem>, Error> {
    let (version, _, rest) = full_header("iloc", bytes)?;
    if version > 2 {
        return Err(Error::Malformed("iloc"));
    }
    let mut c = Cur::new("iloc", rest, 0);
    let b = c.u8()?;
    let offset_size = usize::from(b >> 4);
    let length_size = usize::from(b & 15);
    let b = c.u8()?;
    let base_size = usize::from(b >> 4);
    let index_size = if version >= 1 { usize::from(b & 15) } else { 0 };
    if !(size_ok(offset_size) && size_ok(length_size) && size_ok(base_size) && size_ok(index_size))
    {
        return Err(Error::Malformed("iloc"));
    }
    let id_size: usize = if version == 2 { 4 } else { 2 };
    let count = if version == 2 {
        usize::try_from(c.u32()?).map_err(|_| Error::Malformed("iloc"))?
    } else {
        usize::from(c.u16()?)
    };
    if count > limits.max_items {
        return Err(Error::LimitExceeded(Limit::Items));
    }
    // id, [construction method], data reference index, base offset, extent count
    let min_item = id_size
        .saturating_add(if version >= 1 { 2 } else { 0 })
        .saturating_add(2)
        .saturating_add(base_size)
        .saturating_add(2);
    if !c.fits(count, min_item) {
        return Err(Error::Malformed("iloc"));
    }
    let ext_bytes = index_size
        .saturating_add(offset_size)
        .saturating_add(length_size);
    let mut items = budget.vec::<IlocItem>(count)?;
    let mut total: usize = 0;
    for _ in 0..count {
        let id = if version == 2 {
            c.u32()?
        } else {
            u32::from(c.u16()?)
        };
        let method = if version >= 1 {
            (c.u16()? & 0xf) as u8
        } else {
            0
        };
        let data_reference_index = c.u16()?;
        let base_offset = c.uint(base_size)?;
        let n = usize::from(c.u16()?);
        total = total.saturating_add(n);
        if total > limits.max_extents {
            return Err(Error::LimitExceeded(Limit::Extents));
        }
        if !c.fits(n, ext_bytes) {
            return Err(Error::Malformed("iloc"));
        }
        let mut extents = budget.vec::<Extent>(n)?;
        for _ in 0..n {
            c.uint(index_size)?; // extent_index, not used
            let offset = c.uint(offset_size)?;
            let length = c.uint(length_size)?;
            extents.push(Extent { offset, length });
        }
        items.push(IlocItem {
            id,
            method,
            data_reference_index,
            base_offset,
            extents,
        });
    }
    Ok(items)
}

/// Parses an `iloc` box: `payload` is the range after its header.
pub(crate) fn parse(ctx: &Ctx<'_>, payload: Range<u64>) -> Result<Vec<IlocItem>, Error> {
    parse_bytes(ctx.slice(payload)?, ctx.limits, ctx.budget)
}

/// Turns the extents of an item into absolute file extents, or says why it cannot. `idat` is the
/// payload of the `idat` box (absolute), `file_len` the length of the file. Extent length 0 means
/// "to the end of the file" (method 0) or "to the end of idat" (method 1).
pub(crate) fn resolve(
    item: &mut IlocItem,
    idat: Option<Extent>,
    file_len: u64,
) -> Result<Vec<Extent>, ItemError> {
    if item.data_reference_index != 0 {
        return Err(ItemError::Unsupported("data reference"));
    }
    let (origin, limit) = match item.method {
        0 => (0, file_len),
        1 => {
            let d = idat.ok_or(ItemError::NoIdat)?;
            (d.offset, d.length)
        }
        2 => return Err(ItemError::Unsupported("construction method 2")),
        _ => return Err(ItemError::Unsupported("construction method")),
    };
    let mut extents = std::mem::take(&mut item.extents);
    for e in &mut extents {
        // `rel` is relative to the origin (the file or the idat payload).
        let rel = item
            .base_offset
            .checked_add(e.offset)
            .ok_or(ItemError::OutOfRange)?;
        let len = if e.length == 0 {
            limit.checked_sub(rel).ok_or(ItemError::OutOfRange)?
        } else {
            e.length
        };
        let rel_end = rel.checked_add(len).ok_or(ItemError::OutOfRange)?;
        if rel_end > limit {
            return Err(ItemError::OutOfRange);
        }
        *e = Extent {
            offset: origin.checked_add(rel).ok_or(ItemError::OutOfRange)?,
            length: len,
        };
    }
    Ok(extents)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::AtomicBool;

    fn parse_it(b: &[u8]) -> Result<Vec<IlocItem>, Error> {
        let limits = Limits::default();
        let cancel = AtomicBool::new(false);
        let budget = Budget::new(&limits, &cancel);
        parse_bytes(b, &limits, &budget)
    }

    #[test]
    fn v0_offset4_length4() {
        // 0x44: offset 4, length 4; 0x00: base 0. One item id 1, dref 0, one extent.
        let b = [
            0, 0, 0, 0, 0x44, 0x00, 0, 1, 0, 1, 0, 0, 0, 1, 0, 0, 0, 100, 0, 0, 0, 8,
        ];
        let items = parse_it(&b).unwrap();
        let it = items.first().unwrap();
        assert_eq!((it.id, it.method, it.base_offset), (1, 0, 0));
        assert_eq!(
            it.extents,
            vec![Extent {
                offset: 100,
                length: 8
            }]
        );
    }

    #[test]
    fn v1_idat_method_and_base_offset8() {
        // offset 4, length 4; base 8, index 0. item 5, method 1, base 16, one extent (2, 3).
        let mut b = vec![1, 0, 0, 0, 0x44, 0x80, 0, 1, 0, 5, 0, 1, 0, 0];
        b.extend_from_slice(&16u64.to_be_bytes());
        b.extend_from_slice(&[0, 1, 0, 0, 0, 2, 0, 0, 0, 3]);
        let mut items = parse_it(&b).unwrap();
        let it = items.first().unwrap();
        assert_eq!((it.id, it.method, it.base_offset), (5, 1, 16));
        let idat = Extent {
            offset: 1000,
            length: 30,
        };
        let r = resolve(items.first_mut().unwrap(), Some(idat), 5000).unwrap();
        assert_eq!(
            r,
            vec![Extent {
                offset: 1018,
                length: 3
            }]
        );
    }

    #[test]
    fn v2_u32_ids_and_extent_index() {
        // offset 4, length 4; base 0, index 4. item_count u32 = 1, item id u32 = 70000,
        // method 0, dref 0, base none, 1 extent: index 0, offset 9, length 2.
        let b = [
            2, 0, 0, 0, 0x44, 0x04, 0, 0, 0, 1, 0, 1, 0x11, 0x70, 0, 0, 0, 0, 0, 1, 0, 0, 0, 0, 0,
            0, 0, 9, 0, 0, 0, 2,
        ];
        let items = parse_it(&b).unwrap();
        let it = items.first().unwrap();
        assert_eq!(it.id, 70000);
        assert_eq!(
            it.extents,
            vec![Extent {
                offset: 9,
                length: 2
            }]
        );
    }

    #[test]
    fn rejects_bad_sizes_and_versions() {
        assert!(parse_it(&[0, 0, 0, 0, 0x34, 0, 0, 0]).is_err()); // offset size 3
        assert!(parse_it(&[3, 0, 0, 0, 0x44, 0, 0, 0]).is_err()); // version 3
        // claims one item but has no bytes for it
        assert!(parse_it(&[0, 0, 0, 0, 0x44, 0, 0, 1]).is_err());
    }

    #[test]
    fn resolve_rules() {
        let item = |method, dref, base, e: &[(u64, u64)]| IlocItem {
            id: 1,
            method,
            data_reference_index: dref,
            base_offset: base,
            extents: e
                .iter()
                .map(|&(offset, length)| Extent { offset, length })
                .collect(),
        };
        // method 0: base + offset; length 0 runs to the end of the file
        let r = resolve(&mut item(0, 0, 10, &[(5, 0)]), None, 100).unwrap();
        assert_eq!(
            r,
            vec![Extent {
                offset: 15,
                length: 85
            }]
        );
        // past the end of the file
        assert_eq!(
            resolve(&mut item(0, 0, 0, &[(90, 20)]), None, 100),
            Err(ItemError::OutOfRange)
        );
        // overflow
        assert_eq!(
            resolve(&mut item(0, 0, u64::MAX, &[(1, 1)]), None, 100),
            Err(ItemError::OutOfRange)
        );
        // method 1 without idat, method 2, data reference
        assert_eq!(
            resolve(&mut item(1, 0, 0, &[(0, 1)]), None, 100),
            Err(ItemError::NoIdat)
        );
        assert!(matches!(
            resolve(&mut item(2, 0, 0, &[(0, 1)]), None, 100),
            Err(ItemError::Unsupported(_))
        ));
        assert!(matches!(
            resolve(&mut item(0, 1, 0, &[(0, 1)]), None, 100),
            Err(ItemError::Unsupported(_))
        ));
        // method 1 past the end of idat
        let idat = Some(Extent {
            offset: 50,
            length: 10,
        });
        assert_eq!(
            resolve(&mut item(1, 0, 0, &[(8, 4)]), idat, 100),
            Err(ItemError::OutOfRange)
        );
    }
}
