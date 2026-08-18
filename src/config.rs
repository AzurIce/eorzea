//! 用户可配置设置（Wine 相关）与持久化。
//!
//! 对应 C# 的 `WineSettings.cs` + 配置持久化。
//! 与 `wine.rs` 的关系：这里是**声明式配置**，`WineTool` 是解析后的运行时对象。
//! 支持"启动时使用不同的 wine"：持久化默认配置 + `Launcher::launch_with_wine` 单次覆盖。
//!
//! 配置文件：`~/.xiv-launcher-rs/config.toml`（TOML，旧版 `settings.json` 自动迁移）。
//! 顶层为 Wine 设置（flatten），`[dalamud]` section 为 Dalamud 配置。

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use crate::dalamud::model::DalamudSettings;

use serde::{Deserialize, Serialize};

/// Wine 启动方式。
///
/// `Auto` 为默认值，等价于旧版 `WineTool::ensure(None)` 的行为：
/// 自定义路径 → XIVLauncher 托管 → 系统 wine → 自动下载。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum WineStartupType {
    /// 自动选择：自定义路径 → XIVLauncher 托管 → 系统 wine → 下载（默认）
    #[default]
    Auto,
    /// XIVLauncher 托管：使用/下载官方 wine-xiv（含 FFXIV 补丁）
    Managed,
    /// 用户指定路径（`wine64` 可执行文件或含 `wine64` 的目录）
    Custom,
    /// 系统 PATH 中的 `wine64`
    System,
}

/// DXVK 相关设置。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct DxvkSettings {
    /// 是否安装/使用 DXVK（FFXIV 为 DX11 游戏，wined3d 渲染错误且性能差，默认开启）
    pub enabled: bool,
    /// `DXVK_HUD` 取值：`"0"` / `"fps"` / `"full"`；`None` 不设置
    pub hud: Option<String>,
    /// 帧率上限（`DXVK_FRAME_RATE`）；`None` 不限制
    pub frame_limit: Option<u32>,
}

impl Default for DxvkSettings {
    fn default() -> Self {
        Self {
            enabled: true,
            hud: None,
            frame_limit: None,
        }
    }
}

/// Wine 运行配置。
///
/// 字段对齐 C# `WineSettings.cs`（StartupType / CustomBinPath / Prefix /
/// EsyncOn / FsyncOn / DebugVars / Env / LogFile），并补充 `System` 类型与 DXVK 设置。
/// 所有字段都有默认值，`serde(default)` 保证旧配置文件缺字段也能解析。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct WineSettings {
    /// 启动方式（默认 `Auto`）
    pub startup_type: WineStartupType,
    /// `Custom` 模式下的路径：`wine64` 可执行文件，或含 `wine64` 的 bin 目录
    pub custom_path: Option<PathBuf>,
    /// Wine prefix 目录；`None` 使用默认 `~/.xiv-launcher-rs/prefix`
    pub prefix: Option<PathBuf>,
    /// 启用 esync（`WINEESYNC=1`）
    pub esync: bool,
    /// 启用 fsync（`WINEFSYNC=1`）
    pub fsync: bool,
    /// 启用 msync（`WINEMSYNC=1`，仅 macOS 生效）
    pub msync: bool,
    /// `WINEDEBUG` 值；`None` 不设置
    pub debug_vars: Option<String>,
    /// 附加环境变量（`k=v`），最后应用，可覆盖上述所有项
    pub env: BTreeMap<String, String>,
    /// DXVK 设置
    pub dxvk: DxvkSettings,
    /// 启用 gamemode（`LD_PRELOAD+=libgamemodeauto.so.0`）
    pub gamemode: bool,
}

impl Default for WineSettings {
    fn default() -> Self {
        Self {
            startup_type: WineStartupType::Auto,
            custom_path: None,
            prefix: None,
            esync: false,
            fsync: false,
            msync: false,
            debug_vars: None,
            env: BTreeMap::new(),
            dxvk: DxvkSettings::default(),
            gamemode: false,
        }
    }
}

/// 整个 `config.toml` 的结构。
///
/// 布局刻意保持扁平：
/// - `game_path`：游戏根目录（与 Wine 无关，因此放在 AppConfig 顶层而不是 WineSettings 里）
/// - 其余顶层字段：Wine 设置（`flatten`，兼容旧配置）
/// - `[dalamud]`：Dalamud 设置 section
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct AppConfig {
    /// 游戏根目录（含 `boot/`、`game/`、`sdo/`）。
    pub game_path: Option<PathBuf>,
    /// 默认大区 ID（如 `1`），CLI 未传 `--area` 时使用。
    pub area: Option<String>,
    #[serde(default, flatten)]
    pub settings: WineSettings,
    #[serde(default)]
    pub dalamud: DalamudSettings,
}

/// 从指定路径加载整个配置。
pub fn load_app(path: &Path) -> AppConfig {
    match std::fs::read_to_string(path) {
        Ok(text) => match toml::from_str(&text) {
            Ok(cfg) => cfg,
            Err(e) => {
                tracing::warn!(path = %path.display(), error = %e, "config parse failed, using defaults");
                AppConfig::default()
            }
        },
        Err(_) => AppConfig::default(),
    }
}

/// 保存整个配置到指定路径。
pub fn save_app(path: &Path, cfg: &AppConfig) -> Result<(), std::io::Error> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let content = toml::to_string_pretty(cfg).map_err(std::io::Error::other)?;
    std::fs::write(path, content)
}

/// 配置文件路径：`~/.xiv-launcher-rs/config.toml`。
pub fn settings_path() -> PathBuf {
    dirs::home_dir()
        .map(|h| h.join(".xiv-launcher-rs/config.toml"))
        .unwrap_or_else(|| PathBuf::from("config.toml"))
}

/// 旧版 `settings.json` 路径（迁移用）。
pub fn legacy_settings_path() -> PathBuf {
    dirs::home_dir()
        .map(|h| h.join(".xiv-launcher-rs/settings.json"))
        .unwrap_or_else(|| PathBuf::from("settings.json"))
}

/// 从指定路径加载 Wine 配置；文件不存在或解析失败时返回默认值。
pub fn load_settings_from(path: &Path) -> WineSettings {
    load_app(path).settings
}

/// 加载默认位置（`~/.xiv-launcher-rs/config.toml`）的整个 AppConfig。
///
/// 若新文件不存在但旧版 `settings.json` 存在，自动迁移并保存。
pub fn load_app_default() -> AppConfig {
    let path = settings_path();
    if !path.exists() {
        let legacy = legacy_settings_path();
        if legacy.exists() {
            let app = load_legacy_app(&legacy);
            if app != AppConfig::default() {
                tracing::info!(path = %legacy.display(), "migrating legacy settings.json to config.toml");
                let _ = save_app(&path, &app);
                return app;
            }
        }
    }
    load_app(&path)
}

/// 加载默认位置的 Wine 设置（兼容旧调用方）。
pub fn load_settings() -> WineSettings {
    load_app_default().settings
}

/// 从旧版 JSON 格式（`settings.json`）加载完整配置。
fn load_legacy_app(path: &Path) -> AppConfig {
    match std::fs::read_to_string(path) {
        Ok(text) => match serde_json::from_str(&text) {
            Ok(app) => app,
            Err(e) => {
                tracing::warn!(path = %path.display(), error = %e, "failed to parse legacy settings.json");
                AppConfig::default()
            }
        },
        Err(_) => AppConfig::default(),
    }
}

/// 保存 Wine 配置到指定路径（保留 `[dalamud]` section）。
pub fn save_settings_to(path: &Path, settings: &WineSettings) -> Result<(), std::io::Error> {
    let mut app = load_app(path);
    app.settings = settings.clone();
    save_app(path, &app)
}

/// 保存配置到默认位置。
pub fn save_settings(settings: &WineSettings) -> Result<(), std::io::Error> {
    save_settings_to(&settings_path(), settings)
}

/// 读取默认位置的 `[dalamud]` 配置。
pub fn load_dalamud_settings() -> DalamudSettings {
    load_app_default().dalamud
}

/// 保存 `[dalamud]` 配置到默认位置（保留 Wine 设置）。
pub fn save_dalamud_settings(settings: &DalamudSettings) -> Result<(), std::io::Error> {
    let path = settings_path();
    let mut app = load_app(&path);
    app.dalamud = settings.clone();
    save_app(&path, &app)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_default_settings() {
        let s = WineSettings::default();
        assert_eq!(s.startup_type, WineStartupType::Auto);
        assert!(s.custom_path.is_none());
        assert!(s.prefix.is_none());
        assert!(!s.esync && !s.fsync && !s.msync);
        assert!(s.dxvk.enabled);
        assert!(!s.gamemode);
    }

    #[test]
    fn test_serde_roundtrip() {
        let mut env = BTreeMap::new();
        env.insert("FOO".to_string(), "bar".to_string());

        let s = WineSettings {
            startup_type: WineStartupType::Custom,
            custom_path: Some(PathBuf::from("/opt/wine/bin")),
            prefix: Some(PathBuf::from("/tmp/myprefix")),
            esync: true,
            fsync: false,
            msync: false,
            debug_vars: Some("+seh".to_string()),
            env,
            dxvk: DxvkSettings {
                enabled: true,
                hud: Some("fps".to_string()),
                frame_limit: Some(60),
            },
            gamemode: true,
        };

        let toml_str = toml::to_string_pretty(&s).unwrap();
        let back: WineSettings = toml::from_str(&toml_str).unwrap();
        assert_eq!(back, s);
    }

    #[test]
    fn test_serde_missing_fields_use_defaults() {
        // 旧配置只有部分字段，缺字段应回退默认值
        let toml_str = "startup_type = \"custom\"\ncustom_path = \"/opt/wine\"\n";
        let s: WineSettings = toml::from_str(toml_str).unwrap();
        assert_eq!(s.startup_type, WineStartupType::Custom);
        assert!(!s.esync);
        assert!(s.dxvk.enabled);
        assert_eq!(s.prefix, None);
    }

    #[test]
    fn test_save_load_roundtrip() {
        let dir = std::env::temp_dir().join(format!("xlrs-settings-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("config.toml");

        let s = WineSettings {
            startup_type: WineStartupType::System,
            esync: true,
            ..Default::default()
        };
        save_settings_to(&path, &s).unwrap();
        let loaded = load_settings_from(&path);
        assert_eq!(loaded, s);

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_app_config_game_path_is_top_level_and_backwards_compatible() {
        let toml_str = "game_path = \"/games/ffxiv\"\narea = \"1\"\nstartup_type = \"system\"\n\n[dalamud]\nenabled = true\n";
        let app: AppConfig = toml::from_str(toml_str).unwrap();
        assert_eq!(app.game_path, Some(PathBuf::from("/games/ffxiv")));
        assert_eq!(app.area, Some("1".to_string()));
        assert_eq!(app.settings.startup_type, WineStartupType::System);
        assert!(app.dalamud.enabled);

        // 旧版 TOML 里 game_path 也是顶层（当时通过 flatten 写在 WineSettings 中），
        // 新的 AppConfig 结构必须继续原样解析；area 缺省时为 None。
        let legacy_toml = "game_path = \"/games/ffxiv\"\nstartup_type = \"system\"\n\n[dalamud]\nenabled = true\n";
        let legacy_app: AppConfig = toml::from_str(legacy_toml).unwrap();
        assert_eq!(legacy_app.area, None);

        let roundtrip = toml::to_string_pretty(&app).unwrap();
        let back: AppConfig = toml::from_str(&roundtrip).unwrap();
        assert_eq!(back, app);
    }

    #[test]
    fn test_load_missing_file_returns_default() {
        let dir =
            std::env::temp_dir().join(format!("xlrs-settings-missing-{}", std::process::id()));
        let path = dir.join("nope.toml");
        let s = load_settings_from(&path);
        assert_eq!(s, WineSettings::default());
    }
}
