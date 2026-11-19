//! `grpl`: entity groups.

use std::ops::Range;

use crate::boxes::{Boxes, Ctx, Cur, count_children, full_header};
use crate::{Error, FourCc, Limit};

/// An entity group: `altr` (alternatives, the first is preferred), `ster`, ...
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EntityGroup {
    /// The grouping type.
    pub kind: FourCc,
    /// `group_id`.
    pub id: u32,
    /// The item ids (or track ids) in the group, in file order.
    pub entities: Vec<u32>,
}

/// Parses a `grpl` box: `payload` is the range after its header, `depth` the depth of the box.
/// `ids` counts the entity ids against `max_references`, together with the `iref` ids.
pub(crate) fn parse(
    ctx: &Ctx<'_>,
    payload: Range<u64>,
    depth: usize,
    ids: &mut usize,
) -> Result<Vec<EntityGroup>, Error> {
    let n = count_children(ctx, payload.clone(), depth.saturating_add(1))?;
    let mut out = ctx.budget.vec::<EntityGroup>(n)?;
    let mut boxes = Boxes::new(ctx, payload, depth.saturating_add(1), false)?;
    while let Some(h) = boxes.next_box()? {
        let (_, _, rest) = full_header("grpl", ctx.slice(h.payload())?)?;
        let mut c = Cur::new("grpl", rest, 0);
        let id = c.u32()?;
        let count = usize::try_from(c.u32()?).map_err(|_| Error::Malformed("grpl"))?;
        if c.remaining() / 4 < count {
            return Err(Error::Malformed("grpl"));
        }
        *ids = ids.saturating_add(count);
        if *ids > ctx.limits.max_references {
            return Err(Error::LimitExceeded(Limit::References));
        }
        let mut entities = ctx.budget.vec::<u32>(count)?;
        for _ in 0..count {
            entities.push(c.u32()?);
        }
        out.push(EntityGroup {
            kind: h.kind,
            id,
            entities,
        });
    }
    Ok(out)
}
