# Parity with libheif and libavif

isobmff-rs is a new crate with no Go original. Its oracles are libheif and libavif (HEIF, AVIF) and
ffprobe from ffmpeg (MP4/MOV tracks), which run
only in CI (libheif and libavif are built from source with no codecs). None of them is a dependency of the crate.

| Oracle | Version | Licence | Use |
|---|---|---|---|
| libheif | v1.23.7, commit `12a259a5882dd99d45ec5fc77e296a3b620d1e2e` | LGPL-3.0 (CI tool only, never shipped) | item graph of every file, item bytes, images |
| libavif | v1.4.2, commit `c5240fc79fe5c2407e10afd35f5505ef6333ea49` | BSD-2-Clause | AVIF primary image: size, depth, alpha, ICC, Exif, XMP, transforms, byte extent, gain map |
| ffprobe (ffmpeg) | Debian bookworm 5.1.x (`7:5.1.9-0+deb12u1`) | LGPL/GPL (CI tool only, never shipped) | MP4/MOV tracks: byte range, dts, pts and keyframe flag of every sample of a video track, nearest keyframe |

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
  `rot=<deg>` (anticlockwise), `imir` adds `axis=<0|1>`. Properties of type `free` are not listed and
  not counted in `<n>` on either side: libheif makes `free` placeholders for the unused slots of
  a `mini` box. A real `free` box in `ipco` is rare (no corpus file has one), so this changes no
  other file.
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

Only for files whose `ftyp` major or compatible brands include `avif` or `mif3` (the brand of a `mini`
box file). Decoder settings:
`requestedSource = AVIF_DECODER_SOURCE_PRIMARY_ITEM`, `strictFlags = AVIF_STRICT_DISABLED`,
`imageContentToDecode = AVIF_IMAGE_CONTENT_ALL`, `imageDimensionLimit = 0`, `imageCountLimit = 0`,
`imageSizeLimit` left at its default (16384 x 16384). First line `ok` or `error`:

```
size <w>x<h> depth=<8|10|12> yuv=<444|422|420|400>
alpha <0|1> prem <0|1>
icc <len>                                    bytes in avif-icc.bin when len > 0
exif <len>                                   bytes after the 4-byte exif_tiff_header_offset (libavif; a mini box has none: the whole item) in avif-exif.bin
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
- `gainmap` is 1 only when `ftyp` has the `tmap` brand (for a `mini` box with a gain map: always, as
  libavif adds the brand itself) and the tmap item is found (libavif rule).
  `gainmap-meta`: `base=<n>/<d> alt=<n>/<d> min=<n>/<d>,... max=... gamma=... baseoff=... altoff=... base_cs=<0|1>`
  per channel (3 channels; a single-channel map repeats channel 0).
- If libavif fails with `AVIF_RESULT_INVALID_EXIF_PAYLOAD`, the oracle parses again with
  `ignoreExif = 1` and writes `exif error`.

### `tracks.txt` (`oracle mp4` and ffprobe)

The MP4/MOV side is different: ffprobe is the oracle and it prints no file of this format, so
`tests/parity/ffprobe-tracks.sh` and `ffprobe-tracks.awk` turn its packets into the `s` and `at`
lines below, and `tests/parity/mp4cmp.awk` compares them with `oracle mp4`. One file
`<out>/<name>/tracks.txt` per input. A parse error is the single line `error <class>`
(`Error::class()`). Otherwise:

```
movie timescale=<u32> duration=<u64> tracks=<n> fragmented=<0|1>
track <id> handler=<4cc> timescale=<u32> samples=<n> entries=<4cc,...> config=<4cc|-> edits=<n>
s <id> <n> <pos> <size> <dts-dts(first sample)> <pts> <K|_>      (gated tracks only, every sample)
at <id> <j> <number|none>                                         (gated tracks, j = 0..17)
e <id> <class>                                                    (walk error; no more s/at lines)
skip <id> <reason>                                                (track not gated: `avif`, `handler` or `entry`)
```

- **Gated tracks.** Handler `vide` or `pict`. The oracle models ffmpeg's codec tag: the tag of
  entry 1 is its `frma` original format when its kind is `encv`, else its kind.
  `mov_skip_multiple_stsd` (`mov.c:2524-2548`) drops entry i > 1 when its fourcc differs from the
  tag (except `apcn`, `apch`, `dvpp`, `dvcp`, `jpeg` and `AV1x` with `AVup`, which are kept), and
  ffmpeg then drops the samples that use a dropped entry (`mov.c:4198-4206`). When any later
  entry is kept, `pseudo_stream_id` becomes -1 (`mov.c:2588`) and every sample is kept, even one
  that uses an entry of another fourcc. A track is `skip <id> entry` only when a sample uses an
  entry other than 1 and no later entry is kept, so a second `avc1` after `avc1` (a
  parameter-set change), and `encv(frma avc1)` followed by `avc1`, are compared (the entry
  mapping and the chunk offsets after the switch), while `avc1` followed by `hvc1`, or
  `encv(frma avc1)` followed by `encv`, are skipped. Other handlers are `skip <id> handler`.
- **Skipped files.** One kind of file is `skip <id> <why>` for every track, whatever its handler,
  so that no `s` line of any track is compared (`mp4cmp.awk` compares only the ids with an `s` or
  `e` line in ours, and ignores what ffprobe prints for the others; a file whose tracks are all
  skipped is not in gate 5 at all, nor in the count of 5d):
  - `avif`: the major brand of the first `ftyp` is `avif`. ffmpeg then ignores the `trak` boxes and
    returns the primary `iloc` item as the only packet. Where `meta` precedes `moov`, `iloc` sets
    found_moov and the real `moov` is skipped as a duplicate (`mov.c:1147`, `1178-1183`,
    `7523-7640`); where `moov` comes first the header read fails (`4470`). So
    ffprobe shows a still image where isobmff-rs shows the tracks of the sequence. Exact rule
    (`strncmp(type, "avif", 4)`).
- `s`: `pos` and `size` are the file range of the sample, `dts` is rebased to the first sample (ffmpeg
  shifts it by a per-stream amount), `pts = dts + cto` of the raw tables or of the `trun` records, `K` is `Sample::sync`.
  Fragmented files are gated like the others: `ffprobe-tracks.sh` runs with `-use_tfdt 1`, `-use_mfra_for auto`
  and `-ignore_editlist 1`, and the samples of the `moof` boxes follow those of `moov` in `s` and `at`.
- **Probes.** Let `m = 1 + floor((count - 1) * floor(j / 2) / 8)` for j = 0..17. Probe `t_j` is
  `pts(m) - 1` for even j and `pts(m)` for odd j. Each `at` line is `Track::sync_sample_at(t_j)` on
  our side. On the ffprobe side the awk script takes, among the packets with `K`, the greatest
  `pts <= t_j` (the lowest number among equals), else the first `K` packet, else `none`.
- **Masks.** The oracle prints `-` in a field that ffmpeg computes differently, deciding from the
  raw bytes of `Track::tables` and `Track::other_boxes`. `mp4cmp.awk` skips a field that is `-`.

  | Mask (`-`) | When |
  |---|---|
  | `K` and every `at` | the whole track, when there is an empty `stss`, a `stps`, or an `sbgp` with grouping_type `rap ` (ffmpeg's K comes from those too: `mov.c:4167-4188`) |
  | `pts` and every `at` | a track whose `ctts` has a count of 0 or >= 2^31, an entry with offset magnitude above 2^28, or a total below the number of `moov` samples (`mov.c:3158-3183`, `8842-8856`; the oracle masks every entry, ffmpeg only the ones before the last two) |
  | `dts` and `pts` | a track with any `stts` delta above `0xFFF8ACFF` (ffmpeg clips it: `mov.c:3039-3047`, `9144`) |
  | `K` of one sample (and every `at` of the track) | a fragment sample that is sync by its flags but has depends_on bit `0x0100_0000` set (ffmpeg's keyframe rule: `!(flags & (non-sync | depends-yes))`, `mov.c:5198-5203`) |
  | `pts` and every `at` | a track with fragment samples and any negative cto (ffmpeg's dts_shift is not cancelled for `trun` samples: `mov.c:5179`) |
  | `size` of one sample | a fragment sample that starts in a top-level box and ends past it: ffmpeg cuts a packet at the end of that box (`mov.c:7840-7846`, `8775-8777`) |

- **Why the fields are comparable.** ffmpeg sets index dts to `raw - dts_shift` (`mov.c:4102`) and pts
  to `dts + dts_shift + ctts` (`mov.c:8842`), so pts equals our `dts + cto` exactly. dts is compared
  after rebasing to the first sample. `-ignore_editlist 1` stops `elst` from being read at all
  (`mov.c:5513`, `4337`), so no edit list is applied on either side.
- **ffprobe.** `ffprobe -v error -ignore_editlist 1 -ignore_chapters 1 -use_mfra_for auto
  -use_tfdt 1 -fflags +noparse+nofillin` with `-show_entries packet=stream_index,pts,dts,size,pos,flags`
  (and `stream=index,id` for the track ids), `-of csv=p=0`. The awk scripts run with `gawk -M`, so
  that 64-bit timestamps stay exact. A stream with side data (a non-identity `tkhd` matrix, a `pssh`)
  makes ffprobe print an extra csv row without index and id; the scripts drop such rows. ffprobe's csv writer
  (ffprobe.c `compact_print_section_header`/`footer`) ends such a row with a separator and adds one empty row per side-data element. ffmpeg is Debian bookworm `7:5.1.9-0+deb12u1` when the archive
  still has it, else the current bookworm ffmpeg; `summary.md` prints the version.
- **Comparison** (`gawk -M -v fferr=0|1 -f tests/parity/mp4cmp.awk <ours> <ffprobe>`; `fferr` is 1
  when ffprobe exits non-zero or prints on stderr). Lines are keyed by (kind, id, n or j). Only ids
  with an `s` or `e` line in ours are compared. Fields 4 and up must be equal unless ours is `-`. A
  key on one side only is a mismatch, with two exceptions. When `fferr` is 1 (ffprobe failed or
  warned), only keys on both sides are compared and `at` lines are skipped, for every track. When
  `fferr` is 0, a track with an `e` line skips only its `at` lines (ours has none): a walk error of
  ours is not forgiven, ffprobe keys past our last sample are a mismatch, and the file needs a
  `known-differences.tsv` line (side `mp4`) with the reason. `-v perr=1` is for a file that ours
  failed to parse (`error <class>`): when ffprobe exited 0 (`-v ffrc`, the exit status; its stderr
  does not matter, a warning on a file it reads in full must not hide a parse error), any `s` line
  of ffprobe is a mismatch. The first CI run after the `e` leniency was removed needed no new line.

## The `mini` box in the two oracles

Files with a top-level `mini` box (ISO/IEC 23008-12 Annex O, brand `mif3`) go through both pairs.
libheif 1.23.7 expands them with no flag (`mini.cc` is always compiled); libavif v1.4.2 needs
`-DAVIF_ENABLE_EXPERIMENTAL_MINI=ON` (set in `.github/ci/Dockerfile`) and `avif_oracle.c` and
`oracle avif` accept the brand `mif3` (a mini file has no `avif` brand). isobmff-rs follows libheif
where the two differ (the 7 mini files that were in `known-differences.tsv` are no longer there;
the corpus gate says if one still differs):

- **Syntax.** libheif 1.23.7 (the working draft) reads `matrix_coefficients` always (8 bits) when
  `explicit_cicp_flag` is set and has `gainmap_dimension_same_as_main_item_flag`; libavif v1.4.2
  reads `matrix_coefficients` only when `chroma_subsampling != 0` and has no same-size flag (its
  master has the flag, not yet a release). A file with a gain map, or with explicit CICP on 4:0:0,
  is read differently by the two: no corpus file has either.
- **Codec.** Both take the codec from the `ftyp` minor version (`avif` or `heic`; libavif only
  `avif`) when the box has no explicit codec types, so libavif fails the HEIC mini files (both
  sides print `error`); the VVC `.hmg` files (minor `vvi3`) fail in libheif and isobmff-rs
  (`Unsupported("mini codec")`). With explicit `infe_type` and `codec_config_type` libheif reads
  and ignores them (it always uses the minor version); isobmff-rs uses them.
- **Graph.** libheif has no `tmap` item, gain map item, `dimg` or `altr` group for a mini gain map
  (its TODO) and no `prem` reference; isobmff-rs adds them (layout of libavif, `prem` as the spec
  says). The `heif` side is the gate for items 1, 2, 6, 7; for a premultiplied alpha, a gain map or
  a hidden item 4 a `known-differences.tsv` line says why.
- **Properties.** libheif lists its `free` placeholders (the oracle skips them, see `prop` above);
  pixi has 3 channels in libheif for 4:0:0 (not in `graph.txt`); orientation 5 is `irot` 270 and
  `imir` axis 1 in libheif and isobmff-rs, `irot` 90 and `imir` axis 0 in libavif (the same
  transform; the avif oracle prints only the first `irot` and `imir` of the image: differs for a
  mini file with orientation 5).
- **Exif.** The Exif chunk of a mini box has no `exif_tiff_header_offset`: libavif keeps the whole
  chunk, libheif returns it raw, `Heif::metadata` returns it as is, and the Rust avif oracle does
  the same.
- **Strictness.** libavif requires zero padding bits, exact chunk sums (no trailing bytes), a
  4-byte `av1C`, a matching `pixi` depth, no float, no compressed Exif or XMP; libheif and isobmff-rs
  accept trailing bytes, any config size, float, compressed Exif/XMP (`cenc=deflate`) and do not
  check padding. libavif therefore fails some mini files that isobmff-rs accepts; such files get a
  `known-differences.tsv` line with `avif` as side.
- **Version.** A `mini` of version other than 0 is `Unsupported("mini")`; libheif reads the field
  and does not check it. No corpus file has a version other than 0.
- **Corpus.** Nokia C75, C83 and 1dualMiniVVC (`.hmg`, VVC, no decoder here) and imazen
  `lightning_mini.heif` (the pre-final draft syntax; isobmff-rs parses it and marks the hvcC invalid, libheif does not check its version: a known difference) are in the corpus. No public
  file has a gain map in a `mini` box or sets the `hdr` flag: that path is covered by unit tests and
  the `mini` fuzz target only.

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
5. **MP4/MOV tracks**: `oracle mp4` against ffprobe (`tracks.txt` above) on the corpus, the
   MP4 sources of corpus.toml (`mp4-*`, read by this gate only) and the mutations of both.
   - **5a**: corpus files with a gated track, a walk error or a parse error of class `malformed` or
     `unsupported` (`error malformed`, `error unsupported`: a file ffprobe reads with packets (exit
     0, whatever its stderr says) is a mismatch, so a valid file that parse_movie rejects cannot hide). The classes
     no-moov, truncated, unknown-format and the limit classes are not gated. The parse-error check
     counts every ffprobe packet, whatever the stream, and the corpus of the HEIF/AVIF gates is in
     this input set; the first run of the exit-code form may need a side-`mp4` line for
     `j2ki-siz-tile-alloc-oom.heif` (a parse error, and ffprobe may exit 0 with packets).
     Lines of side `mp4` are stale when the file is now identical, as in gate 1.
   - **5b**: mutations that isobmff-rs reads to the end (a sample line and no `e` line) while
     ffprobe exits 0 with nothing on stderr; the others are only counted. Mutations of a file
     listed for side `mp4` are not diffed.
   - **5c**: `oracle mp4 --read` (`read_movie`, walks through a `File`) equals the slice path.
   - **5d**: at least `PARITY_MIN_MP4` (default 100) corpus files have a gated track; `summary.md`
     prints the count, so the default can be raised to the first green run.
   The mismatches are `mp4.diff` (5a) and `mp4-mut.diff` (5b). Expected known differences, to list
   rather than mask once a run shows them: external `url ` drefs (ours `Unsupported`, ffmpeg reads
   the main file, so only the samples before ours stops are compared), several `trun` boxes of one `traf`
   without data_offset (ffmpeg restarts each at the base, `int data_offset = 0`, `mov.c:5003`; we continue after the
   previous run, as the standard says), fragments with `sidx` but no `tfdt` (ffmpeg takes the sidx time,
   `mov.c:5079-5093`; we take the end of the previous run), a hybrid track (`moov` samples plus fragments)
   with a negative cto, `cmov`. A `traf` of a track without `trex`, or of an unknown track, is kept raw
   here, so its samples are not listed. ffmpeg does not ignore it: `mov_read_tfhd` (`mov.c:4881-4883`)
   skips the `tfhd` with a warning and leaves `found_tfhd` as it was, so the `trun` that follows
   fails with "trun track id unknown" (`mov.c:5013-5016`) when it is in the first `traf` of the
   `moof`, or is added to the track of the previous `traf` of that `moof` (its track id, base and
   defaults). Both differ from ours and are known differences. Two top-level `moov` boxes: ffmpeg
   parses the first and skips the others with a warning (`mov.c:1181`); so do we, and the later
   `moov` is kept in `Movie::other_boxes`. A constant-size `stsz` that counts decoded audio samples
   (QuickTime v1 sound) can have more samples than twice the file length, which `sync_samples`,
   `sync_sample_at` and `samples` reject as `Malformed("stsz")` (docs/limits.md). A `trun` of
   size 0 and a `moof` without `mvex` are errors here. Listed so far (side `mp4`): a file with a track whose samples lie
   past the end of the file (ffmpeg interleaves the tracks and stops reading at the first sample it
   cannot read, `mov.c:8589-8613`, `8801`): `maf/vsaf/1.mp4`.

`summary.md` lists the counts, the corpus maxima (items, references per box, grid tiles, extents
per item, boxes, depth) that docs/limits.md cites, and the first 200 diff lines.

## Transforms (tests/parity/transform.sh)

Oracle for `isobmff::apply_transforms` (`src/transform.rs`): libheif v1.23.7 at the same commit as above, built a second time in its own prefix (`c-oracle-tr` in `.github/ci/Dockerfile`) with libde265 and dav1d from Debian bookworm and the uncompressed codec. The no-codec libheif of the other gates is unchanged. It runs in the `parity` job of ci.yml as a second build (target `transform-report`, artifact `parity-transform`); like the other gates it does not run on pull requests.

**Settings.** `heif-transform-oracle` (`tests/parity/c/transform_oracle.c`) decodes every top-level image, its auxiliary images (alpha, depth, gain map: whatever libheif lists with `heif_image_handle_get_list_of_auxiliary_image_IDs` and `..._depth_image_IDs`) with `colorspace` and `chroma` undefined, so libheif keeps the native planes, and `output_image_nclx_profile_passthrough = 1`, so it does not colour-convert. It decodes twice: with `ignore_transformations = 1` (raw) and with the transforms of the item (expected). The upsampling algorithm is not set: libheif converts to 4:4:4 with its own default options (bilinear) whatever the caller asks, and has no nearest-neighbour YCbCr to YCbCr conversion. Images above 64 Mpixel (`ispe`) are not decoded.

**What libheif does.** It applies `irot`, `imir` and `clap` in `ipma` order, one operation per property (no MIAF order is enforced; a second `irot`, `imir` or `clap` is an error even with `ignore_transformations`). `clap` uses the size after the transforms before it. A crop clamps left and top to 0 and a right or bottom edge past the image (or negative) to the last column or row, so a rectangle entirely right of or below the image is an error (token `clap`) and one entirely left of or above it keeps the full width or height. An alpha plane that belongs to a grid (the alpha items are those of the tiles, the grid has no alpha item) is assembled into the grid image and takes the transforms of the grid (`grid.cc:575-604`, `image_item.cc:1406-1426`); the oracle keeps it as an extra plane of the main `apply_transforms` call. Otherwise alpha is decoded as its own item with its own transforms, not those of the main image, and scaled (nearest neighbour) to the size of the main image if the sizes differ. Auxiliary images likewise take only their own transforms.

**Plane format** (`.planes`, both sides, one file per image named `<item id>.planes`):

```
planes <chroma 400|420|422|444> <w>x<h> <nplanes>\n      w x h is the luma size
<w>x<h> <bytes per sample 1|2>\n                          one line per plane
<samples of plane 0><samples of plane 1>...                rows packed, stride = width, 2-byte samples little-endian
```

Planes are Y, then Cb and Cr unless the chroma is 400, then an alpha plane if the image has one (full size). Chroma planes are `ceil(w/sx) x ceil(h/sy)`. A sample is 1 byte when libheif reports 8 bits or fewer for that channel, else 2 bytes (the value in the low bits). An image that cannot be written (not planar YCbCr or monochrome) gets `<id>.err` with a token in the raw tree and is not compared. In the result trees `<id>.err` holds `clap` (the one error libheif and `Error::Malformed("clap")` share), `decode` or `error`.

**Trees.** `raw/<name>/` is the libheif decode without transforms; `exp/<name>/` is libheif with the transforms; `rs/<name>/` is `oracle transform`, which parses the same file, applies `Image::transforms` of item `<id>` (for the alpha plane: the transforms of the alpha item) to the raw planes with `apply_transforms`, and writes the same format. `oracle transform-cmp <raw> <exp> <rs> <known.tsv> <graph-known.tsv> <report-dir>` compares `exp` with `rs`. A file that `isobmff::parse` rejects and that `tests/parity/known-differences.tsv` lists with side `heif` (a graph known difference, for example `meta` without a first `hdlr`) is not compared and not a transform difference; it is counted in the summary row "files isobmff-rs rejects (graph known difference)". A derived image (grid, `iden`, overlay) whose inputs, at any depth, have transforms is not compared either (summary row "derived images whose inputs have transforms"): libheif passes `ignore_transformations` on to the inputs (`grid.cc:543` and `:677`, `iden.cc:98`, `overlay.cc:392`), so its raw planes also lack the transforms of the inputs (MPEG conformance C023, a grid of cropped `iden` tiles; C039, an `iden` of an `iden`, both with `clap` and `irot`).

**Gate 6** (`transform.diff` must be empty):

- Every image with raw planes must give the same bytes (header and planes), or the same error token on both sides.
- Known differences: `tests/parity/transform-known-differences.tsv` (file, image id, reason) lists images where libheif changes the chroma format of a 4:2:0 or 4:2:2 image to 4:4:4 (reason `libheif-444`). Only libheif changes the chroma format; isobmff-rs keeps it, except 4:2:2 turned by 90 or 270 degrees, which has no 4:2:2 result and becomes 4:4:4 by sample replication, and libheif's bilinear upsampling differs from replication. For a listed image the luma plane and the alpha plane must still be equal, the chroma planes are skipped. A listed entry fails the gate as stale when libheif's result is not 4:4:4 for a 4:2:0 or 4:2:2 raw image, or when the file was not compared at all.
- The rows of the generated files (`unci_*`) are not in the file: `oracle gen-transforms <out-dir> <known.tsv>` writes them next to the files, from the geometry alone, with the libheif rules (4:2:0: rotate 90 with odd width, rotate 270 with odd height, rotate 180 and mirror with an odd width or height, crop with odd left or top; 4:2:2: rotate 90 and 270 always, rotate 180 with odd height, mirror left/right with odd width, crop with odd left), and `transform.sh` adds them. The rows are a first-principles prediction: the first CI run decides which are real, and corpus rows go in the committed file.
- Coverage: at least `TRANSFORM_MIN_IMAGES` (default 200) images compared.

**Inputs.** The HEIF/AVIF corpus of corpus.toml (no `mp4-*`), and the generated files: `oracle gen-transforms` writes about 450 minimal HEIF files with `unci` items (ISO/IEC 23001-17, `uncC` version 0, one tile, component interleave) and no codec: 8-bit with chroma 4:0:0, 4:2:0, 4:2:2 and 4:4:4 at 7x5, 16x12 and 33x18 for each of 22 transform sets (none, `irot` 90/180/270, `imir` both axes, `clap` with even offsets, odd x, odd y, odd x and y, odd size, partly outside left/top and right/bottom, entirely outside right (error) and entirely outside left, clap + irot + imir in MIAF order, irot + imir and imir + irot, rotation then clap); 10, 12 and 16 bits for four of the sets; an alpha `auxl` item with the same transforms, its own transforms, or none while the main image has some; an Apple gain map (`urn:com:apple:photo:2020:aux:hdrgainmap`) `auxl` item with its own transforms. Fractional clap geometry (odd W with even clapW, fractional width, denominators of 3, a negative half offset) on 4:0:0 and 4:4:4, and 4:2:2 rotate 90/270 with row-constant chroma (no known row, chroma compared exactly). Sample values come from an own xorshift, so the files are identical on every run. No corpus file was rewritten to add transform properties: the unci files cover the subsampled formats libheif decodes without a codec, and the corpus is read as it is, with its own transforms.

The `unci` encoding follows what libheif v1.23.7 reads: 8 and 16 bits fill their bytes (16 bits big-endian), 10 and 12 bits sit in big-endian 16-bit containers (`component_align_size` 2), chroma planes of 4:2:2 and 4:2:0 are stored with `floor(w/2)` columns and `floor(h/2)` rows for 4:2:0, so for an odd size libheif leaves the last chroma column or row zero; both sides start from the same raw planes, so it does not matter for the gate.

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
- Construction method 2 (item offset) is resolved by the crate but not by libheif. The `oracle avif`
  rebuild does not follow it (it reports an item error when a method-2 item's range or Exif is
  needed, as before): the libavif side was not checked for method 2, and no AVIF corpus file uses it.
  The heif side compares the item bytes; libheif does not resolve method 2, so a diff is expected there. The one
  corpus file with method 2, `iff_hevc_tile_multiple_items_tbas.heic`, keeps its line in
  `known-differences.tsv` until CI parity shows its diff is empty.
