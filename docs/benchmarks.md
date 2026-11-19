# Benchmarks

There is no speed bar: the numbers are information only and never fail a job.

`.github/workflows/bench.yml` (`workflow_dispatch` and push to `main`, not required) builds the CI
image and runs `tests/parity/bench.sh` in one container. libheif (`heif-oracle --bench`), libavif
(`avif-oracle --bench`) and isobmff-rs (`isobmff-bench`, 1 thread and default threads) each open
the whole parity corpus 20 times (`ITERS`). Same machine, same files, so the ratio is meaningful.
The summary gives the median ns per open and walk by file size class and the ratio isobmff-rs /
C library (below 1 means isobmff-rs is faster). The job summary is appended to the run page and
`bench-out` is uploaded as an artifact.

The parser has no threads, so the 1-thread and default columns are expected to be equal.

## Numbers

Not measured yet. The first `bench.yml` run on `main` fills this table.

| Size class | Files | libheif (ns) | libavif (ns) | isobmff-rs (ns) | ratio vs libheif |
|---|---|---|---|---|---|
| TBD | TBD | TBD | TBD | TBD | TBD |
