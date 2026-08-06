# Linux 下运行 FFXIV 的 Wine 方案调研

> 调研时间：2026-08。对应代码：`src/wine.rs`；
> C# 参考：`XIVLauncher.Common.Unix/Compatibility/`（`CompatibilityTools.cs`、`WineSettings.cs`、`Dxvk.cs`）。

## 概念关系：Wine / Proton / Lutris 都是什么

- **Wine** 是根基：Windows API 的开源重实现，其余一切 Wine 系方案都是它的分支或构建。
- **Proton** 是 Valve 的 Wine fork（Steam 的兼容层）：在 Wine 之上打包了 DXVK、vkd3d（DX12）、media foundation 补丁、FSR、Steam 集成和容器运行环境。它不能像普通 wine 那样裸跑 `wine64 xxx.exe`，必须在 Steam/umu 容器里运行。
- **Lutris / Bottles** 不是兼容层，而是**游戏管理器**：本身不含 Wine 代码，负责按游戏下载某个 wine 构建（runner）、配置独立 prefix、跑安装脚本。XIVLauncher 对 FFXIV 扮演的正是这个角色（自己下载 wine-xiv、管 prefix、装 DXVK），外加 Dalamud 注入。

```
Wine（上游）
├── wine-staging（试验田分支）
│   └── wine-tkg（社区构建系统）
│       └── wine-xiv（XIVLauncher 托管，+FFXIV 补丁）
│           └── unofficial-wine-xiv（rankynbass fork，ntsync）
└── Proton（Valve fork，= Wine + DXVK + Steam 集成 + 容器）
    ├── GE-Proton（社区加强版）
    │   └── proton-xiv（+FFXIV 补丁）
    └── proton-cachyos（CachyOS 优化构建）

Lutris / Bottles / XIVLauncher —— 管理器层：下载上述构建、配 prefix、装 DXVK
```

对 FFXIV 而言的选型层次：**选一个 wine 变体 → 选谁帮你管理它（XIVLauncher / Lutris / Steam / 手动）**，游戏文件始终与两者无关。

## TL;DR

**是的，可以把不同 Wine 方案理解为一套套可互换的运行环境，与游戏文件完全分离。**

Linux 下跑 FFXIV 涉及三个相互独立的目录：

| 组件 | 作用 | 典型位置 |
|------|------|----------|
| **Wine binary** | Windows API 兼容层本体（`wine64`/`wineserver`），即"解释器" | `~/.xlcore/compatibilitytool/beta/wine-xiv-staging-fsync-git-*/bin/` |
| **Wine prefix** | 虚拟 C: 盘：注册表、DLL override、DXVK 的 `d3d11.dll`/`dxgi.dll` 都装在这里，通过 `WINEPREFIX` 环境变量绑定 | `~/.xlcore/wineprefix`（本项目：`~/.xiv-launcher-rs/prefix`） |
| **游戏文件** | `ffxiv_dx11.exe` 及全部游戏数据，**不在 prefix 内**，运行时经 `winepath --windows` 映射为 Z: 盘路径传给游戏 | 独立目录（如 `~/.xlcore/ffxiv`，国服拷贝 Windows 侧游戏文件） |

**prefix 与 wine binary 没有绑定关系**：一个 prefix 可以被不同版本的 wine 共用（`WINEPREFIX` 只是个环境变量，指哪用哪），一个 wine 也可以同时服务多个 prefix。XIVLauncher 就是"单一 prefix + 可换 wine binary"的用法——它把托管 wine 升级到新版本时 prefix 并不重建。

本项目在 Linux/macOS 上要求游戏完整路径只包含 ASCII 字符，例如 `~/Games/ffxiv`。Wine 可以把普通 Unix 路径映射到 Z: 盘，但 FFXIV 国服的启动链路不能可靠处理非 ASCII 路径；使用 `~/Games/最终幻想XIV` 一类目录可能在游戏内表现为误导性的 `5003` 认证错误。启动器会在启动 Wine 前拒绝这类路径并返回明确错误。

换掉 Wine 方案 = 换掉第一个目录（解释器），游戏文件原封不动。唯一的耦合点在 prefix：注册表、DLL override 和 DXVK 安装状态是装在 prefix 里的，换 Wine 家族（如 staging ↔ Proton 系）偶尔需要重建 prefix，但游戏文件从不受影响。

## 方案对比

### 1. wine-xiv（XIVLauncher 托管，主流方案）

- goatcorp 基于 **wine-tkg 构建系统**（Frogging-Family）编译的 wine-staging + FFXIV 专用补丁 + fsync，按发行版分别构建（Arch/Fedora/Ubuntu，glibc 链接差异）。
- 仓库：[goatcorp/wine-xiv-git](https://github.com/goatcorp/wine-xiv-git)。其 FFXIV 专用补丁其实只有 `ffxiv-launcher-workaround` 一个，其余差异全部来自 wine-tkg 默认配置（staging + fsync + protonify + lsteamclient）。
- **官方发版严重滞后**：最新 release 停滞在 wine-staging **10.8 + fsync**（2025-06 构建、2026-03 重新打包），无 ntsync。XLCore 托管下载、国服镜像、本项目 `src/wine.rs` 下载的都是这个版本。
- 上游从 GitHub Releases 下载到 `~/.xlcore/compatibilitytool/beta/`；**CN 分支改走国内镜像** `https://s3.ffxiv.wang/xlcore/deps/wine/{arch,fedora,ubuntu,osx}/...`（`CompatibilityTools.cs:29-48`，Steam Deck 强制用 Ubuntu 包）。
- 为什么自带而不依赖系统 wine：① 带 FFXIV/ACT 专用补丁；② 消除发行版 wine 版本差异便于排障；③ Steam Deck 无系统 wine 且系统只读。
- 仍保留 `WineStartupType.Custom`，可指向任意自定义 wine 的 bin 目录。
- 本项目 `src/wine.rs` 即对应此方案：检测优先级为 自定义路径 → `~/.xlcore/beta/wine` → 系统 `wine64`，未命中则从 S3 下载 Ubuntu 包。

### 2. GE-Proton / Wine-GE（GloriousEggroll）

- GE-Proton 是 Valve Proton + 额外补丁（"proton-ge 之于 proton 就像 wine-staging 之于 wine"）；Wine-GE 是同补丁集、供 Lutris 等非 Steam 场景使用的变体。2026 年仍在活跃发布（GE-Proton10/11 系列）。
- 与 XIVLauncher 兼容：可把 Custom wine 路径指向 Proton-GE 的 `bin`。另有社区项目 [rankynbass/proton-xiv](https://github.com/rankynbass/proton-xiv)（Proton-GE + XIVLauncher 补丁）和 XIVLauncher-SCT（把 XIVLauncher.Core 打包成 Steam 兼容工具）。

### 3. 官方 Wine / Wine-Staging / Wine-TKG

- 可行但折腾：需要自己用 winetricks 装齐依赖（`corefonts dxvk dotnet48 vcrun2022` 等）、没有 FFXIV 专用补丁、发行版打包质量参差。
- wine-tkg 不是发行物而是定制构建系统（wine-xiv 即基于它），适合想自己打 ntsync 等补丁的进阶用户。

### 4. Steam Proton

- 优点：零配置、Steam 集成/Overlay/Deck 支持好。
- 缺点：官方启动器在 Proton 下长期脆弱（依赖 WebView2/IE 组件，黑屏等问题反复出现，见 [ValveSoftware/Proton#580](https://github.com/ValveSoftware/Proton/issues/580)）；绑定 Steam 账号。社区共识是即使用 Steam 版游戏也换 XIVLauncher 启动。
- 国服玩家会入库免费试玩版（appid 312060）来获得 Proton 环境。

### 5. Lutris / Bottles

- Lutris 有官方脚本页（含 "XIVLauncher 版"，用 Lutris runner / Wine-GE），但脚本历史上多次因 winetricks/.NET 问题坏掉；如今已被 XIVLauncher flatpak 取代，不是主流路径。
- Bottles 未能证实有维护中的 FFXIV 配方。

## 社区/第三方 Wine 构建（与官方 wine-xiv 的区别）

官方 wine-xiv 停滞在 10.8 + fsync，于是社区出现了一批替代构建。核心差异维度：**wine 基底版本、同步机制（esync/fsync/ntsync）、是否带 FFXIV 专用补丁**。

FFXIV 专用补丁主要是两个（都在 unofficial-wine-xiv 的 `wine-tkg-userpatches/`）：

- **unix-pid maps**（`xiv-unix-pid-maps.patch`）：把 wine 进程映射到 unix PID，XLCore 靠它管理/杀游戏进程，双开也依赖它。缺失时 XLCore 进程管理不可靠。
- **portable-pdb**：wine 9.0–10.7 上 Dalamud 必需的调试补丁；wine ≥10.8 上游已修复，新构建不再需要。

### 构建一览

| 构建 | 基底 | 同步机制 | FFXIV 补丁 | 维护状态 | 适合谁 |
|------|------|----------|-----------|----------|--------|
| 官方 wine-xiv 10.8 | wine-staging 10.8 (tkg) | fsync/esync | launcher workaround | goatcorp，发版停滞 | 默认求稳、官方支持渠道 |
| [unofficial-wine-xiv](https://github.com/rankynbass/unofficial-wine-xiv-git) v11.x | wine-staging 11.x (tkg) | **ntsync only**（10.20 起；可自编退回 fsync/esync） | +unix-pid maps、protonify、lsteamclient | rankynbass，2026-08 仍活跃 | 想追新 wine/ntsync 的桌面用户 |
| unofficial-wine-xiv `valvebe-*` | Valve bleeding-edge wine 10 | ntsync+fsync+esync 共存 | +DualSense、FSR fshack、portable-pdb | 同上 | 需要 Steam 集成/FSR 的人 |
| [proton-xiv](https://github.com/rankynbass/proton-xiv) | GE-Proton10（11.0 起改以 proton-cachyos 为基底） | ntsync | +unix-pid maps、portable-pdb | rankynbass，活跃 | XIVLauncher-RB（其 XLCore fork）/Steam 流程 |
| [XIVLauncher-SCT](https://github.com/rankynbass/XIVLauncher-SCT) | 打包整个 XLCore 作 Steam 兼容工具，只用 Proton | 取决于所选 Proton | — | rankynbass，低频 | Steam Deck 单账号玩家 |
| [GE-Proton](https://github.com/GloriousEggroll/proton-ge-custom) | Proton Experimental | ntsync（10-9 起，`PROTON_USE_NTSYNC=1`） | **无 XIV 补丁** | GloriousEggroll，很活跃 | Steam 直开、不用 Dalamud 的玩家 |
| [wine-tkg](https://github.com/Frogging-Family/wine-tkg-git) 自建 | 任意 staging/master | 任选（10.18 起 tkg 已无 fsync 补丁） | 自选 | Frogging-Family，活跃 | 愿意自己编译（30–60 分钟）的高级用户 |
| proton-cachyos / wine-cachyos | Proton/wine + tkg 系补丁，x86-64-v3 优化 | ntsync 早鸟（2025-01 起） | 无 | CachyOS 团队，活跃 | Arch 系；proton-xiv 11.0 已改用其作基底 |
| [Kron4ek Wine-Builds](https://github.com/Kron4ek/Wine-Builds) tkg 变体 | wine-tkg | ntsync 可选 | 无 | Kron4ek，活跃 | Lutris/ProtonUp-Qt 通用场景 |
| dwproton / spritz-wine | proton-cachyos / staging | ntsync | 无（面向 gacha 游戏） | 各自团队 | 非 FFXIV 场景 |
| XIV on Mac winecx（macOS） | CrossOver wine fork + DXVK-macOS/MoltenVK | — | Mac 专用 | marzent，活跃 | Apple Silicon |

### 要点

- **ntsync 是社区构建相对官方的最大卖点**：已进入主线内核（6.10+，ntsync 模块完善于 6.14），Wine 10+ 原生支持；相对 fsync 帧数差 ≤5% 但帧 pacing 更稳（CachyOS 论坛实测，非 FFXIV 专项）。官方 XLCore 目前不支持 ntsync 甚至会阻碍外部 wine 启用它（[issue #285](https://github.com/goatcorp/XIVLauncher.Core/issues/285)）。
- **追新有代价**：wine >10.17 会破坏 Browsingway/Aetherment 等浏览器覆盖层插件；10.20 ntsync 版有黑屏报告。unofficial-wine-xiv 出问题一般退回旧 tag。
- **裸 GE-Proton 跑 XLCore 不可行/不受支持**：Proton 需要容器/umu 运行环境，且缺 unix-pid maps 与 portable-pdb（proton-xiv 就是为补这两个补丁而存在）。Wine-GE 已停更（停留 8-26），对新版本 FFXIV 意义不大。
- **国服没有自研 wine**：XIVLauncherCN（ottercorp）托管的就是官方 wine-xiv 10.8 + fsync，只把下载地址换到自家 S3 镜像；macOS 则镜像 XIV on Mac 的 wine。
- 对本项目的启示：复刻托管 wine 逻辑时，官方与国服镜像同为 10.8+fsync；若想提供 ntsync，目前唯一有发布物且活跃的来源是 unofficial-wine-xiv（需内核 ≥6.14 与 `/dev/ntsync`）。



FFXIV 是 DX11 游戏，wine 自带 wined3d 渲染不正确且性能差，**DXVK（DX11→Vulkan）是必需品**。

- 安装方式（C# 参考与 `src/wine.rs` 一致）：直接把 DXVK 的 `x64/*.dll`（d3d9/d3d11/d3d10core/dxgi 等）拷进 `<prefix>/drive_c/windows/system32/`，靠 `WINEDLLOVERRIDES=...d3d9,d3d11,d3d10core,dxgi=n` 加载，不跑 setup 脚本。
- 变体：
  - **dxvk-async**（Sporif）：已停更（停留 DXVK 2.0 时代）。CN 分支目前用的就是 S3 镜像的 `dxvk-async-1.10.1`，本项目 `src/wine.rs` 与之对应。
  - **dxvk-gplasync**（Ph42oN，GitLab）：持续跟进上游（2026 年已基于 DXVK 3.0），画面 bug 更少。上游 goatcorp XIVLauncher.Core 已切换到 gplasync（1.3.1 起提供 DXVK 2.6.1/2.7）。
  - async 的意义：着色器未编译完先画帧，减少卡顿。
- 相关环境变量：`DXVK_ASYNC`、`DXVK_STATE_CACHE_PATH=C:\`、`DXVK_CONFIG_FILE=C:\ffxiv_dx11.conf`、`DXVK_HUD`。

## 技术要点

- **esync / fsync / ntsync**：同步机制依次演进。ntsync 已进入主线内核（~6.10+），Wine 10+ 原生支持，CPU 受限场景（MMO 典型）提升明显。XIVLauncher 目前只暴露 `WINEESYNC`/`WINEFSYNC` 开关（`CompatibilityTools.cs:315-323`），ntsync 需 [rankynbass/unofficial-wine-xiv-git](https://github.com/rankynbass/unofficial-wine-xiv-git) 等第三方构建。
- **Wayland vs XWayland**：wine-wayland 驱动下 FFXIV 有多个已知 bug（失焦断线、转视角时光标不隐藏、鼠标乱飞），**2026 年 XWayland 仍是务实默认**。
- **Dalamud（卫月）**：wine 下可用，是 XIVLauncher 核心功能；`WINEDLLOVERRIDES` 中 `mscoree=n,b` 与之相关。个别插件（如 Browsingway）在 wine 下会崩。
- **国服（盛趣）差异**：国服官方启动器依赖 IE8，而 winetricks 的 IE8 只能装进 32 位 prefix、DX11 又要求 64 位 → 官方启动器路线基本不可行。实际做法：拷贝 Windows 侧游戏文件 + **XIVLauncherCN**（ottercorp 分支，Flathub `cn.ottercorp.xivlaunchercn` / AUR `xivlauncher-cn`），wine 方案与 wine-xiv 相同，只是依赖改走国内 S3/CDN 镜像（`s3.ffxiv.wang`）。这正是本项目参照的路线。

## 启动时的 Wine 环境变量（C# 参考，Linux）

```
WINEPREFIX=<prefix 路径>
WINEDLLOVERRIDES=msquic=,mscoree=n,b;d3d9,d3d11,d3d10core,dxgi=n
WINEDEBUG=<DebugVars>          # 非空才设
WINEESYNC=0/1  WINEFSYNC=0/1   # macOS 用 WINEMSYNC
XL_WINEONLINUX=true            # 游戏侧据此检测运行在 wine 下
DXVK_HUD / DXVK_ASYNC / DXVK_STATE_CACHE_PATH=C:\ / DXVK_CONFIG_FILE=C:\ffxiv_dx11.conf
LD_PRELOAD+=libgamemodeauto.so.0   # 开启 gamemode 时
```

注：本项目 `src/wine.rs` 目前只设了 `WINEPREFIX` + `XL_WINEONLINUX`/`XL_WINEONMAC`，其余（DLL overrides、esync/fsync、DXVK 变量）尚未实现。

## 与本项目的对应关系

| C# 参考 | 本项目 |
|---------|--------|
| `CompatibilityTools.cs`（wine 检测/下载/运行） | `src/wine.rs` `WineTool::detect/ensure/run` |
| `WineSettings.cs`（StartupType/Prefix/esync/fsync/DebugVars） | 未实现 |
| `Dxvk.cs`（下载 + 拷 DLL 到 prefix） | `src/wine.rs` `ensure_dxvk`（当前固定 dxvk-async 1.10.1，跟随 CN 分支） |

## 主要来源

- [goatcorp/wine-xiv-git](https://github.com/goatcorp/wine-xiv-git)、[goatcorp/XIVLauncher.Core](https://github.com/goatcorp/XIVLauncher.Core)（+ 本地 C# 源码 `XIVLauncher.Common.Unix/Compatibility/`）
- [rankynbass/unofficial-wine-xiv-git](https://github.com/rankynbass/unofficial-wine-xiv-git)、[rankynbass/proton-xiv](https://github.com/rankynbass/proton-xiv)、[rankynbass/XIVLauncher-SCT](https://github.com/rankynbass/XIVLauncher-SCT)
- [XIVLauncher.Core#285](https://github.com/goatcorp/XIVLauncher.Core/issues/285)（ntsync/自定义 wine 讨论）
- [GloriousEggroll](https://www.gloriouseggroll.tv/)、[proton-ge-custom releases](https://github.com/gloriouseggroll/proton-ge-custom/releases)、[Frogging-Family/wine-tkg-git](https://github.com/Frogging-Family/wine-tkg-git)、[Kron4ek/Wine-Builds](https://github.com/Kron4ek/Wine-Builds)
- [ValveSoftware/Proton#580](https://github.com/ValveSoftware/Proton/issues/580)（FFXIV 官方启动器追踪）
- [Ph42oN/dxvk-gplasync](https://gitlab.com/Ph42oN/dxvk-gplasync)
- [badspells.com: How to play FFXIV on Linux](https://badspells.com/articles/how-to-play-ffxiv-linux)
- [Flathub cn.ottercorp.xivlaunchercn](https://flathub.org/en/apps/cn.ottercorp.xivlaunchercn)、[ottercorp FAQ](https://aonyx.ffxiv.wang/faq/xl_troubleshooting)
