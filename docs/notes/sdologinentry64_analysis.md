# SDO 登录入口 DLL (`sdologinentry64.dll`) 技术分析

## 概述

本文档详细分析最终幻想 XIV 中国服（SDO/盛趣运营）的登录认证机制，特别是 `sdologinentry64.dll` 的作用、原版限制以及 XIVLauncher 为何需要对其进行修改。

---

## 1. `sdologinentry64.dll` 是什么？

`sdologinentry64.dll` 是盛趣（SDO）为 FFXIV 中国服提供的**登录入口动态链接库**，位于游戏安装目录的 `sdo/sdologin/` 下。

### 核心功能

1. **账号认证接口**：提供 `SDOLInitialize`、`SDOLGetModule` 等导出函数
2. **Session 验证**：验证 `DEV.TestSID`（ticket）和 `XL.SndaId` 的合法性
3. **启动器身份校验**：检查启动器是否为官方启动器（盛趣通行证客户端）
4. **反调试/反修改**：包含对游戏进程的保护逻辑

### 加载时机

游戏启动时，`ffxiv_dx11.exe` 会动态加载此 DLL：

```
SDOLGetModule@00006FFFFDFA14C0
SDOLInitialize@00006FFFFDFA15B0
SDOLTerminal@00006FFFFDFA1570
```

只有在 `SDOLInitialize` 返回成功后，游戏才会进入服务器选择界面。

---

## 2. 原版 DLL 的限制

盛趣原版 `sdologinentry64.dll`（备份为 `sdologinentry64.sdo.dll`）包含以下限制：

### 2.1 启动器白名单

原版 DLL 会校验启动它的进程是否为**官方盛趣启动器**。校验方式包括但不限于：

- **进程名检查**：检查父进程名是否为 `SdoLogin.exe` 或官方启动器
- **数字签名验证**：验证启动器是否带有盛趣的数字签名
- **文件哈希比对**：比对启动器可执行文件的哈希值

### 2.2 环境检测

- **反虚拟机检测**：检测是否在 VM 中运行
- **反调试检测**：检测是否有调试器附加
- **DLL 注入检测**：检测是否有未授权的 DLL 被加载

### 2.3 后果

如果使用原版 DLL + 第三方启动器（如 XIVLauncher）：

1. `SDOLInitialize` 返回失败
2. 游戏显示"账号认证发生错误"
3. 无法进入服务器选择界面

---

## 3. ottercorp 修改版做了什么？

[XIVLauncherCN](https://github.com/ottercorp/FFXIVQuickLauncher)（由 ottercorp 维护）包含一个修改版的 `sdologinentry64.dll`，用于绕过上述限制。

### 3.1 修改内容（基于行为分析）

| 原版限制 | 修改版行为 |
|---------|-----------|
| 启动器白名单检查 | **移除** - 不再检查父进程身份 |
| 数字签名验证 | **移除** - 跳过签名验证 |
| 反调试检测 | **移除** - 允许调试器存在 |
| 环境检测 | **放宽** - 允许在 Wine/虚拟机中运行 |

### 3.2 如何识别修改版

修改版 DLL 在 PE 文件 version info 中标注了 `CompanyName = "ottercorp"`：

```csharp
// C# XIVLauncher 的检测逻辑
if (FileVersionInfo.GetVersionInfo(entryDll).CompanyName == "ottercorp")
{
    // 确认是修改版，进行哈希校验
}
```

### 3.3 修改版来源

修改版 DLL 由 ottercorp 团队维护，位于 XIVLauncher.Core 仓库中：

```
src/XIVLauncher.Core/Resources/binaries/sdologinentry64.dll
```

**注意**：此文件直接包含在源代码仓库中（未被 `.gitignore` 排除），因此从 GitHub 克隆的完整源码中包含此修改版 DLL。

**我们的实现**：Rust launcher 启动时会自动从 ottercorp GitHub 仓库下载此 DLL：

```
https://raw.githubusercontent.com/ottercorp/XIVLauncher.Core/cn/src/XIVLauncher.Core/Resources/binaries/sdologinentry64.dll
```

下载后缓存在 `~/.xiv-launcher-rs/tools/sdologinentry64.dll`，避免重复下载。

---

## 4. 安全风险分析

### 4.1 会封号吗？

**根据 XIVLauncher 官方说明和多年社区实践，使用修改版 DLL 没有封号案例。**

理由：

1. **仅修改启动器校验**：修改版只移除了"启动器必须是官方启动器"的检查，**不涉及游戏内逻辑修改**
2. **认证流程完整**：登录流程（staticLogin → ssoLogin → getPromotionInfo）完全复刻官方，服务器收到的认证请求与官方启动器一致
3. **没有内存修改**：不像 Dalamud（卫月框架）那样注入游戏内存，DLL 替换只在启动阶段发生
4. **社区规模**：XIVLauncherCN 有数万用户，运行多年，无封号报告

### 4.2 盛趣能检测到吗？

**技术上可以检测，但实际不会因此封号。**

- `sdologinentry64.dll` 运行在客户端本地，**不发送任何网络请求**
- 盛趣服务器只能看到认证 API（`staticLogin.json`、`ssoLogin.json`）的请求
- 这些请求的内容（ticket、sndaId、deviceId）与官方启动器完全一致
- 唯一的区别是启动器进程名不同，但这不在服务器监控范围内

### 4.3 与 Dalamud 的区别

| 项目 | sdologinentry64 替换 | Dalamud 注入 |
|------|---------------------|-------------|
| **作用时机** | 游戏启动前 | 游戏运行中 |
| **修改范围** | 启动器校验逻辑 | 游戏内存/渲染管线 |
| **网络影响** | 无 | 无（插件可能） |
| **检测风险** | 极低 | 低（但存在） |
| **封号案例** | 无 | 无 |

---

## 5. C# XIVLauncher 的实现细节

### 5.1 `EnsureLoginEntry()` 方法

```csharp
public void EnsureLoginEntry(DirectoryInfo gamePath)
{
    var bootPath = Path.Combine(gamePath.FullName, "sdo", "sdologin");
    var entryDll = Path.Combine(bootPath, "sdologinentry64.dll");
    var xlEntryDll = Path.Combine(Paths.ResourcesPath, "sdologinentry64.dll");

    // ... 查找修改版 DLL

    if (!File.Exists(entryDll))
    {
        // 纯净客户端：直接复制修改版
        File.Copy(xlEntryDll, entryDll, true);
    }
    else
    {
        if (FileVersionInfo.GetVersionInfo(entryDll).CompanyName == "ottercorp")
        {
            // 已是修改版，检查是否需要更新
            if (GetFileHash(entryDll) != GetFileHash(xlEntryDll))
            {
                File.Copy(xlEntryDll, entryDll, true);
            }
        }
        else
        {
            // 原版 DLL：备份并替换
            File.Copy(entryDll, Path.Combine(bootPath, "sdologinentry64.sdo.dll"), true);
            File.Copy(xlEntryDll, entryDll, true);
        }
    }
}
```

### 5.2 调用时机

在 `LaunchGameSdo()` 方法中，**构建启动参数之前**调用：

```csharp
public Process? LaunchGameSdo(..., DirectoryInfo gamePath, ...)
{
    EnsureLoginEntry(gamePath);  // <-- 先确保 DLL 是修改版

    var argumentBuilder = new ArgumentBuilder()
        .Append("-AppID", "100001900")
        .Append("DEV.TestSID", sessionId)
        ...;

    return runner.Start(exePath, workingDir, arguments, ...);
}
```

### 5.3 路径结构

```
{gamePath}/                    # 游戏安装根目录（如 XIVLauncherGamePath）
├── sdo/
│   └── sdologin/
│       ├── sdologinentry64.dll          # <-- 修改版（ottercorp）
│       └── sdologinentry64.sdo.dll      # <-- 原版备份（如有）
└── game/
    └── ffxiv_dx11.exe                   # 游戏主程序
```

**注意**：`sdo/sdologin/` 目录在游戏安装根目录下，**不在 `game/` 子目录内**。

---

## 6. 我们的 Rust 实现

### 6.1 对应代码

`src/game.rs` 中的 `ensure_login_entry()` 函数：

```rust
pub fn ensure_login_entry(game_path: &std::path::Path) -> Result<(), GameLaunchError> {
    let game_root = game_path
        .parent()
        .and_then(|p| p.parent())
        .unwrap_or_else(|| std::path::Path::new("."));
    let boot_path = game_root.join("sdo/sdologin");
    let entry_dll = boot_path.join("sdologinentry64.dll");

    // 查找 ottercorp 修改版 DLL
    let xl_entry_dll = find_ottercorp_dll();

    fs::create_dir_all(&boot_path)?;

    if !entry_dll.exists() {
        // 没有 DLL，复制修改版
        if let Some(src) = xl_entry_dll {
            fs::copy(src, &entry_dll)?;
        }
    } else if !is_ottercorp_dll(&entry_dll) {
        // 存在但不是修改版，备份并替换
        let backup = boot_path.join("sdologinentry64.sdo.dll");
        fs::copy(&entry_dll, &backup)?;
        if let Some(src) = xl_entry_dll {
            fs::copy(src, &entry_dll)?;
        }
    }

    Ok(())
}
```

### 6.2 下载修改版 DLL

Rust launcher 不再搜索本地文件，而是直接从 ottercorp GitHub 仓库下载：

```rust
const DLL_URL: &str = "https://raw.githubusercontent.com/ottercorp/XIVLauncher.Core/main/src/XIVLauncher.Core/Resources/binaries/sdologinentry64.dll";
```

下载后缓存在 `~/.xiv-launcher-rs/tools/sdologinentry64.dll`，如果已存在则直接使用缓存版本。

### 6.3 识别修改版

PE version info 中的 `CompanyName` 以 UTF-16LE 存储，因此需要同时匹配 ASCII 和 UTF-16LE 两种编码的 `"ottercorp"`：

```rust
fn is_ottercorp_dll(path: &std::path::Path) -> bool {
    if let Ok(data) = std::fs::read(path) {
        const ASCII: &[u8] = b"ottercorp";
        const UTF16LE: &[u8] = b"o\0t\0t\0e\0r\0c\0o\0r\0p\0";
        let found = |needle: &[u8]| data.windows(needle.len()).any(|w| w == needle);
        return found(ASCII) || found(UTF16LE);
    }
    false
}
```

> **历史 bug**：早期实现用 `String::from_utf8_lossy(&data).contains("ottercorp")` 按 UTF-8 搜索，
> 永远匹配不到 UTF-16LE 存储的字符串，导致每次启动都误判为"非修改版"，
> 反复把当前 DLL 备份覆盖到 `sdologinentry64.sdo.dll`——第二次启动后原版备份被修改版覆盖，
> shim 转发到自身，游戏内报 5003「帐号认证发生了错误」。

---

## 7. 技术细节：PE 文件 Version Info

Windows PE（Portable Executable）文件可以嵌入版本信息资源，包含：

- `CompanyName`：公司名称
- `FileDescription`：文件描述
- `FileVersion`：文件版本
- `ProductName`：产品名称
- `LegalCopyright`：版权信息

ottercorp 修改版将 `CompanyName` 设为 `"ottercorp"`，以便 XIVLauncher 识别这是自己分发的版本。

原版 DLL 的 `CompanyName` 通常是 `"Shanda Games"` 或 `"盛大游戏"`。

---

## 8. 常见问题

### Q: 如果不替换 DLL 会怎样？

游戏会启动并渲染，但在认证阶段显示"账号认证发生错误"（错误代码 10000000），无法进入服务器选择界面。

### Q: 替换 DLL 后还能用官方启动器吗？

可以。官方启动器不检查 DLL 的 `CompanyName`，它只使用 DLL 的导出函数。修改版 DLL 保留了所有原版导出函数，只是移除了启动器身份校验。

### Q: 游戏更新后需要重新替换吗？

通常不需要。除非盛趣更新了 `sdologinentry64.dll`（非常罕见），否则修改版继续有效。

### Q: 这个修改是否合法？

从技术上讲，修改游戏文件违反《最终幻想 XIV》的服务条款。但 XIVLauncher 社区运行多年，**没有因此封号的案例**。这类似于使用 ACT（战斗统计工具）或 Teamcraft —— 都是第三方工具，但 Square Enix/盛趣对此采取宽容态度。

---

## 9. 参考资源

- [XIVLauncherCN GitHub](https://github.com/ottercorp/FFXIVQuickLauncher)
- [XIVLauncherCN FAQ](https://ottercorp.github.io/faq/)
- [XIVLauncherCN 安全性说明](https://ottercorp.github.io/faq/xl_troubleshooting#q-xivlauncherdalamud-%E5%92%8C-dalamud-%E6%8F%92%E4%BB%B6%E6%98%AF%E5%90%A6%E5%AE%89%E5%85%A8%E5%8F%AF%E9%9D%A0)
- C# 参考代码：`SdoLauncher.cs` line 755-807 (`EnsureLoginEntry`)

---

## 10. 免责声明

**使用第三方启动器和修改版 DLL 的风险由用户自行承担。**

本文档仅供技术研究和学习目的。最终幻想 XIV 的版权归 Square Enix 和盛趣所有。ottercorp 和本项目的开发者不对任何账号问题负责。

**建议**：
- 不要在游戏内公开讨论使用 XIVLauncher
- 不要同时使用未经审查的第三方插件
- 定期备份游戏配置文件 (`My Games/FFXIV_*`)

---

*文档版本：1.0*
*最后更新：2025-01-09*