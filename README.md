# eorzea

FFXIV 国服（SDO/盛趣）启动器的 Rust 实现，移植自 `XIVLauncher.Core`（C#）的认证与游戏启动逻辑。

## 功能

- **登录**：扫码（叨鱼 App）、密码、session key 自动登录；多账号管理
- **游戏文件管理**：本地版本查看、补丁检查/下载/应用（ZiPatch）、完整性校验
- **启动**：Linux/macOS 通过 Wine 运行游戏（esync/fsync/msync、DXVK、gamemode、自定义环境变量）；Windows 原生启动 `ffxiv_dx11.exe`（Wine 相关配置在 Windows 上不参与构建与持久化）
- **Dalamud**：插件框架集成，版本门控 + 自动安装 + 安全降级
- **双前端**：dioxus-native GUI 与 `eoz` CLI，共享同一套配置（`~/.eorzea/`，按平台裁剪字段）

## 快速开始

```bash
# CLI（工作区默认成员，cargo run 直接跑 eoz）
cargo run -- --help

# GUI
cargo run -p eorzea-app

# 典型流程：设置游戏目录 → 扫码登录 → 启动
eoz config set game_path /games/ffxiv
eoz auth login qr
eoz launch
```

## 发布构建

推送 `v*` tag 会触发 `.github/workflows/release.yml`，构建三平台产物并创建 GitHub Release：
Linux x86_64（Ubuntu 22.04 构建）、Windows x86_64、macOS arm64（暂未真机验证），
每个压缩包内包含 GUI `eorzea` 与 CLI `eoz`。

Dev 构建不随 push `main` 自动触发：在 Actions → Release → Run workflow 手动运行，
默认发布/更新名为 **Dev Build YYYY-MM-DD** 的 pre-release（UTC 发布日期，固定 tag：`dev-build`，非稳定版）；
CI 自动将该 tag 移到本次成功构建的提交并替换附件，不影响 `dev` 分支。
固定下载入口为 [Development build](https://github.com/AzurIce/eorzea/releases/tag/dev-build)。
关闭 `publish_dev` 时只构建并上传 Actions artifact。这样平时的提交不会为 blitz/wgpu 编译买单。

## 目录结构

```text
├── src/                    # 核心库（eorzea_lib）：认证/配置/补丁/启动编排
│   ├── launcher.rs         # 登录 → 启动的编排层
│   ├── game.rs             # 游戏进程启动（Wine / Dalamud Injector）
│   ├── wine.rs             # Wine 解析、prefix 管理、DXVK 安装、环境变量（仅 Linux/macOS）
│   ├── config.rs           # config.toml 配置模型与持久化
│   ├── auth.rs             # auth.toml 账号存储
│   ├── dalamud/            # Dalamud：updater / runtime / assets / runner
│   └── game_files/         # 版本读取、补丁下载与应用、完整性校验
├── packages/
│   ├── eorzea-app/         # GUI（bin `eorzea`）：main.rs + ui/（login / home / settings）
│   ├── eorzea-auth/        # 认证库（feature gate：sdo 默认，se 可选）
│   └── eorzea-cli/         # CLI（bin `eoz`）
├── examples/               # 核心库示例二进制（sdo_launch、wine_test、hash_bench）
├── docs/                   # 用户文档
│   ├── cli.md              #   CLI 完整用法与各命令执行阶段
│   ├── config.md           #   全部配置字段说明
│   ├── wine.md             #   Wine / prefix / DXVK 详解
│   ├── dalamud.md          #   Dalamud 集成详解
│   └── notes/              # 开发笔记与调研（NixOS 指南、协议分析、C# 对照等）
└── TODO.md                 # 与上游 C# 实现的差异追踪
```

## 文档

- 配置手册（Notist）：[`book/config.not`](book/config.not)、[`book/auth.not`](book/auth.not)
- 日常使用与 CLI：[`docs/cli.md`](docs/cli.md)
- 配置字段：[`docs/config.md`](docs/config.md)
- Wine/DXVK 细节与排错：[`docs/wine.md`](docs/wine.md)
- Dalamud 细节与排错：[`docs/dalamud.md`](docs/dalamud.md)
- NixOS：[`docs/notes/nixos.md`](docs/notes/nixos.md)
- 开发笔记（登录协议、C# 对照分析等）：[`docs/notes/`](docs/notes/)

## 开发

Notist 已加入 flake 开发环境。构建或预览配置手册：

```bash
nix develop
notist build   # 输出到 target/book/
notist serve   # 本地预览，监听文档变化
```

```bash
cargo check                      # 默认成员（CLI + 核心库），不编译 GUI
cargo check --workspace          # 含 GUI（dioxus/blitz）
cargo run -p eorzea-app          # 运行 GUI
cargo check -p eorzea-auth       # 认证库（默认 feature：sdo）
cargo check -p eorzea-cli        # CLI
cargo test -p eorzea --lib       # 核心库单元测试
```

与上游 C# 实现的已知差异全部记录在 `TODO.md`；协议细节参考 `docs/notes/` 下的调研文档与 `~/Files/repos/XIVLauncher.Core`（`cn` 分支）。
