//! 不读取账号或发起网络请求的原生文字布局对照。
//! cargo run -p eorzea-app --example span_probe

use dioxus::prelude::*;

#[cfg(not(target_arch = "wasm32"))]
fn main() {
    let window = dioxus_native::WindowAttributes::default()
        .with_title("Span rendering probe")
        .with_inner_size(dioxus_native::LogicalSize::new(640.0, 560.0));
    dioxus_native::launch_cfg(app, vec![], vec![Box::new(window)]);
}

#[cfg(target_arch = "wasm32")]
fn main() {
    dioxus::launch(app);
}

fn app() -> Element {
    rsx! {
        style { "html, body {{ margin: 0; padding: 0; }}" }
        div {
            style: "padding: 24px; background: #fff; color: #09090b; font-family: 'Microsoft YaHei', 'PingFang SC', 'Noto Sans CJK SC', sans-serif;",
            h2 { "原生文字布局对照" }
            p { "此窗口只显示固定文本，不读取配置或进行登录。" }
            p { "普通 inline span" }
            span { "陆行鸟 / Sample account" }
            p { "flex 子项 span + overflow: hidden" }
            button {
                style: "display: flex; align-items: center; gap: 12px; width: 280px; padding: 8px 12px; border: 1px solid #d4d4d8; border-radius: 6px; background: #fff; color: #09090b; font-size: 14px;",
                span {
                    style: "flex: 1; min-width: 0; white-space: nowrap; overflow: hidden; text-overflow: ellipsis;",
                    "陆行鸟 / Sample account"
                }
                Chevron {}
            }
            p { "flex 子项 div（与现用下拉框一致，无裁剪）" }
            button {
                style: "display: flex; align-items: center; gap: 12px; width: 280px; padding: 8px 12px; border: 1px solid #d4d4d8; border-radius: 6px; background: #fff; color: #09090b; font-size: 14px;",
                div {
                    style: "flex: 1; min-width: 0; line-height: 20px; overflow-wrap: anywhere;",
                    "陆行鸟 / Sample account"
                }
                Chevron {}
            }
            p { "div 块级文本（不在 button 内）" }
            div { "陆行鸟 / Sample account" }
        }
    }
}

#[component]
fn Chevron() -> Element {
    rsx! {
        svg {
            style: "flex-shrink: 0;",
            width: "16",
            height: "16",
            view_box: "0 0 24 24",
            fill: "none",
            stroke: "#71717a",
            stroke_width: "1.75",
            stroke_linecap: "round",
            stroke_linejoin: "round",
            path { d: "m6 9 6 6 6-6" }
        }
    }
}
