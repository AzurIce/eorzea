# SDO 登录流程详解

> 本文档梳理国服（SDO/盛趣）登录涉及的**核心概念**（字段）、**API 清单**和各登录方式的**完整流程**。
> 对应代码：`packages/eorzea-auth/src/sdo.rs`；C# 参考：`SdoLauncher.cs`。
> 标注 ✅ 的流程已通过真实登录验证。

## 一、核心概念：登录过程中的各个"东西"是什么

### 设备标识（每次请求附带，风控用）

| 字段 | 含义 | 来源 |
|---|---|---|
| `guid` | 会话 GUID（形如 `03C060559BDD48E3901AAC90B3388D27unilinuxmc`，带 `unilinuxmc` 后缀表示 Linux 客户端） | `getGuid.json` 生成，登录流程的第一步 |
| `device_id` | 设备指纹：`{MD5(MAC)}:{MD5(CPU)}:{MD5(硬盘序列号)}` | `sdo_device.rs` 自动采集 |
| `mac_id` | 首个非空网卡 MAC 的 MD5 | 同上 |

### 登录凭证链（tgt → ticket 的转换）

| 字段 | 含义 | 特征前缀 | 生命周期 |
|---|---|---|---|
| `tgt` | **临时授权票据**（Ticket Granting Ticket）。登录（密码/扫码/推送）成功后获得，是换取游戏 ticket 的中间凭证 | `ULSTGT-` | 短时有效，登录会话期间 |
| `ticket` | **游戏会话 ID**（`DEV.TestSID` 启动参数），最终要拿到的凭证 | `ULS21-` | 启动游戏时使用，一次登录一次 |

### 账号标识

| 字段 | 含义 | 说明 |
|---|---|---|
| `snda_id` | **盛趣账号 ID**（数字），账号的唯一标识 | 三种登录方式都能拿到；`eoz auth` 用它作为账号 key |
| `username`（`input_user_id`） | 登录账号名（如手机号/邮箱），用于展示 | 扫码响应的 `inputUserId` 字段；密码登录即输入的用户名 |

### 自动登录（免密）相关

| 字段 | 含义 | 特征前缀 | 说明 |
|---|---|---|---|
| `auto_login_session_key` | **免密登录密钥**，保存后下次可跳过登录直接进游戏 | `ULSed-` | 由 `codeKeyLogin`（扫码）/ `accountGroupLogin` / `pushMessageLogin`（推送）返回；`eoz auth` 持久化到配置 |
| `auto_login_max_age` | session key 有效期（**秒**） | — | 实测 720 小时 = 30 天（`autoLoginKeepTime=30` 天） |
| `code_key` | 二维码标识（扫码登录轮询用） | — | 从 `getCodeKey.json` 响应的 `Set-Cookie: CODEKEY=...` 提取 |

## 二、API 清单（全部 GET，`https://cas.sdo.com/authen/{endpoint}`）

通用参数（每个请求都带，`common_query()`）：
`authenSource=1&appId=100001900&areaId=1&appIdSite=100001900&locale=zh_CN&productId=4&frameType=1&endpointOS=1&version=21&customSecurityLevel=2&deviceId={device_id}&thirdLoginExtern=0&macId={mac_id}&productVersion=1.9.7.10&tag=0`

| API | 用途 | 关键参数 | 返回（关键字段） |
|---|---|---|---|
| `getGuid.json` | 获取会话 GUID | `generateDynamicKey=1` | `guid` |
| `getCodeKey.json` | 获取二维码 | `maxsize=89` | 二维码图片 + `Set-Cookie: CODEKEY={code_key}` |
| `codeKeyLogin.json` | 轮询扫码状态 | `codeKey`、`guid`、`autoLoginFlag=1`、`autoLoginKeepTime=30` | `-10515805`=未扫；`0`=已扫 → `snda_id`、`tgt`、`auto_login_session_key`、`input_user_id` |
| `getAccountGroup` | **校验账号组**（⚠️ 无 `.json` 后缀） | `serviceUrl=http://www.sdo.com`、`tgt` | `snda_id_array`、`account_array` |
| `accountGroupLogin.json` | 账号组登录：刷新 tgt + 拿新的 session key | `serviceUrl`、`tgt`、`sndaId`、`autoLoginFlag=1`、`autoLoginKeepTime=30` | `tgt`（新）、`auto_login_session_key` |
| `staticLogin.json` | 密码登录 | `inputUserId`、`password`、`guid`、`autoLoginFlag=0` | `snda_id`、`tgt`；风控时返回 `-10386188`(验证码)/`-10242296`(首登设备) |
| `sendPushMessage.json` | 发送推送（叨鱼一键登录） | `inputUserId` | `push_msg_serial_num`、`push_msg_session_key` |
| `pushMessageLogin.json` | 轮询推送确认 | `pushMsgSessionKey`、`guid`、`autoLoginFlag=1` | `-10516808`=未确认；`0`=已确认 → `snda_id`、`tgt`、`auto_login_session_key` |
| `cancelPushMessageLogin.json` | 取消推送 | `pushMsgSessionKey`、`guid` | — |
| `autoLogin.json` | 自动登录（用 session key 换新 key + 剩余期限） | `autoLoginSessionKey`、`guid` | `snda_id`、`tgt`（新）、`auto_login_session_key`（新，旧 key 作废）、`autoLoginMaxAge`；`-10515005`=过期 |
| `fastLogin.json` | autoLogin 后再刷新 tgt/snda_id | `tgt`、`guid` | `snda_id`（新）、`tgt`（新） |
| `getPromotionInfo.json` | 激活 TGT 登录权限（必须调用） | `tgt`、`serviceUrl=http://www.sdo.com` | — |
| `ssoLogin.json` | tgt 换 ticket（最终步骤） | `tgt`、`guid` | `ticket`（即游戏 session_id） |

## 三、各登录方式完整流程

### 1. 扫码登录 ✅（推荐，`eoz auth login qr`）

```text
getGuid.json ──▶ 得到 guid
      │
      ▼
getCodeKey.json ──▶ 二维码图片 + code_key（终端直接显示/保存 PNG）
      │
      ▼ （用户用叨鱼 App 扫码，每 ~2-3s 轮询）
codeKeyLogin.json ──▶ 已扫：snda_id + tgt + auto_login_session_key + input_user_id
      │
      ├── getAccountGroup（校验账号，无 .json 后缀）✅
      ├── accountGroupLogin.json（刷新 tgt + 新 session key）✅
      │
      ▼
getPromotionInfo.json ──▶ 激活权限
      ▼
ssoLogin.json ──▶ ticket（DEV.TestSID）
```

- **session key 来源**：`codeKeyLogin` 响应直接带（实测 `ULSed64c...`）；`accountGroupLogin` 会再刷新一次（两者都保存，后者优先）
- **tgt 更新**：`accountGroupLogin` 返回**新 tgt**，后续 `ssoLogin` 必须用新值

### 2. 密码登录（`eoz auth login password`）

```text
getGuid.json ──▶ staticLogin.json（账号+密码）──▶ tgt + snda_id
      │                                （风控：-10386188 验证码 / -10242296 首登设备）
      ▼
getPromotionInfo.json ──▶ ssoLogin.json ──▶ ticket
```

### 3. 推送/滑动登录（叨鱼一键登录）

```text
getGuid.json ──▶ cancelPushMessageLogin.json（清理旧推送）
      ▼
sendPushMessage.json ──▶ push_msg_serial_num（显示给用户确认码）+ push_msg_session_key
      ▼ （每 ~1s 轮询）
pushMessageLogin.json ──▶ 已确认：snda_id + tgt + auto_login_session_key
      ▼
getPromotionInfo.json ──▶ ssoLogin.json ──▶ ticket
```

### 4. 自动登录（`eoz auth login auto` / `launch` 默认账号）

```text
getGuid.json ──▶ autoLogin.json（autoLoginSessionKey=保存的 key）
      │                       （-10515005 = key 过期，需重新登录）
      ▼
snda_id + tgt（新）+ auto_login_session_key（新！旧 key 立即作废）+ autoLoginMaxAge（剩余期限）
      │
      ▼
fastLogin.json（新 tgt 再刷新 snda_id/tgt）──▶ getPromotionInfo.json ──▶ ssoLogin.json ──▶ ticket
```

- **key 续期机制**（对应 C# `UpdateAutoLoginSessionKey`）：每次 `autoLogin` 服务端发放**新 key**，旧 key 立即作废；`autoLoginMaxAge` 是剩余期限（约 30 天）
- **无限续期**：`eoz launch` 自动登录成功后会把新 key 写回配置，因此只要定期启动就永不失效
- **fastLogin**：`autoLogin` 后调 `fastLogin.json` 再刷新 tgt/snda_id（对应 C# `LoginBySessionKey`）

## 四、字段流转总结

```text
getGuid          ──▶ guid（贯穿全程）
getCodeKey       ──▶ code_key
codeKeyLogin     ──▶ snda_id ──┬─▶ eoz auth 账号 key（配置持久化）
                     ├─▶ tgt ──┬─▶ getAccountGroup / accountGroupLogin（刷新）
                     │         └─▶ getPromotionInfo ──▶ ssoLogin ──▶ ticket（游戏启动）
                     ├─▶ auto_login_session_key ──▶ 配置保存 ──▶ autoLogin.json（下次免密）
                     └─▶ input_user_id（username，展示用）
```

## 五、错误码速查

| 返回码 | 含义 |
|---|---|
| `-10515805` | 二维码未扫描（继续轮询） |
| `-10516808` | 推送未确认（继续轮询） |
| `-10515005` | 自动登录 session key 过期 |
| `-10386188` | 需要验证码 |
| `-10242296` | 首次在该设备登录（改用扫码/推送） |
| `-10250013` | 参数/票据无效（如 endPoint 路径错误、tgt 无效） |
| `0` | 成功 |

## 六、eoz 层面对应

| eoz 命令 | 走上面哪个流程 |
|---|---|
| `auth login qr` | 流程 1（扫码 + 自动保存账号/session key） |
| `auth login password` | 流程 2 |
| `auth login auto --session-key <key>` | 流程 4 |
| `auth status` / `auth default` / `auth logout` | 读取/修改 `~/.xiv-launcher-rs/eorzea.toml` 中的账号 |
| `launch`（不指定账号） | 用默认账号的 session key 走流程 4，成功即启动 |
