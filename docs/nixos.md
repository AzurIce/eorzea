# NixOS 下的 Wine 方案与本启动器的适配

> 调研时间：2026-08。前置阅读：`docs/wine_on_linux.md`。
> 对应代码：`src/wine.rs`、`src/config.rs`（`WineSettings`）、`flake.nix`（devShell）。

## 问题本质

NixOS 没有 FHS 布局：没有 `/lib64/ld-linux-x86-64.so.2`、没有全局库路径。XIVLauncher 托管的 wine-xiv tarball 是针对 Arch/Fedora/Ubuntu glibc 预编译的，解压后直接执行会报 "No such file or directory"（ELF interpreter 缺失），即使解决了 interpreter，运行时 `dlopen` 的库（freetype/gnutls/vulkan-loader…）也找不到。

## 跑外来预编译二进制的四条通用路线

| 方案 | 原理 | 对"整棵 wine 目录"的适用性 |
|------|------|---------------------------|
| patchelf / autoPatchelf | 改 ELF interpreter + RPATH 指向 nix store | 差：wine 树有几十个二进制 + 大量 dlopen 库，每次更新重 patch，易漏 |
| **nix-ld**（`programs.nix-ld.enable`） | 在 `/lib64/ld-linux-*.so.2` 放 shim，库从 `NIX_LD_LIBRARY_PATH` 找 | 可行但要 root 改系统配置、全局维护库清单；不适合"开箱即用" |
| **steam-run** | Steam 的 FHS 容器（bubblewrap），内含完整 FHS + 32/64 位库，挂载 `/run/opengl-driver{,-32}`（Vulkan ICD 直接可用） | **好**：零配置、无需 root；已知坑是容器内时区错乱（FFXIV 游戏内时间不对，`unset TZ` 可解，nixpkgs#279893） |
| **buildFHSEnv** | 自建定制 FHS 环境，精确指定依赖 | 最好但需写一个 nix 表达式分发 |

nixpkgs 官方的 `xivlauncher` 包的答案就是 steam-run：它把整个 XIVLauncher.Core 包进 steam-run（`useSteamRun = true`），于是 XIVLauncher 运行时下载的 wine-xiv 也在 FHS 里跑，动态链接问题由容器解决。

## nixpkgs / nix 生态的 Wine

- **nixpkgs**：`wineWow64Packages.{stable,staging,unstable,waylandFull}`（nixos-unstable 当前是 wine 11.14 staging）。**ntsync 上游原生支持**（Wine 10+），只要内核有 `/dev/ntsync` 就自动启用；**没有 fsync**（那是 wine-tkg/Proton 的 out-of-tree 补丁）。
- **ntsync 内核侧**：主线 ≥6.14，NixOS 需 `boot.kernelModules = [ "ntsync" ]`；`/dev/ntsync` 自 6.14（及 6.13.11 回补）起默认权限 0666，无需 udev 放权规则。
- **[nix-gaming](https://github.com/fufexan/nix-gaming) flake**：仍活跃，提供 `wine-tkg`（11.14 staging+tkg 补丁集）、`wine-cachyos`、`dxvk-gplasync` 等；建议配 cachix（`nix-gaming.cachix.org`）否则本地编译 wine 半小时起。`wine-ge` 停更在 8.x，不用考虑。
- 与 wine-xiv 的差距：缺 `ffxiv-launcher-workaround`（针对官方启动器，本启动器直连游戏，推断不必要）和 unix-pid maps（影响进程管理/双开），其余可用。

## NixOS 上玩 FFXIV 的现有实践

- **nixpkgs `xivlauncher` 包** = XIVLauncher.Core + steam-run 包装（国际服）。
- **Flatpak**：`dev.goats.xivlauncher`（国际服）/ `cn.ottercorp.xivlaunchercn`（国服），runtime 自带 FHS，是最省事的路线。
- 国服没有 nixpkgs 包（仅 AUR 有 xivlauncher-cn）；NixOS 国服玩家实际走 flatpak。
- NixOS 官方 wiki 无 FFXIV 条目。

## 本项目现状（NixOS 视角）

好消息是架构已经就位：`WineSettings.startup_type` 有 `Auto/Managed/Custom/System` 四种，`System`/`Custom` 直接用 nixpkgs wine 就能跑。当前的缺口：

1. **`Auto` 顺序对 NixOS 不利**：自定义 → `~/.xlcore` 托管 → 系统 wine64 → 下载。NixOS 上"下载"这一步拿到的 wine-xiv 裸跑起不来（除非系统开了 nix-ld）。
2. **probe 失败即报错，无回退**：`game.rs` 里 `tool.probe()` 失败直接返回错误，不会尝试系统 wine 或 steam-run。
3. **devShell 方案有隐含依赖**：`flake.nix` 用 `LD_LIBRARY_PATH` 覆盖 wine 的 dlopen 库，在本机可用是因为系统开了 **nix-ld**（`/lib64/ld-linux-x86-64.so.2 → nix-ld` shim）——这个前提没写在任何地方，换台没开 nix-ld 的 NixOS 机器就会莫名失败。
4. 本机现状：`steam-run` 已装（`/run/current-system/sw/bin/steam-run`），无系统 wine，内核 7.1.5 但未加载 ntsync 模块（无 `/dev/ntsync`）。

## 建议的适配路线（按优先级）

### A. NixOS 上 Auto 优先系统 wine，probe 失败可回退（小改动，收益最大）

- `Auto`/`Managed` 模式下 `probe()` 失败时，自动回退尝试 `System`（PATH 中的 wine64），并打出明确警告。
- 引导 NixOS 用户 `environment.systemPackages = [ pkgs.wineWow64Packages.staging ]`，白得 wine 11.14 + ntsync。
- 代价：缺 unix-pid maps 补丁（进程管理/双开受影响），无 FFXIV 专用补丁（本启动器场景推断无影响）。

### B. steam-run 兜底包装（中改动，最贴近上游体验）

- 下载的 wine-xiv `probe()` 失败且 PATH 里有 `steam-run` 时，自动改用 `steam-run <wine64> ...` 启动（等价于 nixpkgs xivlauncher 包的做法）。
- 需在 `WineTool::run` 支持"包装命令"（wine64_path 换成 steam-run，原 wine64 作为第一个参数），并 `env_remove("TZ")` 规避时区 bug。
- 用户零配置（Steam 玩家天然有 steam-run）。

### C. 文档/系统配置指引（零代码）

给 NixOS 用户三条自选路径写进 README/docs：

1. 装 nixpkgs wine + 本启动器用 `system` 模式（最 nix-native）；
2. 开 `programs.nix-ld.enable = true` + `programs.nix-ld.libraries` 列齐 wine 依赖，托管 wine 裸跑（即本机 devShell 正在走的路线，应把库清单从 flake.nix 的 `wineLibs` 同步成文档）；
3. 装 Steam（自带 steam-run）走路线 B。

### D.（备选，工作量大）flake 输出打包 wine

fetchurl wine-xiv + autoPatchelf 整棵树，或引 nix-gaming 的 wine-tkg。最 nix-native 但维护成本最高，暂不建议。

## NixOS 用户上手指南（现阶段，无需改代码）

三条路径任选，按推荐程度排序。

### 路径 1：nixpkgs 系统 wine（推荐，最 nix-native）

```nix
# configuration.nix
environment.systemPackages = [ pkgs.wineWow64Packages.staging ];
```

然后让启动器用系统 wine——编辑 `~/.xiv-launcher-rs/settings.json`：

```json
{ "startup_type": "system" }
```

nixos-unstable 的 staging 已是 wine 11.14，**ntsync 开箱可用**，只需加载内核模块并放权：

```nix
boot.kernelModules = [ "ntsync" ];
```

（`/dev/ntsync` 自内核 6.14 起默认权限就是 0666，无需额外 udev 规则——MODE=0666 的 udev 写法只在 6.13 早期内核上才需要。）

已知取舍：无 FFXIV 专用补丁（本启动器直连游戏，无影响）、缺 unix-pid maps 补丁（进程管理/双开场景受影响）。

### 路径 2：nix-ld + 托管 wine-xiv（保留官方补丁的裸跑方案）

系统级开启 nix-ld，并把 wine 运行时 dlopen 的库列全（与 `flake.nix` 的 `wineLibs` 同源）：

```nix
programs.nix-ld.enable = true;
programs.nix-ld.libraries = with pkgs; [
  freetype fontconfig gnutls libunwind
  vulkan-loader mesa
  libx11 libxext libxrender libxrandr libxi libxcursor libxinerama libxxf86vm libxcb
];
```

之后启动器默认 `Auto` 模式下载的 wine-xiv 即可裸跑。这正是本仓库 devShell 正在走的路线——`flake.nix` 的 `LD_LIBRARY_PATH` 只是开发期等价物，**前提是系统已开 nix-ld**，否则 wine64 连 interpreter 都找不到。

### 路径 3：steam-run 手动包装（想保留托管 wine 但不想动系统配置）

前提：装了 Steam（`programs.steam.enable = true`，自带 `steam-run`）。写一个 wrapper 脚本把托管 wine 包进 FHS 容器：

```sh
# ~/.xiv-launcher-rs/tools/wine64-steam-run
#!/bin/sh
unset TZ  # 规避 steam-run 容器内时区错乱（nixpkgs#279893）
exec steam-run "$HOME/.xiv-launcher-rs/tools/wine/bin/wine64" "$@"
```

`chmod +x` 后把 `settings.json` 设为：

```json
{ "startup_type": "custom", "custom_path": "~/.xiv-launcher-rs/tools/wine64-steam-run" }
```

等价于 nixpkgs `xivlauncher` 包的做法（其把整个 XIVLauncher 包进 steam-run）。注意 wine 需先由启动器下载一次（`Auto` 模式跑一次即可，失败没关系，文件已落盘）。

### 通用建议

```nix
hardware.graphics.enable32Bit = true;  # 32 位图形驱动（prefix 内 32 位组件保险）
```

游戏文件与上述所有方案无关，放在任意目录即可；prefix 默认为 `~/.xiv-launcher-rs/prefix`，可用 `settings.json` 的 `prefix` 字段改。

## 主要来源

- [nixpkgs xivlauncher 包](https://mynixos.com/nixpkgs/package/xivlauncher)（steam-run 包装 + unset TZ）
- [nix-ld](https://github.com/nix-community/nix-ld)、[nix.dev stub-ld FAQ](https://nix.dev/permalink/stub-ld)
- [NixOS wiki: Wine](https://nixos.wiki/wiki/Wine)、[wiki: OpenGL/Graphics](https://wiki.nixos.org/wiki/Graphics)
- [fufexan/nix-gaming](https://github.com/fufexan/nix-gaming)
- [nixpkgs#279893](https://github.com/NixOS/nixpkgs/issues/279893)（steam-run 时区 bug）
- ntsync 内核配置：[参考教程](https://wangwindow.pages.dev/posts/use-ntsync-in-wine-proton/)
