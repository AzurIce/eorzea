use tracing::{info, error};

#[tokio::main]
async fn main() {
    tracing_subscriber::fmt::init();

    info!("=== Wine Detection Test ===");

    // 1. 先检测现有 wine
    match xiv_launcher_rs_lib::wine::WineTool::detect(None) {
        Some(tool) => {
            info!("Found existing wine:");
            info!("  Path: {:?}", tool.wine64_path);
            info!("  Prefix: {:?}", tool.prefix_path);
            info!("  Managed: {}", tool.is_managed);
        }
        None => {
            info!("No existing wine found. Will attempt to download...");

            // 2. 尝试下载
            match xiv_launcher_rs_lib::wine::WineTool::ensure(None).await {
                Ok(tool) => {
                    info!("Wine ready!");
                    info!("  Path: {:?}", tool.wine64_path);
                    info!("  Prefix: {:?}", tool.prefix_path);
                }
                Err(e) => {
                    error!("Failed to setup wine: {}", e);
                }
            }
        }
    }
}