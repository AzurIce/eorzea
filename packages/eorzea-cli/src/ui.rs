//! `eoz` 的终端输出层：统一标题/状态行、进度条，以及 tracing 接线。
//!
//! 约定：
//! - **用户可读内容走 stdout**（结果、表格、提示），**日志与错误走 stderr**；这样
//!   `eoz game check > patches.txt` 之类的重定向不会混进日志噪音。
//! - **库日志默认静默**（`warn` 起）。要看过程用 `-v`（info）/`-vv`（debug）/`-vvv`，
//!   或 `RUST_LOG=…` 精确控制。此前默认 info，导致 `INFO eorzea_auth::sdo: Creating
//!   SdoAuth client` 这类内部分解步骤直接糊在用户输出里。
//! - **进度条用 indicatif**，日志通过 [`IndicatifWriter`] 写出：写日志前会先挂起进度条，
//!   所以不会出现「日志把进度条截断」的花屏。所有 `ui::*` 输出助手同样在挂起状态下打印。
//! - **非 TTY / `--no-progress` 时自动退化**：进度条不再绘制，改为每个阶段一行结果，
//!   管道与 CI 里输出干净可读。

use std::cell::Cell;
use std::fmt::Display;
use std::io::{self, IsTerminal};
use std::sync::OnceLock;
use std::time::Duration;

use console::style;
use indicatif::{MultiProgress, ProgressBar, ProgressDrawTarget, ProgressStyle};
use tracing_indicatif::IndicatifWriter;
use tracing_subscriber::EnvFilter;

/// 所有进度条共用的 `MultiProgress`；日志 writer 也挂在它上面以互相避让。
static PROGRESS: OnceLock<MultiProgress> = OnceLock::new();
/// 是否禁用进度条绘制（`--no-progress` 或 stderr 不是 TTY）。
static QUIET_BARS: OnceLock<bool> = OnceLock::new();

fn progress() -> &'static MultiProgress {
    PROGRESS.get_or_init(MultiProgress::new)
}

fn bars_hidden() -> bool {
    *QUIET_BARS.get_or_init(|| !io::stderr().is_terminal())
}

/// 初始化日志与进度条（在任何输出之前调用一次）。
pub fn init(verbose: u8, no_progress: bool) {
    let _ = QUIET_BARS.set(no_progress || !io::stderr().is_terminal());

    let level = match verbose {
        0 => "warn",
        1 => "info",
        2 => "debug",
        _ => "trace",
    };
    let filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| {
        EnvFilter::new(format!(
            "eorzea_lib={level},eorzea_auth={level},eorzea_cli={level},warn"
        ))
    });

    let fmt = tracing_subscriber::fmt()
        .with_env_filter(filter)
        // 显式指定 stderr 目标：日志与进度条都在 stderr，且写日志前会挂起进度条
        .with_writer(IndicatifWriter::<tracing_indicatif::writer::Stderr>::new(
            progress().clone(),
        ))
        .with_ansi(io::stderr().is_terminal())
        // 默认把 target/时间戳藏起来（CLI 里它们只是噪音）；-vv 起打开便于排查
        .with_target(verbose >= 2)
        .with_file(verbose >= 3)
        .with_line_number(verbose >= 3);
    if verbose >= 2 {
        fmt.init();
    } else {
        fmt.without_time().init();
    }
}

// ── 文本行 ───────────────────────────────────────────────────────────────

/// 原样输出一行到 stdout（安全：会先挂起进度条绘制）。
pub fn line(msg: impl Display) {
    progress().suspend(|| println!("{msg}"));
}

/// 原样输出一行到 stderr。
pub fn eline(msg: impl Display) {
    progress().suspend(|| eprintln!("{msg}"));
}

/// 段落标题：`==> 检查更新`
pub fn banner(title: impl Display) {
    line(format!(
        "\n{} {}",
        style("==>").cyan().bold(),
        style(title).bold()
    ));
}

/// 缩进说明行：`  正在应用补丁…`
pub fn step(msg: impl Display) {
    line(format!("  {msg}"));
}

/// 成功：`✓ 游戏已启动（PID 1234）`
pub fn ok(msg: impl Display) {
    line(format!("{} {msg}", style("✓").green().bold()));
}

/// 提示：`· 使用配置中的默认账号`
pub fn hint(msg: impl Display) {
    line(format!("{} {msg}", style("·").cyan().dim()));
}

/// 次要状态文字（如「禁用」），供 `kv` 等拼接使用。
pub fn dim(msg: impl Display) -> console::StyledObject<String> {
    style(msg.to_string()).dim()
}

/// 强调文字（如版本号、账号名）。
pub fn em(msg: impl Display) -> console::StyledObject<String> {
    style(msg.to_string()).bold()
}

/// 警告（stdout，属于正常流程的一部分）：`! 远端元数据不可用，使用本地记录`
pub fn warn(msg: impl Display) {
    line(format!("{} {msg}", style("!").yellow().bold()));
}

/// 错误（stderr）：`✗ 检查更新失败: …`
pub fn fail(msg: impl Display) {
    eline(format!("{} {msg}", style("✗").red().bold()));
}

/// 键值行：`  账号          xxx`（按显示宽度对齐，中英混排不错位）。
pub fn kv(key: &str, value: impl Display) {
    const COL: usize = 14;
    let pad = COL.saturating_sub(display_width(key));
    line(format!(
        "  {}{}{}",
        style(key).dim(),
        " ".repeat(pad),
        value
    ));
}

/// 表格行：按显示宽度对齐的 `列 列 列`。
pub fn row(cols: &[String], widths: &[usize]) {
    let mut out = String::from("  ");
    for (i, col) in cols.iter().enumerate() {
        let width = widths.get(i).copied().unwrap_or(0);
        if i + 1 == cols.len() {
            out.push_str(col);
        } else {
            // 中文按 2 列宽计算，否则中英混排会错位
            let pad = width.saturating_sub(display_width(col)) + 2;
            out.push_str(col);
            out.push_str(&" ".repeat(pad));
        }
    }
    line(out);
}

/// 估算终端显示宽度（CJK 记 2 列）。
pub fn display_width(s: &str) -> usize {
    s.chars()
        .map(|c| {
            let cp = c as u32;
            let wide = (0x1100..=0x115F).contains(&cp)
                || (0x2E80..=0xA4CF).contains(&cp)
                || (0xAC00..=0xD7A3).contains(&cp)
                || (0xF900..=0xFAFF).contains(&cp)
                || (0xFE30..=0xFE6F).contains(&cp)
                || (0xFF00..=0xFF60).contains(&cp)
                || (0xFFE0..=0xFFE6).contains(&cp)
                || (0x20000..=0x3FFFD).contains(&cp);
            if wide {
                2
            } else {
                1
            }
        })
        .sum()
}

/// 失败并退出（统一错误出口：`✗ …` + exit 1）。
pub fn die(msg: impl Display) -> ! {
    fail(msg);
    std::process::exit(1);
}

// ── 进度条 ───────────────────────────────────────────────────────────────

fn new_bar() -> ProgressBar {
    let bar = ProgressBar::new(0);
    if bars_hidden() {
        bar.set_draw_target(ProgressDrawTarget::hidden());
    } else {
        progress().add(bar.clone());
    }
    bar
}

fn byte_style() -> ProgressStyle {
    ProgressStyle::with_template(
        "{spinner:.cyan} {msg:<20} [{bar:26.cyan/blue}] {binary_bytes:>10}/{binary_total_bytes:>10} {binary_bytes_per_sec:>12} ETA {eta:>4}",
    )
    .expect("valid template")
    .progress_chars("=> ")
}

fn byte_unknown_style() -> ProgressStyle {
    ProgressStyle::with_template(
        "{spinner:.cyan} {msg:<20} {binary_bytes:>10} {binary_bytes_per_sec:>12}",
    )
    .expect("valid template")
}

fn spinner_style() -> ProgressStyle {
    ProgressStyle::with_template("{spinner:.cyan} {msg}").expect("valid template")
}

/// 字节进度条：下载/解压等有明确总量的阶段。
pub struct ByteBar {
    bar: ProgressBar,
    label: String,
    /// 首次拿到总量时切换到「有总量」样式（只切一次）。
    styled_known: Cell<bool>,
}

impl ByteBar {
    /// 新建并立即显示一行「阶段开始」提示（总量未知时按未知样式起步）。
    pub fn new(label: impl Into<String>) -> Self {
        let label = label.into();
        let bar = new_bar();
        bar.set_style(byte_unknown_style());
        bar.set_message(label.clone());
        bar.enable_steady_tick(Duration::from_millis(120));
        if bars_hidden() {
            step(format!("{label}…"));
        }
        Self {
            bar,
            label,
            styled_known: Cell::new(false),
        }
    }

    /// 更新进度（`total == 0` 表示长度未知）。
    pub fn set_progress(&self, done: u64, total: u64) {
        if total > 0 {
            if !self.styled_known.get() {
                self.bar.set_style(byte_style());
                self.styled_known.set(true);
            }
            self.bar.set_length(total);
        }
        self.bar.set_position(done);
    }

    /// 生成可直接传给库里 `on_progress(done, total)` 回调的闭包。
    pub fn callback(&self) -> impl FnMut(u64, u64) + '_ {
        move |done, total| self.set_progress(done, total)
    }

    /// 阶段完成：清掉动态行，打一行 `✓ 结果`。
    pub fn finish(self, msg: impl Display) {
        let bytes = self.bar.position();
        self.bar.finish_and_clear();
        if bars_hidden() && bytes > 0 {
            ok(format!("{}（{}）", msg, human_bytes(bytes)));
        } else {
            ok(msg);
        }
    }

    /// 阶段完成，结果用阶段名：`✓ release 本体 就绪`。
    pub fn finish_ready(self) {
        let label = self.label.clone();
        self.finish(format!("{label} 就绪"));
    }

    /// 只清掉动态行，不输出结果（后面还有别的输出时用）。
    pub fn clear(self) {
        self.bar.finish_and_clear();
    }

    /// 阶段失败：清掉动态行，打一行 `✗ 阶段名: 原因`（不退出，由调用方决定）。
    pub fn fail(self, msg: impl Display) {
        self.bar.finish_and_clear();
        fail(format!("{}: {msg}", self.label));
    }
}

/// 转圈提示：时长不确定的等待（登录、校验、启动游戏等）。
pub struct Spinner {
    bar: ProgressBar,
}

impl Spinner {
    pub fn new(msg: impl Into<String>) -> Self {
        let msg = msg.into();
        let bar = new_bar();
        bar.set_style(spinner_style());
        bar.set_message(msg.clone());
        bar.enable_steady_tick(Duration::from_millis(90));
        if bars_hidden() {
            step(format!("{msg}…"));
        }
        Self { bar }
    }

    /// 结束并输出一行 `✓ 结果`。
    pub fn finish(self, msg: impl Display) {
        self.bar.finish_and_clear();
        ok(msg);
    }

    /// 只清掉动态行，不输出任何结果。
    pub fn clear(self) {
        self.bar.finish_and_clear();
    }
}

/// 1024 进制的人类可读字节数（与 indicatif 的 `binary_bytes` 一致）。
pub fn human_bytes(bytes: u64) -> String {
    const KB: f64 = 1024.0;
    const MB: f64 = KB * 1024.0;
    const GB: f64 = MB * 1024.0;
    let b = bytes as f64;
    if b >= GB {
        format!("{:.2} GiB", b / GB)
    } else if b >= MB {
        format!("{:.2} MiB", b / MB)
    } else if b >= KB {
        format!("{:.2} KiB", b / KB)
    } else {
        format!("{bytes} B")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_display_width_cjk() {
        assert_eq!(display_width("abc"), 3);
        assert_eq!(display_width("大区"), 4);
        assert_eq!(display_width("ex1 版本"), 8);
    }

    #[test]
    fn test_human_bytes() {
        assert_eq!(human_bytes(0), "0 B");
        assert_eq!(human_bytes(1024), "1.00 KiB");
        assert_eq!(human_bytes(13_403_199), "12.78 MiB");
    }

    /// 模板必须能被 indicatif 解析：样式函数里的 `with_template(...).expect(...)`
    /// 在模板写错时会 panic，这个测试保证不再等到真实下载时才炸。
    #[test]
    fn test_progress_templates_are_valid() {
        for style in [byte_style(), byte_unknown_style(), spinner_style()] {
            let bar = ProgressBar::new(100);
            bar.set_style(style);
            bar.set_message("template");
            bar.set_position(50);
            assert_eq!(bar.position(), 50);
        }
    }

    /// 进度条在非 TTY 下不绘制，但回调仍应累计进度（`finish` 会补一行结果）。
    #[test]
    fn test_byte_bar_tracks_progress_when_hidden() {
        let bar = ByteBar::new("测试");
        {
            let mut cb = bar.callback();
            cb(0, 0); // 长度未知
            cb(512, 1024);
            cb(1024, 1024);
        }
        assert_eq!(bar.bar.position(), 1024);
        bar.finish("完成");
    }
}
