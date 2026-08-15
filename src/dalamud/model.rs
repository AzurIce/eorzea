//! Dalamud 集成：加载机制与协议。
//!
//! 对应 C# `XIVLauncher.Common/Dalamud/`。核心原则（参考调查报告）：
//!
//! - Dalamud 是**外部运行时产品**（Windows x64 组件），launcher 不重写注入/CLR hosting
//! - Linux 上在**同一 Wine prefix** 内运行 `Dalamud.Injector.exe`，路径经 `winepath --windows` 转换
//! - 启动前必须校验 `SupportedGameVer == 本地游戏版本`（版本不匹配**绝不加载**）
//! - Rust 的职责：获取/校验/配置/进程编排

use serde::{Deserialize, Serialize};
use std::path::PathBuf;

/// Dalamud 加载方式（对应 C# `DalamudLoadMethod`）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum DalamudLoadMethod {
    /// 游戏入口点改写（启动期早期加载，推荐）
    #[default]
    EntryPoint,
    /// 传统远程 DLL 注入（游戏启动后注入）
    DllInject,
    /// 仅启动游戏并做兼容修复，不加载 Dalamud（内部/排障）
    AclOnly,
}

impl DalamudLoadMethod {
    pub fn to_injector_mode(&self) -> &'static str {
        match self {
            DalamudLoadMethod::EntryPoint => "entrypoint",
            DalamudLoadMethod::DllInject => "inject",
            DalamudLoadMethod::AclOnly => "inject",
        }
    }

    pub fn without_dalamud(&self) -> bool {
        matches!(self, DalamudLoadMethod::AclOnly)
    }
}

/// Dalamud 配置（`config.toml` 的 `[dalamud]` section）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct DalamudSettings {
    /// 是否启用 Dalamud（默认关闭，opt-in）
    pub enabled: bool,
    /// 加载方式（默认 entrypoint）
    pub load_method: DalamudLoadMethod,
    /// 初始化延迟（毫秒）
    pub delay_initialize_ms: u32,
    /// 禁用插件（safe mode，崩溃恢复用）
    pub no_plugins: bool,
    /// 禁用第三方插件
    pub no_third_party_plugins: bool,
    /// 是否由 launcher 管理 Windows x64 .NET runtime。
    ///
    /// release 声明 `RuntimeRequired` 时启动前会自动确保 runtime；
    /// 此开关为 true 时即使 release 未强制也由 launcher 管理。
    pub manage_runtime: bool,
    /// 更新通道：`release` / `staging`（或自定义 track）
    pub track: String,
    /// staging beta key（敏感，不写入普通日志）
    pub beta_key: Option<String>,
    /// 安装根目录 override（开发/排障；默认 `~/.xiv-launcher-rs/dalamud`）
    pub install_root: Option<PathBuf>,
}

impl Default for DalamudSettings {
    fn default() -> Self {
        Self {
            enabled: false,
            load_method: DalamudLoadMethod::EntryPoint,
            delay_initialize_ms: 0,
            no_plugins: false,
            no_third_party_plugins: false,
            manage_runtime: false,
            track: "release".to_string(),
            beta_key: None,
            install_root: None,
        }
    }
}

/// Dalamud release 元数据（来自 `VersionInfo` API，字段为驼峰）。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DalamudVersionInfo {
    #[serde(rename = "AssemblyVersion")]
    pub assembly_version: String,
    #[serde(rename = "SupportedGameVer")]
    pub supported_game_ver: String,
    #[serde(rename = "RuntimeVersion")]
    pub runtime_version: String,
    #[serde(rename = "RuntimeRequired")]
    pub runtime_required: bool,
    /// `hashes.json` 的 MD5（release 完整性校验）
    #[serde(rename = "Hash")]
    pub hash: String,
    #[serde(rename = "GitSha")]
    pub git_sha: String,
    #[serde(rename = "Revision")]
    pub revision: String,
    #[serde(rename = "downloadUrl")]
    pub download_url: String,
    #[serde(rename = "track")]
    pub track: String,
    #[serde(rename = "Key", default)]
    pub key: String,
}

/// Injector stdout 输出（`launch`/`inject` 成功后打印一行 JSON）。
#[derive(Debug, Clone, Deserialize)]
pub struct InjectorResult {
    pub pid: u32,
    #[serde(default)]
    pub handle: Option<u64>,
}

/// Dalamud 安装状态。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum InstallState {
    /// 未安装
    Missing,
    /// 已安装且版本匹配，可启动
    Ready,
    /// 已安装但版本不匹配游戏
    OutOfDate,
    /// 游戏版本过旧（release 尚未支持当前游戏）
    Unsupported,
    /// release 要求的 Windows x64 .NET runtime 尚未就绪
    RuntimeMissing,
    /// Dalamud assets 尚未就绪
    AssetsMissing,
    /// 下载/解压失败
    Failed(String),
}

/// Dalamud 状态报告（`eoz dalamud status` / GUI 展示用）。
#[derive(Debug, Clone)]
pub struct DalamudStatus {
    pub install_state: InstallState,
    /// release API 元数据（如可获取）
    pub remote: Option<DalamudVersionInfo>,
    /// 本机已安装的 AssemblyVersion（Hooks 目录名）
    pub local_assembly_version: Option<String>,
    /// 本机安装目录
    pub install_path: Option<PathBuf>,
    /// 本地游戏版本（`game/ffxivgame.ver`）
    pub local_game_ver: String,
}

impl DalamudStatus {
    /// release 是否支持当前游戏版本。
    pub fn remote_supported(&self) -> bool {
        self.remote
            .as_ref()
            .map(|r| r.supported_game_ver == self.local_game_ver)
            .unwrap_or(false)
    }
}

/// 构建 Injector 启动参数（`launch` 子命令）。
///
/// 对应 C# `DalamudInjectorArgs`。所有路径须已转换为 Windows 格式（`Z:\...`）。
pub struct DalamudStartInfo {
    /// 游戏 exe 的 Windows 路径
    pub game_path: String,
    /// Injector 所在目录（Windows 路径）
    pub working_directory: String,
    pub configuration_path: String,
    pub logging_path: String,
    pub plugin_directory: String,
    pub asset_directory: String,
    pub client_language: i32,
    pub delay_initialize_ms: u32,
    pub no_plugins: bool,
    pub no_third_party_plugins: bool,
}

/// 构造 Injector argv（`--` 之后为游戏参数，逐个传递不拼接）。
pub fn build_injector_launch_args(
    start: &DalamudStartInfo,
    load_method: DalamudLoadMethod,
    game_args: &[String],
    without_dalamud: bool,
) -> Vec<String> {
    let mut args = vec![
        "launch".to_string(),
        format!("--mode={}", load_method.to_injector_mode()),
        format!("--game={}", start.game_path),
        format!("--dalamud-working-directory={}", start.working_directory),
        format!("--dalamud-configuration-path={}", start.configuration_path),
        format!("--logpath={}", start.logging_path),
        format!("--dalamud-plugin-directory={}", start.plugin_directory),
        format!("--dalamud-asset-directory={}", start.asset_directory),
        format!("--dalamud-client-language={}", start.client_language),
    ];
    if start.delay_initialize_ms > 0 {
        args.push(format!(
            "--dalamud-delay-initialize={}",
            start.delay_initialize_ms
        ));
    }
    if without_dalamud || load_method.without_dalamud() {
        args.push("--without-dalamud".to_string());
    }
    if start.no_plugins {
        args.push("--no-plugin".to_string());
    }
    if start.no_third_party_plugins {
        args.push("--no-3rd-plugin".to_string());
    }
    args.push("--".to_string());
    args.extend(game_args.iter().cloned());
    args
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_build_launch_args() {
        let start = DalamudStartInfo {
            game_path: "Z:\\Games\\ffxiv\\game\\ffxiv_dx11.exe".into(),
            working_directory: "Z:\\dalamud\\Hooks\\15.0.3.0".into(),
            configuration_path: "Z:\\config\\dalamudConfig.json".into(),
            logging_path: "Z:\\logs\\dalamud.log".into(),
            plugin_directory: "Z:\\plugins".into(),
            asset_directory: "Z:\\assets".into(),
            client_language: 4,
            delay_initialize_ms: 0,
            no_plugins: false,
            no_third_party_plugins: false,
        };
        let game_args = vec!["-AppID=100001900".to_string()];
        let args =
            build_injector_launch_args(&start, DalamudLoadMethod::EntryPoint, &game_args, false);
        assert!(args.contains(&"--mode=entrypoint".to_string()));
        assert!(args.contains(&"--game=Z:\\Games\\ffxiv\\game\\ffxiv_dx11.exe".to_string()));
        assert!(args.contains(&"--".to_string()));
        assert!(args.contains(&"-AppID=100001900".to_string()));
    }

    #[test]
    fn test_load_method_modes() {
        assert_eq!(
            DalamudLoadMethod::EntryPoint.to_injector_mode(),
            "entrypoint"
        );
        assert_eq!(DalamudLoadMethod::DllInject.to_injector_mode(), "inject");
        assert!(DalamudLoadMethod::AclOnly.without_dalamud());
    }
}
