pub mod auth;
pub mod config;
pub mod dalamud;
pub mod game;
pub mod game_files;
pub mod launcher;
pub mod term_img;
pub mod ui;
/// Wine 兼容层：仅 Linux/macOS。Windows 原生启动不使用 Wine。
#[cfg(not(target_os = "windows"))]
pub mod wine;
