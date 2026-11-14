# Wine 开场动画解码故障

## 结论与证据

本机新安装在标题界面选择开始游戏后卡住；关闭 Dalamud 后仍可复现。正常旧安装的 `FFXIV.cfg` 中 `CutsceneMovieOpening=1`，新安装为 `0`；仅将新安装该值设为 `1` 后用户确认恢复。

开启 `WINEDEBUG=+loaddll` 的日志 `~/.eorzea/logs/game-1794570263.log` 明确记录：

```text
Library winegstreamer.dll (which is needed by L"C:\\windows\\system32\\wmadmod.dll") not found
couldn't load in-process dll L"C:\\windows\\system32\\wmadmod.dll"
no class object {2eeb4adf-4578-4d10-bca7-bb955f56320a} could be created
```

同次日志确认修改版入口及原版 `.sdo.dll` 均加载成功，大厅 TCP 已建立并有双向数据。补齐其他 SDO 文件未改善卡住。

实际 Wine 是 `/nix/store/9vq95bhpxfb5n593x719c5m84g7v2b7w-wine-wow64-staging-11.21`。该包中有 `wmadmod.dll`，没有 `winegstreamer.dll` 或对应 Unix 库；其 derivation 的构建输入也没有 GStreamer。因此不是只在 prefix 里丢了文件，不能靠重建 prefix 或复制一个 Windows DLL 修复。

## 为什么选择 staging 会缺解码器

本机 `/home/azurice/flakes/modules/gaming/default.nix` 选择 `pkgs.wineWow64Packages.staging`。

调查所用 nixpkgs 源码：

- `pkgs/top-level/wine-packages.nix`：`staging = self.base.override { wineRelease = "staging"; }`；`base` 开启 FFmpeg，但没有开启 GStreamer。`stagingFull` 来自 `full`，后者设置 `gstreamerSupport = true`。
- `pkgs/applications/emulators/wine/default.nix`：`gstreamerSupport` 默认 `false`。
- `pkgs/applications/emulators/wine/base.nix`：开启后添加 GStreamer、base/good/ugly/bad 插件和 gst-libav，并包装 Wine 设置 `WINELOADER` 与 `GST_PLUGIN_SYSTEM_PATH_1_0`。

Wine 11.21 源码直接证实此解码路径需要 GStreamer：

- <https://raw.githubusercontent.com/wine-mirror/wine/wine-11.21/dlls/wmadmod/Makefile.in> 的 `IMPORTS` 包含 `winegstreamer`。
- <https://raw.githubusercontent.com/wine-mirror/wine/wine-11.21/dlls/wmadmod/wmadmod.c> 中 WMA 解码器的 `CreateInstance` 直接调用 `winegstreamer_create_wma_decoder()`。
- <https://raw.githubusercontent.com/wine-mirror/wine/wine-11.21/dlls/winegstreamer/Makefile.in> 同时定义 Windows 模块 `winegstreamer.dll` 和 Unix 库 `winegstreamer.so`。

因此开启 FFmpeg 不代表所有 WMA/DirectShow 解码路径都可用。日志和配置对照支持开场动画的音频解码失败；尚未通过调用栈证明游戏为何在失败后持续等待，也没有验证全部动画的编解码格式。

## 修复建议与验证边界

最小构建调整是在系统配置中将 Wine 包改为：

```nix
(pkgs.wineWow64Packages.staging.override {
  gstreamerSupport = true;
})
```

或使用包含更多可选功能的 `pkgs.wineWow64Packages.stagingFull`。不建议跨版本手工复制 DLL，也不能仅在运行时加 GStreamer 到 PATH，缺少的桥接模块必须在 Wine 构建时提供。

已用 `nix eval` 验证当前 nixpkgs 接受上述最小 override，并加入全部预期 GStreamer 构建输入；尚未构建、切换系统或验证实际播放。完整验证应保留可恢复配置，使用新 Wine 及其包装入口，在隔离测试 prefix/配置中重新启用开场动画，检查不再报模块缺失且可播放并进入角色列表。旧 Wine 进程应先正常退出，避免混用不同构建的 wineserver；不需要删除现有 prefix。

`CutsceneMovieOpening=1` 是已验证的临时绕过，仅跳过该动画，不代表修复了其他视频播放能力。未经用户同意不应静默关闭动画。

## SDO 清理

按用户要求，依据复制清单和 SHA256 将之前额外补入的 113 个未变化文件移至游戏根目录的 `.eorzea/sdo-removed-20261114-030625/`，没有覆盖或删除用户新改动。`sdo/` 仅保留修改版 `sdologinentry64.dll` 与原版 `sdologinentry64.sdo.dll`。清理后能否仅靠两者正常登录仍需用户实测，不从动画绕过结果推断。
