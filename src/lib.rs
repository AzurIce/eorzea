pub mod auth;
pub mod commands;
pub mod game_files;
pub mod term_img;
pub mod game;
pub mod launcher;
pub mod config;
pub mod wine;

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    let launcher = launcher::Launcher::new().expect("failed to create Launcher");
    let state = commands::AppState {
        launcher,
        qr: tokio::sync::Mutex::new(None),
        push: tokio::sync::Mutex::new(None),
        tokens: tokio::sync::Mutex::new(std::collections::HashMap::new()),
        areas: tokio::sync::Mutex::new(None),
    };

    tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .manage(state)
        .invoke_handler(tauri::generate_handler![
            commands::get_settings,
            commands::save_settings,
            commands::list_accounts,
            commands::set_default_account,
            commands::remove_account,
            commands::qr_login_start,
            commands::qr_login_wait,
            commands::push_login_start,
            commands::push_login_wait,
            commands::password_login,
            commands::auto_login,
            commands::list_areas,
            commands::game_status,
            commands::check_game,
            commands::update_game,
            commands::launch_game,
            commands::game_root_valid,
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
