# 配置说明

Notist 配置手册分为 [`config.toml`](../book/config.not) 与 [`auth.toml`](../book/auth.not) 两页；在仓库根目录运行 `notist serve` 可预览。

eorzea 的全部配置落在 `~/.eorzea/`（Windows 上即 `C:\Users\<用户>\.eorzea\`），GUI 与 CLI（`eoz`）共享同一份。本文档列出所有字段；字段的读写方式见 [`cli.md`](cli.md) 的 `eoz config` 一节。

## 文件总览

| 路径 | 内容 |
|------|------|
| `~/.eorzea/config.toml` | 主配置：游戏目录、大区、补丁下载、Dalamud；Linux/macOS 另有 Wine、DXVK 与自定义环境变量 |
| `~/.eorzea/auth.toml` | 已保存账号与自动登录 session key（由 `eoz auth` / GUI 登录页管理） |

### 首次运行

`config.toml` 不存在（也没有可迁移的旧版 `settings.json`）时，GUI 或 `eoz` 第一次读取配置会自动创建一份**只含注释**的模板，列出 `game_path` 等常用字段。默认值仍由程序内置，模板不会把默认值写死（以后调整默认值时旧文件不会把它“钉住”）。在 GUI 或 `eoz config set` 保存后文件会被重写，模板注释不再保留。`auth.toml` 在首次登录并保存账号时才创建。

### 平台差异

配置文件按平台裁剪（`AppConfig` 的 `settings` 字段在 Windows 上不存在）：

| 平台 | `config.toml` 内容 |
|------|--------------------|
| Linux / macOS | `game_path`、可选的 `area`、`[download]`、平铺的 Wine 设置（`startup_type`/`custom_path`/`prefix`/`esync`/`fsync`/`msync`/`gamemode`、`[dxvk]`、`[env]`）、`[dalamud]` |
| Windows | `game_path`、可选的 `area`、`[download]`、`[dalamud]`；不写 Wine/DXVK/环境变量，`eoz config set startup_type` 等会报「未知配置键」 |

在 Windows 上读取 Linux/macOS 写出的配置时，Wine 相关键会被忽略；再保存会把它们移除。`eoz config list` 只输出当前平台的字段。

> Windows 上手写路径时注意 TOML 字符串转义：`"G:\FF14\..."` 是非法转义，应写成 `"G:/FF14/..."`（推荐）或 `"G:\\FF14\\..."`。用 GUI 设置页或 `eoz config set game_path` 会自动处理。

### 旧配置迁移

- `config.toml` 不存在而旧版 `~/.eorzea/settings.json` 存在时，自动按新结构解析并写回 TOML（旧文件保留不删）。
- `auth.toml` 不存在而旧版 `~/.eorzea/eorzea.toml` 存在时，自动迁移到 `auth.toml`。
- 配置文件解析失败或缺字段时一律回退默认值（warn 日志），不会报错中断。

## config.toml

### 顶层字段

| 字段 | 类型 | 默认 | 说明 |
|------|------|------|------|
| `game_path` | path | 无 | 游戏根目录（含 `boot/`、`game/`、`sdo/`）。CLI 的 `--game-path` 提供时覆盖此项 |
| `area` | string | 无 | **旧版全局默认大区**（如 `"陆行鸟"`，`eoz areas` 查询；仍兼容旧数字 ID）。默认大区已改为按账号保存在 `auth.toml` 的 `accounts[].area`，此字段仅在账号未设置时作为回退，GUI 不再编辑。CLI 的 `--area` 仍可单次覆盖 |

### `[download]`

GUI 设置页的「补丁下载」与 CLI 共用这组设置，保存后从下次更新任务生效。

| 字段 | 类型 | 默认 | 说明 |
|------|------|------|------|
| `download.concurrency` | uint | `4` | 最大同时下载补丁数，至少为 1；CLI `--concurrency` 单次覆盖 |
| `download.min_concurrency` | uint | `0` | 最少下载并发，不得超过最大值；CLI `--min-concurrency` 单次覆盖。0 保留预取窗口限制；非零允许突破窗口补任务，可能积压更多磁盘缓存。剩余补丁不足时不保证下限 |

优先级：显式 CLI 参数 > `config.toml` > 内置默认值；只覆盖传入的字段，再校验最终组合。

```toml
[download]
concurrency = 8
min_concurrency = 4
```

### Wine（顶层平铺）

Wine 字段通过 `#[serde(flatten)]` 直接写在 TOML 顶层（兼容旧格式），没有 `[wine]` 表。

| 字段 | 类型 | 默认 | 说明 |
|------|------|------|------|
| `startup_type` | string | `"auto"` | Wine 来源：`auto`（custom_path → XIVLauncher 托管 → 系统 → 下载）、`managed`（用/下载官方 wine-xiv）、`custom`（用 `custom_path`）、`system`（PATH 中的 wine） |
| `custom_path` | path | 无 | `custom` 模式下的 wine 可执行文件或含 `wine64`/`wine` 的目录（含 `bin/` 目录） |
| `prefix` | path | 无 | `WINEPREFIX`；缺省 `~/.eorzea/prefix` |
| `esync` | bool | `false` | 设置 `WINEESYNC=1`；与 `fsync` 二选一（GUI 用「关 / esync / fsync」三档滑块，内部是 `SyncMode` 枚举） |
| `fsync` | bool | `false` | 设置 `WINEFSYNC=1`；与 `esync` 二选一，同时为 true 时按 `fsync` 读取 |
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
| `dalamud.enabled` | bool | `false` | 启用 Dalamud（opt-in）。CLI 可用 `--dalamud` / `--no-dalamud` 单次覆盖。**启用即强制**：准备失败（元数据不可达、与游戏版本不匹配、下载/校验失败、runtime 或 assets 缺失）会直接报错且不启动游戏，需要显式 `--no-dalamud`（GUI 用主页「本次启动加载 Dalamud」开关）才能不带插件进游戏 |
| `dalamud.load_method` | string | `"entrypoint"` | 加载方式：`entrypoint`（入口点改写，推荐）、`dllinject`（远程注入）、`aclonly`（只做兼容修复、不加载 Dalamud，排障用） |
| `dalamud.delay_initialize_ms` | uint | `0` | 注入后延迟初始化毫秒数 |
| `dalamud.no_plugins` | bool | `false` | safe mode：禁用全部插件 |
| `dalamud.no_third_party_plugins` | bool | `false` | 只禁用第三方插件 |
| `dalamud.manage_runtime` | bool | `false` | 强制由 launcher 托管 Windows x64 .NET runtime（即使 release 未声明需要） |
| `dalamud.track` | string | `"release"` | 更新通道（`release` / `staging` / 自定义） |
| `dalamud.beta_key` | string | 无 | staging 通道的 beta key |
| `dalamud.install_root` | path | 无 | Dalamud 安装根目录；缺省 `~/.eorzea/dalamud` |

### 完整示例

```toml
game_path = "/games/ffxiv"

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

Windows 上不需要也不接受 Wine 字段，配置包含游戏路径、补丁下载与 Dalamud（默认大区按账号保存）：

```toml
game_path = "G:/FF14/最终幻想XIV"

[dalamud]
enabled = true
```

## auth.toml

由 `eoz auth` 子命令或 GUI 登录页管理，一般不需要手改：

```toml
default_account = "你的账号名"        # 或 snda_id

[[accounts]]
snda_id = "1765973508"
username = "你的账号名"               # 可空
auto_login_session_key = "ULSa…"     # 可空；每次自动登录会刷新并写回
area = "陆行鸟"                       # 可空；该账号的默认大区（名称）
```

要点：

- 第一个保存的账号自动成为默认账号；`eoz auth default <账号>` 可更换（支持 snda_id 或 username）。
- `area` 是该账号的默认大区。GUI 主页的「大区」下拉框直接读写这个字段；切换账号时自动套用该账号的大区。未设置时回退 `config.toml` 的 `area`，再回退到大区列表第一个。CLI 的 `--area` 仍可单次覆盖。
- 自动登录成功后服务器会下发**新的** session key（旧 key 立即作废），launcher 会立即写回 `auth.toml`，否则下次自动登录会过期。
- session key 有效期约 30 天（登录时会显示剩余天数）；过期后需重新 `eoz auth login qr` / `password`。
