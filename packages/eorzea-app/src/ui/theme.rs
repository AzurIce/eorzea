//! 亮/暗双主题（shadcn/ui zinc 色系）。
//!
//! 所有语义色为 hex 字符串，供 rsx `style` 属性 `{}` 插值使用。

/// 一套语义色。
#[derive(Clone, Copy)]
pub struct Theme {
    /// 是否为暗色主题（切换按钮据此决定下一套主题与文案）。
    pub dark: bool,
    /// 页面底色。
    pub page_bg: &'static str,
    /// 卡片底色（Section、下拉列表）。
    pub card_bg: &'static str,
    /// 常规边框（卡片、分隔线、ghost 按钮）。
    pub border: &'static str,
    /// 输入框 / 下拉按钮边框。
    pub input_border: &'static str,
    /// 主文字。
    pub text: &'static str,
    /// 次要文字（提示、标签、状态栏）。
    pub text_secondary: &'static str,
    /// primary 按钮底色 / 进度条填充。
    pub primary_bg: &'static str,
    /// primary 按钮文字。
    pub primary_fg: &'static str,
    /// 侧边栏导航 / 选项组激活项底色。
    pub active_bg: &'static str,
    /// 危险色（错误文字、危险按钮文字）。
    pub danger: &'static str,
    /// 危险按钮边框。
    pub danger_border: &'static str,
    /// 成功提示文字。
    pub success: &'static str,
    /// 警告提示文字。
    pub warning: &'static str,
    /// 进度条轨道。
    pub progress_track: &'static str,
    /// 复选框 accent-color（blitz 用 currentColor：勾选=该色填充 + 白勾）。
    pub checkbox_accent: &'static str,
}

impl Theme {
    /// 暗色主题（zinc dark）。
    pub fn dark() -> Self {
        Self {
            dark: true,
            page_bg: "#09090b",
            card_bg: "#0c0c0f",
            border: "#27272a",
            input_border: "#3f3f46",
            text: "#fafafa",
            text_secondary: "#a1a1aa",
            primary_bg: "#fafafa",
            primary_fg: "#18181b",
            active_bg: "#27272a",
            danger: "#ef4444",
            danger_border: "#7f1d1d",
            success: "#4ade80",
            warning: "#fbbf24",
            progress_track: "#27272a",
            checkbox_accent: "#18181b",
        }
    }

    /// 亮色主题（zinc light）。
    pub fn light() -> Self {
        Self {
            dark: false,
            page_bg: "#ffffff",
            card_bg: "#ffffff",
            border: "#e4e4e7",
            input_border: "#d4d4d8",
            text: "#09090b",
            text_secondary: "#71717a",
            primary_bg: "#18181b",
            primary_fg: "#fafafa",
            active_bg: "#f4f4f5",
            danger: "#dc2626",
            danger_border: "#fca5a5",
            success: "#16a34a",
            warning: "#d97706",
            progress_track: "#e4e4e7",
            checkbox_accent: "#09090b",
        }
    }
}
