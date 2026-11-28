# 用户界面功能

Last reviewed 2026-11-27

## iOS 启动器页面转场

- 入口文件是 `ios/main_l1.m`。
- `NCLRRootController` 管理侧边栏导航、页面控制器和内容区域。
- `NCLRPageTransitionCoordinator` 负责右侧内容快照、Core Image 动态模糊和清理。
- 页面索引决定上下方向。首次页面安装和当前页面重复点击不播放页面转场。
- 设置页的「页面切换模糊效果」使用 `NSUserDefaults` 保存，默认关闭。
- 设置页外观选项提供「跟随系统」「深色」「浅色」，新安装默认跟随系统。
- 设置页「资源下载」提供 1 到 8 路并发，默认 4 路，保存到 `NCLRResourceDownloadConcurrency`。
