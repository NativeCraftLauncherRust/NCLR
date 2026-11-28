#!/bin/bash
# Verify a launcher IPA, its source entry, and the features built into it.
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
IPA="${1:?usage: verify-launcher-ipa.sh path/to/app.ipa [source]}"
SOURCE="${2:-$ROOT/ios/main_l1.m}"

test -f "$IPA"
test -f "$SOURCE"
bash "$ROOT/scripts/check-launcher-source.sh" "$SOURCE"
source "$ROOT/scripts/ios-swift-support.sh"
nclr_configure_ios_toolchain
unzip -tq "$IPA"

UPLOAD="$(mktemp -d "${TMPDIR:-/tmp}/nclr-ipa-check.XXXXXX")"
trap 'rm -rf "$UPLOAD"' EXIT
unzip -q "$IPA" -d "$UPLOAD"

APP="$(find "$UPLOAD/Payload" -maxdepth 1 -name '*.app' -type d -print -quit)"
test -n "$APP"
EXECUTABLE="$APP/$(/usr/libexec/PlistBuddy -c 'Print :CFBundleExecutable' "$APP/Info.plist")"
test -f "$EXECUTABLE"
xcrun lipo -verify_arch "$ARCH" "$EXECUTABLE"
xcrun otool -l "$EXECUTABLE" > "$UPLOAD/load-commands.txt"
xcrun otool -L "$EXECUTABLE" > "$UPLOAD/libraries.txt"
xcrun nm -m -U "$EXECUTABLE" > "$UPLOAD/symbols.txt"

python3 - "$APP/Info.plist" "$UPLOAD/load-commands.txt" "$SDK_NAME" <<'PY'
import plistlib
import re
import sys
from pathlib import Path

with Path(sys.argv[1]).open("rb") as handle:
    info = plistlib.load(handle)
commands = Path(sys.argv[2]).read_text()
sdk = sys.argv[3]
platform = "2" if sdk == "iphoneos" else "7"
bundle_platform = "iPhoneOS" if sdk == "iphoneos" else "iPhoneSimulator"
if not re.search(r"^\s*platform\s+" + platform + r"\s*$", commands, re.MULTILINE):
    raise SystemExit(f"executable does not target {sdk}")
if info.get("CFBundleSupportedPlatforms") != [bundle_platform]:
    raise SystemExit("bundle platform does not match the executable")
minimum = re.search(r"^\s*minos\s+(\S+)", commands, re.MULTILINE)

def version(value):
    parts = tuple(map(int, value.split(".")))
    return parts + (0,) * (3 - len(parts))

if minimum is None or version(minimum[1]) != version(info["MinimumOSVersion"]):
    raise SystemExit("bundle and executable minimum OS versions differ")
PY

# Check the feature methods and Swift bridge among defined symbols, including
# Swift classes whose private external symbols become local during linking.
grep -Fq 'downloadConcurrencyChanged:' "$UPLOAD/symbols.txt"
grep -Fq 'NCLRResourceDownloadConcurrency' "$UPLOAD/symbols.txt"

if nclr_uses_swift_sidebar "$SOURCE"; then
    grep -Eq '[[:space:]]_OBJC_CLASS_\$_NCLRSidebarController$' "$UPLOAD/symbols.txt"
    grep -Fq 's11NCLRSidebar' "$UPLOAD/symbols.txt"
    grep -Fq '/SwiftUI.framework/SwiftUI' "$UPLOAD/libraries.txt"
    grep -Fq 'path @executable_path/Frameworks ' "$UPLOAD/load-commands.txt"
    grep -Fq 'path /usr/lib/swift ' "$UPLOAD/load-commands.txt"

    # The same scanner used during packaging identifies toolchain libraries
    # required in Frameworks, while system-provided Swift libraries stay in iOS.
    xcrun --sdk "$SDK_NAME" swift-stdlib-tool --print --platform "$SDK_NAME" \
        --scan-executable "$EXECUTABLE" --scan-folder "$APP/Frameworks" \
        > "$UPLOAD/swift-libraries.txt"
    while IFS= read -r library; do
        [ -n "$library" ] || continue
        test -f "$APP/Frameworks/$(basename "$library")"
    done < "$UPLOAD/swift-libraries.txt"
    for library in "$APP/Frameworks/"*.dylib; do
        [ -f "$library" ] || continue
        xcrun lipo -verify_arch "$ARCH" "$library"
        codesign --verify --strict "$library"
    done
    printf 'SwiftUI sidebar and Swift runtime check passed\n'
fi

SHA256="$(shasum -a 256 "$IPA" | awk '{print $1}')"
printf 'ipa: %s\n' "$IPA"
printf 'source: %s\n' "$SOURCE"
printf 'sha256: %s\n' "$SHA256"
printf 'artifact check passed\n'
