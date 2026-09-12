//! 设置页：游戏目录、Wine 与 Dalamud 配置，保存写回 `config.toml`。

use crate::config::{self, AppConfig, WineStartupType};
use crate::dalamud::model::DalamudLoadMethod;
use dioxus::prelude::*;

use super::login::{ActionButton, GhostButton, Section, TextInput};
use super::{AppState, Dropdown};

#[component]
pub fn SettingsPage() -> Element {
    let mut state = use_context::<AppState>();
    let t = (state.theme)();

    // 草稿状态（进入页面时从当前内存配置初始化）
    let mut game_path = use_signal(String::new);
    let mut area = use_signal(String::new);
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
    // Some(通道) 供下拉框显示；None 视为 release
    let mut dalamud_track = use_signal(|| Some("release".to_string()));
    let mut dalamud_delay_ms = use_signal(String::new);
    let mut dalamud_install_root = use_signal(String::new);
    let mut dalamud_no_plugins = use_signal(|| false);
    let mut dalamud_no_third_party = use_signal(|| false);

    use_hook(|| {
        let s = state.settings.read();
        game_path.set(path_to_string(&state.game_path.read()));
        // area 在 AppConfig 顶层（AppState 只镜像 game_path），进入页面时读一次
        area.set(config::load_app_default().area.unwrap_or_default());
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
        dalamud_track.set(Some(if d.track.trim().is_empty() {
            "release".to_string()
        } else {
            d.track.clone()
        }));
        dalamud_delay_ms.set(if d.delay_initialize_ms == 0 {
            String::new()
        } else {
            d.delay_initialize_ms.to_string()
        });
        dalamud_install_root.set(path_to_string(&d.install_root));
        dalamud_no_plugins.set(d.no_plugins);
        dalamud_no_third_party.set(d.no_third_party_plugins);
    });

    // ── 浏览游戏根目录（rfd 原生目录选择对话框，阻塞调用放 spawn_blocking）──
    let browse_game_path = move |_: MouseEvent| {
        #[cfg(target_arch = "wasm32")]
        {
            // 浏览器预览没有原生目录选择（rfd 同步 API 仅原生平台）
            state
                .status
                .set("Web 预览不支持目录选择，请手动输入路径".into());
        }
        #[cfg(not(target_arch = "wasm32"))]
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
        d.track = dalamud_track().unwrap_or_else(|| "release".to_string());
        d.delay_initialize_ms = parse_u32(&dalamud_delay_ms.read()).unwrap_or(0);
        d.install_root = string_to_path(&dalamud_install_root.read());
        d.no_plugins = dalamud_no_plugins();
        d.no_third_party_plugins = dalamud_no_third_party();

        let area_id = area.read().trim().to_string();
        let app = AppConfig {
            game_path: string_to_path(&game_path.read()),
            area: if area_id.is_empty() {
                None
            } else {
                Some(area_id)
            },
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

    // 大区 ID 对照（随服务端大区列表动态生成）
    let area_hint = if state.areas.read().is_empty() {
        "大区列表加载中…".to_string()
    } else {
        state
            .areas
            .read()
            .iter()
            .map(|a| format!("{}={}", a.area_id, a.area_name))
            .collect::<Vec<_>>()
            .join(" · ")
    };

    // 更新通道下拉选项；配置为自定义通道时补进列表以正确回显当前值
    let mut track_items = vec![
        ("release".to_string(), "稳定版（release）".to_string()),
        ("staging".to_string(), "测试版（staging）".to_string()),
    ];
    let cur_track = dalamud_track
        .read()
        .clone()
        .unwrap_or_else(|| "release".to_string());
    if !track_items.iter().any(|(k, _)| *k == cur_track) {
        track_items.push((cur_track.clone(), format!("自定义（{cur_track}）")));
    }

    // Wine 配置仅在依赖 wine 的平台展示：Windows 原生启动忽略 wine 参数；
    // esync/fsync/gamemode 依赖 Linux 内核特性，macOS（wine/CrossOver）不支持，隐藏对应开关。
    // 隐藏的开关不重置草稿值，保存时原样写回 config.toml。
    let show_wine_section = cfg!(not(target_os = "windows"));
    let show_linux_wine_toggles = cfg!(target_os = "linux");

    rsx! {
        div {
            style: "flex: 1; min-height: 0; display: flex; flex-direction: column; min-width: 0;",

            // 表单滚动区；保存栏在滚动区外固定，任何位置都能直接保存
            div {
                style: "flex: 1; overflow-y: auto; padding: 24px 28px; display: flex; flex-direction: column; gap: 24px;",

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
                SettingsRow { label: "默认大区",
                    div {
                        style: "display: flex; flex-direction: column; gap: 6px;",
                        TextInput {
                            placeholder: "填下方列出的大区 ID，留空则每次启动到主页选择",
                            value: area,
                        }
                        p {
                            style: "margin: 0; font-size: 12px; color: {t.text_secondary};",
                            "启动时默认使用；留空则每次到主页选择。当前可用：{area_hint}"
                        }
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

            if show_wine_section {
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
                            placeholder: "留空使用默认 ~/.eorzea/prefix",
                            value: prefix,
                        }
                    }

                    div {
                        style: "display: flex; flex-direction: row; gap: 24px; margin-top: 4px; flex-wrap: wrap;",
                        if show_linux_wine_toggles {
                            Checkbox { label: "esync", checked: esync }
                            Checkbox { label: "fsync", checked: fsync }
                        }
                        Checkbox { label: "msync", checked: msync }
                        Checkbox { label: "DXVK", checked: dxvk }
                        if show_linux_wine_toggles {
                            Checkbox { label: "gamemode", checked: gamemode }
                        }
                    }
                }
            }

            Section { title: "Dalamud",
                Checkbox { label: "启用 Dalamud（游戏内插件框架）", checked: dalamud_enabled }

                if dalamud_enabled() {
                    div {
                        style: "margin-top: 16px; display: flex; flex-direction: column; gap: 12px;",
                        SettingsRow { label: "加载方式",
                            div {
                                style: "display: flex; flex-direction: column; gap: 6px;",
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
                                p {
                                    style: "margin: 0; font-size: 12px; color: {t.text_secondary};",
                                    "入口点最稳定；DLL 注入为传统方式；仅兼容修复不加载 Dalamud 本体，用于排查问题。"
                                }
                            }
                        }
                        SettingsRow { label: "更新通道",
                            div {
                                style: "display: flex; flex-direction: column; gap: 6px; max-width: 360px;",
                                Dropdown {
                                    id: "settings-track",
                                    items: track_items,
                                    selected: dalamud_track,
                                    placeholder: "选择通道",
                                }
                                p {
                                    style: "margin: 0; font-size: 12px; color: {t.text_secondary};",
                                    "测试版适配新游戏版本更快，但可能不稳定。"
                                }
                            }
                        }
                        SettingsRow { label: "安装目录",
                            div {
                                style: "display: flex; flex-direction: column; gap: 6px;",
                                TextInput {
                                    placeholder: "~/.eorzea/dalamud（默认，留空即用）",
                                    value: dalamud_install_root,
                                }
                                p {
                                    style: "margin: 0; font-size: 12px; color: {t.text_secondary};",
                                    "默认 ~/.eorzea/dalamud，首次启动游戏时自动下载，之后版本匹配直接复用。"
                                }
                            }
                        }
                        SettingsRow { label: "初始化延迟",
                            div {
                                style: "display: flex; flex-direction: column; gap: 6px;",
                                TextInput {
                                    placeholder: "毫秒，0 = 不延迟",
                                    value: dalamud_delay_ms,
                                }
                                p {
                                    style: "margin: 0; font-size: 12px; color: {t.text_secondary};",
                                    "0 = 不延迟；启动卡顿时可调大。"
                                }
                            }
                        }
                        div {
                            style: "display: flex; flex-direction: column; gap: 6px;",
                            div {
                                style: "display: flex; flex-direction: row; gap: 24px; flex-wrap: wrap;",
                                Checkbox { label: "禁用所有插件（safe mode）", checked: dalamud_no_plugins }
                                Checkbox { label: "禁用第三方插件", checked: dalamud_no_third_party }
                            }
                            p {
                                style: "margin: 0; font-size: 12px; color: {t.text_secondary};",
                                "排查插件问题时使用：前者不加载任何插件，后者仅屏蔽第三方仓库插件。"
                            }
                        }
                        p {
                            style: "font-size: 12px; color: {t.text_secondary}; margin-top: 4px;",
                            "首次启用时启动游戏会按需下载 release；若准备失败（包括 release 尚未支持当前游戏版本）会直接报错、不启动游戏——可用主页的「本次启动加载 Dalamud」开关本次跳过。"
                        }
                    }
                }
            }
            }

            // 保存栏固定在滚动区外，不随表单滚动
            div {
                style: "flex-shrink: 0; padding: 12px 28px; border-top: 1px solid {t.border}; background: {t.page_bg};",
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
                // 必须用 oninput：原生 blitz 对 checkbox 只派发 input 事件、
                // 不派发 change，onchange 在桌面端永远不会触发（勾选只是
                // 控件自身视觉切换，signal 不更新）；web 端浏览器点击
                // checkbox 同样触发 input，两端一致。checked() 从 value
                // （"true"/"false"，两端渲染器均归一为此格式）解析布尔
                oninput: move |e| checked.set(e.checked()),
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
    if s.is_empty() { None } else { Some(s.into()) }
}

fn parse_u32(s: &str) -> Option<u32> {
    let s = s.trim();
    if s.is_empty() {
        Some(0)
    } else {
        s.parse().ok()
    }
}
