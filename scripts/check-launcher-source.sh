#!/bin/bash
# Verify the source entry and download path used by the launcher build.
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
MAIN_SOURCE="${1:-$ROOT/ios/main_l1.m}"

test -f "$MAIN_SOURCE"

grep -q 'assetQueue.maxConcurrentOperationCount = NCLRResourceDownloadConcurrency()' "$MAIN_SOURCE"
grep -q 'downloadConcurrencyChanged:' "$MAIN_SOURCE"

printf 'source check passed: %s\n' "$MAIN_SOURCE"
