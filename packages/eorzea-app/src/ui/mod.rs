//! dioxus-native（Blitz 渲染）GUI。
//!
//! 页面：登录 / 主页 / 设置。所有耗时操作（登录、等扫码、检查更新、下载）
//! 都在 dioxus `spawn` 的异步任务里执行（dioxus-native 内置 tokio runtime），
//! 不阻塞 UI 线程。
//!
//! 视觉风格：shadcn/ui（zinc 亮色系），语义色见 [`theme::Theme`]，
//! 无阴影无渐变；圆角 6~8px；强调色为黑白对比（primary 按钮底色/文字反转）。

mod home;
mod login;
mod settings;
mod theme;

pub use theme::Theme;

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;

use dioxus::dioxus_core::spawn_forever;
use dioxus::prelude::*;
use eorzea::auth::{self, AuthConfig};
use eorzea::config::{self, WineSettings};
use eorzea::dalamud::model::DalamudSettings;
use eorzea::launcher::{LaunchToken, Launcher};
use eorzea_auth::SdoArea;

/// 侧边栏页面。
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
    /// 启动流程进行中（全局：切换页面后按钮仍保持加载状态）。
    pub launching: Signal<bool>,
    /// 应用内日志（`tracing` INFO 及以上，最新在末尾）。
    pub logs: Signal<Vec<crate::logging::LogEntry>>,
    /// 当前展开的下拉框 id（全局同时只允许一个展开；None = 全部收起）。
    pub open_dropdown: Signal<Option<&'static str>>,
    /// 添加账号弹窗独立于页面滚动区，最后绘制以避免原生层叠遮挡。
    pub adding_account: Signal<bool>,
    /// 登录/启动链路（设备指纹采集在启动时完成）。
    pub launcher: Signal<Option<Arc<Launcher>>>,
    /// 界面语义色。
    pub theme: Signal<Theme>,
}

impl AppState {
    /// 登录成功后写回 auth.toml、缓存 token；留在账号页。
    pub fn on_login_success(&mut self, token: LaunchToken) {
        let mut cfg = self.auth_cfg.read().clone();
        let make_default = cfg.default_account().is_none();
        // 账号里存大区名称（对齐 XIVLauncherCN 的 AreaName），数字 ID 只在内部使用
        let selected_area = self.selected_area.read().clone();
        let area_name = selected_area.and_then(|id| {
            self.areas
                .read()
                .iter()
                .find(|a| a.area_id == id)
                .map(|a| a.area_name.clone())
        });
        cfg.upsert(
            auth::Account {
                snda_id: token.snda_id.clone(),
                username: token.username.clone(),
                auto_login_session_key: token.auto_login_session_key.clone(),
                // 新增账号继承当前选中的大区，作为该账号的默认大区
                area: area_name,
            },
            make_default,
        );
        let saved = auth::save(&auth::config_path(), &cfg);
        self.auth_cfg.set(cfg);
        self.selected_account.set(Some(token.snda_id.clone()));
        self.tokens.write().insert(token.snda_id.clone(), token);
        self.status.set(match saved {
            Ok(()) => "账号已添加".to_string(),
            Err(e) => format!("登录成功，但保存账号配置失败: {e}"),
        });
        self.adding_account.set(false);
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
    // 大区默认值：默认账号的 area → config.toml 旧 area（兼容）→ 大区列表第一个
    let initial_area = initial_auth
        .default_account()
        .and_then(|a| a.area.clone())
        .or_else(|| initial_config.area.clone());
    let mut state = AppState {
        tab: use_signal(|| initial_tab),
        auth_cfg: use_signal(|| initial_auth),
        #[cfg(not(target_os = "windows"))]
        settings: use_signal(|| initial_config.settings.clone()),
        // Windows 的配置没有 Wine 字段；UI 仍保留一份默认值，
        // 供隐藏的 Wine 界面代码与 `launch_game` 参数使用（原生分支会忽略）。
        #[cfg(target_os = "windows")]
        settings: use_signal(WineSettings::default),
        game_path: use_signal(|| initial_config.game_path.clone()),
        dalamud_cfg: use_signal(|| initial_config.dalamud.clone()),
        areas: use_signal(Vec::new),
        tokens: use_signal(HashMap::new),
        selected_account: use_signal(|| None),
        selected_area: use_signal(|| None),
        status: use_signal(String::new),
        launching: use_signal(|| false),
        logs: use_signal(Vec::new),
        open_dropdown: use_signal(|| None),
        adding_account: use_signal(|| false),
        launcher: use_signal(|| None),
        theme: use_signal(Theme::light),
    };
    use_context_provider(|| state);

    // 把 tracing 日志增量搬进 signal，供主页「运行日志」区域展示。
    // 用 root scope 的常驻任务，切换页面不会中断；轮询期间不持有日志缓冲锁。
    #[cfg(not(target_arch = "wasm32"))]
    use_hook(move || {
        spawn_forever(async move {
            let mut seq = 0u64;
            loop {
                let (entries, total) = crate::logging::snapshot_since(seq);
                if total != seq {
                    seq = total;
                    let mut logs = state.logs.write();
                    logs.extend(entries);
                    let overflow = logs.len().saturating_sub(crate::logging::MAX_UI_LINES);
                    if overflow > 0 {
                        logs.drain(0..overflow);
                    }
                }
                tokio::time::sleep(std::time::Duration::from_millis(250)).await;
            }
        });
    });

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
                    // 默认选中账号的默认大区（按名称匹配，兼容旧配置的数字 ID）；
                    // 不在列表中时退回第一个大区
                    if let Some(first) = list
                        .iter()
                        .find(|a| {
                            initial_area
                                .as_deref()
                                .is_some_and(|v| a.area_name == v || a.area_id == v)
                        })
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
    rsx! {
        // blitz 默认 UA 样式表带 `body { margin: 8px }`，窗口白底会从四周透出，
        // 这里注入静态样式重置（blitz 会把 mutation 插入的 <style> 编译为 author 样式表）
        style { "html, body {{ margin: 0; padding: 0; }}" }
        div {
            // 字体栈显式指定中文字体：blitz 缺少逐字符回退，默认拉丁字体没有
            // 全角标点（（）等），紧邻西文时渲染为方块；中文字体同时覆盖
            // 拉丁字形与全角标点，两端一致
            style: "position: relative; display: flex; flex-direction: row; width: 100vw; height: 100vh; font-family: \"Microsoft YaHei\", \"PingFang SC\", \"Noto Sans CJK SC\", \"Source Han Sans SC\", sans-serif; background: {t.page_bg}; color: {t.text};",
            // 点击任意下拉外部区域收起展开的下拉：事件冒泡到根容器即收起
            // （下拉容器内部用 stop_propagation 阻断，不依赖 CSS 层叠，
            // web 与原生 blitz 行为一致）
            onclick: move |_| state.open_dropdown.set(None),

            // 侧边栏导航
            div {
                style: "box-sizing: border-box; width: 72px; flex-shrink: 0; display: flex; flex-direction: column; gap: 8px; padding: 16px 10px; border-right: 1px solid {t.border};",
                NavButton { label: "游戏", target: Tab::Home, tab }
                NavButton { label: "账号管理", target: Tab::Login, tab }

                // 设置固定在底部，与日常游戏/账号操作分组。
                div { style: "flex: 1;" }
                NavButton { label: "设置", target: Tab::Settings, tab }
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
            // 必须在页面及状态栏之后绘制，避免 Blitz 的层叠顺序限制。
            if (state.adding_account)() {
                login::AddAccountDialog {}
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
    // SVG 线条图标不依赖字体字形，避免原生 blitz 的符号缺字问题。
    let icon_path = match target {
        Tab::Home => "m3 10 9-7 9 7v10a1 1 0 0 1-1 1h-5v-7H9v7H4a1 1 0 0 1-1-1Z",
        Tab::Login => {
            "M16 21v-2a4 4 0 0 0-4-4H6a4 4 0 0 0-4 4v2M22 21v-2a4 4 0 0 0-3-3.87M16 3.13a4 4 0 0 1 0 7.75M13 7a4 4 0 1 1-8 0 4 4 0 0 1 8 0Z"
        }
        Tab::Settings => {
            "m9 3-1 3-3 1-2 3 2 2-2 2 2 3 3 1 1 3h6l1-3 3-1 2-3-2-2 2-2-2-3-3-1-1-3ZM15 12a3 3 0 1 1-6 0 3 3 0 0 1 6 0Z"
        }
    };
    rsx! {
        button {
            r#type: "button",
            title: label,
            "aria-label": label,
            "aria-current": if active { "page" } else { "false" },
            style: "display: flex; align-items: center; justify-content: center; height: 48px; flex-shrink: 0; padding: 0; border: none; border-radius: 8px; background: {bg}; color: {fg}; cursor: pointer;",
            // 切页时冒泡到根容器的 onclick 会顺带收起展开的下拉
            onclick: move |_| tab.set(target),
            svg {
                width: "22",
                height: "22",
                view_box: "0 0 24 24",
                fill: "none",
                stroke: fg,
                stroke_width: "1.75",
                stroke_linecap: "round",
                stroke_linejoin: "round",
                "aria-hidden": "true",
                path { d: icon_path }
            }
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
    /// 选中项变化后的回调（用于把选择写回账号等）。
    #[props(default)]
    onchange: Option<EventHandler<String>>,
) -> Element {
    let mut open_dropdown = use_context::<AppState>().open_dropdown;
    let is_open = open_dropdown() == Some(id);
    let t = (use_context::<AppState>().theme)();
    let current = selected
        .read()
        .as_ref()
        .and_then(|id| items.iter().find(|(k, _)| k == id))
        .map(|(_, name)| name.clone());
    let label_color = if current.is_some() {
        t.text
    } else {
        t.text_secondary
    };
    let current_label = current.unwrap_or_else(|| placeholder.to_string());
    // SVG 不依赖字体字形；箭头固定在右侧，不再紧跟选项文字。
    let arrow_path = if is_open {
        "m6 15 6-6 6 6"
    } else {
        "m6 9 6 6 6-6"
    };
    let trigger_border = if is_open {
        t.text_secondary
    } else {
        t.input_border
    };

    rsx! {
        div {
            style: "display: flex; flex-direction: column; min-width: 0;",
            // 阻断冒泡：下拉内部（按钮切换/选项选择）的点击不触发根容器的
            // 「点击外部收起」；点击其它下拉按钮时，其 onclick 直接替换
            // open_dropdown，前一个自动收起（互斥不依赖根容器）
            onclick: move |e: MouseEvent| e.stop_propagation(),
            button {
                r#type: "button",
                title: "{current_label}",
                "aria-label": "{placeholder}：{current_label}",
                "aria-expanded": is_open,
                "aria-controls": "{id}-options",
                style: "box-sizing: border-box; display: flex; align-items: center; gap: 12px; min-height: 40px; padding: 8px 12px; border: 1px solid {trigger_border}; border-radius: 6px; background: {t.card_bg}; color: {label_color}; font-size: 14px; text-align: left; cursor: pointer;",
                onkeydown: move |event: KeyboardEvent| {
                    if event.key() == Key::Escape {
                        open_dropdown.set(None);
                    }
                },
                onclick: move |_| {
                    open_dropdown.set(if is_open { None } else { Some(id) });
                },
                // 避免在原生渲染器中对拉伸的 span 建立裁剪区域。
                // 长标签允许换行，箭头保持独立且不参与收缩。
                div {
                    style: "flex: 1; min-width: 0; line-height: 20px; overflow-wrap: anywhere;",
                    "{current_label}"
                }
                svg {
                    style: "flex-shrink: 0;",
                    width: "16",
                    height: "16",
                    view_box: "0 0 24 24",
                    fill: "none",
                    stroke: t.text_secondary,
                    stroke_width: "1.75",
                    stroke_linecap: "round",
                    stroke_linejoin: "round",
                    "aria-hidden": "true",
                    path { d: arrow_path }
                }
            }
            if is_open {
                div {
                    id: "{id}-options",
                    style: "display: flex; flex-direction: column; gap: 2px; margin-top: 4px; max-height: 240px; overflow-y: auto; background: {t.card_bg}; border: 1px solid {t.border}; border-radius: 6px; padding: 4px;",
                    onkeydown: move |event: KeyboardEvent| {
                        if event.key() == Key::Escape {
                            open_dropdown.set(None);
                        }
                    },
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
                                    r#type: "button",
                                    title: "{name}",
                                    "aria-pressed": is_selected,
                                    style: "box-sizing: border-box; display: flex; align-items: center; gap: 12px; min-height: 36px; flex-shrink: 0; padding: 8px; border: none; border-radius: 4px; background: {bg}; color: {t.text}; font-size: 14px; text-align: left; cursor: pointer;",
                                    onclick: move |_| {
                                        selected.set(Some(id.clone()));
                                        open_dropdown.set(None);
                                        if let Some(onchange) = onchange {
                                            onchange.call(id.clone());
                                        }
                                    },
                                    div {
                                        style: "flex: 1; min-width: 0; line-height: 20px; overflow-wrap: anywhere;",
                                        "{name}"
                                    }
                                    div {
                                        style: "display: flex; align-items: center; width: 16px; height: 16px; flex-shrink: 0;",
                                        if is_selected {
                                            svg {
                                                width: "16",
                                                height: "16",
                                                view_box: "0 0 24 24",
                                                fill: "none",
                                                stroke: t.text,
                                                stroke_width: "1.75",
                                                stroke_linecap: "round",
                                                stroke_linejoin: "round",
                                                "aria-hidden": "true",
                                                path { d: "m5 12 4 4L19 6" }
                                            }
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }
    }
}
