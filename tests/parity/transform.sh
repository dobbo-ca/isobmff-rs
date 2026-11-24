#!/bin/sh
# Transform oracle gate of isobmff-rs (docs/parity.md, "Transforms"). CI only, called by
# .github/ci/Dockerfile (stage `transform`) from the repo root:
#   transform.sh <report-dir>     libheif (with libde265, dav1d and the unci codec) against
#                                 isobmff::apply_transforms on the corpus and on the generated files;
#                                 the report is summary.md and transform.diff; the caller writes
#                                 `status` (0 only when the gate passed)
# Binaries on PATH: heif-transform-oracle (tests/parity/c/transform_oracle.c) and isobmff-oracle
# (examples/oracle.rs). The corpus is the BuildKit cache mount of fetch.sh at /data, as in run.sh.
# Environment: TRANSFORM_MIN_IMAGES (default 200): the least number of images libheif decodes and
# isobmff-rs transforms, so a decoder that did not work fails the gate instead of shrinking it.
set -u
export LC_ALL=C
W=/work-tr
D=${DATA:-/data}
LIST=${DATA_LIST:-/data.list}
MIN=${TRANSFORM_MIN_IMAGES:-200}
KNOWN=tests/parity/transform-known-differences.tsv
tab=$(printf '\t')

# The files of every HEIF or AVIF source in $LIST into $1, flattened (/ becomes __), as run.sh does;
# the mp4-* sources are for gate 5 of run.sh only.
corpus() {
  while IFS=$tab read -r name key; do
    case $name in mp4-*) continue ;; esac
    find "$D/v/$key/$name" -type f | while IFS= read -r f; do
      n=$(printf '%s' "${f#"$D/v/$key/"}" | sed 's#/#__#g')
      cp "$f" "$1/$n"
    done
  done < "$LIST"
}

# gate <label> <diff-file>: an empty or missing diff passes.
gate() {
  lines=$(cat "$2" 2>/dev/null | wc -l); lines=$((lines + 0))
  if [ "$lines" -eq 0 ]; then
    echo "| $1 | ok |" >> "$REP/summary.md"; rm -f "$2"
  else
    echo "| $1 | FAIL, $lines diff lines |" >> "$REP/summary.md"; bad=1
  fi
}

# libheif on every input. If the batch run fails (libheif aborting on one hostile file would lose the
# rest), every input runs on its own, and a file that kills or hangs the tool gets a crash or
# timeout marker (<raw>/<name>/0.err), which is counted and not compared.
libheif() {
  timeout 7200 heif-transform-oracle "$1" "$2" "$3" 2> $W/c.err && return 0
  echo "batch run of heif-transform-oracle failed, retrying every file on its own" >&2
  rm -rf "$2" "$3"; mkdir -p "$2" "$3"; : > $W/c.err
  for f in "$1"/*; do
    n=${f##*/}
    rm -rf $W/one; mkdir -p $W/one; ln "$f" "$W/one/$n"
    timeout 300 heif-transform-oracle $W/one "$2" "$3" 2>> $W/c.err
    case $? in
      0) ;;
      124) mkdir -p "$2/$n"; echo timeout > "$2/$n/0.err" ;;
      *) mkdir -p "$2/$n"; echo crash > "$2/$n/0.err" ;;
    esac
  done
  rm -rf $W/one
  return 0
}

full() {
  REP=$1 bad=0
  rm -rf $W; mkdir -p "$REP" $W/in $W/raw $W/exp $W/rs $W/rep
  corpus $W/in
  cn=$(ls $W/in | wc -l)
  # the generated files and the known differences predicted from their geometry
  isobmff-oracle gen-transforms $W/in $W/known-gen.tsv || bad=1
  gn=$(( $(ls $W/in | wc -l) - cn ))
  { cat $KNOWN; cat $W/known-gen.tsv; } > $W/known.tsv

  libheif $W/in $W/raw $W/exp
  isobmff-oracle transform $W/in $W/raw $W/rs 2> $W/rs.err || bad=1
  isobmff-oracle transform-cmp $W/raw $W/exp $W/rs $W/known.tsv tests/parity/known-differences.tsv $W/rep || bad=1

  images=$(cat $W/rep/transform-images 2>/dev/null || echo 0)
  [ "$images" -ge "$MIN" ] || echo "only $images images compared, want at least $MIN" > $W/d-cover
  {
    echo "## isobmff-rs transform parity"; echo
    echo "| input set | count |"; echo "|---|---|"
    echo "| corpus files (corpus.toml, without mp4-*) | $cn |"
    echo "| generated files (oracle gen-transforms) | $gn |"
    echo "| known differences listed (corpus file + generated) | $(awk -F"\t" '$2 ~ /^[0-9]+$/' $W/known.tsv | wc -l) |"
    cat $W/rep/transform-counts.md 2>/dev/null
    echo; echo "| gate | result |"; echo "|---|---|"
  } > "$REP/summary.md"
  gate "6 transform: libheif vs isobmff-rs apply_transforms" $W/rep/transform.diff
  gate "6 coverage: images compared >= $MIN" $W/d-cover
  [ ! -s $W/rep/transform.diff ] || {
    { echo; echo "first 100 diff lines (file, image id, reason):"; echo; head -n 100 $W/rep/transform.diff | sed 's/^/    /'; } >> "$REP/summary.md"
    head -n 5000 $W/rep/transform.diff > "$REP/transform.diff"
  }
  head -n 5000 $W/c.err > "$REP/libheif.err" 2>/dev/null
  head -n 5000 $W/rs.err > "$REP/isobmff-rs.err" 2>/dev/null
  rm -rf $W
  return $bad
}

[ $# -eq 1 ] || { echo "usage: transform.sh <report-dir>" >&2; exit 2; }
full "$1"
