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
| `iloc` | `__fuzz::iloc` | iloc v0..v2 parse and extent resolution |
| `iinf` | `__fuzz::iinf` | iinf and infe v0..v3 |
| `iref` | `__fuzz::iref` | iref v0/v1 |
| `ipma` | `__fuzz::ipma` | ipma v0/v1, both flag widths |
| `ipco` | `__fuzz::ipco` | every property parser |
| `derived` | `__fuzz::grid` and `__fuzz::iovl` | grid and overlay payloads |
| `tmap` | `__fuzz::tmap` | ISO 21496-1 payload, both layouts |

Every target uses a `max_alloc` far below `-malloc_limit_mb=512`; a peak above it is a bug in the
charging.

## CI

`.github/workflows/fuzz.yml` runs weekly, Monday 12:23 UTC (`cron: '23 12 * * 1'`), and on
`workflow_dispatch`; never on push or pull request. Matrix over the ten targets with
`max-parallel: 2` (x86 only: no SIMD, so no arm64 job). Each target runs
`cargo +nightly fuzz run --fuzz-dir tests/fuzz <target> ... -- -max_total_time=$MAX_TOTAL_TIME
-timeout=60 -rss_limit_mb=2048 -malloc_limit_mb=512`. Crash artifacts are uploaded; the grown
corpus is kept in the Actions cache.

Seeds are the first 64 KiB of every parity corpus file, gathered in the fuzz image build
(`tests/parity/run.sh gather`), plus `tests/data/fuzz-grown` if present.

Fuzzing runs in CI only. Dispatch a 15-minute run per target:

```
gh workflow run fuzz.yml --ref <branch> -f max_total_time=900
gh workflow run fuzz.yml --ref <branch> -f only=tmap
```

## Fuzz hours

TBD: sum of `max_total_time` x targets over the runs on `main`.
