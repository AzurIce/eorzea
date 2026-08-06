# FFXIV 中国服（SDO）登录 → 启动 完整链路

> 本文档描述 `xiv-launcher-rs` 项目中，从用户输入凭据到游戏成功启动的完整流程。

## 总览

```
用户输入 ──▶ 登录认证 ──▶ 获取 ticket ──▶ 构造启动参数 ──▶ 准备环境 ──▶ 启动游戏
     │           │            │              │              │           │
     ▼           ▼            ▼              ▼              ▼           ▼
  sdo_login   SdoAuth     sso_login    build_sdo_    WineTool   wine64
  交互示例    HTTP API    (tgt→ticket) launch_args    + DXVK    ffxiv_dx11
```

---

## 阶段 1：用户交互（`examples/sdo_login.rs`）

### 1.1 初始化

```
[tracing_subscriber::fmt::init()]              ← 初始化结构化日志
        │
        ▼
[SdoAuth::new()]                               ← 创建认证客户端
        │
        ├── 自动采集设备指纹
        │   ├── MAC 地址哈希 → `mac_id`
        │   ├── CPU ID 哈希 → `cpu_id`  
        │   └── 磁盘序列号哈希 → `disk_serial`
        │
        └── 组合成 `device_id = "{mac}:{cpu}:{disk}"`
```

### 1.2 获取上下文（`get_context`）

```
[SdoAuth::get_context()]                       ← 调用 `getGuid.json`
        │
        ├── Cookie 注入：`CASCID`, `SECURE_CASCID`, `_rsid`
        ├── 请求头：`Host: cas.sdo.com`
        └── 查询参数：`guid`, `deviceId`, `mac`, `accountDomain`
        │
        ▼
    返回 SdoContext {                              ← 临时上下文
        guid: "xxxunilinuxmc",                      ← 唯一会话标识
        dynamic_key: None                            ← 动态密钥（如有）
    }
```

### 1.3 选择登录方式（5种）

| 选项 | 方法 | 输入 | 输出 |
|------|------|------|------|
| 1 | `static_login` | 账号 + 密码 | `tgt` + `snda_id` |
| 2 | `slide_login` | 账号 | 叨鱼 APP 推送确认 → `tgt` |
| 3 | `qr_code_login` | 无 | 扫描二维码 → `tgt` |
| 4 | `auto_login` | `session_key` | 刷新 `tgt` |
| 5 | `fetch_server_list` | 无 | 大区列表 |

**当前验证通过**：选项 3（扫码登录）✅

---

## 阶段 2：登录认证（以扫码为例）

### 2.1 获取二维码（`qr_code_request`）

```
[SdoAuth::qr_code_request(&ctx)]               ← 调用 `getCodeKey.json`
        │
        ├── Cookie 注入 + Host 头
        ├── 参数：`checkCodeFlag=1`, `areaId=1`, `loginSource=0`
        │
        ▼
    返回 QrCodeResult {
        code_key: "04f6d6926a040dbd00c43ccb04946700",  ← 二维码标识
        image_data: Vec<u8>                              ← PNG 图片字节
    }
        │
        ▼
    保存到 /tmp/xiv_qr.png (343 bytes)
```

### 2.2 轮询扫码结果（`qr_code_poll`）

```
[SdoAuth::qr_code_poll(&ctx, &code_key, 30)]    ← 调用 `codeKeyLogin.json`
        │
        ├── 每 3 秒轮询一次
        ├── 参数：`codeKey=xxx`, `guid=xxx`
        │
        ├── 返回码处理：
        │   ├── -10515805 → "尚未扫描" → 继续等待 (Pending)
        │   ├── -10515806 → "已扫描未确认" → 继续等待
        │   └── 0 → 成功 (Success)
        │
        ▼
    返回 PollResult::Success(SdoLoginData {
        snda_id: Some("1765973508"),                    ← 账号 ID
        tgt: Some("ULSTGT-8b2af9275b774b538d3d6dfeac7da8a2"),  ← 临时令牌
        input_user_id: None,
        auto_login_session_key: None
    })
```

### 2.3 激活权限（`get_promotion_info`）

```
[SdoAuth::get_promotion_info(&tgt)]              ← 调用 `getPromotionInfo.json`
        │
        ├── 参数：`tgt=xxx`, `serviceUrl=http://www.sdo.com`
        │
        ├── 作用：告诉 SDO 服务端此 TGT 已激活，允许换取 ticket
        │
        ▼
    返回：空响应（只需确认 return_code == 0）
```

### 2.4 换取 Ticket（`sso_login`）

```
[SdoAuth::sso_login(&ctx, &tgt)]                 ← 调用 `ssoLogin.json`
        │
        ├── 参数：`tgt=ULSTGT-xxx`, `guid=xxx`
        │
        ├── 服务端校验：
        │   ├── TGT 是否有效
        │   ├── 设备指纹是否匹配
        │   └── 激活权限是否已调用
        │
        ▼
    返回 String: "ULS21-f47c93c1b93746bf90c253701c96849d"   ← 最终游戏 session ID
```

**至此，登录阶段完成。获得：**
- `ticket` = `ULS21-...`（DEV.TestSID）
- `snda_id` = `1765973508`（XL.SndaId）

---

## 阶段 3：构造启动参数（`src/game.rs`）

### 3.1 构建参数（`build_sdo_launch_args`）

```
输入：
    session_id = "ULS21-f47c93c1b93746bf90c253701c96849d"
    snda_id = "1765973508"
    area = SdoArea { area_id: "1", area_lobby: "ffxivlobby01.ff14.sdo.com", ... }
    areas = [陆行鸟, 莫古力, 猫小胖, 豆豆柴]

输出参数列表：
    -AppID=100001900
    -AreaID=1
    Dev.LobbyHost01=ffxivlobby01.ff14.sdo.com
    Dev.LobbyPort01=54994
    Dev.GMServerHost=ffxivgm01.ff14.sdo.com
    Dev.SaveDataBankHost=ffxivsdb01.ff14.sdo.com
    resetConfig=0
    DEV.MaxEntitledExpansionID=1
    DEV.TestSID=ULS21-f47c93c1b93746bf90c253701c96849d        ← 登录获得的 ticket
    XL.SndaId=1765973508                                        ← 登录获得的 snda_id
    XL.LobbyHosts=ffxivlobby01.ff14.sdo.com:54994|ffxivlobby05.ff14.sdo.com:54994|...
```

**格式说明**：使用 `key=value` 格式（C# `ArgumentBuilder.Build()` 行为），不是空格分隔。

---

## 阶段 4：准备运行环境（`src/wine.rs` + `src/game.rs`）

### 4.1 确保 Wine 可用（`WineTool::ensure`）

```
[WineTool::ensure(custom_path=None)]
        │
        ├── 检测优先级：
        │   1. 用户自定义路径
        │   2. ~/.xlcore/beta/wine/bin/wine64（XIVLauncher 已安装）
        │   3. 系统 PATH 中的 wine64
        │
        ├── 未检测到 → 自动下载
        │   ├── URL: https://s3.ffxiv.wang/xlcore/deps/wine/osx/xom-4.17.1/wine.tar.gz
        │   ├── 大小：~285MB
        │   ├── 解压到：~/.xiv-launcher-rs/tools/wine/
        │   └── 验证：wine64 可执行文件存在
        │
        ▼
    返回 WineTool {
        wine64_path: "~/.xiv-launcher-rs/tools/wine/bin/wine64",
        prefix_path: "~/.xiv-launcher-rs/prefix",
        is_managed: true
    }
```

### 4.2 确保 DXVK 已安装（`WineTool::ensure_dxvk`）

```
[WineTool::ensure_dxvk()]
        │
        ├── 检查：prefix/drive_c/windows/system32/d3d11.dll 是否存在
        │
        ├── 不存在 → 自动下载
        │   ├── URL: https://s3.ffxiv.wang/xlcore/deps/dxvk/osx/...
        │   ├── 大小：~2.7MB
        │   ├── 解压到：~/.xiv-launcher-rs/tools/dxvk/
        │   └── 复制 DLL：
        │       ├── x64/d3d11.dll → prefix/system32/
        │       ├── x64/d3d10core.dll → prefix/system32/
        │       └── x32/*.dll → prefix/syswow64/
        │
        ▼
    DXVK 就绪（用于 DirectX → Vulkan 转换）
```

### 4.3 确保登录 DLL 是修改版（`ensure_login_entry`）

```
[ensure_login_entry(game_path)]
        │
        ├── 检查：{gamePath}/../sdo/sdologin/sdologinentry64.dll
        │
        ├── 场景 A：不存在
        │   └── 从 GitHub 下载 ottercorp 修改版
        │       URL: raw.githubusercontent.com/ottercorp/XIVLauncher.Core/cn/...
        │       缓存：~/.xiv-launcher-rs/tools/sdologinentry64.dll
        │       复制到：{gamePath}/../sdo/sdologin/sdologinentry64.dll
        │
        ├── 场景 B：存在但不是修改版（CompanyName != "ottercorp"）
        │   ├── 备份原文件 → sdologinentry64.sdo.dll
        │   └── 替换为修改版
        │
        └── 场景 C：已是修改版 → 跳过
        │
        ▼
    DLL 就绪（绕过盛趣启动器白名单检查）
```

---

## 阶段 5：启动游戏（`launch_game`）

### 5.1 构造 Wine 命令

```
命令行：
    wine64 "/Volumes/Files/_ffxiv/XIVLauncherGamePath/game/ffxiv_dx11.exe"
         -AppID=100001900
         -AreaID=1
         Dev.LobbyHost01=ffxivlobby01.ff14.sdo.com
         Dev.LobbyPort01=54994
         ...
         DEV.TestSID=ULS21-f47c93c1b93746bf90c253701c96849d
         XL.SndaId=1765973508
         XL.LobbyHosts=...

环境变量：
    WINEPREFIX=~/.xiv-launcher-rs/prefix
    XL_WINEONLINUX=true
    XL_WINEONMAC=true
    WINEDLLOVERRIDES="msquic=,mscoree=n,b;d3d11=n;dxgi=n,b"
```

### 5.2 游戏启动流程

```
[wine64 ffxiv_dx11.exe ...]
        │
        ├── Wine 初始化 prefix（首次）
        │   ├── 创建虚拟 C 盘
        │   ├── 注册表初始化
        │   └── 注册 DXVK DLL 覆盖
        │
        ├── 游戏加载 sdologinentry64.dll
        │   └── 修改版 DLL：跳过启动器校验，允许第三方启动器
        │
        ├── DXVK 初始化（MoltenVK → Metal）
        │   ├── Vulkan instance 创建
        │   ├── Apple M2 Max GPU 检测
        │   └── Swapchain 创建 (1920x1080)
        │
        ├── 游戏验证 ticket
        │   ├── 读取 DEV.TestSID
        │   └── 向盛趣大厅服务器发送认证请求
        │
        ▼
    进入服务器选择界面 ← 登录成功！
```

---

## 数据流总结

### 敏感数据流转

```
密码 ──▶ [static_login] ──▶ SDO 服务器（HTTPS）
                              │
                              ▼
                           TGT ──▶ [sso_login] ──▶ SDO 服务器
                                                      │
                                                      ▼
                                                   Ticket ──▶ [ffxiv_dx11.exe]
                                                                  │
                                                                  ▼
                                                               游戏大厅
```

**脱敏处理**：密码在日志中显示为 `***`，ticket 和 snda_id 部分脱敏显示。

### 文件系统变更

```
~/.xiv-launcher-rs/
├── tools/
│   ├── wine/                          ← Wine 运行时（285MB）
│   │   └── bin/wine64
│   ├── dxvk/                          ← DXVK 驱动（2.7MB）
│   │   └── dxvk-macOS-async-v1.10.3/
│   └── sdologinentry64.dll            ← 修改版登录 DLL（16.5KB）
│
└── prefix/                            ← Wine 虚拟 Windows 环境
    └── drive_c/windows/system32/
        ├── d3d11.dll                  ← DXVK 替换版
        └── dxgi.dll                   ← DXVK 替换版

/Volumes/Files/_ffxiv/XIVLauncherGamePath/
└── sdo/sdologin/
    ├── sdologinentry64.dll            ← 修改版（运行时使用）
    └── sdologinentry64.sdo.dll        ← 原版备份（如有）
```

---

## 与 C# XIVLauncher 的对应关系

| Rust 模块/方法 | C# 对应 | 作用 |
|---------------|---------|------|
| `SdoAuth::static_login` | `LoginBySdoStatic` | 密码登录 |
| `SdoAuth::qr_code_request/poll` | `QrCodeLogin` | 扫码登录 |
| `SdoAuth::sso_login` | `GetSessionId` | TGT 换 ticket |
| `build_sdo_launch_args` | `ArgumentBuilder.Build()` | 启动参数构造 |
| `WineTool::ensure` | `CompatibilityTools.EnsureTool` | Wine 下载/检测 |
| `WineTool::ensure_dxvk` | `Dxvk.InstallDxvk` | DXVK 安装 |
| `ensure_login_entry` | `EnsureLoginEntry` | DLL 替换 |
| `launch_game` | `UnixGameRunner.Start` | 进程启动 |

---

## 已知限制（TODO）

1. **密码登录风控**：新设备触发验证码要求，需改用扫码
2. **getAccountGroup** ✅：已实现（`sdo.rs::get_account_group` + `account_group_login`，扫码闭环打通）
3. **Wine 死锁**：macOS 上偶发 `RtlpWaitForCriticalSection` 超时
4. **参数加密**：未实现 Blowfish 加密（P2-2，国服实际不加密）
5. **WeGame 登录**：`thirdPartyLogin` / `LoginBySid` 未实现

## 自动登录与 key 续期

- `autoLogin.json` 用旧 key 换**新 key**（旧 key 立即作废）+ `autoLoginMaxAge`（剩余期限）
- `fastLogin.json` 再用新 tgt 刷新 snda_id/tgt（对齐 C# `LoginBySessionKey`）
- `xlcli launch` 自动登录成功后把新 key 写回配置 → 每次启动自动续期（约 30 天）
- key 过期（`-10515005`）时提示重新 `auth login qr`

---

## 日志查看

默认输出 **INFO** 级别，包含：
- 方法进入/退出（`#[tracing::instrument]`）
- HTTP 请求 URL 和返回码
- Wine/DXVK 安装进度
- DLL 替换状态
- 启动命令行

调试模式：
```bash
RUST_LOG=debug cargo run --example sdo_login
```

---

*文档版本：1.0*
*最后更新：2025-01-09*