//! 设置页：游戏目录与 Wine 配置，保存写回 `config.toml`。

use dioxus::prelude::*;
use xiv_launcher_rs_lib::config::{self, WineStartupType};

use super::login::{ActionButton, Section, TextInput};
use super::AppState;

#[component]
pub fn SettingsPage() -> Element {
    let mut state = use_context::<AppState>();

    // 草稿状态（进入页面时从当前设置初始化）
    let mut game_path = use_signal(String::new);
    let mut startup_type = use_signal(|| WineStartupType::Auto);
    let mut custom_path = use_signal(String::new);
    let mut prefix = use_signal(String::new);
    let mut esync = use_signal(|| false);
    let mut fsync = use_signal(|| false);
    let mut dxvk = use_signal(|| true);
    let mut gamemode = use_signal(|| false);
    use_hook(|| {
        let s = state.settings.read();
        game_path.set(path_to_string(&s.game_path));
        startup_type.set(s.startup_type);
        custom_path.set(path_to_string(&s.custom_path));
        prefix.set(path_to_string(&s.prefix));
        esync.set(s.esync);
        fsync.set(s.fsync);
        dxvk.set(s.dxvk.enabled);
        gamemode.set(s.gamemode);
    });

    let save = move |_: MouseEvent| {
        let mut s = state.settings.read().clone();
        s.game_path = string_to_path(&game_path.read());
        s.startup_type = startup_type();
        s.custom_path = string_to_path(&custom_path.read());
        s.prefix = string_to_path(&prefix.read());
        s.esync = esync();
        s.fsync = fsync();
        s.dxvk.enabled = dxvk();
        s.gamemode = gamemode();
        match config::save_settings(&s) {
            Ok(()) => {
                state.settings.set(s);
                state.status.set("设置已保存".into());
            }
            Err(e) => state.status.set(format!("保存设置失败: {e}")),
        }
    };

    rsx! {
        div {
            style: "display: flex; flex-direction: column; gap: 24px;",

            Section { title: "游戏",
                SettingsRow { label: "游戏根目录",
                    TextInput {
                        placeholder: "例如 /games/ffxiv（含 boot/、game/、sdo/）",
                        value: game_path,
                    }
                }
            }

            Section { title: "Wine",
                SettingsRow { label: "启动方式",
                    div {
                        style: "display: flex; flex-direction: row; gap: 8px;",
                        for (ty, name) in [
                            (WineStartupType::Auto, "自动"),
                            (WineStartupType::Managed, "托管"),
                            (WineStartupType::Custom, "自定义"),
                            (WineStartupType::System, "系统"),
                        ] {
                            {
                                let active = startup_type() == ty;
                                let bg = if active { "#27272a" } else { "transparent" };
                                let fg = if active { "#fafafa" } else { "#a1a1aa" };
                                rsx! {
                                    button {
                                        key: "{name}",
                                        style: "padding: 6px 14px; border: 1px solid #27272a; border-radius: 6px; background: {bg}; color: {fg}; font-size: 13px; cursor: pointer;",
                                        onclick: move |_| startup_type.set(ty),
                                        "{name}"
                                    }
                                }
                            }
                        }
                    }
                }
                if startup_type() == WineStartupType::Custom {
                    SettingsRow { label: "自定义路径",
                        TextInput {
                            placeholder: "wine64 可执行文件或含 wine64 的 bin 目录",
                            value: custom_path,
                        }
                    }
                }
                SettingsRow { label: "Prefix",
                    TextInput {
                        placeholder: "留空使用默认 ~/.xiv-launcher-rs/prefix",
                        value: prefix,
                    }
                }

                div {
                    style: "display: flex; flex-direction: row; gap: 24px; margin-top: 4px; flex-wrap: wrap;",
                    Checkbox { label: "esync", checked: esync }
                    Checkbox { label: "fsync", checked: fsync }
                    Checkbox { label: "DXVK", checked: dxvk }
                    Checkbox { label: "gamemode", checked: gamemode }
                }
            }

            div {
                ActionButton { label: "保存设置", onclick: save }
            }
        }
    }
}

/// 带标签的设置行。
#[component]
fn SettingsRow(label: &'static str, children: Element) -> Element {
    rsx! {
        div {
            style: "display: flex; flex-direction: row; align-items: center; gap: 12px; margin-bottom: 12px;",
            span { style: "width: 80px; font-size: 14px; color: #a1a1aa; flex-shrink: 0;", "{label}" }
            div { style: "flex: 1;", {children} }
        }
    }
}

/// 复选框。
#[component]
fn Checkbox(label: &'static str, checked: Signal<bool>) -> Element {
    rsx! {
        label {
            style: "display: flex; flex-direction: row; align-items: center; gap: 6px; font-size: 14px; color: #fafafa; cursor: pointer;",
            input {
                r#type: "checkbox",
                // blitz 用 currentColor 作 accent-color：勾选=底色填充+白勾，
                // 深色主题下要显式给深色，否则白底白勾不可见
                style: "color: #18181b;",
                checked: checked(),
                onchange: move |e| checked.set(e.checked()),
            }
            "{label}"
        }
    }
}

fn path_to_string(p: &Option<std::path::PathBuf>) -> String {
    p.as_ref()
        .map(|p| p.display().to_string())
        .unwrap_or_default()
}

fn string_to_path(s: &str) -> Option<std::path::PathBuf> {
    let s = s.trim();
    if s.is_empty() {
        None
    } else {
        Some(s.into())
    }
}
