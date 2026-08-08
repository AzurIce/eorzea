//! 主页：账号/大区选择、本地版本、检查/更新游戏、启动游戏。

use dioxus::prelude::*;
use xiv_launcher_auth::PatchListEntry;
use xiv_launcher_rs_lib::game_files::{CheckResult, GameFileManager};

use super::login::{ActionButton, ErrorRow, Section};
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

#[component]
pub fn HomePage() -> Element {
    let mut state = use_context::<AppState>();
    let t = (state.theme)();
    let mut update_state = use_signal(|| UpdateState::Idle);
    let mut patches = use_signal(Vec::<PatchListEntry>::new);
    let mut launching = use_signal(|| false);

    let game_root = state.settings.read().game_path.clone();
    // 本地版本（本地文件读取，随 game_path 变化重算）；只取展示用的 boot/game 版本
    let versions = use_memo(move || {
        state
            .settings
            .read()
            .game_path
            .clone()
            .map(|p| {
                let v = GameFileManager::new().status(&p);
                (v.boot, v.ffxiv)
            })
    });

    // ── 检查更新 ────────────────────────────────────────────────────────
    let check_update = move |_: MouseEvent| {
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
        let Some(root) = state.settings.read().game_path.clone() else {
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
        let Some(snda_id) = state.selected_account.read().clone() else {
            state.status.set("请先选择账号（没有账号请到登录页登录）".into());
            return;
        };
        let Some(area) = selected_area(&state) else {
            state.status.set("请选择大区".into());
            return;
        };
        let Some(root) = state.settings.read().game_path.clone() else {
            state.status.set("请先在设置页配置游戏目录".into());
            return;
        };
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
                                        xiv_launcher_rs_lib::auth::Account {
                                            snda_id: t.snda_id.clone(),
                                            username: t.username.clone(),
                                            auto_login_session_key: t.auto_login_session_key.clone(),
                                        },
                                        false,
                                    );
                                    let _ = xiv_launcher_rs_lib::auth::save(
                                        &xiv_launcher_rs_lib::auth::config_path(),
                                        &cfg,
                                    );
                                    state.auth_cfg.set(cfg);
                                    state.tokens.write().insert(t.snda_id.clone(), t.clone());
                                    Some(t)
                                }
                                Err(e) => {
                                    state.status.set(format!(
                                        "自动登录失败（{e}），请到登录页重新登录"
                                    ));
                                    None
                                }
                            }
                        }
                        None => {
                            state.status.set("该账号没有自动登录凭证，请到登录页登录".into());
                            None
                        }
                    }
                }
            };

            if let Some(token) = token {
                let areas = state.areas.read().clone();
                let wine = state.settings.read().clone();
                let exe = root.join("game/ffxiv_dx11.exe");
                state.status.set("正在启动游戏…".into());
                match launcher.launch_with_wine(&wine, &token, area, areas, &exe).await {
                    Ok(result) => {
                        state.status.set(format!("游戏已启动（PID {}）", result.child.id()))
                    }
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

    rsx! {
        div {
            style: "display: flex; flex-direction: column; gap: 24px;",

            Section { title: "启动设置",
                div {
                    style: "display: flex; flex-direction: column; gap: 12px;",
                    LabeledRow { label: "账号",
                        Dropdown {
                            items: account_items,
                            selected: state.selected_account,
                            placeholder: "请选择账号",
                        }
                    }
                    LabeledRow { label: "大区",
                        Dropdown {
                            items: area_items,
                            selected: state.selected_area,
                            placeholder: area_placeholder,
                        }
                    }
                }
            }

            Section { title: "游戏版本",
                if let Some(v) = versions.read().as_ref() {
                    p { style: "margin: 2px 0; font-size: 14px; color: {t.text};", "boot: {v.0}" }
                    p { style: "margin: 2px 0; font-size: 14px; color: {t.text};", "game: {v.1}" }
                } else {
                    p { style: "color: {t.text_secondary}; font-size: 13px;", "未配置游戏目录，请到设置页填写游戏根目录。" }
                }

                div {
                    style: "display: flex; flex-direction: row; gap: 8px; margin-top: 12px; align-items: center;",
                    ActionButton { label: "检查更新", onclick: check_update }
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
                            let pct = if *total > 0 { *done as f64 / *total as f64 * 100.0 } else { 0.0 };
                            rsx! {
                                div {
                                    style: "margin-top: 12px;",
                                    p { style: "color: {t.text_secondary}; font-size: 13px;", "下载补丁中… {done} / {total} 字节（{pct:.1}%）" }
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
            button {
                style: "padding: 16px; border: none; border-radius: 8px; background: {t.primary_bg}; color: {t.primary_fg}; font-size: 18px; font-weight: 600; cursor: pointer;",
                onclick: launch_game,
                if launching() { "启动中…" } else { "启动游戏" }
            }
        }
    }
}

/// 当前选中大区的完整 `SdoArea`。
fn selected_area(state: &AppState) -> Option<xiv_launcher_auth::SdoArea> {
    let id = state.selected_area.read().clone()?;
    state.areas.read().iter().find(|a| a.area_id == id).cloned()
}

/// 带标签的设置行。
#[component]
fn LabeledRow(label: &'static str, children: Element) -> Element {
    let t = (use_context::<AppState>().theme)();
    rsx! {
        div {
            style: "display: flex; flex-direction: row; align-items: center; gap: 12px;",
            span { style: "width: 48px; font-size: 14px; color: {t.text_secondary};", "{label}" }
            div { style: "flex: 1;", {children} }
        }
    }
}

/// 自定义下拉框（blitz 暂不支持原生 `select`，用按钮 + 展开列表实现）。
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
                style: "width: 100%; padding: 8px 12px; border: 1px solid {t.input_border}; border-radius: 6px; background: transparent; color: {t.text}; font-size: 14px; text-align: left; cursor: pointer;",
                onclick: move |_| open.set(!open()),
                "{current_label} ▾"
            }
            if open() {
                div {
                    style: "position: absolute; left: 0; right: 0; top: 100%; margin-top: 4px; max-height: 240px; overflow-y: auto; background: {t.card_bg}; border: 1px solid {t.border}; border-radius: 6px; z-index: 10; padding: 4px;",
                    for (id, name) in items {
                        button {
                            key: "{id}",
                            style: "display: block; width: 100%; padding: 8px 12px; border: none; border-radius: 4px; background: transparent; color: {t.text}; font-size: 14px; text-align: left; cursor: pointer;",
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
