//! Wine 检测/解析/校验示例。
//!
//! 用法：
//! ```text
//! cargo run -p xiv-launcher-rs --example wine_test
//! cargo run -p xiv-launcher-rs --example wine_test -- /path/to/custom/wine64-or-bin-dir
//! ```
//!
//! 演示基于 `WineSettings` 的解析流程（不再直接调 `detect/ensure`）：
//! 配置 → `WineTool::resolve` → `probe`（打印版本）。

use tracing::{error, info};

use xiv_launcher_rs_lib::settings::{WineSettings, WineStartupType};
use xiv_launcher_rs_lib::wine::WineTool;

#[tokio::main]
async fn main() {
    tracing_subscriber::fmt::init();

    // 可选参数：自定义 wine 路径（wine64 文件或 bin 目录）
    let custom = std::env::args().nth(1);

    let settings = if let Some(path) = custom {
        info!("Custom wine path: {path}");
        WineSettings {
            startup_type: WineStartupType::Custom,
            custom_path: Some(path.into()),
            ..Default::default()
        }
    } else {
        info!("Using default (Auto) wine settings");
        WineSettings::default()
    };

    match WineTool::resolve(&settings).await {
        Ok(tool) => {
            info!("Wine resolved:");
            info!("  Path: {:?}", tool.wine64_path);
            info!("  Prefix: {:?}", tool.prefix_path);
            info!("  Managed: {}", tool.is_managed);
            match tool.probe() {
                Ok(version) => info!("  Version: {version}"),
                Err(e) => info!("  Probe failed: {e}"),
            }

            // 展示将传给子进程的环境变量
            info!("Launch env:");
            for (k, v) in xiv_launcher_rs_lib::wine::build_launch_env(&settings, &tool) {
                info!("  {k}={v}");
            }
        }
        Err(e) => {
            error!("Failed to resolve wine: {e}");
            std::process::exit(1);
        }
    }
}
