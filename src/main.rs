// Prevents additional console window on Windows in release, DO NOT REMOVE!!
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

// UI 组件在 lib（`eorzea_lib::ui`），这里只做平台入口分发：
// - 桌面：dioxus-native（Blitz 渲染）
// - wasm：dioxus web（`dx serve --platform web` 浏览器调试）

#[cfg(not(target_arch = "wasm32"))]
fn main() {
    tracing_subscriber::fmt::init();
    let window = dioxus_native::WindowAttributes::default()
        .with_title("FFXIV 国服启动器")
        .with_inner_size(dioxus_native::LogicalSize::new(720.0, 860.0));
    dioxus_native::launch_cfg(eorzea_lib::ui::app, vec![], vec![Box::new(window)]);
}

#[cfg(target_arch = "wasm32")]
fn main() {
    dioxus::launch(eorzea_lib::ui::app);
}
