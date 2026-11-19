# isobmff-rs design (build spec)

isobmff-rs parses ISO-BMFF containers: HEIF (HEIC), AVIF and, later, MP4/MOV. It decodes no
pixels. It gives an item graph (primary image, tiles, alpha, depth, gain map, thumbnails,
metadata) where every coded item is a codec configuration record plus byte ranges of the file.
hevc-rs and av1-rs decode those bytes; exif-rs parses the Exif and XMP bytes.

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
    iloc.rs     iloc v0..v2 -> absolute file extents
    iref.rs     iref v0/v1
    iprp.rs     ipco (property boxes, kept as ranges) + ipma
    grpl.rs     grpl entity groups (altr, ...)
  props/
    mod.rs      Property enum, parse dispatch by 4cc
    image.rs    ispe pixi pasp irot imir clap colr auxC lsel a1op a1lx
    config.rs   hvcC av1C (parsed header + raw range)
  derived.rs    grid, iovl, tmap payload parsers
  graph.rs      Image, ImageKind, Transform, Aux, GainMap, the item graph API
  metadata.rs   Exif / XMP item extraction
```

## Input model

All positions are **absolute file offsets**. `Extent { offset: u64, length: u64 }` is a range of
the input file. Construction method 1 (idat) extents are translated to absolute offsets of the
`idat` payload, so a caller needs only the file. The parsed `Heif` holds no borrow of the input.

- `isobmff::parse(data: &[u8], limits, cancel) -> Result<Heif, Error>`: the whole file is in
  memory. `Heif::slice(data, extent) -> Option<&[u8]>` gives a borrowed slice (zero copy).
- `isobmff::read<R: Read + Seek>(r, limits, cancel) -> Result<Heif, Error>`: reads only the
  top-level box headers, `ftyp` and the whole `meta` box (at most `Limits::max_meta_bytes`, the
  copy is charged to `max_alloc`); it seeks over `mdat`. The `Heif` is the same as `parse` on the
  same bytes (a fuzz target and a test check this). The caller reads extents itself.

## Box reader (`boxes.rs`)

General ISO-BMFF reader, also for the later MP4/MOV work (`moov/trak/mdia/minf/stbl`).

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
   primary 0. `moov` is noted (`Heif::has_moov`), not parsed in v1. Other boxes (`mdat`, `free`,
   `skip`, `uuid`, ...) are skipped. Once `meta` or `moov` has been found, a top-level box with an
   incomplete header or that runs past the end of the file (libheif and libavif stop reading
   there) ends the walk instead of being `Truncated`; item data past the end of the file then
   fails per item (`item_bytes` gives `Truncated`). Before `meta` and `moov` it is still `Truncated`.
3. `meta` is a FullBox (version 0). Its `hdlr` must be the first child with handler `pict` (else
   `Malformed("hdlr")`).

## Items (`meta/`)

| Box | Versions | Kept |
|---|---|---|
| `pitm` | 0 (u16 id), 1 (u32 id) | `primary` |
| `iinf` | 0 (u16 count), any other version (u32 count, as libheif) | the first entry_count children; non-`infe` skipped; fewer is Malformed |
| `infe` | 2 (u16 id), 3 (u32 id); 0/1 accepted, kept as item type `\0\0\0\0` | id, protection index, item_type, item_name, `mime`: content_type + content_encoding, `uri `: uri_type; `hidden` = flags & 1 |
| `iloc` | 0, 1, 2 | per item: construction method, data_reference_index, base_offset, extents |
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
(method 1). Construction method 2 (item offset) and a data_reference_index other than 0 are not
supported: the item gets `ItemError::Unsupported`. An item id that appears twice in iloc, iinf or
ipma is `Malformed`.

Item errors do not fail the parse: `Item::data` is `Result<Vec<Extent>, ItemError>`. A request for
that item's bytes gives the error. This matches libheif, which fails only when an item's data is
read.

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
  `max_pixels`, `max_grid_tiles`) is what `Heif::image` gives for that item. Payloads over 4 MiB
  are `Malformed`.

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
  `exif::Exif::parse` takes); a bad offset is `Malformed("Exif")`. XMP is the item bytes as is.

## Limits

Every public function takes `&Limits` and `&AtomicBool` (cancel). Each cap has its own `Limit`
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
| `max_extents` | 1 000 000 | iloc extents, all items together | max_iloc_extents_per_item 32 |
| `max_grid_tiles` | 65 536 | rows x columns of one grid; inputs of one iovl | max_number_of_tiles 4096 x 4096 |
| `max_pixels` | 1 << 30 (32768 x 32768) | ispe width x height, grid and iovl output width x height | max_image_size_pixels 32768 x 32768 |
| `max_meta_bytes` | 64 MiB | size of the `meta` box that `read` copies | - |
| `max_metadata_bytes` | 64 MiB | one Exif or XMP item (bytes) that `metadata()` returns | max_memory_block_size 4 GiB |

Why these values: real phone files have 48 dimg references from one grid (iPhone 48 MP, 8 x 6
tiles of 512), plus thumbnails, depth, gain map and its own grid; 200 MP phone files have about
200 tiles. The defaults are far above every corpus file (the parity summary prints the corpus
maxima of items, references, tiles, extents, boxes and depth; docs/limits.md records them) and
bound memory: 65 535 items at about 200 bytes each is about 13 MiB. libheif's per-box caps (100
children, 32 extents per item, 1000 refs) are lower than ours on purpose: a valid file with more
must parse.


## Errors

`Error`: `UnknownFormat`, `NoMeta`, `Truncated`, `Malformed(&'static str)`,
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
`iloc`, `iinf`, `iref`, `ipma`, `ipco` (every property parser), `grid`, `iovl`, `tmap`, `ftyp`.
The fuzz crate (tests/fuzz) has the targets `parse` (whole file + every graph call + metadata),
`read` (`read` over a `Cursor` equals `parse`), `boxes`, `iloc`, `iinf`, `iref`, `ipma`, `ipco`,
`derived` (grid + iovl), `tmap`. A target's input is the payload as is.

## Oracles and parity

Spec of the oracle output: docs/parity.md. Summary:

- `tests/parity/c/heif_oracle.c`: libheif 1.23.7 (`12a259a5`, LGPL-3.0, CI only, built from
  source with no codecs, security limits off). Writes per input a directory with `graph.txt` and
  `item-<id>.bin` (item data) and `icc-<id>.bin`.
- `tests/parity/c/avif_oracle.c`: libavif 1.4.2 (`c5240fc7`, BSD-2, no codecs). Writes
  `avif.txt` (+ `avif-*.bin`) for the primary item of AVIF files.
- `examples/oracle.rs`: the same files from isobmff-rs. `diff -r` of the trees is the gate.
- Corpus: tests/parity/corpus.toml (public samples at pinned commits, licences quoted). Nothing
  of it is in this repo; tests/parity/fetch.sh fetches it into the BuildKit cache mount
  `id=isobmff-rs-data`.

## Implementation notes (where the code settles what the sections above leave open)

- `read<R: Read + Seek>(r: R, ..)` takes the reader by value; pass `&mut reader` to keep it.
- `Limit` has one variant per `Limits` field: `Alloc`, `Boxes`, `Depth`, `Items`, `Properties`,
  `References`, `Extents`, `GridTiles`, `Pixels`, `MetaBytes`, `MetadataBytes`; `class()` gives
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
ranges. Every other child of `meta` (`ipro`, `dinf`, unknown boxes) is kept as a (type, whole-box range) in `Heif::other_boxes`, and the protection index of each item is kept. Dropped while parsing: unknown children of `iprp`, `iloc` entries with no `infe`, and `ipma` entries for unknown ids (none of these is reachable from an item). v1 writes no files; any later API that writes or
filters a file keeps all of these by default and drops them only on an explicit option.
