# NCLR

**NativeCraftLauncherRust — an iOS launcher for the Horizon Rust engine.**

NCLR is a single-app iOS launcher for [Horizon](https://github.com/horizon-rust/horizon), a clean-room
Minecraft: Java Edition 26.3 client rewritten in Rust. The launcher and the game engine live in one
IPA: the launcher is shown first, and the engine starts only when **启动游戏** is tapped.

作者：**ZRXMAXLNX**、**可心**

---

## What is in this repository

This repository contains the **iOS launcher, port layer and build scripts** only.

- UIKit launcher and account/version/mod/settings pages
- iOS entry point that owns `UIApplicationMain` and starts Horizon on demand
- Patched winit late-start path for running the engine inside an already-running UIKit app
- In-game keymap editor overlay
- Metal backend selection and resource-loading fixes for iOS
- GitHub Actions / CircleCI build recipes

It does **not** contain Minecraft jars, Mojang assets, prepared resource trees, signing certificates
or provisioning profiles. Those stay on the build machine or are supplied as local secrets.

---

## Features

- **One app, launcher first**
  - Every launch opens the NCLR launcher.
  - Tapping **启动游戏** calls `horizon_ios_run` and hands the existing UIApplication to winit.
  - No background pre-start, no process exit/relaunch, no second app.
- **Horizon 26.3 engine**
  - Rust `aarch64-apple-ios` static library.
  - Metal backend on iOS.
  - Fixes for the world-resource handoff and loading-screen ordering.
- **Launcher pages**
  - Home, versions, mods, account and settings.
  - L1.1 developer build defaults to the premium/owned state; no login gate.
- **In-game keymap editor**
  - A small **键位** button appears over the game.
  - Collapsible top settings menu with a **按键大小** slider.
  - Drag controls, add controls, save and exit.
  - Saved to `Documents/keymap.json`.
  - The engine reads the file at startup and applies the built-in touch controls.
- **Complete resources on device**
  - The released IPA carries a prepared `horizon-client/26.3` tree so the game can start without a
    first-run download. The resource tree is injected at packaging time and is not committed here.

---

## Requirements

- iOS 15 or later, arm64
- Xcode command-line tools on macOS for local builds
- Rust 1.100.0 with `aarch64-apple-ios`
- For CI: a macOS GitHub Actions runner or CircleCI macOS executor

The engine checkout is pinned and prepared by `scripts/prepare-horizon.sh`. It applies the iOS
patches under `patches/` and copies the patched winit source under `port/winit`.

---

## Build

On macOS:

```sh
export HORIZON_DIR="$HOME/horizon"
bash scripts/prepare-horizon.sh

cd "$HORIZON_DIR"
IPHONEOS_DEPLOYMENT_TARGET=15.0 \
  cargo build -p horizon-ios --target aarch64-apple-ios --profile fast

cd -
HORIZON_DIR="$HORIZON_DIR" PROFILE=fast \
  APP_NAME=NCLR \
  MAIN_SOURCE="$PWD/ios/main_l1.m" \
  INFO_PLIST="$PWD/ios/Info-launcher.plist" \
  BUNDLE_ID=dev.zrxmaxlnx.nclr \
  IPA_NAME=NCLR-L1.1-unsigned.ipa \
  ICON_DIR="$PWD/ios/icons" \
  RESOURCE_DIR="/path/to/prepared/26.3" \
  BUILD_DIR="$PWD/build-nclr" DIST_DIR="$PWD/dist-nclr" \
  bash ios/build.sh
```

`RESOURCE_DIR` must point at a locally prepared `horizon-client/26.3` tree containing
`horizon-resources.json`. Never commit that tree.

The repository's GitHub Actions workflow builds an unsigned IPA without resources and publishes it
as a release asset. Resources and signing are added on the build machine.

---

## Installation

1. Remove any older NCLR / HorizonEngine build.
2. Install the signed IPA for the registered device, or re-sign the unsigned IPA with your own
   certificate and provisioning profile.
3. Open NCLR: the launcher must appear first.
4. Tap **启动游戏**; the engine starts after the tap.
5. After entering the game, tap **键位** in the top-left to edit the touch layout.

The signed development IPA is bound to a single registered device; the unsigned IPA is intended for
re-signing.

---

## Keymap JSON

The launcher and in-game editor write `Documents/keymap.json`:

```json
{
  "buttonSize": 1.0,
  "bindings": {
    "forward": "W",
    "jump": "Space"
  },
  "layout": {
    "forward": { "x": 0.13, "y": 0.76, "size": 0.16 }
  }
}
```

The engine reads this file at game startup. Supported built-in ids are:

`forward`, `back`, `left`, `right`, `jump`, `sneak`, `sprint`, `attack`, `use`, `inventory`,
`chat`, `pause`, `settings`, `debug`, `perspective`, `offhand`, `shield`.

The four movement ids share the game's single movement joystick. Custom ids are preserved in the
JSON but do not create new engine controls in this version.

Changes made in the in-game editor take effect the next time the game is started.

---

## Current limitations

- L1.1 is a developer build; the default owned state is not a production licensing implementation.
- Custom keymap ids beyond the built-in controls are stored but not rendered by the engine.
- The project is not affiliated with Mojang, Microsoft or the Horizon maintainers.
- Minecraft assets and Mojang files are not distributed here.

---

## Credits

- **ZRXMAXLNX** — launcher, iOS port, packaging
- **可心** — product and integration work
- **Horizon** — Rust game engine (separate upstream project)

See [LICENSE](LICENSE) for the repository license.
