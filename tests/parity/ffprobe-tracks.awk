# ffprobe-tracks.awk <ids> FS=, <packets.csv>: the `s` and `at` lines of `oracle mp4` from the
# packets of ffprobe (columns: stream_index, pts, dts, size, pos, flags). Run with gawk -M, so
# that 64-bit timestamps stay exact. <ids> holds `stream_index track_id` lines.
FNR == NR { id[$1] = $2; next }
{ t = id[$1]; n = ++c[t]; if (n == 1) d0[t] = $3
  k = substr($6, 1, 1) == "K" ? "K" : "_"
  print "s", t, n, $5, $4, $3 - d0[t], $2, k; p[t, n] = $2; key[t, n] = k }
END { for (t in c) for (j = 0; j < 18; j++) {
    m = 1 + int((c[t] - 1) * int(j / 2) / 8); tt = p[t, m] - (j % 2 ? 0 : 1); b = 0; f = 0
    for (i = 1; i <= c[t]; i++) if (key[t, i] == "K") {
      if (!f) f = i
      if (p[t, i] <= tt && (!b || p[t, i] > p[t, b])) b = i }
    print "at", t, j, b ? b : (f ? f : "none") } }
