//! `iref` versions 0 and 1.

use std::ops::Range;

use crate::boxes::{Boxes, Ctx, Cur, count_children, full_header};
use crate::{Error, FourCc, Limit};

/// One reference box: `from` refers to every id in `to` with type `kind` (`dimg`, `thmb`,
/// `cdsc`, `auxl`, `prem`, ...).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Reference {
    /// The reference type.
    pub kind: FourCc,
    /// The item that refers.
    pub from: u32,
    /// The items referred to, in file order.
    pub to: Vec<u32>,
}

/// Parses an `iref` box: `payload` is the range after its header, `depth` the depth of the box.
/// `ids` counts the `to` ids of this and the other reference boxes against `max_references`.
pub(crate) fn parse(
    ctx: &Ctx<'_>,
    payload: Range<u64>,
    depth: usize,
    ids: &mut usize,
) -> Result<Vec<Reference>, Error> {
    let (version, _, _) = full_header("iref", ctx.slice(payload.clone())?)?;
    if version > 1 {
        return Err(Error::Malformed("iref"));
    }
    let kids = payload.start.saturating_add(4)..payload.end;
    let n = count_children(ctx, kids.clone(), depth.saturating_add(1))?;
    let mut out = ctx.budget.vec::<Reference>(n)?;
    let mut boxes = Boxes::new(ctx, kids, depth.saturating_add(1), false)?;
    while let Some(h) = boxes.next_box()? {
        let r = h.payload();
        let mut c = Cur::new("iref", ctx.slice(r)?, 0);
        let (from, id_size) = if version == 0 {
            (u32::from(c.u16()?), 2)
        } else {
            (c.u32()?, 4)
        };
        let count = usize::from(c.u16()?);
        if !c.fits(count, id_size) {
            return Err(Error::Malformed("iref"));
        }
        *ids = ids.saturating_add(count);
        if *ids > ctx.limits.max_references {
            return Err(Error::LimitExceeded(Limit::References));
        }
        let mut to = ctx.budget.vec::<u32>(count)?;
        for _ in 0..count {
            to.push(if version == 0 {
                u32::from(c.u16()?)
            } else {
                c.u32()?
            });
        }
        out.push(Reference {
            kind: h.kind,
            from,
            to,
        });
    }
    Ok(out)
}
