# Auth 实现差异与存疑记录

> 本文档记录 Rust `eorzea-auth` 与上游 C# (`XIVLauncher.Core`) 之间的已知差异。
> 部分内容标注为 **"存疑"**，表示尚未通过实际登录验证是否会产生影响，留待启动功能实现后测试确认。

---

## SDO 设备指纹 (`sdo_device.rs` vs `SdoUtils.cs`)

### 现状概述

Rust 当前通过 `sdo_device.rs` 自行实现了设备指纹采集，C# 则依赖 `DeviceId` (MatthewKing) 库。两者在**输入字符串格式**上存在系统性差异，导致同一台机器生成的 MD5 哈希值不同。

### 差异详情（按平台）

#### Linux

| 项目 | C# (`DeviceId` 库) | Rust (`sdo_device.rs`) | 是否一致 |
|------|-------------------|----------------------|---------|
| `mac_address_hash` | MD5(`"MachineId=<uuid>"`) | MD5(`"<uuid>"`) | ❌ 不一致 |
| `mac_id` (raw) | `"MachineId=<uuid>"` | `"<uuid>"` | ❌ 不一致 |
| `cpu_id_hash` | MD5(`"ProcessorId=<model>,...,<model>"`) — 所有 core 不去重 | MD5(`"<model>"`) — 去重后仅保留 1 个 | ❌ 不一致 |
| `disk_serial_hash` | 动态识别根分区所在设备，读取 `SystemDriveSerialNumber=<serial>` | 动态识别根设备（`findmnt` 解析，含 btrfs 子卷）+ `lsblk -o SERIAL` + udev fallback | ⚠️ 输入格式仍不一致（无 `SystemDriveSerialNumber=` 前缀） |

> **2026-08 修复**：disk serial 采集曾有 `lsblk --no` 参数歧义 bug + 硬编码 `/dev/sda`（NVMe/btrfs 根会失败），导致指纹漂移触发风控（-16027517）。已改为 `findmnt` 动态识别根设备（处理 `/dev/sda3[/@root]` 子卷格式）+ `lsblk -o SERIAL`。

#### macOS

| 项目 | C# (`DeviceId` 库) | Rust (`sdo_device.rs`) | 是否一致 |
|------|-------------------|----------------------|---------|
| `mac_address_hash` | MD5(`"IOPlatformSerialNumber=<sn>,SystemDriveSerialNumber=<disk>"`) | MD5(`"<sn><disk>"`) | ❌ 不一致 |
| `mac_id` (raw) | `"IOPlatformSerialNumber=<sn>,SystemDriveSerialNumber=<disk>"` | `"<sn><disk>"` | ❌ 不一致 |
| `cpu_id_hash` | MD5(`"IOPlatformSerialNumber=<sn>"`) | MD5(`"<sn>"`) | ❌ 不一致 |
| `disk_serial_hash` | MD5(`"SystemDriveSerialNumber=<disk>"`) | MD5(`"<disk>"`) | ❌ 不一致 |

#### Windows

| 项目 | C# (`SdoUtils.cs`) | Rust (`sdo_device.rs`) | 是否一致 |
|------|-------------------|----------------------|---------|
| `mac_address_hash` | P/Invoke `iphlpapi.dll` → `GetAdaptersInfo` 取第一个非空 MAC → MD5 | **完全缺失**，fallback 为 `HOSTNAME-USER` 字符串 | ❌ 严重缺失 |
| `cpu_id_hash` | 内联 x86/x64 CPUID 汇编指令取原始 bytes → MD5 | **完全缺失**，fallback 为 `HOSTNAME-USER` 字符串 | ❌ 严重缺失 |
| `disk_serial_hash` | WMI `Win32_DiskDrive` 取 `SerialNumber` → MD5 | **完全缺失**，fallback 为 `HOSTNAME-USER` 字符串 | ❌ 严重缺失 |

### 关键发现

**C# 使用了 `DeviceId` 库的默认 `ToString()` 格式，该格式会在每个组件值前加上 `Key=` 前缀。**

例如：
- C# `GetMacAddress()` 实际 MD5 输入: `MachineId=a1b2c3...`
- Rust `get_mac_address_hash()` 实际 MD5 输入: `a1b2c3...`

这种前缀差异导致**即使原始硬件值相同，MD5 结果也会完全不同**。

### 潜在影响

`device_id` 和 `mac_id` 是 SDO 服务端风控的核心参数。如果 Rust 生成的指纹与 C# 版本不一致，可能：

1. 已用 C# 版本登录过的设备，切换到 Rust 后被识别为**新设备**；
2. 触发首次登录限制（`FirstLoginOnDevice`）；
3. 触发验证码要求（`CaptchaRequired`）。

### 存疑声明

> **⚠️ 存疑**：上述差异是否会造成实际登录失败，**尚未验证**。`device_id` 和 `mac_id` 的本质用途是设备唯一标识，只要 Rust 自身生成的指纹**稳定且唯一**，SDO 服务端理论上应允许新指纹注册。
>
> 因此，当前**不急于完全对齐 C# 的 `DeviceId` 字符串格式**。建议：
> - 保持 Rust 当前实现作为默认行为；
> - 待启动功能完整后，进行实际登录测试；
> - 若测试中发现风控拦截（如频繁要求验证码或拒绝密码登录），再评估是否需要复刻 C# 的 `Key=Value` 前缀格式。

### 已知待改进项（与一致性无关）

- **Linux 系统盘硬编码 `/dev/sda`**：现代系统常用 NVMe（`/dev/nvme0n1`）或云盘（`/dev/vda`、`/dev/xvda`），当前实现可能读取失败。建议改为动态识别根分区所在设备。
- **Windows 完全缺失**：目前全部走 fallback。若未来支持 Windows 平台，需补充 WMI/P/Invoke 实现。

---

## 网络调查补充（2026-05-13）

### SE 国际服登录流程（社区共识已高度公开）

FF14 国际服的登录协议在社区中已有非常成熟的逆向分析，主要参考来源：

- **notnite.com** — [FFXIV Explained - The Login Process](https://notnite.com/blog/ffxiv-login-process)
- **docs.xiv.zone** — [Logging into Official Servers](https://docs.xiv.zone/concept/logging-in-official)
- **project-novum.github.io** — `ffxivlogin.exe` 逆向分析
- **blog.sudeium.com** — [Launching FFXIV on macOS](https://blog.sudeium.com/2021/01/11/launching-final-fantasy-xiv-on-macos/)

#### 确认的关键细节

| 项目 | 社区共识 | 我们当前实现 |
|------|---------|-------------|
| OAuth Top URL 参数 | `lng=en&rgn=3&isft=0&cssmode=1&isnew=1&launchver=3` | ✅ 一致 |
| `_STORED_` 提取 | HTML 中 `<input name="_STORED_" value="...">` | ✅ 一致 |
| `login.send` POST | `sqexid`, `password`, `otppw` + `_STORED_` | ✅ 一致 |
| 响应解析 | `window.external.user("login=auth,ok,...")` | ✅ 一致 |
| **Register Session URL** | `https://patch-gamever.ffxiv.com/http/win32/ffxivneo_release_game/{ver}/{sid}` | ❌ **构造错误**（将 sid 放 URL 开头） |
| **版本报告** | `boot_hash\nex1\tver\nex2\tver\n...` 动态到 ex5 | ⚠️ 硬编码 ex1-ex3 |
| Steam ticket | 附加到 Top URL：`&issteam=1&session_ticket=...` | ❌ **完全缺失** |

#### Blowfish 启动参数加密的字节序问题

**blog.sudeium.com** 详细分析了 macOS 上的启动参数加密实现：

- **Key 生成**：`GetTickCount() & 0xFFFF0000`（Wine 在 macOS 上使用 `mach_absolute_time` 或 `mach_continuous_time`）
- **加密算法**：Blowfish ECB
- **Base64 变形**：`+→-`, `/→_`, `=→*`
- **格式**：`//**sqex{version:D04}{base64}{checksum}**//`

> **关键发现**：C# 的 `Blowfish.cs` 实现有**字节序问题**（endianness bug），与标准 Blowfish 不同。这解释了为什么标准 crypto 库（OpenSSL、CommonCrypto）产生的密文与 XIVLauncher 不匹配。**若需实现启动参数加密，必须复刻这个 bug，不能使用标准库。**

---

### SDO 国服登录（公开逆向资料非常稀少）

相比国际服，SDO 国服的协议细节在公开逆向社区中**极度匮乏**。目前可靠的参考来源只有：

1. C# 源码本身（`SdoLauncher.cs`、`SdoUtils.cs`）
2. 中文游戏资讯站（内容多为 AI 生成，可靠性低，充斥夸张信息如"声纹验证"、"区块链存证"等）

#### 可靠确认点

| API 端点 | 用途 | 我们的状态 |
|---------|------|-----------|
| `cas.sdo.com/authen/getGuid.json` | 获取会话 GUID | ✅ 已实现 |
| `cas.sdo.com/authen/staticLogin.json` | 密码登录 | ✅ 已实现 |
| `cas.sdo.com/authen/sendPushMessage.json` | 推送登录请求 | ✅ 已实现 |
| `cas.sdo.com/authen/pushMessageLogin.json` | 轮询推送状态 | ✅ 已实现 |
| `cas.sdo.com/authen/getCodeKey.json` | 获取二维码 | ✅ 已实现 |
| `cas.sdo.com/authen/codeKeyLogin.json` | 轮询扫码状态 | ✅ 已实现 |
| `cas.sdo.com/authen/autoLogin.json` | 自动登录（换新 key + 剩余期限） | ✅ 已实现 |
| `cas.sdo.com/authen/fastLogin.json` | **快速登录刷新**（autoLogin 后再刷 tgt/snda_id） | ✅ 已实现 |
| `cas.sdo.com/authen/ssoLogin.json` | TGT 换 ticket | ✅ 已实现 |
| `cas.sdo.com/authen/getPromotionInfo.json` | 激活 TGT 权限 | ✅ 已实现（含 `serviceUrl`） |
| `cas.sdo.com/authen/getAccountGroup` | 扫码后账号组查询（⚠️ 无 `.json` 后缀） | ✅ 已实现 |
| `cas.sdo.com/authen/accountGroupLogin` | 账号组登录刷新 TGT + session key | ✅ 已实现 |
| `cas.sdo.com/authen/thirdPartyLogin` | WeGame Token 登录 | ❌ **缺失** |

#### 关于设备指纹的风控

中文资讯站多次提到 SDO/盛趣的风控系统（代号"玄武"）会采集 MAC、硬盘序列号、CPU ID 等硬件信息。**但没有公开逆向资料详细说明其校验逻辑。**

可靠结论：
- `device_id` / MAC 标识有两套表示：`common_query()` 的 `macId` 发送 `SdoUtils.GetMac()` 对应的原始标识；密码登录的 `mac` 参数和 `CASCID` / `SECURE_CASCID` Cookie 使用其 MD5（`CID{MD5(mac)}`）
- `getPromotionInfo.json` 与其他 CAS 请求一样携带通用参数和完整 CAS Cookie；其 JSON `return_code` / `error_type` 必须校验，激活失败不能继续换取 ticket
- **没有证据表明 SDO 服务端会对指纹格式做严格校验**，更可能是将其作为"设备唯一标识"用于关联账号和检测异常登录
- 因此，只要 Rust 生成的指纹**在同一台机器上稳定不变**，就应该能正常工作

---

### `sqexPatch.dll` 逆向（redstrate.com）

参考：[redstrate.com](https://redstrate.com/blog/2025/07/i-figured-out-the-api-to-sqexpatch-dll/)

中韩启动器（盛趣/ActozSoft）都使用 `sqexPatch.dll` 进行补丁更新，关键发现：

1. **硬编码相同的 boot hash**：`ffxivboot.exe/149504/5f2a70612aa58378eb347869e75adeb8f5581a1b`
   - 这就是 C# 中 `CheckBootVersion` 返回空数组的原因（中韩启动器没有真正的 boot 组件）
2. **User-Agent 差异**：`sqexPatch.dll` 使用 `"FFXIV_Patch"`，而国际服使用 `"FFXIV PATCH CLIENT"`
3. **32 位构建**：中韩版本的 DLL 都是 32 位，尽管运行在 64 位系统上

---

### 高优先级修复建议

1. **`register_session` URL 构造**（`se.rs`）
   ```rust
   // 当前错误实现
   let url = format!("{}/http/win32/shanda_release_chs_game/{}", 
                     oauth_result.session_id, game_version);
   // 应为
   let url = format!("https://patch-gamever.ffxiv.com/http/win32/ffxivneo_release_game/{}/{}", 
                     game_version, oauth_result.session_id);
   ```

2. **版本报告动态生成**（`se.rs`）
   - 当前硬编码 `ex1-ex3`，应根据 `max_expansion` 动态生成到 `ex5`

3. **实现 `fastLogin.json`** ✅（`sdo.rs::fast_login`）
   - `autoLogin.json` 后用新 tgt 再刷新 snda_id/tgt，对齐 C# `LoginBySessionKey`

4. **实现扫码后的 `getAccountGroup` 和 `accountGroupLogin`** ✅（`sdo.rs::get_account_group` / `account_group_login`）
   - 扫码登录完整闭环已通；注意 `getAccountGroup` 端点**不带 `.json` 后缀**
   - 自动登录 key 续期机制：`autoLogin.json` 每次返回新 key（旧 key 作废）+ `autoLoginMaxAge` 剩余期限；`launch` 自动登录后保存新 key，实现无限续期

5. **Blowfish 字节序 bug（若需启动参数加密）**
   - 必须复刻 C# `Blowfish.cs` 的 endianness 行为，不能使用标准库

---

*文档更新时间: 2026-05-13*
