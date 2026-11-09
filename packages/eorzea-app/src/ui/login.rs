//! 账号卡片与添加账号弹窗（扫码 / 推送 / 密码）。

use base64::Engine;
use dioxus::core::Task;
use dioxus::prelude::*;
use eorzea::auth;

use super::AppState;

enum QrState {
    Idle,
    Loading,
    Waiting(String),
    Failed(String),
}

enum PushState {
    Idle,
    Loading,
    Waiting(Option<String>),
    Failed(String),
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum LoginMethod {
    Qr,
    Push,
    Password,
}

fn account_status(has_token: bool, has_key: bool) -> &'static str {
    match (has_token, has_key) {
        (true, _) => "本次会话已登录",
        (false, true) => "已保存自动登录凭据，启动时验证",
        (false, false) => "未保存自动登录凭据，请重新添加账号",
    }
}

#[component]
pub fn LoginPage() -> Element {
    let mut state = use_context::<AppState>();
    let t = (state.theme)();
    let mut confirm_delete = use_signal(|| None::<String>);
    let accounts = state.auth_cfg.read().accounts.clone();
    let default_id = state
        .auth_cfg
        .read()
        .default_account()
        .map(|a| a.snda_id.clone());

    rsx! {
        div {
            style: "flex: 1; min-height: 0; overflow-y: auto; padding: 24px 28px;",
            div {
                style: "display: flex; align-items: center; justify-content: space-between; gap: 12px; flex-wrap: wrap; margin-bottom: 8px;",
                h2 { style: "margin: 0; font-size: 20px; font-weight: 600; color: {t.text};", "账号管理" }
                ActionButton {
                    label: "添加账号",
                    onclick: move |_| {
                        confirm_delete.set(None);
                        state.open_dropdown.set(None);
                        state.adding_account.set(true);
                    },
                }
            }
            p {
                style: "margin: 0 0 24px; color: {t.text_secondary}; font-size: 13px; line-height: 1.6;",
                "管理本机保存的账号。启动游戏时会使用已有凭据自动登录。"
            }
            div {
                style: "display: flex; flex-wrap: wrap; gap: 16px;",
                if accounts.is_empty() {
                    div {
                        style: "flex: 1; padding: 32px 20px; border: 1px solid {t.border}; border-radius: 8px; background: {t.card_bg}; color: {t.text_secondary}; font-size: 14px; text-align: center;",
                        "还没有保存的账号，点击「添加账号」开始登录。"
                    }
                }
                for acc in accounts {
                    {
                        let is_default = default_id.as_deref() == Some(acc.snda_id.as_str());
                        let set_default_id = acc.snda_id.clone();
                        let delete_id = acc.snda_id.clone();
                        let confirm_id = acc.snda_id.clone();
                        let confirming = confirm_delete.read().as_deref() == Some(acc.snda_id.as_str());
                        let status = account_status(state.tokens.read().contains_key(&acc.snda_id), acc.can_auto_login());
                        // 旧配置可能存的是数字 ID；展示时尽量换成名称
                        let area_label = acc.area.as_ref().map(|value| {
                            state
                                .areas
                                .read()
                                .iter()
                                .find(|a| a.area_name == *value || a.area_id == *value)
                                .map(|a| a.area_name.clone())
                                .unwrap_or_else(|| value.clone())
                        });
                        rsx! {
                            div {
                                key: "{acc.snda_id}",
                                style: "box-sizing: border-box; flex: 1 1 240px; min-width: 0; padding: 20px; border: 1px solid {t.border}; border-radius: 8px; background: {t.card_bg};",
                                div { style: "font-size: 16px; font-weight: 600; color: {t.text}; overflow-wrap: anywhere;", "{acc.display_name()}" }
                                if acc.username.is_some() {
                                    div { style: "margin-top: 6px; color: {t.text_secondary}; font-size: 12px; overflow-wrap: anywhere;", "SDO ID：{acc.snda_id}" }
                                }
                                if let Some(area) = &area_label {
                                    div { style: "margin-top: 8px; color: {t.text_secondary}; font-size: 12px;", "默认大区：{area}" }
                                }
                                div { style: "margin-top: 12px; color: {t.text_secondary}; font-size: 12px; line-height: 1.6;", "{status}" }
                                div {
                                    style: "display: flex; flex-wrap: wrap; gap: 8px; margin-top: 20px; align-items: center;",
                                    SmallButton {
                                        label: if is_default { "默认" } else { "设为默认" },
                                        disabled: is_default,
                                        onclick: move |_| {
                                            let mut cfg = state.auth_cfg.read().clone();
                                            if let Some(acc) = cfg.find(&set_default_id) {
                                                cfg.default_account = Some(acc.display_name().to_string());
                                            }
                                            match auth::save(&auth::config_path(), &cfg) {
                                                Ok(()) => {
                                                    state.auth_cfg.set(cfg);
                                                    state.status.set("默认账号已更新".into());
                                                }
                                                Err(e) => state.status.set(format!("保存账号配置失败: {e}")),
                                            }
                                        },
                                    }
                                    if confirming {
                                        DangerButton {
                                            label: "确认删除",
                                            onclick: move |_| {
                                                let mut cfg = state.auth_cfg.read().clone();
                                                cfg.remove(&delete_id);
                                                if let Err(e) = auth::save(&auth::config_path(), &cfg) {
                                                    state.status.set(format!("保存账号配置失败: {e}"));
                                                    return;
                                                }
                                                state.auth_cfg.set(cfg);
                                                state.tokens.write().remove(&delete_id);
                                                if state.selected_account.read().as_deref() == Some(delete_id.as_str()) {
                                                    state.selected_account.set(None);
                                                }
                                                confirm_delete.set(None);
                                                state.status.set("账号已从本机移除".into());
                                            },
                                        }
                                        SmallButton { label: "取消", onclick: move |_| confirm_delete.set(None) }
                                    } else {
                                        DangerButton { label: "删除", onclick: move |_| confirm_delete.set(Some(confirm_id.clone())) }
                                    }
                                }
                                if confirming {
                                    div { style: "margin-top: 12px; font-size: 12px; color: {t.text_secondary};", "只移除本机记录，不会注销盛趣账号。" }
                                }
                            }
                        }
                    }
                }
            }
        }
    }
}

fn cancel_task(mut task: Signal<Option<Task>>) {
    if let Some(task) = task.write().take() {
        task.cancel();
    }
}

fn start_qr(mut state: AppState, mut qr_state: Signal<QrState>, mut qr_task: Signal<Option<Task>>) {
    cancel_task(qr_task);
    let Some(launcher) = state.launcher.read().clone() else {
        qr_state.set(QrState::Failed("启动器尚未初始化完成".into()));
        return;
    };
    qr_state.set(QrState::Loading);
    let task = spawn(async move {
        match launcher.request_qr_code().await {
            Ok(session) => {
                let b64 = base64::engine::general_purpose::STANDARD.encode(session.image_data());
                qr_state.set(QrState::Waiting(b64));
                match session.wait_for_scan(None).await {
                    Ok(token) => state.on_login_success(token),
                    Err(e) => qr_state.set(QrState::Failed(format!("{e}"))),
                }
            }
            Err(e) => qr_state.set(QrState::Failed(format!("{e}"))),
        }
    });
    qr_task.set(Some(task));
}

/// 最后挂载到应用根容器，不受账号页面滚动区裁剪。
#[component]
pub fn AddAccountDialog() -> Element {
    let mut state = use_context::<AppState>();
    let t = (state.theme)();
    let mut method = use_signal(|| LoginMethod::Qr);
    let mut qr_state = use_signal(|| QrState::Idle);
    let qr_task = use_signal(|| None::<Task>);
    let mut push_state = use_signal(|| PushState::Idle);
    let mut push_task = use_signal(|| None::<Task>);
    let push_account = use_signal(String::new);
    let pwd_account = use_signal(String::new);
    let mut pwd_password = use_signal(String::new);
    let mut pwd_task = use_signal(|| None::<Task>);
    let mut pwd_busy = use_signal(|| false);
    let mut pwd_error = use_signal(|| None::<String>);

    // 打开扫码 tab 自动请求二维码；初始化完成后也会自动重试准备。
    use_effect(move || {
        let ready = state.launcher.read().is_some();
        if method() == LoginMethod::Qr && ready {
            start_qr(state, qr_state, qr_task);
        }
    });
    use_drop(move || {
        cancel_task(qr_task);
        cancel_task(push_task);
        cancel_task(pwd_task);
    });

    let mut close_dialog = move || {
        cancel_task(qr_task);
        cancel_task(push_task);
        cancel_task(pwd_task);
        pwd_password.set(String::new());
        state.adding_account.set(false);
    };
    let mut switch_method = move |next: LoginMethod| {
        if method() == next {
            return;
        }
        cancel_task(qr_task);
        cancel_task(push_task);
        cancel_task(pwd_task);
        qr_state.set(QrState::Idle);
        push_state.set(PushState::Idle);
        pwd_busy.set(false);
        pwd_password.set(String::new());
        pwd_error.set(None);
        method.set(next);
    };
    let start_push = move |_: MouseEvent| {
        let account = push_account.read().trim().to_string();
        if account.is_empty() {
            push_state.set(PushState::Failed("请输入账号".into()));
            return;
        }
        let Some(launcher) = state.launcher.read().clone() else {
            push_state.set(PushState::Failed("启动器尚未初始化完成".into()));
            return;
        };
        cancel_task(push_task);
        push_state.set(PushState::Loading);
        let task = spawn(async move {
            match launcher.request_push_login(&account).await {
                Ok(session) => {
                    push_state.set(PushState::Waiting(session.serial_num().map(str::to_string)));
                    match session.wait_for_confirm(None).await {
                        Ok(token) => state.on_login_success(token),
                        Err(e) => push_state.set(PushState::Failed(format!("{e}"))),
                    }
                }
                Err(e) => push_state.set(PushState::Failed(format!("{e}"))),
            }
        });
        push_task.set(Some(task));
    };
    let start_password = move |_: MouseEvent| {
        if pwd_busy() {
            return;
        }
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
        let task = spawn(async move {
            match launcher.login_password(&account, &password).await {
                Ok(token) => {
                    pwd_password.set(String::new());
                    state.on_login_success(token);
                }
                Err(e) => pwd_error.set(Some(format!("{e}"))),
            }
            pwd_busy.set(false);
        });
        pwd_task.set(Some(task));
    };

    rsx! {
        div {
            style: "position: absolute; inset: 0; z-index: 10; display: flex; align-items: center; justify-content: center; padding: 16px; background: rgba(0, 0, 0, 0.35);",
            onclick: move |event: MouseEvent| event.stop_propagation(),
            onkeydown: move |event: KeyboardEvent| {
                if event.key() == Key::Escape {
                    close_dialog();
                    event.stop_propagation();
                }
            },
            div {
                role: "dialog",
                tabindex: "-1",
                onmounted: move |event| async move {
                    // 先把键盘焦点移入弹窗，确保 Escape 可以立即关闭。
                    let _ = event.set_focus(true).await;
                },
                "aria-modal": "true",
                "aria-labelledby": "add-account-title",
                style: "box-sizing: border-box; display: flex; flex-direction: column; gap: 20px; width: 440px; max-width: 100%; max-height: 100%; overflow-y: auto; padding: 24px; border: 1px solid {t.border}; border-radius: 8px; background: {t.card_bg}; color: {t.text};",
                div {
                    style: "display: flex; align-items: center; justify-content: space-between; gap: 12px;",
                    h2 { id: "add-account-title", style: "margin: 0; font-size: 18px; font-weight: 600;", "添加账号" }
                    GhostButton { label: "关闭", onclick: move |_| close_dialog() }
                }
                div {
                    role: "group",
                    "aria-label": "登录方式",
                    style: "display: flex; gap: 4px; padding: 4px; border-radius: 6px; background: {t.active_bg};",
                    for (value, label) in [(LoginMethod::Qr, "叨鱼扫码登录"), (LoginMethod::Push, "叨鱼推送登录"), (LoginMethod::Password, "密码登录")] {
                        {
                            let active = method() == value;
                            let bg = if active { t.card_bg } else { "transparent" };
                            let fg = if active { t.text } else { t.text_secondary };
                            rsx! {
                                button {
                                    r#type: "button",
                                    "aria-pressed": active,
                                    style: "display: flex; align-items: center; justify-content: center; flex: 1 1 0; min-width: 0; padding: 8px 4px; border: none; border-radius: 4px; background: {bg}; color: {fg}; font-size: 13px; text-align: center; cursor: pointer;",
                                    onclick: move |_| switch_method(value),
                                    "{label}"
                                }
                            }
                        }
                    }
                }
                match method() {
                    LoginMethod::Qr => rsx! {
                        div {
                            style: "display: flex; flex-direction: column; align-items: center; gap: 16px;",
                            match &*qr_state.read() {
                                QrState::Idle => rsx! { p { style: "color: {t.text_secondary}; font-size: 13px;", "正在初始化登录服务…" } },
                                QrState::Loading => rsx! { p { style: "color: {t.text_secondary}; font-size: 13px;", "正在获取二维码…" } },
                                QrState::Waiting(b64) => rsx! {
                                    div {
                                        style: "background: #fff; padding: 12px; border-radius: 6px;",
                                        img { src: "data:image/png;base64,{b64}", alt: "叨鱼登录二维码", style: "display: block; width: 220px; height: 220px;" }
                                    }
                                    div { style: "color: {t.text_secondary}; font-size: 13px; line-height: 1.6; text-align: center;", "使用叨鱼 App 扫码并确认，5 分钟内有效。" }
                                },
                                QrState::Failed(e) => rsx! { ErrorRow { message: "{e}" } },
                            }
                            GhostButton {
                                label: "刷新二维码",
                                disabled: matches!(&*qr_state.read(), QrState::Loading) || state.launcher.read().is_none(),
                                onclick: move |_| start_qr(state, qr_state, qr_task),
                            }
                        }
                    },
                    LoginMethod::Push => rsx! {
                        div {
                            style: "display: flex; flex-direction: column; gap: 12px;",
                            match &*push_state.read() {
                                PushState::Idle | PushState::Failed(_) => rsx! {
                                    if let PushState::Failed(e) = &*push_state.read() { ErrorRow { message: "{e}" } }
                                    label {
                                        style: "display: flex; flex-direction: column; gap: 8px; color: {t.text}; font-size: 13px;",
                                        "账号"
                                        TextInput { value: push_account }
                                    }
                                    ActionButton { label: "发送推送", onclick: start_push }
                                },
                                PushState::Loading => rsx! { p { style: "color: {t.text_secondary}; font-size: 13px;", "正在发送推送…" } },
                                PushState::Waiting(serial) => rsx! {
                                    if let Some(serial) = serial {
                                        div { style: "font-size: 20px; font-weight: 600; color: {t.text};", "验证序号：{serial}" }
                                    }
                                    p { style: "color: {t.text_secondary}; font-size: 13px; line-height: 1.6;", "请在叨鱼 App 中核对序号并确认，30 秒内有效。" }
                                    GhostButton {
                                        label: "取消推送",
                                        onclick: move |_| {
                                            cancel_task(push_task);
                                            push_state.set(PushState::Idle);
                                        },
                                    }
                                },
                            }
                        }
                    },
                    LoginMethod::Password => rsx! {
                        div {
                            style: "display: flex; flex-direction: column; gap: 16px;",
                            if let Some(e) = &*pwd_error.read() { ErrorRow { message: "{e}" } }
                            label {
                                style: "display: flex; flex-direction: column; gap: 8px; color: {t.text}; font-size: 13px;",
                                "账号"
                                TextInput { value: pwd_account }
                            }
                            label {
                                style: "display: flex; flex-direction: column; gap: 8px; color: {t.text}; font-size: 13px;",
                                "密码"
                                PasswordInput { value: pwd_password }
                            }
                            ActionButton {
                                label: if pwd_busy() { "登录中…" } else { "登录并添加" },
                                disabled: pwd_busy(),
                                onclick: start_password,
                            }
                            div { style: "color: {t.text_secondary}; font-size: 12px;", "密码只用于本次登录，不会保存到本机。" }
                        }
                    },
                }
            }
        }
    }
}

#[component]
pub fn Section(
    title: &'static str,
    /// 标题右侧的平台/范围提示（如「Linux / macOS」），None 不显示。
    #[props(default)]
    badge: Option<&'static str>,
    children: Element,
) -> Element {
    let t = (use_context::<AppState>().theme)();
    rsx! {
        div {
            style: "background: {t.card_bg}; border: 1px solid {t.border}; border-radius: 8px; padding: 20px;",
            // 不用 flex 行放标题+标签：blitz 对 flex 内文本项的主轴尺寸测量不可靠，
            // 即使窗口很宽也可能按很小的宽度换行，而且容器高度仍按单行计算，
            // 第二行会压到下面的内容上。块级堆叠让标题按容器全宽排版，换行自然撑高。
            div {
                style: "margin-bottom: 12px;",
                h3 { style: "margin: 0; font-size: 15px; font-weight: 500; color: {t.text};", "{title}" }
                if let Some(badge) = badge {
                    div {
                        style: "display: inline-block; margin-top: 6px; padding: 1px 8px; border: 1px solid {t.border}; border-radius: 999px; background: {t.active_bg}; color: {t.text_secondary}; font-size: 11px; line-height: 18px; white-space: nowrap;",
                        "{badge}"
                    }
                }
            }
            {children}
        }
    }
}

#[component]
pub fn ActionButton(
    label: String,
    onclick: EventHandler<MouseEvent>,
    #[props(default)] disabled: bool,
) -> Element {
    let t = (use_context::<AppState>().theme)();
    // 禁用态用中性灰，而不是把主按钮整体降透明度（后者看起来仍像可点）。
    let bg = if disabled {
        t.progress_track
    } else {
        t.primary_bg
    };
    let fg = if disabled {
        t.text_secondary
    } else {
        t.primary_fg
    };
    let cursor = if disabled { "default" } else { "pointer" };
    rsx! {
        button {
            r#type: "button",
            disabled,
            style: "padding: 8px 16px; border: none; border-radius: 6px; background: {bg}; color: {fg}; font-size: 14px; font-weight: 500; cursor: {cursor};",
            onclick: move |e| { if !disabled { onclick.call(e); } },
            "{label}"
        }
    }
}

#[component]
pub fn GhostButton(
    label: &'static str,
    onclick: EventHandler<MouseEvent>,
    #[props(default)] disabled: bool,
) -> Element {
    let t = (use_context::<AppState>().theme)();
    let opacity = if disabled { "0.5" } else { "1" };
    let cursor = if disabled { "default" } else { "pointer" };
    rsx! {
        button {
            r#type: "button",
            disabled,
            style: "padding: 8px 16px; border: 1px solid {t.border}; border-radius: 6px; background: transparent; color: {t.text_secondary}; font-size: 13px; cursor: {cursor}; flex-shrink: 0; opacity: {opacity};",
            onclick: move |e| { if !disabled { onclick.call(e); } },
            "{label}"
        }
    }
}

#[component]
fn SmallButton(
    label: String,
    onclick: EventHandler<MouseEvent>,
    #[props(default)] disabled: bool,
) -> Element {
    let t = (use_context::<AppState>().theme)();
    let bg = if disabled { t.active_bg } else { "transparent" };
    let cursor = if disabled { "default" } else { "pointer" };
    rsx! {
        button {
            r#type: "button",
            disabled,
            style: "padding: 6px 10px; border: 1px solid {t.border}; border-radius: 6px; background: {bg}; color: {t.text_secondary}; font-size: 12px; cursor: {cursor};",
            onclick: move |e| { if !disabled { onclick.call(e); } },
            "{label}"
        }
    }
}

#[component]
fn DangerButton(label: &'static str, onclick: EventHandler<MouseEvent>) -> Element {
    let t = (use_context::<AppState>().theme)();
    rsx! {
        button {
            r#type: "button",
            style: "padding: 6px 10px; border: 1px solid {t.danger_border}; border-radius: 6px; background: transparent; color: {t.danger}; font-size: 12px; cursor: pointer;",
            onclick: move |e| onclick.call(e),
            "{label}"
        }
    }
}

#[component]
pub fn ErrorRow(message: String) -> Element {
    let t = (use_context::<AppState>().theme)();
    rsx! { p { style: "color: {t.danger}; font-size: 13px; margin: 4px 0;", "{message}" } }
}

#[component]
pub fn TextInput(#[props(default)] placeholder: String, value: Signal<String>) -> Element {
    rsx! { InputField { placeholder, value, password: false } }
}

#[component]
pub fn PasswordInput(#[props(default)] placeholder: String, value: Signal<String>) -> Element {
    rsx! { InputField { placeholder, value, password: true } }
}

/// Blitz 暂不绘制原生 placeholder；仅原生端补一个不接收点击的提示层。
/// 输入值始终保持真实值，不能把提示文字塞进 value（尤其是密码框）。
#[component]
fn InputField(placeholder: String, value: Signal<String>, password: bool) -> Element {
    let t = (use_context::<AppState>().theme)();
    let show_hint =
        cfg!(not(target_arch = "wasm32")) && !placeholder.is_empty() && value.read().is_empty();
    rsx! {
        div {
            style: "position: relative; display: flex; flex: 1; min-width: 0;",
            // 提示位于透明 input 后面，input 通过直接兄弟间的 z-index 保持在上层；
            // 原生端即便忽略 pointer-events，命中测试仍落在真实输入框上。
            if show_hint {
                div {
                    "aria-hidden": "true",
                    style: "position: absolute; left: 13px; right: 13px; top: 0; bottom: 0; display: flex; align-items: center; pointer-events: none; overflow: hidden; color: {t.text_secondary}; font-size: 14px; line-height: 20px;",
                    "{placeholder}"
                }
            }
            input {
                r#type: if password { "password" } else { "text" },
                style: "position: relative; z-index: 1; box-sizing: border-box; flex: 1; min-width: 0; width: 100%; padding: 8px 12px; border: 1px solid {t.input_border}; border-radius: 6px; background: transparent; color: {t.text}; font-size: 14px; line-height: 20px;",
                placeholder: "{placeholder}",
                "aria-label": (!placeholder.is_empty()).then(|| placeholder.clone()),
                value: "{value}",
                oninput: move |e| value.set(e.value()),
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::account_status;

    #[test]
    fn saved_credentials_are_not_presented_as_a_verified_session() {
        assert_eq!(
            account_status(false, true),
            "已保存自动登录凭据，启动时验证"
        );
        assert_eq!(account_status(true, true), "本次会话已登录");
        assert_eq!(account_status(true, false), "本次会话已登录");
        assert_eq!(
            account_status(false, false),
            "未保存自动登录凭据，请重新添加账号"
        );
    }
}
