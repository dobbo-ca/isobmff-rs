# Dependencies

The crate has none: `[dependencies]` is empty, nothing in `[build-dependencies]`, no `-sys` crate,
no FFI. `tests/fuzz` depends on `libfuzzer-sys` (the one allowed exception) and is a separate
crate.

## CI-only oracles

Both run only in CI, built from source with no codecs. Neither is linked into the crate or shipped
with it.

| Oracle | Version | Licence | Use |
|---|---|---|---|
| libheif | 1.23.7 (`12a259a5`) | LGPL-3.0 | item graph of every file, item bytes, images (`tests/parity/c/heif_oracle.c`). Design reference only: no code copied. |
| libavif | 1.4.2 (`c5240fc7`) | BSD-2-Clause | AVIF primary image: size, depth, alpha, ICC, Exif, XMP, transforms, extent, gain map (`tests/parity/c/avif_oracle.c`). |

libavif is BSD-2, so two of its rules are ported with attribution in `NOTICE`: the clean-aperture
crop rectangle (`Clap::crop`) and the gain map payload checks. The libultrahdr (MIT or Apache-2.0)
common-denominator layout was read, not copied. The corpus licences are in docs/corpus.md.
