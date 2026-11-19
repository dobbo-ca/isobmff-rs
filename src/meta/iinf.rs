//! `iinf` and `infe` (versions 0 to 3).

use std::ops::Range;

use crate::boxes::{Boxes, Ctx, Cur, full_header};
use crate::budget::Budget;
use crate::{Error, FourCc, Limit};

/// What one `infe` says about an item.
#[derive(Debug, Clone)]
pub(crate) struct ItemInfo {
    pub(crate) id: u32,
    pub(crate) item_type: FourCc,
    pub(crate) name: String,
    pub(crate) content_type: String,
    pub(crate) content_encoding: String,
    pub(crate) uri_type: String,
    pub(crate) hidden: bool,
    pub(crate) protection_index: u16,
}

/// Parses one `infe` payload (starting at its version byte).
pub(crate) fn parse_infe(bytes: &[u8], budget: &Budget<'_>) -> Result<ItemInfo, Error> {
    let (version, flags, rest) = full_header("infe", bytes)?;
    let mut c = Cur::new("infe", rest, 0);
    let mut info = ItemInfo {
        id: 0,
        item_type: FourCc([0; 4]),
        name: String::new(),
        content_type: String::new(),
        content_encoding: String::new(),
        uri_type: String::new(),
        hidden: flags & 1 != 0,
        protection_index: 0,
    };
    match version {
        0 | 1 => {
            info.id = u32::from(c.u16()?);
            info.protection_index = c.u16()?;
            info.name = budget.string(c.cstr())?;
            info.content_type = budget.string(c.cstr())?;
            info.content_encoding = budget.string(c.cstr())?;
        }
        2 | 3 => {
            info.id = if version == 2 {
                u32::from(c.u16()?)
            } else {
                c.u32()?
            };
            info.protection_index = c.u16()?;
            info.item_type = c.fourcc()?;
            info.name = budget.string(c.cstr())?;
            match &info.item_type.0 {
                b"mime" => {
                    info.content_type = budget.string(c.cstr())?;
                    info.content_encoding = budget.string(c.cstr())?;
                }
                b"uri " => info.uri_type = budget.string(c.cstr())?,
                _ => {}
            }
        }
        _ => return Err(Error::Malformed("infe")),
    }
    Ok(info)
}

/// Parses an `iinf` box: `payload` is the range after its header, `depth` the depth of the box.
pub(crate) fn parse(
    ctx: &Ctx<'_>,
    payload: Range<u64>,
    depth: usize,
) -> Result<Vec<ItemInfo>, Error> {
    let bytes = ctx.slice(payload.clone())?;
    let (version, _, rest) = full_header("iinf", bytes)?;
    let mut c = Cur::new("iinf", rest, 0);
    let (count, head) = match version {
        // libheif reads a u32 count for any version above 0 (box.cc:2650).
        0 => (usize::from(c.u16()?), 6u64),
        _ => (
            usize::try_from(c.u32()?).map_err(|_| Error::Malformed("iinf"))?,
            8u64,
        ),
    };
    if count > ctx.limits.max_items {
        return Err(Error::LimitExceeded(Limit::Items));
    }
    let mut out = ctx.budget.vec::<ItemInfo>(count)?;
    let kids = payload.start.saturating_add(head)..payload.end;
    let mut boxes = Boxes::new(ctx, kids, depth.saturating_add(1), false)?;
    // The first `count` children are the entries, whatever their type (libheif box.cc:1154-1192,
    // libavif read.c:3318-3331); one that is not `infe` is skipped, later children are ignored.
    let mut seen: usize = 0;
    while seen < count {
        let Some(h) = boxes.next_box()? else { break };
        seen = seen.saturating_add(1);
        if h.kind.0 == *b"infe" {
            out.push(parse_infe(ctx.slice(h.payload())?, ctx.budget)?);
        }
    }
    if seen != count {
        return Err(Error::Malformed("iinf"));
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Limits;
    use crate::boxes::Src;
    use std::sync::atomic::AtomicBool;

    fn budget() -> (Limits, AtomicBool) {
        (Limits::default(), AtomicBool::new(false))
    }

    #[test]
    fn infe_v2_hvc1() {
        let (l, c) = budget();
        let b = Budget::new(&l, &c);
        let mut p = vec![2, 0, 0, 1, 0, 7, 0, 0];
        p.extend_from_slice(b"hvc1\0");
        let i = parse_infe(&p, &b).unwrap();
        assert_eq!((i.id, i.item_type, i.hidden), (7, FourCc(*b"hvc1"), true));
    }

    #[test]
    fn infe_v3_mime_and_uri() {
        let (l, c) = budget();
        let b = Budget::new(&l, &c);
        let mut p = vec![3, 0, 0, 0, 0, 1, 0, 0, 0, 0];
        p.extend_from_slice(b"mime\0application/rdf+xml\0");
        let i = parse_infe(&p, &b).unwrap();
        assert_eq!(
            (i.id, i.content_type.as_str(), i.content_encoding.as_str()),
            (65536, "application/rdf+xml", "")
        );
        let mut u = vec![2, 0, 0, 0, 0, 2, 0, 0];
        u.extend_from_slice(b"uri \0urn:x\0");
        let i = parse_infe(&u, &b).unwrap();
        assert_eq!(i.uri_type, "urn:x");
    }

    #[test]
    fn infe_v0_has_no_type() {
        let (l, c) = budget();
        let b = Budget::new(&l, &c);
        let mut p = vec![0, 0, 0, 0, 0, 3, 0, 0];
        p.extend_from_slice(b"n\0image/jpeg\0");
        let i = parse_infe(&p, &b).unwrap();
        assert_eq!(
            (i.id, i.item_type, i.name.as_str(), i.content_type.as_str()),
            (3, FourCc([0; 4]), "n", "image/jpeg")
        );
    }

    #[test]
    fn infe_too_short_or_bad_version() {
        let (l, c) = budget();
        let b = Budget::new(&l, &c);
        assert!(parse_infe(&[2, 0, 0, 0, 0, 1], &b).is_err());
        assert!(parse_infe(&[9, 0, 0, 0, 0, 1, 0, 0], &b).is_err());
    }

    #[test]
    fn infe_keeps_the_protection_index() {
        let (l, c) = budget();
        let b = Budget::new(&l, &c);
        let mut p = vec![2, 0, 0, 0, 0, 7, 0, 5];
        p.extend_from_slice(b"hvc1\0");
        assert_eq!(parse_infe(&p, &b).unwrap().protection_index, 5);
    }

    #[test]
    fn iinf_of_any_version_above_0_has_a_u32_count() {
        let (l, c) = budget();
        let b = Budget::new(&l, &c);
        let infe = b"\0\0\0\x15infe\x02\0\0\0\0\x01\0\0hvc1\0";
        for version in [1u8, 2, 3] {
            let mut p = vec![version, 0, 0, 0, 0, 0, 0, 1];
            p.extend_from_slice(infe);
            let len = p.len() as u64;
            let src = Src {
                bytes: &p,
                base: 0,
                file_len: len,
            };
            let ctx = Ctx::new(&src, &b, &l);
            let items = parse(&ctx, 0..len, 1).unwrap();
            assert_eq!(items.len(), 1, "version {version}");
            assert_eq!(items[0].id, 1, "version {version}");
        }
    }
}
