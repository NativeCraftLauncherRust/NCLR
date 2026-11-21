# NCLR L1.1 · 单 App：点开始后才启动引擎

作者：**ZRXMAXLNX**、**可心**  
平台：iOS 15+ / arm64 / 横屏  
版本：**L1.1** 开发者测试版

---

## 1. 单 App 方案

L1.1 现在是一个 IPA / 一个 App：

- 启动时由启动器自己持有 `UIApplicationMain`；
- 引擎**不会**在启动器显示时预启动；
- 点“启动游戏”后，才调用 `horizon_ios_run`；
- 补丁后的 winit 复用已经运行的 UIApplication，在同一个进程里接管窗口和事件循环；
- 不退出、不重启、不跳转外部 App。

这解决了之前的两层问题：

- 不再需要退出重开才能进游戏；
- 引擎不是后台预启动，而是点击之后才开始初始化。

iOS 的 winit 原本强制自己调用 `UIApplicationMain`；L1.1 给 winit 增加了 late-start 路径：
`UIApplication` 已存在时，重放 launch transition、发送 `Resumed`，再用主 run loop 驱动引擎，不再第二次
`UIApplicationMain`。

---

## 2. 启动器

- 主页 / 版本 / 模组 / 账号 / 设置；
- 默认 **正版用户（开发者默认）**，不强制登录；
- 内置 `26.3` 完整资源树，引擎和启动器同一个容器；
- 设置 → **编辑游戏内按键映射**，或进入游戏后点左上角 **键位**：
  - 上方可收起的“按键设置”小菜单；
  - 展开后是标注为 **按键大小** 的滑条（0.50x–2.00x）；
  - 中间是 **游戏内布局预览**，按钮可拖动改变位置；
  - **+ 添加按键** 可以增加新按键；
  - 底部 **保存** / **退出**；
  - 保存到：

```text
Documents/keymap.json
```

JSON 包含：

```json
{
  "buttonSize": 1.0,
  "bindings": { "forward": "W", "jump": "Space" },
  "layout": {
    "forward": { "x": 0.13, "y": 0.76, "size": 0.16 }
  }
}
```

进入游戏后左上角会出现一个小 **键位** 按钮，点开就是同一套编辑器，直接盖在游戏画面上。
保存后需要重新进一次游戏（退出到桌面再打开 NCLR）才会被引擎重新读取。

**当前边界**：引擎启动时会读取 `keymap.json`，把 `buttonSize` 和已知按键
（`forward/back/left/right` 合并到移动摇杆、`jump`、`sneak`、`sprint`、`attack`、
`use`、`inventory`、`chat`、`pause`、`settings`、`debug`、`perspective`、`offhand`、`shield`）
应用到 Horizon `TouchControls`。  
`+ 添加按键` 新增的 id 如果属于上面这些内置控件，会真正出现在游戏中；其它自定义 id 会保存进 JSON，
但这一版不会生成新的引擎控件。

---

## 3. 文件

| 文件 | 大小 | SHA-256 |
|---|---:|---|
| `NCLR-L1.1-signed.ipa` | 105,375,661 B | `1560287C…5D4A28F` |
| `NCLR-L1.1-unsigned.ipa` | 105,246,288 B | `2C7DF82D…51439C` |

- 已签名包：Bundle ID `app.badger212.aries5602`，只绑定 UDID 以 `0E7A401E` 结尾的设备；
- 未签名包：Bundle ID `dev.zrxmaxlnx.nclr`；
- 版本：`L1`（CFBundleShortVersionString `1.1`，CFBundleVersion `202611211205`）；
- 包内引擎：Horizon 26.3 / route3 iOS 静态库（含 Metal 修复）；
- 包内资源：完整 `horizon-client/26.3`。

---

## 4. 这次闪退的原因与修复

归档里的 `latest.log`：

```text
Horizon renderer: Apple A18 Pro GPU (Metal, IntegratedGpu)
world renderer ready after 10675 ms
panicked at .../game/runtime/resource_tables.rs:28:14:
world resources joined before entry
```

不是 Metal/黑屏问题，而是 **世界资源后台加载还没完成，标题界面就读取 `Resources::rest()`，
旧代码直接 `expect` 崩溃**。  
L1 先加了 `ios-resource-wait.patch`，让读取方等待；但 `latest(11).log` 显示 loader 已打印
`resources loaded in 9.8s`，`rest` 仍未发布，标题一直 `waiting for world resources`。  \nL1.1 又修了 late-start 的 `Resumed` 时序：改用 `CFRunLoopAfterWaiting` 观察器，并把 redraw 请求挂起到下一个事件循环周期；同时补全资源包内的 `fluid_state_definitions`，并做安全区、紧凑窗口和 iPhone/iPad 尺寸适配。  
所以又加了 `ios-rest-join.patch`：**在把资源交给标题界面前，先 join 世界资源 loader**，
让加载画面多停留一会儿，避免标题空转。启动时可能晚几秒进标题，但不会再停在
`waiting for world resources`。  \nL1.1 又修了 late-start 的 `Resumed` 时序：改用 `CFRunLoopAfterWaiting` 观察器，并把 redraw 请求挂起到下一个事件循环周期；同时补全资源包内的 `fluid_state_definitions`，并做安全区、紧凑窗口和 iPhone/iPad 尺寸适配。

---

## 5. 安装

1. 删除旧的 HorizonEngine / 旧 NCLR / 旧独立游戏端；
2. 用 Sideloadly 安装 **NCLR-L1.1-signed.ipa**（已签名设备）  
   或 **NCLR-L1.1-unsigned.ipa**（自己的 Apple ID 重签）；
3. 打开 NCLR，应直接看到启动器；
4. 点“启动游戏”，启动器隐藏，进入引擎；
5. 下次从主屏幕打开 NCLR，仍然先进入启动器。

包压缩后约 105 MB，安装后约 430 MB。

---

## 6. 当前边界

- 启动器先显示；点“启动游戏”后才调用引擎 late-start，不再后台预启动。
- 引擎在启动时读取 keymap；游戏内保存后需要重新进一次游戏才生效。新增 id 目前不会生成新的引擎触屏按钮。
- 默认正版仅用于 L1.1 开发者测试，不是正式分发/绕过购买。
- 未在真机上复验本包；若进引擎黑屏，请发 `NCLR` 的 `.ips` 或 `idevicesyslog`。
- 不是 Mojang / Microsoft 官方产品。

---

## 7. 构建来源

- 入口：`ios/main_l1.m`
- Info：`ios/Info-launcher.plist`
- 构建：`ios/build.sh`，Codemagic #16
- 资源：`resources/26.3`（构建后注入并重签）
- 作者：**ZRXMAXLNX**、**可心**
