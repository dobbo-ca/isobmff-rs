#!/bin/sh
# fetch.sh <corpus.toml> <dir>: for every [[source]] of corpus.toml make a blobless, sparse git
# checkout of `repo` at `commit` in <dir>/src/<name> (the checkout is verified: rev-parse HEAD must
# be the pinned SHA), pull the included paths of Git LFS sources, and put hardlinks (a copy if
# the link fails) of the checked-out files under <dir>/x/<name>/<path in the repo>. Only the files
# selected by `include` minus `exclude` are in x/. A source whose stamp <dir>/<name>.ok matches
# (commit, lfs, include, exclude) is not fetched again, so a second run is a no-op.
# Run it inside a BuildKit cache mount, id=isobmff-rs-data, sharing=locked. Needs git, and git-lfs
# for the LFS sources. The sources are fetched one after the other with a pause and a retry with
# backoff, so a run does not hit a rate limit of GitHub.
set -eu
[ $# -eq 2 ] || { echo "usage: fetch.sh <corpus.toml> <dir>" >&2; exit 2; }
toml=$1 dir=$2
tab=$(printf '\t')
mkdir -p "$dir/src" "$dir/x"
export GIT_TERMINAL_PROMPT=0

# One "name repo commit lfs include exclude" line per [[source]], tab separated; the globs are
# space separated, an empty exclude is "-".
awk -F' = ' -v OFS='\t' '
  function str(v) { gsub(/"/, "", v); return v }
  function arr(v) { sub(/^\[/, "", v); sub(/\]$/, "", v); gsub(/"/, "", v); gsub(/, */, " ", v); sub(/^ +/, "", v); sub(/ +$/, "", v); return v }
  function emit() {
    if (n != "") { if (l == "") l = 0; if (ex == "") ex = "-"; print n, r, c, l, inc, ex }
    n = r = c = l = inc = ex = ""
  }
  /^\[\[source\]\]/ { emit(); next }
  $1 == "name" { n = str($2) } $1 == "repo" { r = str($2) } $1 == "commit" { c = str($2) }
  $1 == "lfs" { l = ($2 == "true") ? 1 : 0 }
  $1 == "include" { inc = arr($2) } $1 == "exclude" { ex = arr($2) }
  END { emit() }' "$toml" > "$dir/sources.tsv"

# retry <cmd>...: three tries, 10 s and 30 s apart.
retry() {
  "$@" && return 0
  for pause in 10 30; do
    echo "retry in ${pause}s: $*" >&2
    sleep "$pause"
    "$@" && return 0
  done
  return 1
}

fetch_commit() { git -C "$1" fetch -q --depth 1 --filter=blob:none origin "$2"; }

first=1
while IFS=$tab read -r name repo commit lfs include exclude; do
  src=$dir/src/$name x=$dir/x/$name stamp=$dir/$name.ok
  want="$commit $lfs $include | $exclude"
  if [ "$(cat "$stamp" 2>/dev/null || true)" = "$want" ] && [ -d "$x" ]; then
    echo "$name: up to date, $(find "$x" -type f | wc -l) files"
    continue
  fi
  [ "$first" = 1 ] || sleep 2
  first=0
  echo "fetch $name $commit"
  rm -rf "$src" "$x" "$stamp"
  git init -q "$src"
  git -C "$src" remote add origin "$repo"
  set -f
  { for g in $include; do echo "/$g"; done
    [ "$exclude" = "-" ] || for g in $exclude; do echo "!/$g"; done
  } | git -C "$src" sparse-checkout set --no-cone --stdin
  set +f
  retry fetch_commit "$src" "$commit" </dev/null
  if [ "$lfs" = 1 ]; then
    git -C "$src" lfs install --local --skip-smudge >/dev/null
  fi
  git -C "$src" checkout -q --detach FETCH_HEAD
  got=$(git -C "$src" rev-parse HEAD)
  [ "$got" = "$commit" ] || { echo "$name: HEAD is $got, want $commit" >&2; exit 1; }
  if [ "$lfs" = 1 ]; then
    # Only the included paths; the exclude list of an LFS source is empty.
    retry git -C "$src" lfs pull --include="$(echo "$include" | tr ' ' ',')" </dev/null
  fi
  mkdir -p "$x"
  find "$src" -path "$src/.git" -prune -o -type f -print | while IFS= read -r f; do
    rel=${f#"$src"/}
    mkdir -p "$x/$(dirname "$rel")"
    ln "$f" "$x/$rel" 2>/dev/null || cp "$f" "$x/$rel"
  done
  n=$(find "$x" -type f | wc -l)
  [ "$n" -gt 0 ] || { echo "$name: no file matched the include patterns" >&2; exit 1; }
  if [ "$lfs" = 1 ]; then
    # A file that is still an LFS pointer would be a corpus file of 130 bytes: fail instead.
    find "$x" -type f -size -300c | while IFS= read -r f; do
      if head -c 40 "$f" | grep -q '^version https://git-lfs'; then
        echo "$name: $f is still an LFS pointer" >&2
        exit 1
      fi
    done
  fi
  printf '%s' "$want" > "$stamp"
  echo "$name: $n files"
done < "$dir/sources.tsv"
echo "isobmff-rs-data: $(du -sh "$dir" | cut -f1) in $dir, $(find "$dir/x" -type f | wc -l) files"
