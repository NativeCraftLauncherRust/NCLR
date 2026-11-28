# 实现记忆

Last reviewed 2026-11-28

- 页面转场只捕获右侧内容区域，快照通过根视图绘制以包含背景。
- 页面切换模糊开关的新用户默认值为关闭，已有 `NSUserDefaults` 值优先保留。
- 模糊使用共享 GPU `CIContext`、复用的 `CIAffineClamp` 和 `CIGaussianBlur`，`CADisplayLink` 限制为 60Hz。
- `transitionGeneration` 和控制器引用负责处理快速点击、反向切换、旋转、后台和动效设置变化。
- 主题使用 `NCLRThemeMode` 保存跟随系统、深色或浅色模式，旧版 `NCLRThemeDark` 值会兼容迁移。
- `ios/build.sh` 必须链接 Core Image。完整链接检查可使用 `.local/horizon/target/aarch64-apple-ios/fast/libhorizon_ios.a`。
- SwiftUI 侧边栏由 `UIHostingController` 承载，根控制器通过标准子控制器流程接入。手写的 `ios/NCLRSidebar.h` 与 Swift 的 `@objc` 选择器保持一致。
- 侧边栏根布局使用 `.ignoresSafeArea(.container, edges: .bottom)`，账号按钮由 8pt 底部内边距定位。品牌按内容高度布局，底部 8pt 内边距与导航首行 32pt 占位合计 40pt 间距。
- 侧边栏渐变直接使用主题底色的透明度，实色区与渐变起点使用同一颜色。首尾占位属于导航的滚动目标，避免返回首尾项目时被自动滚动消耗。
- 选中图层使用 `CASpringAnimation`，刚度 280、阻尼 26，时长取 `settlingDuration`，从 presentation layer 接续快速反向切换。普通 SwiftUI 更新保留进行中的动画，布局尺寸变化与减少动态效果取消动画。页面控制器仍同步构建，首次页面构建耗时需要与选择器渲染分别评估。
- 右侧进入、退出、进入延迟和模糊时长统一缩短 20%，减少动态效果路径同步缩短。
- 头像占位背景显式设置 `ignoresSafeAreaEdges: []`，并将头像裁剪在 32pt 方形内，避免背景延伸到底部边缘。
- 完整启动器的 Objective-C 和 Swift 分别编译为对象，再由 `swiftc` 链接，Swift 使用 5 语言模式，最低 iOS 15。`scripts/ios-swift-support.sh` 管理 SDK、目标架构和运行库扫描。
- `scripts/check-launcher-source.sh --compile` 可编译检查混合 UI，`scripts/verify-launcher-ipa.sh` 检查侧边栏定义、SwiftUI 依赖、运行库和最低系统版本。
- `ios/main.m` 的并发下载实现不会进入 `ios/main_l1.m` 的产物；功能需要同时检查入口源码和最终 IPA 符号。
- `scripts/build-launcher.sh` 会在 `dist/` 生成同名 manifest，记录提交、工作区状态、入口和 SHA-256。

- WASM 标准库资源由 `ios/wasm-std/manifest.json` 定义，完整构建入口默认强制校验并打包整个兼容集合。`WASM_STD` 继续支持附加旧单文件。
- `scripts/verify-wasm-ipa.py` 校验最终 IPA 的完整清单、成员哈希与构建身份，再通过生产加载器的 Pulley64 后端检查 TouchController 的链接、API 和初始化。通过后才发布正式 IPA 文件名，CI 同时上传身份 manifest。
- `patches/ios-all.patch` 保留加载器发现、去重、多候选诊断及发布 verifier。准备脚本在检查基础补丁前暂时移除触控叠加补丁，检查失败也会恢复它。设置 Git 发现边界，避免仓库内解压的 vendor 树因上层 Git 跳过补丁并误报成功。回归实际核对新增文件、连续准备后的字节一致性和两个补丁的反向检查。已有准备树的额外源码修改需保留。
- `scripts/test-wasm-std.py --ipa /path/to/app.ipa` 覆盖候选损坏、不兼容回退、旧单文件、哈希不符、API 布局拒绝、构建前失败保护，以及最终 IPA 删改资源。宿主机检查与真机游戏验收分别记录。
- 历史 `horizon-wasm-std.wasm` 的完整编译器与 SDK 来源未核实，清单以 null 如实记录。1.99.1 成员已用固定 SDK 重建并核对完整字节。
