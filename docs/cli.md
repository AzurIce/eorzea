# `eoz` 命令行工具

`eoz` 是 eorzea（FFXIV 国服启动器）的命令行入口，覆盖大区查询、游戏文件管理、账号管理、Dalamud 集成以及一键登录启动。与 GUI 共享同一套配置（`~/.eorzea/`）。

## 目录

- [构建与安装](#构建与安装)
- [文件位置](#文件位置)
- [全局约定](#全局约定)
- [输出约定](#输出约定)
- [日志](#日志)
- [命令总览](#命令总览)
- [`eoz areas`](#eoz-areas)
- [`eoz game`](#eoz-game)
- [`eoz auth`](#eoz-auth)
- [`eoz launch`](#eoz-launch)
- [`eoz dalamud`](#eoz-dalamud)
- [`eoz config`](#eoz-config)
- [典型工作流](#典型工作流)
- [故障排查](#故障排查)

---

## 构建与安装

```bash
cargo build -p eorzea-cli                  # 仅构建
cargo run -p eorzea-cli -- --help          # 直接运行
cargo install --path packages/eorzea-cli   # 安装到 ~/.cargo/bin
```

产物为二进制 `eoz`。

## 文件位置

| 路径 | 用途 |
|------|------|
| `~/.eorzea/config.toml` | 主配置（字段详见 [`config.md`](config.md)） |
| `~/.eorzea/auth.toml` | 已保存账号与自动登录 session key |
| `~/.eorzea/patches/` | 补丁下载暂存目录 |
| `~/.eorzea/prefix/` | 默认 Wine prefix |
| `~/.eorzea/tools/` | 下载的 Wine、DXVK、登录 DLL 缓存 |
| `~/.eorzea/dalamud/` | Dalamud 安装目录（详见 [`dalamud.md`](dalamud.md)） |
| `~/.eorzea/logs/game-{ts}.log` | 每次启动的 wine/游戏输出 |

如果你通过 git 管理 dotfiles，只需要管理 `config.toml`。

## 全局约定

- `--game-path` 指向**游戏根目录**（含 `boot/`、`game/`、`sdo/`）；提供时覆盖 `config.toml` 的 `game_path`。
- `--area` 为大区 ID（如 `1` = 陆行鸟，`eoz areas` 查询）；提供时覆盖 `config.toml` 的 `area`。
- `eoz config` 的布尔值接受 `true/false`、`1/0`、`yes/no`、`on/off`。
- 游戏路径含非 ASCII 字符时启动前直接报错（避免游戏内误导性的 5003「帐号认证发生了错误」）。

## 输出约定 {#输出约定}

- **stdout 只放用户要的结果**（表格、状态行、TOML），**日志与错误走 stderr**；因此 `eoz config list > backup.toml`、`eoz areas | grep 豆豆柴` 这类重定向不会被日志污染。
- **默认不刷库日志**：`eoz` 自己的输出已经覆盖了用户在意的信息。要看过程用 `-v`（info）/`-vv`（debug）/`-vvv`（trace），或用 `RUST_LOG=…` 精确控制：
  ```bash
  eoz launch -v                    # 看 wine 解析 / prefix / DXVK / Dalamud 准备细节
  RUST_LOG=eorzea_lib::dalamud=debug eoz dalamud install
  ```
- **进度条**（indicatif）画在 stderr：显示阶段名、百分比、速度与剩余时间（ETA）。日志输出会自动挂起进度条，不会互相截断。
- **非 TTY / `--no-progress` 时自动退化**：不画动态进度条，改为每个阶段一行 `  阶段名…` + 结束时 `✓ 阶段名 就绪（12.78 MiB）`，CI 与管道里同样可读。
- 状态行约定：`==> 标题`（段落）、`✓`（成功）、`!`（警告）、`✗`（失败，同时退出码 1）、`·`（提示）。

```bash
eoz --no-progress game update     # 强制关掉进度条
```

---

## 日志

默认只显示 warn 及以上（且 `eorzea_*` 中已处理的降级路径只记 info，不会再刷屏）。`-v` 起按项目 crate 显示 info/debug/trace；`RUST_LOG` 优先级最高：

```bash
RUST_LOG=debug eoz launch        # 全量调试日志
RUST_LOG=warn eoz launch         # 只要警告和错误
```

wine/游戏本身的输出不打印到终端，每次启动写入 `~/.eorzea/logs/game-{unix_ts}.log`（启动成功时 CLI 会打印该路径）。

---

## 命令总览

```text
eoz areas                          # 列出大区
eoz game status                    # 显示本地游戏版本
eoz game check                     # 检查更新
eoz game update                    # 下载并应用补丁（带进度条）
eoz game verify                    # 完整性校验
eoz auth login qr|password|auto    # 登录并保存账号
eoz auth status / default / logout # 账号管理
eoz launch                         # 登录并启动游戏
eoz dalamud status / install / launch  # Dalamud 状态 / 预安装 / 强制启动
eoz config get|set|unset|list|path # 读写 config.toml
```

全局选项：`-v/--verbose`（可叠加）、`--no-progress`。

---

## `eoz areas`

从 SDO 服务器获取大区列表，输出大区 ID、`area_lobby`（ lobby 地址）与 `area_patch`（补丁服务器地址）——后两者是 `game check/update` 和 `launch` 的内部输入。

```bash
eoz areas
```

---

## `eoz game`

游戏文件管理，**全部免登录**（补丁接口不要求认证，UA 为 `FFXIV_Patch`）。

### `eoz game status`

读取本地版本文件并显示：

- `boot/ffxivboot.ver` → boot 版本
- `game/ffxivgame.ver` → 游戏本体（ffxiv）版本
- `game/sqpack/exN/exN.ver` → 资料片 ex1–ex5 版本

文件缺失或为空时显示基础版本 `2012.01.01.0000.0000`。

### `eoz game check`

检查是否有待更新补丁。执行阶段：

1. **读取本地版本**（同上；`--repair` 时强制按基础版本上报，服务器会返回全部补丁，用于修复/全新安装）。
2. **构造版本报告**：第一行为硬编码的 boot hash，随后每行 `exN<TAB>版本`（按 `--max-expansion` 截断，默认 5）。
3. **POST 补丁服务器** `http://{area_patch}/http/win32/shanda_release_chs_game/{当前版本}`，带 `X-Hash-Check: enabled` 头。
4. **解析响应**：空 body → 已是最新；否则解析 TSV 补丁列表（含每个补丁的 length、version、分块 SHA1、URL）。响应头 `X-Patch-Unique-Id` 是补丁会话 UID（当前服务器返回空，仅展示）。

选项：`--area`、`--max-expansion <n>`、`--repair`。

### `eoz game update`

下载并应用补丁。执行阶段：

1. **check**：同上，拿到补丁列表。
2. **并发下载**（默认 4 并发，信号量控制）到暂存目录；缓存文件名为 `<sha1(url)>-<文件名>.<版本>`，避免跨仓库撞名。已存在的缓存先按补丁元数据做**分块 SHA1 校验**，通过则跳过下载，失败则整体重下（断点续传未实现）。下载后校验字节数与 SHA1。
3. **应用补丁（ZiPatch）**：按列表顺序逐补丁把 chunk 序列写入 `boot/` 或 `game/` 对应仓库；应用前再次 SHA1 校验缓存身份。
4. **同步版本文件**：写对应的 `.ver`（如 `game/ffxivgame.ver`、`game/sqpack/ex1/ex1.ver`）。
5. **备份**：全部成功后把各仓库的 `.ver` 复制为 `.bck`。

选项：`--area`、`--max-expansion`、`--repair`、`--patch-dir <path>`（默认 `~/.eorzea/patches`）、`--concurrency <n>`（默认 4）。

### `eoz game verify`

文件级完整性检查（国服没有国际服的 `.patch.index`，因此不做全量内容哈希），随后附带一次免登录的版本状态检查（本地 vs 服务器最新）。

检查内容：

- `.ver` 版本文件存在且非空（ffxiv + ex1..5；异常记为 **Warning**）
- 关键文件存在：`game/ffxiv_dx11.exe`、`sdo/sdologin/sdologinentry64.dll`（缺失记为 **Missing**）
- 每个 sqpack 仓库（`game/sqpack/ffxiv` + `exN`）：`.win32.dat*`/`.win32.index*` 的大小、`SqPack` 魔数、header size（异常记为 **Corrupt**）
- `game/movie` 目录存在（缺失记为 **Warning**）

输出按 Missing / Corrupt / Warning 分级汇总，并给出修复建议（`eoz game update`）。

---

## `eoz auth`

多账号管理，持久化到 `auth.toml`。

### `eoz auth login <qr|password|auto>`

登录并保存账号；第一个保存的账号自动成为默认账号。三种方式的执行阶段：

- **qr**：向 SDO 请求二维码（PNG 保存到 `~/xiv_qr.png`，终端支持 kitty/iTerm2 图片协议时直接显示）→ 每 3 秒轮询扫码状态（300 秒超时）→ 扫码确认后走公共收尾（见下）。
- **password**：交互输入账号密码 → `staticLogin` 拿 `snda_id` + TGT → `getPromotionInfo` 激活 → `ssoLogin` 换 ticket。
- **auto**：用已有 session key `autoLogin`（服务器返回**新** key，旧 key 立即作废，launcher 会立即写回 `auth.toml`）→ `fastLogin` 刷新 TGT → 同上换 ticket。

扫码/推送的公共收尾：`getAccountGroup` 校验并解析显示名 → `accountGroupLogin` 刷新 TGT 并拿 30 天 `auto_login_session_key` → `getPromotionInfo` → `ssoLogin`。其中认证/风控类错误（验证码、设备首登、key 过期等）会立即中断，网络类瞬时错误降级继续。

选项：`--username`、`--session-key`、`--qr-file`。

### `eoz auth status` / `default` / `logout`

- `status`：列出账号、默认标记、是否有 session key（可否自动登录）。
- `default <账号>`：设置默认账号（snda_id 或 username 均可）。
- `logout [--account <账号>]`：删除账号，缺省删默认账号。

---

## `eoz launch`

登录并启动游戏。这是最复杂的命令，执行阶段如下（每步失败都有明确报错；启用 Dalamud 时**不会静默降级**）：

1. **大区解析**：获取大区列表，按 `--area`/配置找到目标大区（ID 写错在这里就报错，不会浪费一次登录）。
2. **Dalamud 准备**：启用时按需补齐 release / runtime / assets（下载显示进度条）；失败则**报错且不启动游戏**，提示用 `--no-dalamud` 本次跳过。必须在登录之前完成，否则失败会让一次性 session key 作废。
3. **登录**：未指定 `--method` 时，用 `--account` 或默认账号的 session key 自动登录（新 key 写回 `auth.toml`，并显示剩余有效期）；`--method qr|password|auto` 则本次手动登录（流程同 `eoz auth login`）。
4. **登录 DLL 检查**：确保 `sdo/sdologin/sdologinentry64.dll` 是 ottercorp 修改版——缺失则下载（缓存于 `~/.eorzea/tools/`），是原版则备份为 `sdologinentry64.sdo.dll` 后替换。
5. **Wine 准备**：按 `startup_type` 解析 Wine（必要时自动下载）→ `wine64 --version` 探针 → 确保 prefix 存在且为 64 位（详见 [`wine.md`](wine.md)）。
6. **DXVK 检查**：`dxvk.enabled` 时确保 DXVK 已装入 prefix（未装则下载安装）。
7. **组装启动参数与环境**：`-AppID=100001900 -AreaID=… Dev.LobbyHost01=… DEV.TestSID=<ticket> XL.SndaId=…` 等；环境变量由配置生成（`WINEDLLOVERRIDES`、esync/fsync、DXVK、`[env]` 自定义项等）。
8. **启动**：
   - 启用 Dalamud 且已就绪 → 路径经 `winepath` 转换，通过 `Dalamud.Injector.exe` 启动并等待其报告游戏 PID（30 秒超时）。
   - 未启用（`--no-dalamud` 或配置关闭）→ 直接 `wine64 ffxiv_dx11.exe …` 启动。

成功后输出游戏 PID 与日志文件路径；失败时打印首行摘要并提示 `-v` 看完整输出。

```bash
eoz launch                          # 默认账号自动登录 + 启动（推荐日常用法）
eoz launch --account 你的账号        # 指定已保存账号
eoz launch --method qr              # 本次手动扫码
eoz launch --method password --username 你的账号
eoz launch --wine /opt/wine/bin/wine64   # 本次覆盖 Wine
eoz launch --dalamud / --no-dalamud      # 本次覆盖 [dalamud].enabled
```

选项：`--game-path`、`--area`、`--account`、`--method`、`--username`、`--wine`、`--qr-file`、`--dalamud` / `--no-dalamud`。

---

## `eoz dalamud`

### `eoz dalamud status`

显示 release 元数据（版本、支持的游戏版本、runtime 要求）、本机安装版本、以及 `InstallState` 结论（Ready / Missing / OutOfDate / Unsupported / RuntimeMissing / AssetsMissing，含义见 [`dalamud.md`](dalamud.md)）。元数据请求失败时会退回本机 `version.json`，并明确标注来源。

### `eoz dalamud install`

预下载安装三组件（release 本体 → Windows .NET runtime → assets），每步一个进度条；已安装且校验通过的直接跳过。适合提前把组件备齐（离线/弱网时不必赌启动那一刻的下载）。失败即报错退出。

```bash
eoz dalamud install              # 用配置里的 game_path
eoz dalamud install --game-path /games/ffxiv
```

### `eoz dalamud launch`

等价于 `eoz launch --dalamud`：强制本次通过 Injector 启动；未就绪（含 release 尚未支持当前游戏版本）直接报错，不启动游戏。

---

## `eoz config`

类似 `git config` 的 `config.toml` 读写工具。全部字段的含义与默认值见 [`config.md`](config.md)。

```bash
eoz config get game_path                 # 读取
eoz config set game_path /games/ffxiv    # 设置（立即写回）
eoz config set dalamud.enabled true
eoz config set dxvk.hud fps
eoz config set env.WINEDLLOVERRIDES "msquic=,mscoree=n,b"
eoz config unset dalamud.enabled         # 删除（恢复默认值）
eoz config list                          # 列出当前生效配置（含默认值）
eoz config path                          # 显示配置文件路径
```

写盘前会做完整反序列化校验（类型/枚举错误会被拦下，不会写出坏配置）。可写键：顶层 `game_path`、`area`、全部 Wine 字段、`dxvk.*`、`dalamud.*`，以及任意 `env.<NAME>`。

---

## 典型工作流

### 首次使用

```bash
eoz config set game_path /games/ffxiv
eoz config set area 1
eoz game check          # 需要更新则 eoz game update
```

### 登录并启动（推荐）

```bash
eoz auth login qr       # 首次：扫码登录并保存账号
eoz launch              # 之后每次：自动登录 + 启动
```

### 启用 Dalamud

```bash
eoz config set dalamud.enabled true
eoz dalamud status      # 确认版本匹配
eoz launch              # 版本匹配时自动安装并加载
```

### 崩溃恢复

```bash
eoz config set dalamud.no_plugins true   # safe mode
eoz launch
```

---

## 故障排查

- **先看日志**：`eoz launch -v`（或 `RUST_LOG=debug eoz launch`）；wine/游戏输出在 `~/.eorzea/logs/game-{ts}.log`。
- **游戏路径找不到**：确认 `--game-path` 指向含 `boot/`、`game/`、`sdo/` 的根目录，或先 `eoz config set game_path …`。
- **自动登录失败**：session key 过期，重新 `eoz auth login qr` / `password`。
- **Dalamud 未就绪**：`eoz dalamud status` 看具体原因（`eoz launch` 此时会直接报错不启动游戏）；游戏刚更新时 release 未跟进是常态，用 `--no-dalamud` 先玩游戏，或先 `eoz dalamud install` 把组件备齐。
- **进度条花了/输出想喂给脚本**：`--no-progress` 关掉动态进度条；结果在 stdout、日志在 stderr，`eoz areas | grep …` 之类不受影响。
- **启动报 5003**：游戏路径含非 ASCII 字符，或登录 DLL 未正确替换。
- **Wine/DXVK 问题**（dxgi.dll not found、prefix 反复重建等）：见 [`wine.md`](wine.md) 故障排查。
