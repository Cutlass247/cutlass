#!/usr/bin/env bash
# Build the update manifest the app checks on launch.
#
# Tauri's updater fetches `latest.json` from the endpoint in tauri.conf.json,
# verifies the installer against the signature inside it, and refuses anything
# it can't verify. So the manifest and the .sig are not optional extras of a
# release — without them, a published version is invisible to everyone who
# already has Cutlass installed.
#
#   scripts/release-manifest.sh <version> "<release notes>"
#
# Expects the trial installer to have been built with signing turned on:
#   TAURI_SIGNING_PRIVATE_KEY="$(cat ~/.cutlass-keys/updater.key)" \
#   TAURI_SIGNING_PRIVATE_KEY_PASSWORD= \
#   npm run tauri build
set -euo pipefail

VERSION="${1:?usage: release-manifest.sh <version> [notes]  |  --verify <version>}"
NOTES="${2:-}"
ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"

# --verify: check the published release the way a user's copy of Cutlass will.
# Worth its own mode, because every failure here is silent from the inside —
# a wrong asset name or a missing manifest looks exactly like "no update
# available", and nobody finds out until the next release also fails to land.
if [ "$VERSION" = "--verify" ]; then
  V="${2:?usage: release-manifest.sh --verify <version>}"
  BASE="https://github.com/Cutlass247/cutlass/releases"
  MANIFEST="$BASE/latest/download/latest.json"
  fail=0

  echo "manifest: $MANIFEST"
  json="$(curl -sfL "$MANIFEST" || true)"
  if [ -z "$json" ]; then
    echo "  FAIL — not published. Installed copies will never see this release." >&2
    exit 1
  fi

  got="$(printf '%s' "$json" | python -c 'import json,sys; print(json.load(sys.stdin)["version"])')"
  if [ "$got" = "$V" ]; then
    echo "  ok — advertises $got"
  else
    echo "  FAIL — advertises $got, expected $V" >&2
    fail=1
  fi

  url="$(printf '%s' "$json" | python -c 'import json,sys; print(json.load(sys.stdin)["platforms"]["windows-x86_64"]["url"])')"
  echo "installer: $url"
  code="$(curl -s -o /dev/null -w '%{http_code}' -L "$url")"
  if [ "$code" = "200" ]; then
    echo "  ok — downloadable"
  else
    echo "  FAIL — HTTP $code. The manifest names an asset that isn't there," >&2
    echo "         usually because the uploaded file was renamed." >&2
    fail=1
  fi

  sig="$(printf '%s' "$json" | python -c 'import json,sys; print(len(json.load(sys.stdin)["platforms"]["windows-x86_64"]["signature"]))')"
  [ "$sig" -gt 100 ] && echo "signature: ok — $sig chars" || { echo "signature: FAIL — empty or truncated" >&2; fail=1; }

  [ "$fail" = 0 ] && echo "" && echo "This release will reach installed copies of Cutlass." || exit 1
  exit 0
fi
NSIS="$ROOT/target/release/bundle/nsis"
SETUP="$NSIS/Cutlass_${VERSION}_x64-setup.exe"
SIG="$SETUP.sig"

[ -f "$SETUP" ] || { echo "no installer at $SETUP" >&2; exit 1; }
if [ ! -f "$SIG" ]; then
  echo "no signature at $SIG" >&2
  echo "The build ran without TAURI_SIGNING_PRIVATE_KEY set, so this installer" >&2
  echo "cannot be offered as an update. Rebuild with the key (see the header)." >&2
  exit 1
fi

# The asset name the updater downloads must match what gets uploaded to the
# release, exactly, or every client 404s. So it comes from this one place and
# is the name the build already produced — nothing gets renamed on the way to
# the release.
#
# 0.1.1 was published as `Cutlass-0.1.1-trial-x64-setup.exe`, a hand-rename of
# the built file. That convention is deliberately dropped here: a manual rename
# between build and upload is a step that breaks updates silently when it is
# forgotten or spelled differently, and it is not worth the word "trial" in a
# filename. The release page and README say what the download is.
ASSET="Cutlass_${VERSION}_x64-setup.exe"
URL="https://github.com/Cutlass247/cutlass/releases/download/v${VERSION}/${ASSET}"

OUT="$NSIS/latest.json"
python - "$VERSION" "$NOTES" "$URL" "$SIG" "$OUT" <<'PY'
import json, sys, datetime
version, notes, url, sig_path, out = sys.argv[1:6]
with open(sig_path, encoding="utf-8") as f:
    signature = f.read().strip()
manifest = {
    "version": version,
    "notes": notes,
    "pub_date": datetime.datetime.now(datetime.timezone.utc)
        .replace(microsecond=0).isoformat().replace("+00:00", "Z"),
    "platforms": {"windows-x86_64": {"signature": signature, "url": url}},
}
with open(out, "w", encoding="utf-8") as f:
    json.dump(manifest, f, indent=2)
    f.write("\n")
print(f"wrote {out}")
PY

# Paths relative to the repo root: `gh` on Windows does not understand the
# /d/... form a bash $PWD produces here.
REL_SETUP="target/release/bundle/nsis/${ASSET}"
REL_OUT="target/release/bundle/nsis/latest.json"

cat <<EOF

Next, attach BOTH to the v${VERSION} release — the installer alone is not
enough, since the updater only ever reads latest.json. Run from the repo root:

  gh release upload v${VERSION} \\
    "$REL_SETUP" \\
    "$REL_OUT" --clobber

Then verify it from outside, which is the only check that counts:

  scripts/release-manifest.sh --verify ${VERSION}
EOF
