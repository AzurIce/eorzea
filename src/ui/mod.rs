//! dioxus-native（Blitz 渲染）GUI。
//!
//! 页面：登录 / 主页 / 设置。所有耗时操作（登录、等扫码、检查更新、下载）
//! 都在 dioxus `spawn` 的异步任务里执行（dioxus-native 内置 tokio runtime），
//! 不阻塞 UI 线程。
//!
//! 视觉风格：shadcn/ui（zinc 色系）亮/暗双主题，语义色见 [`theme::Theme`]，
//! 无阴影无渐变；圆角 6~8px；强调色为黑白对比（primary 按钮底色/文字反转）。

mod home;
mod login;
mod settings;
mod theme;

pub use theme::Theme;

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;

use crate::auth::{self, AuthConfig};
use crate::config::{self, WineSettings};
use crate::dalamud::model::DalamudSettings;
use crate::launcher::{LaunchToken, Launcher};
use dioxus::prelude::*;
use eorzea_auth::SdoArea;

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
    /// 游戏根目录（config.toml 顶层 `game_path`）。
    pub game_path: Signal<Option<PathBuf>>,
    /// `[dalamud]` section 的内存镜像（设置页保存后写盘）。
    pub dalamud_cfg: Signal<DalamudSettings>,
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
    /// 当前展开的下拉框 id（全局同时只允许一个展开；None = 全部收起）。
    pub open_dropdown: Signal<Option<&'static str>>,
    /// 登录/启动链路（设备指纹采集在启动时完成）。
    pub launcher: Signal<Option<Arc<Launcher>>>,
    /// 当前主题（亮/暗，仅内存切换，不持久化）。
    pub theme: Signal<Theme>,
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
    let initial_config = config::load_app_default();
    let initial_auth = auth::load(&auth::config_path());
    // 首次启动直接落到登录页；已有账号和游戏目录时再进入主页。
    let initial_tab = if initial_auth.accounts.is_empty() || initial_config.game_path.is_none() {
        Tab::Login
    } else {
        Tab::Home
    };
    let mut state = AppState {
        tab: use_signal(|| initial_tab),
        auth_cfg: use_signal(|| initial_auth),
        settings: use_signal(|| initial_config.settings.clone()),
        game_path: use_signal(|| initial_config.game_path.clone()),
        dalamud_cfg: use_signal(|| initial_config.dalamud.clone()),
        areas: use_signal(Vec::new),
        tokens: use_signal(HashMap::new),
        selected_account: use_signal(|| None),
        selected_area: use_signal(|| None),
        status: use_signal(String::new),
        open_dropdown: use_signal(|| None),
        launcher: use_signal(|| None),
        theme: use_signal(Theme::light),
    };
    use_context_provider(|| state);

    let default_area = initial_config.area.clone();
    // 启动时初始化 Launcher（设备指纹）并拉取大区列表。
    use_hook(move || {
        spawn(async move {
            match Launcher::new() {
                Ok(l) => state.launcher.set(Some(Arc::new(l))),
                Err(e) => state.status.set(format!("初始化失败: {e}")),
            }
            match eorzea_auth::sdo::SdoAuth::fetch_server_list().await {
                Ok(mut list) => {
                    list.sort_by_key(|a| a.area_order);
                    // 默认选中：config.toml 的默认大区（设置页可配），
                    // 不在列表中时退回第一个大区
                    if let Some(first) = list
                        .iter()
                        .find(|a| Some(&a.area_id) == default_area.as_ref())
                        .or_else(|| list.first())
                    {
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
    let t = (state.theme)();
    // 主题切换不用 ☀/☾：原生 blitz 默认字体缺字渲染为方块
    let toggle_label = if t.dark {
        "亮色主题"
    } else {
        "暗色主题"
    };
    rsx! {
        // blitz 默认 UA 样式表带 `body { margin: 8px }`，窗口白底会从四周透出，
        // 这里注入静态样式重置（blitz 会把 mutation 插入的 <style> 编译为 author 样式表）
        style { "html, body {{ margin: 0; padding: 0; }}" }
        div {
            // 字体栈显式指定中文字体：blitz 缺少逐字符回退，默认拉丁字体没有
            // 全角标点（（）等），紧邻西文时渲染为方块；中文字体同时覆盖
            // 拉丁字形与全角标点，两端一致
            style: "display: flex; flex-direction: row; width: 100vw; height: 100vh; font-family: \"Microsoft YaHei\", \"PingFang SC\", \"Noto Sans CJK SC\", \"Source Han Sans SC\", sans-serif; background: {t.page_bg}; color: {t.text};",
            // 点击任意下拉外部区域收起展开的下拉：事件冒泡到根容器即收起
            // （下拉容器内部用 stop_propagation 阻断，不依赖 CSS 层叠，
            // web 与原生 blitz 行为一致）
            onclick: move |_| state.open_dropdown.set(None),

            // 侧边栏导航
            div {
                style: "width: 190px; flex-shrink: 0; display: flex; flex-direction: column; gap: 2px; padding: 16px 12px; border-right: 1px solid {t.border};",
                div {
                    style: "padding: 4px 12px 20px 12px; font-size: 15px; font-weight: 600; color: {t.text};",
                    "FFXIV 国服启动器"
                }
                NavButton { label: "主页", target: Tab::Home, tab }
                NavButton { label: "登录 / 账号", target: Tab::Login, tab }
                NavButton { label: "设置", target: Tab::Settings, tab }

                // 亮/暗主题切换（底部）
                div { style: "flex: 1;" }
                button {
                    style: "padding: 8px 12px; border: 1px solid {t.border}; border-radius: 6px; background: transparent; color: {t.text_secondary}; font-size: 13px; text-align: left; cursor: pointer;",
                    onclick: move |_| {
                        let mut theme = state.theme;
                        theme.set(if t.dark { Theme::light() } else { Theme::dark() });
                    },
                    "{toggle_label}"
                }
            }

            // 右栏：页面内容 + 状态栏。
            // 滚动由各页面自持（设置页需要把保存栏固定在滚动区外）
            div {
                style: "flex: 1; min-height: 0; display: flex; flex-direction: column; min-width: 0;",
                match tab() {
                    Tab::Login => rsx! { login::LoginPage {} },
                    Tab::Home => rsx! { home::HomePage {} },
                    Tab::Settings => rsx! { settings::SettingsPage {} },
                }

                // 状态栏。overflow-wrap: anywhere 让无空格长串（URL/路径）
                // 也能断行，避免超长错误信息单行溢出窗口
                div {
                    style: "padding: 8px 28px; border-top: 1px solid {t.border}; font-size: 12px; line-height: 1.5; color: {t.text_secondary}; overflow-wrap: anywhere;",
                    "{state.status}"
                }
            }
        }
    }
}

#[component]
fn NavButton(label: &'static str, target: Tab, tab: Signal<Tab>) -> Element {
    let t = (use_context::<AppState>().theme)();
    let active = tab() == target;
    let bg = if active { t.active_bg } else { "transparent" };
    let fg = if active { t.text } else { t.text_secondary };
    rsx! {
        button {
            // display: block 撑满侧边栏；不要用 width: 100%（content-box 下会叠加 padding 溢出）
            style: "display: block; padding: 8px 12px; border: none; border-radius: 6px; background: {bg}; color: {fg}; font-size: 14px; text-align: left; cursor: pointer;",
            // 切页时冒泡到根容器的 onclick 会顺带收起展开的下拉
            onclick: move |_| tab.set(target),
            "{label}"
        }
    }
}

/// 自定义下拉框（blitz 暂不支持原生 `select`，用按钮 + 展开列表实现）。
///
/// 注意：
/// - 按钮用 `display: block` 撑满容器而不是 `width: 100%`——
///   后者在 content-box 下会叠加 padding/border 导致横向溢出。
/// - 展开列表走文档流内联展开（不用 `position: absolute + z-index`）：
///   原生 blitz 渲染器对层叠上下文支持不完整，绝对定位的弹层会被
///   文档序靠后的卡片遮挡；内联展开把下方内容顶开，两端渲染一致。
/// - 展开状态收在全局 `AppState.open_dropdown`（`id` 区分实例）：
///   同时只允许一个展开，且点击外部（事件冒泡到根容器）收起。
#[component]
pub fn Dropdown(
    id: &'static str,
    items: Vec<(String, String)>,
    selected: Signal<Option<String>>,
    placeholder: &'static str,
) -> Element {
    let mut open_dropdown = use_context::<AppState>().open_dropdown;
    let is_open = open_dropdown() == Some(id);
    let t = (use_context::<AppState>().theme)();
    let current = selected
        .read()
        .as_ref()
        .and_then(|id| items.iter().find(|(k, _)| k == id))
        .map(|(_, name)| name.clone());
    let current_label = current.unwrap_or_else(|| placeholder.to_string());
    // 箭头用 ASCII v/^：▾ 在原生 blitz 默认字体下缺字渲染为方块
    let arrow = if is_open { "^" } else { "v" };

    rsx! {
        div {
            style: "display: flex; flex-direction: column;",
            // 阻断冒泡：下拉内部（按钮切换/选项选择）的点击不触发根容器的
            // 「点击外部收起」；点击其它下拉按钮时，其 onclick 直接替换
            // open_dropdown，前一个自动收起（互斥不依赖根容器）
            onclick: move |e: MouseEvent| e.stop_propagation(),
            button {
                style: "display: block; padding: 8px 12px; border: 1px solid {t.input_border}; border-radius: 6px; background: transparent; color: {t.text}; font-size: 14px; text-align: left; cursor: pointer; white-space: nowrap; overflow: hidden; text-overflow: ellipsis;",
                onclick: move |_| {
                    open_dropdown.set(if is_open { None } else { Some(id) });
                },
                "{current_label} {arrow}"
            }
            if is_open {
                div {
                    style: "margin-top: 4px; max-height: 240px; overflow-y: auto; background: {t.card_bg}; border: 1px solid {t.border}; border-radius: 6px; padding: 4px;",
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
                                        open_dropdown.set(None);
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
