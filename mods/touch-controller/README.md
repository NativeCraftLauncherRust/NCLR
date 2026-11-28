# TouchController for NCLR

把 [TouchController](https://github.com/TouchController/TouchController) 的核心触控操作移植成 NCLR 的 `.hmod` 模组。

上游是 Java 版模组（Fabric/NeoForge/Forge，LGPL-3.0-or-later），依赖 Combine GUI 库和一整套游戏内布局编辑器；Horizon/NCLR 的游戏本体是 Rust，模组只能通过模组 API 重写，因此这里是**按功能重写**的移植，不是逐行翻译。许可证沿用上游的 LGPL-3.0-or-later。

## 功能

- 摇杆（按推开的距离模拟移动）或方向键（四个移动键）两种布局
- 攻击、使用：按下即点一次，按住持续（挖方块、拉弓、连续放置都正常）
- 跳跃、疾跑（按住）、潜行（默认点按开关，可在设置里改回按住）
- 背包、交换副手
- 左手模式（左右镜像）、控件大小、不透明度、摇杆死区、潜行开关
- 位置由 `config/touch_controller-client.toml` 决定，逐控件可改；其余选项在 **选项 > 模组 > TouchController > 配置** 里

不包含（上游有、这里没有）：游戏内拖拽布局编辑器、点按世界瞄准的 tap 模式、自定义按键绑定、多套预设、Combine 界面。布局直接在 NCLR 启动器的触控编辑器里调，比在游戏里拖更省事。

## 原理

模组调用 `touch_input` 钩子拿手指事件（id、阶段、坐标），自己画控件；手指落在控件上时把事件"认领"（`canceled = true`），引擎内建的触控层就不会对同一根手指再响应。按键通过 `set_mapping_down` / `click_mapping` 按原版按键映射走，服务端看到的和正常客户端一样；移动走 `movement_input` 的模拟轴。

`touch_input` 是本仓库的 Horizon 分支新增的模组 API 钩子（`crates/horizon-mod-vocab/src/hooks/touch_input.rs`，在 `Game::touch_event` 里触发）。**上游 Horizon 没有这个钩子，所以这个 .hmod 只能跑在带该钩子的引擎上**（即本仓库构建的 IPA）。

## 安装

1. NCLR 需要带 `touch_input` 钩子和 `horizon-wasm-std-1.99.1.wasm` 的构建（本仓库 `ios/build.sh` + `WASM_STD_DIR=ios/wasm-std`）。
2. 把 `touch_controller.hmod` 导入 NCLR（模组页右上角导入，或直接用文件 App 放进 `Documents/horizon-mods/`），在模组页打开它。
3. 进游戏后第一次触摸屏幕时控件出现；在 NCLR 的触控编辑器里把**重复的**控件（摇杆、攻击、使用、跳跃、潜行、疾跑）设为隐藏，只留暂停/设置等你想要的，避免两套控件叠在一起。

## 配置

`config/touch_controller-client.toml`（首次启动自动生成）：

```toml
[layout]
mode = "joystick"      # 或 "dpad"

[button.attack]
x = 0.855              # 屏幕宽度的比例，0 左 1 右
y = 0.855              # 屏幕高度的比例，0 上 1 下
size = 0.093           # 屏幕高度的比例
```

删掉某一行就恢复该控件的内置值；超出屏幕的数值会被拉回边缘。设置里的"控件大小/不透明度/左手模式/布局"在这个文件之上生效。

## 构建

```sh
# 需要 Rust 1.99.1 + rust-src + wasm32-unknown-unknown（工具会自动装）
cargo horizon-mod build --wasm        # 产出 target/horizon-mod/touch_controller.hmod
cargo horizon-mod check target/horizon-mod/touch_controller.hmod
```

`Cargo.toml` 里 `horizon-mod-api` 指向 `.horizon/sdk/api/crates/horizon-mod-api`：由 `cargo horizon-mod new` 拉取的 SDK，或（本仓库的做法）指到本地 Horizon 签出，因为这里需要带 `touch_input` 的自定义 API。

## 许可

LGPL-3.0-or-later，与上游一致。控件外观、默认布局和文案为本次移植新写；不包含上游的任何 Java 代码或贴图。
