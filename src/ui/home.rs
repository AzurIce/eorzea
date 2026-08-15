//! 主页：账号/大区选择、状态仪表盘（游戏位置/版本、Dalamud、Wine）、检查/更新游戏、启动游戏。

use dioxus::prelude::*;
use eorzea_auth::PatchListEntry;
use eorzea_lib::config::WineStartupType;
use eorzea_lib::dalamud::{updater, DalamudStatus, InstallState};
use eorzea_lib::game_files::{CheckResult, GameFileManager};
use eorzea_lib::wine::WineTool;

use super::login::{ActionButton, ErrorRow, Section};
use super::settings::Checkbox;
use super::AppState;

/// 更新流程状态机。
enum UpdateState {
    Idle,
    Checking,
    UpToDate,
    /// 有 N 个补丁待下载。
    NeedsPatch(usize),
    /// 下载中（已下载字节, 总字节）。
    Downloading(u64, u64),
    Installing,
    Done(String),
    Failed(String),
}

impl UpdateState {
    fn is_busy(&self) -> bool {
        matches!(
            self,
            UpdateState::Checking | UpdateState::Downloading(..) | UpdateState::Installing
        )
    }
}

#[component]
pub fn HomePage() -> Element {
    let mut state = use_context::<AppState>();
    let t = (state.theme)();
    let mut update_state = use_signal(|| UpdateState::Idle);
    let mut patches = use_signal(Vec::<PatchListEntry>::new);
    let mut launching = use_signal(|| false);
    // 「本次启动加载 Dalamud」的会话内覆盖，不写回 config.toml。
    let dalamud_this_launch = use_signal(|| state.dalamud_cfg.read().enabled);

    // Dalamud 状态为异步检测（release API + 本地安装 + 版本门控），避免在 render 里做网络/磁盘探测。
    let mut dalamud_status = use_signal(|| None::<DalamudStatus>);
    let mut dalamud_loading = use_signal(|| true);
    use_hook(move || {
        spawn(async move {
            let game_root = state.game_path.read().clone();
            let cfg = state.dalamud_cfg.read().clone();
            if cfg.enabled {
                if let Some(root) = game_root {
                    let install_root = cfg
                        .install_root
                        .clone()
                        .unwrap_or_else(updater::default_install_root);
                    let client = reqwest::Client::new();
                    let st = updater::status(&client, &install_root, &root, &cfg.track).await;
                    dalamud_status.set(Some(st));
                }
            }
            dalamud_loading.set(false);
        });
    });

    let game_root = state.game_path.read().clone();
    // 本地版本（本地文件读取，随 game_path 变化重算）；只取展示用的 boot/game 版本
    let versions = use_memo(move || {
        state.game_path.read().clone().map(|p| {
            let v = GameFileManager::new().status(&p);
            (v.boot, v.ffxiv)
        })
    });

    // ── 检查更新 ────────────────────────────────────────────────────────
    let check_update = move |_: MouseEvent| {
        if update_state.read().is_busy() {
            state.status.set("更新任务进行中，请等待完成".into());
            return;
        }
        let (Some(root), Some(area)) = (game_root.clone(), selected_area(&state)) else {
            state.status.set("请先设置游戏目录并选择大区".into());
            return;
        };
        update_state.set(UpdateState::Checking);
        spawn(async move {
            let mgr = GameFileManager::new();
            match mgr.check_update(&area, &root, false, 5).await {
                Ok(CheckResult::UpToDate { .. }) => {
                    patches.set(Vec::new());
                    update_state.set(UpdateState::UpToDate);
                }
                Ok(CheckResult::NeedsPatch { patches: list, .. }) => {
                    let n = list.len();
                    patches.set(list);
                    update_state.set(UpdateState::NeedsPatch(n));
                }
                Ok(CheckResult::NeedsPatchBoot) => {
                    update_state.set(UpdateState::Failed("boot 需要更新（暂不支持）".into()));
                }
                Err(e) => update_state.set(UpdateState::Failed(format!("{e}"))),
            }
        });
    };

    // ── 更新游戏（下载 + 安装）──────────────────────────────────────────
    let run_update = move |_: MouseEvent| {
        if update_state.read().is_busy() {
            return;
        }
        let Some(root) = state.game_path.read().clone() else {
            state.status.set("请先在设置页配置游戏目录".into());
            return;
        };
        let list = patches.read().clone();
        if list.is_empty() {
            return;
        }
        let patch_dir = dirs::home_dir()
            .map(|h| h.join(".xiv-launcher-rs/patches"))
            .unwrap_or_else(|| "patches".into());
        update_state.set(UpdateState::Downloading(0, 0));
        spawn(async move {
            let mgr = GameFileManager::new();
            let downloaded = mgr
                .download(&list, &patch_dir, 4, |done, total| {
                    update_state.set(UpdateState::Downloading(done, total));
                })
                .await;
            if let Err(e) = downloaded {
                update_state.set(UpdateState::Failed(format!("下载失败: {e}")));
                return;
            }
            update_state.set(UpdateState::Installing);
            match mgr.install(&list, &patch_dir, &root).await {
                Ok(summary) => {
                    patches.set(Vec::new());
                    update_state.set(UpdateState::Done(format!(
                        "更新完成（应用 {} 个补丁，跳过 {} 个）",
                        summary.installed.len(),
                        summary.skipped
                    )));
                }
                Err(e) => update_state.set(UpdateState::Failed(format!("安装失败: {e}"))),
            }
        });
    };

    // ── 启动游戏 ────────────────────────────────────────────────────────
    let launch_game = move |_: MouseEvent| {
        if launching() {
            return;
        }
        let Some(snda_id) = state.selected_account.read().clone() else {
            state
                .status
                .set("请先选择账号（没有账号请到登录页登录）".into());
            return;
        };
        let Some(area) = selected_area(&state) else {
            state.status.set("请选择大区".into());
            return;
        };
        let Some(root) = state.game_path.read().clone() else {
            state.status.set("请先在设置页配置游戏目录".into());
            return;
        };
        let exe = root.join("game/ffxiv_dx11.exe");
        if !exe.is_file() {
            state
                .status
                .set(format!("未找到游戏可执行文件: {}", exe.display()));
            return;
        }
        let Some(launcher) = state.launcher.read().clone() else {
            state.status.set("启动器尚未初始化完成".into());
            return;
        };
        launching.set(true);
        spawn(async move {
            // 本会话没有 token 时先尝试自动登录（key 会轮换，需写回）
            let existing = state.tokens.read().get(&snda_id).cloned();
            let token = match existing {
                Some(t) => Some(t),
                None => {
                    let key = state
                        .auth_cfg
                        .read()
                        .find(&snda_id)
                        .and_then(|a| a.auto_login_session_key.clone());
                    match key {
                        Some(key) => {
                            state.status.set("自动登录中…".into());
                            match launcher.login_auto(&key).await {
                                Ok(t) => {
                                    // 写回轮换后的 session key
                                    let mut cfg = state.auth_cfg.read().clone();
                                    cfg.upsert(
                                        eorzea_lib::auth::Account {
                                            snda_id: t.snda_id.clone(),
                                            username: t.username.clone(),
                                            auto_login_session_key: t
                                                .auto_login_session_key
                                                .clone(),
                                        },
                                        false,
                                    );
                                    let _ = eorzea_lib::auth::save(
                                        &eorzea_lib::auth::config_path(),
                                        &cfg,
                                    );
                                    state.auth_cfg.set(cfg);
                                    state.tokens.write().insert(t.snda_id.clone(), t.clone());
                                    Some(t)
                                }
                                Err(e) => {
                                    state
                                        .status
                                        .set(format!("自动登录失败（{e}），请到登录页重新登录"));
                                    None
                                }
                            }
                        }
                        None => {
                            state
                                .status
                                .set("该账号没有自动登录凭证，请到登录页登录".into());
                            None
                        }
                    }
                }
            };

            if let Some(token) = token {
                let areas = state.areas.read().clone();
                let wine = state.settings.read().clone();
                state.status.set("正在启动游戏…".into());
                match launcher
                    .launch_with_options(
                        &wine,
                        Some(dalamud_this_launch()),
                        &token,
                        area,
                        areas,
                        &exe,
                    )
                    .await
                {
                    Ok(result) => state
                        .status
                        .set(format!("游戏已启动（PID {}）", result.child.id())),
                    Err(e) => state.status.set(format!("启动失败: {e}")),
                }
            }
            launching.set(false);
        });
    };

    let accounts = state.auth_cfg.read().accounts.clone();
    let account_items: Vec<(String, String)> = accounts
        .iter()
        .map(|a| (a.snda_id.clone(), a.display_name().to_string()))
        .collect();
    let area_items: Vec<(String, String)> = state
        .areas
        .read()
        .iter()
        .map(|a| (a.area_id.clone(), a.area_name.clone()))
        .collect();
    let area_placeholder: &'static str = if state.areas.read().is_empty() {
        "大区列表加载中…"
    } else {
        "请选择大区"
    };

    // ── 状态卡片数据 ─────────────────────────────────────────────────────
    let status_game_root = state.game_path.read().clone();
    let game_exe_ok = status_game_root
        .as_ref()
        .map(|r| r.join("game/ffxiv_dx11.exe").is_file())
        .unwrap_or(false);

    let dalamud_cfg = state.dalamud_cfg.read().clone();
    let (dalamud_main, dalamud_sub, dalamud_color) = if !dalamud_cfg.enabled {
        (
            "未启用".to_string(),
            "可在设置页启用（默认关闭，opt-in）".to_string(),
            t.text_secondary,
        )
    } else if dalamud_loading() {
        (
            "检查中…".to_string(),
            "正在获取 release 信息并核对游戏版本".to_string(),
            t.text_secondary,
        )
    } else {
        match dalamud_status.read().as_ref() {
            Some(st) => match &st.install_state {
                InstallState::Ready => (
                    format!(
                        "已安装 {}",
                        st.local_assembly_version.as_deref().unwrap_or("未知版本")
                    ),
                    format!("版本匹配（{}），启动时加载", st.local_game_ver),
                    t.success,
                ),
                InstallState::Missing => (
                    "未安装".to_string(),
                    format!(
                        "启动时自动安装 release {}",
                        st.remote
                            .as_ref()
                            .map(|r| r.assembly_version.as_str())
                            .unwrap_or("?")
                    ),
                    t.warning,
                ),
                InstallState::OutOfDate => (
                    "需要更新".to_string(),
                    format!(
                        "已安装 {}，与本地游戏版本 {} 不匹配",
                        st.local_assembly_version.as_deref().unwrap_or("未知版本"),
                        st.local_game_ver
                    ),
                    t.warning,
                ),
                InstallState::Unsupported => (
                    "暂不兼容".to_string(),
                    format!(
                        "release {} 仅支持 {}，本地游戏 {}",
                        st.remote
                            .as_ref()
                            .map(|r| r.assembly_version.as_str())
                            .unwrap_or("?"),
                        st.remote
                            .as_ref()
                            .map(|r| r.supported_game_ver.as_str())
                            .unwrap_or("?"),
                        st.local_game_ver
                    ),
                    t.danger,
                ),
                InstallState::RuntimeMissing => (
                    "缺少 .NET Runtime".to_string(),
                    "启动时将自动下载 Windows x64 .NET runtime".to_string(),
                    t.warning,
                ),
                InstallState::AssetsMissing => (
                    "缺少 Assets".to_string(),
                    "启动时将自动下载 Dalamud assets".to_string(),
                    t.warning,
                ),
                InstallState::Failed(msg) => ("安装异常".to_string(), msg.clone(), t.danger),
            },
            None => (
                "无法获取状态".to_string(),
                "release 信息不可用，启动时将安全降级为不加载".to_string(),
                t.warning,
            ),
        }
    };

    let wine_cfg = state.settings.read().clone();
    let wine_startup = match wine_cfg.startup_type {
        WineStartupType::Auto => "自动",
        WineStartupType::Managed => "托管",
        WineStartupType::Custom => "自定义",
        WineStartupType::System => "系统",
    };
    // detect 只做本地探测（自定义路径 → 托管目录 → PATH），不会触发下载
    let wine_tool = WineTool::detect(wine_cfg.custom_path.as_deref());
    let wine_sub = match &wine_tool {
        Some(w) if w.is_managed => format!("{}（托管）", w.wine64_path.display()),
        Some(w) => format!("{}", w.wine64_path.display()),
        None => "未检测到可用 wine".to_string(),
    };

    let update_is_busy = update_state.read().is_busy();
    let launch_button_bg = if launching() {
        t.active_bg
    } else {
        t.primary_bg
    };
    let launch_button_fg = if launching() {
        t.text_secondary
    } else {
        t.primary_fg
    };
    let launch_button_cursor = if launching() { "default" } else { "pointer" };

    rsx! {
        div {
            style: "display: flex; flex-direction: column; gap: 16px;",

            // ── 启动设置（紧凑单行）─────────────────────────────────────
            Section { title: "启动设置",
                div {
                    style: "display: flex; flex-direction: row; align-items: center; gap: 12px; flex-wrap: wrap;",
                    span { style: "font-size: 14px; color: {t.text_secondary};", "账号" }
                    div {
                        style: "width: 220px;",
                        Dropdown {
                            items: account_items,
                            selected: state.selected_account,
                            placeholder: "请选择账号",
                        }
                    }
                    span { style: "font-size: 14px; color: {t.text_secondary};", "大区" }
                    div {
                        style: "width: 220px;",
                        Dropdown {
                            items: area_items,
                            selected: state.selected_area,
                            placeholder: area_placeholder,
                        }
                    }
                }
            }

            // ── 状态仪表盘（2×2，窄窗口自动换行）────────────────────────
            div {
                style: "display: flex; flex-direction: row; gap: 12px; flex-wrap: wrap;",
                StatusCard { title: "游戏位置",
                    if let Some(root) = &status_game_root {
                        p { style: "margin: 0; font-size: 13px; color: {t.text}; overflow-wrap: anywhere;", "{root.display()}" }
                        if game_exe_ok {
                            p { style: "margin: 4px 0 0 0; font-size: 12px; color: {t.success};", "✓ 已找到 game/ffxiv_dx11.exe" }
                        } else {
                            p { style: "margin: 4px 0 0 0; font-size: 12px; color: {t.danger};", "✗ 未找到 game/ffxiv_dx11.exe" }
                        }
                    } else {
                        p { style: "margin: 0; font-size: 13px; color: {t.text_secondary};", "未配置，请到设置页选择游戏根目录。" }
                    }
                }
                StatusCard { title: "游戏版本",
                    if let Some(v) = versions.read().as_ref() {
                        p { style: "margin: 0; font-size: 13px; color: {t.text};", "game: {v.1}" }
                        p { style: "margin: 4px 0 0 0; font-size: 12px; color: {t.text_secondary};", "boot: {v.0}" }
                    } else {
                        p { style: "margin: 0; font-size: 13px; color: {t.text_secondary};", "未配置" }
                    }
                }
            }
            div {
                style: "display: flex; flex-direction: row; gap: 12px; flex-wrap: wrap;",
                StatusCard { title: "Dalamud",
                    p { style: "margin: 0; font-size: 13px; color: {dalamud_color};", "{dalamud_main}" }
                    p { style: "margin: 4px 0 0 0; font-size: 12px; color: {t.text_secondary}; overflow-wrap: anywhere;", "{dalamud_sub}" }
                }
                StatusCard { title: "Wine",
                    p { style: "margin: 0; font-size: 13px; color: {t.text};", "启动方式：{wine_startup}" }
                    p { style: "margin: 4px 0 0 0; font-size: 12px; color: {t.text_secondary}; overflow-wrap: anywhere;", "{wine_sub}" }
                }
            }

            // ── 游戏更新 ────────────────────────────────────────────────
            Section { title: "游戏更新",
                div {
                    style: "display: flex; flex-direction: row; gap: 8px; align-items: center;",
                    ActionButton {
                        label: if update_is_busy { "更新任务进行中…".to_string() } else { "检查更新".to_string() },
                        onclick: check_update,
                    }
                    if matches!(&*update_state.read(), UpdateState::NeedsPatch(_)) {
                        ActionButton { label: "更新游戏", onclick: run_update }
                    }
                }

                match &*update_state.read() {
                    UpdateState::Idle => rsx! {},
                    UpdateState::Checking => rsx! { p { style: "color: {t.text_secondary}; font-size: 13px;", "正在检查更新…" } },
                    UpdateState::UpToDate => rsx! { p { style: "color: {t.success}; font-size: 13px;", "游戏已是最新。" } },
                    UpdateState::NeedsPatch(n) => rsx! {
                        p { style: "color: {t.warning}; font-size: 13px;", "发现 {n} 个补丁，点击「更新游戏」开始下载。" }
                    },
                    UpdateState::Downloading(done, total) => rsx! {
                        {
                            let pct = if *total > 0 {
                                (*done as f64 / *total as f64 * 100.0).clamp(0.0, 100.0)
                            } else {
                                0.0
                            };
                            rsx! {
                                div {
                                    style: "margin-top: 12px;",
                                    p { style: "color: {t.text_secondary}; font-size: 13px;", "下载补丁中… {human_bytes(*done)} / {human_bytes(*total)}（{pct:.1}%）" }
                                    div {
                                        style: "height: 4px; background: {t.progress_track}; border-radius: 2px; overflow: hidden;",
                                        div { style: "height: 100%; width: {pct}%; background: {t.primary_bg};" }
                                    }
                                }
                            }
                        }
                    },
                    UpdateState::Installing => rsx! { p { style: "color: {t.text_secondary}; font-size: 13px;", "安装中…" } },
                    UpdateState::Done(msg) => rsx! { p { style: "color: {t.success}; font-size: 13px;", "{msg}" } },
                    UpdateState::Failed(e) => rsx! { ErrorRow { message: "{e}" } },
                }
            }

            // ── 启动游戏 ────────────────────────────────────────────────
            div {
                style: "display: flex; flex-direction: column; gap: 12px;",
                Checkbox { label: "本次启动加载 Dalamud（插件）", checked: dalamud_this_launch }
                button {
                    style: "padding: 16px; border: none; border-radius: 8px; background: {launch_button_bg}; color: {launch_button_fg}; font-size: 18px; font-weight: 600; cursor: {launch_button_cursor};",
                    onclick: launch_game,
                    if launching() { "启动中…" } else { "启动游戏" }
                }
            }
        }
    }
}

/// 当前选中大区的完整 `SdoArea`。
fn selected_area(state: &AppState) -> Option<eorzea_auth::SdoArea> {
    let id = state.selected_area.read().clone()?;
    state.areas.read().iter().find(|a| a.area_id == id).cloned()
}

/// 状态仪表盘小卡片（标题 + 内容，等宽弹性布局）。
#[component]
fn StatusCard(title: &'static str, children: Element) -> Element {
    let t = (use_context::<AppState>().theme)();
    rsx! {
        div {
            style: "flex: 1; min-width: 180px; background: {t.card_bg}; border: 1px solid {t.border}; border-radius: 8px; padding: 12px 16px;",
            p { style: "margin: 0 0 6px 0; font-size: 12px; color: {t.text_secondary};", "{title}" }
            {children}
        }
    }
}

/// 自定义下拉框（blitz 暂不支持原生 `select`，用按钮 + 展开列表实现）。
///
/// 注意：按钮用 `display: block` 撑满容器而不是 `width: 100%`——
/// 后者在 content-box 下会叠加 padding/border 导致横向溢出。
#[component]
fn Dropdown(
    items: Vec<(String, String)>,
    selected: Signal<Option<String>>,
    placeholder: &'static str,
) -> Element {
    let mut open = use_signal(|| false);
    let t = (use_context::<AppState>().theme)();
    let current = selected
        .read()
        .as_ref()
        .and_then(|id| items.iter().find(|(k, _)| k == id))
        .map(|(_, name)| name.clone());
    let current_label = current.unwrap_or_else(|| placeholder.to_string());

    rsx! {
        div {
            style: "position: relative;",
            button {
                style: "display: block; padding: 8px 12px; border: 1px solid {t.input_border}; border-radius: 6px; background: transparent; color: {t.text}; font-size: 14px; text-align: left; cursor: pointer; white-space: nowrap; overflow: hidden; text-overflow: ellipsis;",
                onclick: move |_| open.set(!open()),
                "{current_label} ▾"
            }
            if open() {
                div {
                    style: "position: absolute; left: 0; right: 0; top: 100%; margin-top: 4px; max-height: 240px; overflow-y: auto; background: {t.card_bg}; border: 1px solid {t.border}; border-radius: 6px; z-index: 10; padding: 4px;",
                    if items.is_empty() {
                        div {
                            style: "padding: 8px 12px; color: {t.text_secondary}; font-size: 13px;",
                            "暂无选项"
                        }
                    }
                    for (id, name) in items {
                        {
                            let is_selected = selected.read().as_deref() == Some(id.as_str());
                            let bg = if is_selected { t.active_bg } else { "transparent" };
                            rsx! {
                                button {
                                    key: "{id}",
                                    style: "display: block; padding: 8px 12px; border: none; border-radius: 4px; background: {bg}; color: {t.text}; font-size: 14px; text-align: left; cursor: pointer; white-space: nowrap; overflow: hidden; text-overflow: ellipsis;",
                                    onclick: move |_| {
                                        selected.set(Some(id.clone()));
                                        open.set(false);
                                    },
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

/// 把字节数格式化为可读的 MiB/GiB。
fn human_bytes(bytes: u64) -> String {
    const MIB: f64 = 1024.0 * 1024.0;
    const GIB: f64 = 1024.0 * 1024.0 * 1024.0;
    if bytes as f64 >= GIB {
        format!("{:.2} GiB", bytes as f64 / GIB)
    } else {
        format!("{:.1} MiB", bytes as f64 / MIB)
    }
}
