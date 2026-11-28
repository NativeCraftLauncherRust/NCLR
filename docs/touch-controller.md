# TouchController 触控模组（.hmod）

把 [TouchController](https://github.com/TouchController/TouchController) 的核心触控操作移植成 NCLR 的 `.hmod` 模组。

上游是 Java 版模组（Fabric / NeoForge / Forge，LGPL-3.0-or-later）。Horizon 的引擎是 Rust，模组只能通过模组 API 重写，所以这里是按功能重写的移植：摇杆 / 方向键、视角拖动之外的攻击与使用、跳跃、潜行、疾跑、背包、换手，加上可配置的位置和设置。上游的游戏内拖拽布局编辑器、Combine 界面、自定义按键绑定没有移植——布局在 NCLR 启动器自带的触控编辑器里调。

## 为什么需要引擎改动

模组 API 原本没有任何触摸钩子：iOS 上手指只喂给引擎内建的触控层，模组只能看到鼠标（`mouse_button_pre`、`mouse_scrolling`）和键位。所以这次移植包含两部分：

| 部分 | 位置 | 作用 |
| --- | --- | --- |
| 模组 API 钩子 `touch_input` | `crates/horizon-mod-vocab/src/hooks/touch_input.rs`（声明）、`crates/horizon-client/src/game/native_hooks/touch_input.rs`（入口）、`crates/horizon-client/src/game/interface/pointer_capture.rs`（触发） | 把每根手指的 id、阶段、归一化坐标和画布尺寸交给模组；模组可以 `canceled = true` 认领这根手指 |
| 模组包 | `mods/touch-controller/` | `touch_controller.hmod`（wasm 代码模组） |

钩子是 cancel 契约：引擎内建的 `TouchControls` 只在模组没认领的手指上工作，所以两套控件不会同时响应同一根手指。钩子在 `hardware_input_attached` 判断之前触发，接键盘/鼠标时模组仍能拿到触摸；标题界面和加载界面的点击仍归菜单。

引擎侧的改动以 `patches/mod-api-touch-input.patch` 维护，由 `scripts/prepare-horizon.sh` 在 `ios-all.patch` 之后自动应用。

## 数据流

```
iOS UIKit touch
  → winit WindowEvent::Touch
  → Game::touch_event
      → touch_input 钩子（模组认领自己控件上的手指，画面上显示摇杆/按钮）
      → 未被认领：内建 TouchControls（视角拖动、快捷栏点击、暂停按钮…）
模组按下按钮
  → ClientHost::set_mapping_down / click_mapping（原版按键映射，服务端看到正常客户端）
模组摇杆
  → movement_input 钩子的模拟轴（forward / strafe）
```

## 构建

```sh
# 1. 准备引擎（首次，或引擎改动后）
HORIZON_DIR="$PWD/.local/horizon" bash scripts/prepare-horizon.sh

# 2. 编译模组（需要 .local/rustup 里的 1.99.1 工具链，脚本自己设置）
bash mods/touch-controller/build.sh
```

产物：`mods/touch-controller/touch_controller.hmod`，同时把 `[wasm]` 的 sha512 / toolchain 写回项目的 `horizon-mod.toml`。

## 安装与使用

1. 用带 `touch_input` 钩子的构建（`ios/build.sh`，并把 `ios/wasm-std/` 打进去）生成 IPA。
2. 把 `touch_controller.hmod` 导入 NCLR 的模组页，打开开关（文件放在 `Documents/horizon-mods/touch_controller.hmod`）。
3. 进游戏后第一次触摸时控件出现。
4. 在 NCLR 的触控编辑器里把重复的内建控件设为隐藏（摇杆、攻击、使用、跳跃、潜行、疾跑），保留暂停 / 设置等想要的按钮。两套控件同时显示不会互相抢手指，但会重叠。

## 配置

`config/touch_controller-client.toml`（首次启动自动生成，改完即热重载）：

```toml
[layout]
mode = "joystick"        # joystick | dpad

[stick]
x = 0.170                # 屏幕宽度比例
y = 0.700                # 屏幕高度比例
size = 0.230             # 屏幕高度比例（直径）

[button.attack]
x = 0.855
y = 0.855
size = 0.093
```

其它 `[button.*]`：`use`、`jump`、`sneak`、`sprint`、`inventory`、`swap`。删掉一行就恢复内置值；越界值会被拉回屏幕内。

选项 → 模组 → TouchController → 配置：

| 设置 | 作用 |
| --- | --- |
| 布局 | 摇杆 / 方向键（选方向键会覆盖配置文件的 joystick） |
| 控件大小 | 0.6–1.6 倍 |
| 控件不透明度 | 0.2–1.0 |
| 摇杆死区 | 0–0.45 |
| 左手模式 | 左右镜像 |
| 潜行开关 | 点一下保持潜行，再点取消；关掉则按住才潜行 |

## 已知限制

- 只能跑在带 `touch_input` 钩子的引擎上：上游 Horizon 没有这个钩子，官方 SDK 也没有。
- 模组不能打开暂停菜单（模组 API 只给 `close_screen` / `open_inventory`），所以暂停按钮仍用内建的。
- 没有点按世界瞄准的 tap 模式：那需要模组接管视角拖动（目前视角仍由内建触控处理，灵敏度走游戏设置）。
- 布局编辑在配置文件和启动器里做，游戏内没有拖拽界面。
- wasm 模组需要 `horizon-wasm-std-1.99.1.wasm`（`ios/wasm-std/` 已有，`ios/build.sh` 的 `WASM_STD_DIR` 会打进 app）。

## 许可

模组包与引擎改动均按 LGPL-3.0-or-later 发布，与上游一致。控件外观、默认布局、文案和配置格式为本次移植新写，不含上游 Java 代码或贴图。
