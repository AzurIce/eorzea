# Wine / Prefix / DXVK

本文档讲解 eorzea 在 Linux/macOS 下运行游戏的 Wine 层：Wine 解析、prefix 管理、DXVK、环境变量与日志。配置字段见 [`config.md`](config.md)，NixOS 特殊处理见 [`notes/nixos.md`](notes/nixos.md)。

## Wine 解析（`WineTool::resolve`）

由 `startup_type` 决定使用哪个 Wine：

| `startup_type` | 行为 |
|----------------|------|
| `auto`（默认） | 依次尝试：`custom_path` → XIVLauncher 托管（`~/.xlcore/beta/wine/bin/wine64`）→ 系统 wine（PATH 中 `wine64`，回退 `wine`）→ 自动下载 |
| `managed` | XIVLauncher 托管路径 → 本项目已下载的（`~/.xiv-launcher-rs/tools/wine/bin/wine64`）→ 下载 |
| `custom` | 使用 `custom_path`（必须是 wine 可执行文件，或含 `wine64`/`wine` 的目录/`bin` 目录） |
| `system` | 只用 PATH 中的系统 wine，找不到则报错 |

自动下载的 Wine 来自 CN 镜像（Linux 为 `wine-xiv-staging-fsync`，macOS 为 xom 构建），解压到 `~/.xiv-launcher-rs/tools/wine/`。

解析完成后会执行一次 `wine64 --version` 探针（probe），提前暴露不可执行/版本不对的问题。

## Prefix 管理

- 默认 prefix 为 `~/.xiv-launcher-rs/prefix`，可用配置项 `prefix` 覆盖。
- 每次启动前 `ensure_prefix()`：
  - prefix 不存在 → 用 `WINEARCH=win64` 执行 `wineboot --init` 创建（设 `WINEDLLOVERRIDES=mscoree=n` 屏蔽 wine-mono 弹窗，Dalamud 使用自己托管的 .NET runtime）；
  - 已存在 → 读取 `system.reg` 头部的 `#arch=` 行判断架构，是 `win32` 或无法识别时**删除重建**为 64 位。
- FFXIV 与 Dalamud 都要求 64 位环境，因此架构检查是强制的。

## DXVK

FFXIV 是 DX11 游戏，wined3d 渲染错误且性能差，DXVK 默认开启（`dxvk.enabled = true`）。

- 首次启动时从 CN 镜像下载 `dxvk-async 1.10.1`，解压缓存于 `~/.xiv-launcher-rs/tools/dxvk/`，并把 x64 DLL 复制到 prefix 的 `system32`、x32 复制到 `syswow64`。
- 安装判定：`system32/d3d11.dll` 存在 **且** prefix 根目录有 `.dxvk-installed` 标记文件。只看 DLL 是不够的——prefix 被 wineboot 重建后 builtin d3d11 会占回原位，标记文件可以区分这种情况并触发重装。
- 与 DLL override 的关系：`dxvk.enabled = true` 时 `WINEDLLOVERRIDES` 中 `d3d9,d3d11,d3d10core,dxgi=n`（native，指 DXVK）；`= false` 时为 `=b`（builtin，回退 wined3d）。macOS 只覆盖 `d3d11` 且 `dxgi=n,b`。

## 环境变量（`build_launch_env`）

启动游戏进程时设置（对齐 C# `CompatibilityTools.RunInPrefix`）：

| 变量 | 值 / 来源 |
|------|-----------|
| `WINEPREFIX` | 解析后的 prefix 路径 |
| `WINEARCH` | `win64` |
| `WINEDLLOVERRIDES` | `msquic=,mscoree=n,b;d3d9,d3d11,d3d10core,dxgi=n`（DXVK 开；macOS 略有不同） |
| `WINEESYNC` / `WINEFSYNC` / `WINEMSYNC` | `esync` / `fsync` / `msync` 开启时为 `1`（msync 仅 macOS） |
| `WINEDEBUG` | `debug_vars` 配置（如 `+seh`、`-all`） |
| `DXVK_STATE_CACHE_PATH` / `DXVK_CONFIG_FILE` | `C:\` / `C:\ffxiv_dx11.conf`（DXVK 开启时） |
| `DXVK_HUD` / `DXVK_FRAME_RATE` | `dxvk.hud` / `dxvk.frame_limit` 配置 |
| `LD_PRELOAD` | `gamemode` 开启时追加 `libgamemodeauto.so.0` |
| `XL_WINEONLINUX` / `XL_WINEONMAC` | `true`（平台标记，供 Dalamud 等识别） |

`config.toml` 的 `[env]` 表（`env.FOO = "bar"`）在最后应用，可覆盖以上任意项。启用 Dalamud 且托管了 .NET runtime 时，还会设置 `DALAMUD_RUNTIME` 与 `DOTNET_ROOT`（经 `winepath` 转换的 Windows 路径）。

## 日志

- 每次启动的 wine/游戏输出重定向到 `~/.xiv-launcher-rs/logs/game-{unix_ts}.log`，不污染终端；CLI 启动成功后会打印该路径。
- 启动器自身的结构化日志走 `tracing`：CLI 默认显示 `eorzea_*` 的 info 级，可用 `RUST_LOG=debug eoz launch` 看全部细节（wine 解析、prefix 检查、DXVK 复制等）。

## 故障排查

- **游戏内报 5003「帐号认证发生了错误」**：游戏路径含非 ASCII 字符（启动前有预检，会直接拒绝）；或 `sdo/sdologin/sdologinentry64.dll` 不是 ottercorp 修改版（启动时会自动下载替换，原版备份为 `sdologinentry64.sdo.dll`）。
- **`import_dll Library dxgi.dll not found` / Dalamud.Boot 加载失败**：prefix 里的 DXVK 被 wineboot 覆盖回了 builtin。删除 prefix 根部的残留后重跑启动即可（新版会通过 `.dxvk-installed` 标记自动检测并重装）。
- **prefix 每次启动都重建**（反复出现 `configuration in prefix is being updated`）：检查 `system.reg` 头部是否有 `#arch=win64`；此问题已在 `detect_prefix_arch` 改为扫描头部多行后修复。
- **系统 wine 不可用（NixOS 等）**：见 [`notes/nixos.md`](notes/nixos.md) 的系统 wine / nix-ld / steam-run 三条路径。
- **想看 wine 内部日志**：`eoz config set debug_vars "+seh"`（或 `-all` 之外的任意 channel），输出在游戏日志文件中。
