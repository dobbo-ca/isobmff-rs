#!/bin/sh
# Parity gates of isobmff-rs (docs/parity.md). CI only, called by .github/ci/Dockerfile from the
# repo root; the corpus is the BuildKit cache mount of fetch.sh at /data: the source variants that
# /data.list (fetch.sh <list>, one "name<TAB>key" line per source) names, under /data/v/<key>/<name>.
#   run.sh gather <dst>          seeds for the fuzz targets: the first 65536 bytes of every corpus
#                                file and of tests/data/fuzz-grown/* (if present), each named by
#                                its path with / replaced by __
#   run.sh full <report-dir>     gates 1-5 of docs/parity.md; the report is summary.md, *.diff and
#                                a `status` written by the caller (0 only when every gate passed)
#   run.sh ref <out-dir>         isobmff-oracle (x86_64) on the libheif and libavif test data
#   run.sh arm <ref-dir>         the aarch64 isobmff-oracle under qemu-user on the same files,
#                                compared with <ref-dir>
# Binaries on PATH: isobmff-oracle (examples/oracle.rs), heif-oracle, avif-oracle (tests/parity/c);
# for `arm` /usr/local/bin/isobmff-oracle-arm64, run by $ARM_RUN; for gate 5 ffprobe and gawk.
# Environment: PARITY_MIN_INPUTS (default 600): the HEIF and AVIF corpus has about 634 files
# (corpus.toml), so a source that did not arrive fails gate 4 instead of shrinking the gates.
# PARITY_MIN_MP4 (default 100): the least number of corpus files with a gated MP4 video track.
# The sources named mp4-* in corpus.toml are read by gate 5 only.
set -u
export LC_ALL=C
W=/work
D=${DATA:-/data}
MIN=${PARITY_MIN_INPUTS:-600}
MIN_MP4=${PARITY_MIN_MP4:-100}
KNOWN=tests/parity/known-differences.tsv
tab=$(printf '\t')
LIST=${DATA_LIST:-/data.list}

# corpus <dst> [maxbytes]: the files of every source in $LIST into <dst>, flattened by flat with
# <name>/<path> as the relative path (the names do not depend on the variant key).
corpus() {
  dst=$1 max=${2:-}
  while IFS=$tab read -r name key; do
    find "$D/v/$key/$name" -type f | flat "$dst" "$D/v/$key" $max
  done < "$LIST"
}

# flat <dst> <root> [maxbytes]: the files named on stdin (paths under <root>) into <dst>, flattened
# (/ becomes __); with maxbytes only their first bytes.
flat() {
  while IFS= read -r f; do
    n=$(printf '%s' "${f#"$2"/}" | sed 's#/#__#g')
    if [ -n "${3:-}" ]; then head -c "$3" "$f" > "$1/$n"; else cp "$f" "$1/$n"; fi
  done
}

gather() {
  mkdir -p "$1"
  corpus "$1" 65536
  [ -d tests/data/fuzz-grown ] && find tests/data/fuzz-grown -type f ! -name '.*' | flat "$1" tests/data 65536
  # MP4 files up to 1 MiB whole: the moov box is often past the first 64 KiB.
  while IFS=$tab read -r name key; do
    case $name in mp4-*) find "$D/v/$key/$name" -type f -size -1048577c | flat "$1" "$D/v/$key" ;; esac
  done < "$LIST"
  return 0
}

run() { "$@" || { echo "FAILED: $*" >&2; bad=1; }; }

# robust <tool> <in> <out> <marker>: a batch run of a C oracle. If it fails (libheif aborting on
# one hostile file would lose the rest of the batch), every input is run on its own, and a file
# that kills or hangs the tool gets `crash` or `timeout` as its marker file. The error lines of the
# tool (`name<TAB>message`, on stderr) are kept in "$3.err".
robust() {
  timeout 3600 "$1" "$2" "$3" 2> "$3.err" && return 0
  echo "batch run of $1 failed, retrying every file on its own" >&2
  rm -rf "$3" $W/one; mkdir -p "$3"; : > "$3.err"
  for f in "$2"/*; do
    n=${f##*/}
    mkdir -p $W/one/in; ln "$f" "$W/one/in/$n"
    timeout 120 "$1" $W/one/in "$3" 2>> "$3.err"
    case $? in
      0) ;;
      124) mkdir -p "$3/$n"; echo timeout > "$3/$n/$4" ;;
      *) mkdir -p "$3/$n"; echo crash > "$3/$n/$4" ;;
    esac
    rm -rf $W/one
  done
  return 0
}

# first <file>: the first line, empty when the file is missing.
first() { l=; [ -f "$1" ] && read -r l < "$1"; printf '%s' "$l"; }

# gate <label> <diff-file>: an empty or missing diff passes.
gate() {
  lines=$(cat "$2" 2>/dev/null | wc -l); lines=$((lines + 0))
  if [ "$lines" -eq 0 ]; then
    echo "| $1 | ok |" >> "$REP/summary.md"; rm -f "$2"
  else
    echo "| $1 | FAIL, $lines diff lines |" >> "$REP/summary.md"; bad=1
  fi
}

# okcount <tree> <marker> <names-file>: "ok/error/missing" of the first lines of <marker>.
okcount() {
  o=0 e=0 m=0
  while IFS= read -r n; do
    case $(first "$1/$n/$2") in ok) o=$((o + 1)) ;; '') m=$((m + 1)) ;; *) e=$((e + 1)) ;; esac
  done < "$3"
  echo "$o / $e / $m"
}

# corpus_side <side> <dirA> <dirB> <names-file>: gate 1 for one oracle pair. A name listed in
# known-differences.tsv for this side is printed (D/d-known) but not counted; a listed name whose
# diff is empty, or that was not compared at all, is a stale entry.
corpus_side() {
  : >> $W/d-$1
  while IFS= read -r n; do
    echo "$n$tab$1" >> $W/seen
    reason=$(awk -F'\t' -v n="$n" -v s="$1" '$1 == n && $2 == s { print $3; exit }' $W/known)
    if diff -r "$2/$n" "$3/$n" > $W/d1 2>&1; then
      [ -z "$reason" ] || echo "stale known difference: $n ($1) is now identical" >> $W/d-stale
    elif [ -n "$reason" ]; then
      { echo "=== $n ($1), known: $reason"; head -n 40 $W/d1; } >> $W/d-known
    else
      { echo "=== $n ($1)"; head -n 100 $W/d1; } >> $W/d-$1
    fi
  done < "$4"
}

# mut_side <side> <marker> <dirA> <dirB> <names-file>: gate 2. Where both sides give `ok`, the
# trees must be equal; the ok/error split is counted, not gated.
mut_side() {
  both=0 same=0 known=0 a_only=0 b_only=0 neither=0 skipped=0
  : > $W/d-mut-$1
  while IFS= read -r n; do
    a=$(first "$3/$n/$2"); b=$(first "$4/$n/$2")
    if [ -z "$a" ] && [ -z "$b" ]; then skipped=$((skipped + 1)); continue; fi
    if [ "$a" = ok ] && [ "$b" = ok ]; then
      both=$((both + 1))
      # a mutation of a file listed in known-differences.tsv for this side is not diffed
      if awk -F'\t' -v n="${n%.*}" -v s="$1" '$1 == n && $2 == s { f = 1 } END { exit !f }' $W/known; then
        known=$((known + 1))
      elif diff -r "$3/$n" "$4/$n" > $W/d1 2>&1; then same=$((same + 1))
      else { echo "=== $n ($1)"; head -n 100 $W/d1; } >> $W/d-mut-$1; fi
    elif [ "$a" = ok ]; then a_only=$((a_only + 1))
    elif [ "$b" = ok ]; then b_only=$((b_only + 1))
    else neither=$((neither + 1)); fi
  done < "$5"
  {
    echo "| $1 mutations: both ok, equal / both ok, different / of a known-difference file, not diffed | $same / $((both - same - known)) / $known |"
    echo "| $1 mutations: only $6 ok / only isobmff-rs ok / both error | $a_only / $b_only / $neither |"
    [ "$skipped" -eq 0 ] || echo "| $1 mutations: not read by either tool | $skipped |"
  } >> $W/split.md
}

# errors <pair> <oracle.err> <isobmff-rs.err>: one line per file name, `name pair oracle isobmff-rs`,
# the error lines of each side joined with " | ". The side is told by the file name, not by FNR,
# so an empty first file does not shift the sides.
errors() {
  : >> "$2"; : >> "$3"
  awk -F'\t' -v pair="$1" '
    NF >= 2 {
      side = (FILENAME == ARGV[1]) ? 1 : 2
      msg = substr($0, length($1) + 2)
      names[$1] = 1
      if (side == 1) o[$1] = ($1 in o) ? o[$1] " | " msg : msg
      else r[$1] = ($1 in r) ? r[$1] " | " msg : msg
    }
    END { for (n in names) printf "%s\t%s\t%s\t%s\n", n, pair, o[n], r[n] }' "$2" "$3" | sort
}

# mp4_cmp <name>: ffprobe on one input of gate 5 and the comparison with the isobmff-rs tracks.txt.
# The mismatch lines go to $W/d1; $fe is 1 when ffprobe failed or printed errors.
mp4_cmp() {
  ft=$W/mff/$1
  sh tests/parity/ffprobe-tracks.sh "$W/mp4in/$1" "$ft" > "$ft.txt" 2> /dev/null
  fe=0
  rc=$(cat "$ft.rc" 2> /dev/null)
  [ "$rc" = 0 ] || fe=1
  [ ! -s "$ft.err" ] || fe=1
  perr=0
  ! grep -qE '^error (malformed|unsupported)$' "$W/m1/$1/tracks.txt" 2> /dev/null || perr=1
  gawk -M -v fferr=$fe -v perr=$perr -v ffrc="${rc:-1}" -f tests/parity/mp4cmp.awk "$W/m1/$1/tracks.txt" "$ft.txt" > $W/d1 2>&1
}

# mp4_corpus: gate 5a and the count of gate 5d. Every corpus file whose isobmff-rs tracks.txt has an
# `error malformed` or `error unsupported` line (a parse failure: ffprobe must read no packets from
# it either, else it needs a known-differences line), or a
# sample or an error line of a gated track, goes through ffprobe; known-differences.tsv lines of
# side mp4 work as in gate 1. $gated counts the files with at least one sample line.
mp4_corpus() {
  : > $W/d-mp4; : > $W/d-mp4cover
  gated=0
  while IFS= read -r n; do
    grep -qE '^((s|e) |error (malformed|unsupported)$)' "$W/m1/$n/tracks.txt" 2> /dev/null || continue
    echo "$n${tab}mp4" >> $W/seen
    ! grep -q '^s ' "$W/m1/$n/tracks.txt" || gated=$((gated + 1))
    mp4_cmp "$n"
    reason=$(awk -F'\t' -v n="$n" '$1 == n && $2 == "mp4" { print $3; exit }' $W/known)
    if [ ! -s $W/d1 ]; then
      [ -z "$reason" ] || echo "stale known difference: $n (mp4) is now identical" >> $W/d-stale
    elif [ -n "$reason" ]; then
      { echo "=== $n (mp4), known: $reason"; head -n 40 $W/d1; } >> $W/d-known
    else
      { echo "=== $n (mp4)"; head -n 100 $W/d1; } >> $W/d-mp4
    fi
  done < $W/mp4corpus.names
  [ "$gated" -ge "$MIN_MP4" ] || echo "mp4 corpus has $gated files with a gated track, want at least $MIN_MP4" > $W/d-mp4cover
}

# mp4_mut: gate 5b. A mutation is compared when isobmff-rs reads its samples to the end (no `e`
# line) and ffprobe exited 0 with nothing on stderr; the others are only counted.
mp4_mut() {
  : > $W/d-mp4-mut
  cmpd=0 skip=0
  while IFS= read -r n; do
    grep -q '^s ' "$W/m1/$n/tracks.txt" 2> /dev/null || continue
    if grep -q '^e ' "$W/m1/$n/tracks.txt"; then skip=$((skip + 1)); continue; fi
    mp4_cmp "$n"
    if [ "$fe" -ne 0 ]; then skip=$((skip + 1)); continue; fi
    cmpd=$((cmpd + 1))
    # a mutation of a file listed in known-differences.tsv for side mp4 is not diffed
    awk -F'\t' -v n="${n%.*}" '$1 == n && $2 == "mp4" { f = 1 } END { exit !f }' $W/known && continue
    [ ! -s $W/d1 ] || { echo "=== $n (mp4)"; head -n 100 $W/d1; } >> $W/d-mp4-mut
  done < $W/mp4mut.names
  echo "| mp4 mutations with samples: compared / not compared (isobmff-rs error or ffprobe error) | $cmpd / $skip |" >> $W/split.md
}

full() {
  REP=$1 bad=0
  rm -rf $W; mkdir -p "$REP" $W/corpus $W/mp4c $W/small $W/msmall $W/mut $W/mmut $W/all $W/mp4in $W/mff
  # --- the inputs
  corpus $W/corpus
  [ -d tests/data/fuzz-grown ] && find tests/data/fuzz-grown -type f ! -name '.*' | flat $W/corpus tests/data
  # The MP4 sources (named mp4-* in corpus.toml) are for gate 5 only: libheif and libavif do not
  # read them, and the HEIF and AVIF gates stay as they were.
  for f in $W/corpus/mp4-*; do [ ! -e "$f" ] || mv "$f" $W/mp4c/; done
  ls $W/corpus > $W/corpus.names
  cn=$(wc -l < $W/corpus.names)
  # gate 4
  [ "$cn" -ge "$MIN" ] || { echo "corpus has $cn files, want at least $MIN" | tee $W/d-cover >&2; }
  find $W/corpus -type f -size -1048576c | while IFS= read -r f; do ln "$f" $W/small/; done
  run isobmff-oracle gen $W/small $W/mut
  ls $W/mut > $W/mut.names
  cp -al $W/corpus/. $W/all/ && cp -al $W/mut/. $W/all/ || bad=1
  # gate 5 inputs: the whole corpus, the MP4 sources and the mutations of both
  find $W/mp4c -type f -size -1048576c | while IFS= read -r f; do ln "$f" $W/msmall/; done
  run isobmff-oracle gen $W/msmall $W/mmut
  cp -al $W/corpus/. $W/mp4in/ && cp -al $W/mp4c/. $W/mp4in/ && cp -al $W/mut/. $W/mp4in/ && cp -al $W/mmut/. $W/mp4in/ || bad=1
  { ls $W/corpus; ls $W/mp4c; } | sort > $W/mp4corpus.names
  { ls $W/mut; ls $W/mmut; } > $W/mp4mut.names

  # --- the oracles, on corpus + mutations
  mkdir -p $W/ch $W/ca $W/rh $W/ra $W/rh1 $W/rhr
  robust heif-oracle $W/all $W/ch graph.txt
  robust avif-oracle $W/all $W/ca avif.txt
  run isobmff-oracle heif $W/all $W/rh 2> $W/rh.err
  run isobmff-oracle avif $W/all $W/ra 2> $W/ra.err
  run isobmff-oracle heif --threads 1 $W/all $W/rh1
  run isobmff-oracle heif --read $W/all $W/rhr
  run isobmff-oracle stats $W/corpus $W/stats.tsv
  mkdir -p $W/m1 $W/m2
  run isobmff-oracle mp4 $W/mp4in $W/m1 2> $W/m1.err
  run isobmff-oracle mp4 --read $W/mp4in $W/m2

  # --- gate 1: corpus, with known differences
  awk -F'\t' 'NR > 1 && !/^#/ && NF >= 2' $KNOWN > $W/known
  : > $W/seen; : > $W/d-stale; : > $W/d-known
  { ls $W/ca; ls $W/ra; } | sort -u | comm -12 - $W/corpus.names > $W/avif.names
  corpus_side heif $W/ch $W/rh $W/corpus.names
  corpus_side avif $W/ca $W/ra $W/avif.names
  mp4_corpus
  while IFS=$tab read -r kn ks kr; do
    [ -n "$kn" ] || continue
    grep -qxF "$kn$tab$ks" $W/seen || echo "stale known difference: $kn ($ks) was not compared" >> $W/d-stale
  done < $W/known

  # --- gate 2: mutations
  : > $W/split.md
  mut_side heif graph.txt $W/ch $W/rh $W/mut.names libheif
  mut_side avif avif.txt $W/ca $W/ra $W/mut.names libavif
  mp4_mut

  # --- gate 3: threads and read, on the corpus and the mutations
  diff -rq $W/rh $W/rh1 > $W/d-threads
  diff -rq $W/rh $W/rhr > $W/d-read
  diff -rq $W/m1 $W/m2 > $W/d-mp4read

  {
    echo "## isobmff-rs parity"; echo
    echo "| input set | count |"; echo "|---|---|"
    echo "| corpus files (corpus.toml and fuzz-grown) | $cn |"
    echo "| corpus files read by the AVIF pair | $(wc -l < $W/avif.names) |"
    echo "| mutations (files of the corpus under 1 MiB) | $(wc -l < $W/mut.names) |"
    echo "| corpus, libheif ok / error / missing | $(okcount $W/ch graph.txt $W/corpus.names) |"
    echo "| corpus, isobmff-rs heif ok / error / missing | $(okcount $W/rh graph.txt $W/corpus.names) |"
    echo "| corpus, libavif ok / error / missing | $(okcount $W/ca avif.txt $W/avif.names) |"
    echo "| corpus, isobmff-rs avif ok / error / missing | $(okcount $W/ra avif.txt $W/avif.names) |"
    echo "| mp4 inputs: corpus files (all sources) / mutations | $(wc -l < $W/mp4corpus.names) / $(wc -l < $W/mp4mut.names) |"
    echo "| mp4 corpus files with a gated video track (least $MIN_MP4) | $gated |"
    echo "| ffprobe | $(ffprobe -version 2>&1 | head -n 1) |"
    cat $W/split.md
    echo; echo "| gate | result |"; echo "|---|---|"
  } > "$REP/summary.md"
  gate "1 corpus: libheif vs isobmff-rs heif" $W/d-heif
  gate "1 corpus: libavif vs isobmff-rs avif" $W/d-avif
  gate "1 corpus: no stale known difference" $W/d-stale
  gate "2 mutations: libheif vs isobmff-rs heif where both ok" $W/d-mut-heif
  gate "2 mutations: libavif vs isobmff-rs avif where both ok" $W/d-mut-avif
  gate "3 isobmff-rs heif: --threads 1 vs default" $W/d-threads
  gate "3 isobmff-rs heif: --read vs slice" $W/d-read
  gate "4 coverage: corpus files >= $MIN" $W/d-cover
  gate "5a corpus: ffprobe vs isobmff-rs mp4" $W/d-mp4
  gate "5b mutations: ffprobe vs isobmff-rs mp4 where read to the end" $W/d-mp4-mut
  gate "5c isobmff-rs mp4: --read vs slice" $W/d-mp4read
  gate "5d coverage: corpus files with a gated video track >= $MIN_MP4" $W/d-mp4cover
  [ -s $W/d-known ] && { echo; echo "known differences (not counted, docs/parity.md):"; echo; sed 's/^/    /' $W/d-known; } >> "$REP/summary.md"
  stats_table >> "$REP/summary.md"
  { printf 'name\tpair\toracle\tisobmff-rs\n'; errors heif $W/ch.err $W/rh.err; errors avif $W/ca.err $W/ra.err; } > "$REP/errors.tsv"

  # the diffs: one file each in the report, and the first 200 lines in summary.md
  for d in d-heif d-avif d-stale d-mut-heif d-mut-avif d-threads d-read d-known d-mp4 d-mp4-mut d-mp4read d-mp4cover; do
    [ -s $W/$d ] && { echo "=== $d"; cat $W/$d; } >> $W/all.diff
  done
  if [ -s $W/all.diff ]; then
    { echo; echo "first 200 diff lines:"; echo; head -n 200 $W/all.diff | sed 's/^/    /'; } >> "$REP/summary.md"
    for d in d-heif d-avif d-stale d-mut-heif d-mut-avif d-threads d-read d-known d-mp4 d-mp4-mut d-mp4read d-mp4cover; do
      [ -s $W/$d ] && head -n 5000 $W/$d > "$REP/${d#d-}.diff"
    done
  fi
  rm -rf $W
  return $bad
}

# The corpus maxima of docs/limits.md, from `oracle stats` (columns: name, boxes, max depth, items,
# largest reference count of one iref entry, largest grid in tiles, largest extent count of one
# item, properties): the largest value of each column and the file that has it.
stats_table() {
  [ -s $W/stats.tsv ] || { echo; echo "no corpus statistics (oracle stats failed)"; return 0; }
  echo; echo "corpus maxima (docs/limits.md):"; echo
  echo "| quantity | max | file |"; echo "|---|---|---|"
  awk -F'\t' '
    BEGIN { split("boxes|max box depth|items|references in one iref entry|tiles in one grid|extents of one item|properties", l, "|") }
    { for (c = 2; c <= 8; c++) if (!(c in m) || $c + 0 > m[c]) { m[c] = $c + 0; f[c] = $1 } }
    END { for (c = 2; c <= 8; c++) if (c in m) printf "| %s | %d | %s |\n", l[c - 1], m[c], f[c] }' $W/stats.tsv
}

# The test data of libheif and libavif: the subset that the aarch64 run is compared on.
subset() {
  mkdir -p "$1"
  while IFS=$tab read -r name key; do
    case $name in libheif | libavif) ;; *) continue ;; esac
    if [ -d "$D/v/$key/$name/tests/data" ]; then find "$D/v/$key/$name/tests/data" -type f | flat "$1" "$D/v/$key"; fi
  done < "$LIST"
  [ -n "$(ls "$1")" ] || { echo "no libheif/libavif test data in $LIST" >&2; return 1; }
}

ref() {
  rm -rf $W; subset $W/in || return 1
  mkdir -p "$1/heif" "$1/avif"
  run isobmff-oracle heif --threads 1 $W/in "$1/heif"
  run isobmff-oracle avif $W/in "$1/avif"
  rm -rf $W
  return $bad
}

arm() {
  REP=$1 bad=0
  rm -rf $W; subset $W/in || return 1
  mkdir -p $W/heif $W/avif
  run ${ARM_RUN:-qemu-aarch64 -L /usr/aarch64-linux-gnu} /usr/local/bin/isobmff-oracle-arm64 heif --threads 1 $W/in $W/heif
  run ${ARM_RUN:-qemu-aarch64 -L /usr/aarch64-linux-gnu} /usr/local/bin/isobmff-oracle-arm64 avif $W/in $W/avif
  for k in heif avif; do
    if diff -rq "$REP/$k" $W/$k > $W/d; then echo "aarch64 $k: identical to x86_64 ($(ls $W/$k | wc -l) inputs)"
    else echo "aarch64 $k: DIFF"; head -n 50 $W/d; bad=1; fi
  done
  rm -rf $W
  return $bad
}

cmd=${1:-}; [ $# -gt 0 ] && shift
case $cmd in
  gather) [ $# -eq 1 ] || { echo "usage: run.sh gather <dst>" >&2; exit 2; }; gather "$@" ;;
  full) [ $# -eq 1 ] || { echo "usage: run.sh full <report-dir>" >&2; exit 2; }; full "$@" ;;
  ref) bad=0; ref "$@" ;;
  arm) arm "$@" ;;
  *) echo "usage: run.sh gather|full|ref|arm ..." >&2; exit 2 ;;
esac
