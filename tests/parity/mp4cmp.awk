# mp4cmp.awk: gawk -M -v fferr=0|1 [-v perr=1] -f mp4cmp.awk <tracks.txt of oracle mp4> <output of ffprobe-tracks.sh>
# Prints two lines (ours, ffprobe) per mismatch and nothing when the two agree (docs/parity.md,
# gate 5). Lines are keyed by (kind, track id, sample number or probe number). Only tracks that
# have an `s` or `e` line in ours are compared. Fields 4 and up must be equal, except where ours
# is `-` (a field that ffmpeg computes differently). A key on one side only is a mismatch. For a
# file where fferr is 1 (ffprobe failed or printed errors, so its
# output may be a prefix), only keys on both sides are compared and the `at` lines are skipped. Where
# ours has an `e` line and fferr is 0, ffprobe must stop at the same sample: a key of ffprobe past our
# last sample is a mismatch (only the `at` lines of that track are skipped, ours has none). perr=1:
# ours failed to parse the file (tracks.txt is `error <class>`); with fferr 0, any packet of ffprobe is
# a mismatch.
BEGIN { PROCINFO["sorted_in"] = "@ind_str_asc" }
function mismatch(k) {
  print "ours:    " ((k in o) ? o[k] : "(missing)")
  print "ffprobe: " ((k in f) ? f[k] : "(missing)")
}
FNR == NR {
  if ($1 == "s" || $1 == "at") o[$1 SUBSEP $2 SUBSEP $3] = $0
  if ($1 == "s" || $1 == "e") seen[$2] = 1
  if ($1 == "e") err[$2] = 1
  next
}
($1 == "s" || $1 == "at") && ($2 in seen) { f[$1 SUBSEP $2 SUBSEP $3] = $0 }
perr && $1 == "s" { np++ }
END {
  if (perr) {
    if (!fferr && np > 0) { print "ours:    parse error"; print "ffprobe: " np " packets" }
    exit
  }
  for (k in o) keys[k] = 1
  for (k in f) keys[k] = 1
  for (k in keys) {
    split(k, p, SUBSEP)
    lenient = fferr
    if (p[1] == "at" && (fferr || (p[2] in err))) continue
    if (!(k in o) || !(k in f)) { if (!lenient) mismatch(k); continue }
    n1 = split(o[k], a, " "); n2 = split(f[k], b, " ")
    bad = (n1 != n2)
    for (i = 4; i <= n1 && !bad; i++) if (a[i] != "-" && a[i] != b[i]) bad = 1
    if (bad) mismatch(k)
  }
}
