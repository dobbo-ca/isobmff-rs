# Parity corpus

Public HEIF/AVIF and MP4/MOV samples at pinned commits, listed in `tests/parity/corpus.toml`. Nothing of it is
stored in this repo: `tests/parity/fetch.sh` makes a blobless sparse checkout per source into the
BuildKit cache mount `isobmff-rs-data`. About 634 HEIF and AVIF files; gate 4 fails below `PARITY_MIN_INPUTS`
(600). The MP4/MOV sources (named `mp4-*`, about 200 files, see below) are read by gate 5 only: it also reads
every file above that has a `moov` (the animated AVIF files). `PARITY_MIN_MP4` (100) is a guess: set it from
the first green run. Research of 2026-11-19 used the GitHub API only; the feature tags below come from each
source's own docs and file names, not from inspecting the files, so check them with the first run.

## Sources

| Source | Commit | Files | Licence (quoted) | Covers |
|---|---|---|---|---|
| libavif `tests/data` | `17bdf4c6` | 63 | `tests/data/README.md:9`: "License: [same as libavif]" (BSD-2); kodim: "released by the Eastman Kodak Company for unrestricted usage"; cosmos1650: CC BY 3.0 | grid, alpha, ICC, Exif/XMP, idat, avis, a1lx/a1op/lsel, clap/irot/imir, tmap gain maps |
| libheif `tests/data`, `examples`, `fuzzing/data` | `12a259a5` | 240 | `COPYING`: "The library libheif is distributed under the terms of the GNU Lesser General Public License. The sample applications and the Go and C++ wrappers are distributed under the terms of the MIT License." No data-specific grant | alpha, clap, thumbnails, malformed fuzz corpus |
| link-u/avif-sample-images | `c666a368` | 156 | `README.md:9`: "Most images are licensed under CC-BY-SA 4.0"; red-at-12-oclock: "GNU LGPL v2.1 or 2 claused BSD License" | alpha, ICC, avis, clap/irot/imir |
| AOMediaCodec/av1-avif | `bf4c18d1` | 17 | Apple `license.txt`: "CC-BY-SA-4.0"; Xiph `COPYING`: CC BY-SA 3.0 and CC BY 3.0; Microsoft `ReadMe.md`: bbb "Creative Commons Attribution 3.0" | multilayer a1lx/a1op/lsel, grids, alpha |
| MPEGGroup/FileFormatConformance | `767a5bcb` | 61 (LFS) | `LICENSE:1-3`: "The Clear BSD License / Copyright (c) 2023, Apple Inc." | HEIF conformance: grid, iovl, alpha, thumbnails, Exif, clap/irot/imir, sequences |
| nokiatech/heif (gh-pages) | `f01283f3` | 22 | `CONTENT-LICENSES.md:148-160`, section 7: "Freely redistributable for conformance testing and demonstration." (C71.heic, C75.hmg, C83.hmg, 1dualMiniVVC.hmg are listed by name) | conformance files not in the MPEG repo (C043..C053, C71, MIAF); `mini` box files: C71 (HEVC, orientation 8), C75, C83 and 1dualMiniVVC (VVC `.hmg`: libheif and isobmff-rs reject them, no codec types) |
| mozilla/mp4parse-rust | `d3a9abfc` | 69 | no data statement; repo MPL-2.0 | small valid and corrupt AVIF: colr kinds, ipma, essential flags, odd box sizes |
| imazen/heic `testdata/apple-hdr`, `testdata/libheif-examples/lightning_mini.heif` | `4d72cadd` | 2 | `testdata/README.md`: apple-hdr "Licensed under the MIT License"; `testdata/README.md:7-10`: libheif-examples "Licensed under the MIT License (see `libheif-examples/COPYING`)" | Apple `hdrgainmap` aux image and its XMP; the pre-final draft syntax of the `mini` box (byte-identical to libheif 0733824f `tests/data/lightning_mini.heif`, replaced there on 2026-04-12); isobmff-rs parses it (hvcC Invalid) |
| aaron-rust/hdr-sample-images | `df0d5df4` | 4 (LFS) | `README.md:31`: "All image files in this repository are licensed under the Creative Commons Zero (CC0) license" | large HEVC/AV1 images, grids, HDR |

## MP4/MOV sources (gate 5)

| Source | Commit | Files | Licence (quoted) | Covers |
|---|---|---|---|---|
| MPEGGroup/FileFormatConformance `mp4-ffc` | `767a5bcb` | about 160 (LFS) | `LICENSE:1-3`: "The Clear BSD License / Copyright (c) 2023, Apple Inc."; `README.md:37`: "This project operates under the Clear BSD License"; the per-file `.json` licence is empty except for the RA_BodeMuseum files (CC BY-NC-ND 4.0, excluded) | `ctts` v0 (about 45 files) and v1 with negative offsets (`17_negative_ctso`, `hvc2_extractors`, `vvc_basic_track`), `elst` with media_time, `stz2` (`a7-tone-oddities`, `04_bifs_video`, `02_dref_edts_img`), external `url ` drefs (`02_dref_edts_img`, `04_bifs_video`, `a6_tone_multifile`), `sbgp 'rap '`, avc1, hvc1/hev1, mp4v, vvc1, samr, encv; fragmented files (kept raw for now). No AV1. |
| link-u/avif-sample-images `mp4-link-u` | `c666a368` | 1 (`star.mp4`) | `README.md:550`: "License: CC-BY" | AV1 in MP4 with `stss` and `elst`; its `star-*.avifs` are in the HEIF source above |
| mozilla/mp4parse-rust `mp4-mp4parse` | `d3a9abfc` | about 40 | no data statement; repo MPL-2.0 (same basis as above); `bbb_*` is Blender, CC BY | `chunk_out_of_range`, `stco_extra_chunk`, `zero_empty_stsc`, `no_timescale`, `tiny_av1`, amr/h263/mp4v 3gp, init-only files with 0 samples |

Also in gate 5, from the HEIF sources above: the animated AVIF files of libavif (`colors-animated-*`,
`animated_alpha_sample_without_shown_frame`: av01 `trak`, `stss`, `elst`, an alpha `auxv` track, an `mp4a` track), the
`star-*.avifs` of link-u and the avis files of mp4parse-rust.

Not used in gate 5: the 12 FFC segment files that have no `moov` (`mp4-live-LastTime-depRep-R*-000*.mp4`,
`hev1_clg1_segment.m4s`, `DDF_10s_25fps-dynamic_rep-R*_seg-0005.3gs`; they need their init file in front, and
fragments are the next step), the two FFC `.mp4.zip` files of more than 4 GB (the only `co64` files), the 6
RA_BodeMuseum files, mp4parse-rust `bipbop*` (Apple content, no grant). Not usable on licence: FFmpeg FATE
(no licence; best edit-list cases), Chromium `media/test/data` (no per-file grant), gpac testsuite, Bento4
`Test/Data` (GPL), libheif sequence files (no data grant). `fetch.sh` passes the exclude globs of an LFS
source to `git lfs pull`, so an excluded file is not downloaded.

## Gaps

- No source with a clear licence has: a tmap gain map in HEIC, `iloc` v2, `largesize`, an iPhone
  depth map, or a `mini` box with a gain map (libavif builds those in memory only, in
  `avifminitest`).
- The 11 public `mini` box files are 4:2:0, 4:4:4 or alpha images; none sets `hdr`. `C83_dup.hmg`
  is the same blob as `C83.hmg` and is left out.

## Excluded

| Source | Reason |
|---|---|
| nokiatech/heif `master` | "Licensed Field means the non-commercial purposes of evaluation, testing and academic research"; no sample files |
| nokiatech/heif_conformance | no licence file in the tree |
| av1-avif `Netflix/**`, Microsoft Chimera files | CC BY-NC-ND 4.0 |
| libavif `arc_triomphe_*.avif` | CC BY-NC 2.5 |
| PhotoPrism samples | proprietary "Documentation and Digital Assets" agreement |
| Nokia `content/nokia`, `content/personal` | no grant, or evaluation only |
| imazen/heic `testdata/features`, `synthetic` | undocumented data under AGPL-3.0; some look like Nokia copies |
| pillow_heif, Exiv2, libvips, Pillow | no licence for the data |
| Chromium and WebKit test images | licence not verified per file |
| av1-avif Elecard-derived and Exif-copyright files | no stated grant |
