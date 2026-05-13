use std::path::Path;

#[tokio::main]
async fn main() {
    println!("=== Wine Detection Test ===\n");

    // 1. 先检测现有 wine
    match xiv_launcher_rs_lib::wine::WineTool::detect(None) {
        Some(tool) => {
            println!("Found existing wine:");
            println!("  Path: {:?}", tool.wine64_path);
            println!("  Prefix: {:?}", tool.prefix_path);
            println!("  Managed: {}", tool.is_managed);
        }
        None => {
            println!("No existing wine found.");
            println!("Will attempt to download...\n");

            // 2. 尝试下载
            match xiv_launcher_rs_lib::wine::WineTool::ensure(None).await {
                Ok(tool) => {
                    println!("\nWine ready!");
                    println!("  Path: {:?}", tool.wine64_path);
                    println!("  Prefix: {:?}", tool.prefix_path);
                }
                Err(e) => {
                    eprintln!("\nFailed to setup wine: {}", e);
                }
            }
        }
    }
}