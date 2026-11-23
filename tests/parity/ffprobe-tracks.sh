#!/bin/sh
# ffprobe-tracks.sh <file> <tmp>: the packets of every stream of <file> as `s` and `at` lines in
# the format of `oracle mp4` (docs/parity.md, gate 5), on stdout. CI only (ffprobe from Debian
# bookworm, gawk). The options make ffprobe report the raw tables: no edit list, no parsers, no
# fill-in of timestamps. A stream with side data (a rotation matrix, a `pssh`) makes ffprobe print
# an extra csv row without index and id (ffprobe.c compact_print_section_header/footer end the row
# with a separator and add one empty row per side-data element): rows without both are dropped. Leaves <tmp>.ids (stream
# index, track id), <tmp>.csv (packets), <tmp>.err (what ffprobe printed on stderr) and <tmp>.rc
# (its exit status). Always exits 0: the caller
# decides from <tmp>.rc and <tmp>.err whether ffprobe's output is complete.
O="-v error -ignore_editlist 1 -ignore_chapters 1 -use_mfra_for auto -use_tfdt 1 -fflags +noparse+nofillin"
here=$(dirname "$0")
# shellcheck disable=SC2086
ffprobe $O -show_entries stream=index,id -of csv=p=0 "$1" 2> /dev/null |
  while IFS=, read -r i id; do [ -n "$i" ] && [ -n "$id" ] || continue; printf '%s %d\n' "$i" "$id"; done > "$2.ids"
# shellcheck disable=SC2086
ffprobe $O -show_entries packet=stream_index,pts,dts,size,pos,flags -of csv=p=0 "$1" > "$2.csv" 2> "$2.err"
echo $? > "$2.rc"
# An empty ids file would make the awk script read the packets as ids.
[ -s "$2.ids" ] && [ -s "$2.csv" ] && gawk -M -f "$here/ffprobe-tracks.awk" "$2.ids" FS=, "$2.csv"
exit 0
