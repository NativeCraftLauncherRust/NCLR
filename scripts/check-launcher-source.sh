#!/bin/bash
# Verify the complete launcher's source and optionally compile its UI objects.
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
COMPILE=0
if [ "${1:-}" = --compile ]; then
    COMPILE=1
    shift
fi
MAIN_SOURCE="${1:-$ROOT/ios/main_l1.m}"
if [ "$#" -gt 1 ]; then
    echo 'usage: check-launcher-source.sh [--compile] [source]' >&2
    exit 1
fi
source "$ROOT/scripts/ios-swift-support.sh"

test -f "$MAIN_SOURCE"

grep -q 'assetQueue.maxConcurrentOperationCount = NCLRResourceDownloadConcurrency()' "$MAIN_SOURCE"
grep -q 'downloadConcurrencyChanged:' "$MAIN_SOURCE"

if nclr_uses_swift_sidebar "$MAIN_SOURCE"; then
    test -s "$ROOT/ios/NCLRSidebar.swift"
    test -s "$ROOT/ios/NCLRSidebar.h"
    grep -Eq '^[[:space:]]*#[[:space:]]*import[[:space:]]*[<"]NCLRSidebar[.]h[>"]' "$MAIN_SOURCE"
    grep -Eq '^[[:space:]]*import[[:space:]]+SwiftUI([[:space:]]|$)' "$ROOT/ios/NCLRSidebar.swift"
    grep -Eq '@interface[[:space:]]+NCLRSidebarController([[:space:]:]|$)' "$ROOT/ios/NCLRSidebar.h"
    grep -Eq '@objc\(NCLRSidebarController\)' "$ROOT/ios/NCLRSidebar.swift"
fi

if [ "$COMPILE" = 1 ]; then
    nclr_configure_ios_toolchain
    OBJECT_DIR="$(mktemp -d "${TMPDIR:-/tmp}/nclr-source-check.XXXXXX")"
    trap 'rm -rf "$OBJECT_DIR"' EXIT
    nclr_compile_ios_sources "$MAIN_SOURCE" "$OBJECT_DIR"
    if nclr_uses_swift_sidebar "$MAIN_SOURCE"; then
        xcrun nm -U "$OBJECT_DIR/NCLRSidebar.o" > "$OBJECT_DIR/swift-symbols.txt"
        grep -Eq '[[:space:]]_OBJC_CLASS_\$_NCLRSidebarController$' "$OBJECT_DIR/swift-symbols.txt"
    fi
    printf 'UI object compilation passed: %s\n' "$IOS_TARGET"
fi

printf 'source check passed: %s\n' "$MAIN_SOURCE"
