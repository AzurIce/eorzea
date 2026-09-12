# Dalamud 集成

eorzea 支持通过 Dalamud.Injector 加载 Dalamud 插件框架：Linux/macOS 在与游戏相同的 Wine prefix 内运行 Injector，Windows 原生运行。

设计原则是**版本门控 + 启用即强制**：`SupportedGameVer == 本地游戏版本` 才允许加载；启用 Dalamud（配置 `[dalamud].enabled = true` 或 `--dalamud` / GUI「本次启动加载」开关）后，任何环节没准备好都**直接报错、不启动游戏**，不会静默启动一个没有插件的游戏。想在没有 Dalamud 的情况下进游戏必须显式选择：`--no-dalamud`、GUI 关闭本次开关，或改配置。

```bash
eoz launch --dalamud        # 强制本次加载；未就绪 → 报错退出（不启动游戏）
eoz launch --no-dalamud     # 强制本次不加载（游戏照常启动）
```

配置字段见 [`config.md`](config.md) 的 `[dalamud]` 一节；加载机制的深度调研见 [`notes/dalamud_integration.md`](notes/dalamud_integration.md)。

## 目录布局

默认安装根目录 `~/.eorzea/dalamud`（可用 `dalamud.install_root` 覆盖）：

```text
~/.eorzea/dalamud/
├── Hooks/<AssemblyVersion>/    # Dalamud release（Dalamud.Injector.exe、Dalamud.dll、hashes.json、version.json）
│   └── dev/                    # 开发版（状态检测时跳过）
├── runtime/                    # 托管的 Windows x64 .NET runtime
│   ├── host/fxr/<ver>/hostfxr.dll
│   └── shared/Microsoft.{NETCore,WindowsDesktop}.App/<ver>/
├── dalamudAssets/<version>/    # UI 资源（asset.ver 记录当前版本）
├── dalamudConfig.json          # Dalamud 配置
├── logs/dalamud.log            # Dalamud 日志
└── installedPlugins/           # 插件目录
```

## 版本门控与状态

`eoz dalamud status`（以及启动前的检查）汇总三方面的信息：本地游戏版本（`game/ffxivgame.ver`）、远端 release 元数据、本机安装情况。

远端元数据来自 `https://aonyx.ffxiv.wang/Dalamud/Release/VersionInfo?track=<track>`，包含 `AssemblyVersion`、`SupportedGameVer`、`RuntimeVersion`、`RuntimeRequired`、`Hash`（`hashes.json` 的 MD5）、`downloadUrl` 等。

版本门控是**硬等值**：`SupportedGameVer == 本地游戏版本` 才允许加载。

| InstallState | 含义 | 启动时行为 |
|--------------|------|-----------|
| `Ready` | 已安装且版本匹配 | 走 Injector |
| `Missing` | 未安装，但 release 匹配游戏版本 | 自动下载安装后走 Injector |
| `OutOfDate` | 已安装但版本不匹配 | **报错，不启动游戏** |
| `Unsupported` | release 尚未支持当前游戏版本 | **报错，不启动游戏**（等待 Dalamud 发版，或显式 `--no-dalamud` 进游戏） |
| `RuntimeMissing` | release 要求 .NET runtime 但未安装 | 自动下载后走 Injector |
| `AssetsMissing` | assets 缺失 | 自动下载后走 Injector |

游戏更新后 Dalamud release 通常要一两天才跟进，这期间要么用 `--no-dalamud` 进游戏，要么在设置里关掉 Dalamud。

## 启动时的自动准备流程

启用 Dalamud 的启动会**先**准备 Dalamud、再登录（避免失败白白消耗一次性 SSO ticket）；任一步失败都报错并终止启动（`LauncherError::DalamudNotReady`），不会静默降级：

1. 获取远端元数据：请求失败按重试（8s/次超时、4xx 不重试；本机已有安装时只试 1 次，避免离线启动白等），仍失败则退回本机 `Hooks/<版本>/version.json`（安装时原样保存的服务端元数据）；连本地记录都没有 → **报错**。这样一次网络抖动不会把已装好且匹配的 Dalamud 拦下来，`DalamudStatus.remote_from_cache` 会标出「元数据来自本地记录」。
2. 版本门控：`SupportedGameVer != 本地游戏版本` → **报错**（提示等待发版，并说明可用 `--no-dalamud` 进游戏）。
3. **Hooks（release 本体）**：本地 `Hooks/<AssemblyVersion>` 无有效安装（缺关键文件或 MD5 校验失败）→ 从元数据 `downloadUrl` 下载归档，用内置的**纯 Rust 解压**（按文件头识别 7z/zip，不依赖外部 `7z` 命令）展开，校验关键文件与逐文件 MD5，原子 rename 到 `Hooks/<AssemblyVersion>/`、写 `version.json` 并清理旧版本目录（保留 `dev`）。已安装且校验通过时跳过下载；失败 → **报错**。
4. **runtime**：`RuntimeRequired` 或 `dalamud.manage_runtime = true` 时，从华为 NuGet 镜像（失败回退官方 NuGet）下载 `Microsoft.{NETCore,WindowsDesktop}.App` 的 win-x64 runtime nupkg，只提取需要的目录，组装后原子替换到 `runtime/`；本机已是目标版本时直接复用（不联网）；失败 → **报错**。注入前会把 `DALAMUD_RUNTIME` / `DOTNET_ROOT` 指向它（非 Windows 经 `winepath` 转 Windows 路径，Windows 直接传原生路径），不依赖 wine-mono。
5. **assets**：从 `https://aonyx.ffxiv.wang/Dalamud/Asset/Meta` 取元数据，逐文件 SHA1 校验、缺失才下载；Noto 字体有多个 CTAN 镜像 fallback。完成后写 `asset.ver` 并清理旧版本目录（保留 `dev`）。元数据拉取失败时退回本地 `dalamudAssets/<asset.ver>`；连本地都没有 → **报错**。

因此**本地三件套齐全时，完全离线也能照常加载 Dalamud**（`eoz dalamud install` 同样如此，见下）；而真正缺东西时会明确报错告诉你缺什么，而不是悄悄不带插件启动。

## 通过 Injector 启动（runner）

非 Windows 上所有传给 Injector 的路径先经 `winepath --windows` 转成 `Z:\...` 形式；Windows 上 Injector 与游戏都是原生进程，路径直接传递（无 `WINEPREFIX`/`XL_WINEON*` 环境变量）。命令行形态一致：

```text
wine64 Dalamud.Injector.exe launch --mode=<entrypoint|inject>
    --game=... --dalamud-working-directory=... --dalamud-configuration-path=...
    --logpath=... --dalamud-plugin-directory=... --dalamud-asset-directory=...
    --dalamud-client-language=4 [--dalamud-delay-initialize=<ms>]
    [--without-dalamud] [--no-plugin] [--no-3rd-plugin]
    -- <游戏启动参数...>
```

- `load_method`：`entrypoint`（默认，入口点改写）、`dllinject`（`--mode=inject`）、`aclonly`（`--mode=inject` + `--without-dalamud`，只启动游戏不加载，用于排查 Dalamud 是否导致问题）。
- Injector 启动游戏后会在 stdout 输出一行 JSON `{"pid":…, "handle":…}`，launcher 据此拿到游戏进程；总超时 30 秒。
- 报错 `Injector exited without reporting a result`：Injector 进程退出/关闭管道前没输出合法 JSON，错误信息附带其 stderr 尾部（最近 50 行），常见原因是 Dalamud.Boot.dll 依赖加载失败（如 prefix 里 DXVK 缺失导致 `dxgi.dll not found`，见 [`wine.md`](wine.md) 故障排查）。

## 常用操作

```bash
eoz dalamud status                    # 查看版本兼容性/缺失组件
eoz dalamud install                   # 预下载安装 release + runtime + assets（失败即报错退出）
eoz config set dalamud.enabled true   # 启用（启用后 launch 未就绪会报错，不再静默降级）
eoz launch                            # 版本匹配时自动安装并加载
eoz launch --no-dalamud               # 本次不加载（游戏照常启动；游戏刚更新、release 未跟进时用）
eoz config set dalamud.no_plugins true  # 崩溃排查：safe mode
```

`dalamud install` 与 `launch` 的惰性准备走同一套函数与版本门控（`Launcher::prepare_dalamud`
→ `updater::ensure_release` → `ensure_runtime` → `ensure_assets`），区别只在时机：`install`
适合提前把组件备齐（离线/弱网时不必赌启动那一刻的下载），`launch` 则在启动前顺带补齐。
两者的失败都会显式报错：`install` 直接退出；`launch` 终止启动并提示用 `--no-dalamud` 跳过。
已安装且 `hashes.json` 校验通过时两者都跳过下载。
