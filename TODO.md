# xiv-launcher-auth 与 XIVLauncher.Core 差异追踪

> 本文件记录 Rust `auth` 子 crate 与上游 C# (`XIVLauncher.Core`) 实现之间的所有已知差异。
> 当前聚焦 **SDO（中国服/盛趣）** 登录功能。`se`（国际服）为低优先级。

---

## P0 — SDO 核心登录链路（必须先通）

### P0-1 设备指纹生成 (`SdoUtils.cs`)

- [x] **实现 `generate_device_id()`**：格式为 `{MD5_UPPER(MAC)}:{MD5_UPPER(CPU_ID)}:{MD5_UPPER(磁盘序列号)}` — `sdo_device::get_device_id()`
- [x] **实现 `generate_mac_id()`**：格式为 `MD5_UPPER(MAC地址)` — `sdo_device::get_mac_address_hash()`
- [x] **跨平台硬件信息采集**：macOS 用 `ioreg` + `diskutil`，Linux 用 `/etc/machine-id` + `/proc/cpuinfo` + `lsblk`
- [x] **集成到 `SdoAuth`**：`SdoAuth::new()` 自动采集，`common_query()` 使用自身存储的值，`SdoContext` 不再需要调用方传入

> C# 参考：`SdoUtils.cs` — `GetMacAddress()`, `GetCPUId()`, `GetDiskSerialNumber()`, `GetDeviceId()`, `GetMD5()`
>
> **⚠️ 存疑**：Rust 与 C# 的 MD5 输入字符串格式存在系统性差异（C# `DeviceId` 库默认加 `Key=` 前缀，Rust 没有），导致同一台机器指纹不同。详见 [`docs/auth.md`](./docs/auth.md)。
> 待启动功能实现后通过实际登录测试验证：若 SDO 服务端仅要求"稳定唯一"则无需修复；若触发风控（频繁要求验证码/拒绝密码登录），则需复刻 C# 的 `Key=Value` 格式。

### P0-2 SDO HTTP 请求细节 (`SdoLauncher.cs` → `GetSdoHttpRequestMessage`)

- [x] **自动注入 Cookie**：每个请求注入 `CASCID=CID{MD5(MAC)}` 和 `SECURE_CASCID=CID{MD5(MAC)}` 及 `_rsid=""`（通过 `build_cookie_header()` 方法）
- [x] **`getPromotionInfo` 补全 `serviceUrl` 参数**：已添加 `serviceUrl=http%3A%2F%2Fwww.sdo.com`
- [x] **请求头 `Host`**：所有 SDO 请求添加 `Host: cas.sdo.com`

### P0-3 SDO 错误处理完善

- [x] **错误码保留**：`AuthError::SdoError { code, message, remove_auto_login }` 现在保留原始 `return_code`
- [x] **自动登录过期标记**：`SdoError.remove_auto_login` 字段标记是否需删除本地 session key
- [x] **`SdoLoginData.ErrorType` 字段**：已添加 `error_type: Option<i32>`

### P0-4 域名故障转移

- [x] **`with_fallback_url()` 方法**：可在创建后切换到备用域名 `n1.cas.sdo.com`

---

## P1 — SDO 扫码/推送/自动登录完善

### P1-1 扫码登录后处理 (`SdoLauncher.cs` → `QrCodeLogin`)

- [ ] **`getAccountGroup` 调用**：扫码成功后需调用 `getAccountGroup.json?tgt={tgt}&serviceUrl=http://www.sdo.com` 获取账号列表
- [ ] **`accountGroupLogin` 调用**（自动登录时）：传入 `sndaId` 和 `autoLoginKeepTime` 获取新的 tgt
- [ ] **选择 `sndaId`**：如果账号组有多个 sndaId，需要让用户选择或使用默认
- [x] **QR 扫码完整链路**：`qr_code_request` → 轮询 → `sso_login(tgt)` → ticket 获取已验证通过

### P1-2 推送/滑动登录超时与取消

- [ ] **超时机制**：QR 码 300s 超时，推送 30s 超时。当前全由调用方控制，应提供 `tokio::time::timeout` 包装
- [ ] **取消推送**：`slide_login_request` 之前应该调用 `cancelPushMessageLogin`，当前已实现但逻辑应更健壮

### P1-3 WeGame 登录

- [ ] **`thirdPartyLogin` 实现**：`companyid=310`, 传入 WeGame `userId` 和 `token`
- [ ] **WeGame SID 直接构造**：跳过 SDO 认证，直接构造 `OauthLoginResult`

---

## P2 — 游戏启动参数构造

### P2-1 国服启动参数 (`SdoLauncher.cs` → `LaunchGameSdo`)

- [x] **启动参数构造**：`game.rs` 中 `build_sdo_launch_args()` 实现完整参数拼接
- [x] **参数分隔符修正**：C# `ArgumentBuilder.Build()` 使用 `key=value` 格式（不是空格分隔）
- [x] **`areasInfo` 计算**：`build_lobby_hosts()` 将所有大区 `lobby:54994` 用 `|` 分隔
- [x] **`XL.DcTraveler` 参数**：当 `dc_travel_port > 0` 时添加
- [x] **端到端验证**：`sdo_login` 示例完成 QR 扫码 → `sso_login` → ticket 获取 → 启动参数构造
- [x] **`sdologinentry64.dll` 替换**：`EnsureLoginEntry()` — 自动从 ottercorp GitHub 下载修改版 DLL，缓存到 `~/.xiv-launcher-rs/tools/`，复制到 `{gamePath}/sdo/sdologinentry64.dll`

### P2-2 参数加密 (`ArgumentBuilder.cs` → `BuildEncrypted`)

- [ ] **Blowfish ECB 实现**：需实现 SE 的变体 Blowfish（带 signed byte bug）
- [ ] **启动参数加密流程**：
  1. 取 `Environment.TickCount` 作 key
  2. 构建 ` /key =value` 格式参数串
  3. LegacyBlowfish ECB 加密
  4. SE Base64 变形编码 (`+→-`, `/→_`, `=*`)
  5. 计算校验字符
  6. 结果格式：`//**sqex{version:D04}{base64}{checksum}**//`

### P2-3 版本检查与补丁 (`Launcher.cs` → `CheckGameUpdate`)

- [x] **版本报告生成**：`game_files/version.rs` 的 `build_version_report()` 按 `max_expansion` 动态生成 ex1-ex5（不再硬编码），对齐 C# `GetVersionReport()`（首行为硬编码 boot hash，国服同 C# FIXME 值）
- [x] **SDO 版本检查协议**：`game_files` 的 `check_update()` 实现 `CheckGameUpdate`（POST `{area_patch}/http/win32/shanda_release_chs_game/{ver}`，`X-Hash-Check` 头，解析 `X-Patch-Unique-Id` + TSV），免登录，已通过真实 API 验证（返回 185 补丁/126 GiB）
- [x] **补丁列表解析**：`game_files/patch_list.rs` 实现 `PatchListParser`（跳过前 5 行、9 字段带 hash / 6 字段 boot），`PatchListEntry` 扩展为完整字段（`hash_type`/`hash_block_size`/`hashes[]`）
- [x] **补丁下载管理**：`game_files/patch_manager.rs` 实现下载管线（并发 4 槽同 C# `MAX_DOWNLOADS_AT_ONCE`、SHA1 块校验同 `CheckPatchValidity`、已校验文件跳过、进度回调）；SHA1 块算法已用真实补丁验证通过
- [x] **xlcli 命令行**：`src/bin/xlcli.rs`（clap）— `areas` / `game status` / `game check` / `game update`（下载 + 应用）
- [x] **补丁应用**（ZiPatch）：`src/game_files/zpatch/` 完整移植 C# `ZiPatch` 解析与应用（FHDR/APLY/SQPK:T/F/A/D/E/H/I/X/ADIR/DELD/EOF），`RemotePatchInstaller` 流程（应用 → `SetVer` → `VerToBck`）；**已通过真实游戏目录端到端验证**（17 补丁 1.17 GiB 全部应用成功，再次 check 返回已最新）
- [ ] **Boot 版本检查**：国服无需实现（C# `CheckBootVersion` 对 CN 直接 `return Array.Empty`）
- [ ] **完整性校验**（`PatchVerifier`）：未实现，需 IndexedZiPatch 索引或逐文件 hash
- [ ] **断点续传**（Range）：当前不完整文件整体重下，部分下载续传待加
- [ ] **UID 缓存**：缺失 `IUniqueIdCache` 实现（`X-Patch-Unique-Id` 缓存）

---

## P3 — 国际服 (SE) 补全（低优先级）

### P3-1 SE OAuth 完善 (`se.rs` vs `Launcher.cs`)

- [ ] **`Referer` 头缺失**：OAuth Top 和 Login.send 请求应带 `Referer` 头（从 frontier URL 模板生成）
- [ ] **Steam 登录支持**：完全缺失 — Steam ticket 获取、Blowfish ECB 加密、CrtRand、SE Base64 变形
- [ ] **`login()` 流程**：当前返回 `LoginResult` 但未自动调用 `register_session`
- [ ] **`make_computer_id` 编码问题**：Rust 用 UTF-8 算 SHA1，C# 用 `Encoding.Unicode`（UTF-16 LE），生成的 ID 完全不同

### P3-2 Steam 加密 (`Ticket.cs`, `CrtRand.cs`)

- [ ] **CrtRand 伪随机**：MSVC CRT `rand()` 的 Rust 实现（seed = time ^ ticket_sum）
- [ ] **Steam Auth Session Ticket 加密**：完整实现 `Ticket.EncryptAuthSessionTicket`
- [ ] **SE Base64 变形**：已实现 `to_mangled_se_base64()`，但上传时需按 300 字符分块用逗号连接

---

## P4 — 代码质量与健壮性

### P4-1 日志与调试

- [ ] **敏感字段脱敏**：ticket、tgt、sessionId、sndaId 等在日志中应做掩码处理（如 `abc***xyz`）
- [ ] **请求/响应日志**：`log::debug!` 级别记录 URL 和返回码，方便调试

### P4-2 测试

- [ ] **`crypto.rs` 单元测试**：`make_computer_id` 的 UTF-16 LE 编码修复后需对照 C# 输出验证
- [ ] **`SdoArea` 反序列化测试**：用真实 serverlist_new.js 内容做 fixture 测试
- [ ] **SE OAuth HTML 解析测试**：用 fixture HTML 验证 `_STORED_` 和 `login=auth,ok,...` 提取

### P4-3 错误处理

- [ ] **`SdoLoginData` 补全 `AccountArray` / `SndaIdArray`**：扫码成功后返回的账号列表
- [ ] **DC 跨服传送 API** (`DcTraveler.cs`)：`ff14bjz.sdo.com` 下的旅行/回归 API，暂不在 P0 范围内

---

## 已完成

- [x] SDO 密码登录 (`static_login`)
- [x] SDO 推送登录 — 请求 (`slide_login_request`)
- [x] SDO 推送登录 — 轮询 (`slide_login_poll`)
- [x] SDO 扫码登录 — 获取二维码 (`qr_code_request`)
- [x] SDO 扫码登录 — 轮询 (`qr_code_poll`)
- [x] SDO 自动登录 (`auto_login`)
- [x] SDO SSO — TGT 换 ticket (`sso_login`)
- [x] SDO 激活权限 (`get_promotion_info`)
- [x] SDO 服务器列表获取 (`fetch_server_list`) — 已通过真实 API 验证
- [x] SE OAuth Top — 获取 `_STORED_` token
- [x] SE OAuth Login — 提交用户名/密码/OTP
- [x] SE Register Session — 版本检查与补丁检测
- [x] Feature gate (`sdo` default, `se` optional)
- [x] Cargo workspace 重构（根 crate = tauri，`packages/` 下子 crate）
- [x] Rust doc 注释覆盖所有公开 API
- [x] **`SdoLoginData.SndaId` 字段大小写修复**：`model.rs` 中 `snda_id` 和 `snda_id_array` 添加 `alias` 同时支持 `SndaId`/`sndaId` 两种大小写（C# 参考使用小写 `sndaId`）

---

## 备注

- 以上条目按优先级分组：P0 为核心登录链路必须打通的，P1 为扫码/推送完善，P2 为游戏启动所需，P3 为国际服（低优先级），P4 为质量提升。
- 每次修复后应在对应条目旁标注完成日期和 commit hash，保持 TODO.md 与实际代码同步。
- C# 参考文件路径均相对于 `XIVLauncher.Core/lib/FFXIVQuickLauncher/src/XIVLauncher.Common/Game/`。