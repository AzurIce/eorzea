// Prevents additional console window on Windows in release, DO NOT REMOVE!!
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod ui;

use dioxus_native::{LogicalSize, WindowAttributes};

fn main() {
    tracing_subscriber::fmt::init();
    let window = WindowAttributes::default()
        .with_title("FFXIV 国服启动器")
        .with_inner_size(LogicalSize::new(720.0, 860.0));
    dioxus_native::launch_cfg(ui::app, vec![], vec![Box::new(window)]);
}
