# NCLR 项目上下文

Last reviewed 2026-11-27

- [features.md](features.md) 记录启动器的页面转场与动效开关。
- [memory.md](memory.md) 记录本次实现的稳定性和构建约束。

## 构建入口

- 新版完整界面使用 `ios/main_l1.m`，由 `scripts/build-launcher.sh` 或 `ios/build.sh` 构建。
- 独立旧版启动器使用 `ios/main_launcher_only.m`，由 `ios/build-launcher.sh` 构建。
- 发布前运行 `scripts/check-launcher-source.sh`，产物生成后运行 `scripts/verify-launcher-ipa.sh`。
