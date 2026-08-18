# 配置说明

eorzea 的全部配置落在 `~/.xiv-launcher-rs/`，GUI 与 CLI（`eoz`）共享同一份。本文档列出所有字段；字段的读写方式见 [`cli.md`](cli.md) 的 `eoz config` 一节。

## 文件总览

| 路径 | 内容 |
|------|------|
| `~/.xiv-launcher-rs/config.toml` | 主配置：游戏目录、大区、Wine、DXVK、Dalamud、自定义环境变量 |
| `~/.xiv-launcher-rs/auth.toml` | 已保存账号与自动登录 session key（由 `eoz auth` / GUI 登录页管理） |

### 旧配置迁移

- `config.toml` 不存在而旧版 `~/.xiv-launcher-rs/settings.json` 存在时，自动按新结构解析并写回 TOML（旧文件保留不删）。
- `auth.toml` 不存在而旧版 `~/.xiv-launcher-rs/eorzea.toml` 存在时，自动迁移到 `auth.toml`。
- 配置文件解析失败或缺字段时一律回退默认值（warn 日志），不会报错中断。

## config.toml

### 顶层字段

| 字段 | 类型 | 默认 | 说明 |
|------|------|------|------|
| `game_path` | path | 无 | 游戏根目录（含 `boot/`、`game/`、`sdo/`）。CLI 的 `--game-path` 提供时覆盖此项 |
| `area` | string | 无 | 默认大区 ID（如 `"1"` = 陆行鸟，`eoz areas` 查询）。CLI 的 `--area` 提供时覆盖此项 |

### Wine（顶层平铺）

Wine 字段通过 `#[serde(flatten)]` 直接写在 TOML 顶层（兼容旧格式），没有 `[wine]` 表。

| 字段 | 类型 | 默认 | 说明 |
|------|------|------|------|
| `startup_type` | string | `"auto"` | Wine 来源：`auto`（custom_path → XIVLauncher 托管 → 系统 → 下载）、`managed`（用/下载官方 wine-xiv）、`custom`（用 `custom_path`）、`system`（PATH 中的 wine） |
| `custom_path` | path | 无 | `custom` 模式下的 wine 可执行文件或含 `wine64`/`wine` 的目录（含 `bin/` 目录） |
| `prefix` | path | 无 | `WINEPREFIX`；缺省 `~/.xiv-launcher-rs/prefix` |
| `esync` | bool | `false` | 设置 `WINEESYNC=1` |
| `fsync` | bool | `false` | 设置 `WINEFSYNC=1` |
| `msync` | bool | `false` | 设置 `WINEMSYNC=1`（仅 macOS 生效） |
| `debug_vars` | string | 无 | `WINEDEBUG` 值，如 `"+seh"` |
| `gamemode` | bool | `false` | `LD_PRELOAD` 追加 `libgamemodeauto.so.0` |
| `[env]` | table | 空 | 任意 `env.FOO = "bar"`，作为环境变量传入 wine/游戏进程；最后应用，可覆盖上述所有项 |

### `[dxvk]`

| 字段 | 类型 | 默认 | 说明 |
|------|------|------|------|
| `dxvk.enabled` | bool | `true` | 安装/使用 DXVK（`d3d9,d3d11,d3d10core,dxgi=n`）；关闭回退 wined3d（`=b`） |
| `dxvk.hud` | string | 无 | `DXVK_HUD`，如 `"fps"`、`"full"`、`"0"` |
| `dxvk.frame_limit` | uint | 无 | `DXVK_FRAME_RATE` 帧率上限 |

### `[dalamud]`

| 字段 | 类型 | 默认 | 说明 |
|------|------|------|------|
| `dalamud.enabled` | bool | `false` | 启用 Dalamud（opt-in）。CLI 可用 `--dalamud` / `--no-dalamud` 单次覆盖 |
| `dalamud.load_method` | string | `"entrypoint"` | 加载方式：`entrypoint`（入口点改写，推荐）、`dllinject`（远程注入）、`aclonly`（只做兼容修复、不加载 Dalamud，排障用） |
| `dalamud.delay_initialize_ms` | uint | `0` | 注入后延迟初始化毫秒数 |
| `dalamud.no_plugins` | bool | `false` | safe mode：禁用全部插件 |
| `dalamud.no_third_party_plugins` | bool | `false` | 只禁用第三方插件 |
| `dalamud.manage_runtime` | bool | `false` | 强制由 launcher 托管 Windows x64 .NET runtime（即使 release 未声明需要） |
| `dalamud.track` | string | `"release"` | 更新通道（`release` / `staging` / 自定义） |
| `dalamud.beta_key` | string | 无 | staging 通道的 beta key |
| `dalamud.install_root` | path | 无 | Dalamud 安装根目录；缺省 `~/.xiv-launcher-rs/dalamud` |

### 完整示例

```toml
game_path = "/games/ffxiv"
area = "1"

# Wine（顶层平铺）
startup_type = "auto"
prefix = "/games/ffxiv/.wine"
esync = true
fsync = true

[dxvk]
enabled = true
hud = "fps"

[dalamud]
enabled = true
load_method = "entrypoint"

[env]
# 自定义环境变量，最后应用、可覆盖上面的所有项
WINEDLLOVERRIDES = "msquic=,mscoree=n,b"
```

## auth.toml

由 `eoz auth` 子命令或 GUI 登录页管理，一般不需要手改：

```toml
default_account = "你的账号名"        # 或 snda_id

[[accounts]]
snda_id = "1765973508"
username = "你的账号名"               # 可空
auto_login_session_key = "ULSa…"     # 可空；每次自动登录会刷新并写回
```

要点：

- 第一个保存的账号自动成为默认账号；`eoz auth default <账号>` 可更换（支持 snda_id 或 username）。
- 自动登录成功后服务器会下发**新的** session key（旧 key 立即作废），launcher 会立即写回 `auth.toml`，否则下次自动登录会过期。
- session key 有效期约 30 天（登录时会显示剩余天数）；过期后需重新 `eoz auth login qr` / `password`。
