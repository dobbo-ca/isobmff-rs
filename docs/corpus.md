# Parity corpus

Public HEIF/AVIF samples at pinned commits, listed in `tests/parity/corpus.toml`. Nothing of it is
stored in this repo: `tests/parity/fetch.sh` makes a blobless sparse checkout per source into the
BuildKit cache mount `isobmff-rs-data`. About 630 files; the gate fails below `PARITY_MIN_INPUTS`
(600). Research of 2026-11-19 used the GitHub API only; the feature tags below come from each
source's own docs and file names, not from inspecting the files, so check them with the first run.

## Sources

| Source | Commit | Files | Licence (quoted) | Covers |
|---|---|---|---|---|
| libavif `tests/data` | `17bdf4c6` | 63 | `tests/data/README.md:9`: "License: [same as libavif]" (BSD-2); kodim: "released by the Eastman Kodak Company for unrestricted usage"; cosmos1650: CC BY 3.0 | grid, alpha, ICC, Exif/XMP, idat, avis, a1lx/a1op/lsel, clap/irot/imir, tmap gain maps |
| libheif `tests/data`, `examples`, `fuzzing/data` | `12a259a5` | 240 | `COPYING`: "The library libheif is distributed under the terms of the GNU Lesser General Public License. The sample applications and the Go and C++ wrappers are distributed under the terms of the MIT License." No data-specific grant | alpha, clap, thumbnails, malformed fuzz corpus |
| link-u/avif-sample-images | `c666a368` | 156 | `README.md:9`: "Most images are licensed under CC-BY-SA 4.0"; red-at-12-oclock: "GNU LGPL v2.1 or 2 claused BSD License" | alpha, ICC, avis, clap/irot/imir |
| AOMediaCodec/av1-avif | `bf4c18d1` | 17 | Apple `license.txt`: "CC-BY-SA-4.0"; Xiph `COPYING`: CC BY-SA 3.0 and CC BY 3.0; Microsoft `ReadMe.md`: bbb "Creative Commons Attribution 3.0" | multilayer a1lx/a1op/lsel, grids, alpha |
| MPEGGroup/FileFormatConformance | `767a5bcb` | 61 (LFS) | `LICENSE:1-3`: "The Clear BSD License / Copyright (c) 2023, Apple Inc." | HEIF conformance: grid, iovl, alpha, thumbnails, Exif, clap/irot/imir, sequences |
| nokiatech/heif (gh-pages) | `f01283f3` | 19 | `CONTENT-LICENSES.md` section 7: "Freely redistributable for conformance testing and demonstration." | conformance files not in the MPEG repo (C043..C053, C71, MIAF) |
| mozilla/mp4parse-rust | `d3a9abfc` | 69 | no data statement; repo MPL-2.0 | small valid and corrupt AVIF: colr kinds, ipma, essential flags, odd box sizes |
| imazen/heic `testdata/apple-hdr` | `4d72cadd` | 1 | `testdata/README.md`: "Licensed under the MIT License" | Apple `hdrgainmap` aux image and its XMP |
| aaron-rust/hdr-sample-images | `df0d5df4` | 4 (LFS) | `README.md:31`: "All image files in this repository are licensed under the Creative Commons Zero (CC0) license" | large HEVC/AV1 images, grids, HDR |

## Gaps

- No source with a clear licence has: a tmap gain map in HEIC, `iloc` v2, `largesize`, or an iPhone
  depth map.

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
