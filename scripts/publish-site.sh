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
#   scripts/publish-site.sh --force    publish even if the page already matches

REPO="Cutlass247/Cutlass247.github.io"
URL="https://cutlass247.github.io/"
ROOT="$(git rev-parse --show-toplevel)"
SRC="$ROOT/site/index.html"

[ -f "$SRC" ] || { echo "missing $SRC" >&2; exit 1; }

FORCE="no"
case "${1:-}" in
  --force) FORCE="yes" ;;
  --verify|"") ;;
  *) echo "unknown option: $1" >&2; echo "usage: publish-site.sh [--verify|--force]" >&2; exit 2 ;;
esac

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
  strip_stamp < "$SRC" > "$tmp/src.html"

  # Retry rather than judge on one look. A publish and the push that follows it
  # are seconds apart, but GitHub Pages takes a minute or two to serve the new
  # page — so a single check right after a push reports "stale" for a page that
  # is merely still building, and the red that produces teaches you to ignore
  # the one guard against actually forgetting to publish.
  #
  # Forgetting still fails, just two minutes later. That trade is worth it: a
  # slow true signal beats a fast one nobody believes.
  for attempt in $(seq 1 12); do
    if fetch_live > "$tmp/raw.html"; then
      strip_stamp < "$tmp/raw.html" > "$tmp/live.html"
      if diff -q "$tmp/live.html" "$tmp/src.html" >/dev/null; then
        echo "OK  $URL matches site/index.html"
        grep -o 'updated [^<]*' "$tmp/raw.html" | head -1 | sed 's/^/    /'
        exit 0
      fi
      [ "$attempt" = 1 ] && echo "waiting for Pages to serve the new page..."
    else
      [ "$attempt" = 1 ] && echo "waiting for $URL to answer..."
    fi
    [ "$attempt" = 12 ] || sleep 10
  done

  if [ ! -s "$tmp/raw.html" ]; then
    echo "UNREACHABLE  $URL did not answer after two minutes" >&2
    exit 1
  fi
  echo "STALE  $URL still does not match site/index.html after two minutes" >&2
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

# Look the remote file up once. It decides two things: whether this is a
# create or an update, and whether comparing against the live page means
# anything at all — a CDN can still be serving a page for a repo that no
# longer contains one, and trusting it then would skip the very push that
# matters.
if remote_sha="$(gh api "repos/$REPO/contents/index.html" --jq .sha 2>/dev/null)"; then
  # Skip a no-op push. Publishing an identical page would still make a
  # commit, and a history of empty commits hides the ones that changed
  # something.
  #
  # --force overrides that, because the comparison ignores the build stamp and
  # therefore cannot refresh one. That matters after a history rewrite: the
  # stamp names a commit, and the commit it named may no longer exist, leaving
  # the page correct but its own fingerprint pointing at nothing.
  if [ "$FORCE" = "no" ] && fetch_live | strip_stamp > "$tmp/live.html" 2>/dev/null; then
    if diff -q "$tmp/live.html" <(strip_stamp < "$SRC") >/dev/null; then
      echo "unchanged — $URL already serves this page"
      echo "  (--force publishes anyway, e.g. to refresh a stale build stamp)"
      exit 0
    fi
  fi
else
  remote_sha=""
  echo "no index.html in $REPO yet — creating it"
fi

base64 -w0 "$tmp/index.html" | tr -d '\n' > "$tmp/content.b64"

# Field-from-file rather than an argument: the page is ~23 KB, which
# base64-encodes to more than Windows allows on a command line.
args=(-F message="Publish landing page from cutlass@$rev"
      -F content=@"$tmp/content.b64")

# sha names the blob being replaced, and is left out when there is nothing
# to replace — creating and updating share one endpoint, and a sha for a
# file that does not exist is a 422.
if [ -n "$remote_sha" ]; then
  args+=(-F sha="$remote_sha")
fi

commit="$(gh api --method PUT "repos/$REPO/contents/index.html" \
  "${args[@]}" --jq '.commit.sha')"

echo "pushed ${commit:0:7} to $REPO"
echo "$stamp"
echo ""
echo "Pages takes a minute or so. Then:  scripts/publish-site.sh --verify"
