//! 设置页：游戏目录、Wine 与 Dalamud 配置，保存写回 `config.toml`。

use dioxus::prelude::*;
use eorzea::config::{self, AppConfig, SyncMode, WineStartupType};
use eorzea::dalamud::model::{DalamudLoadMethod, DalamudSettings};

use super::login::{ActionButton, GhostButton, Section, TextInput};
use super::{AppState, Dropdown};

#[component]
pub fn SettingsPage() -> Element {
    let mut state = use_context::<AppState>();
    let t = (state.theme)();

    // 提示直接复用运行时默认值，不在 UI 中维护另一份路径或数字。
    #[cfg(not(target_os = "windows"))]
    let default_prefix = eorzea::wine::WineTool::default_prefix_path()
        .display()
        .to_string();
    #[cfg(target_os = "windows")]
    let default_prefix = String::new(); // Windows 隐藏 Wine 设置
    let default_install_root = eorzea::dalamud::updater::default_install_root()
        .display()
        .to_string();
    let default_delay_ms = DalamudSettings::default().delay_initialize_ms.to_string();

    // 草稿状态（进入页面时从当前内存配置初始化）
    let mut game_path = use_signal(String::new);
    let mut startup_type = use_signal(|| WineStartupType::Auto);
    let mut custom_path = use_signal(String::new);
    let mut prefix = use_signal(String::new);
    // esync / fsync 是互斥的同步机制，UI 用单选表示。
    let mut sync_mode = use_signal(|| SyncMode::None);
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

    // 从当前已保存配置重置草稿；进入页面和「恢复」按钮共用。
    let reset_drafts = use_callback(move |_: ()| {
        let s = state.settings.read();
        game_path.set(path_to_string(&state.game_path.read()));
        startup_type.set(s.startup_type);
        custom_path.set(path_to_string(&s.custom_path));
        prefix.set(path_to_string(&s.prefix));
        sync_mode.set(s.sync.mode());
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
    use_hook(move || reset_drafts.call(()));

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
        s.sync = sync_mode().into();
        s.msync = msync();
        s.dxvk.enabled = dxvk();
        s.gamemode = gamemode();

        let mut d = state.dalamud_cfg.read().clone();
        d.enabled = dalamud_enabled();
        d.load_method = dalamud_load_method();
        d.track = dalamud_track().unwrap_or_else(|| "release".to_string());
        d.delay_initialize_ms = parse_u32(&dalamud_delay_ms.read())
            .unwrap_or_else(|| DalamudSettings::default().delay_initialize_ms);
        d.install_root = string_to_path(&dalamud_install_root.read());
        d.no_plugins = dalamud_no_plugins();
        d.no_third_party_plugins = dalamud_no_third_party();

        // 大区已改为账号级设置（主页下拉框写回 auth.toml）；
        // 这里保留旧配置值，避免旧文件里的 area 被设置页保存时清掉。
        let app = AppConfig {
            game_path: string_to_path(&game_path.read()),
            area: config::load_app_default().area,
            #[cfg(not(target_os = "windows"))]
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

    // 丢弃草稿，回到已保存配置
    let revert = move |_: MouseEvent| reset_drafts.call(());

    // 保存按钮只在草稿与已保存配置不同时可用。这里复用保存逻辑的归一化
    // 规则（空 channel → release、空延迟 → 默认值），避免刚进入页面就误判为脏。
    let dirty = {
        let saved = state.settings.read();
        let saved_dalamud = state.dalamud_cfg.read();
        let saved_game_path = state.game_path.read().clone();

        let draft_game_path = string_to_path(&game_path.read());
        let draft_track = dalamud_track().unwrap_or_else(|| "release".to_string());
        let saved_track = if saved_dalamud.track.trim().is_empty() {
            "release".to_string()
        } else {
            saved_dalamud.track.trim().to_string()
        };
        let draft_delay = parse_u32(&dalamud_delay_ms.read())
            .unwrap_or_else(|| DalamudSettings::default().delay_initialize_ms);

        draft_game_path != saved_game_path
            || startup_type() != saved.startup_type
            || string_to_path(&custom_path.read()) != saved.custom_path
            || string_to_path(&prefix.read()) != saved.prefix
            || sync_mode() != saved.sync.mode()
            || msync() != saved.msync
            || dxvk() != saved.dxvk.enabled
            || gamemode() != saved.gamemode
            || dalamud_enabled() != saved_dalamud.enabled
            || dalamud_load_method() != saved_dalamud.load_method
            || draft_track != saved_track
            || draft_delay != saved_dalamud.delay_initialize_ms
            || string_to_path(&dalamud_install_root.read()) != saved_dalamud.install_root
            || dalamud_no_plugins() != saved_dalamud.no_plugins
            || dalamud_no_third_party() != saved_dalamud.no_third_party_plugins
    };

    // Wine 配置仅在依赖 wine 的平台展示：Windows 原生启动忽略 wine 参数；
    // 同步方式/esync/fsync、gamemode 依赖 Linux 内核特性，macOS（wine/CrossOver）
    // 不支持，隐藏对应控件。隐藏的开关不重置草稿值，保存时原样写回 config.toml。
    let show_wine_section = cfg!(not(target_os = "windows"));
    let show_linux_wine_toggles = cfg!(target_os = "linux");
    // msync 只有 macOS 的 Wine 构建支持（运行时也是 `cfg(target_os = "macos")`
    // 才设置 WINEMSYNC），Linux 上不显示，避免给出无效开关。
    let show_msync_toggle = cfg!(target_os = "macos");

    rsx! {
        div {
            style: "flex: 1; min-height: 0; display: flex; flex-direction: column; min-width: 0;",

            // 表单滚动区；保存栏在滚动区外固定，任何位置都能直接保存
            div {
                style: "flex: 1; overflow-y: auto; padding: 24px 28px; display: flex; flex-direction: column; gap: 24px;",

                Section { title: "游戏",
                SettingsRow { label: "游戏根目录",
                    div {
                        style: "display: flex; flex-direction: column; gap: 6px;",
                        div {
                            style: "display: flex; flex-direction: row; gap: 8px; align-items: center;",
                            TextInput { value: game_path }
                            GhostButton { label: "浏览…", onclick: browse_game_path }
                        }
                        p {
                            style: "margin: 0; font-size: 12px; color: {t.text_secondary};",
                            "选择包含 boot/、game/、sdo/ 的游戏根目录。"
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
                Section { title: "Wine", badge: Some("Linux / macOS"),
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
                            div {
                                style: "display: flex; flex-direction: column; gap: 6px;",
                                TextInput { value: custom_path }
                                p {
                                    style: "margin: 0; font-size: 12px; color: {t.text_secondary};",
                                    "指定 wine64 / wine 可执行文件，或包含它们的目录（也会搜索 bin/）。"
                                }
                            }
                        }
                    }
                    SettingsRow { label: "Prefix",
                        TextInput {
                            placeholder: default_prefix,
                            value: prefix,
                        }
                    }

                    // 同步方式是一个三档滑块：关 / esync / fsync。esync 与 fsync
                    // 互斥，滑到哪一档就用哪个后端；滑到「关」则用 Wine 默认的
                    // 服务器同步。
                    if show_linux_wine_toggles {
                        {
                            let sync_index = match sync_mode() {
                                SyncMode::None => 0,
                                SyncMode::Esync => 1,
                                SyncMode::Fsync => 2,
                            };
                            let thumb_left = ["0%", "33.333%", "66.666%"][sync_index];
                            rsx! {
                                SettingsRow { label: "同步方式",
                                    // 整个轨道是一个控件，黑色滑块在三个档位之间移动
                                    div {
                                        style: "position: relative; display: flex; flex-direction: row; width: 264px; max-width: 100%; height: 28px; box-sizing: border-box; border: 1px solid {t.border}; border-radius: 7px; background: {t.active_bg};",
                                        div {
                                            style: "position: absolute; top: 2px; bottom: 2px; width: 33.333%; left: {thumb_left}; border-radius: 5px; background: {t.primary_bg};",
                                        }
                                        for (mode, name) in [
                                            (SyncMode::None, "关"),
                                            (SyncMode::Esync, "esync"),
                                            (SyncMode::Fsync, "fsync"),
                                        ] {
                                            {
                                                let active = sync_mode() == mode;
                                                let fg = if active { t.primary_fg } else { t.text_secondary };
                                                rsx! {
                                                    div {
                                                        style: "position: relative; z-index: 1; flex: 1; display: flex; align-items: center; justify-content: center; font-size: 12px; color: {fg}; cursor: pointer;",
                                                        onclick: move |_| sync_mode.set(mode),
                                                        "{name}"
                                                    }
                                                }
                                            }
                                        }
                                    }
                                }
                            }
                        }
                    }

                    if show_msync_toggle {
                        SwitchRow {
                            label: "msync",
                            hint: "macOS 的 futex 同步",
                            checked: msync(),
                            onchange: move |on| msync.set(on),
                        }
                    }

                    SwitchRow {
                        label: "DXVK",
                        hint: "D3D9/10/11 转 Vulkan",
                        checked: dxvk(),
                        onchange: move |on| dxvk.set(on),
                    }

                    if show_linux_wine_toggles {
                        SwitchRow {
                            label: "gamemode",
                            hint: "接入 Feral GameMode",
                            checked: gamemode(),
                            onchange: move |on| gamemode.set(on),
                        }
                    }
                }
            }

            Section { title: "Dalamud",
                Checkbox { label: "启用 Dalamud", checked: dalamud_enabled }

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
                                    placeholder: default_install_root,
                                    value: dalamud_install_root,
                                }
                                p {
                                    style: "margin: 0; font-size: 12px; color: {t.text_secondary};",
                                    "留空使用提示的默认目录。首次启动游戏时自动下载，之后版本匹配直接复用。"
                                }
                            }
                        }
                        SettingsRow { label: "初始化延迟",
                            div {
                                style: "display: flex; flex-direction: column; gap: 6px;",
                                TextInput {
                            placeholder: default_delay_ms,
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
                style: "flex-shrink: 0; display: flex; flex-direction: row; gap: 8px; padding: 12px 28px; border-top: 1px solid {t.border}; background: {t.page_bg};",
                GhostButton { label: "恢复", onclick: revert, disabled: !dirty }
                ActionButton { label: "保存设置", onclick: save, disabled: !dirty }
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
            div { style: "width: 80px; font-size: 14px; color: {t.text_secondary}; flex-shrink: 0;", "{label}" }
            div { style: "flex: 1; min-width: 0;", {children} }
        }
    }
}

/// 带名称的开关行：左侧名称/说明，右侧滑动开关。
#[component]
fn SwitchRow(
    label: &'static str,
    checked: bool,
    onchange: EventHandler<bool>,
    #[props(default)] hint: Option<&'static str>,
) -> Element {
    let t = (use_context::<AppState>().theme)();
    let track_bg = if checked {
        t.primary_bg
    } else {
        t.progress_track
    };
    // 40 - padding 2*2 - knob 18 = 18px 行程
    let knob_left = if checked { "18px" } else { "0px" };
    rsx! {
        div {
            role: "switch",
            "aria-checked": checked,
            style: "display: flex; flex-direction: row; align-items: center; gap: 12px; margin-bottom: 12px; cursor: pointer;",
            onclick: move |_| onchange.call(!checked),
            div {
                style: "flex: 1; min-width: 0; display: flex; flex-direction: column; gap: 2px;",
                div { style: "font-size: 14px; color: {t.text};", "{label}" }
                if let Some(hint) = hint {
                    div { style: "font-size: 12px; color: {t.text_secondary};", "{hint}" }
                }
            }
            div {
                style: "flex-shrink: 0; width: 40px; height: 22px; box-sizing: border-box; padding: 2px; border-radius: 11px; background: {track_bg}; display: flex; flex-direction: row; align-items: center;",
                div {
                    style: "width: 18px; height: 18px; border-radius: 9px; background: #ffffff; margin-left: {knob_left};",
                }
            }
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
