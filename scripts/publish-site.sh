#!/usr/bin/env bash
set -euo pipefail

# Publish site/index.html to https://cutlass247.github.io/
#
# That address is NOT served by this repository. It is a GitHub *user site*,
# served from a second repo — Cutlass247/Cutlass247.github.io — which for a
# long time held a hand-edited copy of this page. The two drifted: the copy at
# the root kept a "See the AI in action" button months after it was replaced by
# "Buy now", and went on advertising collaboration after the feature was pulled.
# Nothing detected that, because nothing connected them.
#
# site/index.html in this repo is now the only source. This script copies it
# there, and --verify checks that the live page still matches. The GitHub
# Actions workflow runs --verify on every push that touches site/, so a change
# published here and forgotten there shows up as a failed check rather than as
# a stale page nobody looks at.
#
# Usage:
#   scripts/publish-site.sh            publish site/index.html to the root site
#   scripts/publish-site.sh --verify   check the live page matches, change nothing

REPO="Cutlass247/Cutlass247.github.io"
URL="https://cutlass247.github.io/"
ROOT="$(git rev-parse --show-toplevel)"
SRC="$ROOT/site/index.html"

[ -f "$SRC" ] || { echo "missing $SRC" >&2; exit 1; }

tmp="$(mktemp -d)"
trap 'rm -rf "$tmp"' EXIT

# Compare pages while ignoring the deploy stamp, which is expected to differ:
# it records when the page was published, so it is never equal to the source.
strip_stamp() {
  sed -E 's/updated [0-9]{4}-[0-9]{2}-[0-9]{2} [0-9]{2}:[0-9]{2} UTC · [0-9a-f]+(-dirty)?/__BUILD_STAMP__/' \
    | tr -d '\r'
}

fetch_live() {
  # Cache-buster and no-cache: GitHub Pages serves max-age=600, and a check
  # that can pass against a ten-minute-old copy is not a check.
  curl -fsS -H 'Cache-Control: no-cache' "${URL}?cb=$(date +%s%N)"
}

if [ "${1:-}" = "--verify" ]; then
  fetch_live | strip_stamp > "$tmp/live.html"
  strip_stamp < "$SRC" > "$tmp/src.html"
  if diff -q "$tmp/live.html" "$tmp/src.html" >/dev/null; then
    echo "OK  $URL matches site/index.html"
    fetch_live | grep -o 'updated [^<]*' | head -1 | sed 's/^/    /'
    exit 0
  fi
  echo "STALE  $URL does not match site/index.html" >&2
  echo "" >&2
  diff "$tmp/live.html" "$tmp/src.html" | head -40 >&2
  echo "" >&2
  echo "Run scripts/publish-site.sh to push the current page." >&2
  exit 1
fi

# Stamp the same way .github/workflows/pages.yml does, so both deployments
# describe themselves in one format. -dirty when site/ has uncommitted edits:
# the stamp names a commit, and it should not name one it did not come from.
rev="$(git -C "$ROOT" rev-parse --short HEAD)"
git -C "$ROOT" diff --quiet -- site/index.html || rev="${rev}-dirty"
stamp="updated $(date -u '+%Y-%m-%d %H:%M UTC') · $rev"
sed "s|__BUILD_STAMP__|$stamp|g" "$SRC" > "$tmp/index.html"

# Skip a no-op push. Publishing an identical page would still make a commit,
# and a history of empty commits hides the ones that changed something.
if fetch_live | strip_stamp > "$tmp/live.html" 2>/dev/null; then
  if diff -q "$tmp/live.html" <(strip_stamp < "$SRC") >/dev/null; then
    echo "unchanged — $URL already serves this page"
    exit 0
  fi
fi

sha="$(gh api "repos/$REPO/contents/index.html" --jq .sha)"
base64 -w0 "$tmp/index.html" | tr -d '\n' > "$tmp/content.b64"

# --input-style field-from-file, not an argument: the page is ~23 KB, which
# base64-encodes to more than Windows allows on a command line.
commit="$(gh api --method PUT "repos/$REPO/contents/index.html" \
  -F message="Publish landing page from cutlass@$rev" \
  -F content=@"$tmp/content.b64" \
  -F sha="$sha" \
  --jq '.commit.sha')"

echo "pushed ${commit:0:7} to $REPO"
echo "$stamp"
echo ""
echo "Pages takes a minute or so. Then:  scripts/publish-site.sh --verify"
