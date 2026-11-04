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
run denoise_check   "$CLIP" "$OUT/denoise"
# Dissolve, dip, overlay, titles, retime and fades in one render. Left out of
# this list, it was the only thing that ran a dissolve through real ffmpeg —
# so when ffmpeg 7.1 broke every dissolve, nothing here noticed, and 0.2.0
# shipped unable to export one. Reads up to 6s into the clip.
run export_check    "$CLIP" "$CLIP"
# drawtext never wraps: a caption wider than a vertical frame was cut off at
# both edges. Renders every clip shape and finds the caption by frame diff.
run title_fit_check "$CLIP" "$OUT/titles"

# Every file type the Import dialog offers, made by the shipped ffmpeg, then
# imported, previewed, played and exported the way the app does it. A test in
# the app rather than an example here, because it drives the app's own import;
# ignored by `cargo test`, as it needs this ffmpeg and takes a couple of
# minutes. The Import dialog once offered six video types while drops took
# seventeen, MP3 among them, and the MP3s then failed to import at all.
printf '\n=== file types ===\n'
if cargo test -q -p cutlass-desktop --all-features -- --ignored every_offered_file_type_imports 2>&1 \
    | tee "$OUT/file_types.log" | grep -q "test result: ok"; then
  echo "  -> ok"; pass=$((pass+1))
else
  grep -A60 "don't work" "$OUT/file_types.log" | head -40
  echo "  -> FAILURES in file types"; fail=$((fail+1))
fi

printf '\n%s\n' "----------------------------------------"
echo "$pass ok, $fail with failures"
echo "logs: $OUT"
# release-manifest.sh refuses to write a release's update file unless this is
# newer than the installer — so the harnesses can't be skipped by forgetting.
PASSED="$ROOT/target/harnesses-passed"
if [ "$fail" -eq 0 ]; then
  echo "$(date -u +%Y-%m-%dT%H:%MZ) at $(git -C "$ROOT" rev-parse --short HEAD) on $(basename "$CLIP")" > "$PASSED"
else
  rm -f "$PASSED"
fi
[ "$fail" -eq 0 ]
