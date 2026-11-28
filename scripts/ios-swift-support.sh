#!/bin/bash
# Shared toolchain settings for the hand-written Objective-C/Swift launcher.
# Callers set ROOT and their own shell error handling before sourcing this file.

nclr_configure_ios_toolchain() {
    SDK_NAME="${SDK_NAME:-iphoneos}"
    ARCH="${ARCH:-arm64}"
    IPHONEOS_DEPLOYMENT_TARGET="${IPHONEOS_DEPLOYMENT_TARGET:-15.0}"

    case "$SDK_NAME/$ARCH" in
        iphoneos/arm64)
            IOS_TARGET="arm64-apple-ios$IPHONEOS_DEPLOYMENT_TARGET"
            RUST_TARGET=aarch64-apple-ios
            BUNDLE_PLATFORM=iPhoneOS
            ;;
        iphonesimulator/arm64)
            IOS_TARGET="arm64-apple-ios$IPHONEOS_DEPLOYMENT_TARGET-simulator"
            RUST_TARGET=aarch64-apple-ios-sim
            BUNDLE_PLATFORM=iPhoneSimulator
            ;;
        iphonesimulator/x86_64)
            IOS_TARGET="x86_64-apple-ios$IPHONEOS_DEPLOYMENT_TARGET-simulator"
            RUST_TARGET=x86_64-apple-ios
            BUNDLE_PLATFORM=iPhoneSimulator
            ;;
        *)
            printf 'unsupported iOS SDK/architecture: %s/%s\n' "$SDK_NAME" "$ARCH" >&2
            return 1
            ;;
    esac

    SDK="$(xcrun --sdk "$SDK_NAME" --show-sdk-path)"
    export SDK_NAME ARCH IPHONEOS_DEPLOYMENT_TARGET
}

nclr_uses_swift_sidebar() {
    [ "$(basename "$1")" = main_l1.m ] ||
        grep -Eq '^[[:space:]]*#[[:space:]]*import[[:space:]]*[<"]NCLRSidebar[.]h[>"]' "$1"
}

nclr_compile_ios_sources() {
    local source="$1"
    local objects="$2"
    mkdir -p "$objects"

    IOS_OBJECTS=("$objects/main.o")
    IOS_LINKER=(xcrun --sdk "$SDK_NAME" clang -target "$IOS_TARGET" -isysroot "$SDK")

    if nclr_uses_swift_sidebar "$source"; then
        # Library compilation supplies @objc symbols alongside Objective-C main.
        # Objective-C consumes the hand-written NCLRSidebar.h.
        xcrun --sdk "$SDK_NAME" swiftc \
            -sdk "$SDK" -target "$IOS_TARGET" \
            -swift-version 5 -parse-as-library -module-name NCLRSidebar \
            -O -whole-module-optimization -emit-object \
            "$ROOT/ios/NCLRSidebar.swift" -o "$objects/NCLRSidebar.o"
        IOS_OBJECTS+=("$objects/NCLRSidebar.o")
        # The Swift driver supplies SDK overlays and runtime compatibility libs.
        IOS_LINKER=(xcrun --sdk "$SDK_NAME" swiftc -sdk "$SDK" -target "$IOS_TARGET")
    fi

    xcrun --sdk "$SDK_NAME" clang \
        -target "$IOS_TARGET" -isysroot "$SDK" \
        -fobjc-arc -O2 -I "$ROOT/ios" \
        -c "$source" -o "$objects/main.o"
}

nclr_embed_swift_runtime() {
    local app="$1"
    local executable="$2"
    local library
    mkdir -p "$app/Frameworks"
    # The scanner uses the executable's deployment target to select back-deployed
    # libraries. iOS supplies the baseline Swift runtime at our iOS 15 minimum.
    xcrun --sdk "$SDK_NAME" swift-stdlib-tool --copy \
        --platform "$SDK_NAME" --scan-executable "$app/$executable" \
        --destination "$app/Frameworks"
    for library in "$app/Frameworks/"*.dylib; do
        [ -f "$library" ] || continue
        codesign --force --sign - --timestamp=none "$library"
    done
}
