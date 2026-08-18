# eorzea

FFXIV 国服（SDO/盛趣）启动器的 Rust 实现，移植自 `XIVLauncher.Core`（C#）的认证与游戏启动逻辑。

## 功能

- **登录**：扫码（叨鱼 App）、密码、session key 自动登录；多账号管理
- **游戏文件管理**：本地版本查看、补丁检查/下载/应用（ZiPatch）、完整性校验
- **启动**：Wine 运行游戏（esync/fsync/msync、DXVK、gamemode、自定义环境变量）
- **Dalamud**：插件框架集成，版本门控 + 自动安装 + 安全降级
- **双前端**：dioxus-native GUI 与 `eoz` CLI，共享同一套配置（`~/.xiv-launcher-rs/`）

## 快速开始

```bash
# GUI
cargo run

# CLI
cargo run -p eorzea-cli -- --help

# 典型流程：设置游戏目录 → 扫码登录 → 启动
eoz config set game_path /games/ffxiv
eoz config set area 1
eoz auth login qr
eoz launch
```

## 目录结构

```text
├── src/                    # 核心库（eorzea_lib）+ GUI crate
│   ├── main.rs             # GUI 入口（dioxus-native）
│   ├── ui/                 # GUI 页面：login / home / settings
│   ├── launcher.rs         # 登录 → 启动的编排层
│   ├── game.rs             # 游戏进程启动（Wine / Dalamud Injector）
│   ├── wine.rs             # Wine 解析、prefix 管理、DXVK 安装、环境变量
│   ├── config.rs           # config.toml 配置模型与持久化
│   ├── auth.rs             # auth.toml 账号存储
│   ├── dalamud/            # Dalamud：updater / runtime / assets / runner
│   └── game_files/         # 版本读取、补丁下载与应用、完整性校验
├── packages/
│   ├── eorzea-auth/        # 认证库（feature gate：sdo 默认，se 可选）
│   └── eorzea-cli/         # CLI（bin `eoz`）
├── examples/               # 示例二进制（sdo_launch、wine_test）
├── docs/                   # 用户文档
│   ├── cli.md              #   CLI 完整用法与各命令执行阶段
│   ├── config.md           #   全部配置字段说明
│   ├── wine.md             #   Wine / prefix / DXVK 详解
│   ├── dalamud.md          #   Dalamud 集成详解
│   └── notes/              # 开发笔记与调研（NixOS 指南、协议分析、C# 对照等）
└── TODO.md                 # 与上游 C# 实现的差异追踪
```

## 文档

- 日常使用与 CLI：[`docs/cli.md`](docs/cli.md)
- 配置字段：[`docs/config.md`](docs/config.md)
- Wine/DXVK 细节与排错：[`docs/wine.md`](docs/wine.md)
- Dalamud 细节与排错：[`docs/dalamud.md`](docs/dalamud.md)
- NixOS：[`docs/notes/nixos.md`](docs/notes/nixos.md)
- 开发笔记（登录协议、C# 对照分析等）：[`docs/notes/`](docs/notes/)

## 开发

```bash
cargo check                      # 工作区（GUI + 核心库）
cargo check -p eorzea-auth       # 认证库（默认 feature：sdo）
cargo check -p eorzea-cli        # CLI
cargo test -p eorzea --lib       # 核心库单元测试
```

与上游 C# 实现的已知差异全部记录在 `TODO.md`；协议细节参考 `docs/notes/` 下的调研文档与 `~/Files/repos/XIVLauncher.Core`（`cn` 分支）。
