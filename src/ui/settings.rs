//! 设置页：游戏目录、Wine 与 Dalamud 配置，保存写回 `config.toml`。

use dioxus::prelude::*;
use eorzea_lib::config::{self, AppConfig, WineStartupType};
use eorzea_lib::dalamud::model::DalamudLoadMethod;

use super::login::{ActionButton, GhostButton, Section, TextInput};
use super::AppState;

#[component]
pub fn SettingsPage() -> Element {
    let mut state = use_context::<AppState>();
    let t = (state.theme)();

    // 草稿状态（进入页面时从当前内存配置初始化）
    let mut game_path = use_signal(String::new);
    let mut startup_type = use_signal(|| WineStartupType::Auto);
    let mut custom_path = use_signal(String::new);
    let mut prefix = use_signal(String::new);
    let mut esync = use_signal(|| false);
    let mut fsync = use_signal(|| false);
    let mut msync = use_signal(|| false);
    let mut dxvk = use_signal(|| true);
    let mut gamemode = use_signal(|| false);

    let mut dalamud_enabled = use_signal(|| false);
    let mut dalamud_load_method = use_signal(|| DalamudLoadMethod::EntryPoint);
    let mut dalamud_track = use_signal(String::new);
    let mut dalamud_delay_ms = use_signal(String::new);
    let mut dalamud_no_plugins = use_signal(|| false);
    let mut dalamud_no_third_party = use_signal(|| false);

    use_hook(|| {
        let s = state.settings.read();
        game_path.set(path_to_string(&state.game_path.read()));
        startup_type.set(s.startup_type);
        custom_path.set(path_to_string(&s.custom_path));
        prefix.set(path_to_string(&s.prefix));
        esync.set(s.esync);
        fsync.set(s.fsync);
        msync.set(s.msync);
        dxvk.set(s.dxvk.enabled);
        gamemode.set(s.gamemode);

        let d = state.dalamud_cfg.read();
        dalamud_enabled.set(d.enabled);
        dalamud_load_method.set(d.load_method);
        dalamud_track.set(d.track.clone());
        dalamud_delay_ms.set(if d.delay_initialize_ms == 0 {
            String::new()
        } else {
            d.delay_initialize_ms.to_string()
        });
        dalamud_no_plugins.set(d.no_plugins);
        dalamud_no_third_party.set(d.no_third_party_plugins);
    });

    // ── 浏览游戏根目录（rfd 原生目录选择对话框，阻塞调用放 spawn_blocking）──
    let browse_game_path = move |_: MouseEvent| {
        spawn(async move {
            let picked = tokio::task::spawn_blocking(|| {
                rfd::FileDialog::new()
                    .set_title("选择游戏根目录")
                    .pick_folder()
            })
            .await;
            match picked {
                Ok(Some(path)) => game_path.set(path.display().to_string()),
                Ok(None) => {}
                Err(e) => state.status.set(format!("目录选择对话框失败: {e}")),
            }
        });
    };

    let save = move |_: MouseEvent| {
        let mut s = state.settings.read().clone();
        s.startup_type = startup_type();
        s.custom_path = string_to_path(&custom_path.read());
        s.prefix = string_to_path(&prefix.read());
        s.esync = esync();
        s.fsync = fsync();
        s.msync = msync();
        s.dxvk.enabled = dxvk();
        s.gamemode = gamemode();

        let mut d = state.dalamud_cfg.read().clone();
        d.enabled = dalamud_enabled();
        d.load_method = dalamud_load_method();
        let track = dalamud_track.read().trim().to_string();
        d.track = if track.is_empty() {
            "release".to_string()
        } else {
            track
        };
        d.delay_initialize_ms = parse_u32(&dalamud_delay_ms.read()).unwrap_or(0);
        d.no_plugins = dalamud_no_plugins();
        d.no_third_party_plugins = dalamud_no_third_party();

        let app = AppConfig {
            game_path: string_to_path(&game_path.read()),
            settings: s.clone(),
            dalamud: d.clone(),
        };
        match config::save_app(&config::settings_path(), &app) {
            Ok(()) => {
                state.game_path.set(app.game_path.clone());
                state.settings.set(s);
                state.dalamud_cfg.set(d);
                state.status.set("设置已保存".into());
            }
            Err(e) => state.status.set(format!("保存设置失败: {e}")),
        }
    };

    // 游戏路径即时校验提示（只做存在性检查，不等保存）
    let draft_root = string_to_path(&game_path.read());
    let draft_game_ok = draft_root
        .as_ref()
        .map(|p| p.join("game/ffxiv_dx11.exe").exists())
        .unwrap_or(false);
    let game_hint_color = if draft_game_ok { t.success } else { t.warning };

    rsx! {
        div {
            style: "display: flex; flex-direction: column; gap: 24px;",

            Section { title: "游戏",
                SettingsRow { label: "游戏根目录",
                    div {
                        style: "display: flex; flex-direction: row; gap: 8px; align-items: center;",
                        TextInput {
                            placeholder: "例如 /games/ffxiv（含 boot/、game/、sdo/）",
                            value: game_path,
                        }
                        GhostButton { label: "浏览…", onclick: browse_game_path }
                    }
                }
                if let Some(root) = &draft_root {
                    p {
                        style: "margin: -4px 0 0 92px; font-size: 12px; color: {game_hint_color}; overflow-wrap: anywhere;",
                        if draft_game_ok {
                            "✓ 已找到 game/ffxiv_dx11.exe"
                        } else {
                            "⚠ 未找到 {root.display()}/game/ffxiv_dx11.exe，启动前请确认路径"
                        }
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
                                let bg = if active { t.active_bg } else { "transparent" };
                                let fg = if active { t.text } else { t.text_secondary };
                                rsx! {
                                    button {
                                        key: "{name}",
                                        style: "padding: 6px 14px; border: 1px solid {t.border}; border-radius: 6px; background: {bg}; color: {fg}; font-size: 13px; cursor: pointer;",
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
                    Checkbox { label: "msync", checked: msync }
                    Checkbox { label: "DXVK", checked: dxvk }
                    Checkbox { label: "gamemode", checked: gamemode }
                }
            }

            Section { title: "Dalamud",
                Checkbox { label: "启用 Dalamud（游戏内插件框架）", checked: dalamud_enabled }

                if dalamud_enabled() {
                    div {
                        style: "margin-top: 16px; display: flex; flex-direction: column; gap: 12px;",
                        SettingsRow { label: "加载方式",
                            div {
                                style: "display: flex; flex-direction: row; gap: 8px; flex-wrap: wrap;",
                                for (method, name, hint) in [
                                    (DalamudLoadMethod::EntryPoint, "入口点", "推荐"),
                                    (DalamudLoadMethod::DllInject, "DLL 注入", "传统方式"),
                                    (DalamudLoadMethod::AclOnly, "仅兼容修复", "不加载插件"),
                                ] {
                                    {
                                        let active = dalamud_load_method() == method;
                                        let bg = if active { t.active_bg } else { "transparent" };
                                        let fg = if active { t.text } else { t.text_secondary };
                                        rsx! {
                                            button {
                                                key: "{hint}",
                                                style: "padding: 6px 14px; border: 1px solid {t.border}; border-radius: 6px; background: {bg}; color: {fg}; font-size: 13px; cursor: pointer;",
                                                onclick: move |_| dalamud_load_method.set(method),
                                                "{name} · {hint}"
                                            }
                                        }
                                    }
                                }
                            }
                        }
                        SettingsRow { label: "更新通道",
                            TextInput {
                                placeholder: "release / staging / 自定义 track",
                                value: dalamud_track,
                            }
                        }
                        SettingsRow { label: "初始化延迟",
                            TextInput {
                                placeholder: "毫秒（留空为 0）",
                                value: dalamud_delay_ms,
                            }
                        }
                        div {
                            style: "display: flex; flex-direction: row; gap: 24px; flex-wrap: wrap;",
                            Checkbox { label: "禁用所有插件（safe mode）", checked: dalamud_no_plugins }
                            Checkbox { label: "禁用第三方插件", checked: dalamud_no_third_party }
                        }
                        p {
                            style: "font-size: 12px; color: {t.text_secondary}; margin-top: 4px;",
                            "首次启用时启动游戏会按需下载 release；若游戏版本尚未被 release 支持，将自动降级为不加载 Dalamud。"
                        }
                    }
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
    let t = (use_context::<AppState>().theme)();
    rsx! {
        div {
            style: "display: flex; flex-direction: row; align-items: center; gap: 12px; margin-bottom: 12px;",
            span { style: "width: 80px; font-size: 14px; color: {t.text_secondary}; flex-shrink: 0;", "{label}" }
            div { style: "flex: 1; min-width: 0;", {children} }
        }
    }
}

/// 复选框。
#[component]
pub fn Checkbox(label: &'static str, checked: Signal<bool>) -> Element {
    let t = (use_context::<AppState>().theme)();
    rsx! {
        label {
            style: "display: flex; flex-direction: row; align-items: center; gap: 6px; font-size: 14px; color: {t.text}; cursor: pointer;",
            input {
                r#type: "checkbox",
                // blitz 用 currentColor 作 accent-color：勾选=底色填充+白勾，
                // 要显式给深色，否则白底白勾不可见
                style: "color: {t.checkbox_accent};",
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

fn parse_u32(s: &str) -> Option<u32> {
    let s = s.trim();
    if s.is_empty() {
        Some(0)
    } else {
        s.parse().ok()
    }
}
