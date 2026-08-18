# Dalamud 集成

eorzea 支持通过 Dalamud.Injector 在与游戏相同的 Wine prefix 内加载 Dalamud 插件框架。设计原则是**版本门控 + 安全降级**：任何环节不满足条件都退回直接启动游戏，绝不带着版本不匹配的 Dalamud 强行启动。

配置字段见 [`config.md`](config.md) 的 `[dalamud]` 一节；加载机制的深度调研见 [`notes/dalamud_integration.md`](notes/dalamud_integration.md)。

## 目录布局

默认安装根目录 `~/.xiv-launcher-rs/dalamud`（可用 `dalamud.install_root` 覆盖）：

```text
~/.xiv-launcher-rs/dalamud/
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
| `OutOfDate` | 已安装但版本不匹配（或远端元数据不可用） | 降级直接启动 |
| `Unsupported` | release 尚未支持当前游戏版本 | 降级直接启动，等待 Dalamud 发版 |
| `RuntimeMissing` | release 要求 .NET runtime 但未安装 | 自动下载后走 Injector |
| `AssetsMissing` | assets 缺失 | 自动下载后走 Injector |

游戏更新后 Dalamud release 通常要一两天才跟进，这期间应保持禁用或接受降级。

## 启动时的自动准备流程

启用 Dalamud 的启动会按顺序惰性补齐组件，**任一步失败都 warn 后降级为直接启动**：

1. 获取远端元数据；不可用 → 降级。
2. 版本门控：`SupportedGameVer != 本地游戏版本` → 降级。
3. **Hooks（release 本体）**：本地无有效安装（缺关键文件或 MD5 校验失败）→ 从元数据 `downloadUrl` 下载 `.7z`，用 `7zz/7z/7za` 解压，校验关键文件与逐文件 MD5，原子 rename 到 `Hooks/<AssemblyVersion>/` 并写 `version.json`。
4. **runtime**：`RuntimeRequired` 或 `dalamud.manage_runtime = true` 时，从华为 NuGet 镜像（失败回退官方 NuGet）下载 `Microsoft.{NETCore,WindowsDesktop}.App` 的 win-x64 runtime nupkg，只提取需要的目录，组装后原子替换到 `runtime/`。注入前会把 `DALAMUD_RUNTIME` / `DOTNET_ROOT` 指向它（经 `winepath` 转 Windows 路径），不依赖 wine-mono。
5. **assets**：从 `https://aonyx.ffxiv.wang/Dalamud/Asset/Meta` 取元数据，逐文件 SHA1 校验、缺失才下载；Noto 字体有多个 CTAN 镜像 fallback。完成后写 `asset.ver` 并清理旧版本目录（保留 `dev`）。

## 通过 Injector 启动（runner）

所有传给 Injector 的路径先经 `winepath --windows` 转成 `Z:\...` 形式，然后：

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
eoz config set dalamud.enabled true   # 启用
eoz launch                            # 版本匹配时自动安装并加载
eoz launch --no-dalamud               # 本次禁用（游戏刚更新、release 未跟进时）
eoz config set dalamud.no_plugins true  # 崩溃排查：safe mode
```
