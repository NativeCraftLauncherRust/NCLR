#!/bin/bash
# Build the standalone NCLR launcher IPA. It links no game engine and no
# Mojang-derived data: versions and mods are managed locally, and the future
# game app receives the selected version through nclr-game://.
set -euxo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
APP_NAME="${APP_NAME:-NCLR}"
DISPLAY_NAME="${DISPLAY_NAME:-NCLR}"
BUNDLE_ID="${BUNDLE_ID:-dev.zrxmaxlnx.nclr}"
BUILD_NUMBER="${BUILD_NUMBER:-$(date -u +%Y%m%d%H%M)}"
IPA_NAME="${IPA_NAME:-NCLR-Launcher-unsigned.ipa}"
MAIN_SOURCE="${MAIN_SOURCE:-$ROOT/ios/main_launcher_only.m}"
SDK="$(xcrun --sdk iphoneos --show-sdk-path)"
OUT="${BUILD_DIR:-$ROOT/build-launcher}"
APP="$OUT/$APP_NAME.app"
DIST="${DIST_DIR:-$ROOT/dist-launcher}"

rm -rf "$OUT" "$DIST"
mkdir -p "$APP" "$DIST"

xcrun --sdk iphoneos clang \
    -arch arm64 \
    -mios-version-min=15.0 \
    -isysroot "$SDK" \
    -fobjc-arc \
    -O2 \
    -framework UIKit \
    -framework Foundation \
    -framework CoreFoundation \
    -framework CoreGraphics \
    -framework QuartzCore \
    -framework Security \
    "$MAIN_SOURCE" \
    -o "$APP/$APP_NAME"

cp "$ROOT/ios/Info-launcher.plist" "$APP/Info.plist"
/usr/libexec/PlistBuddy -c "Set :CFBundleVersion $BUILD_NUMBER" "$APP/Info.plist"
/usr/libexec/PlistBuddy -c "Set :CFBundleIdentifier $BUNDLE_ID" "$APP/Info.plist"
/usr/libexec/PlistBuddy -c "Set :CFBundleExecutable $APP_NAME" "$APP/Info.plist"
/usr/libexec/PlistBuddy -c "Set :CFBundleName $APP_NAME" "$APP/Info.plist"
/usr/libexec/PlistBuddy -c "Set :CFBundleDisplayName $DISPLAY_NAME" "$APP/Info.plist"

bash "$ROOT/ios/compile-app-icon.sh" "$APP" "$OUT" "$ROOT/ios/icons"
mkdir -p "$APP/versions" "$APP/mods"

codesign --force --sign - --timestamp=none "$APP/$APP_NAME"
codesign --force --sign - --timestamp=none "$APP"

mkdir -p "$OUT/Payload"
cp -R "$APP" "$OUT/Payload/"
(cd "$OUT" && zip -qry "$DIST/$IPA_NAME" Payload)

echo "--- artifact ---"
ls -lh "$DIST/$IPA_NAME"
shasum -a 256 "$DIST/$IPA_NAME"
echo "--- app ---"
file "$APP/$APP_NAME"
plutil -p "$APP/Info.plist" | head -50
unzip -l "$DIST/$IPA_NAME" | head -30
