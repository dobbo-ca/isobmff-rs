//! An ISO-BMFF container parser for HEIF (HEIC), AVIF and, later, MP4/MOV. It decodes no pixels.
//!
//! [`parse`] (a file in memory) and [`read`] (a `Read + Seek`) give a [`Heif`]: the item graph of
//! the file. Every coded item is a codec configuration record ([`Hvcc`], [`Av1c`]) plus byte
//! ranges of the file ([`Extent`]), which hevc-rs and av1-rs decode. [`Heif::primary_image`] and
//! [`Heif::image`] join an item with its properties, transforms, alpha and depth planes,
//! thumbnails and gain map; [`Heif::metadata`] gives the Exif and XMP bytes for exif-rs. All
//! positions are absolute file offsets, so a caller needs only the file; the [`Heif`] holds no
//! borrow of it. No call panics on any input.
//!
//! [`parse_movie`] (a file in memory) and [`read_movie`] (a `Read + Seek`) give a [`Movie`]: the
//! tracks of an MP4, MOV, 3GP or AVIS file, with the codec configuration of each sample entry
//! and the sample tables kept as ranges of the file. [`Track::sync_samples`] and
//! [`Track::sync_sample_at`] give the byte ranges of keyframes for hevc-rs and av1-rs; no sample
//! is decoded. Fragmented files (`mvex`, `moof`) give their samples in the same walks.
//!
//! # Example
//!
//! ```
//! use std::sync::atomic::AtomicBool;
//! use isobmff::{ImageKind, Limits};
//!
//! # fn bx(kind: &[u8; 4], body: &[u8]) -> Vec<u8> {
//! #     let mut v = ((body.len() as u32) + 8).to_be_bytes().to_vec();
//! #     v.extend_from_slice(kind);
//! #     v.extend_from_slice(body);
//! #     v
//! # }
//! # let cat = |parts: &[&[u8]]| parts.concat();
//! // A 64 x 48 HEVC image whose 4 data bytes are in `mdat`.
//! # let ftyp = bx(b"ftyp", b"heic\0\0\0\0mif1heic");
//! # let meta = |offset: u32| {
//! #     let hdlr = bx(b"hdlr", &cat(&[&[0; 8], b"pict", &[0; 13]]));
//! #     let pitm = bx(b"pitm", &[0, 0, 0, 0, 0, 1]);
//! #     let infe = bx(b"infe", &cat(&[&[2, 0, 0, 0, 0, 1, 0, 0], b"hvc1", &[0]]));
//! #     let iinf = bx(b"iinf", &cat(&[&[0, 0, 0, 0, 0, 1], &infe]));
//! #     let head: [u8; 14] = [0, 0, 0, 0, 0x44, 0, 0, 1, 0, 1, 0, 0, 0, 1];
//! #     let iloc = bx(b"iloc", &cat(&[&head, &offset.to_be_bytes(), &[0, 0, 0, 4]]));
//! #     let ispe = bx(b"ispe", &[0, 0, 0, 0, 0, 0, 0, 64, 0, 0, 0, 48]);
//! #     let hvcc = bx(b"hvcC", &[1, 1, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 93, 0xF0, 0, 0xFC, 0xFD, 0xF8, 0xF8, 0, 0, 3, 0]);
//! #     let ipco = bx(b"ipco", &cat(&[&ispe, &hvcc]));
//! #     let ipma = bx(b"ipma", &[0, 0, 0, 0, 0, 0, 0, 1, 0, 1, 2, 1, 0x82]);
//! #     let iprp = bx(b"iprp", &cat(&[&ipco, &ipma]));
//! #     bx(b"meta", &cat(&[&[0; 4], &hdlr, &pitm, &iinf, &iloc, &iprp]))
//! # };
//! # let offset = (ftyp.len() + meta(0).len() + 8) as u32;
//! # let file = cat(&[&ftyp, &meta(offset), &bx(b"mdat", &[1, 2, 3, 4])]);
//! let limits = Limits { max_threads: 1, max_alloc: 64 << 20, ..Limits::default() };
//! let cancel = AtomicBool::new(false);
//! let heif = isobmff::parse(&file, &limits, &cancel)?;
//! let image = heif.primary_image()?;
//! assert_eq!((image.width, image.height), (64, 48));
//! assert!(matches!(image.kind, ImageKind::Coded { .. }));
//! let bytes = heif.item_bytes(&file, image.id, &limits, &cancel)?;
//! assert_eq!(&bytes[..], &[1, 2, 3, 4]);
//! // `read` gives the same graph from a reader, without reading `mdat`.
//! let again = isobmff::read(std::io::Cursor::new(&file[..]), &limits, &cancel)?;
//! assert_eq!(again, heif);
//! # Ok::<(), isobmff::Error>(())
//! ```
//!
//! # Resource limits
//!
//! `parse`, `read`, `item_bytes`, `metadata`, `parse_movie`, `read_movie` and the sample walks of
//! [`Track`] take [`Limits`] and a cancel flag (`&AtomicBool`); the `Heif` graph accessors work on the already-bounded graph.
//!
//! - `max_alloc` (default 256 MiB) is the enforced peak of the working memory of one call: each
//!   buffer whose size comes from the input (items, properties, references, extents, the copy of
//!   `meta` that [`read`] makes, metadata copies) is charged before it is allocated. More is
//!   [`Error::LimitExceeded`]`(`[`Limit::Alloc`]`)`.
//! - `max_threads` (default `available_parallelism`): this crate never spawns a thread, so every
//!   call runs on the calling thread for every value, including 1.
//! - The structural caps (`max_boxes`, `max_depth`, `max_items`, `max_properties`,
//!   `max_references`, `max_extents`, `max_grid_tiles`, `max_pixels`, `max_meta_bytes`,
//!   `max_metadata_bytes`, `max_moov_bytes`) each give [`Error::LimitExceeded`] with their own [`Limit`]. The
//!   defaults are far above every real phone and camera file.
//! - Set the cancel flag from another thread to stop a call: it returns [`Error::Cancelled`].
//!   The flag is checked every 1024 boxes and before each item is read.
//! - A bad item does not fail the parse: the error of one item (an extent past the end of the
//!   file, a data reference other than 0) is in [`Item::data`] and surfaces when that item's
//!   bytes are requested. A bad property fails only the images that use it.

#![forbid(unsafe_code)]
#![cfg_attr(
    not(test),
    deny(
        clippy::unwrap_used,
        clippy::expect_used,
        clippy::panic,
        clippy::todo,
        clippy::unimplemented,
        clippy::indexing_slicing,
        clippy::arithmetic_side_effects
    )
)]

use std::io::{Read, Seek, SeekFrom};
use std::sync::atomic::AtomicBool;

mod boxes;
mod budget;
mod derived;
mod error;
mod fourcc;
mod ftyp;
mod graph;
mod limits;
mod meta;
mod metadata;
mod mini;
mod movie;
mod props;

pub use boxes::Extent;
pub use derived::{Fraction, GainMapChannel, GainMapMetadata, Grid, Overlay, OverlayInput};
pub use error::{Error, ItemError, Limit};
pub use fourcc::FourCc;
pub use ftyp::Ftyp;
pub use graph::{
    Aux, Codec, CodecConfig, Derived, GainMap, GainMapSource, Heif, Image, ImageKind, Item, Layer,
    Stats, Transform,
};
pub use limits::Limits;
pub use meta::{Association, EntityGroup, Reference};
pub use metadata::Metadata;
pub use mini::{Mini, MiniAmve, MiniCclv, MiniClli, MiniGainMap, MiniHdr, MiniMdcv, MiniReve};
pub use movie::{Edit, Movie, Sample, SampleEntry, Samples, Track, parse_movie, read_movie};
pub use props::{Av1c, Axis, Clap, Colour, Hvcc, NalArray, PropValue, Property, Rect};

use boxes::{BoxHeader, Boxes, Ctx, Src, decode_header};
use budget::Budget;
use graph::Fetch;

/// Parses the structure of a HEIF, HEIC or AVIF file held in memory: `ftyp`, `meta` (or the `mini`
/// box that replaces it, see [`Heif::mini`]) with its
/// items, properties, references and groups, and the small payloads of `grid`, `iovl` and `tmap`
/// items. The item data itself is not read; [`Heif::item_bytes`] gives it from the same `data`.
///
/// Limits enforced: `max_alloc`, `max_boxes`, `max_depth`, `max_items`, `max_properties`,
/// `max_references`, `max_extents` and `max_grid_tiles` (a bad grid or overlay is an error of
/// that image, not of the parse); the cancel flag every 1024 boxes.
///
/// # Errors
///
/// [`Error::UnknownFormat`] when the first box is not `ftyp`; [`Error::NoMeta`] when the file has
/// neither `meta` nor `moov` (a file with `moov` only parses to an empty item graph);
/// [`Error::Truncated`] when a top-level box before `meta` and `moov` runs past the end of `data` (after
/// `meta` or `moov`, an incomplete or overlong box ends the walk and item data fails per item);
/// [`Error::Malformed`] for an invalid box (a size smaller than its header or past its parent,
/// two `meta` boxes, two `mini` boxes or a `mini` with a `meta`, a `meta` that is not version 0 or
/// whose first child is not an `hdlr` of
/// handler `pict`, a box that is too short for its fields, an item id that appears twice in
/// `iinf`, `iloc` or `ipma`, a `mini` whose chunks run past its payload, ...);
/// [`Error::Unsupported`] for a `mini` of a version other than 0, or without explicit codec types and with an `ftyp` minor version other than `avif` or `heic`;
/// [`Error::LimitExceeded`] for any [`Limits`] field;
/// [`Error::OutOfMemory`]; [`Error::Cancelled`].
pub fn parse(data: &[u8], limits: &Limits, cancel: &AtomicBool) -> Result<Heif, Error> {
    let budget = Budget::new(limits, cancel);
    budget.check_cancel()?;
    let src = Src {
        bytes: data,
        base: 0,
        file_len: data.len() as u64,
    };
    if data.get(4..8) != Some(b"ftyp".as_slice()) {
        return Err(Error::UnknownFormat);
    }
    let ctx = Ctx::new(&src, &budget, limits);
    let mut boxes = Boxes::new(&ctx, 0..src.file_len, 0, true)?;
    let mut top = Top::default();
    loop {
        let h = match boxes.next_box() {
            Ok(Some(h)) => h,
            Ok(None) => break,
            // After `meta` or `mini`, a damaged tail ends the walk (as in libheif).
            Err(Error::Truncated) if top.meta.is_some() || top.has_moov => break,
            Err(e) => return Err(e),
        };
        top.note(&h);
        if h.kind.0 == *b"ftyp" && top.ftyp.is_none() {
            top.ftyp = Some(ftyp::parse(ctx.slice(h.payload())?, &budget)?);
        }
    }
    let (ftyp, hdr, is_mini, has_moov) = top.finish()?;
    let (meta, mini) = match &hdr {
        Some(hdr) if is_mini => {
            let (m, mini) = mini::parse(&ctx, hdr, &ftyp)?;
            (m, Some(mini))
        }
        Some(hdr) => (meta::parse(&ctx, hdr)?, None),
        None => (meta::Meta::default(), None),
    };
    graph::build(&ctx, ftyp, has_moov, meta, mini, &mut SliceFetch(data))
}

/// What the top-level walk finds.
#[derive(Default)]
struct Top {
    ftyp: Option<Ftyp>,
    /// The first `meta` or `mini` box (a file has one of them, see `finish`).
    meta: Option<BoxHeader>,
    metas: usize,
    /// The box in `meta` is a `mini`.
    mini: bool,
    minis: usize,
    has_moov: bool,
}

impl Top {
    fn note(&mut self, h: &BoxHeader) {
        match &h.kind.0 {
            b"meta" => {
                self.metas = self.metas.saturating_add(1);
                if self.meta.is_none() {
                    self.meta = Some(*h);
                }
            }
            b"mini" => {
                self.minis = self.minis.saturating_add(1);
                if self.meta.is_none() {
                    self.meta = Some(*h);
                    self.mini = true;
                }
            }
            b"moov" => self.has_moov = true,
            _ => {}
        }
    }

    /// `None` for the `meta` (or `mini`) header when the file has a `moov` but neither (an image
    /// sequence: an empty item graph); then the flag says whether the header is a `mini`.
    fn finish(self) -> Result<(Ftyp, Option<BoxHeader>, bool, bool), Error> {
        let ftyp = self.ftyp.ok_or(Error::UnknownFormat)?;
        if self.metas > 1 {
            return Err(Error::Malformed("meta"));
        }
        if self.minis > 1 || (self.minis > 0 && self.metas > 0) {
            return Err(Error::Malformed("mini"));
        }
        if self.meta.is_none() && !self.has_moov {
            return Err(Error::NoMeta);
        }
        Ok((ftyp, self.meta, self.mini, self.has_moov))
    }
}

struct SliceFetch<'a>(&'a [u8]);

impl Fetch for SliceFetch<'_> {
    fn fetch(&mut self, extent: Extent, out: &mut Vec<u8>) -> Result<(), Error> {
        out.extend_from_slice(Heif::slice(self.0, extent).ok_or(Error::Truncated)?);
        Ok(())
    }
}

fn io_err(e: std::io::Error) -> Error {
    if e.kind() == std::io::ErrorKind::UnexpectedEof {
        Error::Truncated
    } else {
        Error::Io(e.kind())
    }
}

/// Fills `buf` from the file bytes at `pos`.
fn read_at<R: Read + Seek>(r: &mut R, pos: u64, buf: &mut [u8]) -> Result<(), Error> {
    r.seek(SeekFrom::Start(pos)).map_err(io_err)?;
    r.read_exact(buf).map_err(io_err)
}

/// Reads `len` bytes at `pos` into a charged buffer.
fn read_vec<R: Read + Seek>(
    r: &mut R,
    budget: &Budget<'_>,
    pos: u64,
    len: usize,
) -> Result<Vec<u8>, Error> {
    let mut v = budget.vec::<u8>(len)?;
    v.resize(len, 0);
    read_at(r, pos, v.as_mut_slice())?;
    Ok(v)
}

struct ReadFetch<'a, R>(&'a mut R);

impl<R: Read + Seek> Fetch for ReadFetch<'_, R> {
    fn fetch(&mut self, extent: Extent, out: &mut Vec<u8>) -> Result<(), Error> {
        let len = usize::try_from(extent.length).map_err(|_| Error::Truncated)?;
        let start = out.len();
        // The caller reserved the sum of all extents, so this does not reallocate.
        out.resize(start.saturating_add(len), 0);
        let dst = out.get_mut(start..).ok_or(Error::Truncated)?;
        read_at(self.0, extent.offset, dst)
    }
}

/// Parses the structure of a HEIF, HEIC or AVIF file from a reader. It reads the top-level box
/// headers, `ftyp` and the whole `meta` box (at most `limits.max_meta_bytes`, charged to
/// `max_alloc`), and the few bytes of each `grid`, `iovl` and `tmap` item; it seeks over `mdat`.
/// The result equals [`parse`] on the same bytes. Item extents are absolute file offsets, so the
/// caller reads item data itself with the same reader.
///
/// Limits enforced: those of [`parse`] and `max_meta_bytes`.
///
/// # Errors
///
/// Those of [`parse`], and [`Error::LimitExceeded`] with `Limit::MetaBytes` for a larger `meta` or `mini`;
/// [`Error::Truncated`] when the reader ends early; [`Error::Io`] for another I/O error.
pub fn read<R: Read + Seek>(mut r: R, limits: &Limits, cancel: &AtomicBool) -> Result<Heif, Error> {
    let budget = Budget::new(limits, cancel);
    budget.check_cancel()?;
    let file_len = r.seek(SeekFrom::End(0)).map_err(io_err)?;
    let none = Src {
        bytes: &[],
        base: 0,
        file_len,
    };
    let walk = Ctx::new(&none, &budget, limits);
    let mut top = Top::default();
    let mut pos: u64 = 0;
    let mut head = [0u8; 32];
    while pos < file_len {
        walk.tick()?;
        let avail = usize::try_from(file_len.saturating_sub(pos).min(32)).unwrap_or(32);
        let buf = head.get_mut(..avail).ok_or(Error::Truncated)?;
        read_at(&mut r, pos, buf)?;
        if pos == 0 && buf.get(4..8) != Some(b"ftyp".as_slice()) {
            return Err(Error::UnknownFormat);
        }
        let h = match decode_header(buf, pos, file_len, true) {
            Ok(h) => h,
            Err(Error::Truncated) if top.meta.is_some() || top.has_moov => break,
            Err(e) => return Err(e),
        };
        top.note(&h);
        if h.kind.0 == *b"ftyp" && top.ftyp.is_none() {
            let p = h.payload();
            let len = usize::try_from(p.end.saturating_sub(p.start))
                .map_err(|_| Error::LimitExceeded(Limit::MetaBytes))?;
            if len > limits.max_meta_bytes {
                return Err(Error::LimitExceeded(Limit::MetaBytes));
            }
            let bytes = read_vec(&mut r, &budget, p.start, len)?;
            top.ftyp = Some(ftyp::parse(&bytes, &budget)?);
            budget.release(len);
        }
        pos = h.end;
    }
    let (ftyp, hdr, is_mini, has_moov) = top.finish()?;
    let Some(hdr) = hdr else {
        return graph::build(
            &walk,
            ftyp,
            has_moov,
            meta::Meta::default(),
            None,
            &mut ReadFetch(&mut r),
        );
    };
    let len = usize::try_from(hdr.end.saturating_sub(hdr.start))
        .map_err(|_| Error::LimitExceeded(Limit::MetaBytes))?;
    if len > limits.max_meta_bytes {
        return Err(Error::LimitExceeded(Limit::MetaBytes));
    }
    let bytes = read_vec(&mut r, &budget, hdr.start, len)?;
    let src = Src {
        bytes: &bytes,
        base: hdr.start,
        file_len,
    };
    let ctx = Ctx::new(&src, &budget, limits);
    ctx.boxes.set(walk.boxes.get());
    let (meta, mini) = if is_mini {
        let (m, mini) = mini::parse(&ctx, &hdr, &ftyp)?;
        (m, Some(mini))
    } else {
        (meta::parse(&ctx, &hdr)?, None)
    };
    graph::build(&ctx, ftyp, has_moov, meta, mini, &mut ReadFetch(&mut r))
}

/// Entry points for the fuzz targets: one per parser, on a raw box payload (the bytes after the
/// box header, FullBox version and flags included) or, for [`boxes`](__fuzz::boxes), on a whole
/// box tree. Not part of the API.
#[doc(hidden)]
pub mod __fuzz {
    use super::meta::{iinf, iloc, iprp, iref};
    use super::*;

    /// Runs `f` on a context over `data` with a cancel flag that is never set.
    fn with(
        data: &[u8],
        limits: &Limits,
        f: impl FnOnce(&Ctx<'_>) -> Result<(), Error>,
    ) -> Result<(), Error> {
        let cancel = AtomicBool::new(false);
        let budget = Budget::new(limits, &cancel);
        let src = Src {
            bytes: data,
            base: 0,
            file_len: data.len() as u64,
        };
        f(&Ctx::new(&src, &budget, limits))
    }

    fn len(data: &[u8]) -> u64 {
        data.len() as u64
    }

    fn walk(
        ctx: &Ctx<'_>,
        range: std::ops::Range<u64>,
        depth: usize,
        top: bool,
    ) -> Result<(), Error> {
        let mut b = Boxes::new(ctx, range, depth, top)?;
        while let Some(h) = b.next_box()? {
            match walk(ctx, h.payload(), depth.saturating_add(1), false) {
                Ok(()) | Err(Error::Malformed(_) | Error::Truncated) => {}
                Err(e) => return Err(e),
            }
        }
        Ok(())
    }

    /// Walks every box to `max_depth`, recursing into every child that parses as a box list.
    ///
    /// # Errors
    ///
    /// Limit errors; a malformed inner box list is not an error.
    pub fn boxes(data: &[u8], limits: &Limits) -> Result<(), Error> {
        with(data, limits, |ctx| walk(ctx, 0..len(data), 0, true))
    }

    /// The `iloc` parser, then the resolution of every item with `data` as the file. Every item
    /// gets an `iloc` reference to its neighbours, so construction method 2 is followed.
    ///
    /// # Errors
    ///
    /// `Malformed`, limit errors.
    pub fn iloc(data: &[u8], limits: &Limits) -> Result<(), Error> {
        with(data, limits, |ctx| {
            let idat = Some(Extent {
                offset: 0,
                length: len(data),
            });
            let mut locs = iloc::parse_bytes(data, limits, ctx.budget)?;
            locs.sort_unstable_by_key(|l| l.id);
            let mut refs = ctx.budget.vec::<Reference>(locs.len())?;
            let mut fwd = ctx.budget.vec::<(u32, u32)>(locs.len())?;
            for (i, l) in locs.iter().enumerate() {
                let mut to = ctx.budget.vec::<u32>(3)?;
                to.extend(locs.iter().skip(i.saturating_sub(1)).take(3).map(|n| n.id));
                fwd.push((l.id, i as u32));
                refs.push(Reference {
                    kind: FourCc(*b"iloc"),
                    from: l.id,
                    to,
                });
            }
            fwd.sort_unstable();
            iloc::resolve_all(&mut locs, &refs, &fwd, idat, ctx).map(|_| ())
        })
    }

    /// The `iinf` parser (a box whose payload is `data`).
    ///
    /// # Errors
    ///
    /// `Malformed`, limit errors.
    pub fn iinf(data: &[u8], limits: &Limits) -> Result<(), Error> {
        with(data, limits, |ctx| {
            iinf::parse(ctx, 0..len(data), 1).map(|_| ())
        })
    }

    /// The `iref` parser.
    ///
    /// # Errors
    ///
    /// `Malformed`, limit errors.
    pub fn iref(data: &[u8], limits: &Limits) -> Result<(), Error> {
        with(data, limits, |ctx| {
            iref::parse(ctx, 0..len(data), 1, &mut 0).map(|_| ())
        })
    }

    /// The `ipma` parser.
    ///
    /// # Errors
    ///
    /// `Malformed`, limit errors.
    pub fn ipma(data: &[u8], limits: &Limits) -> Result<(), Error> {
        let cancel = AtomicBool::new(false);
        let budget = Budget::new(limits, &cancel);
        iprp::parse_ipma(data, limits, &budget, &mut 0).map(|_| ())
    }

    /// The `ipco` parser: every property parser, on the children of `data`.
    ///
    /// # Errors
    ///
    /// Limit errors (a bad property is a value, not an error).
    pub fn ipco(data: &[u8], limits: &Limits) -> Result<(), Error> {
        with(data, limits, |ctx| {
            iprp::parse_ipco(ctx, 0..len(data), 1).map(|_| ())
        })
    }

    /// The `grid` payload parser.
    ///
    /// # Errors
    ///
    /// `Malformed`, limit errors.
    pub fn grid(data: &[u8], limits: &Limits) -> Result<(), Error> {
        derived::parse_grid(data, limits).map(|_| ())
    }

    /// The `iovl` payload parser.
    ///
    /// # Errors
    ///
    /// `Malformed`, limit errors.
    pub fn iovl(data: &[u8], limits: &Limits) -> Result<(), Error> {
        let cancel = AtomicBool::new(false);
        let budget = Budget::new(limits, &cancel);
        derived::parse_iovl(data, limits, &budget).map(|_| ())
    }

    /// The `tmap` payload parser.
    ///
    /// # Errors
    ///
    /// `Malformed`.
    pub fn tmap(data: &[u8], _limits: &Limits) -> Result<(), Error> {
        derived::parse_tmap(data).map(|_| ())
    }

    /// The `mini` parser on the payload `data` (the `ftyp` minor version is `avif` for an even
    /// length and `heic` for an odd one), then the item graph built from it and every item
    /// looked at.
    ///
    /// # Errors
    ///
    /// `Malformed`, `Unsupported`, limit errors.
    pub fn mini(data: &[u8], limits: &Limits) -> Result<(), Error> {
        let cancel = AtomicBool::new(false);
        let budget = Budget::new(limits, &cancel);
        let src = Src {
            bytes: data,
            base: 0,
            file_len: len(data),
        };
        let ctx = Ctx::new(&src, &budget, limits);
        let hdr = BoxHeader {
            kind: FourCc(*b"mini"),
            uuid: None,
            start: 0,
            header_len: 0,
            end: len(data),
        };
        let brand = if data.len() & 1 == 0 {
            *b"avif"
        } else {
            *b"heic"
        };
        let ftyp = Ftyp {
            major: FourCc(*b"mif3"),
            minor_version: u32::from_be_bytes(brand),
            compatible: Vec::new(),
        };
        let (m, mi) = super::mini::parse(&ctx, &hdr, &ftyp)?;
        let h = graph::build(&ctx, ftyp, false, m, Some(mi), &mut SliceFetch(data))?;
        for it in &h.items {
            let _ = h.image(it.id);
            let _ = h.item_bytes(data, it.id, limits, &cancel);
        }
        let _ = h.metadata(data, h.primary, limits, &cancel);
        Ok(())
    }

    /// The `ftyp` parser.
    ///
    /// # Errors
    ///
    /// `Malformed`, limit errors.
    pub fn ftyp(data: &[u8], limits: &Limits) -> Result<(), Error> {
        let cancel = AtomicBool::new(false);
        let budget = Budget::new(limits, &cancel);
        super::ftyp::parse(data, &budget).map(|_| ())
    }

    /// `parse_movie`, then for every track a walk of all samples, `sync_samples(4)` (for a fragmented file only on tracks of at most 65 536 samples) and
    /// `sync_sample_at` at `i64::MIN`, 0 and `i64::MAX`, all over `Cursor::new(data)`.
    ///
    /// # Errors
    ///
    /// The error of `parse_movie` (a failing walk is not an error here).
    pub fn movie(data: &[u8], limits: &Limits) -> Result<(), Error> {
        let cancel = AtomicBool::new(false);
        let m = parse_movie(data, limits, &cancel)?;
        for t in &m.tracks {
            let walk = t.samples(std::io::Cursor::new(data), limits, &cancel);
            let _ = walk.take(1 << 16).take_while(Result::is_ok).count();
            // Fragment runs can overlap, so a walk to the end is bounded only by the cancel flag.
            if t.sample_count <= 1 << 16 || !m.fragmented {
                let _ = t.sync_samples(std::io::Cursor::new(data), 4, limits, &cancel);
            }
            if t.sample_count <= 1 << 16 {
                for at in [i64::MIN, 0, i64::MAX] {
                    let _ = t.sync_sample_at(std::io::Cursor::new(data), at, limits, &cancel);
                }
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    fn bx(kind: &[u8; 4], body: &[u8]) -> Vec<u8> {
        let mut v = ((body.len() as u32) + 8).to_be_bytes().to_vec();
        v.extend_from_slice(kind);
        v.extend_from_slice(body);
        v
    }

    fn cat(parts: &[&[u8]]) -> Vec<u8> {
        parts.concat()
    }

    /// An AVIF-like file: primary `av01` item 1 (`ispe` 10 x 20, `av1C`, `irot` 90) with data
    /// in `mdat`, an Exif item 2 (`cdsc` to 1) in two extents, and `extra` meta children.
    fn sample(extra: &[u8]) -> Vec<u8> {
        let ftyp = bx(b"ftyp", b"avif\0\0\0\0avifmif1");
        let build = |mdat_at: u32| {
            let hdlr = bx(b"hdlr", &cat(&[&[0; 8], b"pict", &[0; 13]]));
            let pitm = bx(b"pitm", &[0, 0, 0, 0, 0, 1]);
            let infe1 = bx(b"infe", &cat(&[&[2, 0, 0, 0, 0, 1, 0, 0], b"av01", &[0]]));
            let infe2 = bx(b"infe", &cat(&[&[2, 0, 0, 1, 0, 2, 0, 0], b"Exif", &[0]]));
            let iinf = bx(b"iinf", &cat(&[&[0, 0, 0, 0, 0, 2], &infe1, &infe2]));
            // v0, offset 4 length 4: item 1 one extent (mdat_at, 3); item 2 two extents.
            let mut iloc = vec![0, 0, 0, 0, 0x44, 0, 0, 2];
            iloc.extend_from_slice(&[0, 1, 0, 0, 0, 1]);
            iloc.extend_from_slice(&mdat_at.to_be_bytes());
            iloc.extend_from_slice(&3u32.to_be_bytes());
            iloc.extend_from_slice(&[0, 2, 0, 0, 0, 2]);
            iloc.extend_from_slice(&(mdat_at + 3).to_be_bytes());
            iloc.extend_from_slice(&5u32.to_be_bytes());
            iloc.extend_from_slice(&(mdat_at + 8).to_be_bytes());
            iloc.extend_from_slice(&2u32.to_be_bytes());
            let iloc = bx(b"iloc", &iloc);
            // cdsc: item 2 -> item 1
            let cdsc = bx(b"cdsc", &[0, 2, 0, 1, 0, 1]);
            let iref = bx(b"iref", &cat(&[&[0; 4], &cdsc]));
            let ispe = bx(b"ispe", &[0, 0, 0, 0, 0, 0, 0, 10, 0, 0, 0, 20]);
            let av1c = bx(b"av1C", &[0x81, 0, 0x4c, 0, 0x0a]);
            let irot = bx(b"irot", &[1]);
            let ipco = bx(b"ipco", &cat(&[&ispe, &av1c, &irot]));
            let ipma = bx(b"ipma", &[0, 0, 0, 0, 0, 0, 0, 1, 0, 1, 3, 1, 0x82, 0x83]);
            let iprp = bx(b"iprp", &cat(&[&ipco, &ipma]));
            bx(
                b"meta",
                &cat(&[&[0; 4], &hdlr, &pitm, &iinf, &iloc, &iref, &iprp, extra]),
            )
        };
        let at = (ftyp.len() + build(0).len() + 8) as u32;
        // Exif item bytes: offset 0 then "MM".. (10 bytes in two extents: 5 + 2 are used).
        let mdat = bx(b"mdat", &[1, 2, 3, 0, 0, 0, 0, 0, b'M', b'M']);
        cat(&[&ftyp, &build(at), &mdat])
    }

    fn lim() -> (Limits, AtomicBool) {
        (Limits::default(), AtomicBool::new(false))
    }

    #[test]
    fn parse_graph_and_bytes() {
        let (l, c) = lim();
        let file = sample(&[]);
        let h = parse(&file, &l, &c).unwrap();
        assert_eq!(h.ftyp.major, FourCc(*b"avif"));
        assert_eq!(h.primary, 1);
        assert_eq!(h.items.len(), 2);
        assert_eq!(h.properties.len(), 3);
        let img = h.primary_image().unwrap();
        assert_eq!((img.width, img.height), (10, 20));
        assert_eq!(img.transforms, vec![Transform::Rotate(90)]);
        assert_eq!(img.display_size(), Some((20, 10)));
        assert_eq!(img.exif, vec![2]);
        assert!(matches!(
            img.kind,
            ImageKind::Coded {
                codec: Codec::Av1,
                ..
            }
        ));
        assert_eq!(h.top_level_images(), vec![1]);
        let b = h.item_bytes(&file, 1, &l, &c).unwrap();
        assert!(matches!(b, std::borrow::Cow::Borrowed(_)));
        assert_eq!(&b[..], &[1, 2, 3]);
        assert_eq!(h.item_bytes(&file, 9, &l, &c), Err(Error::NoSuchItem(9)));
    }

    #[test]
    fn two_extent_item_is_a_charged_copy_and_metadata_strips_the_offset() {
        let (l, c) = lim();
        let file = sample(&[]);
        let h = parse(&file, &l, &c).unwrap();
        let b = h.item_bytes(&file, 2, &l, &c).unwrap();
        assert!(matches!(b, std::borrow::Cow::Owned(_)));
        assert_eq!(b.len(), 7);
        // The extents hold [0,0,0,0,0] + "MM": offset 0, so the TIFF bytes are [0, M, M].
        let m = h.metadata(&file, 1, &l, &c).unwrap();
        assert_eq!(m.exif.as_deref(), Some(&[0, b'M', b'M'][..]));
        assert_eq!(m.xmp, None);
        let tiny = Limits {
            max_alloc: 2,
            ..Limits::default()
        };
        assert_eq!(
            h.item_bytes(&file, 2, &tiny, &c),
            Err(Error::LimitExceeded(Limit::Alloc))
        );
        let small = Limits {
            max_metadata_bytes: 1,
            ..Limits::default()
        };
        assert_eq!(
            h.metadata(&file, 1, &small, &c),
            Err(Error::LimitExceeded(Limit::MetadataBytes))
        );
    }

    #[test]
    fn unknown_meta_children_are_kept_as_ranges() {
        let (l, c) = lim();
        let ipro = bx(b"ipro", &[0, 0, 0, 0, 0, 0]);
        let file = sample(&ipro);
        let h = parse(&file, &l, &c).unwrap();
        assert_eq!(h.other_boxes.len(), 1);
        let (kind, e) = h.other_boxes[0];
        assert_eq!(kind, FourCc(*b"ipro"));
        assert_eq!(Heif::slice(&file, e), Some(&ipro[..]));
        assert_eq!(read(Cursor::new(&file[..]), &l, &c).unwrap(), h);
        assert!(parse(&sample(&[]), &l, &c).unwrap().other_boxes.is_empty());
    }

    #[test]
    fn read_equals_parse() {
        let (l, c) = lim();
        let file = sample(&[]);
        let a = parse(&file, &l, &c).unwrap();
        let b = read(Cursor::new(&file[..]), &l, &c).unwrap();
        assert_eq!(a, b);
        assert_eq!(a.stats, b.stats);
        assert!(a.stats.boxes > 10);
    }

    #[test]
    fn top_level_errors() {
        let (l, c) = lim();
        assert_eq!(parse(&[], &l, &c).err(), Some(Error::UnknownFormat));
        assert_eq!(
            parse(b"\0\0\0\x08free", &l, &c).err(),
            Some(Error::UnknownFormat)
        );
        let ftyp = bx(b"ftyp", b"heic\0\0\0\0");
        assert_eq!(parse(&ftyp, &l, &c).err(), Some(Error::NoMeta));
        assert_eq!(
            read(Cursor::new(&ftyp[..]), &l, &c).err(),
            Some(Error::NoMeta)
        );
        // A `meta` that runs past the end of the file.
        let mut cut = sample(&[]);
        let at = cut.windows(4).position(|w| w == b"meta").unwrap();
        cut.truncate(at + 8);
        assert_eq!(parse(&cut, &l, &c).err(), Some(Error::Truncated));
        assert_eq!(
            read(Cursor::new(&cut[..]), &l, &c).err(),
            Some(Error::Truncated)
        );
        // After `meta`, a box that runs past the end ends the walk; the item data fails.
        let mut cut = sample(&[]);
        cut.truncate(cut.len() - 3);
        let h = parse(&cut, &l, &c).unwrap();
        assert_eq!(h.item_bytes(&cut, 2, &l, &c), Err(Error::Truncated));
        assert_eq!(read(Cursor::new(&cut[..]), &l, &c).unwrap(), h);
        // A `moov` without `meta` is an empty item graph.
        let mut seq = bx(b"ftyp", b"avis\0\0\0\0avis");
        seq.extend_from_slice(&bx(b"moov", &[]));
        let h = parse(&seq, &l, &c).unwrap();
        assert!(h.has_moov && h.items.is_empty() && h.primary == 0);
        assert_eq!(read(Cursor::new(&seq[..]), &l, &c).unwrap(), h);
        // A truncated box after `moov` ends the walk.
        seq.extend_from_slice(&[0, 0, 0, 0x40, b'm', b'd', b'a', b't']);
        let h = parse(&seq, &l, &c).unwrap();
        assert!(h.has_moov);
        assert_eq!(read(Cursor::new(&seq[..]), &l, &c).unwrap(), h);
        // Two meta boxes.
        let mut two = sample(&[]);
        let meta_only = bx(
            b"meta",
            &cat(&[&[0; 4], &bx(b"hdlr", &cat(&[&[0; 8], b"pict", &[0; 13]]))]),
        );
        two.extend_from_slice(&meta_only);
        assert_eq!(parse(&two, &l, &c).err(), Some(Error::Malformed("meta")));
    }

    #[test]
    fn meta_must_start_with_pict_hdlr() {
        let (l, c) = lim();
        let ftyp = bx(b"ftyp", b"heic\0\0\0\0");
        let bad = bx(
            b"meta",
            &cat(&[&[0; 4], &bx(b"hdlr", &cat(&[&[0; 8], b"vide", &[0; 13]]))]),
        );
        assert_eq!(
            parse(&cat(&[&ftyp, &bad]), &l, &c).err(),
            Some(Error::Malformed("hdlr"))
        );
        let first_pitm = bx(b"meta", &cat(&[&[0; 4], &bx(b"pitm", &[0, 0, 0, 0, 0, 1])]));
        assert_eq!(
            parse(&cat(&[&ftyp, &first_pitm]), &l, &c).err(),
            Some(Error::Malformed("hdlr"))
        );
    }

    #[test]
    fn limits_and_cancel() {
        let (l, c) = lim();
        let file = sample(&[]);
        let few = Limits {
            max_boxes: 3,
            ..Limits::default()
        };
        assert_eq!(
            parse(&file, &few, &c).err(),
            Some(Error::LimitExceeded(Limit::Boxes))
        );
        let flat = Limits {
            max_depth: 1,
            ..Limits::default()
        };
        assert_eq!(
            parse(&file, &flat, &c).err(),
            Some(Error::LimitExceeded(Limit::Depth))
        );
        let none = Limits {
            max_alloc: 0,
            ..Limits::default()
        };
        assert_eq!(
            parse(&file, &none, &c).err(),
            Some(Error::LimitExceeded(Limit::Alloc))
        );
        let meta = Limits {
            max_meta_bytes: 16,
            ..Limits::default()
        };
        assert_eq!(
            read(Cursor::new(&file[..]), &meta, &c).err(),
            Some(Error::LimitExceeded(Limit::MetaBytes))
        );
        let stop = AtomicBool::new(true);
        assert_eq!(parse(&file, &l, &stop).err(), Some(Error::Cancelled));
    }

    #[test]
    fn size_zero_mdat_and_uuid_boxes_are_skipped() {
        let (l, c) = lim();
        let mut file = sample(&[]);
        file.extend_from_slice(&bx(b"free", &[9; 5]));
        let mut uuid = ((8 + 16 + 2) as u32).to_be_bytes().to_vec();
        uuid.extend_from_slice(b"uuid");
        uuid.extend_from_slice(&[7; 16]);
        uuid.extend_from_slice(&[1, 2]);
        file.extend_from_slice(&uuid);
        // A final box with size 0 runs to the end of the file.
        file.extend_from_slice(&[0, 0, 0, 0, b'm', b'd', b'a', b't', 1, 2, 3]);
        let h = parse(&file, &l, &c).unwrap();
        assert_eq!(h.items.len(), 2);
        assert_eq!(read(Cursor::new(&file[..]), &l, &c).unwrap(), h);
    }

    #[test]
    fn item_errors_do_not_fail_the_parse() {
        let (l, c) = lim();
        // iloc item 1 extent beyond the file: rewrite the sample's mdat offset.
        let mut file = sample(&[]);
        let n = file.len();
        let h = parse(&file, &l, &c).unwrap();
        let ext = h
            .item(1)
            .unwrap()
            .data
            .as_ref()
            .unwrap()
            .first()
            .copied()
            .unwrap();
        // Find the 4-byte offset in the file and make it huge.
        let pat = (ext.offset as u32).to_be_bytes();
        let at = file.windows(4).position(|w| w == pat).unwrap();
        file[at..at + 4].copy_from_slice(&((n as u32) - 1).to_be_bytes());
        let h = parse(&file, &l, &c).unwrap();
        assert_eq!(h.item(1).unwrap().data, Err(ItemError::OutOfRange));
        assert_eq!(h.item_bytes(&file, 1, &l, &c), Err(Error::Truncated));
        assert!(h.primary_image().is_ok());
    }

    #[test]
    fn fuzz_entry_points_do_not_panic_on_short_input() {
        let l = Limits::default();
        let inputs: [&[u8]; 4] = [&[], &[0], &[0, 0, 0, 0, 0, 0, 0, 1], &[0xff; 40]];
        for data in inputs {
            let _ = __fuzz::boxes(data, &l);
            let _ = __fuzz::iloc(data, &l);
            let _ = __fuzz::iinf(data, &l);
            let _ = __fuzz::iref(data, &l);
            let _ = __fuzz::ipma(data, &l);
            let _ = __fuzz::ipco(data, &l);
            let _ = __fuzz::grid(data, &l);
            let _ = __fuzz::iovl(data, &l);
            let _ = __fuzz::tmap(data, &l);
            let _ = __fuzz::ftyp(data, &l);
            let _ = __fuzz::movie(data, &l);
        }
    }
}
