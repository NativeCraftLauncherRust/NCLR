#!/bin/bash
# Build an unsigned development IPA for the Horizon iOS shell or engine app.
#
# Inputs:
#   HORIZON_DIR       checkout of horizon-rust/horizon with horizon-ios added
#   CARGO_TARGET_DIR  cargo target directory (default: $HORIZON_DIR/target)
#   APP_NAME          app/executable name (default: NCLR)
#   MAIN_SOURCE       Objective-C entry source (default: $ROOT/ios/main.m)
#   BUNDLE_ID         CFBundleIdentifier (default: dev.zrxmaxlnx.nclr)
#   DISPLAY_NAME      Home-screen name (default: NCLR)
#   IPA_NAME          output IPA name (default: ${APP_NAME}-dev-unsigned.ipa)
#   BUILD_DIR         app build directory (default: $ROOT/build)
#   DIST_DIR          IPA output directory (default: $ROOT/dist)
#   BUILD_NUMBER      CFBundleVersion (default: UTC timestamp)
#   SHIP_BUNDLE       ship.hzr3 to place in the app for first-run assembly
#                     (default: $ROOT/ship.hzr3 when present)
#   WASM_STD          horizon-wasm-std.wasm placed beside the executable
#                     (optional; wasm mods refuse to load without it)
set -euxo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
HORIZON_DIR="${HORIZON_DIR:-/tmp/horizon}"
TARGET_DIR="${CARGO_TARGET_DIR:-$HORIZON_DIR/target}"
BUILD_NUMBER="${BUILD_NUMBER:-$(date -u +%Y%m%d%H%M)}"
PROFILE="${PROFILE:-release}"
APP_NAME="${APP_NAME:-NCLR}"
MAIN_SOURCE="${MAIN_SOURCE:-$ROOT/ios/main.m}"
BUNDLE_ID="${BUNDLE_ID:-dev.zrxmaxlnx.nclr}"
DISPLAY_NAME="${DISPLAY_NAME:-NCLR}"
IPA_NAME="${IPA_NAME:-${APP_NAME}-dev-unsigned.ipa}"
SHIP_BUNDLE="${SHIP_BUNDLE-$ROOT/ship.hzr3}"
INFO_PLIST="${INFO_PLIST:-$ROOT/ios/Info.plist}"
RESOURCE_DIR="${RESOURCE_DIR:-}"
ICON_DIR="${ICON_DIR:-$ROOT/ios/icons}"
SDK="$(xcrun --sdk iphoneos --show-sdk-path)"
OUT="${BUILD_DIR:-$ROOT/build}"
APP="$OUT/$APP_NAME.app"
DIST="${DIST_DIR:-$ROOT/dist}"

rm -rf "$OUT" "$DIST"
mkdir -p "$APP" "$DIST"

STATIC_LIB="$TARGET_DIR/aarch64-apple-ios/$PROFILE/libhorizon_ios.a"
test -f "$STATIC_LIB"

xcrun --sdk iphoneos clang \
    -arch arm64 \
    -mios-version-min=15.0 \
    -isysroot "$SDK" \
    -fobjc-arc \
    -O2 \
    -Wl,-stack_size,0xA000000 \
    -framework UIKit \
    -framework Foundation \
    -framework CoreFoundation \
    -framework CoreGraphics \
    -framework Metal \
    -framework QuartzCore \
    -framework Security \
    -framework AuthenticationServices \
    -framework SafariServices \
    -framework IOKit \
    -framework AudioToolbox \
    -framework CoreAudio \
    -framework AVFoundation \
    -framework SceneKit \
    -framework GameController \
    "$MAIN_SOURCE" \
    "$STATIC_LIB" \
    -o "$APP/$APP_NAME"

cp "$INFO_PLIST" "$APP/Info.plist"
/usr/libexec/PlistBuddy -c "Set :CFBundleVersion $BUILD_NUMBER" "$APP/Info.plist"
/usr/libexec/PlistBuddy -c "Set :CFBundleIdentifier $BUNDLE_ID" "$APP/Info.plist"
/usr/libexec/PlistBuddy -c "Set :CFBundleExecutable $APP_NAME" "$APP/Info.plist"
/usr/libexec/PlistBuddy -c "Set :CFBundleName $APP_NAME" "$APP/Info.plist"
/usr/libexec/PlistBuddy -c "Set :CFBundleDisplayName $DISPLAY_NAME" "$APP/Info.plist"

# The engine app looks for a prepared resource tree in its bundle. It stays
# empty in the development package: Mojang assets are never bundled.
mkdir -p "$APP/horizon-client/26.3"

if [ -n "${RESOURCE_DIR:-}" ]; then
    test -f "$RESOURCE_DIR/horizon-resources.json"
    cp -R "$RESOURCE_DIR/"* "$APP/horizon-client/26.3/"
fi

if [ -n "${ICON_DIR:-}" ]; then
    cp "$ICON_DIR/"*.png "$APP/"
fi

# The shipped table bundle sits at the app root, where the shell looks it up by
# name (pathForResource:@"ship" ofType:@"hzr3"). With it, a first run needs no
# computer: the app downloads its own copy from Mojang and assembles around it.
# Leave SHIP_BUNDLE unset for a build that carries no derived data at all.
if [ -n "${SHIP_BUNDLE:-}" ]; then
    test -f "$SHIP_BUNDLE"
    cp "$SHIP_BUNDLE" "$APP/ship.hzr3"
    ls -lh "$APP/ship.hzr3"
fi

# The wasm loader finds one or more std members beside the executable.
if [ -n "${WASM_STD_DIR:-}" ]; then
    test -d "$WASM_STD_DIR"
    mkdir -p "$APP/horizon-wasm-std"
    cp "$WASM_STD_DIR/"*.wasm "$APP/horizon-wasm-std/"
    ls -lh "$APP/horizon-wasm-std/"
fi
if [ -n "${WASM_STD:-}" ]; then
    test -f "$WASM_STD"
    cp "$WASM_STD" "$APP/horizon-wasm-std.wasm"
    ls -lh "$APP/horizon-wasm-std.wasm"
fi

# Ad-hoc signature only: the IPA stays re-signable by Sideloadly/AltStore.
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
plutil -p "$APP/Info.plist" | head -40
unzip -l "$DIST/$IPA_NAME" | head -30
