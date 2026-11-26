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

## Hosted-runner baseline

bench.yml run 45710469541 on main a1dbc6b, on a GitHub-hosted `ubuntu-24.04` runner (median ns per file,
20 iterations; information only). This is the baseline for later runs. The older numbers below come
from a self-hosted runner, on a different machine: compare ratios, not ns.

### libheif (560 files timed by all three tools)

| size class (file) | files | libheif ns | isobmff-rs 1 thread ns | isobmff-rs default ns | 1 thread / libheif | default / libheif |
|---|---|---|---|---|---|---|
| <= 4 KiB | 272 | 4981.5 | 701 | 701 | 0.14 | 0.14 |
| <= 64 KiB | 113 | 8344 | 1332 | 1352 | 0.16 | 0.16 |
| <= 1 MiB | 153 | 5083 | 762 | 772 | 0.15 | 0.15 |
| <= 16 MiB | 22 | 7780.5 | 1102 | 1081 | 0.14 | 0.14 |
| all | 560 | 5387 | 791 | 791 | 0.15 | 0.15 |

### libavif (207 AVIF files timed by all three tools)

| size class (file) | files | libavif ns | isobmff-rs 1 thread ns | isobmff-rs default ns | 1 thread / libavif | default / libavif |
|---|---|---|---|---|---|---|
| <= 4 KiB | 65 | 1204 | 901 | 1212 | 0.75 | 1.01 |
| <= 64 KiB | 29 | 1309 | 1382 | 1422 | 1.06 | 1.09 |
| <= 1 MiB | 98 | 941.5 | 761 | 771 | 0.81 | 0.82 |
| <= 16 MiB | 15 | 1099 | 1092 | 1072 | 0.99 | 0.98 |
| all | 207 | 951 | 811 | 812 | 0.85 | 0.85 |

### apply_transforms on 8000 x 6000 4:2:0 planes, one thread, median of 5 (ms)

| case | ms |
|---|---|
| transform__rotate90__8bit | 75.70 |
| transform__rotate180__8bit | 20.43 |
| transform__mirror-lr__8bit | 15.71 |
| transform__mirror-tb__8bit | 9.18 |
| transform__clap__8bit | 28.18 |
| transform__rotate90__10bit | 119.99 |
| transform__rotate180__10bit | 28.42 |
| transform__mirror-lr__10bit | 20.77 |
| transform__mirror-tb__10bit | 16.33 |
| transform__clap__10bit | 54.81 |

## Earlier self-hosted runner

bench.yml run 44579129000 on main ebc589b (median ns per file, 20 iterations; information only):

### libheif (552 files timed by all three tools)

| size class (file) | files | libheif ns | isobmff-rs 1 thread ns | isobmff-rs default ns | 1 thread / libheif | default / libheif |
|---|---|---|---|---|---|---|
| <= 4 KiB | 268 | 4079.5 | 640 | 630 | 0.16 | 0.15 |
| <= 64 KiB | 109 | 6878 | 1270 | 1260 | 0.18 | 0.18 |
| <= 1 MiB | 153 | 4166 | 750 | 730 | 0.18 | 0.18 |
| <= 16 MiB | 22 | 6301.5 | 920 | 940 | 0.15 | 0.15 |
| all | 552 | 4317 | 740 | 720 | 0.17 | 0.17 |

### libavif (203 AVIF files timed by all three tools)

| size class (file) | files | libavif ns | isobmff-rs 1 thread ns | isobmff-rs default ns | 1 thread / libavif | default / libavif |
|---|---|---|---|---|---|---|
| <= 4 KiB | 62 | 1050 | 770 | 850 | 0.73 | 0.81 |
| <= 64 KiB | 28 | 1085.5 | 1320 | 1300 | 1.22 | 1.20 |
| <= 1 MiB | 98 | 733.5 | 740 | 720 | 1.01 | 0.98 |
| <= 16 MiB | 15 | 884 | 910 | 930 | 1.03 | 1.05 |
| all | 203 | 761 | 750 | 730 | 0.99 | 0.96 |
