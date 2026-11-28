# NCLR

NCLR 是一个面向 iOS 的独立 Minecraft 启动器，以开源项目 Horizon 的 Rust 与 wgpu 客户端实现作为游戏引擎基础。NCLR 项目负责启动器界面、iOS 平台集成、账号与版本管理、资源导出与设备端组装、触控布局以及相关兼容修复。

项目作者为 **ZRXMAXLNX** 与 **可心**。

## 关于 Horizon

Horizon 是使用 Rust 与 wgpu 构建 Minecraft: Java Edition 客户端实现的开源项目。Horizon 及其贡献者完成了游戏引擎架构、世界渲染、游戏逻辑、资源格式、网络协议以及大量底层功能。

NCLR 建立在 Horizon 的工作之上。NCLR 作者负责启动器、iOS 平台适配、触控与键盘映射、资源导出链整合、设备端资源组装以及 NCLR 自身的界面和工程改动。

谨向 Horizon 项目及所有贡献者致以诚挚敬意。第三方组件继续适用各自的许可证与版权声明。

> **仓库、CI 构建目录和发布的 IPA 不包含任何 Mojang 或 Microsoft 的 Minecraft 专有游戏资源，也不提供游戏资源镜像。**
>
> **No Mojang or Microsoft proprietary Minecraft assets are bundled or redistributed by this repository or its CI artifacts.**

## 版权与资源分发原则

NCLR 与 Mojang Studios、Microsoft 及其关联公司没有隶属、授权或背书关系。Minecraft 名称、游戏内容、商标及相关权利归各自权利人所有。

本仓库只维护项目自行编写的启动器代码、iOS 适配代码、构建脚本、测试和文档，不分发 Minecraft 客户端 JAR、服务端 JAR、资源索引、纹理、音效、音乐、语言文件、字体、官方库、Realms 或 Xbox 相关二进制，也不附带由官方文件生成的 `ship.hzr3` 等数据束。

仓库通过以下方式避免提交或打包官方游戏资源。

- `.gitignore` 排除 `*.jar`、`.local/`、`horizon-client/`、`assets/objects/` 与 `*.ipa`
- CI 构建使用空的 `SHIP_BUNDLE` 与 `RESOURCE_DIR`
- 发布 IPA 不包含 `horizon-client/26.3` 资源树
- 发布 IPA 不包含 `ship.hzr3`
- Pull Request、Issue 附件和构建缓存不得包含 Mojang 专有资源

## 游戏文件如何获得

玩家启动游戏时，NCLR 会读取 Mojang 官方版本清单，并根据清单元数据从官方地址下载当前版本所需的文件。下载由设备直接发起，文件仅保存在用户本机的 NCLR 应用目录中。

本项目不运营 Minecraft 文件镜像、网盘、种子、代理下载池或资源分发 API，不向其他人转售、转存或分享下载得到的游戏内容。

在线登录、正版验证与 Minecraft 服务访问由 Microsoft 和 Mojang 的官方服务处理。NCLR 不绕过账号验证、所有权检查、付费流程或数字版权管理。用户需要自行确保自己拥有使用相关游戏内容与在线服务所需的合法资格，并遵守 Minecraft 最终用户许可协议、Microsoft 服务条款及适用法律。

## 作者移植的 Rust 资源导出与设备端组装链

NCLR 包含作者移植并整合的 Rust 资源导出与设备端组装链路。这条链路让 iPhone 使用玩家自己下载的 `client.jar` 在设备上生成 Horizon 所需的数据表，不再依赖预先生成的 `ship.hzr3`，也不需要在电脑上运行资源准备工具。

资源准备流程如下。

1. 从 Mojang 官方版本清单获取当前版本的 `client.jar` 与资源索引
2. `hzr-export` 在设备上读取 JAR 字节码和 Minecraft 数据文件
3. 生成方块、物品、命令、碰撞、生物、渲染、结构、创造模式等 38 组数据表
4. `hzr-device` 将数据表组装为本地资源目录
5. 生成 `horizon-resources.json` 和准备完成标记
6. 所有原始文件与生成结果只保存在当前设备

当前 Rust 导出链已经覆盖以下内容。

- 38/38 组 Horizon 数据表
- 1,286/1,286 个方块的 27 个 gameplay 字段
- 35,723 行方块状态行为
- 14/14 个创造模式物品分类
- 1,658/1,658 个物品的语义一致性
- 碰撞、遮挡、支撑形状、结构方块、粒子、声音、命令注册表与生物数据

工程上，`hzr-export` 已拆分为一个 Rust 库和 10 个分组命令，解决大型表在单次 `rustc` 编译时可能出现的内存耗尽，同时让设备端只链接实际需要的导出库。设备端构建不再需要提交 `ship.hzr3`，也不需要把旧版资源准备流程生成的中间数据放入 IPA。

这条资源导出链是 NCLR 无 Mojang 资源分发的技术基础。仓库保留转换和组装代码，官方 JAR、资源文件和生成后的数据表都留在用户设备上，不进入 Git 历史、CI artifact 或公开发布包。

## TouchController 触控模组

`mods/touch-controller/` 是 [TouchController](https://github.com/TouchController/TouchController) 核心触控操作的 `.hmod` 移植（LGPL-3.0-or-later）。模组 API 原本没有触摸钩子，因此这次移植同时给引擎加了一个 `touch_input` 钩子，把每根手指的 id、阶段和坐标交给模组，并允许模组认领自己的控件；内建触控层只在没被认领的手指上工作。

```bash
HORIZON_DIR="$PWD/.local/horizon" bash scripts/prepare-horizon.sh   # 引擎补丁（含 touch_input）
bash mods/touch-controller/build.sh                                 # 产出 touch_controller.hmod
```

细节、配置项与已知限制见 [docs/touch-controller.md](docs/touch-controller.md)。

## 权利人联系

如果权利人认为仓库中的某个文件侵犯其权利，请通过仓库维护者公布的 GitHub 联系方式提交具体文件路径、权利说明和处理请求。维护者会检查相关内容，并及时删除、替换或停止分发有争议的材料。

请勿在 Issue、Pull Request 或讨论区上传游戏 JAR、资源包、官方资源索引或其他专有文件。

以上内容为项目的资源管理与发布原则，不构成法律意见。

## 功能

- iOS 15 及以上，arm64，横屏
- Microsoft 与 Minecraft 官方账号登录
- Mojang 官方版本清单与运行资源准备
- 版本、模组、光影与资源包管理
- 触控布局编辑、按键大小与位置调整
- 常驻系统键盘按钮，可通过触控编辑器调整位置和尺寸
- 点按与切换触发方式
- 陀螺仪视角、灵敏度及 X/Y 轴反转
- 外接键盘与鼠标
- 局域网与服务器联机
- Metal 渲染与高刷新率显示
- TouchController 触控模组：`touch_controller.hmod` 提供摇杆/方向键、攻击使用、跳跃潜行疾跑等按钮，位置由配置文件决定

## 环境要求

- iOS 或 iPadOS 15.0 及以上
- arm64 设备
- 安装应用需要用户自己的签名证书或有效的侧载环境
- 在线功能需要可用的 Microsoft 账号与 Minecraft: Java Edition 访问资格

## 安装

NCLR 发布的是未签名开发构建，应用需要由使用者使用自己的 Apple 开发者证书或侧载工具重新签名。签名、设备授权和安装过程由使用者自行完成。

安装包只包含 NCLR 项目代码，不包含 Minecraft 游戏文件。首次准备版本时，应用会从官方服务下载所需内容。

## 从源码构建

构建环境需要 macOS、Xcode、Rust 1.100 工具链以及 iOS 编译目标。

```bash
rustup target add aarch64-apple-ios wasm32-unknown-unknown
HORIZON_DIR="$PWD/.local/horizon" bash scripts/prepare-horizon.sh
```

生成新版启动器时，请使用完整引擎构建脚本。它固定使用 `ios/main_l1.m`，包含游戏大厅、版本库和主题设置。

```bash
RESOURCE_DIR="" \
SHIP_BUNDLE="" \
bash ios/build.sh
```

发布前使用统一构建脚本。它会先检查新版入口的关键实现，再构建并验证 IPA 内的符号，最后写出带提交号、工作区改动和 SHA-256 的清单文件。

```bash
HORIZON_DIR="$PWD/.local/horizon" PROFILE=fast bash scripts/build-launcher.sh
```

产物位于 `dist/`，同时生成同名的 `.manifest.json`。源码检查可以单独运行 `bash scripts/check-launcher-source.sh`，已有 IPA 可以运行 `bash scripts/verify-launcher-ipa.sh dist/<文件名>.ipa` 复核。

仓库管理约定：功能改动从 `main` 拉出短生命周期分支，用 Pull Request 或至少一次明确的独立提交合入。每次合并后重新构建并验证产物。新的 UI、下载器或模组工作先放在独立分支，完成构建与产物检查后合入。CI 和本地发布都使用 `ios/main_l1.m`；旧入口脚本只在明确测试旧启动器时使用。

`ios/build-launcher.sh` 用于独立的旧版启动器入口 `ios/main_launcher_only.m`。完整引擎构建需要先构建 `horizon-ios` 静态库。两个脚本都会在构建输出中打印实际使用的入口文件。

如需自行准备测试资源，请将文件保存在本机被忽略的目录中，并确认你的使用行为符合相关协议。这类文件不应提交到仓库，也不应上传到 CI、Issue 或 Pull Request。

## 开发与贡献

提交代码前请确认改动只涉及项目源码、脚本、测试和文档。以下内容不接收。

- Minecraft 客户端或服务端 JAR
- 官方资源索引与 Objects
- 官方纹理、音效、音乐、语言文件和字体
- `ship.hzr3` 或其他由官方文件生成的数据束
- Mojang、Microsoft 或第三方账户令牌
- 用于绕过登录、所有权验证或付费流程的代码

涉及第三方模组、光影、资源包或库时，请同时遵守对应项目的许可证。

## 许可证

NCLR 使用 GNU Lesser General Public License v3.0。仓库中的第三方组件继续适用各自的许可证与版权声明。

Minecraft、Mojang 与 Xbox 是各自权利人的商标或注册商标。NCLR 项目不声称拥有这些名称、商标或游戏内容的任何权利。

## 致谢

- Horizon 项目及全体贡献者
- winit 与 Rust 社区
- NCLR 项目作者 **ZRXMAXLNX** 与 **可心**
