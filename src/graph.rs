//! The item graph: items, references and properties joined into [`Image`]s.

use std::borrow::Cow;
use std::sync::Arc;
use std::sync::atomic::AtomicBool;

use crate::boxes::Ctx;
use crate::budget::Budget;
use crate::derived::{
    GainMapMetadata, Grid, Overlay, OverlayInput, parse_grid, parse_iovl, parse_tmap,
};
use crate::error::ItemError;
use crate::ftyp::Ftyp;
use crate::meta::{Association, EntityGroup, Meta, Reference, iloc};
use crate::metadata::{Metadata, exif_tiff};
use crate::mini::Mini;
use crate::props::{Av1c, Axis, Clap, Colour, Hvcc, PropValue, Property};
use crate::{Error, Extent, FourCc, Limit, Limits};

/// The largest payload of a `grid`, `iovl` or `tmap` item that is read while the graph is built.
const DERIVED_MAX: usize = 4 << 20;
/// The most payload bytes read for all derived items of one graph (iloc lets items share bytes).
const DERIVED_TOTAL_MAX: usize = 64 << 20;
/// The longest valid `grid` payload.
const GRID_MAX: usize = 12;

const ALPHA_TYPES: [&str; 3] = [
    "urn:mpeg:mpegB:cicp:systems:auxiliary:alpha",
    "urn:mpeg:hevc:2015:auxid:1",
    "urn:mpeg:avc:2015:auxid:1",
];
const DEPTH_TYPES: [&str; 2] = [
    "urn:mpeg:mpegB:cicp:systems:auxiliary:depth",
    "urn:mpeg:hevc:2015:auxid:2",
];
const APPLE_GAIN_MAP: &str = "urn:com:apple:photo:2020:aux:hdrgainmap";
const XMP_TYPE: &str = "application/rdf+xml";

/// The reference types an `rref` may list (libheif box.cc:4313-4320).
const RREF_TYPES: [&[u8; 4]; 6] = [b"thmb", b"base", b"cdsc", b"dimg", b"auxl", b"prem"];

/// Item types that are images (the candidates of [`Heif::top_level_images`]).
const IMAGE_TYPES: [&[u8; 4]; 12] = [
    b"hvc1", b"av01", b"grid", b"iovl", b"iden", b"tmap", b"jpeg", b"j2k1", b"unci", b"mski",
    b"vvc1", b"avc1",
];

fn is_image_type(k: FourCc) -> bool {
    IMAGE_TYPES.iter().any(|t| **t == k.0)
}

/// Gives the bytes of an extent to the graph builder: appends them to `out`, which the caller
/// made large enough (charged to the budget) for all the extents it asks for.
pub(crate) trait Fetch {
    fn fetch(&mut self, extent: Extent, out: &mut Vec<u8>) -> Result<(), Error>;
}

/// What the file counts, for the corpus statistics.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Stats {
    /// Boxes visited.
    pub boxes: usize,
    /// The deepest box nesting, top level = 0.
    pub max_depth: usize,
}

/// The parsed derived payload of a `grid`, `iovl` or `tmap` item.
#[derive(Debug, Clone, PartialEq, Eq)]
#[allow(clippy::large_enum_variant)]
pub enum Derived {
    /// A `grid` (its `tiles` are empty here; [`ImageKind::Grid`] has them).
    Grid(Grid),
    /// An `iovl` (its input ids are 0 here; [`ImageKind::Overlay`] has them).
    Overlay(Overlay),
    /// A `tmap`: the gain map metadata, `None` for a version this crate does not read.
    Tmap(Option<GainMapMetadata>),
}

/// One item of `iinf`, joined with its `iloc` entry and its `ipma` associations.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Item {
    /// `item_ID`.
    pub id: u32,
    /// `item_type` (`\0\0\0\0` for an `infe` of version 0 or 1).
    pub item_type: FourCc,
    /// `item_name`.
    pub name: String,
    /// `content_type` (`mime` items).
    pub content_type: String,
    /// `content_encoding` (`mime` items).
    pub content_encoding: String,
    /// `item_uri_type` (`uri ` items).
    pub uri_type: String,
    /// `infe` flags bit 0.
    pub hidden: bool,
    /// `item_protection_index` of the `infe` (0: not protected).
    pub protection_index: u16,
    /// The absolute file extents of the item data, or why they cannot be had. Bytes of an item
    /// with an error are an error only when requested.
    pub data: Result<Vec<Extent>, ItemError>,
    /// `iloc` construction method (0 file, 1 idat, 2 item offset); 0 for an item without an `iloc`
    /// entry.
    pub construction_method: u8,
    /// The properties of the item, in `ipma` order (index 0 entries dropped).
    pub associations: Vec<Association>,
    /// The parsed payload of a `grid`, `iovl` or `tmap` item; the `Err` is what
    /// [`Heif::image`] returns for it.
    pub derived: Option<Result<Derived, Error>>,
}

/// A parsed HEIF, HEIC or AVIF file. It holds no borrow of the input: items point into the file
/// by absolute [`Extent`]s.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Heif {
    /// The file type box.
    pub ftyp: Ftyp,
    /// `pitm`: the primary item id (0 when the file has none).
    pub primary: u32,
    /// Every `infe`, in `iinf` order.
    pub items: Vec<Item>,
    /// Every `ipco` child, in order (property index `i + 1`).
    pub properties: Vec<Property>,
    /// Every reference box of `iref`, in file order.
    pub references: Vec<Reference>,
    /// Every entity group of `grpl`, in file order.
    pub groups: Vec<EntityGroup>,
    /// Every other child of `meta` (`ipro`, `dinf`, unknown boxes) as (type, whole-box range), in
    /// file order. Kept so a caller can copy or inspect them; not parsed.
    pub other_boxes: Vec<(FourCc, Extent)>,
    /// Every child of `iprp` that is not `ipco` or `ipma` (unknown boxes) as (type, whole-box range),
    /// in file order. Kept, not parsed.
    pub iprp_other: Vec<(FourCc, Extent)>,
    /// The top-level `mini` box (ISO/IEC 23008-12 Annex O) when the file has one instead of `meta`:
    /// its header fields and chunk ranges. The items and properties above are made from it.
    pub mini: Option<Mini>,
    /// The file has a top-level `moov` box (not parsed in this version).
    pub has_moov: bool,
    /// Box counts of the parse.
    pub stats: Stats,
    max_pixels: u64,
    /// The `max_alloc` of the parse: the budget of each [`Heif::image`] call.
    max_alloc: AllocCap,
    /// (item id, position in `items`), sorted.
    index: Vec<(u32, u32)>,
    /// (`from` id, position in `references`), sorted.
    fwd: Vec<(u32, u32)>,
    /// (`to` id, position in `references`), sorted.
    rev: Vec<(u32, u32)>,
    /// (entity id, `groups` position, first position in the group) of every `altr` group,
    /// sorted, one entry per (entity, group).
    altr: Vec<(u32, u32, u32)>,
}

/// The `max_alloc` a parse kept. Not part of the value: two parses of one file are equal
/// whatever their limits.
#[derive(Debug, Clone, Copy)]
struct AllocCap(usize);

impl PartialEq for AllocCap {
    fn eq(&self, _: &Self) -> bool {
        true
    }
}

impl Eq for AllocCap {}

/// The codec of a coded image.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Codec {
    /// HEVC (`hvc1`).
    Hevc,
    /// AV1 (`av01`).
    Av1,
}

/// The codec configuration of a coded image.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CodecConfig {
    /// The `hvcC` of an `hvc1` item.
    Hevc(Arc<Hvcc>),
    /// The `av1C` of an `av01` item.
    Av1(Av1c),
}

/// What an image item is.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ImageKind {
    /// A coded image: the item data (see [`Item::data`]) is one HEVC or AV1 bitstream in the
    /// format of `config`.
    Coded {
        /// The codec.
        codec: Codec,
        /// The parsed configuration record (header fields and raw range).
        config: CodecConfig,
    },
    /// A grid of tiles.
    Grid(Grid),
    /// An overlay.
    Overlay(Overlay),
    /// An identity derived image: the same pixels as `input` (with this item's transforms).
    Identity {
        /// The input item id.
        input: u32,
    },
    /// Anything else: an unsupported item type (`jpeg`, `unci`, `hvt1`, `tmap`, ...) or an image
    /// with an essential property that this crate does not know.
    Unsupported(FourCc),
}

/// A transformative property.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Transform {
    /// `clap`: crop.
    Clap(Clap),
    /// `irot`: rotate anticlockwise by 0, 90, 180 or 270 degrees.
    Rotate(u16),
    /// `imir`: mirror.
    Mirror(Axis),
}

/// The layer properties of an image (`lsel`, `a1op`, `a1lx`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Layer {
    /// `lsel` `layer_id`.
    pub layer_id: Option<u16>,
    /// `a1op` `op_index`.
    pub op_index: Option<u8>,
    /// `a1lx` layer sizes.
    pub sizes: Option<[u32; 3]>,
}

/// An auxiliary image (`auxl` reference to the image) and its `auxC` type.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Aux {
    /// The auxiliary item id.
    pub id: u32,
    /// The `auxC` `aux_type` (empty when the item has no `auxC`).
    pub aux_type: Arc<str>,
    /// The `auxC` subtype bytes, as an extent of the file.
    pub subtype: Extent,
}

/// Where a gain map comes from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GainMapSource {
    /// A `tmap` item (ISO 21496-1), in an `altr` group with the base image.
    Tmap,
    /// An Apple HDR gain map: an `auxl` image of type `urn:com:apple:photo:2020:aux:hdrgainmap`.
    Apple,
}

/// A gain map of an image.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GainMap {
    /// The gain map image item.
    pub image: u32,
    /// The `tmap` item ([`GainMapSource::Tmap`]). Its payload extents are in its [`Item::data`]
    /// (useful when `metadata` is `None` because of an unknown version).
    pub tmap: Option<u32>,
    /// The parsed ISO 21496-1 metadata; `None` for the Apple source and for an unknown version.
    pub metadata: Option<GainMapMetadata>,
    /// Where it comes from.
    pub source: GainMapSource,
    /// The XMP items of the gain map image (Apple: the headroom is in this XMP).
    pub xmp: Vec<u32>,
}

/// An image item with everything the graph knows about it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Image {
    /// The item id.
    pub id: u32,
    /// What it is.
    pub kind: ImageKind,
    /// `ispe` width (0 when the image has no `ispe`).
    pub width: u32,
    /// `ispe` height (0 when the image has no `ispe`).
    pub height: u32,
    /// The transformative properties, in `ipma` order. A reader applies them in this order.
    pub transforms: Vec<Transform>,
    /// Every `colr` property (`nclx` and/or ICC).
    pub colour: Vec<Colour>,
    /// `pixi` bits per channel.
    pub pixi: Option<Vec<u8>>,
    /// `pasp` (h spacing, v spacing).
    pub pasp: Option<(u32, u32)>,
    /// `lsel`, `a1op`, `a1lx`.
    pub layer: Option<Layer>,
    /// The alpha plane item.
    pub alpha: Option<u32>,
    /// The depth map item.
    pub depth: Option<u32>,
    /// Every `auxl` image of this image, ascending id.
    pub aux: Vec<Aux>,
    /// Thumbnails (`thmb` references to this image), ascending id.
    pub thumbnails: Vec<u32>,
    /// The gain map, from a `tmap` item or an Apple auxiliary image.
    pub gain_map: Option<GainMap>,
    /// `Exif` items that describe this image (`cdsc`), ascending id.
    pub exif: Vec<u32>,
    /// XMP items that describe this image (`cdsc`), ascending id.
    pub xmp: Vec<u32>,
    /// The alpha plane is premultiplied (this item is the source of a `prem` reference).
    pub premultiplied: bool,
}

impl Image {
    /// The size after the transforms, in order, by the rule of
    /// [`apply_transforms`](crate::apply_transforms): a `clap` with a [`Clap::crop_rounded`]
    /// rectangle sets the crop size
    /// (one that `apply_transforms` rejects is left out here), `irot` 90 or 270 swaps the axes, `imir`
    /// keeps the size. `None` when the image has no `ispe`.
    pub fn display_size(&self) -> Option<(u32, u32)> {
        if self.width == 0 && self.height == 0 {
            return None;
        }
        let (mut w, mut h) = (self.width, self.height);
        for t in &self.transforms {
            match t {
                Transform::Clap(c) => {
                    if let Some(r) = c.crop_rounded(w, h) {
                        (w, h) = (r.width, r.height);
                    }
                }
                Transform::Rotate(deg) => {
                    if *deg == 90 || *deg == 270 {
                        (w, h) = (h, w);
                    }
                }
                Transform::Mirror(_) => {}
            }
        }
        Some((w, h))
    }
}

/// A stable name for a property 4cc, for `Malformed`.
fn kind_name(k: FourCc) -> &'static str {
    match &k.0 {
        b"ispe" => "ispe",
        b"pixi" => "pixi",
        b"pasp" => "pasp",
        b"irot" => "irot",
        b"imir" => "imir",
        b"clap" => "clap",
        b"colr" => "colr",
        b"auxC" => "auxC",
        b"lsel" => "lsel",
        b"a1op" => "a1op",
        b"a1lx" => "a1lx",
        b"hvcC" => "hvcC",
        b"av1C" => "av1C",
        _ => "property",
    }
}

/// The entries of a sorted `(key, value)` index with this key.
pub(crate) fn span(idx: &[(u32, u32)], key: u32) -> &[(u32, u32)] {
    let lo = idx.partition_point(|e| e.0 < key);
    let hi = idx.partition_point(|e| e.0 <= key);
    idx.get(lo..hi).unwrap_or_default()
}

/// True for errors that a bad derived payload gives; the rest (limits, memory, cancel) abort.
fn soft(e: &Error) -> bool {
    matches!(
        e,
        Error::Malformed(_)
            | Error::Truncated
            | Error::LimitExceeded(Limit::Pixels | Limit::GridTiles)
    )
}

/// Reads and parses the payload of a `grid`, `iovl` or `tmap` item. `None` for other types.
/// `mini`: the file has a `mini` box, whose `tmap` item data lacks the leading version byte (0)
/// of the payload.
fn derive<F: Fetch>(
    ctx: &Ctx<'_>,
    fetch: &mut F,
    kind: FourCc,
    data: &Result<Vec<Extent>, ItemError>,
    spent: &mut usize,
    mini: bool,
) -> Result<Option<Result<Derived, Error>>, Error> {
    let what = match &kind.0 {
        b"grid" => "grid",
        b"iovl" => "iovl",
        b"tmap" => "tmap",
        _ => return Ok(None),
    };
    let extents = match data {
        Ok(e) => e,
        Err(e) => return Ok(Some(Err(Error::from(*e)))),
    };
    let mut total: usize = 0;
    for e in extents {
        total = total.saturating_add(usize::try_from(e.length).unwrap_or(usize::MAX));
    }
    if total > DERIVED_MAX || (what == "grid" && total > GRID_MAX) {
        return Ok(Some(Err(Error::Malformed(what))));
    }
    // Only payloads that are read count toward the total.
    *spent = spent.saturating_add(total);
    if *spent > DERIVED_TOTAL_MAX {
        return Ok(Some(Err(Error::Malformed(what))));
    }
    let prefix = usize::from(mini && what == "tmap");
    let held = total.saturating_add(prefix);
    let mut buf = ctx.budget.vec::<u8>(held)?;
    if prefix == 1 {
        buf.push(0);
    }
    for e in extents {
        fetch.fetch(*e, &mut buf)?;
    }
    let parsed = match what {
        "grid" => parse_grid(&buf, ctx.limits).map(Derived::Grid),
        "iovl" => parse_iovl(&buf, ctx.limits, ctx.budget).map(Derived::Overlay),
        _ => parse_tmap(&buf).map(Derived::Tmap),
    };
    ctx.budget.release(held);
    match parsed {
        Ok(d) => Ok(Some(Ok(d))),
        Err(e) if soft(&e) => Ok(Some(Err(e))),
        Err(e) => Err(e),
    }
}

/// Joins what `meta` holds into the graph. `fetch` reads the payloads of derived items.
pub(crate) fn build<F: Fetch>(
    ctx: &Ctx<'_>,
    ftyp: Ftyp,
    has_moov: bool,
    meta: Meta,
    mini: Option<Mini>,
    fetch: &mut F,
) -> Result<Heif, Error> {
    let budget = ctx.budget;
    let Meta {
        primary,
        items: infos,
        iloc: mut locs,
        idat,
        refs,
        props,
        mut assoc,
        groups,
        other,
        iprp_other,
    } = meta;
    locs.sort_unstable_by_key(|i| i.id);
    if locs.windows(2).any(|w| matches!(w, [a, b] if a.id == b.id)) {
        return Err(Error::Malformed("iloc"));
    }
    assoc.sort_unstable_by_key(|e| e.0);
    if assoc.windows(2).any(|w| matches!(w, [a, b] if a.0 == b.0)) {
        return Err(Error::Malformed("ipma"));
    }
    let n = infos.len();
    let mut index = budget.vec::<(u32, u32)>(n)?;
    for (pos, info) in infos.iter().enumerate() {
        index.push((info.id, pos as u32));
    }
    index.sort_unstable();
    if index.windows(2).any(|w| matches!(w, [a, b] if a.0 == b.0)) {
        return Err(Error::Malformed("iinf"));
    }
    let mut fwd = budget.vec::<(u32, u32)>(refs.len())?;
    for (pos, r) in refs.iter().enumerate() {
        fwd.push((r.from, pos as u32));
    }
    fwd.sort_unstable();
    // Every item is resolved before the first derived payload is read: method 2 (item offset)
    // items point into other items.
    let mut resolved = iloc::resolve_all(&mut locs, &refs, &fwd, idat, ctx)?;
    let mut items = budget.vec::<Item>(n)?;
    let mut derived_spent = 0usize;
    for info in infos {
        budget.check_cancel()?;
        let (construction_method, data) = match locs.binary_search_by_key(&info.id, |i| i.id) {
            Ok(pos) => (
                locs.get(pos).map_or(0, |it| it.method),
                resolved
                    .get_mut(pos)
                    .and_then(Option::take)
                    .unwrap_or(Err(ItemError::NoLocation)),
            ),
            Err(_) => (0, Err(ItemError::NoLocation)),
        };
        let associations = match assoc.binary_search_by_key(&info.id, |e| e.0) {
            Ok(pos) => assoc
                .get_mut(pos)
                .map(|e| std::mem::take(&mut e.1))
                .unwrap_or_default(),
            Err(_) => Vec::new(),
        };
        let derived = derive(
            ctx,
            fetch,
            info.item_type,
            &data,
            &mut derived_spent,
            mini.is_some(),
        )?;
        items.push(Item {
            id: info.id,
            item_type: info.item_type,
            name: info.name,
            content_type: info.content_type,
            content_encoding: info.content_encoding,
            uri_type: info.uri_type,
            hidden: info.hidden,
            protection_index: info.protection_index,
            data,
            construction_method,
            associations,
            derived,
        });
    }
    let mut to_total: usize = 0;
    for r in &refs {
        to_total = to_total.saturating_add(r.to.len());
    }
    let mut rev = budget.vec::<(u32, u32)>(to_total)?;
    for (pos, r) in refs.iter().enumerate() {
        for &t in &r.to {
            rev.push((t, pos as u32));
        }
    }
    rev.sort_unstable();
    rev.dedup();
    let altr_groups = || {
        groups
            .iter()
            .enumerate()
            .filter(|(_, g)| g.kind.0 == *b"altr")
    };
    let altr_total = altr_groups().fold(0usize, |n, (_, g)| n.saturating_add(g.entities.len()));
    let mut altr = budget.vec::<(u32, u32, u32)>(altr_total)?;
    for (gi, g) in altr_groups() {
        for (pos, &e) in g.entities.iter().enumerate() {
            altr.push((e, gi as u32, pos as u32));
        }
    }
    // Sorted by (entity, group, position): dedup keeps the first position of each pair.
    altr.sort_unstable();
    altr.dedup_by_key(|a| (a.0, a.1));
    Ok(Heif {
        ftyp,
        primary,
        items,
        properties: props,
        references: refs,
        groups,
        other_boxes: other,
        iprp_other,
        mini,
        has_moov,
        stats: Stats {
            boxes: ctx.boxes.get(),
            max_depth: ctx.max_depth.get(),
        },
        max_pixels: ctx.limits.max_pixels,
        max_alloc: AllocCap(ctx.limits.max_alloc),
        index,
        fwd,
        rev,
        altr,
    })
}

impl Heif {
    /// A borrowed slice of `data` for an extent of the file, `None` when it is not inside `data`.
    pub fn slice(data: &[u8], extent: Extent) -> Option<&[u8]> {
        let start = usize::try_from(extent.offset).ok()?;
        let end = usize::try_from(extent.end()?).ok()?;
        data.get(start..end)
    }

    /// The item with this id.
    pub fn item(&self, id: u32) -> Option<&Item> {
        let pos = self.index.binary_search_by_key(&id, |e| e.0).ok()?;
        let at = self.index.get(pos)?.1;
        self.items.get(usize::try_from(at).ok()?)
    }

    /// The references with `from` = `id`, in file order.
    fn refs_from(&self, id: u32) -> impl Iterator<Item = &Reference> {
        span(&self.fwd, id)
            .iter()
            .filter_map(|e| self.references.get(usize::try_from(e.1).ok()?))
    }

    /// The references that have `id` among their `to` ids, in file order.
    fn refs_to(&self, id: u32) -> impl Iterator<Item = &Reference> {
        span(&self.rev, id)
            .iter()
            .filter_map(|e| self.references.get(usize::try_from(e.1).ok()?))
    }

    /// The `dimg` inputs of a derived item, in order, charged to `budget`. `Malformed(what)`
    /// unless there are exactly `want`, found before the list is made.
    fn dimg(
        &self,
        id: u32,
        want: u64,
        what: &'static str,
        budget: &Budget<'_>,
    ) -> Result<Vec<u32>, Error> {
        let refs = || self.refs_from(id).filter(|r| r.kind.0 == *b"dimg");
        let n = refs().fold(0usize, |n, r| n.saturating_add(r.to.len()));
        if n as u64 != want {
            return Err(Error::Malformed(what));
        }
        let mut out = budget.vec::<u32>(n)?;
        for r in refs() {
            out.extend_from_slice(&r.to);
        }
        Ok(out)
    }

    /// The distinct `from` ids of the `kind` references to `id` whose item passes `keep`, in
    /// reference order. The list and a mark per item are charged to `budget`: the memory is
    /// bounded by the items, however many references repeat an id.
    fn sources(
        &self,
        id: u32,
        kind: &[u8; 4],
        keep: impl Fn(&Item) -> bool,
        budget: &Budget<'_>,
    ) -> Result<Vec<u32>, Error> {
        let refs = || self.refs_to(id).filter(move |r| r.kind.0 == *kind);
        let at = |from: u32| {
            let pos = self.index.binary_search_by_key(&from, |e| e.0).ok()?;
            usize::try_from(self.index.get(pos)?.1).ok()
        };
        let len = self.items.len();
        let mut marks = budget.vec::<bool>(len)?;
        marks.resize(len, false);
        let mut n = 0usize;
        for r in refs() {
            let Some(i) = at(r.from) else { continue };
            if let (Some(m), Some(it)) = (marks.get_mut(i), self.items.get(i))
                && !*m
                && keep(it)
            {
                *m = true;
                n = n.saturating_add(1);
            }
        }
        let mut out = budget.vec::<u32>(n)?;
        // A mark is cleared when its id is listed, so each id is listed once.
        for r in refs() {
            if let Some(m) = at(r.from).and_then(|i| marks.get_mut(i))
                && *m
            {
                *m = false;
                out.push(r.from);
            }
        }
        drop(marks);
        budget.release(len);
        Ok(out)
    }

    /// The property of an association, or `Malformed("ipma")` for an index past `ipco`.
    fn property(&self, a: &Association) -> Result<&Property, Error> {
        usize::from(a.index)
            .checked_sub(1)
            .and_then(|i| self.properties.get(i))
            .ok_or(Error::Malformed("ipma"))
    }

    /// The first `ispe` of an item.
    fn ispe(&self, item: &Item) -> Option<(u32, u32)> {
        item.associations
            .iter()
            .find_map(|a| match self.property(a).ok()?.value {
                PropValue::Ispe { width, height } => Some((width, height)),
                _ => None,
            })
    }

    /// The `auxC` of an item: type and subtype.
    fn aux_c(&self, item: &Item) -> Option<(Arc<str>, Extent)> {
        item.associations
            .iter()
            .find_map(|a| match &self.property(a).ok()?.value {
                PropValue::AuxC { aux_type, subtype } => Some((Arc::clone(aux_type), *subtype)),
                _ => None,
            })
    }

    /// The Exif and XMP items that describe `id` (`cdsc`), ascending, charged to `budget`.
    fn linked_metadata(&self, id: u32, budget: &Budget<'_>) -> Result<(Vec<u32>, Vec<u32>), Error> {
        let mut exif = self.sources(id, b"cdsc", |it| it.item_type.0 == *b"Exif", budget)?;
        let is_xmp = |it: &Item| it.item_type.0 == *b"mime" && it.content_type == XMP_TYPE;
        let mut xmp = self.sources(id, b"cdsc", is_xmp, budget)?;
        exif.sort_unstable();
        xmp.sort_unstable();
        Ok((exif, xmp))
    }

    /// The `altr` index entries of entity `id`: (id, group, first position), sorted by group.
    fn altr_of(&self, id: u32) -> &[(u32, u32, u32)] {
        let lo = self.altr.partition_point(|a| a.0 < id);
        let hi = self.altr.partition_point(|a| a.0 <= id);
        self.altr.get(lo..hi).unwrap_or(&[])
    }

    /// The gain map of image `id`: a `tmap` item with `dimg` `[id, gain map]` that comes before
    /// `id` in an `altr` group, else an Apple `auxl` gain map.
    ///
    /// Each `tmap` is looked at once; the `altr` order comes from the sorted index built with the
    /// graph, and each candidate walks its own groups, so a call costs O(`altr` entries of the
    /// candidates x log groups), at most the `altr` index once, not a group scan per candidate.
    /// The lists are bounded by the `tmap` items and charged to `budget`.
    fn gain_map(
        &self,
        id: u32,
        aux: &[Aux],
        budget: &Budget<'_>,
    ) -> Result<Option<GainMap>, Error> {
        let tmaps = self.sources(id, b"dimg", |it| it.item_type.0 == *b"tmap", budget)?;
        let mut cands = budget.vec::<(u32, u32, Option<GainMapMetadata>)>(tmaps.len())?;
        for &t in &tmaps {
            let Some(item) = self.item(t) else {
                continue;
            };
            let Some(Ok(Derived::Tmap(metadata))) = &item.derived else {
                continue;
            };
            let mut ids = self
                .refs_from(t)
                .filter(|r| r.kind.0 == *b"dimg")
                .flat_map(|r| r.to.iter().copied());
            if let (Some(base), Some(gm), None) = (ids.next(), ids.next(), ids.next())
                && base == id
            {
                cands.push((t, gm, *metadata));
            }
        }
        // The first candidate (in reference order) that precedes `id` in some `altr` group. The
        // groups of each candidate are walked and looked up among the groups of `id` (sorted by
        // group); each `tmap` is a candidate once, so the walks are at most the `altr` entries.
        let mine = self.altr_of(id);
        let best = cands.iter().find(|c| {
            self.altr_of(c.0).iter().any(|&(_, g, tpos)| {
                mine.binary_search_by_key(&g, |a| a.1)
                    .ok()
                    .and_then(|i| mine.get(i))
                    .is_some_and(|a| tpos < a.2)
            })
        });
        if let Some(&(t, gm, metadata)) = best {
            return Ok(Some(GainMap {
                image: gm,
                tmap: Some(t),
                metadata,
                source: GainMapSource::Tmap,
                xmp: Vec::new(),
            }));
        }
        let Some(a) = aux.iter().find(|a| &*a.aux_type == APPLE_GAIN_MAP) else {
            return Ok(None);
        };
        Ok(Some(GainMap {
            image: a.id,
            tmap: None,
            metadata: None,
            source: GainMapSource::Apple,
            xmp: self.linked_metadata(a.id, budget)?.1,
        }))
    }

    /// The `grid` image of item `id` with its tiles, checked against the tiles' `ispe`.
    fn grid(&self, item: &Item, budget: &Budget<'_>) -> Result<Grid, Error> {
        let mut g = match &item.derived {
            Some(Ok(Derived::Grid(g))) => g.clone(),
            Some(Err(e)) => return Err(e.clone()),
            _ => return Err(Error::Malformed("grid")),
        };
        let want = u64::from(g.rows).saturating_mul(u64::from(g.columns));
        let tiles = self.dimg(item.id, want, "grid", budget)?;
        let mut size = None;
        for &t in &tiles {
            let s = self
                .item(t)
                .and_then(|it| self.ispe(it))
                .ok_or(Error::Malformed("grid"))?;
            if *size.get_or_insert(s) != s {
                return Err(Error::Malformed("grid"));
            }
        }
        let (tw, th) = size.ok_or(Error::Malformed("grid"))?;
        let fits = |out: u32, tile: u32, n: u32| {
            u64::from(out) <= u64::from(tile).saturating_mul(u64::from(n))
        };
        if !fits(g.width, tw, g.columns) || !fits(g.height, th, g.rows) {
            return Err(Error::Malformed("grid"));
        }
        g.tiles = tiles;
        Ok(g)
    }

    /// The `iovl` image of item `id` with its input ids.
    fn overlay(&self, item: &Item, budget: &Budget<'_>) -> Result<Overlay, Error> {
        let o = match &item.derived {
            Some(Ok(Derived::Overlay(o))) => o,
            Some(Err(e)) => return Err(e.clone()),
            _ => return Err(Error::Malformed("iovl")),
        };
        // The count is checked before the inputs are copied, and the copy is charged.
        let ids = self.dimg(item.id, o.inputs.len() as u64, "iovl", budget)?;
        let mut inputs = budget.vec::<OverlayInput>(o.inputs.len())?;
        for (input, id) in o.inputs.iter().zip(ids) {
            inputs.push(OverlayInput { id, ..*input });
        }
        Ok(Overlay {
            fill: o.fill,
            width: o.width,
            height: o.height,
            inputs,
        })
    }

    /// The image item `id` with its properties, derived structure and linked items.
    ///
    /// # Errors
    ///
    /// [`Error::NoSuchItem`] for an unknown id; [`Error::Unsupported`] when the item is not an
    /// image type or its `rref` lists a reference type other than `thmb`, `base`, `cdsc`, `dimg`,
    /// `auxl` or `prem`; [`Error::Malformed`] when a property of the image other than `pixi`,
    /// `pasp` or `lsel` (optional, as in libheif) does not parse, an `ipma`
    /// index is past `ipco`, a coded image lacks its `hvcC` or `av1C`, or a grid, overlay or
    /// identity image does not match its tiles or inputs; [`Error::LimitExceeded`] with
    /// `Limit::Pixels` for an image larger than the `max_pixels` of the parse (and the limit
    /// error of the item's payload, stored at parse time), or with `Limit::Alloc` when the id
    /// lists of the image (tiles, inputs, auxiliary images, thumbnails, Exif and XMP items) do not
    /// fit the `max_alloc` of the parse; [`Error::OutOfMemory`].
    ///
    /// The id lists are charged to a budget of the `max_alloc` of the parse, one per call, before
    /// they are made. They are bounded by the items of the file: a reference repeated a million
    /// times gives one id, and a grid, overlay or identity image with the wrong count of `dimg`
    /// ids is an error before its list is made.
    pub fn image(&self, id: u32) -> Result<Image, Error> {
        let never = AtomicBool::new(false);
        let budget = Budget::with_max(self.max_alloc.0, &never);
        let budget = &budget;
        let item = self.item(id).ok_or(Error::NoSuchItem(id))?;
        if !is_image_type(item.item_type) {
            return Err(Error::Unsupported("item type"));
        }
        let mut img = Image {
            id,
            kind: ImageKind::Unsupported(item.item_type),
            width: 0,
            height: 0,
            transforms: Vec::new(),
            colour: Vec::new(),
            pixi: None,
            pasp: None,
            layer: None,
            alpha: None,
            depth: None,
            aux: Vec::new(),
            thumbnails: Vec::new(),
            gain_map: None,
            exif: Vec::new(),
            xmp: Vec::new(),
            premultiplied: false,
        };
        let mut hvcc: Option<Arc<Hvcc>> = None;
        let mut av1c: Option<Av1c> = None;
        let mut unknown_essential = false;
        let mut ispe_seen = false;
        let mut layer = Layer::default();
        let mut has_layer = false;
        for a in &item.associations {
            let p = self.property(a)?;
            match &p.value {
                PropValue::Ispe { width, height } => {
                    if !ispe_seen {
                        ispe_seen = true;
                        (img.width, img.height) = (*width, *height);
                    }
                }
                // The first property of a type wins, as in libheif and libavif.
                PropValue::Pixi { bits } => {
                    if img.pixi.is_none() {
                        img.pixi = Some(bits.clone());
                    }
                }
                PropValue::Colr(c) => img.colour.push(*c),
                PropValue::Irot { angle } => img.transforms.push(Transform::Rotate(*angle)),
                PropValue::Imir { axis } => img.transforms.push(Transform::Mirror(*axis)),
                PropValue::Clap(c) => img.transforms.push(Transform::Clap(*c)),
                PropValue::Pasp {
                    h_spacing,
                    v_spacing,
                } => {
                    if img.pasp.is_none() {
                        img.pasp = Some((*h_spacing, *v_spacing));
                    }
                }
                PropValue::Lsel { layer_id } => {
                    layer.layer_id.get_or_insert(*layer_id);
                    has_layer = true;
                }
                PropValue::A1op { op_index } => {
                    layer.op_index.get_or_insert(*op_index);
                    has_layer = true;
                }
                PropValue::A1lx { sizes } => {
                    layer.sizes.get_or_insert(*sizes);
                    has_layer = true;
                }
                PropValue::Hvcc(h) => {
                    hvcc.get_or_insert_with(|| Arc::clone(h));
                }
                PropValue::Av1c(v) => {
                    av1c.get_or_insert(*v);
                }
                PropValue::AuxC { .. } | PropValue::Other => {}
                PropValue::Unknown => unknown_essential |= a.essential,
                // libheif treats these as optional: the image keeps going without them
                // (box.h:1402,1429,1452; context.cc:740-753).
                PropValue::Invalid if matches!(&p.kind.0, b"pixi" | b"pasp" | b"lsel") => {}
                PropValue::Invalid => return Err(Error::Malformed(kind_name(p.kind))),
                PropValue::Rref { types } => {
                    if types.iter().any(|t| !RREF_TYPES.contains(&&t.0)) {
                        return Err(Error::Unsupported("rref"));
                    }
                }
            }
        }
        if has_layer {
            img.layer = Some(layer);
        }
        if u64::from(img.width).saturating_mul(u64::from(img.height)) > self.max_pixels {
            return Err(Error::LimitExceeded(Limit::Pixels));
        }
        // The codec config is required even when an unknown essential property makes the image
        // unsupported (libheif cannot set up its decoder without it).
        let coded = match &item.item_type.0 {
            b"hvc1" => Some(ImageKind::Coded {
                codec: Codec::Hevc,
                config: CodecConfig::Hevc(hvcc.ok_or(Error::Malformed("hvcC"))?),
            }),
            b"av01" => Some(ImageKind::Coded {
                codec: Codec::Av1,
                config: CodecConfig::Av1(av1c.ok_or(Error::Malformed("av1C"))?),
            }),
            _ => None,
        };
        if !unknown_essential {
            img.kind = match (coded, &item.item_type.0) {
                (Some(kind), _) => kind,
                (None, b"grid") => ImageKind::Grid(self.grid(item, budget)?),
                (None, b"iovl") => ImageKind::Overlay(self.overlay(item, budget)?),
                (None, b"iden") => match self.dimg(id, 1, "iden", budget)?.as_slice() {
                    &[input] => ImageKind::Identity { input },
                    _ => return Err(Error::Malformed("iden")),
                },
                _ => ImageKind::Unsupported(item.item_type),
            };
        }
        let is_image = |it: &Item| is_image_type(it.item_type);
        let mut aux_ids = self.sources(id, b"auxl", is_image, budget)?;
        aux_ids.sort_unstable();
        img.aux = budget.vec::<Aux>(aux_ids.len())?;
        // One shared empty type for every auxiliary image without `auxC`.
        let none: Arc<str> = Arc::from("");
        for aux_id in aux_ids {
            let (aux_type, subtype) = self
                .item(aux_id)
                .and_then(|it| self.aux_c(it))
                .unwrap_or_else(|| (Arc::clone(&none), Extent::default()));
            if img.alpha.is_none() && ALPHA_TYPES.contains(&&*aux_type) {
                img.alpha = Some(aux_id);
            }
            if img.depth.is_none() && DEPTH_TYPES.contains(&&*aux_type) {
                img.depth = Some(aux_id);
            }
            img.aux.push(Aux {
                id: aux_id,
                aux_type,
                subtype,
            });
        }
        img.thumbnails = self.sources(id, b"thmb", is_image, budget)?;
        img.thumbnails.sort_unstable();
        (img.exif, img.xmp) = self.linked_metadata(id, budget)?;
        img.premultiplied = self.refs_from(id).any(|r| r.kind.0 == *b"prem");
        img.gain_map = self.gain_map(id, &img.aux, budget)?;
        Ok(img)
    }

    /// The primary image (`pitm`), see [`image`](Self::image).
    ///
    /// # Errors
    ///
    /// [`Error::Malformed`]`("pitm")` when the file has no `pitm`; otherwise those of
    /// [`image`](Self::image).
    pub fn primary_image(&self) -> Result<Image, Error> {
        if self.primary == 0 {
            return Err(Error::Malformed("pitm"));
        }
        self.image(self.primary)
    }

    /// The ids of the top-level images, ascending: image items (`hvc1`, `av01`, `grid`, `iovl`,
    /// `iden`, `jpeg`, `j2k1`, `unci`, `mski`, `vvc1`, `avc1`) that are not hidden and are not
    /// the source of a `thmb` or `auxl` reference. A `tmap` (gain map) item is not listed: it is
    /// an image for [`image`](Self::image) but not for libheif. The same set as libheif's
    /// `heif_context_get_list_of_top_level_image_IDs`.
    pub fn top_level_images(&self) -> Vec<u32> {
        let mut ids: Vec<u32> = self
            .items
            .iter()
            .filter(|it| is_image_type(it.item_type) && it.item_type.0 != *b"tmap" && !it.hidden)
            .filter(|it| {
                !self
                    .refs_from(it.id)
                    .any(|r| r.kind.0 == *b"thmb" || r.kind.0 == *b"auxl")
            })
            .map(|it| it.id)
            .collect();
        ids.sort_unstable();
        ids.dedup();
        ids
    }

    /// The bytes of an item: its extents concatenated, as they are in the file (no
    /// decompression of `mime` items). Borrowed from `data` when the item has at most one
    /// extent, else a copy charged to `limits.max_alloc`. `data` is the whole file.
    ///
    /// # Errors
    ///
    /// [`Error::NoSuchItem`]; the [`ItemError`] of the item as an [`Error`] (`Malformed("iloc")`
    /// without a location, `Unsupported` for a data reference or a method-2 item with no single `iloc` reference or a source chain that is too long (or a cycle),
    /// `Truncated` for an extent past the file or, for method 2, an `extent_index` of 0 or past the
    /// reference or a range past the end of the source item, `Malformed("idat")`); [`Error::Truncated`] when
    /// an extent is not inside `data`; `LimitExceeded(Alloc)`; [`Error::OutOfMemory`];
    /// [`Error::Cancelled`].
    pub fn item_bytes<'a>(
        &self,
        data: &'a [u8],
        id: u32,
        limits: &Limits,
        cancel: &AtomicBool,
    ) -> Result<Cow<'a, [u8]>, Error> {
        let budget = Budget::new(limits, cancel);
        self.bytes_of(data, id, &budget)
    }

    fn bytes_of<'a>(
        &self,
        data: &'a [u8],
        id: u32,
        budget: &Budget<'_>,
    ) -> Result<Cow<'a, [u8]>, Error> {
        budget.check_cancel()?;
        let item = self.item(id).ok_or(Error::NoSuchItem(id))?;
        let extents = item.data.as_ref().map_err(|e| Error::from(*e))?;
        match extents.as_slice() {
            [] => Ok(Cow::Borrowed(&[])),
            [e] => Heif::slice(data, *e)
                .map(Cow::Borrowed)
                .ok_or(Error::Truncated),
            many => {
                let mut total: u64 = 0;
                for e in many {
                    total = total
                        .checked_add(e.length)
                        .ok_or(Error::LimitExceeded(Limit::Alloc))?;
                }
                let total =
                    usize::try_from(total).map_err(|_| Error::LimitExceeded(Limit::Alloc))?;
                let mut v = budget.vec::<u8>(total)?;
                for e in many {
                    v.extend_from_slice(Heif::slice(data, *e).ok_or(Error::Truncated)?);
                }
                Ok(Cow::Owned(v))
            }
        }
    }

    /// The bytes of a metadata item, after the `max_metadata_bytes` check.
    fn metadata_item<'a>(
        &self,
        data: &'a [u8],
        id: u32,
        limits: &Limits,
        budget: &Budget<'_>,
    ) -> Result<Cow<'a, [u8]>, Error> {
        let item = self.item(id).ok_or(Error::NoSuchItem(id))?;
        let extents = item.data.as_ref().map_err(|e| Error::from(*e))?;
        let mut total: u64 = 0;
        for e in extents {
            total = total.saturating_add(e.length);
        }
        if total > limits.max_metadata_bytes as u64 {
            return Err(Error::LimitExceeded(Limit::MetadataBytes));
        }
        self.bytes_of(data, id, budget)
    }

    /// The Exif and XMP bytes of image `image` (the first `cdsc` item of each kind). The Exif
    /// item payload is a big-endian `exif_tiff_header_offset`, that many bytes, then the TIFF
    /// header: the result starts at the TIFF header (what `exif::Exif::parse` takes). The XMP
    /// packet is the item bytes as they are. Borrowed from `data` for a one-extent item. The Exif
    /// item of a `mini` box has no offset field: its bytes are the TIFF data as they are, or a
    /// deflate stream when the item's `content_encoding` is `deflate` (`exif_xmp_compressed_flag`).
    /// Nothing is inflated: check [`Item::content_encoding`] of the items before parsing.
    ///
    /// # Errors
    ///
    /// [`Error::NoSuchItem`] for an unknown `image`; [`Error::Malformed`]`("Exif")` for an Exif
    /// item shorter than 4 bytes or an offset past its end; the errors of
    /// [`item_bytes`](Self::item_bytes); [`Error::LimitExceeded`] with `Limit::MetadataBytes`
    /// (an item larger than `limits.max_metadata_bytes`) or `Limit::Alloc`;
    /// [`Error::Cancelled`].
    pub fn metadata<'a>(
        &self,
        data: &'a [u8],
        image: u32,
        limits: &Limits,
        cancel: &AtomicBool,
    ) -> Result<Metadata<'a>, Error> {
        let budget = Budget::new(limits, cancel);
        self.item(image).ok_or(Error::NoSuchItem(image))?;
        let (exif_ids, xmp_ids) = self.linked_metadata(image, &budget)?;
        let exif = match exif_ids.first() {
            Some(&id) => {
                let item = self.metadata_item(data, id, limits, &budget)?;
                // The `mini` box has no `exif_tiff_header_offset` (libavif skips it too).
                Some(if self.mini.is_some() {
                    item
                } else {
                    exif_tiff(item)?
                })
            }
            None => None,
        };
        let xmp = match xmp_ids.first() {
            Some(&id) => Some(self.metadata_item(data, id, limits, &budget)?),
            None => None,
        };
        Ok(Metadata { exif, xmp })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn image(width: u32, height: u32, transforms: Vec<Transform>) -> Image {
        Image {
            id: 1,
            kind: ImageKind::Unsupported(FourCc(*b"hvc1")),
            width,
            height,
            transforms,
            colour: Vec::new(),
            pixi: None,
            pasp: None,
            layer: None,
            alpha: None,
            depth: None,
            aux: Vec::new(),
            thumbnails: Vec::new(),
            gain_map: None,
            exif: Vec::new(),
            xmp: Vec::new(),
            premultiplied: false,
        }
    }

    #[test]
    fn display_size_applies_transforms_in_order() {
        let clap = Clap {
            width_n: 100,
            width_d: 1,
            height_n: 40,
            height_d: 1,
            horiz_off_n: 0,
            horiz_off_d: 1,
            vert_off_n: 0,
            vert_off_d: 1,
        };
        let i = image(200, 100, vec![Transform::Clap(clap), Transform::Rotate(90)]);
        assert_eq!(i.display_size(), Some((40, 100)));
        let i = image(
            200,
            100,
            vec![Transform::Rotate(270), Transform::Mirror(Axis::LeftRight)],
        );
        assert_eq!(i.display_size(), Some((100, 200)));
        // An invalid clap leaves the size alone.
        let bad = Clap { width_d: 0, ..clap };
        assert_eq!(
            image(200, 100, vec![Transform::Clap(bad)]).display_size(),
            Some((200, 100))
        );
        assert_eq!(image(0, 0, vec![]).display_size(), None);
    }

    #[test]
    fn display_size_uses_the_rounded_crop_and_agrees_with_transformed_size() {
        let clap = |wn, hn, hon| Clap {
            width_n: wn,
            width_d: 1,
            height_n: hn,
            height_d: 1,
            horiz_off_n: hon,
            horiz_off_d: 1,
            vert_off_n: 0,
            vert_off_d: 1,
        };
        // 5 x 5 with a 2 x 2 clap: the origin is 1.5, which `Clap::crop` rejects and
        // `crop_rounded` rounds, so the size is 2 x 2.
        let t = vec![Transform::Clap(clap(2, 2, 0)), Transform::Rotate(90)];
        let i = image(5, 5, t.clone());
        assert_eq!(i.display_size(), Some((2, 2)));
        assert_eq!(
            crate::transformed_size(5, 5, &t, &crate::Limits::default()),
            Ok((2, 2))
        );
        // Partly outside: clamped, 45 wide.
        let t = vec![Transform::Clap(clap(50, 50, -30))];
        assert_eq!(image(100, 100, t.clone()).display_size(), Some((45, 50)));
        assert_eq!(
            crate::transformed_size(100, 100, &t, &crate::Limits::default()),
            Ok((45, 50))
        );
        // Rejected by `apply_transforms`: left out of the display size.
        let t = vec![Transform::Clap(clap(4, 4, 20)), Transform::Rotate(90)];
        assert_eq!(image(10, 20, t.clone()).display_size(), Some((20, 10)));
        assert!(crate::transformed_size(10, 20, &t, &crate::Limits::default()).is_err());
    }
    #[test]
    fn span_finds_equal_keys() {
        let idx = [(1, 0), (3, 1), (3, 2), (5, 3)];
        assert_eq!(span(&idx, 3), &[(3, 1), (3, 2)]);
        assert!(span(&idx, 4).is_empty());
    }
}
