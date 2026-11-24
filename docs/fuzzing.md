# Fuzzing

`tests/fuzz` is a cargo-fuzz crate (run with `--fuzz-dir tests/fuzz`); it is the only place that
depends on `libfuzzer-sys`. One target per entry point. Each runs with small `Limits` and checks
that the call returns (never panics, never hangs); an error may be any variant. Parser targets
call `isobmff::__fuzz::*` (hidden, `fn(&[u8], &Limits) -> Result<(), Error>` on a raw box payload).

| Target | Entry point | Checks |
|---|---|---|
| `parse` | `parse` on the whole input, then every graph call and `metadata` | graph building, every `Image`, item bytes, no panic |
| `read` | `read` over a `Cursor` | equals `parse` on the same bytes (`Ok` and `Err`) |
| `boxes` | `__fuzz::boxes` | box tree walked to `max_depth`, size/largesize/uuid headers |
| `iloc` | `__fuzz::iloc` | iloc v0..v2 parse and extent resolution, construction method 2 over neighbouring items |
| `iinf` | `__fuzz::iinf` | iinf and infe v0..v3 |
| `iref` | `__fuzz::iref` | iref v0/v1 |
| `ipma` | `__fuzz::ipma` | ipma v0/v1, both flag widths |
| `ipco` | `__fuzz::ipco` | every property parser |
| `derived` | `__fuzz::grid` and `__fuzz::iovl` | grid and overlay payloads |
| `tmap` | `__fuzz::tmap` | ISO 21496-1 payload, both layouts |
| `movie` | `__fuzz::movie` | `parse_movie` on the whole input (`moov` and every `moof`: `trex`, `tfhd`, `tfdt`, `trun`), then for every track a walk of its first 65 536 samples (moov and fragment samples), `sync_samples(4)` and `sync_sample_at` at `i64::MIN`, 0 and `i64::MAX`, over `Cursor::new(data)` (the walks stop at a sample number above twice the input length, so they need no sample-count skip) |
| `movie_read` | `read_movie` over a `Cursor` | equals `parse_movie` (`Ok` and `Err`; each `moof` is copied and parsed in turn), and the first 256 samples of every track are equal |
| `transform` | `apply_transforms` and `transformed_size` | up to 64 x 64 planes, any chroma, 8 or 16 bits per plane, extra planes, up to six transforms with arbitrary clap fields; no panic, and on `Ok` the output size equals `transformed_size` and the layout is valid |
| `mini` | `__fuzz::mini` | the bit header and chunks of a `mini` box, then the graph built from it (the `ftyp` minor version is `avif` for an even length, `heic` for an odd one) |

Every target uses a `max_alloc` far below `-malloc_limit_mb=512`; a peak above it is a bug in the
charging.

## CI

`.github/workflows/fuzz.yml` runs weekly, Monday 12:23 UTC (`cron: '23 12 * * 1'`), and on
`workflow_dispatch`; never on push or pull request. Matrix over the fourteen targets with
`max-parallel: 2` (x86 only: no SIMD, so no arm64 job; the conventions ask for a fuzz-arm64 matrix, left out on purpose). Each target runs
`cargo +nightly fuzz run --fuzz-dir tests/fuzz <target> ... -- -max_total_time=$MAX_TOTAL_TIME
-timeout=60 -rss_limit_mb=2048 -malloc_limit_mb=512`. Crash artifacts are uploaded; the grown
corpus is kept in the Actions cache.

Seeds are the first 64 KiB of every parity corpus file (the MP4 files up to 1 MiB whole, as their `moov` is often
past 64 KiB), gathered in the fuzz image build
(`tests/parity/run.sh gather`), plus `tests/data/fuzz-grown` if present.

Fuzzing runs in CI only. Dispatch a 15-minute run per target:

```
gh workflow run fuzz.yml --ref <branch> -f max_total_time=900
gh workflow run fuzz.yml --ref <branch> -f only=tmap
```

## Fuzz hours

2.5 h, 0 crashes: fuzz.yml run 44581507012 on main 68eaa95, 10 targets x 900 s.

Five targets (`iloc`, `iinf`, `iref`, `ipma`, `ipco`) and `derived`, `tmap` and `mini` take a raw box or
item payload, which a whole-file seed never reaches. `tests/fuzz/payload-seeds/<target>/` holds
small hand-written payloads for them; the workflow passes that directory as an extra seed dir.
The `movie` and `movie_read` targets take whole files, but a fragmented file is rare in a corpus cut at
64 KiB, so `payload-seeds/movie/fragmented` and `payload-seeds/movie_read/fragmented` (the same 476 bytes,
written with `printf`: `ftyp`, `moov` with `mvex`/`trex`, one `moof` with `tfhd`, `tfdt` and `trun`, `mdat`)
seed the fragment parsers. `tests/movie.rs` parses that file, so a change to the format shows there.
