#!/bin/bash
# Verify a launcher IPA, its source entry, and the features built into it.
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
IPA="${1:?usage: verify-launcher-ipa.sh path/to/app.ipa [source]}"
SOURCE="${2:-$ROOT/ios/main_l1.m}"

test -f "$IPA"
test -f "$SOURCE"
unzip -tq "$IPA"

UPLOAD="$(mktemp -d "${TMPDIR:-/tmp}/nclr-ipa-check.XXXXXX")"
trap 'rm -rf "$UPLOAD"' EXIT
unzip -q "$IPA" -d "$UPLOAD"

APP="$(find "$UPLOAD/Payload" -maxdepth 1 -name '*.app' -type d -print -quit)"
test -n "$APP"
EXECUTABLE="$APP/$(/usr/libexec/PlistBuddy -c 'Print :CFBundleExecutable' "$APP/Info.plist")"
test -f "$EXECUTABLE"
file "$EXECUTABLE" | grep -q 'Mach-O 64-bit executable arm64'

grep -q 'assetQueue.maxConcurrentOperationCount = NCLRResourceDownloadConcurrency()' "$SOURCE"
grep -q 'downloadConcurrencyChanged:' "$SOURCE"

# Objective-C selectors and classes survive in the symbol table even when
# the final binary is stripped.
nm -m "$EXECUTABLE" 2>/dev/null | grep -F 'downloadConcurrencyChanged:' >/dev/null
nm -m "$EXECUTABLE" 2>/dev/null | grep -F 'NCLRResourceDownloadConcurrency' >/dev/null

SHA256="$(shasum -a 256 "$IPA" | awk '{print $1}')"
printf 'ipa: %s\n' "$IPA"
printf 'source: %s\n' "$SOURCE"
printf 'sha256: %s\n' "$SHA256"
printf 'artifact check passed\n'
