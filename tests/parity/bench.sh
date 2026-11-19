#!/bin/sh
# bench.sh <report-dir>: libheif (heif-oracle --bench), libavif (avif-oracle --bench) and isobmff-rs
# (isobmff-bench, 1 thread and default threads) open the same corpus ITERS times (default 20) in
# one container; summary.md has the median ns per open and walk by file size class and the ratio
# isobmff-rs / C library (below 1 means isobmff-rs is faster). Information only, no gate: the
# status is 0 unless a tool fails or no file was timed by all the tools of the first table.
# The tools print TSV `name<TAB>ns`; the corpus is the cache mount of fetch.sh under /data/x. The
# parser has no threads, so the 1-thread and default columns are expected to be equal.
set -u
export LC_ALL=C
REP=$1 iters=${ITERS:-20} W=/work D=${DATA:-/data} tab=$(printf '\t')
rm -rf $W; mkdir -p "$REP" $W/corpus
find "$D/x" -type f | while IFS= read -r f; do
  cp "$f" "$W/corpus/$(printf '%s' "${f#"$D"/x/}" | sed 's#/#__#g')"
done
bad=0
find $W/corpus -type f -printf '%f\t%s\n' | sort -t "$tab" -k1,1 > $W/sizes.tsv
heif-oracle --bench "$iters" $W/corpus > $W/h.raw || bad=1
avif-oracle --bench "$iters" $W/corpus > $W/a.raw || bad=1
isobmff-bench --threads 1 "$iters" $W/corpus > $W/r1.raw || bad=1
isobmff-bench "$iters" $W/corpus > $W/rn.raw || bad=1
for t in h a r1 rn; do sort -t "$tab" -k1,1 $W/$t.raw > $W/$t.tsv; done
# name size libheif ns, rust 1 thread ns, rust default ns (and the same with libavif)
join -t "$tab" $W/sizes.tsv $W/h.tsv | join -t "$tab" - $W/r1.tsv | join -t "$tab" - $W/rn.tsv > $W/heif.tsv
join -t "$tab" $W/sizes.tsv $W/a.tsv | join -t "$tab" - $W/r1.tsv | join -t "$tab" - $W/rn.tsv > $W/avif.tsv
[ -s $W/heif.tsv ] || bad=1

# median <file> <column> <min size> <max size>: median of the column over the files of one class.
median() {
  awk -F'\t' -v c="$2" -v lo="$3" -v hi="$4" '$2 > lo && $2 <= hi { print $c }' "$1" | sort -n |
    awk '{ a[NR] = $1 } END { if (NR == 0) print "-"; else print ((NR % 2) ? a[(NR + 1) / 2] : (a[NR / 2] + a[NR / 2 + 1]) / 2) }'
}
count() { awk -F'\t' -v lo="$2" -v hi="$3" '$2 > lo && $2 <= hi { n++ } END { print n + 0 }' "$1"; }
ratio() { awk -v a="$1" -v b="$2" 'BEGIN { if (a == "-" || b == "-" || b == 0) print "-"; else printf "%.2f", a / b }'; }

# table <file> <C library name>
table() {
  echo "| size class (file) | files | $2 ns | isobmff-rs 1 thread ns | isobmff-rs default ns | 1 thread / $2 | default / $2 |"
  echo "|---|---|---|---|---|---|---|"
  lo=-1
  for cls in "<= 4 KiB:4096" "<= 64 KiB:65536" "<= 1 MiB:1048576" "<= 16 MiB:16777216" "> 16 MiB:99999999999"; do
    label=${cls%:*} hi=${cls##*:}
    n=$(count "$1" $lo $hi)
    if [ "$n" -gt 0 ]; then
      c=$(median "$1" 3 $lo $hi) r1=$(median "$1" 4 $lo $hi) rn=$(median "$1" 5 $lo $hi)
      echo "| $label | $n | $c | $r1 | $rn | $(ratio "$r1" "$c") | $(ratio "$rn" "$c") |"
    fi
    lo=$hi
  done
  n=$(count "$1" -1 99999999999)
  if [ "$n" -gt 0 ]; then
    c=$(median "$1" 3 -1 99999999999) r1=$(median "$1" 4 -1 99999999999) rn=$(median "$1" 5 -1 99999999999)
    echo "| all | $n | $c | $r1 | $rn | $(ratio "$r1" "$c") | $(ratio "$rn" "$c") |"
  fi
}

{
  echo "## isobmff-rs vs libheif and libavif: open + walk, median ns per file (lower is better)"; echo
  echo "$iters iterations per file. Information only, no gate. libheif: alloc + read from memory + items,"
  echo "references, properties; libavif: create + parse of the primary item; isobmff-rs: parse + the"
  echo "same enumeration. A file that the C library does not open is not timed."; echo
  echo "### libheif ($(wc -l < $W/heif.tsv) files timed by all three tools)"; echo
  table $W/heif.tsv libheif; echo
  echo "### libavif ($(wc -l < $W/avif.tsv) AVIF files timed by all three tools)"; echo
  table $W/avif.tsv libavif
} > "$REP/summary.md"
cp $W/heif.tsv "$REP/heif.tsv"; cp $W/avif.tsv "$REP/avif.tsv"
rm -rf $W
exit $bad
