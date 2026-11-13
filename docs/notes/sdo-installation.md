# 国服空目录安装与 SDO 组件来源

## 已确认的来源

官方客户端的 `Launcher3Configs/newVersionCenter.ini` 给出独立于游戏 ZiPatch 的启动器更新入口：

- 版本索引：<https://ff.autopatch.sdo.com/v3launcher/build/ver2data/791000814/8821/-1/ver2.dat>
- 全文件清单：<https://ff.autopatch.sdo.com/v3launcher/build/791000814/8821/client-all-files-list/client_all_files_list.dat>
- 启动器版本：<https://ff.autopatch.sdo.com/v3launcher/build/791000814/8821/client-all-files-list/LocalVersion3.xml>

这些入口无需账号即可读取。调查时索引目标版本为 `0.0.0.26`；实现不能固定该版本。

全文件清单第一行是 `baseUrl|productId|version`，后续为 `Windows相对路径|长度|MD5`。清单包含：

- `Launcher3Update\Launcher3Modules\sdologinentry64.dll`
- `Launcher3Update\Launcher3Modules\sdologin\sdologinsdk64.dll`
- `Launcher3Update\sdo\sdologin\sdologinentry64.dll`
- SDO 的其他 DLL、配置及资源。

两处官方 `sdologinentry64.dll` 不是同一个文件，不能仅凭文件名互换。全文件清单的 `baseUrl + 相对路径` 直取 DLL 实测返回 404；该清单不能直接当作已验证的逐文件下载接口。

## 可验证的下载链

`ver2.dat` 包含 `baseUrl` 和 `packages[]`，每项含 `from`、`to`、`fileListUrl`、`md5`。将 `baseUrl` 与 `fileListUrl` 拼接可取得 JSON 文件列表，其 MD5 与索引一致。文件列表包含：

- 包级 `baseUrl`；
- `directFileUrl` 指向 PList；
- `fileList[]` 给出 ZIP 的 `url`、`size`、`md5`。

例如从 `0.0.0.0` 到 `0.0.0.1` 的初始 ZIP 长度为 67,122,048 字节，MD5 为 `81B693A64844B4D0D2C1694C2FA1AF1D`。服务端支持 HTTP Range；读取中央目录确认其中有 370 个成员，包括：

- `Pkg\Launcher3Update\Launcher3Modules\sdologinentry64.dll`（529,728 字节）
- `Pkg\Launcher3Update\Launcher3Modules\sdologin\sdologinsdk64.dll`（2,032,960 字节）
- `Pkg\Launcher3Update\sdo\sdologin\sdologinentry64.dll`（46,080 字节）

两个入口 DLL 已通过 Range 提取、DEFLATE 解压、长度与 ZIP CRC 校验；没有执行。未下载完整 ZIP，因此没有验证整包 MD5。完整包的安装语义、后续差分和官方启动器运行时部署行为仍需验证，不能直接把每个 ZIP 解压到游戏目录就视作安装完成。

## 本机故障边界

`~/Games/ffxiv/Launcher3Modules/sdologinentry64.dll` 与该安装的 `sdo/sdologin/sdologinentry64.sdo.dll` SHA256 一致：`d884bead00ecba3012dc9e0f6faae8692249a439dfaa8fb60db9570dde48986f`，长度 529,792 字节。该文件含 `sdologin\\sdologinsdk64.dll` 和 SDO 更新策略 URL 字符串。这是依赖调查线索，不等同于运行时加载证据。

从空目录使用本项目更新的安装最初仅有 ottercorp shim，没有原版 `.sdo.dll`，实际出现 `Can not load ...sdologinentry64.sdo.dll` 弹窗。补回原版后用户仍报告卡住；继续从旧安装补齐 113 个 SDO 文件（包括 SDK、配置及资源）后仍卡住。因此，缺原版导致的加载弹窗与标题界面的卡住是两个问题，不能以本次卡住推断整套 SDO 组件都是第三方登录所必需的。

卡住现场的 `/proc/<pid>/maps` 未发现 SDO 文件映射，Dalamud boot 日志也未记录其加载；大厅 TCP 连接已建立并持续有少量双向数据。该快照不能证明 DLL 从未加载或从未卸载，但尚无证据表明本次卡住正在等待 SDO SDK。现场仍启用了 Dalamud 与插件，需要一次关闭 Dalamud、启用 DLL 加载日志的对照启动，不能把安装缺口等同于当前卡住根因。

对照启动关闭 Dalamud、设置 `WINEDEBUG=+loaddll` 后仍卡住，日志确认 shim 与原版 `.sdo.dll` 均成功加载。末尾错误是 `wmadmod.dll` 依赖的 `winegstreamer.dll` 缺失；实际 Wine 构建中未找到该组件，构建输入也没有 GStreamer。正常安装与新安装的游戏 EXE 和版本文件 SHA256 相同，但游戏目录下 `My Games/FINAL FANTASY XIV - A Realm Reborn/FFXIV.cfg` 的 `CutsceneMovieOpening` 分别为 `1` 和 `0`。仅将新安装这一项从 `0` 改为 `1` 后，用户确认卡住解决。这支持首次开场动画播放触发媒体解码故障，而非登录网络卡住；该修改是绕过动画，不是修复 Wine 解码能力。

原版 DLL 的启动前存在性检查有实际弹窗依据；SDK 的最小依赖集合、官方组件的必要部署范围以及是否需要完整 SDO 登录器，仍未验证。后续应在隔离的最小安装中逐项验证，避免将整目录补齐当作已证实的必要条件。

## 与 XIVLauncher.Core 的对比

- `lib/FFXIVQuickLauncher/src/XIVLauncher.Common/Game/Launcher.cs` 的 `CheckBootVersion()` 在 CN 分支直接返回空列表。
- 同目录 `SdoLauncher.cs` 的 `EnsureLoginEntry()` 在原 DLL 不存在时仅复制 shim；原 DLL 存在且不是 ottercorp 版本时才生成 `.sdo.dll`。
- 上游资源 shim 与本机 shim SHA256 相同：`a7ae15660d00eb0b15e76902736721d2cc7450fed811dfbde879a07336ae65cd`。

因此此缺口不是少执行一次国际服 boot 更新；现有上游逻辑也不能证明国服空目录安装完整。

## 完整安装的实现建议

1. 增加独立的国服组件引导阶段，使用官方启动器更新链获取可信组件；验证包长度、摘要与解压路径，拒绝路径穿越。
2. 在暂存目录还原并验证组件集合，明确 `Launcher3Modules` 到游戏 `sdo` 目录的部署规则及 x64 SDK 的依赖；最小集合须经加载日志与实际登录验证，不能只取一个入口 DLL。
3. 原版组件完整后再部署 ottercorp shim；原版保存为 `.sdo.dll`，校验其不是 shim，禁止覆盖有效原版备份。
4. 保留现有 game ZiPatch 更新链；将组件引导、游戏更新、Wine/DXVK 环境准备分别报告状态，任一未完成不得宣称安装完成。
5. 扩展启动预检和 `game verify`，覆盖原版、SDK 及必要配置；记录组件版本、来源与摘要，不记录账号凭据。
6. 用全新目录、无本机旧安装可借用的环境验证：下载中断恢复、重启幂等、组件升级、标题界面到角色列表及进入游戏。

调查下载材料保存在 `/tmp/opencode/ffxiv-install-investigation/`；该临时目录不是持久缓存或可发布依赖。
