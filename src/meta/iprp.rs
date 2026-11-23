//! `iprp`: `ipco` (the property boxes) and `ipma` (which item has which property).

use std::ops::Range;

use crate::boxes::{Boxes, Ctx, Cur, count_children, full_header};
use crate::budget::Budget;
use crate::props::{Property, parse_property};
use crate::{Error, Extent, FourCc, Limit, Limits};

/// One association of an item with a property.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Association {
    /// The 1-based index into [`Heif::properties`](crate::Heif::properties). Associations with
    /// index 0 ("no property") are dropped.
    pub index: u16,
    /// The `essential` bit.
    pub essential: bool,
}

/// The entries of one `ipma`: item id and its associations in order.
pub(crate) type Entries = Vec<(u32, Vec<Association>)>;

/// What `iprp` holds: the properties, the associations and every other child (type and whole-box
/// range).
pub(crate) type Parsed = (Vec<Property>, Entries, Vec<(FourCc, Extent)>);

/// Parses the payload of an `ipma` box (starting at its version byte). `assoc` counts the
/// associations of the whole call against `max_properties * 16`.
pub(crate) fn parse_ipma(
    bytes: &[u8],
    limits: &Limits,
    budget: &Budget<'_>,
    assoc: &mut usize,
) -> Result<Entries, Error> {
    let (version, flags, rest) = full_header("ipma", bytes)?;
    if version > 1 {
        return Err(Error::Malformed("ipma"));
    }
    let mut c = Cur::new("ipma", rest, 0);
    let count = usize::try_from(c.u32()?).map_err(|_| Error::Malformed("ipma"))?;
    if count > limits.max_items {
        return Err(Error::LimitExceeded(Limit::Items));
    }
    let id_size: usize = if version == 0 { 2 } else { 4 };
    if !c.fits(count, id_size.saturating_add(1)) {
        return Err(Error::Malformed("ipma"));
    }
    let max_assoc = limits.max_properties.saturating_mul(16);
    let mut out = budget.vec::<(u32, Vec<Association>)>(count)?;
    for _ in 0..count {
        let id = if version == 0 {
            u32::from(c.u16()?)
        } else {
            c.u32()?
        };
        let n = usize::from(c.u8()?);
        *assoc = assoc.saturating_add(n);
        if *assoc > max_assoc {
            return Err(Error::LimitExceeded(Limit::Properties));
        }
        let mut list = budget.vec::<Association>(n)?;
        for _ in 0..n {
            let (essential, index) = if flags & 1 != 0 {
                let v = c.u16()?;
                (v & 0x8000 != 0, v & 0x7fff)
            } else {
                let v = u16::from(c.u8()?);
                (v & 0x80 != 0, v & 0x7f)
            };
            if index != 0 {
                list.push(Association { index, essential });
            }
        }
        out.push((id, list));
    }
    Ok(out)
}

/// Parses an `ipco` box: `payload` is the range after its header, `depth` the depth of the box.
pub(crate) fn parse_ipco(
    ctx: &Ctx<'_>,
    payload: Range<u64>,
    depth: usize,
) -> Result<Vec<Property>, Error> {
    let n = count_children(ctx, payload.clone(), depth.saturating_add(1))?;
    if n > ctx.limits.max_properties {
        return Err(Error::LimitExceeded(Limit::Properties));
    }
    let mut props = ctx.budget.vec::<Property>(n)?;
    let mut boxes = Boxes::new(ctx, payload, depth.saturating_add(1), false)?;
    while let Some(h) = boxes.next_box()? {
        props.push(parse_property(ctx, &h)?);
    }
    Ok(props)
}

/// Parses an `iprp` box: the properties, the associations of every item (unsorted, as in the
/// file; an item that appears twice is `Malformed("ipma")`) and every other child kept raw.
pub(crate) fn parse(ctx: &Ctx<'_>, payload: Range<u64>, depth: usize) -> Result<Parsed, Error> {
    let mut props: Option<Vec<Property>> = None;
    let mut entries: Entries = Vec::new();
    let mut other: Vec<(FourCc, Extent)> = Vec::new();
    let mut ipmas: usize = 0;
    let mut assoc: usize = 0;
    let mut boxes = Boxes::new(ctx, payload, depth.saturating_add(1), false)?;
    while let Some(h) = boxes.next_box()? {
        match &h.kind.0 {
            b"ipco" => {
                if props.is_some() {
                    return Err(Error::Malformed("ipco"));
                }
                props = Some(parse_ipco(ctx, h.payload(), depth.saturating_add(1))?);
            }
            b"ipma" => {
                // One per (version, flags) pair at most.
                ipmas = ipmas.saturating_add(1);
                if ipmas > 4 {
                    return Err(Error::Malformed("ipma"));
                }
                let mut part =
                    parse_ipma(ctx.slice(h.payload())?, ctx.limits, ctx.budget, &mut assoc)?;
                let total = entries.len().saturating_add(part.len());
                if total > ctx.limits.max_items {
                    return Err(Error::LimitExceeded(Limit::Items));
                }
                if entries.is_empty() {
                    entries = part;
                } else {
                    let mut merged = ctx.budget.vec::<(u32, Vec<Association>)>(total)?;
                    merged.append(&mut entries);
                    merged.append(&mut part);
                    entries = merged;
                }
            }
            _ => super::keep(ctx, &mut other, &h)?,
        }
    }
    Ok((props.unwrap_or_default(), entries, other))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::AtomicBool;

    fn run(b: &[u8]) -> Result<Entries, Error> {
        let limits = Limits::default();
        let cancel = AtomicBool::new(false);
        let budget = Budget::new(&limits, &cancel);
        parse_ipma(b, &limits, &budget, &mut 0)
    }

    #[test]
    fn ipma_7_bit_indexes() {
        // v0, flags 0, 1 entry: item 3, two associations: 0x81 (essential, 1) and 0x02.
        let e = run(&[0, 0, 0, 0, 0, 0, 0, 1, 0, 3, 2, 0x81, 0x02]).unwrap();
        assert_eq!(
            e,
            vec![(
                3,
                vec![
                    Association {
                        index: 1,
                        essential: true
                    },
                    Association {
                        index: 2,
                        essential: false
                    }
                ]
            )]
        );
    }

    #[test]
    fn ipma_15_bit_indexes_and_u32_ids() {
        // v1, flags 1: item 70000, associations 0x8123 (essential, 0x123) and 0x0000 (dropped).
        let e = run(&[
            1, 0, 0, 1, 0, 0, 0, 1, 0, 1, 0x11, 0x70, 2, 0x81, 0x23, 0x00, 0x00,
        ])
        .unwrap();
        assert_eq!(
            e,
            vec![(
                70000,
                vec![Association {
                    index: 0x123,
                    essential: true
                }]
            )]
        );
    }

    #[test]
    fn ipma_rejects_short_and_over_limit() {
        assert!(run(&[0, 0, 0, 0, 0, 0, 0, 1]).is_err());
        assert!(run(&[0, 0, 0, 0, 0, 0, 0, 1, 0, 3, 2, 0x81]).is_err());
        let limits = Limits {
            max_properties: 0,
            ..Limits::default()
        };
        let cancel = AtomicBool::new(false);
        let budget = Budget::new(&limits, &cancel);
        let b = [0, 0, 0, 0, 0, 0, 0, 1, 0, 3, 1, 0x01];
        assert_eq!(
            parse_ipma(&b, &limits, &budget, &mut 0),
            Err(Error::LimitExceeded(Limit::Properties))
        );
    }
}
