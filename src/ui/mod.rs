//! dioxus-native（Blitz 渲染）GUI。
//!
//! 页面：登录 / 主页 / 设置。所有耗时操作（登录、等扫码、检查更新、下载）
//! 都在 dioxus `spawn` 的异步任务里执行（dioxus-native 内置 tokio runtime），
//! 不阻塞 UI 线程。
//!
//! 视觉风格：shadcn/ui dark（zinc 色系）——页面底 #09090b，卡片 #0c0c0f +
//! 1px 边框 #27272a，无阴影无渐变；圆角 6~8px；强调色为黑白对比
//! （primary 白底黑字）；主文字 #fafafa，次要 #a1a1aa。

mod home;
mod login;
mod settings;

use std::collections::HashMap;
use std::sync::Arc;

use dioxus::prelude::*;
use xiv_launcher_auth::SdoArea;
use xiv_launcher_rs_lib::auth::{self, AuthConfig};
use xiv_launcher_rs_lib::config::{self, WineSettings};
use xiv_launcher_rs_lib::launcher::{LaunchToken, Launcher};

/// 顶部标签页。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tab {
    Login,
    Home,
    Settings,
}

/// 跨页面共享的应用状态（signal 均为 Copy，可直接放入 context）。
#[derive(Clone, Copy)]
pub struct AppState {
    pub tab: Signal<Tab>,
    /// `auth.toml` 的内存镜像（改动后立即写盘）。
    pub auth_cfg: Signal<AuthConfig>,
    /// `config.toml` 的内存镜像（设置页保存后写盘）。
    pub settings: Signal<WineSettings>,
    /// 大区列表（按 area_order 排序）。
    pub areas: Signal<Vec<SdoArea>>,
    /// 本会话的登录 token（snda_id -> token），不落盘。
    pub tokens: Signal<HashMap<String, LaunchToken>>,
    /// 主页选中的账号（snda_id）。
    pub selected_account: Signal<Option<String>>,
    /// 主页选中大区（area_id）。
    pub selected_area: Signal<Option<String>>,
    /// 底部全局状态栏文本。
    pub status: Signal<String>,
    /// 登录/启动链路（设备指纹采集在启动时完成）。
    pub launcher: Signal<Option<Arc<Launcher>>>,
}

impl AppState {
    /// 登录成功后的公共收尾：写回 auth.toml、缓存 token、跳转到主页。
    pub fn on_login_success(&mut self, token: LaunchToken) {
        let mut cfg = self.auth_cfg.read().clone();
        let make_default = cfg.default_account().is_none();
        cfg.upsert(
            auth::Account {
                snda_id: token.snda_id.clone(),
                username: token.username.clone(),
                auto_login_session_key: token.auto_login_session_key.clone(),
            },
            make_default,
        );
        if let Err(e) = auth::save(&auth::config_path(), &cfg) {
            self.status.set(format!("保存账号配置失败: {e}"));
        }
        self.auth_cfg.set(cfg);
        self.selected_account.set(Some(token.snda_id.clone()));
        self.tokens.write().insert(token.snda_id.clone(), token);
        self.status.set("登录成功".to_string());
        self.tab.set(Tab::Home);
    }
}

/// 根组件：侧边栏导航 + 页面内容 + 状态栏。
pub fn app() -> Element {
    let mut state = AppState {
        tab: use_signal(|| Tab::Home),
        auth_cfg: use_signal(|| auth::load(&auth::config_path())),
        settings: use_signal(config::load_settings),
        areas: use_signal(Vec::new),
        tokens: use_signal(HashMap::new),
        selected_account: use_signal(|| None),
        selected_area: use_signal(|| None),
        status: use_signal(String::new),
        launcher: use_signal(|| None),
    };
    use_context_provider(|| state);

    // 启动时初始化 Launcher（设备指纹）并拉取大区列表。
    use_hook(move || {
        spawn(async move {
            match Launcher::new() {
                Ok(l) => state.launcher.set(Some(Arc::new(l))),
                Err(e) => state.status.set(format!("初始化失败: {e}")),
            }
            match xiv_launcher_auth::sdo::SdoAuth::fetch_server_list().await {
                Ok(mut list) => {
                    list.sort_by_key(|a| a.area_order);
                    // 默认选中第一个大区
                    if let Some(first) = list.first() {
                        state.selected_area.set(Some(first.area_id.clone()));
                    }
                    state.areas.set(list);
                }
                Err(e) => state.status.set(format!("获取大区列表失败: {e}")),
            }
            // 默认选中默认账号
            if let Some(acc) = state.auth_cfg.read().default_account() {
                state.selected_account.set(Some(acc.snda_id.clone()));
            }
        });
    });

    let tab = state.tab;
    rsx! {
        div {
            style: "display: flex; flex-direction: row; height: 100vh; font-family: sans-serif; background: #09090b; color: #fafafa;",

            // 侧边栏导航
            div {
                style: "width: 190px; flex-shrink: 0; display: flex; flex-direction: column; gap: 2px; padding: 16px 12px; border-right: 1px solid #27272a;",
                div {
                    style: "padding: 4px 12px 20px 12px; font-size: 15px; font-weight: 600; color: #fafafa;",
                    "FFXIV 国服启动器"
                }
                NavButton { label: "主页", target: Tab::Home, tab }
                NavButton { label: "登录 / 账号", target: Tab::Login, tab }
                NavButton { label: "设置", target: Tab::Settings, tab }
            }

            // 右栏：页面内容 + 状态栏
            div {
                style: "flex: 1; display: flex; flex-direction: column; min-width: 0;",
                div {
                    style: "flex: 1; overflow-y: auto; padding: 24px 28px;",
                    match tab() {
                        Tab::Login => rsx! { login::LoginPage {} },
                        Tab::Home => rsx! { home::HomePage {} },
                        Tab::Settings => rsx! { settings::SettingsPage {} },
                    }
                }

                // 状态栏
                div {
                    style: "padding: 8px 28px; border-top: 1px solid #27272a; font-size: 12px; color: #a1a1aa;",
                    "{state.status}"
                }
            }
        }
    }
}

#[component]
fn NavButton(label: &'static str, target: Tab, tab: Signal<Tab>) -> Element {
    let active = tab() == target;
    let bg = if active { "#27272a" } else { "transparent" };
    let fg = if active { "#fafafa" } else { "#a1a1aa" };
    rsx! {
        button {
            style: "display: block; width: 100%; padding: 8px 12px; border: none; border-radius: 6px; background: {bg}; color: {fg}; font-size: 14px; text-align: left; cursor: pointer;",
            onclick: move |_| tab.set(target),
            "{label}"
        }
    }
}
