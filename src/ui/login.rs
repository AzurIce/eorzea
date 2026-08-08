//! 登录页：账号列表 + 扫码 / 推送 / 密码三种登录方式。

use base64::Engine;
use dioxus::core::Task;
use dioxus::prelude::*;
use eorzea_lib::auth;

use super::AppState;

/// 扫码登录状态机。
enum QrState {
    Idle,
    Loading,
    /// 等待扫码（内容为二维码 PNG 的 base64）。
    Waiting(String),
    Failed(String),
}

/// 推送登录状态机。
enum PushState {
    Idle,
    Loading,
    /// 等待叨鱼 App 确认（内容为验证序号）。
    Waiting(Option<String>),
    Failed(String),
}

#[component]
pub fn LoginPage() -> Element {
    let mut state = use_context::<AppState>();
    let t = (state.theme)();

    let mut qr_state = use_signal(|| QrState::Idle);
    let mut qr_task = use_signal(|| None::<Task>);
    let mut push_state = use_signal(|| PushState::Idle);
    let mut push_task = use_signal(|| None::<Task>);
    let push_account = use_signal(String::new);
    let pwd_account = use_signal(String::new);
    let pwd_password = use_signal(String::new);
    let mut pwd_busy = use_signal(|| false);
    let mut pwd_error = use_signal(|| None::<String>);

    // ── 扫码登录 ────────────────────────────────────────────────────────
    let start_qr = move |_: MouseEvent| {
        if let Some(t) = qr_task.write().take() {
            t.cancel();
        }
        let Some(launcher) = state.launcher.read().clone() else {
            qr_state.set(QrState::Failed("启动器尚未初始化完成".into()));
            return;
        };
        qr_state.set(QrState::Loading);
        let task = spawn(async move {
            match launcher.request_qr_code().await {
                Ok(session) => {
                    let b64 =
                        base64::engine::general_purpose::STANDARD.encode(session.image_data());
                    qr_state.set(QrState::Waiting(b64));
                    match session.wait_for_scan(None).await {
                        Ok(token) => {
                            qr_state.set(QrState::Idle);
                            state.on_login_success(token);
                        }
                        Err(e) => qr_state.set(QrState::Failed(format!("{e}"))),
                    }
                }
                Err(e) => qr_state.set(QrState::Failed(format!("{e}"))),
            }
        });
        qr_task.set(Some(task));
    };
    let cancel_qr = move |_: MouseEvent| {
        if let Some(t) = qr_task.write().take() {
            t.cancel();
        }
        qr_state.set(QrState::Idle);
    };

    // ── 推送登录 ────────────────────────────────────────────────────────
    let start_push = move |_: MouseEvent| {
        if let Some(t) = push_task.write().take() {
            t.cancel();
        }
        let account = push_account.read().trim().to_string();
        if account.is_empty() {
            push_state.set(PushState::Failed("请输入账号".into()));
            return;
        }
        let Some(launcher) = state.launcher.read().clone() else {
            push_state.set(PushState::Failed("启动器尚未初始化完成".into()));
            return;
        };
        push_state.set(PushState::Loading);
        let task = spawn(async move {
            match launcher.request_push_login(&account).await {
                Ok(session) => {
                    let serial = session.serial_num().map(str::to_string);
                    push_state.set(PushState::Waiting(serial));
                    match session.wait_for_confirm(None).await {
                        Ok(token) => {
                            push_state.set(PushState::Idle);
                            state.on_login_success(token);
                        }
                        Err(e) => push_state.set(PushState::Failed(format!("{e}"))),
                    }
                }
                Err(e) => push_state.set(PushState::Failed(format!("{e}"))),
            }
        });
        push_task.set(Some(task));
    };
    let cancel_push = move |_: MouseEvent| {
        if let Some(t) = push_task.write().take() {
            t.cancel();
        }
        push_state.set(PushState::Idle);
    };

    // ── 密码登录 ────────────────────────────────────────────────────────
    let start_password = move |_: MouseEvent| {
        let account = pwd_account.read().trim().to_string();
        let password = pwd_password.read().clone();
        if account.is_empty() || password.is_empty() {
            pwd_error.set(Some("请输入账号和密码".into()));
            return;
        }
        let Some(launcher) = state.launcher.read().clone() else {
            pwd_error.set(Some("启动器尚未初始化完成".into()));
            return;
        };
        pwd_busy.set(true);
        pwd_error.set(None);
        spawn(async move {
            match launcher.login_password(&account, &password).await {
                Ok(token) => state.on_login_success(token),
                Err(e) => pwd_error.set(Some(format!("{e}"))),
            }
            pwd_busy.set(false);
        });
    };

    let accounts = state.auth_cfg.read().accounts.clone();
    let default_id = state
        .auth_cfg
        .read()
        .default_account()
        .map(|a| a.snda_id.clone());

    rsx! {
        div {
            style: "display: flex; flex-direction: column; gap: 24px;",

            // ── 账号列表 ────────────────────────────────────────────────
            Section { title: "已保存账号",
                if accounts.is_empty() {
                    p { style: "color: {t.text_secondary}; font-size: 13px;", "暂无账号，请通过下方任意方式登录。" }
                }
                for acc in accounts {
                    {
                        let is_default = default_id.as_deref() == Some(acc.snda_id.as_str());
                        let snda_id = acc.snda_id.clone();
                        let snda_id2 = acc.snda_id.clone();
                        let key = acc.auto_login_session_key.clone();
                        let mut display = acc.display_name().to_string();
                        if is_default {
                            display.push_str("（默认）");
                        }
                        if key.is_some() {
                            display.push_str(" · 可自动登录");
                        }
                        rsx! {
                            div {
                                key: "{acc.snda_id}",
                                style: "display: flex; flex-direction: row; align-items: center; gap: 8px; padding: 10px 0; border-bottom: 1px solid {t.border};",
                                span { style: "flex: 1; font-size: 14px; color: {t.text};", "{display}" }
                                if key.is_some() {
                                    SmallButton { label: "自动登录",
                                        onclick: move |_| {
                                            let Some(key) = key.clone() else { return };
                                            let Some(launcher) = state.launcher.read().clone() else {
                                                state.status.set("启动器尚未初始化完成".into());
                                                return;
                                            };
                                            state.status.set("自动登录中…".into());
                                            spawn(async move {
                                                match launcher.login_auto(&key).await {
                                                    Ok(token) => state.on_login_success(token),
                                                    Err(e) => state.status.set(format!("自动登录失败: {e}")),
                                                }
                                            });
                                        }
                                    }
                                }
                                SmallButton { label: "设默认",
                                    onclick: move |_| {
                                        let mut cfg = state.auth_cfg.read().clone();
                                        if let Some(acc) = cfg.find(&snda_id) {
                                            cfg.default_account = Some(acc.display_name().to_string());
                                        }
                                        if let Err(e) = auth::save(&auth::config_path(), &cfg) {
                                            state.status.set(format!("保存账号配置失败: {e}"));
                                        }
                                        state.auth_cfg.set(cfg);
                                    }
                                }
                                DangerButton { label: "删除",
                                    onclick: move |_| {
                                        let mut cfg = state.auth_cfg.read().clone();
                                        cfg.remove(&snda_id2);
                                        if let Err(e) = auth::save(&auth::config_path(), &cfg) {
                                            state.status.set(format!("保存账号配置失败: {e}"));
                                        }
                                        state.auth_cfg.set(cfg);
                                        state.tokens.write().remove(&snda_id2);
                                        if state.selected_account.read().as_deref() == Some(snda_id2.as_str()) {
                                            state.selected_account.set(None);
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
            }

            // ── 扫码登录 ────────────────────────────────────────────────
            Section { title: "扫码登录",
                match &*qr_state.read() {
                    QrState::Idle => rsx! {
                        ActionButton { label: "获取二维码", onclick: start_qr }
                    },
                    QrState::Loading => rsx! {
                        p { style: "color: {t.text_secondary}; font-size: 13px;", "正在获取二维码…" }
                    },
                    QrState::Waiting(b64) => rsx! {
                        div {
                            style: "display: flex; flex-direction: column; align-items: center; gap: 16px; padding: 8px 0;",
                            div {
                                // 二维码卡片两种主题下都保留白色衬底
                                style: "background: #fff; border-radius: 8px; padding: 12px;",
                                img {
                                    src: "data:image/png;base64,{b64}",
                                    style: "width: 220px; height: 220px;",
                                }
                            }
                            p { style: "color: {t.text_secondary}; font-size: 13px;", "请使用叨鱼 App 扫码并确认（5 分钟内有效）" }
                            GhostButton { label: "取消", onclick: cancel_qr }
                        }
                    },
                    QrState::Failed(e) => rsx! {
                        ErrorRow { message: "{e}" }
                        ActionButton { label: "重试", onclick: start_qr }
                    },
                }
            }

            // ── 推送登录 ────────────────────────────────────────────────
            Section { title: "推送登录（叨鱼一键登录）",
                match &*push_state.read() {
                    PushState::Idle | PushState::Failed(_) => rsx! {
                        if let PushState::Failed(e) = &*push_state.read() {
                            ErrorRow { message: "{e}" }
                        }
                        div {
                            style: "display: flex; flex-direction: row; gap: 8px; align-items: center;",
                            TextInput {
                                placeholder: "账号",
                                value: push_account,
                            }
                            ActionButton { label: "发送推送", onclick: start_push }
                        }
                    },
                    PushState::Loading => rsx! {
                        p { style: "color: {t.text_secondary}; font-size: 13px;", "正在发送推送…" }
                    },
                    PushState::Waiting(serial) => rsx! {
                        div {
                            style: "display: flex; flex-direction: column; align-items: center; gap: 12px; padding: 8px 0;",
                            if let Some(serial) = serial {
                                p { style: "font-size: 20px; font-weight: 600; letter-spacing: 4px; color: {t.text};", "验证序号：{serial}" }
                            }
                            p { style: "color: {t.text_secondary}; font-size: 13px;", "请在叨鱼 App 上核对序号并确认（30 秒内有效）" }
                            GhostButton { label: "取消", onclick: cancel_push }
                        }
                    },
                }
            }

            // ── 密码登录 ────────────────────────────────────────────────
            Section { title: "密码登录",
                if let Some(e) = &*pwd_error.read() {
                    ErrorRow { message: "{e}" }
                }
                div {
                    style: "display: flex; flex-direction: column; gap: 12px; max-width: 320px;",
                    TextInput { placeholder: "账号", value: pwd_account }
                    PasswordInput { placeholder: "密码", value: pwd_password }
                    ActionButton {
                        label: if pwd_busy() { "登录中…" } else { "登录" },
                        onclick: start_password,
                    }
                }
            }
        }
    }
}

/// 卡片式分节容器。
#[component]
pub fn Section(title: &'static str, children: Element) -> Element {
    let t = (use_context::<AppState>().theme)();
    rsx! {
        div {
            style: "background: {t.card_bg}; border: 1px solid {t.border}; border-radius: 8px; padding: 20px;",
            h3 { style: "margin: 0 0 12px 0; font-size: 15px; font-weight: 500; color: {t.text};", "{title}" }
            {children}
        }
    }
}

/// 主要操作按钮（primary：底色/文字反转）。
#[component]
pub fn ActionButton(label: String, onclick: EventHandler<MouseEvent>) -> Element {
    let t = (use_context::<AppState>().theme)();
    rsx! {
        button {
            style: "padding: 8px 16px; border: none; border-radius: 6px; background: {t.primary_bg}; color: {t.primary_fg}; font-size: 14px; font-weight: 500; cursor: pointer;",
            onclick: move |e| onclick.call(e),
            "{label}"
        }
    }
}

/// 次要操作按钮（ghost：透明底 + 细边框）。
#[component]
pub fn GhostButton(label: &'static str, onclick: EventHandler<MouseEvent>) -> Element {
    let t = (use_context::<AppState>().theme)();
    rsx! {
        button {
            style: "padding: 8px 16px; border: 1px solid {t.border}; border-radius: 6px; background: transparent; color: {t.text_secondary}; font-size: 14px; cursor: pointer;",
            onclick: move |e| onclick.call(e),
            "{label}"
        }
    }
}

/// 行内小按钮（ghost）。
#[component]
fn SmallButton(label: &'static str, onclick: EventHandler<MouseEvent>) -> Element {
    let t = (use_context::<AppState>().theme)();
    rsx! {
        button {
            style: "padding: 4px 10px; border: 1px solid {t.border}; border-radius: 6px; background: transparent; color: {t.text_secondary}; font-size: 12px; cursor: pointer;",
            onclick: move |e| onclick.call(e),
            "{label}"
        }
    }
}

/// 危险操作小按钮（低饱和红）。
#[component]
fn DangerButton(label: &'static str, onclick: EventHandler<MouseEvent>) -> Element {
    let t = (use_context::<AppState>().theme)();
    rsx! {
        button {
            style: "padding: 4px 10px; border: 1px solid {t.danger_border}; border-radius: 6px; background: transparent; color: {t.danger}; font-size: 12px; cursor: pointer;",
            onclick: move |e| onclick.call(e),
            "{label}"
        }
    }
}

/// 错误提示行。
#[component]
pub fn ErrorRow(message: String) -> Element {
    let t = (use_context::<AppState>().theme)();
    rsx! {
        p { style: "color: {t.danger}; font-size: 13px; margin: 4px 0;", "{message}" }
    }
}

/// 单行文本输入框（受控）。
#[component]
pub fn TextInput(placeholder: &'static str, value: Signal<String>) -> Element {
    let t = (use_context::<AppState>().theme)();
    rsx! {
        input {
            style: "padding: 8px 12px; border: 1px solid {t.input_border}; border-radius: 6px; background: transparent; color: {t.text}; font-size: 14px; flex: 1;",
            placeholder: "{placeholder}",
            value: "{value}",
            oninput: move |e| value.set(e.value()),
        }
    }
}

/// 密码输入框（受控）。
#[component]
pub fn PasswordInput(placeholder: &'static str, value: Signal<String>) -> Element {
    let t = (use_context::<AppState>().theme)();
    rsx! {
        input {
            r#type: "password",
            style: "padding: 8px 12px; border: 1px solid {t.input_border}; border-radius: 6px; background: transparent; color: {t.text}; font-size: 14px;",
            placeholder: "{placeholder}",
            value: "{value}",
            oninput: move |e| value.set(e.value()),
        }
    }
}
