//! The `meta` box: `hdlr`, `pitm`, `iinf`, `iloc`, `idat`, `iref`, `iprp` and `grpl` assembled.

pub(crate) mod grpl;
pub(crate) mod iinf;
pub(crate) mod iloc;
pub(crate) mod iprp;
pub(crate) mod iref;

pub use grpl::EntityGroup;
pub use iprp::Association;
pub use iref::Reference;

use crate::boxes::{BoxHeader, Boxes, Ctx, Cur, full_header};
use crate::props::Property;
use crate::{Error, Extent};

use iinf::ItemInfo;
use iloc::IlocItem;

/// Everything `meta` holds, before the item graph is built from it.
#[derive(Default)]
pub(crate) struct Meta {
    /// `pitm` (0 when absent).
    pub(crate) primary: u32,
    pub(crate) items: Vec<ItemInfo>,
    pub(crate) iloc: Vec<IlocItem>,
    /// The payload of `idat`, absolute.
    pub(crate) idat: Option<Extent>,
    pub(crate) refs: Vec<Reference>,
    pub(crate) props: Vec<Property>,
    /// Associations per item id, in file order.
    pub(crate) assoc: iprp::Entries,
    pub(crate) groups: Vec<EntityGroup>,
}

fn once<T>(slot: &mut Option<T>, v: T, what: &'static str) -> Result<(), Error> {
    if slot.is_some() {
        return Err(Error::Malformed(what));
    }
    *slot = Some(v);
    Ok(())
}

/// Parses the top-level `meta` box `hdr` (depth 0). It is a FullBox of version 0 whose first
/// child is an `hdlr` with handler `pict`; each of the other boxes may appear once.
pub(crate) fn parse(ctx: &Ctx<'_>, hdr: &BoxHeader) -> Result<Meta, Error> {
    let payload = hdr.payload();
    let (version, _, _) = full_header("meta", ctx.slice(payload.clone())?)?;
    if version != 0 {
        return Err(Error::Malformed("meta"));
    }
    let kids = payload.start.saturating_add(4)..payload.end;
    let mut boxes = Boxes::new(ctx, kids, 1, false)?;
    let mut first = true;
    let mut primary: Option<u32> = None;
    let mut items: Option<Vec<ItemInfo>> = None;
    let mut iloc: Option<Vec<IlocItem>> = None;
    let mut idat: Option<Extent> = None;
    let mut refs: Option<Vec<Reference>> = None;
    let mut iprp: Option<(Vec<Property>, iprp::Entries)> = None;
    let mut groups: Option<Vec<EntityGroup>> = None;
    let mut ids: usize = 0;
    while let Some(h) = boxes.next_box()? {
        let p = h.payload();
        if first {
            first = false;
            if h.kind.0 != *b"hdlr" {
                return Err(Error::Malformed("hdlr"));
            }
            let (_, _, rest) = full_header("hdlr", ctx.slice(p)?)?;
            let mut c = Cur::new("hdlr", rest, 0);
            c.skip(4)?; // pre_defined
            if c.fourcc()?.0 != *b"pict" {
                return Err(Error::Malformed("hdlr"));
            }
            continue;
        }
        match &h.kind.0 {
            b"pitm" => {
                let (v, _, rest) = full_header("pitm", ctx.slice(p)?)?;
                let mut c = Cur::new("pitm", rest, 0);
                let id = match v {
                    0 => u32::from(c.u16()?),
                    1 => c.u32()?,
                    _ => return Err(Error::Malformed("pitm")),
                };
                once(&mut primary, id, "pitm")?;
            }
            b"iinf" => once(&mut items, iinf::parse(ctx, p, 1)?, "iinf")?,
            b"iloc" => once(&mut iloc, iloc::parse(ctx, p)?, "iloc")?,
            b"idat" => once(
                &mut idat,
                Extent {
                    offset: p.start,
                    length: p.end.saturating_sub(p.start),
                },
                "idat",
            )?,
            b"iref" => once(&mut refs, iref::parse(ctx, p, 1, &mut ids)?, "iref")?,
            b"iprp" => once(&mut iprp, iprp::parse(ctx, p, 1)?, "iprp")?,
            b"grpl" => once(&mut groups, grpl::parse(ctx, p, 1, &mut ids)?, "grpl")?,
            _ => {}
        }
    }
    if first {
        return Err(Error::Malformed("hdlr"));
    }
    let (props, assoc) = iprp.unwrap_or_default();
    Ok(Meta {
        primary: primary.unwrap_or(0),
        items: items.unwrap_or_default(),
        iloc: iloc.unwrap_or_default(),
        idat,
        refs: refs.unwrap_or_default(),
        props,
        assoc,
        groups: groups.unwrap_or_default(),
    })
}
