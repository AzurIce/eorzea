# eorzea 集成 Dalamud 调查报告

> 调查日期：2026-08-07  
> 主要参考：`XIVLauncher.Core` 的 `cn` 分支及其 `lib/FFXIVQuickLauncher` 子模块；同时检查了本机由该实现安装的国服 Dalamud 14.0.4.4 发行目录，以及 `eorzea` 当前的启动、Wine、配置和补丁下载代码。

## 摘要

Dalamud 不是由 Rust/Tauri 直接加载的普通库。XIVLauncher 的职责主要是：获取与当前游戏版本严格匹配的 Dalamud 发行包、配套资源和 Windows x64 .NET Runtime，然后以规定参数启动发行包中的 `Dalamud.Injector.exe`。Injector 创建或接管 FFXIV 进程，将原生引导 DLL 放入游戏进程；引导 DLL 再通过 `nethost`/`hostfxr` 在游戏进程内启动 CoreCLR，最后加载托管的 `Dalamud.dll`。

Linux/macOS 上也不是把 Linux 版 .NET 注入 Linux 进程。FFXIV、Injector、引导 DLL 和 .NET Runtime 都是 Windows x64 组件，并且都运行在同一个 Wine prefix 内；Unix runner 只是负责 Unix→Wine 路径转换、环境变量、执行 Injector，以及在需要时把 Wine PID 映射回 Unix PID。因此，最稳妥的 Rust 方案是复用官方/CN 发行包和 Injector 协议，不要在 MVP 中重写注入器或 CLR 宿主。

## 1. Dalamud 加载机制

### 1.1 组件与加载链

当前国服发行包可见的关键文件包括：

- `Dalamud.Injector.exe` / `Dalamud.Injector.dll`：命令行启动器和注入控制器。`launch` 子命令可创建游戏并加载 Dalamud；`inject` 子命令可注入已有 PID。
- `Dalamud.Boot.dll`：当前发行版的进程内原生引导/CLR host。旧版实现和资料常把这一层称为 `Dalamud.Loader`；其职责在当前版本主要落在 `Dalamud.Boot`。集成代码不应硬编码历史名称，而应把发行包视为整体。
- `nethost.dll`：Microsoft .NET 原生 hosting 入口，用于定位 `hostfxr`。
- `hostfxr.dll` 及 `shared/Microsoft.NETCore.App`、`shared/Microsoft.WindowsDesktop.App`：托管的 Windows x64 .NET Runtime。
- `Dalamud.runtimeconfig.json` / `Dalamud.deps.json`：指定目标框架、运行时与依赖解析信息。
- `Dalamud.dll`：最终在 FFXIV 进程中运行的托管框架主体。
- `ImGuiScene.dll`、`cimgui.dll`、`FFXIVClientStructs.dll` 等：渲染、游戏结构和其他运行依赖。
- 独立的 Dalamud assets：字体、UI/本地化等资源，由更新器单独确保。

概念链路如下：

```text
eorzea
  -> Dalamud.Injector.exe launch ... -- <FFXIV 登录参数>
     -> 创建 ffxiv_dx11.exe（并按 load method 安排早期加载或注入）
        -> Dalamud.Boot.dll（旧称/概念称 Dalamud.Loader）进入游戏进程
           -> nethost.dll -> hostfxr.dll
              -> 按 Dalamud.runtimeconfig.json 初始化 CoreCLR
                 -> 加载 Dalamud.dll 及插件
```

检查当前发行二进制可看到 `Dalamud.Boot.dll` 中存在 `hostfxr_initialize_for_runtime_config`、`hostfxr_get_runtime_delegate`、`hostfxr_close` 和 `InitializeClrAndGetEntryPoint` 等符号；Injector 中可见 `CreateRemoteThread`/`LoadLibrary`。这与“Injector 负责进程控制，Boot/Loader 在目标进程内宿主 CLR”的分工一致。具体内部 ABI 属于 Dalamud 发行物的实现细节，应通过调用 Injector 而不是由 Rust 复制。

### 1.2 Windows：进程创建、注入与 CLR hosting

`WindowsDalamudRunner` 不直接 P/Invoke 注入 Dalamud；它构造参数并启动 `Dalamud.Injector.exe`。Injector 支持两种实际加载模式：

- `entrypoint`：参考代码称为 OEP（Original Entry Point）rewrite。在新建游戏进程真正执行游戏入口前安排 Boot/Loader 初始化，属于启动期加载，通常能更早、较稳定地取得控制权。
- `inject`：参考代码称为 DLL inject；对目标进程执行传统远程 DLL 加载路径，发行二进制中也可观察到 `CreateRemoteThread`/`LoadLibrary` 相关符号。

另有 `ACLonly`，它仍通过 launcher 路径启动游戏，但传 `--without-dalamud`，只做兼容/ACL 修复而不加载 Dalamud；它在 Injector 参数层面映射为 `--mode=inject --without-dalamud`，不是第三种 CLR 加载技术。

Windows runner 的重要进程管理行为是：

1. 复制一个可继承的 launcher 进程句柄，以 `--handle-owner=<handle>` 交给 Injector。
2. 合并启动环境（包括 `DALAMUD_RUNTIME`），用 `CreateProcess` 启动 Injector，并通过可继承 pipe 捕获 stdout。
3. 最多等待 Injector 60 秒，要求退出码为 0。
4. 读取 Injector 输出的单行 JSON（`pid`、`handle`），恢复游戏 `Process`；handle 不可用时按 PID 回退。

进入目标进程后，Boot/Loader 是 Windows 原生 DLL；它通过 .NET native hosting API 初始化 CoreCLR，而不是依赖游戏本身已经安装 CLR。`DALAMUD_RUNTIME` 指向 launcher 管理的 runtime 根目录，使 `nethost`/`hostfxr` 能找到完全匹配的 Windows Runtime。

### 1.3 Unix/Linux：不是原生 Linux 注入，而是 Wine 内的 Windows 注入

`UnixDalamudRunner` 与 Windows runner 调用的是同一个 `Dalamud.Injector.exe` 和同一套 Windows Dalamud/runtime。差异在外围编排：

1. 用 `winepath --windows` 把游戏、runtime、日志、配置、插件、assets 和工作目录转换为 Wine 可识别的 `Z:\...` 或 prefix 内盘符路径。
2. 如果调用环境没有设置 `DALAMUD_RUNTIME`，把转换后的 Windows runtime 路径加入游戏/Injector 环境。
3. 在与 FFXIV 相同的 `WINEPREFIX` 中运行：

   ```text
   wine64 <unix-path>/Dalamud.Injector.exe launch
     --mode=entrypoint|inject
     --game="Z:\...\ffxiv_dx11.exe"
     --dalamud-*=<Wine 路径>
     -- <游戏参数>
   ```

   runner 自身路径可以作为宿主侧 Unix 路径传给 `wine64`，但传入 Windows 进程的各业务路径必须转换。
4. 读取 Injector stdout 中的 JSON。这里的 `pid` 是 Wine/Windows PID，不是 Unix PID。
5. 参考实现通过 `winedbg --command "info procmap"` 将 Wine PID 映射为 Unix PID，以便返回和管理真正的宿主进程。旧 Wine 需要 `unix-pid maps` 补丁；无法映射并不会否定注入本身成功，但会使 launcher 的监控、等待和终止能力失真。

Wine 内部仍执行 Windows `CreateProcess`、DLL 注入和 Windows CoreCLR hosting。不能改用宿主 Linux 的 `dotnet` 或 `libcoreclr.so`：它们的 ABI、路径语义和进程边界都不匹配 Windows 游戏进程。

Wine 还需要关键环境兼容项，尤其是 `WINEDLLOVERRIDES=msquic=,mscoree=n,b;...`。`mscoree=n,b` 允许使用所需的 CLR/Wine 行为；当前 `eorzea::build_launch_env()` 已覆盖这一项及 DXVK、esync/fsync、日志和自定义环境合并，可作为 Dalamud runner 的环境基础。

## 2. XIVLauncher 的集成组件

### 2.1 `DalamudLauncher.cs`：更新与启动之间的协调层

`DalamudLauncher` 本身既不下载文件，也不实现平台注入。它连接 `DalamudUpdater` 和 `IDalamudRunner`：

- `HoldForUpdate()` 等待 updater 到达 `Done`，处理 `NoIntegrity`，确认 Injector 存在，并再次核对游戏版本。
- `ReCheckVersion()` 读取安装目录的 `version.json`，要求 `SupportedGameVer` 与本地 FFXIV 版本完全相同；版本不匹配返回 `OutOfDate`，不冒险加载。
- `Run()` 建立插件目录并组装 `DalamudStartInfo`，再委托平台 runner。
- `Inject(pid)` 是已有进程注入的辅助入口；常规路径应优先使用 `launch`，因为 Injector 能协调游戏创建与早期加载。
- `CanRunDalamud()` 可从 release 元数据预判当前游戏版本是否受支持。

`DalamudStartInfo` 的来源包括：

- `WorkingDirectory`：当前版本发行目录（Injector 所在目录）。
- `ConfigurationPath`：通常为 `<config>/dalamudConfig.json`。
- `LoggingPath`、`PluginDirectory`、`AssetDirectory`。
- `Language`、`DelayInitializeMs`。
- `GameVersion`（当前 runner 参数未直接传递，但用于版本判断/诊断）。
- `TroubleshootingPackData`：UTF-8 后 Base64，以 `--dalamud-tspack-b64` 传递。
- `LauncherDirectory`。

### 2.2 `DalamudUpdater.cs`：release、runtime、assets 与完整性

Updater 维护的是三组相互关联但独立落盘的内容：

1. **Dalamud release**
   - 从 `.../Dalamud/Release/VersionInfo?track=release&bucket=<Control|Canary>` 获取 `DalamudVersionInfo`。
   - 元数据包含 `AssemblyVersion`、`SupportedGameVer`、`RuntimeVersion`、`RuntimeRequired`、`DownloadUrl`、`Hash` 等。
   - 可通过 `DalamudBetaKey`/`DalamudBetaKind` 选择 staging，并向服务器校验 key。
   - 安装到 `<addon>/Hooks/<AssemblyVersion>/`，runner 固定为该目录的 `Dalamud.Injector.exe`。
   - 下载包可能是 zip 或 7z；安装成功后写 `version.json`，并清理旧版本（保留当前与 `dev`）。

2. **Windows x64 .NET Runtime**
   - 当服务端声明 `RuntimeRequired` 或用户强制启用时确保指定 `RuntimeVersion`。
   - 从 NuGet/Huawei 镜像下载 `Microsoft.NETCore.App.Runtime.win-x64` 和 `Microsoft.WindowsDesktop.App.Runtime.win-x64`。
   - 拼装 `shared/Microsoft.NETCore.App/<ver>`、`shared/Microsoft.WindowsDesktop.App/<ver>` 与 `host/fxr/<ver>/hostfxr.dll`。
   - 通过版本文件和服务端 runtime hash manifest 校验。

3. **Dalamud assets**
   - 由 `AssetManager.EnsureAssets` 独立更新并返回实际 asset 版本目录。
   - 启动参数必须传这个最终目录，不能假设 assets 与 Hooks 同目录。

release 完整性检查至少要求 `Dalamud.Injector.exe`、`Dalamud.dll`、`ImGuiScene.dll` 可读，并按 `hashes.json` 对所有列出的文件做 MD5；远端 `Hash` 是 `hashes.json` 本身的 MD5。该机制是兼容上游协议的最低线，不应被误解为现代密码学签名。Rust 实现宜在兼容检查之外增加 HTTPS、下载长度限制、archive path traversal 防护和原子目录切换。

Updater 最多重试十次，并通过 `IDalamudLoadingOverlay` 报告 Dalamud、Runtime、Assets、Starting、Unavailable 等阶段。Tauri 中应将这些状态转成异步 command/event，而不是像 `HoldForUpdate()` 那样 busy-wait。

### 2.3 `DalamudInjectorArgs.cs` / `DalamudStartInfo.cs`：稳定的进程边界

常规启动命令的协议为：

```text
Dalamud.Injector.exe launch
  --mode=entrypoint|inject
  --game="<Windows 游戏路径>"
  --dalamud-working-directory="..."
  --dalamud-configuration-path="..."
  --logpath="..."
  --dalamud-plugin-directory="..."
  --dalamud-asset-directory="..."
  --dalamud-client-language=<int>
  --dalamud-delay-initialize=<ms>
  --dalamud-tspack-b64=<base64>
  --launcher-directory="..."
  [--handle-owner=<Windows handle>]
  [--without-dalamud]
  [--fake-arguments]
  [--no-plugin]
  [--no-3rd-plugin]
  -- <完整 FFXIV 参数>
```

这是 Rust 与 Dalamud 最适合保持的边界。Rust 应使用 `std::process::Command::args` 逐项传参；不要先拼接 shell 字符串，也不要用 `split_whitespace()` 拆解游戏参数，否则含空格、引号或特殊字符的值会损坏。参考 C# 用字符串拼接是历史实现细节，不值得复制。

### 2.4 `WindowsDalamudRunner.cs` 与 `UnixDalamudRunner.cs`

| 方面 | Windows | Unix/Linux/macOS |
|---|---|---|
| Injector 执行环境 | 原生 Windows | 同一 prefix 内的 Wine |
| 传入路径 | Windows 原生路径 | 业务路径先经 `winepath --windows` |
| .NET Runtime | Windows x64，直接传 `DALAMUD_RUNTIME` | 同一 Windows x64 runtime，路径转成 Wine 格式 |
| 环境 | 合并 Windows environment block | `WINEPREFIX`、DLL override、DXVK、同步机制等 Wine 环境 |
| 结果 | JSON 中 PID + 可继承进程 handle | JSON 中 Wine PID；可选映射为 Unix PID |
| 超时/输出 | 等 Injector 最多 60 秒退出再解析一行 | 读取输出并最多等约 30 秒出现包含 PID 的 JSON |
| 特殊依赖 | VC++ 2015–2022 x64 redistributable、x64 架构 | x64 Wine；旧版本可能需 portable-PDB，可靠 PID 管理需 unix-pid maps |

两个 runner 的共同点比差异更重要：都由 Injector 启动 FFXIV，并解析它返回的 JSON；Rust 不应先直接启动游戏再尝试跨平台补注入，除非实现明确的“注入已有进程”恢复功能。

### 2.5 `DalamudLoadMethod.cs` / `DalamudSettings.cs`

`DalamudLoadMethod`：

- `EntryPoint` → `--mode=entrypoint`。
- `DllInject` → `--mode=inject`。
- `ACLonly` → `--mode=inject --without-dalamud`。

`DalamudSettings` 是 Dalamud 自身/更新通道相关配置的最小模型：

- `DalamudBetaKey`：staging 授权 key。
- `DalamudBetaKind`：staging track 名，空时默认为 `staging`。
- `DoDalamudRuntime`：即使 release 未强制，也由 launcher 管理 runtime。
- 配置文件默认 `<config>/dalamudConfig.json`；反序列化失败时回退默认值。

是否启用 Dalamud、load method、插件安全模式等 launcher 级选项在上层设置中决定，不能只靠 `DalamudSettings`。Rust 侧应把“launcher 是否启动 Dalamud”和“交给 Dalamud 的配置文件”分开。

## 3. eorzea 的集成方案

### 3.1 建议模块边界

建议新增独立 `src/dalamud/`（或后续拆成 workspace crate），保持以下职责：

- `model.rs`：`DalamudVersionInfo`、安装状态、progress、`DalamudStartInfo`、Injector JSON 输出。
- `updater.rs`：release 元数据、rollout/staging、下载、解压、hash manifest、原子安装和旧版清理。
- `runtime.rs`：Windows x64 .NET runtime 下载/组装/校验。
- `assets.rs`：按上游 AssetManager 协议确保 assets。
- `args.rs`：类型化生成 Injector argv。
- `runner.rs`：共同 orchestration，按 `cfg(target_os)` 调用 Windows 或 Wine adapter。
- `wine_runner.rs`：路径转换、prefix 环境、Wine PID 处理。

不要把这些逻辑放进 `eorzea-auth`：Dalamud 属于游戏启动/本地资产生命周期，与认证协议无关。

### 3.2 release 下载与版本管理

现有 `src/game_files/patch_manager.rs` 已提供 `reqwest`、并发下载、进度回调和 SHA-1 校验的思路，`src/wine.rs` 也已有工具包下载与解压。但 Dalamud 不应直接复用“游戏 patch entry”数据模型；应抽取或新增通用的 streamed downloader/progress primitive，再由各自 updater 管理元数据和完整性规则。

最低要求：

1. 读取本地游戏版本文件，并请求 CN 分支使用的 release API；保留 endpoint 可配置能力，避免把国际服与国服通道混用。
2. 在登录/启动前并行预取 metadata，但在启动前强制 `SupportedGameVer == local game version`。游戏刚更新而 Dalamud 尚未发布时，必须明确提示并允许“禁用 Dalamud 启动”，绝不能加载旧版。
3. 下载到临时文件/临时目录，限制响应大小，校验 `hashes.json` 及全部文件，拒绝绝对路径、`..` 和 symlink escape，最后原子 rename 为 `Hooks/<AssemblyVersion>`。
4. zip 可用 Rust crate；CN release 当前可能为 7z，需选择纯 Rust 7z crate、受控外部 `7zz`，或服务端 zip。不能假定 zip-only。
5. 分别管理 runtime 和 assets；只有三者均完成才把安装标记为 ready。
6. 保存 `version.json` 与 rollout bucket。bucket 应首次随机生成后持久化，而不是每次运行重新抽签，以免控制组抖动。
7. 失败时保留已验证的旧版本供回滚，但旧版本仅能在 `SupportedGameVer` 仍匹配时使用。

建议默认根目录延续项目现状，如 `~/.xiv-launcher-rs/dalamud/{Hooks,runtime,assets,config,logs,installedPlugins}`，不要依赖用户恰好存在的 `~/.xlcore_cn`；可在开发期提供只读 `runner_override` 便于使用现有安装验证。

### 3.3 启动时注入

当前 `launch_game()` 是直接执行 `ffxiv_dx11.exe`/`WineTool::run(game_path, ...)`。启用 Dalamud 后需改为二选一的 launch backend：

```text
disabled -> 当前 direct game runner
enabled  -> ensure release/runtime/assets -> Injector runner -> Injector 创建游戏
```

Linux 实现的关键步骤：

1. 用当前 `WineTool::resolve()` 得到与游戏完全相同的 Wine binary 和 prefix，并完成 DXVK 等准备。
2. 提供 `WineTool::winepath_windows(&Path)`；所有写入 Injector option 的路径均转换。转换操作可以批量/并发，但必须使用同一 prefix 和环境。
3. `build_launch_env()` 后追加 `DALAMUD_RUNTIME=<Wine runtime path>`；自定义 env 的覆盖优先级需要明确，避免用户无意覆盖为宿主 Linux runtime。
4. 调用 `wine64 <Dalamud.Injector.exe> launch ... -- <game argv>`，当前目录设为 Hooks 版本目录，捕获 stdout/stderr 到独立日志。
5. 对 stdout 使用带总超时的逐行读取，找到可反序列化的 `{pid,handle}` 结果；不能仅靠字符串包含 `pid`。校验 Injector exit status，并把诊断输出保留给 UI。
6. `GameLaunchResult` 不能继续只持有 `std::process::Child`：Injector 可能很快退出，而游戏是另一个 Wine 进程。应改成可表达 `launcher_child`、`wine_pid`、可选 `unix_pid` 的 `GameProcess`/handle abstraction。
7. MVP 可以在无法映射 Unix PID 时报告“游戏已启动但无法精确管理进程”，用 wineserver/prefix 级状态作弱监控；完整版本再实现 `winedbg info procmap` 或采用带 unix-pid maps 的托管 Wine。不得把 Injector child 的退出误报为游戏退出。

Windows 实现则用原生 `Command` 启动 Injector，并解析同一 JSON。第一版可以按 PID 打开进程而不复制 C# 的 inherited handle 优化；完整版本再用 Win32 handle inheritance/ownership，使 launcher 生命周期和进程回收与上游一致。

### 3.4 配置与 Tauri 接口

建议在现有 TOML 顶层加入 `DalamudSettings`，并用 `#[serde(default)]` 保持旧配置兼容：

```toml
[dalamud]
enabled = false                 # 首次实现建议 opt-in
load_method = "entrypoint"     # entrypoint | inject；ACL-only 作为内部/高级模式
delay_initialize_ms = 0
no_plugins = false
no_third_party_plugins = false
manage_runtime = true
track = "release"
# beta_key 不应写入普通日志；如必须持久化，至少按敏感配置处理
```

还需要：

- 一次性 safe mode（`--no-plugin`），用于崩溃恢复，不应永久改写主配置。
- “本次禁用 Dalamud 启动”，用于游戏更新后等待兼容版本或排障。
- 配置、日志、plugins、assets 的路径 override（高级/开发选项）。
- UI 状态：checking metadata、downloading release/runtime/assets、verifying、starting、unsupported game version、failed。
- Tauri events 中只发送结构化进度和脱敏错误；游戏 ticket、SDO ID、完整游戏 argv、beta key、troubleshooting pack 均不得进入普通前端日志。

`dalamudConfig.json` 应作为 Dalamud 自己管理的 JSON 文件传入，不要强行合并进 launcher TOML；launcher TOML 只保存启用策略与启动方式。

### 3.5 风险与依赖

| 风险 | 影响 | 建议 |
|---|---|---|
| 游戏版本与 Dalamud 不匹配 | 启动崩溃、内存结构错误 | 启动前严格比较 `SupportedGameVer`；不匹配时默认禁用/阻止加载 |
| release API/CN CDN 属于外部服务 | 无法更新或通道变化 | endpoint 可配置；缓存已验证 metadata；清晰区分离线与不兼容 |
| 上游仅 MD5 manifest，无签名 | 供应链校验强度有限 | HTTPS、限制重定向域、固定元数据 schema；未来支持签名/更强摘要；不要降低上游校验 |
| 7z/zip 解压 | path traversal、磁盘占满、半安装 | 临时目录、安全路径校验、大小/文件数限制、原子提交 |
| Windows .NET Runtime 体积和版本绑定 | 首次下载大；错误 runtime 无法加载 | 按服务端 `RuntimeVersion` 管理 Windows x64 runtime；不要调用宿主 `dotnet` |
| Wine CLR/调试兼容性 | Injector 或插件崩溃 | 使用已验证的 wine-xiv；Wine 9.0–10.7 注意 portable-PDB，10.8+ 已上游修复 |
| Wine PID 与 Unix PID 不同 | 无法等待/终止/检测重复启动 | 完整版要求 unix-pid maps 或可靠映射；MVP 明确降级语义 |
| `mscoree`/DXVK override 冲突 | CLR 或渲染加载错误 | 统一从同一个 env builder 启动 Injector 和游戏，不允许两套 prefix/env |
| 插件的 Wine 兼容性 | 单个插件可导致崩溃 | safe mode、禁第三方插件、崩溃后恢复入口；说明 Browsingway 等存在 Wine 特定问题 |
| 架构 | x86/ARM32 不支持，ARM64 依赖 x64 仿真 | 启动前检查；第一阶段仅承诺 x86_64 Windows/Linux |
| Windows native 依赖 | 缺 VC++ runtime 导致 Boot/依赖无法加载 | Windows 检测 VC++ 2015–2022 x64；Wine 使用已验证发行/prefix |
| 参数与日志泄密 | ticket/账号凭证外泄 | 全程结构化 argv，不打印 `--` 后原始参数；复用并加强现有脱敏 |
| 上游内部协议演进 | 硬编码文件/参数失效 | 发行包整体更新；只依赖 version metadata 和 Injector CLI；保留 override 做集成测试 |

## 4. 分阶段建议

### 阶段 0：协议探针与可重复验证

- 建立 `DalamudVersionInfo`、目录布局和 Injector argv 的 Rust 模型。
- 增加开发用 `runner_override`、runtime/assets override，先用已安装且与测试游戏匹配的 CN 发行包验证。
- 在 Linux 上实现同 prefix 的 `winepath`、`DALAMUD_RUNTIME` 和 Injector stdout JSON 解析。
- 写一个独立 CLI/example，只打印脱敏状态，不先接 Tauri UI。

验收标准：能通过 Rust 入口让 Injector 创建游戏并加载 Dalamud；能区分 Injector 失败、游戏启动成功、PID 映射不可用三种状态。

### 阶段 1：MVP（建议先做 Linux x86_64 + CN release）

- 配置只提供 `enabled`、`entrypoint` 和一次性 safe mode；默认 opt-in。
- 实现 release metadata、zip/7z 下载、上游 hash 校验、游戏版本 gate。
- 实现必需 runtime 与 assets 下载；若 assets 协议工作量过大，MVP 可要求用户提供已验证 assets override，但不能把缺 assets 的状态标为完整安装。
- 将 `launch_game()` 抽象为 direct/Injector 两个 backend；Linux 使用现有 `WineTool` 和同一 env builder。
- Tauri 展示分阶段进度；更新失败或不兼容时给用户明确的“本次不加载 Dalamud”路径。

验收标准：全新用户目录中可完成安装和启动；游戏版本不匹配绝不注入；safe mode 可恢复插件导致的崩溃。

### 阶段 2：可靠性与 Windows 对齐

- Windows runner、VC++/架构检查、Injector 超时和 exit-code 语义。
- 原子安装、断点/重试、rollout bucket 持久化、旧版回滚与清理。
- 类型化 `GameProcess`，实现 Windows handle 与 Wine PID→Unix PID 管理。
- 完整支持 `DllInject`、`no_plugins`、`no_third_party_plugins`、初始化延迟和诊断包。
- 为 metadata、hash 校验、安全解压、argv、PID JSON 解析加单元/集成测试。

### 阶段 3：完整体验

- staging/beta key 与多 track、canary/control rollout、开发版 override。
- Dalamud assets 增量更新、runtime 完整性修复、后台预取与 UI 取消/重试。
- 崩溃检测后自动建议 safe mode，展示独立 Dalamud/Injector/Wine 日志。
- macOS/Wine ARM64+x64 仿真验证；对受支持 Wine 构建建立兼容矩阵。
- 与 game update 串联：游戏补丁完成后立即刷新 Dalamud compatibility，但仍保持两个 updater 的数据模型和事务边界独立。

## 结论

最小风险路线是把 Dalamud 当成一个有严格版本约束、携带自己 Injector/Boot/托管程序集的外部运行时产品：Rust 负责可靠获取、校验、配置和进程编排，不重写其内存注入或 CLR hosting。Linux 的正确实现不是“原生加载 .NET Dalamud”，而是在与游戏相同的 Wine prefix 中运行 Windows `Dalamud.Injector.exe`，传入 Windows 路径的托管 Windows x64 Runtime，并接受 Wine PID/Unix PID 两层进程模型。

建议从 Linux CN release 的 CLI 探针和 `entrypoint` MVP 开始，随后补齐 runtime/assets、Tauri 进度和版本 gate，再实现 Windows handle、完整 PID 管理、staging 与崩溃恢复。对架构最关键的改动是：让 Injector 成为启用 Dalamud 时的游戏创建者，并将当前只容纳一个 `Child` 的启动结果升级为可表达 Injector、Wine PID 和游戏进程生命周期的抽象。

## 参考代码索引

- `XIVLauncher.Common/Dalamud/DalamudLauncher.cs`
- `XIVLauncher.Common/Dalamud/DalamudUpdater.cs`
- `XIVLauncher.Common/Dalamud/AssetManager.cs`
- `XIVLauncher.Common/Dalamud/DalamudStartInfo.cs`
- `XIVLauncher.Common/Dalamud/DalamudInjectorArgs.cs`
- `XIVLauncher.Common/Dalamud/DalamudLoadMethod.cs`
- `XIVLauncher.Common/Dalamud/DalamudSettings.cs`
- `XIVLauncher.Common/Dalamud/DalamudVersionInfo.cs`
- `XIVLauncher.Common.Windows/WindowsDalamudRunner.cs`
- `XIVLauncher.Common.Windows/WindowsGameRunner.cs`
- `XIVLauncher.Common.Windows/WindowsDalamudCompatibilityCheck.cs`
- `XIVLauncher.Common.Unix/UnixDalamudRunner.cs`
- `XIVLauncher.Common.Unix/UnixGameRunner.cs`
- `XIVLauncher.Common.Unix/Compatibility/CompatibilityTools.cs`
- `XIVLauncher.Common.Unix/UnixDalamudCompatibilityCheck.cs`
- 本项目：`src/game.rs`、`src/wine.rs`、`src/config.rs`、`src/game_files/patch_manager.rs`
