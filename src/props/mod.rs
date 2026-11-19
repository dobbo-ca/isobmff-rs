//! Item properties: the children of `ipco`, each parsed by its 4cc.

mod config;
mod image;

use std::sync::Arc;

use crate::boxes::{BoxHeader, Ctx};
use crate::budget::Budget;
use crate::{Error, Extent, FourCc};

pub use config::{Av1c, Hvcc, NalArray};
pub use image::{Axis, Clap, Colour, Rect};

pub(crate) use config::{parse_av1c, parse_hvcc};

/// One child of `ipco`. Its index in [`Heif::properties`](crate::Heif::properties) plus one is
/// the property index that `ipma` uses.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Property {
    /// The box type.
    pub kind: FourCc,
    /// The payload of the box (after its header), as an extent of the file.
    pub range: Extent,
    /// The parsed value.
    pub value: PropValue,
}

/// The parsed value of a [`Property`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PropValue {
    /// `ispe`: the image size before transforms.
    Ispe {
        /// Width in pixels.
        width: u32,
        /// Height in pixels.
        height: u32,
    },
    /// `pixi`: bits per channel.
    Pixi {
        /// One entry per channel.
        bits: Vec<u8>,
    },
    /// `colr` with colour type `nclx`, `prof` or `rICC`.
    Colr(Colour),
    /// `irot`: anticlockwise rotation in degrees (0, 90, 180 or 270).
    Irot {
        /// The angle.
        angle: u16,
    },
    /// `imir`.
    Imir {
        /// The mirror axis.
        axis: Axis,
    },
    /// `clap`.
    Clap(Clap),
    /// `pasp`.
    Pasp {
        /// `hSpacing`.
        h_spacing: u32,
        /// `vSpacing`.
        v_spacing: u32,
    },
    /// `auxC`.
    AuxC {
        /// The auxiliary type URN.
        aux_type: Arc<str>,
        /// The subtype bytes, as an extent of the file.
        subtype: Extent,
    },
    /// `lsel`.
    Lsel {
        /// `layer_id`.
        layer_id: u16,
    },
    /// `a1op`.
    A1op {
        /// `op_index`.
        op_index: u8,
    },
    /// `a1lx`.
    A1lx {
        /// The sizes of the first three layers.
        sizes: [u32; 3],
    },
    /// `hvcC`.
    Hvcc(Arc<Hvcc>),
    /// `av1C`.
    Av1c(Av1c),
    /// A property this crate knows by name and ignores (`clli`, `mdcv`, `cclv`, `amve`,
    /// `altt`, or a `colr` of another colour type).
    Other,
    /// A property type this crate does not know. Essential, it makes the image unsupported.
    Unknown,
    /// A known property that does not parse. Only the images that use it fail, with
    /// [`Error::Malformed`].
    Invalid,
}

/// The value of a property box, `Invalid` when its bytes do not parse. Only limits, memory and
/// cancel are errors.
pub(crate) fn parse_value(
    kind: FourCc,
    bytes: &[u8],
    base: u64,
    budget: &Budget<'_>,
) -> Result<PropValue, Error> {
    let r = match &kind.0 {
        b"ispe" => image::ispe(bytes, base),
        b"pixi" => image::pixi(bytes, base, budget),
        b"pasp" => image::pasp(bytes, base),
        b"irot" => image::irot(bytes, base),
        b"imir" => image::imir(bytes, base),
        b"clap" => image::clap(bytes, base),
        b"colr" => image::colr(bytes, base),
        b"auxC" => image::auxc(bytes, base, budget),
        b"lsel" => image::lsel(bytes, base),
        b"a1op" => image::a1op(bytes, base),
        b"a1lx" => image::a1lx(bytes, base),
        b"hvcC" => budget
            .charge(size_of::<Hvcc>())
            .and_then(|()| parse_hvcc(bytes, base, budget))
            .map(|h| PropValue::Hvcc(Arc::new(h))),
        b"av1C" => parse_av1c(bytes, base).map(PropValue::Av1c),
        b"clli" | b"mdcv" | b"cclv" | b"amve" | b"altt" => Ok(PropValue::Other),
        _ => Ok(PropValue::Unknown),
    };
    match r {
        Err(Error::Malformed(_) | Error::Truncated) => Ok(PropValue::Invalid),
        other => other,
    }
}

/// Parses the property box `h` (one child of `ipco`).
pub(crate) fn parse_property(ctx: &Ctx<'_>, h: &BoxHeader) -> Result<Property, Error> {
    let payload = h.payload();
    let bytes = ctx.slice(payload.clone())?;
    Ok(Property {
        kind: h.kind,
        range: Extent {
            offset: payload.start,
            length: payload.end.saturating_sub(payload.start),
        },
        value: parse_value(h.kind, bytes, payload.start, ctx.budget)?,
    })
}
