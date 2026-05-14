use std::path::PathBuf;
use xiv_launcher_auth::sdo::SdoAuth;
use xiv_launcher_auth::SdoArea;
use xiv_launcher_rs_lib::game::{GameLaunchConfig, GameLaunchError};

fn prompt(label: &str) -> String {
    print!("{}: ", label);
    std::io::Write::flush(&mut std::io::stdout()).unwrap();
    let mut buf = String::new();
    std::io::stdin().read_line(&mut buf).unwrap();
    buf.trim().to_string()
}

#[tokio::main]
async fn main() {
    tracing_subscriber::fmt::init();

    let areas = SdoAuth::fetch_server_list().await.expect("Failed to fetch server list");
    println!("Available areas:");
    for (i, a) in areas.iter().enumerate() {
        println!("  {}: {}", i, a.area_name);
    }

    let idx: usize = prompt("Select area index (0-3)").parse().unwrap_or(0);
    let area = areas[idx].clone();

    let session_id = prompt("Session ID (ticket)");
    let snda_id = prompt("snda_id");

    let game_path = PathBuf::from("/Volumes/Files/_ffxiv/XIVLauncherGamePath/game/ffxiv_dx11.exe");
    let config = GameLaunchConfig {
        game_path: game_path.clone(),
        session_id,
        snda_id,
        area: area.clone(),
        areas: areas.clone(),
        max_expansion: 1,
        dc_travel_port: None,
        reset_config: 0,
        additional_args: String::new(),
    };

    println!("\nDetecting wine...");
    match xiv_launcher_rs_lib::game::launch_game(&config, None).await {
        Ok(result) => {
            println!("Game launched!");
            println!("Command: {}", result.command);
            println!("PID: {}", result.child.id());
        }
        Err(GameLaunchError::Wine(msg)) => {
            eprintln!("Wine setup failed: {}", msg);
            eprintln!("Please install wine manually or ensure network connection.");
        }
        Err(e) => {
            eprintln!("Launch failed: {}", e);
        }
    }
}