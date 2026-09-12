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

VERSION="${1:?usage: release-manifest.sh <version> [notes]}"
NOTES="${2:-}"
ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
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
# release, so both come from this one place.
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

Then confirm what the world actually sees, which is the only check that counts:

  curl -sL https://github.com/Cutlass247/cutlass/releases/latest/download/latest.json
EOF
