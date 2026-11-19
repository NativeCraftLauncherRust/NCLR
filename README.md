# Horizon iOS

iOS port layer for [Horizon](https://github.com/horizon-rust/horizon), the
Minecraft: Java Edition 26.3 client rewritten in Rust.

This repository contains **port code only**. It does not contain Minecraft
game files, assets, or Mojang code. Horizon itself requires the player's own
Minecraft: Java Edition installation; prepared resources stay private and are
never committed here.

- Repository: <https://github.com/zhangrongxue/horizon-ios>
- M3 development release: <https://github.com/zhangrongxue/horizon-ios/releases/tag/m3-engine-dev-20261116>
- M4 release (Documents import): <https://github.com/zhangrongxue/horizon-ios/releases/tag/m4-documents-dev>

## Status

| Milestone | What it proves | State |
|---|---|---|
| M1 | Rust → iOS arm64 static library → UIKit shell → unsigned IPA | done |
| M2 | `horizon-client-lib` compiles for `aarch64-apple-ios` and links | done |
| M3 | full `horizon-client` engine links and boots on an iPhone | done |
| M4 | engine reads imported resources from Documents; Files sharing | built, device retest pending |
| M5 | prepare the user's own 26.3 resources and import them | resources prepared locally, import pending |
| M6 | touch controls, audio, playable world | not started |

The M3 device log proves the engine starts (`Horizon 0.1.0`, open-file-limit
raise) and then refuses to continue because `horizon-resources.json` is
missing. M4 changes the resource lookup to prefer
`Documents/horizon-client/26.3`, enables `UIFileSharingEnabled` and
`LSSupportsOpeningDocumentsInPlace`, and keeps the empty bundle directory only
as a fallback.

The engine app is a 55.6 MB unsigned arm64 IPA whose main binary is about
173.9 MB. It has **no bundled Minecraft resources** and **no touch controls**;
it is not playable until the prepared resources are imported.

## Layout

```
ios/
  main.m             UIKit diagnostic shell (M1/M2)
  main_engine.m      engine app: calls horizon_ios_run, winit owns UIApplicationMain
  Info.plist         shared app metadata
  build.sh           links the static library, ad-hoc signs, packages an IPA
port/horizon-ios/    Rust static-library crate and C ABI
patches/             changes the port needs in upstream Horizon
scripts/             build/prepare helpers
.github/workflows/   macOS CI
```

The Rust crate exposes:

- `horizon_ios_diagnostics()` — a C string with build/engine facts;
- `horizon_ios_self_test()` — a linked-core check;
- `horizon_ios_run(resources, game_dir)` — starts the real client.

## Build on macOS

Requirements: Xcode with an iOS SDK, Rust 1.100+, Python 3, `zip`,
`codesign`, `PlistBuddy`.

```bash
git clone https://github.com/zhangrongxue/horizon-ios.git
cd horizon-ios
HORIZON_DIR="$PWD/../horizon" bash scripts/prepare-horizon.sh
cd "$HORIZON_DIR"
IPHONEOS_DEPLOYMENT_TARGET=15.0 \
  cargo build -p horizon-ios --target aarch64-apple-ios --profile fast
cd -
HORIZON_DIR="$HORIZON_DIR" PROFILE=fast bash ios/build.sh
```

The engine app:

```bash
HORIZON_DIR="$HORIZON_DIR" PROFILE=fast \
  APP_NAME=HorizonEngine \
  MAIN_SOURCE="$PWD/ios/main_engine.m" \
  BUNDLE_ID=dev.horizon.ios.engine \
  IPA_NAME=HorizonEngine-dev-unsigned.ipa \
  BUILD_DIR="$PWD/build-engine" DIST_DIR="$PWD/dist-engine" \
  bash ios/build.sh
```

Or run the `ios-build` GitHub Actions workflow; it uploads both IPAs as
artifacts.

## Pinned upstream

The port is tested against Horizon `69ab572dbd1bf2f2924a424f791014e09d9d0b50`.
The patch and the crate live here, so upstream is cloned fresh for each build.

## Assets and distribution

- Never commit Mojang jars, assets, decompiled sources, or `.local/` resources.
- The IPA contains no Minecraft assets.
- For a playable build, prepare `horizon-client/26.3` on a desktop with the
  player's own Minecraft installation and import it into the app's Documents
  directory. That import path is part of M5.

## License

The port code is LGPL-3.0-or-later, matching Horizon. Horizon's original work
and its third-party notices remain with the upstream project. Minecraft is a
trademark of Mojang Synergies AB; this is not an official Minecraft product.
