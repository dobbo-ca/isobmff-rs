//! `iloc` versions 0 to 2, resolved to absolute file extents.

use super::iref::Reference;
use crate::boxes::{Ctx, Cur, full_header};
use crate::budget::Budget;
use crate::error::ItemError;
use crate::graph::span;
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
    /// Construction method 2 only (empty otherwise): the 1-based `extent_index` of every extent,
    /// which picks the source item among the `to` ids of the `iloc` reference. 1 when the
    /// `index_size` is 0.
    pub(crate) extent_index: Vec<u64>,
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
        let mut extent_index = if method == 2 {
            budget.vec::<u64>(n)?
        } else {
            Vec::new()
        };
        for _ in 0..n {
            let index = c.uint(index_size)?;
            if method == 2 {
                extent_index.push(if index_size == 0 { 1 } else { index });
            }
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
            extent_index,
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
/// "to the end of the file" (method 0) or "to the end of idat" (method 1). Method 2 needs the other
/// items: [`resolve_all`] handles it and this gives `Unsupported`.
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

/// The most method-2 items followed in a row (an item whose source is a method-2 item whose
/// source is ...). A longer chain, or a cycle, is `Unsupported`.
const MAX_HOPS: usize = 8;

/// The data of one item, or why it cannot be had.
pub(crate) type Resolved = Result<Vec<Extent>, ItemError>;

/// How the data of one method-2 item came out.
enum Fill {
    Done,
    Item(ItemError),
    /// A source chain longer than [`MAX_HOPS`] (or a cycle) was found below this item.
    TooDeep,
}

/// Pushes `e`, growing `v` by doubling with the whole new capacity charged first.
fn push_charged(budget: &Budget<'_>, v: &mut Vec<Extent>, e: Extent) -> Result<(), Error> {
    if v.len() == v.capacity() {
        let add = v.capacity().max(4);
        budget.charge(add.saturating_mul(size_of::<Extent>()))?;
        v.try_reserve_exact(add).map_err(|_| Error::OutOfMemory)?;
    }
    v.push(e);
    Ok(())
}

/// Counts one unit of work (an extent of a method-2 item or a source extent visited) against
/// `max_extents`.
fn tick(steps: &mut usize, max: usize) -> Result<(), Error> {
    *steps = steps.saturating_add(1);
    if *steps > max {
        return Err(Error::LimitExceeded(Limit::Extents));
    }
    Ok(())
}

/// Resolves the method-2 items of one `iloc`. `locs` is sorted by id; `out` has one slot per item
/// (`None` until it is resolved).
struct Solver<'a, 'c> {
    ctx: &'a Ctx<'c>,
    locs: &'a [IlocItem],
    refs: &'a [Reference],
    /// (`from` id, position in `refs`), sorted.
    fwd: &'a [(u32, u32)],
    out: Vec<Option<Resolved>>,
    /// Per item: the length of its method-2 source chain (0 for methods 0 and 1).
    hops: Vec<u8>,
    /// Per item: being solved right now (a source that is `busy` is a cycle).
    busy: Vec<bool>,
    steps: usize,
}

impl Solver<'_, '_> {
    /// Makes sure `out[pos]` is set. `false`: the recursion reached [`MAX_HOPS`] below an unsolved
    /// item and nothing was stored. Only the item at depth 0 stores that failure: the chain below
    /// it is then longer than [`MAX_HOPS`] whatever the order the items are visited in. Chains
    /// that are fully solved are judged by `hops`.
    fn solve(&mut self, pos: usize, depth: usize) -> Result<bool, Error> {
        if matches!(self.out.get(pos), Some(Some(_))) {
            return Ok(true);
        }
        if depth >= MAX_HOPS {
            return Ok(false);
        }
        self.ctx.budget.check_cancel()?;
        let mut acc = Vec::new();
        let mut hop = 0u8;
        if let Some(b) = self.busy.get_mut(pos) {
            *b = true;
        }
        let filled = self.fill(pos, depth, &mut acc, &mut hop)?;
        if let Some(b) = self.busy.get_mut(pos) {
            *b = false;
        }
        let spent = acc.capacity().saturating_mul(size_of::<Extent>());
        let hop = hop.saturating_add(1);
        let r = match filled {
            Fill::Done if usize::from(hop) > MAX_HOPS => {
                self.ctx.budget.release(spent);
                Err(ItemError::Unsupported("construction method 2"))
            }
            Fill::Done => {
                if let Some(h) = self.hops.get_mut(pos) {
                    *h = hop;
                }
                Ok(acc)
            }
            Fill::Item(e) => {
                self.ctx.budget.release(spent);
                Err(e)
            }
            Fill::TooDeep => {
                self.ctx.budget.release(spent);
                if depth > 0 {
                    return Ok(false);
                }
                Err(ItemError::Unsupported("construction method 2"))
            }
        };
        if let Some(slot) = self.out.get_mut(pos) {
            *slot = Some(r);
        }
        Ok(true)
    }

    /// Maps every extent of the method-2 item at `pos` onto the file extents of its source items.
    fn fill(
        &mut self,
        pos: usize,
        depth: usize,
        acc: &mut Vec<Extent>,
        hop: &mut u8,
    ) -> Result<Fill, Error> {
        let ctx = self.ctx;
        let max = ctx.limits.max_extents;
        let (locs, refs, fwd) = (self.locs, self.refs, self.fwd);
        let Some(it) = locs.get(pos) else {
            return Ok(Fill::Item(ItemError::NoLocation));
        };
        if it.data_reference_index != 0 {
            return Ok(Fill::Item(ItemError::Unsupported("data reference")));
        }
        // Exactly one `iloc` reference from this item; each reference scanned counts as work.
        let mut found = None;
        for &(_, p) in span(fwd, it.id) {
            tick(&mut self.steps, max)?;
            let Some(r) = usize::try_from(p).ok().and_then(|p| refs.get(p)) else {
                continue;
            };
            if r.kind.0 == *b"iloc" {
                if found.is_some() {
                    return Ok(Fill::Item(ItemError::Unsupported("construction method 2")));
                }
                found = Some(r);
            }
        }
        let Some(r) = found else {
            return Ok(Fill::Item(ItemError::Unsupported("construction method 2")));
        };
        for (e, &index) in it.extents.iter().zip(&it.extent_index) {
            tick(&mut self.steps, max)?;
            let Some(&source) = index
                .checked_sub(1)
                .and_then(|i| usize::try_from(i).ok())
                .and_then(|i| r.to.get(i))
            else {
                return Ok(Fill::Item(ItemError::OutOfRange));
            };
            let Ok(sp) = locs.binary_search_by_key(&source, |l| l.id) else {
                return Ok(Fill::Item(ItemError::NoLocation));
            };
            if self.busy.get(sp) == Some(&true) {
                return Ok(Fill::Item(ItemError::Unsupported("construction method 2")));
            }
            if !matches!(self.out.get(sp), Some(Some(_)))
                && !self.solve(sp, depth.saturating_add(1))?
            {
                return Ok(Fill::TooDeep);
            }
            *hop = (*hop).max(self.hops.get(sp).copied().unwrap_or(0));
            let src = match self.out.get(sp) {
                Some(Some(Ok(s))) => s,
                Some(Some(Err(err))) => return Ok(Fill::Item(*err)),
                _ => return Ok(Fill::Item(ItemError::NoLocation)),
            };
            // The range [base_offset + offset, + length) of the source data; length 0 = to its end.
            let Some(start) = it.base_offset.checked_add(e.offset) else {
                return Ok(Fill::Item(ItemError::OutOfRange));
            };
            let to_end = e.length == 0;
            let mut skip = start;
            let mut left = if to_end { u64::MAX } else { e.length };
            for s in src {
                if left == 0 {
                    break;
                }
                tick(&mut self.steps, max)?;
                if skip >= s.length {
                    skip = skip.saturating_sub(s.length);
                    continue;
                }
                let take = s.length.saturating_sub(skip).min(left);
                let Some(offset) = s.offset.checked_add(skip) else {
                    return Ok(Fill::Item(ItemError::OutOfRange));
                };
                push_charged(
                    ctx.budget,
                    acc,
                    Extent {
                        offset,
                        length: take,
                    },
                )?;
                skip = 0;
                left = left.saturating_sub(take);
            }
            if skip > 0 || (!to_end && left > 0) {
                return Ok(Fill::Item(ItemError::OutOfRange));
            }
        }
        Ok(Fill::Done)
    }
}

/// Resolves every item of `locs` (sorted by id) to absolute file extents: methods 0 and 1 as
/// [`resolve`], then method 2 (item offset): the source item of an extent is
/// `to[extent_index - 1]` of the one `iloc` reference (`refs`, with `fwd` its sorted
/// `(from, position)` index) whose `from` is the item. The result has one slot per item, `None`
/// for none (the caller takes each slot once).
pub(crate) fn resolve_all(
    locs: &mut [IlocItem],
    refs: &[Reference],
    fwd: &[(u32, u32)],
    idat: Option<Extent>,
    ctx: &Ctx<'_>,
) -> Result<Vec<Option<Resolved>>, Error> {
    let mut out = ctx.budget.vec::<Option<Resolved>>(locs.len())?;
    for it in locs.iter_mut() {
        out.push(if it.method == 2 {
            None
        } else {
            Some(resolve(it, idat, ctx.src.file_len))
        });
    }
    let locs: &[IlocItem] = locs;
    let mut hops = ctx.budget.vec::<u8>(locs.len())?;
    let mut busy = ctx.budget.vec::<bool>(locs.len())?;
    for _ in locs {
        hops.push(0);
        busy.push(false);
    }
    let mut solver = Solver {
        ctx,
        locs,
        refs,
        fwd,
        out,
        hops,
        busy,
        steps: 0,
    };
    for pos in 0..locs.len() {
        solver.solve(pos, 0)?;
    }
    Ok(solver.out)
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
    fn method_2_reference_scan_counts_against_max_extents() {
        use crate::FourCc;
        use crate::boxes::Src;
        let limits = Limits {
            max_extents: 50,
            ..Limits::default()
        };
        let cancel = AtomicBool::new(false);
        let budget = Budget::new(&limits, &cancel);
        let src = Src {
            bytes: &[],
            base: 0,
            file_len: 0,
        };
        let ctx = Ctx::new(&src, &budget, &limits);
        let mut locs = vec![IlocItem {
            id: 1,
            method: 2,
            data_reference_index: 0,
            base_offset: 0,
            extents: vec![Extent {
                offset: 0,
                length: 1,
            }],
            extent_index: vec![1],
        }];
        // 1000 empty `cdsc` references from item 1: the scan alone exceeds the work cap.
        let refs: Vec<Reference> = (0..1000)
            .map(|_| Reference {
                kind: FourCc(*b"cdsc"),
                from: 1,
                to: Vec::new(),
            })
            .collect();
        let fwd: Vec<(u32, u32)> = (0..1000u32).map(|i| (1, i)).collect();
        assert!(matches!(
            resolve_all(&mut locs, &refs, &fwd, None, &ctx),
            Err(Error::LimitExceeded(Limit::Extents))
        ));
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
    fn method_2_keeps_extent_index() {
        // v1, offset 4, length 4, base 0, index 4. One item (id 7, method 2, dref 0, no base) with
        // two extents: (index 2, offset 1, length 3) and (index 1, offset 0, length 0).
        let b = [
            1, 0, 0, 0, 0x44, 0x04, 0, 1, 0, 7, 0, 2, 0, 0, 0, 2, 0, 0, 0, 2, 0, 0, 0, 1, 0, 0, 0,
            3, 0, 0, 0, 1, 0, 0, 0, 0, 0, 0, 0, 0,
        ];
        let items = parse_it(&b).unwrap();
        let it = items.first().unwrap();
        assert_eq!(
            (it.id, it.method, it.extent_index.clone()),
            (7, 2, vec![2, 1])
        );
        // index_size 0: every extent is index 1.
        let b = [
            1, 0, 0, 0, 0x44, 0x00, 0, 1, 0, 7, 0, 2, 0, 0, 0, 1, 0, 0, 0, 0, 0, 0, 0, 3,
        ];
        let items = parse_it(&b).unwrap();
        assert_eq!(items.first().unwrap().extent_index, vec![1]);
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
            extent_index: Vec::new(),
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
