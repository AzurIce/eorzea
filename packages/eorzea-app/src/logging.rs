//! 在应用内保留一份 `tracing` 日志，供主页的「运行日志」区域展示。
//!
//! 桌面端没有控制台（Windows release 尤其如此），启动链路（Dalamud 下载、
//! 自动登录、wine 探测等）的日志只在终端可见。这里用一个额外的
//! [`tracing_subscriber::Layer`] 把 INFO 及以上级别的事件写入全局环形缓冲，
//! UI 定时增量读取后渲染。

use std::collections::VecDeque;
use std::fmt::Debug;
use std::sync::{LazyLock, Mutex};
use std::time::Instant;

use tracing::field::{Field, Visit};
use tracing::{Event, Level, Subscriber};
use tracing_subscriber::EnvFilter;
use tracing_subscriber::layer::{Context, Layer, SubscriberExt};
use tracing_subscriber::registry;
use tracing_subscriber::util::SubscriberInitExt;

/// 日志级别（UI 只区分这三种颜色）。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LogLevel {
    Info,
    Warn,
    Error,
}

/// 一条日志。
#[derive(Clone, Debug)]
pub struct LogEntry {
    pub level: LogLevel,
    pub text: String,
}

/// 全局保留的最大行数（超出丢弃最旧的）。
const MAX_LINES: usize = 1000;
/// UI 侧最多渲染的行数。
pub const MAX_UI_LINES: usize = 300;

struct Store {
    lines: VecDeque<LogEntry>,
    /// 累计写入总行数，UI 用它做增量读取。
    total: u64,
}

static STORE: LazyLock<Mutex<Store>> = LazyLock::new(|| {
    Mutex::new(Store {
        lines: VecDeque::new(),
        total: 0,
    })
});

static START: LazyLock<Instant> = LazyLock::new(Instant::now);

/// 安装全局 subscriber：控制台 fmt 输出 + UI 环形缓冲。
///
/// 默认只打开本项目的 INFO 日志（`RUST_LOG` 可覆盖），避免依赖库的噪音。
pub fn init() {
    // 时间戳从应用启动（而不是第一条日志）开始计时。
    LazyLock::force(&START);
    let filter = EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| EnvFilter::new("eorzea=info,eorzea_auth=info,eorzea_app=info"));
    let _ = registry()
        .with(filter)
        .with(tracing_subscriber::fmt::layer())
        .with(UiLayer)
        .try_init();
}

/// 读取 `seq` 之后新增的日志，并返回当前总行数。
///
/// 环形缓冲已丢弃的旧行不会返回；调用方按返回的总行数继续增量读取即可。
pub fn snapshot_since(seq: u64) -> (Vec<LogEntry>, u64) {
    let store = STORE.lock().unwrap_or_else(|e| e.into_inner());
    let first = store.total.saturating_sub(store.lines.len() as u64);
    let skip = seq.saturating_sub(first) as usize;
    let entries = store.lines.iter().skip(skip).cloned().collect();
    (entries, store.total)
}

fn push(entry: LogEntry) {
    let mut store = STORE.lock().unwrap_or_else(|e| e.into_inner());
    store.total += 1;
    store.lines.push_back(entry);
    while store.lines.len() > MAX_LINES {
        store.lines.pop_front();
    }
}

struct UiLayer;

impl<S: Subscriber> Layer<S> for UiLayer {
    fn on_event(&self, event: &Event<'_>, _ctx: Context<'_, S>) {
        let level = match *event.metadata().level() {
            Level::ERROR => LogLevel::Error,
            Level::WARN => LogLevel::Warn,
            Level::INFO => LogLevel::Info,
            // DEBUG/TRACE 不进 UI，避免刷屏（终端仍按 RUST_LOG 输出）。
            _ => return,
        };

        let mut visitor = Visitor::default();
        event.record(&mut visitor);

        let elapsed = START.elapsed().as_secs_f64();
        let tag = match level {
            LogLevel::Info => "INFO ",
            LogLevel::Warn => "WARN ",
            LogLevel::Error => "ERROR",
        };
        let mut text = format!("[{elapsed:>8.2}s] {tag} ");
        text.push_str(&visitor.message);
        if !visitor.fields.is_empty() {
            text.push(' ');
            text.push_str(&visitor.fields.join(" "));
        }

        push(LogEntry { level, text });
    }
}

/// 提取 `message` 与其余字段（`path=... wine_type=...`）。
#[derive(Default)]
struct Visitor {
    message: String,
    fields: Vec<String>,
}

impl Visit for Visitor {
    fn record_str(&mut self, field: &Field, value: &str) {
        if field.name() == "message" {
            self.message = value.to_string();
        } else {
            self.fields.push(format!("{}={}", field.name(), value));
        }
    }

    fn record_debug(&mut self, field: &Field, value: &dyn Debug) {
        if field.name() == "message" {
            // `message` 以 `format_args!` 记录，Debug 即原始文本。
            self.message = format!("{value:?}");
        } else {
            self.fields.push(format!("{}={:?}", field.name(), value));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn snapshot_returns_only_new_lines() {
        let (_, base) = snapshot_since(0);
        push(LogEntry {
            level: LogLevel::Info,
            text: "first".into(),
        });
        push(LogEntry {
            level: LogLevel::Warn,
            text: "second".into(),
        });

        let (entries, total) = snapshot_since(base);
        assert_eq!(entries.len(), 2);
        assert_eq!(entries[1].text, "second");
        assert_eq!(total, base + 2);

        let (none, total_again) = snapshot_since(total);
        assert!(none.is_empty());
        assert_eq!(total_again, total);
    }
}
