#!/bin/bash
# Compile the Icon Composer source for iPhone and iPad, including the
# deployment-target-compatible renditions produced by actool.
set -euo pipefail
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
APP="$1"
OUT="$2"
ICON_DIR="$3"
ICON_SOURCE="$ROOT/ios/NCLRG.icon"
PARTIAL="$OUT/NCLRG-partial.plist"

xcodebuild -version | grep -E '^Xcode 27(\.|$)'
test -f "$ICON_SOURCE/icon.json"
test -d "$ICON_SOURCE/Assets"

# Keep the independent in-app brand image; do not copy the old app-icon
# PNGs over the new renditions. The legacy source assets remain in git.
if [ -f "$ICON_DIR/BrandIcon.png" ]; then
    cp "$ICON_DIR/BrandIcon.png" "$APP/"
fi

xcrun --sdk iphoneos actool "$ICON_SOURCE" \
    --compile "$APP" \
    --platform iphoneos \
    --minimum-deployment-target 15.0 \
    --target-device iphone \
    --target-device ipad \
    --app-icon NCLRG \
    --output-partial-info-plist "$PARTIAL" \
    --output-format human-readable-text --warnings --errors

test -s "$APP/Assets.car"
test -s "$PARTIAL"

# Replace stale PNG icon declarations rather than merging arrays or
# dictionaries with conflicting names. Keep all non-icon app metadata.
python3 - "$APP/Info.plist" "$PARTIAL" <<'PY'
import plistlib
import sys
from pathlib import Path

info_path, partial_path = map(Path, sys.argv[1:])
info = plistlib.loads(info_path.read_bytes())
partial = plistlib.loads(partial_path.read_bytes())
primary = partial.get("CFBundleIcons", {}).get("CFBundlePrimaryIcon", {})
if primary.get("CFBundleIconName", partial.get("CFBundleIconName")) != "NCLRG":
    raise SystemExit(f"actool did not register NCLRG as the primary icon: {partial}")
for key in ("CFBundleIconFile", "CFBundleIconFiles", "CFBundleIconName",
            "CFBundleIcons", "CFBundleIcons~ipad"):
    info.pop(key, None)
info.update(partial)
info_path.write_bytes(plistlib.dumps(info, sort_keys=False))
PY
plutil -lint "$APP/Info.plist"
echo "Compiled native NCLRG icon: $APP/Assets.car"
