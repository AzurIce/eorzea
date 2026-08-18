//! Dalamud 集成模块。
//!
//! 职责边界（参考 `docs/notes/dalamud_integration.md` 调查报告）：
//! - **获取**：release 元数据（`VersionInfo` API）、版本门控（`SupportedGameVer`）
//! - **编排**：通过 `Dalamud.Injector.exe` 启动游戏（Linux 上在同一 Wine prefix 内）
//! - **不重写**：注入器、CLR hosting（由官方发行包负责）
//!
//! 模块结构：
//! - `model`：版本信息、配置、状态、Injector argv
//! - `updater`：release 获取、版本匹配、安装检测
//! - `runner`：Wine 路径转换、Injector 启动与 JSON 解析

pub mod assets;
pub mod model;
pub mod runner;
pub mod runtime;
pub mod updater;

pub use model::{
    build_injector_launch_args, DalamudLoadMethod, DalamudSettings, DalamudStartInfo,
    DalamudStatus, DalamudVersionInfo, InstallState,
};
pub use assets::ensure_assets;
pub use runtime::ensure_runtime;
pub use updater::{fetch_version_info, local_game_version, status, DalamudError};
