//! Dalamud 启动编排（Linux 通过 Wine 运行 Injector）。
//!
//! 对应 C# `UnixDalamudRunner.cs`。关键点：
//! - Injector 是 Windows 组件，在**与游戏相同的 Wine prefix** 内运行
//! - 所有传入 Injector 的路径须经 `winepath --windows` 转换为 `Z:\...`
//! - Injector stdout 输出单行 JSON `{pid, handle}`（Wine PID）

use std::path::Path;
use std::process::Command;
use tracing::{debug, info};

use super::model::{
    build_injector_launch_args, DalamudLoadMethod, DalamudStartInfo, InjectorResult,
};
use crate::wine::{WineError, WineTool};

/// Injector 启动结果。
#[derive(Debug)]
pub struct InjectorLaunch {
    /// Injector 进程（可能已退出；游戏是另一个 Wine 进程）。
    pub injector: std::process::Child,
    /// Injector 报告的 Wine PID（游戏进程）。
    pub wine_pid: u32,
}

/// 通过 winepath 将 Unix 路径转换为 Windows 路径（`Z:\...`）。
///
/// 对应 C# `CompatibilityTools.UnixToWinePath()`。
pub fn unix_to_wine_path(wine: &WineTool, unix_path: &Path) -> Result<String, WineError> {
    wine.ensure_prefix()?;
    let output = Command::new(&wine.wine64_path)
        .args(["winepath", "--windows"])
        .arg(unix_path)
        .env("WINEPREFIX", &wine.prefix_path)
        .output()
        .map_err(WineError::Io)?;
    if !output.status.success() {
        return Err(WineError::Probe(format!(
            "winepath failed for {}: {}",
            unix_path.display(),
            String::from_utf8_lossy(&output.stderr).trim()
        )));
    }
    let text = String::from_utf8_lossy(&output.stdout);
    let win_path = text
        .lines()
        .next_back()
        .unwrap_or_default()
        .trim()
        .to_string();
    if win_path.is_empty() {
        return Err(WineError::Probe(format!(
            "winepath returned empty for {}",
            unix_path.display()
        )));
    }
    Ok(win_path)
}

/// 批量转换一组路径。
pub fn convert_paths(
    wine: &WineTool,
    paths: &[(&str, &Path)],
) -> Result<Vec<(String, String)>, WineError> {
    let mut out = Vec::new();
    for (label, p) in paths {
        let win = unix_to_wine_path(wine, p)?;
        out.push((label.to_string(), win));
    }
    Ok(out)
}

/// 通过 Wine 运行 `Dalamud.Injector.exe launch`，解析 JSON 结果。
///
/// `injector_exe`：Injector 的 Unix 路径（Hooks/<ver>/Dalamud.Injector.exe）。
/// `start` 中的路径须已转换为 Windows 格式。
/// `game_args`：`--` 之后的游戏启动参数（**逐个传递，不拼接**）。
pub fn launch_through_injector(
    wine: &WineTool,
    injector_exe: &Path,
    start: &DalamudStartInfo,
    load_method: DalamudLoadMethod,
    game_args: &[String],
    without_dalamud: bool,
    env: &[(String, String)],
) -> Result<InjectorLaunch, WineError> {
    let injector_args = build_injector_launch_args(start, load_method, game_args, without_dalamud);

    info!(injector = ?injector_exe, "launching game through Dalamud Injector");
    let mut cmd = Command::new(&wine.wine64_path);
    cmd.arg(injector_exe)
        .args(&injector_args)
        // start.working_directory 已转换为 Z:\...（Windows 语义），只能传给
        // Injector；宿主进程的 current_dir 必须是 Unix 路径，否则 spawn 直接失败。
        .current_dir(
            injector_exe
                .parent()
                .filter(|p| !p.as_os_str().is_empty())
                .unwrap_or_else(|| Path::new(".")),
        )
        .env("WINEPREFIX", &wine.prefix_path)
        .env("XL_WINEONLINUX", "true");
    for (k, v) in env {
        cmd.env(k, v);
    }

    // 捕获 stdout 用于解析 JSON（stderr 独立，避免干扰）
    use std::process::Stdio;
    cmd.stdout(Stdio::piped()).stderr(Stdio::piped());

    let mut child = cmd.spawn().map_err(WineError::Io)?;

    // stderr 必须持续排空：Injector 的诊断输出可能超过管道缓冲，
    // 若无人读取会阻塞进程，导致永远等不到 stdout 里的 JSON。
    // 同时收集最近若干行，出错时回显给用户排障。
    let stderr_lines: std::sync::Arc<std::sync::Mutex<Vec<String>>> =
        std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
    if let Some(stderr) = child.stderr.take() {
        let buf = stderr_lines.clone();
        std::thread::spawn(move || {
            use std::io::BufRead;
            let reader = std::io::BufReader::new(stderr);
            for line in reader.lines() {
                match line {
                    Ok(line) if !line.trim().is_empty() => {
                        debug!(line = %line, "injector stderr");
                        let mut b = buf.lock().unwrap();
                        if b.len() >= 50 {
                            b.remove(0);
                        }
                        b.push(line);
                    }
                    Ok(_) => {}
                    Err(_) => break,
                }
            }
        });
    }

    // 读取 stdout 单行 JSON（参考 C#：最多等待 ~30s）
    let pid = read_injector_pid(&mut child, stderr_lines)?;

    debug!(wine_pid = pid, "Injector reported game process");
    Ok(InjectorLaunch {
        injector: child,
        wine_pid: pid,
    })
}

/// 从 Injector stdout 读取并解析 `{pid, handle}` JSON（带总超时）。
///
/// 读取放在独立线程里，用 `recv_timeout` 驱动超时；如果直接在调用线程
/// `read_line`，Injector 不输出换行时阻塞调用无法被 30s deadline 打断。

fn read_injector_pid(
    child: &mut std::process::Child,
    stderr_lines: std::sync::Arc<std::sync::Mutex<Vec<String>>>,
) -> Result<u32, WineError> {
    use std::io::BufRead;
    use std::sync::mpsc;
    use std::time::{Duration, Instant};

    let stdout = child
        .stdout
        .take()
        .ok_or_else(|| WineError::Probe("injector stdout unavailable".into()))?;
    let (tx, rx) = mpsc::channel::<std::io::Result<String>>();
    std::thread::spawn(move || {
        let reader = std::io::BufReader::new(stdout);
        for line in reader.lines() {
            if tx.send(line).is_err() {
                break;
            }
        }
    });

    let format_stderr = || {
        let lines = stderr_lines.lock().unwrap();
        if lines.is_empty() {
            String::new()
        } else {
            format!("\nInjector stderr:\n{}", lines.join("\n"))
        }
    };

    let deadline = Instant::now() + Duration::from_secs(30);
    loop {
        let now = Instant::now();
        if now >= deadline {
            let _ = child.kill();
            return Err(WineError::Probe(format!(
                "timed out waiting for Injector result{}",
                format_stderr()
            )));
        }
        match rx.recv_timeout(deadline - now) {
            Ok(Ok(line)) => {
                let line = line.trim();
                if line.is_empty() {
                    continue;
                }
                debug!(line = line, "injector output line");
                if let Ok(result) = serde_json::from_str::<InjectorResult>(line) {
                    return Ok(result.pid);
                }
                // 非 JSON 行（诊断输出）继续读
            }
            Ok(Err(e)) => return Err(WineError::Io(e)),
            Err(mpsc::RecvTimeoutError::Timeout) => continue,
            Err(mpsc::RecvTimeoutError::Disconnected) => {
                return Err(WineError::Probe(format!(
                    "Injector exited without reporting a result{}",
                    format_stderr()
                )));
            }
        }
    }
}
