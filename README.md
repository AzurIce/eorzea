# eorzea

使用 Rust 编写的《最终幻想 XIV》国服（盛趣）第三方启动器，提供图形界面和命令行工具。

## 功能

- 叨鱼扫码、密码和自动登录，支持多账号管理
- 游戏更新、补丁下载与文件完整性校验
- Windows 原生启动，Linux / macOS 通过 Wine 启动
- Dalamud 插件框架集成
- 图形界面与命令行工具共享账号和配置

## 下载

前往 [Releases](https://github.com/AzurIce/eorzea/releases) 下载对应平台的压缩包，解压后运行 `eorzea-app`（图形界面）或 `eoz`（命令行）。

提供 Linux x86_64、Windows x86_64 和 macOS arm64 构建。macOS 尚未经过真机验证。

最新开发构建见 [Development build](https://github.com/AzurIce/eorzea/releases/tag/dev-build)，可能不稳定。

## 快速开始

### 图形界面

运行 `eorzea-app`，在设置中选择游戏目录，添加账号并登录，然后启动游戏。

### 命令行

将 `eoz` 加入 `PATH`，或在下列命令中使用它的完整路径：

```bash
# 设置游戏根目录（包含 boot、game、sdo 文件夹）
eoz config set game_path /games/ffxiv

# 使用叨鱼 App 扫码登录
eoz auth login qr

# 更新并启动游戏
eoz game update
eoz launch
```

运行 `eoz --help` 查看全部命令。账号与配置保存在 `~/.eorzea/`。

Linux / macOS 通过 Wine 运行游戏，配置与排错请参阅 [Wine 使用说明](docs/wine.md)。Apple Silicon Mac 还需要安装 Rosetta 2。

## 从源码运行

安装 Rust 工具链后，在仓库目录中执行：

```bash
# 图形界面
cargo run -p eorzea-app

# 命令行
cargo run -p eorzea-cli -- --help
```

## 文档

- [命令行使用](docs/cli.md)
- [配置说明](docs/config.md)
- [Wine 与 DXVK](docs/wine.md)
- [Dalamud](docs/dalamud.md)
- [NixOS](docs/notes/nixos.md)
- [已知问题与待办](TODO.md)

## 致谢

认证与游戏启动逻辑参考了 [XIVLauncher.Core](https://github.com/goatcorp/XIVLauncher.Core)。本项目与 SQUARE ENIX、盛趣游戏无隶属关系。
