# 实现记忆

Last reviewed 2026-11-27

- 页面转场只捕获右侧内容区域，快照通过根视图绘制以包含背景。
- 页面切换模糊开关的新用户默认值为关闭，已有 `NSUserDefaults` 值优先保留。
- 模糊使用共享 GPU `CIContext`、复用的 `CIAffineClamp` 和 `CIGaussianBlur`，`CADisplayLink` 限制为 60Hz。
- `transitionGeneration` 和控制器引用负责处理快速点击、反向切换、旋转、后台和动效设置变化。
- 主题使用 `NCLRThemeMode` 保存跟随系统、深色或浅色模式，旧版 `NCLRThemeDark` 值会兼容迁移。
- `ios/build.sh` 必须链接 Core Image。完整链接检查可使用 `.local/horizon/target/aarch64-apple-ios/fast/libhorizon_ios.a`。
