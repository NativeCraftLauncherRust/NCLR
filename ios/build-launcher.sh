#!/bin/bash
# Build the standalone legacy NCLR L1 launcher IPA. It links no game engine and no
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
OUT="${BUILD_DIR:-$ROOT/build-launcher}"
APP="$OUT/$APP_NAME.app"
DIST="${DIST_DIR:-$ROOT/dist-launcher}"

source "$ROOT/scripts/ios-swift-support.sh"
nclr_configure_ios_toolchain
test -f "$MAIN_SOURCE"
if nclr_uses_swift_sidebar "$MAIN_SOURCE"; then
    bash "$ROOT/scripts/check-launcher-source.sh" "$MAIN_SOURCE"
fi

rm -rf "$OUT" "$DIST"
mkdir -p "$APP" "$DIST"

nclr_compile_ios_sources "$MAIN_SOURCE" "$OUT/Objects"

"${IOS_LINKER[@]}" \
    -Xlinker -rpath -Xlinker @executable_path/Frameworks \
    -framework UIKit \
    -framework Foundation \
    -framework CoreFoundation \
    -framework CoreGraphics \
    -framework QuartzCore \
    -framework Security \
    "${IOS_OBJECTS[@]}" \
    -o "$APP/$APP_NAME"

cp "$ROOT/ios/Info-launcher.plist" "$APP/Info.plist"
/usr/libexec/PlistBuddy -c "Set :CFBundleVersion $BUILD_NUMBER" "$APP/Info.plist"
/usr/libexec/PlistBuddy -c "Set :CFBundleIdentifier $BUNDLE_ID" "$APP/Info.plist"
/usr/libexec/PlistBuddy -c "Set :CFBundleExecutable $APP_NAME" "$APP/Info.plist"
/usr/libexec/PlistBuddy -c "Set :CFBundleName $APP_NAME" "$APP/Info.plist"
/usr/libexec/PlistBuddy -c "Set :CFBundleDisplayName $DISPLAY_NAME" "$APP/Info.plist"
/usr/libexec/PlistBuddy -c "Set :MinimumOSVersion $IPHONEOS_DEPLOYMENT_TARGET" "$APP/Info.plist"
/usr/libexec/PlistBuddy -c "Set :CFBundleSupportedPlatforms:0 $BUNDLE_PLATFORM" "$APP/Info.plist"

bash "$ROOT/ios/compile-app-icon.sh" "$APP" "$OUT" "$ROOT/ios/icons"
mkdir -p "$APP/versions" "$APP/mods"

if nclr_uses_swift_sidebar "$MAIN_SOURCE"; then
    nclr_embed_swift_runtime "$APP" "$APP_NAME"
fi
codesign --force --sign - --timestamp=none "$APP/$APP_NAME"
codesign --force --sign - --timestamp=none "$APP"

mkdir -p "$OUT/Payload"
cp -R "$APP" "$OUT/Payload/"
(cd "$OUT" && zip -qry "$DIST/$IPA_NAME" Payload)

echo "--- artifact ---"
echo "entrypoint: $MAIN_SOURCE"
ls -lh "$DIST/$IPA_NAME"
shasum -a 256 "$DIST/$IPA_NAME"
echo "--- app ---"
file "$APP/$APP_NAME"
plutil -p "$APP/Info.plist" | head -50
unzip -l "$DIST/$IPA_NAME" | head -30
