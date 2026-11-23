# Parity with libheif and libavif

isobmff-rs is a new crate with no Go original. Its oracles are libheif and libavif, which run
only in CI, built from source with no codecs. Neither library is a dependency of the crate.

| Oracle | Version | Licence | Use |
|---|---|---|---|
| libheif | v1.23.7, commit `12a259a5882dd99d45ec5fc77e296a3b620d1e2e` | LGPL-3.0 (CI tool only, never shipped) | item graph of every file, item bytes, images |
| libavif | v1.4.2, commit `c5240fc79fe5c2407e10afd35f5505ef6333ea49` | BSD-2-Clause | AVIF primary image: size, depth, alpha, ICC, Exif, XMP, transforms, byte extent, gain map |

## Output format (both sides write exactly this)

For each input `<name>` (path under the corpus root with `/` replaced by `__`), each tool writes a
directory `<out>/<name>/`. `diff -r` of the libheif tree and the isobmff-rs tree is the gate.

Strings are written escaped: bytes 0x21..0x7e except `%` are written as is; every other byte
(space, `%`, control, non-ASCII) is `%XX` (upper-case hex). An empty string is `""`.
A FourCC is written with the same escape. Lists are comma separated, `-` when empty. Numbers are
decimal. Every line ends with `\n`. No trailing spaces.

Strings that isobmff-rs stores as UTF-8 (item name, `ctype`, `cenc`, `uri`, aux type, meta content
type) are written after the lossy UTF-8 conversion of `String::from_utf8_lossy` (one U+FFFD,
`%EF%BF%BD`, per maximal invalid sequence); the C oracle does the same before escaping. FourCCs
(including the meta type) are raw bytes. `ispe` is unsigned.

Both oracles also write one line per error to stderr, `<name>TAB<what>: <message>` (open or parse
failure, an image or aux handle that does not open, item data that fails, libavif's
`diag.error`); run.sh collects them in `errors.tsv` (name, pair, oracle, isobmff-rs). They are for
diagnosis, not gated.

### `graph.txt` (heif_oracle and `oracle heif`)

First line `ok`, or `error` and nothing else (the file did not open: libheif
`heif_context_read_from_file` failed, or `isobmff::parse` failed). Then, in this order:

```
brand <major> <compat,...>
primary <id>
item <id> <type> hidden=<0|1> name=<s> ctype=<s> cenc=<s> uri=<s>     one per item, ascending id
data <id> <len>                                                       or: data <id> error
ref <from> <type> <to,...>                                            items ascending, then file order
prop <id> <n> <type> [rot=<deg>|axis=<0|1>]                           per item ascending, ipma order
group <type> <group_id> <ids>                                         grpl order
image <id> ispe=<w>x<h>|- size=<w>x<h> alpha=<0|1> prem=<0|1> bits=<l>/<c>|-
colr <id> nclx=<p>,<t>,<m>,<f>|- icc=<len>|-
aux <id> <aux_id> <aux_type>                                          aux ids ascending
depth <id> <depth_id>                                                 ascending
thumb <id> <thumb_id>                                                 ascending
meta <id> <meta_id> <type> <ctype> <len>                              ascending meta id
grid <id> <columns>x<rows> tile=<w>x<h> out=<w>x<h> tiles=<ids>       tiles row-major
```

- `brand`: libheif `heif_read_main_brand` and `heif_list_compatible_brands` on the file bytes;
  isobmff-rs `Heif::ftyp`.
- `item`: every item id (libheif: `heif_context_get_list_of_item_IDs`, sorted). `ctype`/`cenc`
  only for `mime` items, `uri` only for `uri ` items, else `-`.
- `data`: libheif `heif_item_get_item_data` with a non-NULL compression pointer (raw bytes, no
  decompression), which is the iloc extents concatenated; the bytes go to `item-<id>.bin`.
  isobmff-rs: `Heif::item_bytes`. A failure on either side is `error` and no `.bin`.
- `ref`: libheif `heif_context_get_item_references(ctx, from, index, ...)`, index 0, 1, ...
- `prop`: libheif `heif_item_get_properties_of_type(..., heif_item_property_type_invalid, ...)`;
  `<n>` is the 1-based position in the item's ipma list (index 0 skipped). `irot` adds
  `rot=<deg>` (anticlockwise), `imir` adds `axis=<0|1>`.
- `prop`: a property that failed to parse is listed by libheif as type `ERR ` (`ERR%20`);
  isobmff-rs prints the same for `PropValue::Invalid`. Files where the two parsers disagree on
  what is broken (e.g. `pixi` with version other than 0) differ.
- `group`: `heif_context_get_entity_groups(ctx, 0, 0, &n)`. libheif only exposes `altr`, `ster`
  and `pymd` groups, so isobmff-rs prints only those types.
- `colr`: libheif keeps the last `colr` of each kind and treats nclx (2, 2, 2, full range) as no
  profile; the oracle does the same. A grid without a profile takes its first tile's.
- `alpha` (of `image`): for a grid, libheif reports alpha when any tile has alpha; the oracle
  checks the tiles. `aux`: `-` as type when libheif cannot open the aux image (no `av1C`/`hvcC`).
- `image` and the lines after it: one block per top-level image, ascending id
  (`heif_context_get_list_of_top_level_image_IDs`, sorted). `size` is
  `heif_image_handle_get_width/height` (after transforms), isobmff-rs `Image::display_size`.
  `bits` only for `hvc1` and `av01` items (luma/chroma bits from hvcC/av1C), else `-`. `icc` bytes
  go to `icc-<id>.bin`. `grid` only for `grid` items:
  `heif_image_handle_get_image_tiling(h, 0, &t)` and `heif_image_handle_get_grid_image_tile_id`;
  `tile=` is the first tile's size after its transforms (`Image::display_size`), as libheif
  reports it. The Rust oracle runs `heif` with `max_pixels` unlimited, as libheif runs with its
  security limits off.

### `avif.txt` (avif_oracle and `oracle avif`)

Only for files whose `ftyp` major or compatible brands include `avif`. Decoder settings:
`requestedSource = AVIF_DECODER_SOURCE_PRIMARY_ITEM`, `strictFlags = AVIF_STRICT_DISABLED`,
`imageContentToDecode = AVIF_IMAGE_CONTENT_ALL`, `imageDimensionLimit = 0`, `imageCountLimit = 0`,
`imageSizeLimit` left at its default (16384 x 16384). First line `ok` or `error`:

```
size <w>x<h> depth=<8|10|12> yuv=<444|422|420|400>
alpha <0|1> prem <0|1>
icc <len>                                    bytes in avif-icc.bin when len > 0
exif <len>                                   bytes after the 4-byte exif_tiff_header_offset (libavif) in avif-exif.bin
xmp <len>                                    bytes in avif-xmp.bin
irot <deg>|-
imir <axis>|-
clap <wN>/<wD>,<hN>/<hD>,<xN>/<xD>,<yN>/<yD>|-          xN, yN signed
pasp <h>/<v>|-
extent <offset> <size>                       avifDecoderNthImageMaxExtent(0)
gainmap <0|1>
gainmap-meta <fields>                        only when gainmap 1 (see below)
```

- `size` is ispe (or grid output size), untransformed. `depth`/`yuv` from av1C.
- `extent`: one merged range from the lowest offset to the highest end of every extent of the
  colour item (all tiles of a grid), the alpha item (all its tiles) and the gain map item, when
  each exists; `0 0` when the data is in `idat`. For a grid, libavif has alpha only when every
  tile has an alpha item (`alpha 1`), and the alpha items of the tiles are in the range.
  libavif checks the file size only when it reads the data, so the oracle prints `extent error`
  when the range ends past the end of the file (isobmff-rs drops such an item's extents at parse).
- `pixi`/`pasp`/`lsel`: libheif treats them as optional (the image opens without them); libavif
  fails the whole parse when one does not parse, so the avif oracle prints `error` for such a file.
- `iden`: libheif checks the input of an `iden` only when decoding; the heif oracle reports an
  `iden` without exactly one `dimg` input as `note` (no image block), like isobmff-rs.
- `gainmap` is 1 only when `ftyp` has the `tmap` brand and the tmap item is found (libavif rule).
  `gainmap-meta`: `base=<n>/<d> alt=<n>/<d> min=<n>/<d>,... max=... gamma=... baseoff=... altoff=... base_cs=<0|1>`
  per channel (3 channels; a single-channel map repeats channel 0).
- If libavif fails with `AVIF_RESULT_INVALID_EXIF_PAYLOAD`, the oracle parses again with
  `ignoreExif = 1` and writes `exif error`.

## Gates (tests/parity/run.sh)

1. **Corpus**: every corpus file through `heif_oracle` and `oracle heif`; AVIF files also through
   `avif_oracle` and `oracle avif`. `diff -r` must be empty, except files listed in
   `tests/parity/known-differences.tsv` (file, side, reason), whose diff is printed but not
   counted. A known difference whose diff became empty fails the gate (stale entry).
2. **Mutations**: `oracle gen` makes deterministic mutations (truncations, bit flips, size-field
   edits) of every corpus file under 1 MiB. For each, both sides run; where both give `ok`, the
   trees must be equal, except mutations of a file listed in `known-differences.tsv` for that
   side (the name without the mutation suffix), which are counted as known and not diffed. The
   ok/error split is reported, not gated (libheif and isobmff-rs reject different broken files).
3. **Threads and read**: `oracle heif --threads 1` equals the default, and `oracle heif --read`
   (the `Read + Seek` path, item bytes read through the reader) equals the slice path.
4. **Coverage**: the run fails when the corpus has fewer files than `PARITY_MIN_INPUTS`, so a
   missing mount does not shrink the gate.

`summary.md` lists the counts, the corpus maxima (items, references per box, grid tiles, extents
per item, boxes, depth) that docs/limits.md cites, and the first 200 diff lines.

## Corpus statistics

`oracle stats <in-dir> <out.tsv>` writes one line per file: name, boxes, max depth, items,
largest reference count of one iref entry, largest grid (tiles), largest extent count of one
item, properties. isobmff-rs exposes these as `Heif::stats` (`Stats { boxes, max_depth }`) plus
counts of the public vectors. run.sh puts the maxima in summary.md.

## Not gated

- `Heif::metadata()` (TIFF offset stripping, first `cdsc` by ascending id) has no oracle line: the
  avif oracle rebuilds libavif's Exif rules from `item_bytes` (all bytes after the 4-byte field,
  so the usual `Exif\0\0` prefix stays), and the heif oracle does not call it. Unit tests cover it.
- An `iloc` extent with `extent_length` 0 is read here as "to the end of the file" (method 0) or
  of `idat` (method 1), as ISO 14496-12 says. libheif and libavif read zero bytes. No corpus file
  has one.
- Construction method 2 (item offset) is resolved by the crate and by libheif. The `oracle avif`
  rebuild does not follow it (it reports an item error when a method-2 item's range or Exif is
  needed, as before): the libavif side was not checked for method 2, and no AVIF corpus file uses it.
  The heif side compares the item bytes, which are the same when both resolve the item. The one
  corpus file with method 2, `iff_hevc_tile_multiple_items_tbas.heic`, keeps its line in
  `known-differences.tsv` until CI parity shows its diff is empty.
