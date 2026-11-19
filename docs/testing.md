# Testing

All builds and tests run in CI (nothing runs on the owner's Mac).

| Where | Covers |
|---|---|
| `src/boxes.rs` | box headers: size, largesize, size 0, uuid, bad sizes, depth and box limits |
| `src/ftyp.rs`, `src/meta/*` | brands; iinf/infe, iloc v0..v2 (extent resolution, idat, errors), iref, ipma/ipco |
| `src/props/*` | ispe, pixi, colr, irot, imir, clap (`Clap::crop` rule), auxC, hvcC, av1C |
| `src/derived.rs` | grid, iovl, tmap: both tmap layouts, bad denominators, trailing bytes |
| `src/graph.rs`, `src/metadata.rs` | image kinds, transforms, alpha/depth/thumbnails, gain maps, top-level images, Exif/XMP bytes |
| doc-test in `src/lib.rs` | `parse`, `primary_image`, `item_bytes` and `read` on a built file, with explicit `Limits` |
| `tests/parity` | libheif and libavif against `examples/oracle.rs` on the corpus (docs/parity.md) |
| `tests/fuzz` | ten cargo-fuzz targets (docs/fuzzing.md) |

Parity gates (`tests/parity/run.sh full`): corpus `diff -r` with `known-differences.tsv`; mutated
files (truncations, bit flips, size edits) where both sides are `ok`; `--threads 1` and `--read`
equal the default; a minimum input count (`PARITY_MIN_INPUTS`).

## CI jobs (`.github/workflows/ci.yml`)

- `test`: fmt, clippy `-D warnings`, `cargo test`. Runs on pull requests; the required check.
- `arm64`: `cargo test` under qemu. No SIMD, so it checks arithmetic on a second target.
- `parity`, `parity-arm64`: the oracle gates.
- `fuzz` (`fuzz.yml`) and `bench` (`bench.yml`): weekly and information only.

The crate is done when `arm64`, `parity` and `parity-arm64` are green on `main`.
