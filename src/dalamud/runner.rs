//! Dalamud 启动编排（Linux 通过 Wine 运行 Injector）。
//!
//! 对应 C# `UnixDalamudRunner.cs`。关键点：
//! - Injector 是 Windows 组件，在**与游戏相同的 Wine prefix** 内运行
//! - 所有传入 Injector 的路径须经 `winepath --windows` 转换为 `Z:\...`
//! - Injector stdout 输出单行 JSON `{pid, handle}`（Wine PID）

use std::path::Path;
use std::process::Command;
use tracing::{debug, info};

use super::model::{build_injector_launch_args, DalamudLoadMethod, DalamudStartInfo, InjectorResult};
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
    let injector_args =
        build_injector_launch_args(start, load_method, game_args, without_dalamud);

    info!(injector = ?injector_exe, "launching game through Dalamud Injector");
    let mut cmd = Command::new(&wine.wine64_path);
    cmd.arg(injector_exe)
        .args(&injector_args)
        .current_dir(start.working_directory.clone())
        .env("WINEPREFIX", &wine.prefix_path)
        .env("XL_WINEONLINUX", "true");
    for (k, v) in env {
        cmd.env(k, v);
    }

    // 捕获 stdout 用于解析 JSON（stderr 独立，避免干扰）
    use std::process::Stdio;
    cmd.stdout(Stdio::piped()).stderr(Stdio::piped());

    let mut child = cmd.spawn().map_err(WineError::Io)?;

    // 读取 stdout 单行 JSON（参考 C#：最多等待 ~30s）
    let pid = read_injector_pid(&mut child)?;

    debug!(wine_pid = pid, "Injector reported game process");
    Ok(InjectorLaunch {
        injector: child,
        wine_pid: pid,
    })
}

/// 从 Injector stdout 读取并解析 `{pid, handle}` JSON（带超时）。
fn read_injector_pid(child: &mut std::process::Child) -> Result<u32, WineError> {
    use std::io::BufRead;
    use std::time::{Duration, Instant};

    let stdout = child
        .stdout
        .take()
        .ok_or_else(|| WineError::Probe("injector stdout unavailable".into()))?;

    let mut reader = std::io::BufReader::new(stdout);
    let deadline = Instant::now() + Duration::from_secs(30);
    let mut buf = String::new();

    loop {
        if Instant::now() > deadline {
            let _ = child.kill();
            return Err(WineError::Probe(
                "timed out waiting for Injector result".into(),
            ));
        }
        buf.clear();
        match reader.read_line(&mut buf) {
            Ok(0) => {
                return Err(WineError::Probe(
                    "Injector exited without reporting a result".into(),
                ));
            }
            Ok(_) => {
                let line = buf.trim();
                if line.is_empty() {
                    continue;
                }
                debug!(line = line, "injector output line");
                if let Ok(result) = serde_json::from_str::<InjectorResult>(line) {
                    return Ok(result.pid);
                }
                // 非 JSON 行（诊断输出）继续读
            }
            Err(e) => return Err(WineError::Io(e)),
        }
    }
}

/// 构建完整游戏启动参数（供 Injector `--` 后使用）。
///
/// 复用 `game::build_sdo_launch_args`，返回逐参数列表（不拼接）。
pub fn game_argv(args_str: &str) -> Vec<String> {
    args_str
        .split_whitespace()
        .map(|s| s.to_string())
        .collect()
}

/// 错误转换：把 WineError 包装为启动错误信息。
pub fn map_wine_error(e: WineError) -> String {
    match e {
        WineError::Probe(msg) => msg,
        other => other.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_game_argv_split() {
        let args = game_argv("-AppID=1 Dev.LobbyHost01=a.com DEV.TestSID=tok");
        assert_eq!(args.len(), 3);
        assert_eq!(args[0], "-AppID=1");
    }
}
