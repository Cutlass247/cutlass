#!/usr/bin/env bash
# Run the export harnesses that need a real ffmpeg and real footage.
#
#   scripts/run-harnesses.sh <a-video-file>
#
# `cargo test` does not run examples — it only compiles them. So every harness
# under crates/*/examples was built by the pre-push hook and never executed,
# which means each one was checking nothing at all unless somebody remembered
# to run it by hand. The gap-deadlock regression lived there for months; it is
# now a real test in export.rs. These are the ones left that genuinely need
# footage and an encoder, so they cannot be.
#
# Run this before cutting a release. It takes a few minutes.
set -uo pipefail

CLIP="${1:-}"
if [ -z "$CLIP" ] || [ ! -f "$CLIP" ]; then
  echo "usage: scripts/run-harnesses.sh <a-video-file>" >&2
  echo "Any short clip with audio will do — these render it, they don't inspect it." >&2
  exit 2
fi

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
CLIP="$(cd "$(dirname "$CLIP")" && pwd)/$(basename "$CLIP")"
OUT="${TMPDIR:-/tmp}/cutlass-harness-$$"
mkdir -p "$OUT"

# The shipped LGPL ffmpeg, not the GPL build ffmpeg-sidecar downloads. Using
# the wrong one hides LGPL-only failures — a filter that works here and is
# missing for every user.
export FFMPEG_BINARY="$ROOT/vendor/ffmpeg/bin/ffmpeg.exe"
[ -f "$FFMPEG_BINARY" ] || { echo "no vendored ffmpeg at $FFMPEG_BINARY" >&2; exit 1; }

pass=0; fail=0
run() {
  local name="$1"; shift
  printf '\n=== %s ===\n' "$name"
  if cargo run -q -p cutlass-core --example "$name" -- "$@" 2>&1 | tee "$OUT/$name.log"; then
    # These print PASS/FAIL per case rather than setting an exit code, so the
    # output is what has to be read.
    if grep -q "FAIL" "$OUT/$name.log"; then
      echo "  -> FAILURES in $name"; fail=$((fail+1))
    else
      echo "  -> ok"; pass=$((pass+1))
    fi
  else
    echo "  -> $name did not finish"; fail=$((fail+1))
  fi
}

run escaping_check  "$CLIP" "$OUT/escaping"
run limits_check    "$CLIP" "$OUT/limits"
run failure_modes   "$CLIP" "$OUT/failures"
run cancel_staged   "$CLIP" "$OUT/cancel"

printf '\n%s\n' "----------------------------------------"
echo "$pass ok, $fail with failures"
echo "logs: $OUT"
[ "$fail" -eq 0 ]
