#!/bin/bash
# Build the release launcher without losing track of which source and
# workspace changes produced the IPA.
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
HORIZON_DIR="${HORIZON_DIR:-$ROOT/.local/horizon}"
TARGET_DIR="${CARGO_TARGET_DIR:-$HORIZON_DIR/target}"
PROFILE="${PROFILE:-fast}"
MAIN_SOURCE="$ROOT/ios/main_l1.m"
BUILD_NUMBER="${BUILD_NUMBER:-$(date -u +%Y%m%d%H%M)}"
IPA_NAME="${IPA_NAME:-NCLR-unsigned.ipa}"
DIST_DIR="${DIST_DIR:-$ROOT/dist}"
BUILD_DIR="${BUILD_DIR:-$ROOT/build-nclr}"

source "$ROOT/scripts/ios-swift-support.sh"
nclr_configure_ios_toolchain

mkdir -p "$DIST_DIR"

cd "$ROOT"
bash scripts/check-launcher-source.sh "$MAIN_SOURCE"

if [ ! -f "$TARGET_DIR/$RUST_TARGET/$PROFILE/libhorizon_ios.a" ]; then
    echo "missing static library: $TARGET_DIR/$RUST_TARGET/$PROFILE/libhorizon_ios.a" >&2
    echo "prepare and build horizon-ios first" >&2
    exit 1
fi

HORIZON_DIR="$HORIZON_DIR" \
CARGO_TARGET_DIR="$TARGET_DIR" \
PROFILE="$PROFILE" \
MAIN_SOURCE="$MAIN_SOURCE" \
INFO_PLIST="$ROOT/ios/Info-launcher.plist" \
RESOURCE_DIR="" \
SHIP_BUNDLE="" \
APP_NAME=NCLR \
IPA_NAME="$IPA_NAME" \
BUILD_NUMBER="$BUILD_NUMBER" \
BUILD_DIR="$BUILD_DIR" \
DIST_DIR="$DIST_DIR" \
bash ios/build.sh

IPA="$DIST_DIR/$IPA_NAME"

MANIFEST="$DIST_DIR/${IPA_NAME%.ipa}.manifest.json"
python3 - "$ROOT" "$MAIN_SOURCE" "$IPA" "$MANIFEST" "$BUILD_NUMBER" "$PROFILE" "$IOS_TARGET" <<'PY'
import hashlib
import json
import subprocess
import sys
import zipfile
from datetime import datetime, timezone
from pathlib import Path

root = Path(sys.argv[1])
source = Path(sys.argv[2])
ipa = Path(sys.argv[3])
output = Path(sys.argv[4])
build_number = sys.argv[5]
profile = sys.argv[6]
ios_target = sys.argv[7]

def git(*args):
    return subprocess.check_output(["git", "-C", str(root), *args], text=True).strip()

digest = hashlib.sha256(ipa.read_bytes()).hexdigest()
sources = [source, root / "ios/NCLRSidebar.swift", root / "ios/NCLRSidebar.h"]
with zipfile.ZipFile(ipa) as archive:
    names = [name for name in archive.namelist() if name.startswith("Payload/") and name.endswith(".app/nclr-build.json")]
    if len(names) != 1:
        raise SystemExit("expected one engine build identity in IPA")
    engine_build = json.loads(archive.read(names[0]))
manifest = {
    "wasm_runtime_check": json.loads(output.read_text())["wasm_runtime_check"],
    "engine": engine_build,
    "ipa": ipa.name,
    "sha256": digest,
    "source": str(source.relative_to(root)),
    "source_sha256": {
        str(path.relative_to(root)): hashlib.sha256(path.read_bytes()).hexdigest()
        for path in sources
    },
    "ios_target": ios_target,
    "profile": profile,
    "build_number": build_number,
    "commit": git("rev-parse", "HEAD"),
    "dirty": bool(git("status", "--porcelain")),
    "worktree": git("status", "--porcelain"),
    "built_at": datetime.now(timezone.utc).isoformat(),
    "feature_download_concurrency": True,
}
output.write_text(json.dumps(manifest, ensure_ascii=False, indent=2) + "\n")
print(f"manifest: {output}")
PY
