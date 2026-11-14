# Wine 开场动画解码故障

## 最终结论

本例由两个环节共同导致：

1. 系统的 nixpkgs `wineWow64Packages.staging` 未启用 GStreamer，Wine 的 WMA 解码器因此无法加载。
2. 切换到启用 GStreamer 的同版本 Wine 后，已有 prefix 没有同步新增组件；仅换 Wine 构建仍然卡住。

有效修复是：**使用启用 GStreamer 的 Wine，并用该 Wine 对现有 prefix 执行 `wineboot --update`**。用户在恢复开场动画设置后执行更新，确认问题解决；检查确认 prefix 的 `system32/winegstreamer.dll` 已存在。无需删除重建 prefix，也不需要以禁用动画作为永久修复。

适用证据是本机 Wine 11.21 与调查所用 nixpkgs 的实现，不应据此断言所有 Wine 构建或所有动画故障都相同。未在修复后采集完整媒体调用栈，也不据此承诺全部过场动画的解码兼容性。

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

触发故障的系统配置 `/home/azurice/flakes/modules/gaming/default.nix` 使用 `pkgs.wineWow64Packages.staging`。

调查所用 nixpkgs 源码：

- `pkgs/top-level/wine-packages.nix`：`staging = self.base.override { wineRelease = "staging"; }`；`base` 开启 FFmpeg，但没有开启 GStreamer。`stagingFull` 来自 `full`，后者设置 `gstreamerSupport = true`。
- `pkgs/applications/emulators/wine/default.nix`：`gstreamerSupport` 默认 `false`。
- `pkgs/applications/emulators/wine/base.nix`：开启后添加 GStreamer、base/good/ugly/bad 插件和 gst-libav，并包装 Wine 设置 `WINELOADER` 与 `GST_PLUGIN_SYSTEM_PATH_1_0`。

Wine 11.21 源码直接证实此解码路径需要 GStreamer：

- <https://raw.githubusercontent.com/wine-mirror/wine/wine-11.21/dlls/wmadmod/Makefile.in> 的 `IMPORTS` 包含 `winegstreamer`。
- <https://raw.githubusercontent.com/wine-mirror/wine/wine-11.21/dlls/wmadmod/wmadmod.c> 中 WMA 解码器的 `CreateInstance` 直接调用 `winegstreamer_create_wma_decoder()`。
- <https://raw.githubusercontent.com/wine-mirror/wine/wine-11.21/dlls/winegstreamer/Makefile.in> 同时定义 Windows 模块 `winegstreamer.dll` 和 Unix 库 `winegstreamer.so`。

因此开启 FFmpeg 不代表所有 WMA/DirectShow 解码路径都可用。日志和配置对照支持开场动画的音频解码失败；尚未通过调用栈证明游戏为何在失败后持续等待，也没有验证全部动画的编解码格式。

## 修复步骤

最小构建调整是在系统配置中将 Wine 包改为：

```nix
(pkgs.wineWow64Packages.staging.override {
  gstreamerSupport = true;
})
```

或使用包含更多可选功能的 `pkgs.wineWow64Packages.stagingFull`。不建议跨版本手工复制 DLL，也不能仅在运行时加 GStreamer 到 PATH，缺少的桥接模块必须在 Wine 构建时提供。

应用系统配置后，必须确认游戏实际使用新 Wine，而不是只确认系统 PATH 已更新。可检查 `/proc/<游戏PID>/exe`、进程环境中的 `WINELOADER` 和 `GST_PLUGIN_SYSTEM_PATH_1_0`，以及该 Wine 安装目录中的 `winegstreamer.dll`、`winegstreamer.so`。日志中的 `WINEDEBUG=+loaddll` 可用于辅助检查；不要公开包含登录票据的完整启动日志。

本例新 Wine 为 `/nix/store/l2xn2hf8hg1a7hla9wf12f2hxrxfmhzs-wine-wow64-staging-11.21`，安装目录具备两个架构的 `winegstreamer.dll` 与 `x86_64-unix/winegstreamer.so`，游戏进程也继承了包装器设置的 `WINELOADER` 和完整插件路径。但首次切换后仍卡住，现场未见 GStreamer/插件库映射，旧 prefix 的 `system32/winegstreamer.dll` 仍不存在。**新构建包含组件，不等于现有 prefix 已同步。**

正常退出使用该 prefix 的游戏和其他 Wine 程序，按需要备份 prefix 后，用**同一新 Wine 的包装入口**更新；不要用旧构建或隐藏的 `.wine` 绕过插件环境设置。例如本机验证所用命令：

```bash
WINEPREFIX="$HOME/.eorzea/prefix" \
/nix/store/l2xn2hf8hg1a7hla9wf12f2hxrxfmhzs-wine-wow64-staging-11.21/bin/wine \
wineboot --update
```

其他机器应将 Wine 路径替换为实际选定的构建，不要照抄本机 store 路径。若 PATH 中的 `wine` 就是该新构建的包装入口，也可使用：

```bash
WINEPREFIX="$HOME/.eorzea/prefix" wine wineboot --update
```

更新会修改已有 prefix 的内置组件和注册信息，不是清空重建。它可能影响已部署的 DXVK；若后续出现图形错误，应检查并重新部署 DXVK，而不是直接删除 prefix。不要在旧 Wine 游戏进程运行时进行更新或混用不同构建的 wineserver。

验证要点：

- prefix 的 `drive_c/windows/system32/winegstreamer.dll` 存在；仅存在文件还不等于成功解码。
- 使用相同游戏和配置重新启动，不再出现该模块缺失错误，并实际验证开场流程不再卡住。
- 本例用户在执行更新后确认恢复，prefix 中桥接 DLL 也已补齐；修复后 `CutsceneMovieOpening=1`，但该值本身不能区分游戏播放完成后的写回与人为跳过，应以实际播放观察为准。

## 启动器的待完善项

`WineTool::ensure_prefix()` 对现有 win64 prefix 直接返回成功，没有跟踪 Wine 构建身份或主动同步同版本不同特性构建。本例已验证手工更新可解决切换后的缺口；启动器仍需安全的构建身份检测、prefix 更新和更新后的 DXVK 检查，不能每次启动无条件更新或重建 prefix。只比较 `wine --version` 不足以区分本例两个同为 11.21、可选组件不同的构建。

`CutsceneMovieOpening=1` 是已验证的临时绕过，仅跳过该动画，不代表修复了其他视频播放能力。未经用户同意不应静默关闭动画。

## SDO 清理

按用户要求，依据复制清单和 SHA256 将之前额外补入的 113 个未变化文件移至游戏根目录的 `.eorzea/sdo-removed-20261114-030625/`，没有覆盖或删除用户新改动。清理动作保留了修改版入口和原版备份；最终检查时 `sdo/` 中仅见修改版 `sdologinentry64.dll`。本次媒体故障不能用于推断原版或 SDK 在所有登录路径下是否必需；登录组件需求仍应独立验证，见 [`sdo-installation.md`](sdo-installation.md)。
