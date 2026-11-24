# isobmff-rs design (build spec)

isobmff-rs parses ISO-BMFF containers: HEIF (HEIC), AVIF and MP4/MOV. It decodes no
pixels. It gives an item graph (primary image, tiles, alpha, depth, gain map, thumbnails,
metadata) where every coded item is a codec configuration record plus byte ranges of the file.
hevc-rs and av1-rs decode those bytes; exif-rs parses the Exif and XMP bytes. For MP4, MOV, 3GP and
AVIS files it gives the tracks and the byte ranges of their keyframes ("MP4/MOV tracks" below).

This page is the spec. Code, tests, oracles and docs follow it. When the code must differ, change
this page in the same PR.

## Rules (from rust-ports docs/conventions.md)

- Package `isobmff`, edition 2024, toolchain 1.99.1, `Cargo.lock` committed, no dependencies.
- `#![forbid(unsafe_code)]`. A container parser has no SIMD kernel and no hot loop that needs
  `unsafe` (docs/safety.md says so). So there is no `ISOBMFF_SIMD` variable and no tier tests.
- Non-test code denies `clippy::unwrap_used`, `expect_used`, `panic`, `todo`, `unimplemented`,
  and also `clippy::indexing_slicing` and `clippy::arithmetic_side_effects` (all index and size
  arithmetic on input values is checked; use `get`, `checked_*`, `split_at_checked`).
- Never panic on any input. Never trust a size: every box, extent and count is checked against
  its parent, the input length and `Limits` before use.
- Never allocate from an input size before it passes `Limits`. Every `Vec` whose length comes from
  the input is made by `Budget::vec` (charge, then `try_reserve_exact`), never `vec![..; n]`,
  `Vec::with_capacity(n)` or `collect()` on an input-sized iterator without a charge first.
- No threads: the parse is a short serial walk. `Limits::max_threads` exists (every dobbo-ca
  crate has it) and is never used to spawn: every call runs on the calling thread.

## Module layout

```
src/
  lib.rs        crate docs (Resource limits section + doc-test), re-exports, parse(), read()
  limits.rs     Limits + Default
  error.rs      Error, Limit, Error::class()
  budget.rs     Budget: max_alloc ledger + cancel flag (same shape as exif-rs src/budget.rs)
  fourcc.rs     FourCc
  boxes.rs      BoxHeader, Boxes iterator (general box reader, used by every parser)
  ftyp.rs       Ftyp, brands
  meta/
    mod.rs      Meta: hdlr, pitm, iinf, iloc, idat, iref, iprp, grpl assembled
    iinf.rs     iinf + infe v0..v3
    iloc.rs     iloc v0..v2 -> absolute file extents (construction method 2 resolved in a second pass)
    iref.rs     iref v0/v1
    iprp.rs     ipco (property boxes, kept as ranges) + ipma; other children kept as ranges
    grpl.rs     grpl entity groups (altr, ...)
  props/
    mod.rs      Property enum, parse dispatch by 4cc
    image.rs    ispe pixi pasp irot imir clap colr auxC lsel a1op a1lx
    config.rs   hvcC av1C (parsed header + raw range)
  derived.rs    grid, iovl, tmap payload parsers
  mini.rs       the `mini` box (low-overhead HEIF/AVIF): bit header -> the same `Meta` as `meta`
  graph.rs      Image, ImageKind, Transform, Aux, GainMap, the item graph API
  metadata.rs   Exif / XMP item extraction
  movie/        MP4/MOV tracks: moov, sample tables kept as ranges, the sample walk (see "MP4/MOV tracks")
```

## Input model

All positions are **absolute file offsets**. `Extent { offset: u64, length: u64 }` is a range of
the input file. Construction method 1 (idat) extents are translated to absolute offsets of the
`idat` payload, so a caller needs only the file. The parsed `Heif` holds no borrow of the input.

- `isobmff::parse(data: &[u8], limits, cancel) -> Result<Heif, Error>`: the whole file is in
  memory. `Heif::slice(data, extent) -> Option<&[u8]>` gives a borrowed slice (zero copy).
- `isobmff::read<R: Read + Seek>(r, limits, cancel) -> Result<Heif, Error>`: reads only the
  top-level box headers, `ftyp` and the whole `meta` (or `mini`) box (at most `Limits::max_meta_bytes`, the
  copy is charged to `max_alloc`); it seeks over `mdat`. The `Heif` is the same as `parse` on the
  same bytes (a fuzz target and a test check this). The caller reads extents itself.

## Box reader (`boxes.rs`)

General ISO-BMFF reader, also used by the MP4/MOV tracks (`moov/trak/mdia/minf/stbl`).

- Header: `size: u32`, `type: [u8; 4]`. `size == 1`: a `largesize: u64` follows. `size == 0`:
  the box runs to the end of its parent (for a top-level box: the end of the file). `type ==
  'uuid'`: a 16-byte extended type follows. A size smaller than the header, or an end past the
  parent end, is `Error::Malformed("box size")` — except a top-level box that runs past the end of
  the file, which is `Error::Truncated` (but see "What `parse` checks at the top level": after
  `meta` it ends the walk).
- `BoxHeader { kind: FourCc, uuid: Option<[u8; 16]>, start: u64, header_len: u8 (8, 16, 24 or 32),
  end: u64 }` with `payload() -> Range<u64>`.
- FullBox: `version: u8`, `flags: u24` after the header (`full_header(payload) -> (version, flags,
  rest)`).
- `Boxes::new(range, depth, ctx)` iterates the children of a range. Each `next()` counts one box
  against `Limits::max_boxes` (all boxes of the call together), checks `depth <=
  Limits::max_depth` when it descends, and checks the cancel flag every 1024 boxes.
- Unknown boxes are skipped. A known box that is too short for its fields is `Malformed(<4cc>)`.

## What `parse` checks at the top level

1. The first box is `ftyp` (else `Error::UnknownFormat`). Brands are kept as given.
2. Top-level boxes are walked to the end of the file. `meta` must appear at most once (else
   `Malformed("meta")`). Neither `meta` nor `moov`: `Error::NoMeta`. `moov` without `meta` (an
   image sequence, as libheif accepts it) is an empty item graph: `has_moov` set, no items,
   primary 0. `moov` is noted (`Heif::has_moov`), not parsed by `parse` ("MP4/MOV tracks" has `parse_movie`). Other boxes (`mdat`, `free`,
   `skip`, `uuid`, ...) are skipped. Once `meta` or `moov` has been found, a top-level box with an
   incomplete header or that runs past the end of the file (libheif and libavif stop reading
   there) ends the walk instead of being `Truncated`; item data past the end of the file then
   fails per item (`item_bytes` gives `Truncated`). Before `meta` and `moov` it is still `Truncated`.
   A top-level `mini` box (ISO/IEC 23008-12 Annex O, brand `mif3`) takes the place of `meta` (see
   "The `mini` box" below). A file with two `mini` boxes, or with a `mini` and a `meta`, is
   `Malformed("mini")`. There is no brand check: any top-level `mini` is used (libheif does the
   same; libavif needs `mif3` and no `avif` brand). The truncation rule above applies after
   `mini` as after `meta`.
3. `meta` is a FullBox (version 0). Its `hdlr` must be the first child with handler `pict` (else
   `Malformed("hdlr")`).

## Items (`meta/`)

| Box | Versions | Kept |
|---|---|---|
| `pitm` | 0 (u16 id), 1 (u32 id) | `primary` |
| `iinf` | 0 (u16 count), any other version (u32 count, as libheif) | the first entry_count children; non-`infe` skipped; fewer is Malformed |
| `infe` | 2 (u16 id), 3 (u32 id); 0/1 accepted, kept as item type `\0\0\0\0` | id, protection index, item_type, item_name, `mime`: content_type + content_encoding, `uri `: uri_type; `hidden` = flags & 1 |
| `iloc` | 0, 1, 2 | per item: construction method, data_reference_index, base_offset, extents (method 2 also the extent_index of each) |
| `idat` | - | payload range, for construction method 1 |
| `iref` | 0 (u16 ids), 1 (u32 ids) | `(type, from, [to...])` in file order |
| `iprp`/`ipco` | - | property boxes in order (index 1..n), each as `Property` |
| `ipma` | 0/1, flags & 1 | per item: `(property index, essential)` in order |
| `grpl` | - | entity groups: `(type, group id, [entity ids])` (`altr` for AVIF gain maps) |

iloc details: `offset_size`, `length_size`, `base_offset_size` in {0, 4, 8} (else Malformed);
`index_size` (v1/v2) in {0, 4, 8}; item id u16 (v0/v1) or u32 (v2); construction_method (v1/v2)
low 4 bits of a u16. Absolute offset of an extent = base_offset + extent_offset (method 0) or
idat payload start + base_offset + extent_offset (method 1); overflow or an end past the file
(method 0) or past the idat payload (method 1) is recorded as an item error (below), not a parse
error. Extent length 0 means "to the end of the file" (method 0) or "to the end of idat"
(method 1). A data_reference_index other than 0 is not supported: the item gets
`ItemError::Unsupported`. An item id that appears twice in iloc, iinf or ipma is `Malformed`.

Construction method 2 (item offset, ISO 14496-12 8.11.3) is resolved after every method 0 and 1
item and before the payloads of `grid`, `iovl` and `tmap` items are read, so `Item::data` is still
absolute file extents and `item_bytes` and the derived parsers need no change. The source of an
extent is `to[extent_index - 1]` of the one `iref` of type `iloc` whose `from` is the item; the
`extent_index` of each extent is kept (1 when `index_size` is 0, as the spec says). The range
`[base_offset + extent_offset, + extent_length)` of the concatenated data of the source item (length
0: to its end) is cut into absolute extents along the source extents. A source that is itself
method 2 is resolved first, at most 8 method-2 items in a row. Errors are item errors: no or more
than one `iloc` reference, a chain of more than 8 items or a cycle: `Unsupported("construction
method 2")`; an extent_index of 0 or past `to`, or a range past the end of the source:
`OutOfRange`; a source with no `iloc` entry: `NoLocation`; a source with an error gives that error.
The chain length of each item is stored with its result (0 for methods 0 and 1, else 1 + the longest source chain) and a cycle is found by an in-progress mark, so the result of an item does not depend on the order items are visited in. Every extent of a method-2
item and every source extent visited counts against `max_extents` (`Limit::Extents` fails the
parse); the result vectors are charged through the `Budget`.

Item errors do not fail the parse: `Item::data` is `Result<Vec<Extent>, ItemError>`. A request for
that item's bytes gives the error. This matches libheif, which fails only when an item's data is
read.

## The `mini` box (`mini.rs`)

A top-level `mini` box (ISO/IEC 23008-12 Annex O, brand `mif3`) holds a whole image in one bit-packed
header and its chunks, with no `meta`. `mini::parse` reads it and builds the `Meta` that a `meta`
box would give (items, `iloc`, `iref`, `ipco`, `ipma`, `grpl`), so `graph::build` and every `Heif`
accessor are the ones of a normal file. `Heif::mini: Option<Mini>` also keeps every parsed header
field and the `Extent` of every chunk (all absolute file offsets, a missing chunk has length 0).

**Syntax.** The bits are read most significant first, as libheif 1.23.7 `mini.cc` (the working draft
of the spec) reads them. It differs from libavif v1.4.2 in two places, which this crate follows
libheif in (and docs/parity.md says where the oracles then disagree):

- `matrix_coefficients` is always 8 bits when `explicit_cicp_flag` is set (libavif reads it only
  when `chroma_subsampling != 0`).
- `gainmap_dimension_same_as_main_item_flag` is present before the gain map width and height; when
  set, they are the main size (libavif v1.4.2 does not have the flag; its master does).

Fields in order: `version` 2 (must be 0, else `Error::Unsupported("mini")`); flags
`explicit_codec_types float full_range alpha explicit_cicp hdr icc exif xmp`; `chroma_subsampling` 2;
`orientation_minus1` 3; `large_dimensions`; width and height minus 1 (7 or 15 bits each);
chroma centring flags (horizontal for 4:2:0 and 4:2:2, vertical for 4:2:0); bit depth (float:
`log2 - 4` in 2 bits for 16, 32 or 64, the value 3 is `Malformed("mini")`; else a high bit depth
flag and `depth - 9` in 3 bits, default 8); `alpha_is_premultiplied` when alpha; `colour_primaries`,
`transfer_characteristics`, `matrix_coefficients` (8 bits each) when explicit, else 1 (2 with an ICC
profile), 13 (2 with an ICC profile) and 6 (2 for 4:0:0); `infe_type` and `codec_config_type` (32 bits each)
when explicit; with `hdr`: `gainmap_flag`, and with it the gain map fields (same-size flag, size,
matrix, full range, chroma, float and depth as above, `tmap_icc_flag`, `tmap_explicit_cicp_flag`
and its CICP and range, default 1 / 13 / 6 full range), then the HDR property block (six flags
`clli mdcv cclv amve reve ndwt`, then the present payloads in that order) for the main item and,
with a gain map, for the `tmap` item; `large_metadata` (only when icc, exif, xmp or a gain map),
`large_codec_config`, `large_item_data`; the chunk sizes (icc, tmap icc, gain map metadata, gain map
data and config, main config and data minus 1, alpha data and config, `exif_xmp_compressed`, exif
and xmp); padding to a byte. Padding bits and bytes after the last chunk are not checked. The
chunks follow in this order: main config, alpha config, gain map config (a config of size 0 is the
main one), ICC, `tmap` ICC, gain map metadata, alpha data, gain map data, main data, Exif, XMP.
The sum of the chunk sizes past the end of the payload is `Malformed("mini")` (`Truncated` when the
box itself runs past the end of the file). All sizes are fixed-width bit fields of at most 32
bits, so the header needs no limit of its own; chunk ends are checked sums and nothing is
allocated from a size.

**Codec.** With explicit codec types, the item type is `infe_type` and the configuration property
type is `codec_config_type` (`vvc1`/`vvcC` gives an unsupported image, with the graph). Without,
the `ftyp` minor version decides: `avif` gives `av01` / `av1C`, `heic` gives `hvc1` / `hvcC`; any
other is `Error::Unsupported("mini codec")` (libheif fails those files too, for example the VVC
`.hmg` files of Nokia). The configuration property is parsed from its chunk by the normal `hvcC`
and `av1C` parsers; a configuration of size 0 gives no property (the image then fails with
`Malformed("hvcC")` or `Malformed("av1C")`).

**Graph** (the ids of libheif; item 1 is the primary item):

| Item | Type | When | Data | Notes |
|---|---|---|---|---|
| 1 | `av01` / `hvc1` / explicit | always | main data | |
| 2 | as 1 | alpha flag and `alpha_item_data_size > 0` | alpha data | `auxl` 2 to 1; `prem` 1 to 2 when `alpha_is_premultiplied` (libheif 1.23.7 omits `prem`) |
| 3 | `tmap` | gain map | gain map metadata | `dimg` 3 to [1, 4] (or [1] without item 4), `altr` group [3, 1] (id 0); the derived payload is the byte 0 followed by the chunk, parsed by the normal `tmap` parser |
| 4 | as 1, hidden | gain map data > 0 | gain map data | |
| 6 | `Exif` | exif flag | Exif chunk | `cdsc` 6 to 1; no `exif_tiff_header_offset`: `Heif::metadata` returns the chunk as is |
| 7 | `mime` `application/rdf+xml` | xmp flag | XMP chunk | `cdsc` 7 to 1; `content_encoding` `deflate` when `exif_xmp_compressed` (the bytes stay as stored) |

Items 2, 6 and 7 are not hidden (libheif prints `hidden=0` for them). Item data are single extents
of the chunks; an empty gain map metadata chunk gives an item with no extents.

Properties are synthesised in `ipco` order main config, `ispe`, `pixi` (1 channel for 4:0:0, else 3),
`colr` `nclx`, `colr` ICC (an `Extent` of the ICC chunk), alpha config, `auxC` (alpha URN), the HDR
properties of the main item (`clli mdcv cclv amve ndwt`, as `PropValue::Other`; `reve` has none),
`irot`, `imir`, then the gain map image's config, `ispe`, `pixi`, `colr` (`nclx` 2 / 2 / gain map
matrix) and the `tmap` item's `ispe` (width and height swapped for orientations 5 to 8), `colr`
`nclx` (when `tmap_explicit_cicp` or no `tmap` ICC), `colr` ICC and HDR properties. libheif's
`free` placeholder properties are left out. A property without bytes of its own has the range
`{ offset: payload start, length: 0 }`; configuration and ICC properties have their chunk range.
Item 1 lists config, `ispe`, `pixi`, `colr` `nclx`, ICC, HDR properties, `irot`, `imir`; item 2
config, `ispe` (shared with item 1), `auxC`, `irot`, `imir`; item 4 the same transforms as item 1.
The config, `colr`, `auxC`, `irot` and `imir` associations are essential. The orientation maps as libheif does:
2 `imir` axis 1, 3 `irot` 180, 4 `imir` axis 0, 5 `irot` 270 and `imir` axis 1 (libavif: `irot` 90
and `imir` axis 0, the same transform), 6 `irot` 270, 7 `irot` 270 and `imir` axis 0, 8 `irot` 90.

`read` copies the `mini` box like `meta` (at most `max_meta_bytes`, charged to `max_alloc`).
`__fuzz::mini` runs the parser and the graph on a raw payload.

## Properties (`props/`)

Every `ipco` child becomes a `Property { kind: FourCc, range: Extent (payload), value: PropValue }`.
Parsing a property is lazy-safe: a known property that does not parse is `PropValue::Invalid` (the
graph then fails only the images that use it, with `Error::Malformed(<4cc>)`), except `pixi`,
`pasp` and `lsel`, which libheif treats as optional: the image is kept without them.

| 4cc | Value |
|---|---|
| `ispe` | FullBox v0: width u32, height u32 |
| `pixi` | FullBox v0: num_channels u8, bits_per_channel [u8] |
| `colr` | colour_type `nclx`: primaries u16, transfer u16, matrix u16, full_range (1 bit); `prof`/`rICC`: ICC profile as an `Extent` (zero copy) |
| `irot` | angle = (byte & 3) * 90, anticlockwise |
| `imir` | axis (byte & 1): 0 = top and bottom swap (vertical flip), 1 = left and right swap (horizontal flip) (23008-12:2022 6.5.12; libavif avif.h and libheif agree) |
| `clap` | 8 x u32: widthN/D, heightN/D, horizOffN/D (signed), vertOffN/D (signed) |
| `pasp` | hSpacing u32, vSpacing u32 |
| `auxC` | FullBox v0: aux_type (NUL-terminated string), aux_subtype bytes (Extent) |
| `lsel` | layer_id u16 |
| `rref` | FullBox v0/1: count (u32 when the payload is a non-empty multiple of 4, else u8), reference types; an image whose `rref` lists a type other than `thmb base cdsc dimg auxl prem` is `Unsupported("rref")` |
| `a1op` | op_index u8 |
| `a1lx` | flags & 1 -> u32 sizes, else u16; 3 layer sizes |
| `hvcC` | header fields (version, profile space/tier/idc, chroma_format_idc, bit depths, length_size_minus_one) + NAL arrays: `(nal_type, [Extent])`; whole payload as `Extent` |
| `av1C` | marker/version byte 0x81, seq_profile, seq_level_idx_0, tier, high_bitdepth, twelve_bit, monochrome, subsampling_x/y, chroma_sample_position, configOBUs as `Extent`; whole payload as `Extent` |
| other | `PropValue::Unknown` |

An essential property that this crate does not know (`Unknown` with `essential = true`) makes the
image `Unsupported` in the graph, as HEIF requires. An `hvc1` or `av01` image still needs its
`hvcC` or `av1C` in that case (else `Malformed`): libheif cannot set up its decoder without it and
drops the item.

## Derived images (`derived.rs`)

- `grid` (item data, 8 or 12 bytes): version 0; flags & 1 -> 32-bit output size; rows_minus_one u8,
  columns_minus_one u8, output_width, output_height. Tiles are the `dimg` references of the grid
  item, in order; their count must be rows x columns (else `Malformed("grid")`). Every tile must
  have the same ispe; output size <= tile size x count in each axis (libheif rule), and
  rows x columns <= `Limits::max_grid_tiles`.
- `iovl`: version 0; flags & 1 -> 32-bit fields; canvas fill RGBA 4 x u16; output_width/height;
  then per `dimg` input a horizontal and vertical offset (signed). The input count is the dimg
  count.
- `iden`: one dimg input.
- `tmap` (ISO 21496-1 gain map metadata item payload): item version u8 (0), `minimum_version`
  u16, `writer_version` u16, flags u8 (bit 7 multichannel, bit 6 use_base_colour_space, bit 2
  backward_direction, bit 3 common denominator), then the headroom and per-channel fractions
  (min, max, gamma, base offset, alternate offset; numerators of min, max and the offsets are
  signed). Bit 3 set: one shared u32 denominator first, then numerators only. The raw flags byte
  is kept. Zero denominators, a zero gamma numerator, max < min, or trailing bytes after a
  `writer_version` 0 payload are `Malformed("tmap")`. Unknown item version or `minimum_version`
  > 0: `Derived::Tmap(None)`, so `GainMap::metadata = None` (the payload extents are the item's
  `data`).
- Derived payloads are read while the graph is built (so that `read` needs no `Heif` method that
  takes the file): `Item::derived` is `Option<Result<Derived, Error>>`; the `Err` (malformed,
  `max_pixels`, `max_grid_tiles`) is what `Heif::image` gives for that item. Payloads over 4 MiB,
  grid payloads over 12 bytes, and payloads once 64 MiB of derived payloads were read for one
  graph are `Malformed`.

## Transforms

Transformative properties are `clap`, `irot`, `imir`. `Image::transforms` lists them in the
order of the image's ipma associations, not reordered (libheif applies them in ipma order; MIAF
allows at most one of each, in the order clap, irot, imir). A reader applies them in list order.
Descriptive properties after a transformative one are kept (libavif does the same).

- `irot`: `Transform::Rotate(deg)`, deg = (byte & 3) * 90, anticlockwise.
- `imir`: `Transform::Mirror(Axis::TopBottom)` for axis 0, `Axis::LeftRight` for axis 1.
- `clap`: `Transform::Clap(Clap)` with the 8 raw values. `Clap::crop(width, height) ->
  Option<Rect { x, y, width, height }>` uses exact fractions (rule of libavif `avif.c`, BSD-2):
  every D > 0, width N and height N >= 0, clap width and height integer, x = (W - clapW)/2 +
  horizOff and y = (H - clapH)/2 + vertOff integer and >= 0, and the rectangle inside W x H;
  else `None`. Compute in i128, so no input overflows.
- `Image::display_size() -> Option<(u32, u32)>`: start at ispe; apply each transform in order:
  clap with a `Some` crop sets the size to the crop size, a `None` crop leaves it (libheif then
  rounds; such files are listed in tests/parity/known-differences.tsv); irot 90 or 270 swaps;
  imir keeps. `None` when the image has no ispe. The parity oracle compares this with libheif
  `heif_image_handle_get_width/height`.


## Item graph (`graph.rs`)

```rust
pub struct Heif {
    pub ftyp: Ftyp,                 // major brand, minor version, compatible brands
    pub primary: u32,               // pitm
    pub items: Vec<Item>,           // iinf order
    pub properties: Vec<Property>,  // ipco order (index i+1)
    pub references: Vec<Reference>, // iref order
    pub groups: Vec<EntityGroup>,   // grpl order
    pub other_boxes: Vec<(FourCc, Extent)>, // other meta children (ipro, dinf, unknown), whole box
    pub iprp_other: Vec<(FourCc, Extent)>,  // iprp children other than ipco/ipma, whole box
    pub mini: Option<Mini>,         // the top-level `mini` box, when the file has one instead of `meta`
    pub has_moov: bool,
    pub stats: Stats,               // boxes visited, max depth
}
pub struct Item { pub id: u32, pub item_type: FourCc, pub name: String, pub content_type: String,
                  pub content_encoding: String, pub uri_type: String, pub hidden: bool, pub protection_index: u16,
                  pub data: Result<Vec<Extent>, ItemError>, pub construction_method: u8,
                  pub associations: Vec<Association>,
                  pub derived: Option<Result<Derived, Error>> }  // grid / iovl / tmap payload
pub struct Reference { pub kind: FourCc, pub from: u32, pub to: Vec<u32> }

impl Heif {
    pub fn primary_image(&self) -> Result<Image, Error>;
    pub fn image(&self, id: u32) -> Result<Image, Error>;
    pub fn item(&self, id: u32) -> Option<&Item>;
    pub fn item_bytes<'a>(&self, data: &'a [u8], id: u32, limits: &Limits, cancel: &AtomicBool)
        -> Result<Cow<'a, [u8]>, Error>;   // borrowed for one extent, charged copy otherwise
    pub fn metadata<'a>(&self, data: &'a [u8], image: u32, limits, cancel)
        -> Result<Metadata<'a>, Error>;    // (exif TIFF bytes, xmp bytes)
}
pub struct Image {
    pub id: u32,
    pub kind: ImageKind,           // Coded { codec, config } | Grid(Grid) | Overlay(Overlay) | Identity { input } | Unsupported(FourCc)
    pub width: u32, pub height: u32,          // ispe
    pub transforms: Vec<Transform>,           // Clap(Clap) | Rotate(u16) | Mirror(Axis), ipma order
    pub colour: Vec<Colour>,                  // nclx and/or ICC
    pub pixi: Option<Vec<u8>>, pub pasp: Option<(u32, u32)>,
    pub layer: Option<Layer>,                 // lsel / a1op / a1lx
    pub alpha: Option<u32>, pub depth: Option<u32>,
    pub aux: Vec<Aux>,                        // every auxl image: id, aux_type, subtype extent
    pub thumbnails: Vec<u32>,                 // thmb sources pointing at this image
    pub gain_map: Option<GainMap>,            // tmap (ISO 21496-1) or Apple aux gain map
    pub exif: Vec<u32>, pub xmp: Vec<u32>,    // cdsc items
    pub premultiplied: bool,                  // prem (this item is the source of a prem reference)
}
```

- Coded: `item_type` `hvc1` (HEVC, needs hvcC) or `av01` (AV1, needs av1C). Others (`jpeg`,
  `unci`, `hvt1`...) are `Unsupported(type)`. `config` is the parsed hvcC/av1C, `data` the item
  extents.
- alpha: an `auxl` reference from an item whose auxC type is `urn:mpeg:mpegB:cicp:systems:auxiliary:alpha`
  `urn:mpeg:hevc:2015:auxid:1` or `urn:mpeg:avc:2015:auxid:1`. depth: `urn:mpeg:hevc:2015:auxid:2` or
  `urn:mpeg:mpegB:cicp:systems:auxiliary:depth`. Others are in `aux` only. Only image items
  count as sources of `auxl` and `thmb` references (libheif). A grid's alpha may live on its
  tiles: `Image::alpha` of the grid covers only an alpha item linked to the grid itself (libheif
  reports alpha for a grid when any tile has it, libavif when every tile has it; the oracle
  looks at the tiles).
- gain map: (a) ISO 21496-1: a `tmap` item with `dimg` [base, gain map] that comes before the
  base in an `altr` group (libavif ignores a `tmap` listed after the base; AVIF/libavif and HEIF); (b) Apple: an `auxl` image with aux type
  `urn:com:apple:photo:2020:aux:hdrgainmap` (its headroom lives in the gain map's XMP; we give the
  XMP item ids). `GainMap { image: u32, tmap: Option<u32>, metadata: Option<GainMapMetadata>,
  source: GainMapSource }`.
- thumbnails: items with a `thmb` reference to this image.
- exif / xmp: items with a `cdsc` reference to this image; Exif is item type `Exif`, XMP is
  `mime` with content type `application/rdf+xml`.
- Metadata bytes: Exif item payload = `exif_tiff_header_offset: u32` (big endian) then that many
  bytes, then the TIFF header. `metadata()` gives the bytes from the TIFF header on (what
  `exif::Exif::parse` takes); a bad offset is `Malformed("Exif")`. XMP is the item bytes as is. The
  Exif item of a `mini` box has no offset field (libavif skips it too): `metadata()` gives its bytes
  as they are (`Heif::mini` is `Some`).

## MP4/MOV tracks (`movie/`)

isobmff-rs finds keyframes in MP4, MOV, 3GP and AVIS image sequences, so that hevc-rs and av1-rs can decode one and Drive can build a thumbnail. For a track it gives the codec configuration and the byte ranges of its first N sync samples, or of the sync sample nearest a time. It decodes no samples. `parse` and `read` (the HEIF item graph) are unchanged: `moov` stays "noted" there. A caller who wants both the item graph and the tracks of an AVIS file calls both. It reads `moov` and the movie fragments (`moof`) of fragmented files ("Fragmented MP4", below); `mvex` sets `Movie::fragmented`.

```
src/movie/
  mod.rs      Movie, Track, SampleEntry, Edit, Sample, parse_movie, read_movie, the moov/trak/mdia/minf/stbl/stsd parse
  stbl.rs     table headers: version, count, byte range, count checked against the payload
  frag.rs     trex, tfhd, tfdt, trun: one record per run, kept as ranges
  walk.rs     the sample walk (moov samples, then fragment runs): Samples, Track::samples, sync_samples, sync_sample_at
```

### Entry points and types

```rust
pub fn parse_movie(data: &[u8], limits: &Limits, cancel: &AtomicBool) -> Result<Movie, Error>;
pub fn read_movie<R: Read + Seek>(r: R, limits: &Limits, cancel: &AtomicBool) -> Result<Movie, Error>;

pub struct Movie {
    pub ftyp: Option<Ftyp>,                  // None: QuickTime file without ftyp
    pub timescale: u32, pub duration: u64,   // mvhd (v0 fields widened); 0 without mvhd
    pub tracks: Vec<Track>,                  // trak order
    pub fragmented: bool,                    // moov has mvex
    pub other_boxes: Vec<(FourCc, Extent)>,  // every top-level box and moov child not parsed, whole box, file order
    pub stats: Stats,
}
impl Movie {
    pub fn track(&self, id: u32) -> Option<&Track>;
    pub fn video_track(&self) -> Option<&Track>; // first trak whose handler is `vide` or `pict`
}

pub struct Track {
    pub id: u32, pub flags: u32,             // tkhd track_ID, tkhd flags
    pub tkhd: Extent,                        // tkhd payload
    pub matrix: [i32; 9],                    // tkhd matrix (rotation of a phone video)
    pub width: u32, pub height: u32,         // tkhd, raw 16.16 fixed point
    pub handler: FourCc,                     // mdia/hdlr handler_type
    pub timescale: u32, pub duration: u64,   // mdhd; timescale 0 is kept, callers check it before dividing
    pub entries: Vec<SampleEntry>,           // stsd children, index i+1 = sample description index
    pub edits: Vec<Edit>,                    // elst entries, empty without elst
    pub sample_count: u64,                   // stsz/stz2 count + every fragment run count
    pub tables: Vec<(FourCc, Extent)>,       // the parsed stbl tables, whole box (stts ctts stss stsc stsz stz2 stco co64)
    pub other_boxes: Vec<(FourCc, Extent)>,  // every box under trak not parsed, whole box
    /* private: table ranges and counts */
}
pub struct SampleEntry {
    pub kind: FourCc,                        // hvc1 hev1 avc1 avc3 av01 encv mp4a ...
    pub range: Extent,                       // whole box
    pub data_reference_index: u16,
    pub size: Option<(u16, u16)>,            // width, height of a visual entry
    pub boxes: Vec<Property>,                // children after the 78 visual fields; empty for non-visual entries
}
impl SampleEntry { pub fn config(&self) -> Option<&Property>; } // first child of kind hvcC, avcC, av1C or vvcC
pub struct Edit { pub segment_duration: u64, pub media_time: i64, pub rate_integer: i16, pub rate_fraction: i16 }

pub struct Sample {
    pub number: u64,         // 1-based decode order: moov samples, then fragment samples in file order
    pub range: Extent,       // absolute file range of the sample bytes
    pub dts: i64,            // decode time, media timescale, no edit list
    pub cto: i32,            // composition offset (ctts or trun), 0 when there is none
    pub sync: bool,
    pub entry: u32,          // sample description index (1-based) into Track::entries
    pub flags: Option<u32>,  // resolved fragment sample_flags; None for moov samples
}
impl Sample { pub fn pts(&self) -> i64 { self.dts.saturating_add(i64::from(self.cto)) } }

impl Track {
    pub fn samples<'a, R: Read + Seek>(&'a self, r: R, limits: &Limits, cancel: &'a AtomicBool)
        -> Samples<'a, R>;                    // Iterator<Item = Result<Sample, Error>>; None after the first Err
    pub fn sync_samples<R: Read + Seek>(&self, r: R, n: usize, limits: &Limits, cancel: &AtomicBool)
        -> Result<Vec<Sample>, Error>;        // the first n sync samples, by number
    pub fn sync_sample_at<R: Read + Seek>(&self, r: R, t: i64, limits: &Limits, cancel: &AtomicBool)
        -> Result<Option<Sample>, Error>;
}
```

- **Codec config of a sample:** `track.entries[sample.entry - 1].config()`. This is a `Property`, built by the same `parse_property` that `ipco` uses. `hvcC` gives `PropValue::Hvcc`, `av1C` gives `PropValue::Av1c`, and `avcC`/`vvcC` give `Unknown` with the payload `Extent`. `colr`, `pasp`, `clap` and the rest parse as they do in HEIF.
- **Byte source.** The sample walks read table bytes through `Read + Seek`. In-memory callers pass `Cursor::new(data)`, and `&mut reader` also works. The `Movie` holds no borrow and no table bytes.
- **Matrix, width, height.** `matrix` and the raw 16.16 `width` and `height` are `tkhd` values, so a caller can rotate a thumbnail without parsing `tkhd`; `tkhd` stays an `Extent` too.

### Top level

- **First box.** It must be one of `ftyp moov mdat free skip wide pnot`, else `UnknownFormat` (a QuickTime file may start with any of them). The first `ftyp` is parsed with `ftyp::parse`; a second one is kept in `other_boxes`.
- **moov count.** No `moov` is `Error::NoMoov` (class `no-moov`). Two `moov` boxes are `Malformed("moov")`.
- **cmov.** A `cmov` (compressed moov) is `Unsupported("cmov")`.
- **Damaged tail.** After `moov`, a top-level box with an incomplete header or one that runs past the end of the file ends the walk, as in HEIF. Before `moov` it is `Truncated`.
- **other_boxes.** Every other top-level box goes into `Movie::other_boxes` where it is met, and so does each `moov` child that is not parsed, at the place of `moov` in the file order: `mdat`, `free`, `meta`, `udta`, `mvex`, `sidx`, `styp`, `uuid`, and so on. The `moof` boxes are parsed, not listed; their children that are not parsed come last.
- **moof.** Every top-level `moof` is parsed after `moov`, in file order.

### Boxes parsed and boxes kept raw

| Box | Kept |
|---|---|
| `mvhd` v0/v1 | timescale, duration |
| `trak` | one `Track`; a duplicate or 0 track id is `Malformed("tkhd")` |
| `tkhd` v0/v1 | track_ID, flags, matrix, width, height, payload `Extent` |
| `edts/elst` v0/v1 | every entry as `Edit` (lossless, so no raw copy); other `edts` children are raw |
| `mdia/mdhd` v0/v1 | timescale, duration |
| `mdia/hdlr` | handler_type (a `minf/hdlr`, the QuickTime data handler, is raw) |
| `minf/dinf/dref` | `dinf` is kept raw in `Track::other_boxes`; the self-contained flag (`flags & 1`) of each `dref` entry is also read |
| `stbl/stsd` | every entry as a `SampleEntry`. An entry is visual when the handler is `vide`, `pict` or `auxv`. A visual body shorter than 78 bytes is `Malformed("stsd")`. A child tail shorter than 8 bytes (the QuickTime terminator) is ignored. |
| `stts ctts stss stsc stsz stz2 stco co64` | payload range of the entries plus entry count (below), and whole box in `Track::tables` |
| everything else (`udta`, `meta`, `tref`, `vmhd`, `sdtp`, `sbgp`, `sgpd`, `stps`, `subs`, `cslg`, `saiz`, `saio`, `senc`, `mvex`, ...) | `(FourCc, whole-box Extent)` in the `other_boxes` of the track (under `trak`) or of the movie (under `moov`) |

Each of `tkhd mdia mdhd hdlr minf stbl stsd stts stsc` is required, plus one of `stsz`/`stz2` and one of `stco`/`co64`. A missing one is `Malformed(<4cc>)` (`stsz` or `stco` when neither of the pair is there). A second copy of a table, or both `stsz`+`stz2` or `stco`+`co64`, is `Malformed("stbl")`; a second `tkhd`, `mdia`, `mdhd`, `hdlr`, `minf`, `stbl` or `elst` is `Malformed(<4cc>)`. Box-structure errors inside `moov` fail the parse, as inside `meta`.

### How tables are stored

Tables are not decoded into Vecs. At parse time each table's FullBox version and entry count are read, and the count is checked against the payload:

| Table | Version | Check |
|---|---|---|
| `stts` | 0 | `count x 8 <= payload - 8` |
| `ctts` | 0/1 | `count x 8 <= payload - 8` |
| `stss` | 0 | `count x 4 <= payload - 8` |
| `stsc` | 0 | `count x 12 <= payload - 8` |
| `stsz` | 0 | sample_size and sample_count; when sample_size is 0, `count x 4 <= payload - 12` |
| `stz2` | 0 | field_size in {4, 8, 16}; `ceil(count x fs / 8) <= payload - 12` |
| `stco` | 0 | `count x 4 <= payload - 8` |
| `co64` | 0 | `count x 8 <= payload - 8` |

- **On failure.** Any other version, or a count that does not fit, is `Malformed(<4cc>)`. Trailing bytes are allowed.
- **What the Track keeps.** Only the `Extent` of the entries and the count. So a track costs O(1) memory whatever its sample count, and no allocation is sized from a count before the bytes exist.
- **Walks.** One walk (one `samples`, `sync_samples` or `sync_sample_at` call) keeps a forward window per table: `stts`, `ctts`, `stss`, `stsc`, the sizes and the chunk offsets. A window holds at most 4 KiB of its table, read with `read_at` when a read leaves it, and the 7 windows (the seventh holds the `trun` records of the current fragment run) are charged to `max_alloc` once per walk (a table smaller than 4 KiB costs its size).
- **Cost.** A walk reads only the table bytes it passes over. Skipping uses run lengths (below), so `stsz` is read only for the samples of the target chunks.
- **Cancel.** The flag is checked at walk start, whenever a window is refilled (at least every 4096 table bytes) and every 4096 samples.

### Sample number to chunk to offset (moov samples)

- **Sample count.** The number of samples is the `stsz`/`stz2` sample_count. Sample n (1-based) gets the size at entry n-1 (or the constant `sample_size`). `stz2` 4-bit entries are high nibble first.
- **stsc runs.** The entries are `(first_chunk f_j, samples_per_chunk p_j, sample_description_index d_j)`.
  - `f_0` must be 1, and `f_j` must strictly increase, else `Malformed("stsc")` (an `stsc` with no entries and samples is `Malformed("stsc")` too).
  - Run j covers chunks `f_j ..= f_{j+1} - 1`; the last run ends at chunk C, where C is the `stco`/`co64` count.
  - `p_j = 0` is allowed: those chunks hold no samples, and a run of them is crossed in one step.
- **Walk state** is `(j, c, k, off)`: the current run, the chunk (1-based), the samples already taken from that chunk, and the offset of the next sample.
- **Next sample:**
  1. While `k == p_j`, move on one chunk: `c += 1`, `j += 1` when `c == f_{j+1}`, `k = 0`, `off = chunk_offset(c)`. Passing chunk C while samples remain is `Malformed("stco")` (`"co64"`).
  2. The sample is `Extent { off, size }` with entry `d_j`. An end past the file is `Truncated` (the iterator ends there).
  3. Then `off += size` and `k += 1`.
- **Skip to sample s.** A sparse walk (`stss`) advances in three steps, with no per-sample reads in the first two:
  1. Finish the current chunk.
  2. Skip whole chunks run by run, `q = min(chunks left in run, floor(r / p_j))`.
  3. Add the sizes of the samples of the target chunk that come before s. A constant `sample_size` is multiplied instead.
- **Arithmetic.** Every sum and product is `u64` `checked_*`. An overflow of an offset is `Malformed("stco")` (`"co64"`), of a time `Malformed("stts")`.
- **Data references.** A sample whose entry's `data_reference_index` names a `dref` entry without the self-contained flag is `Unsupported("data reference")`, the same rule as iloc. A missing `dref` counts as self-contained.
- **Description index.** An entry index of 0 or past `Track::entries` is `Malformed("stsc")`.
- **dts.** `dts(n)` is the sum of the `stts` deltas of samples 1..n-1, as unsigned u32 deltas, skipped run by run.
  - Past the last run, the last delta repeats (ffmpeg does the same).
  - An empty `stts` with samples is `Malformed("stts")`.
  - A total above `i64::MAX` is `Malformed("stts")`.
- **cto.** It comes from the `ctts` runs. Both versions are read as **signed i32**, as ffmpeg and gpac do; this is a deliberate deviation from ISO 14496-12 for v0. Samples past the end of `ctts` get 0.
- **sync.**
  - No `stss`: every sample is a sync sample.
  - `stss` present: a sample is sync when its number is listed. Entries must be at least 1 and strictly increasing, else `Malformed("stss")` (met when the walk reads that entry). Entries past `sample_count` are ignored.
  - An empty `stss` means no sync samples.
  - `stps`, `sdtp` and `sbgp 'rap '` are kept raw and do not set `sync`.
- **sync_samples(n).** Walks `stss` and skips to each listed sample, or takes samples 1..n when there is no `stss`; then the fragment samples that are sync. The Vec is charged for `min(n, listed or all moov samples + fragment samples)`.
- **sync_sample_at(t).** `t` is in the track's media timescale, on the raw composition timeline (`pts = dts + cto`, no edit list).
  - It returns the sync sample with the greatest `pts <= t`; ties go to the lowest number.
  - When no sync sample has `pts <= t`, it returns the lowest-numbered sync sample. It returns `None` when the track has no sync sample.
  - It visits every sync sample, because pts is not monotone in sample order.
- **Edit lists** are exposed as `Track::edits` only. No pts is shifted by the edit list. A caller who needs presentation times applies the single-edit shift itself: `pts + rescale(empty edits) - media_time`. That shift is what ffmpeg's `-advanced_editlist 0` applies, so it can be gated later if a consumer needs it.

### read_movie

- **Top level.** It walks top-level headers as `read` does, seeking over `mdat`.
- **moov.** It copies the whole `moov` (at most `Limits::max_moov_bytes`, charged while it is parsed, then given back) and parses it with the same `Ctx`/`Boxes` code over a `Src` with `base = moov start`, sharing the box counter. The `ftyp` payload is copied under the same cap.
- **moof.** It records the header of every top-level `moof`, then copies and parses each one after the walk (see "Fragmented MP4").
- **Equality.** The `Movie` is equal to `parse_movie` on the same bytes, `stats` included. Sample walks read only table windows and need the reader again.

### Limits and errors

`max_moov_bytes` (64 MiB; `Limit::MoovBytes`, class `limit-moov-bytes`) caps the `moov` box, and each `moof` box, that `read_movie` copies. The 64 MiB default is an estimate: a 2-hour 60 fps file with B-frames and AAC has about 10 MB of `moov`. The existing limits also apply: `max_boxes` and `max_depth` to the `moov` walk, `max_properties` to the children of sample entries (all entries together), `max_alloc` to tracks, entries, properties, edits, `other_boxes`, the `dref` flags, the `trex` list, the `moof` headers, the fragment runs, the walk windows and the `sync_samples` Vec (the growing Vecs use the doubling-charged pattern of `meta/mod.rs`), and the cancel flag to boxes and walks. A walk allocates nothing whose size comes from the input; its time is bounded by the table bytes in the file, `sample_count` (at most chunks x file size) and the cancel flag.

`Error::NoMoov` is new. Tokens used with `Malformed`: `moov mvhd tkhd mdhd hdlr stbl stsd stts ctts stss stsc stsz stz2 stco co64 elst dref mvex trex traf tfhd tfdt trun`. `Unsupported`: `cmov`, `data reference`. A sample past the end of the file is `Truncated`.

### Fragmented MP4

`moov` with `mvex` makes the file fragmented (`Movie::fragmented`). Every top-level `moof` is parsed after `moov` and the walk of the top-level boxes (so a `moof` before `moov` works), in file order. `parse_movie` and `read_movie` give equal `Movie` values: `read_movie` collects the `moof` headers (charged), then copies each `moof` in turn (at most `max_moov_bytes`, charged, given back after the parse) and parses it with a `Src` whose `base` is the `moof` start, sharing the box counters. Nothing is copied or kept per sample.

- **Precondition.** A `moof` when `moov` has no `mvex` is `Malformed("mvex")`.
- **trex.** One per track id, read from the `trex` children of `mvex`; `mvex` itself stays in `Movie::other_boxes`. A duplicate is `Malformed("trex")`. A `traf` of a track in `moov` that has no `trex` is `Malformed("trex")`. A `trex` of an unknown track is ignored.
- **moof.** Children other than `traf` (`mfhd`, `pssh`, ...) go to `Movie::other_boxes`, after the boxes of the top-level walk.
- **traf.** `tfhd` must be the first child (else `Malformed("traf")`); its track id is not 0 (`Malformed("tfhd")`). A `traf` of a track id that is not in `moov` is kept whole in `Movie::other_boxes`. `tfdt` (v0 u32, v1 u64, above `i64::MAX` is `Malformed("tfdt")`) applies to the first run of the `traf` when it comes before that run. Each `trun` is a run. Other children go to the `other_boxes` of the track.
- **tfhd flags.** `0x01` base_data_offset (u64), `0x02` sample description index, `0x08` default duration, `0x10` default size, `0x20` default flags, `0x020000` default-base-is-moof. The description index (tfhd, else trex) must be in `1..=entries` (else `Malformed("tfhd")`).
- **Base offset.** The explicit base when present. Else the `moof` start with default-base-is-moof. Else where the data of the last run of this `moof` ended (the `moof` start before any run), as ffmpeg does (`implicit_offset`, `mov.c:1435`, `5288`).
- **Run start.** `base + data_offset` (i32, checked: overflow is `Malformed("trun")`) when trun flag `0x01` is set, else the end of the previous run of this `traf` (the base for the first run). ffmpeg restarts at the base for every `trun` without data_offset, so a `traf` with several such runs differs (a known difference, docs/parity.md).
- **trun.** v0 and v1. Flags: `0x01` data_offset, `0x04` first_sample_flags, `0x100` duration, `0x200` size, `0x400` flags, `0x800` cto (i32 for both versions). The record has 4 bytes per field present, and `count x record <= payload left` (else `Malformed("trun")`). With no size field and a default size of 0 a run of 1 or more samples is `Malformed("trun")` (a run whose length the file could not bound).
- **Field resolution.** Duration, size and cto come from the record, else (duration, size) from tfhd, else trex. The flags of a sample come from the record, else (sample 0) first_sample_flags, else tfhd, else trex. `Sample::sync` is `flags & 0x0001_0000 == 0`; `Sample::flags` holds the resolved value.
- **Run dts.** The `tfdt` for the first run of a `traf` that has one, else the end of the previous run of the track (any `moof`), starting at the sum of the `stts` deltas of `moov` (a track with no `moov` samples starts at 0). Past `i64::MAX` is `Malformed("trun")`.
- **What parse keeps.** Per non-empty `trun`, a private record of about 80 bytes (start offset, first dts, the `Extent` of the per-sample records, count, flags, the resolved defaults, the description index), charged with the doubling pattern. Parse reads each record range once, to sum durations and sizes (O(1) memory, cancel every 4096 records); a run without per-sample duration and size costs O(1). `Track::sample_count` is the `moov` count plus the counts of all runs.
- **Walks.** `samples`, `sync_samples` and `sync_sample_at` continue after the `moov` samples through the runs in file order. A walk has one more window (4 KiB at most, charged at the start) for the records of the current run. A fragment sample past the end of the file is `Truncated`. `sync_samples` takes the `stss`-listed (or all) `moov` samples, then the fragment samples whose flags say sync; `sync_sample_at` treats both kinds alike.
- **Time.** A walk to the end visits every sample; runs may overlap, so a hostile file can make that long (only the cancel flag bounds it). The fuzz target walks to the end only for tracks with at most 65 536 samples.
- **Kept raw.** `mfra`/`tfra`, `sidx`, `mehd`, `sdtp`, `senc` and the other boxes are kept raw and not used.
- **Errors.** Tokens `mvex trex traf tfhd tfdt trun`.

## Limits

`parse`, `read`, `item_bytes`, `metadata`, `parse_movie`, `read_movie` and the sample walks of `Track` take `&Limits` and `&AtomicBool` (cancel); the `Heif` accessors work on the already-bounded graph. Each cap has its own `Limit`
variant: `Error::LimitExceeded(Limit::X)`.

| Field | Default | Counts | libheif 1.23.7 default |
|---|---|---|---|
| `max_alloc` | 256 MiB | peak bytes of every input-sized allocation of one call (items, properties, references, extents, the `read` copy of `meta`, metadata copies) | max_total_memory 4 GiB |
| `max_threads` | `available_parallelism()` | never spawns; field kept for the shared `Limits` shape | - |
| `max_boxes` | 1 000 000 | boxes visited in one call, all levels | max_children_per_box 100 (per box) |
| `max_depth` | 32 | box nesting depth (top level = 0) | - |
| `max_items` | 65 535 | `infe` entries, `iloc` items and `ipma` entries (each count) | max_items 1000 |
| `max_properties` | 65 535 | `ipco` children; also associations per call / 16 | max_children_per_box 100 |
| `max_references` | 1 000 000 | all `to` ids of all `iref` boxes plus all entity ids of `grpl`, together | max_items 1000 per iref box (libvips sets 16, which breaks iPhone files: libheif #1845) |
| `max_extents` | 1 000 000 | iloc extents, all items together; also the work of resolving construction method 2 (its extents and the source extents it visits) | max_iloc_extents_per_item 32 |
| `max_grid_tiles` | 65 536 | rows x columns of one grid; inputs of one iovl | max_number_of_tiles 4096 x 4096 |
| `max_pixels` | 1 << 30 (32768 x 32768) | ispe width x height, grid and iovl output width x height | max_image_size_pixels 32768 x 32768 |
| `max_meta_bytes` | 64 MiB | size of the `meta` or `mini` box that `read` copies | - |
| `max_metadata_bytes` | 64 MiB | one Exif or XMP item (bytes) that `metadata()` returns | max_memory_block_size 4 GiB |
| `max_moov_bytes` | 64 MiB | size of the `moov` box, of each `moof` box (and of the `ftyp` payload) that `read_movie` copies | - |

Why these values: real phone files have 48 dimg references from one grid (iPhone 48 MP, 8 x 6
tiles of 512), plus thumbnails, depth, gain map and its own grid; 200 MP phone files have about
200 tiles. The defaults are far above every corpus file (the parity summary prints the corpus
maxima of items, references, tiles, extents, boxes and depth; docs/limits.md records them) and
bound memory: 65 535 items at about 200 bytes each is about 13 MiB. libheif's per-box caps (100
children, 32 extents per item, 1000 refs) are lower than ours on purpose: a valid file with more
must parse.


## Errors

`Error`: `UnknownFormat`, `NoMeta`, `NoMoov`, `Truncated`, `Malformed(&'static str)`,
`Unsupported(&'static str)`, `NoSuchItem(u32)`, `LimitExceeded(Limit)`, `OutOfMemory`, `Io(ErrorKind)` (a reader error in `read`; an early end is `Truncated`),
`Cancelled`. `Error::class()` gives a stable token for the oracle.

## Top-level images

`Heif::top_level_images() -> Vec<u32>` (ascending id), the same set as libheif's
`heif_context_get_list_of_top_level_image_IDs`: image items (`hvc1`, `av01`, `grid`, `iovl`,
`iden`, `jpeg`, `j2k1`, `unci`, `mski`, `vvc1`, `avc1`) that are not hidden and are not the
source of a `thmb` or `auxl` reference. A `tmap` (gain map) item is not listed: it is an image
for `Heif::image` but libheif has no image class for it. The parity run shows if this rule differs from libheif;
fix the rule, not the oracle.

## Fuzz entry points

`#[doc(hidden)] pub mod __fuzz` exposes one function per parser, each `fn(&[u8], &Limits) ->
Result<(), Error>` on a raw payload (a cancel flag that is never set is made inside):
`boxes` (walk every box to `max_depth`, recursing into every child that parses as a box list),
`iloc`, `iinf`, `iref`, `ipma`, `ipco` (every property parser), `grid`, `iovl`, `tmap`, `ftyp`, `mini` (the payload of a `mini` box, then the graph built from it and every item looked at; the `ftyp` minor version is `avif` for an even length and `heic` for an odd one) and `movie` (a whole file).
The fuzz crate (tests/fuzz) has the targets `parse` (whole file + every graph call + metadata),
`read` (`read` over a `Cursor` equals `parse`), `boxes`, `iloc`, `iinf`, `iref`, `ipma`, `ipco`,
`derived` (grid + iovl), `tmap`, `mini`, `movie` (whole file: `parse_movie` and every walk of every track),
`movie_read` (`read_movie` over a `Cursor` equals `parse_movie`, same first samples). A target's input is
the payload as is, except `parse`, `read`, `movie` and `movie_read`, which take a whole file.

## Oracles and parity

Spec of the oracle output: docs/parity.md. Summary:

- `tests/parity/c/heif_oracle.c`: libheif 1.23.7 (`12a259a5`, LGPL-3.0, CI only, built from
  source with no codecs, security limits off). Writes per input a directory with `graph.txt` and
  `item-<id>.bin` (item data) and `icc-<id>.bin`.
- `tests/parity/c/avif_oracle.c`: libavif 1.4.2 (`c5240fc7`, BSD-2, no codecs). Writes
  `avif.txt` (+ `avif-*.bin`) for the primary item of AVIF files (`avif` or `mif3` brand; built with
  `AVIF_ENABLE_EXPERIMENTAL_MINI=ON` so it reads `mini` boxes). libheif reads `mini` without a flag.
- `examples/oracle.rs`: the same files from isobmff-rs. `diff -r` of the trees is the gate.
- ffprobe (Debian bookworm ffmpeg 5.1.x, CI only) and `oracle mp4`: the MP4/MOV tracks (gate 5).
  `oracle mp4` writes `tracks.txt` (the samples of every video track and the nearest sync sample at 18
  probe times); `tests/parity/ffprobe-tracks.sh` prints the same lines from the packets that ffprobe
  reads with `-ignore_editlist 1`, and `tests/parity/mp4cmp.awk` compares them field by field (docs/parity.md).
- Corpus: tests/parity/corpus.toml (public samples at pinned commits, licences quoted). Nothing
  of it is in this repo; tests/parity/fetch.sh fetches it into the BuildKit cache mount
  `id=isobmff-rs-data`.

## Implementation notes (where the code settles what the sections above leave open)

- `read<R: Read + Seek>(r: R, ..)` takes the reader by value; pass `&mut reader` to keep it.
- `Limit` has one variant per `Limits` field: `Alloc`, `Boxes`, `Depth`, `Items`, `Properties`,
  `References`, `Extents`, `GridTiles`, `Pixels`, `MetaBytes`, `MetadataBytes`, `MoovBytes`; `class()` gives
  `limit-` plus the kebab-case name. `max_pixels` is copied into the `Heif` and checked by
  `Heif::image`; `max_properties` caps associations at 16 times its value.
- `Image::width`/`height` are 0 when the image has no `ispe`; `display_size()` is then `None`.
- `ipma` associations with property index 0 are dropped; an index past `ipco` fails the images
  that use it with `Malformed("ipma")`. At most 4 `ipma` boxes per `iprp`.
- `GainMap::xmp` lists the XMP items of the gain map image (Apple source).
- A `tmap` item as an image is `ImageKind::Unsupported(tmap)`; its gain map hangs off the base.
- Strings (`name`, `content_type`, ...) are UTF-8; bytes that are not UTF-8 become U+FFFD, so the
  oracle's `%XX` escape of a non-UTF-8 name differs from libheif's raw bytes.
- `Heif::image` and `primary_image` take no `Limits`; their small result vectors are bounded by
  the already-charged parse and are not charged again.

## Metadata is kept (owner rule 2026-11-19)

The item graph exposes and keeps every item, property, reference and entity group: Exif, XMP,
ICC and nclx, gain-map items, depth, thumbnails, and unknown items and properties as raw byte
ranges. The `mini` box, when the file has one, is kept as `Heif::mini` with every header field and chunk range. Every other child of `meta` (`ipro`, `dinf`, unknown boxes) is kept as a (type, whole-box range) in `Heif::other_boxes`, every child of `iprp` other than `ipco` and `ipma` in `Heif::iprp_other` (same shape, charged by doubling), and the protection index of each item is kept. Dropped while parsing: `iloc` entries with no `infe`, and `ipma` entries for unknown ids (none of these is reachable from an item). v1 writes no files; any later API that writes or
filters a file keeps all of these by default and drops them only on an explicit option.

The movie view keeps the same way: every box of `moov`, `trak` and the top level that is not parsed is a (type, whole-box range) in `Movie::other_boxes` or `Track::other_boxes`, and the parsed sample tables are kept as ranges in `Track::tables`.
