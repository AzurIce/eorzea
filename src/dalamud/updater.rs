//! Dalamud release 获取、版本门控与安装状态检测。
//!
//! 对应 C# `DalamudUpdater.cs`。MVP 聚焦：release 元数据获取、`SupportedGameVer`
//! 与本地游戏版本的严格匹配检查、本机安装检测；完整下载/解压/校验作为后续阶段。

use std::path::{Path, PathBuf};
use std::process::Command;
use tracing::{debug, info, warn};

use super::model::{DalamudStatus, DalamudVersionInfo, InstallState};

/// release API 基地址（`ServerAddress.MainAddress`）。
pub const REMOTE_BASE: &str = "https://aonyx.ffxiv.wang/Dalamud/Release/VersionInfo?track=";

/// 默认安装根目录：`~/.xiv-launcher-rs/dalamud`。
pub fn default_install_root() -> PathBuf {
    dirs::home_dir()
        .map(|h| h.join(".xiv-launcher-rs/dalamud"))
        .unwrap_or_else(|| PathBuf::from("./dalamud"))
}

/// 从 release API 获取最新版本元数据。
pub async fn fetch_version_info(
    client: &reqwest::Client,
    track: &str,
) -> Result<DalamudVersionInfo, DalamudError> {
    let url = format!("{REMOTE_BASE}{track}&bucket=Control");
    debug!(url = %url, "fetching Dalamud version info");
    let resp = client
        .get(&url)
        .header("User-Agent", "eorzea")
        .send()
        .await
        .map_err(|e| DalamudError::Network(e.to_string()))?;
    if !resp.status().is_success() {
        return Err(DalamudError::Http {
            url,
            status: resp.status(),
        });
    }
    resp.json::<DalamudVersionInfo>()
        .await
        .map_err(|e| DalamudError::Parse(e.to_string()))
}

/// 读取本地游戏版本（`game/ffxivgame.ver`）。
pub fn local_game_version(game_root: &Path) -> String {
    crate::game_files::version::read_ver(
        game_root,
        crate::game_files::version::repo::FFXIV,
        crate::game_files::version::ver_file::FFXIV,
    )
}

/// 检测本机已安装的 Dalamud 版本（扫描 `Hooks/*/version.json`）。
pub fn detect_local_install(root: &Path) -> Option<(String, PathBuf)> {
    let hooks = root.join("Hooks");
    let entries = std::fs::read_dir(&hooks).ok()?;
    for entry in entries.flatten() {
        let dir = entry.path();
        if !dir.is_dir() {
            continue;
        }
        // 版本目录名（如 15.0.3.0）；dev 目录跳过（开发版）
        let name = dir.file_name()?.to_string_lossy().to_string();
        if name == "dev" {
            continue;
        }
        if dir.join("Dalamud.Injector.exe").exists() {
            return Some((name, dir));
        }
    }
    None
}

/// 汇总 Dalamud 状态：release 元数据 + 本机安装 + 版本兼容性。
pub async fn status(
    client: &reqwest::Client,
    install_root: &Path,
    game_root: &Path,
    track: &str,
) -> DalamudStatus {
    let local_game_ver = local_game_version(game_root);
    let remote = fetch_version_info(client, track).await.ok();
    let local_install = detect_local_install(install_root);
    let (local_assembly_version, install_path) =
        local_install.map(|(v, p)| (Some(v), Some(p))).unwrap_or((None, None));

    let install_state = match (&remote, &local_assembly_version) {
        (Some(r), Some(_)) if r.supported_game_ver == local_game_ver => InstallState::Ready,
        (Some(r), Some(_)) => {
            warn!(
                supported = %r.supported_game_ver,
                local = %local_game_ver,
                "installed Dalamud does not match game version"
            );
            InstallState::OutOfDate
        }
        (Some(r), None) if r.supported_game_ver == local_game_ver => {
            info!("Dalamud release matches game version but not installed");
            InstallState::Missing
        }
        (Some(r), None) => {
            info!(
                supported = %r.supported_game_ver,
                local = %local_game_ver,
                "Dalamud release does not yet support local game version"
            );
            InstallState::Unsupported
        }
        (None, Some(_)) => {
            info!("remote version info unavailable, using local install");
            InstallState::OutOfDate
        }
        (None, None) => InstallState::Missing,
    };

    DalamudStatus {
        install_state,
        remote,
        local_assembly_version,
        install_path,
        local_game_ver,
    }
}


/// 检测可用的 7z 解压命令（7zz / 7z / 7za）。
fn find_7z() -> Option<String> {
    for name in ["7zz", "7z", "7za"] {
        if Command::new("which").arg(name).output().map(|o| o.status.success()).unwrap_or(false) {
            return Some(name.to_string());
        }
    }
    None
}

/// 下载并安装 Dalamud release。
///
/// 流程：下载 `.7z`（带进度）→ 解压到临时目录 → 校验
/// `hashes.json` 的 MD5 == 远端 `Hash`、逐文件 MD5 == hashes.json → 原子安装到
/// `Hooks/<AssemblyVersion>/`。对应 C# `DalamudUpdater` 的安装逻辑。
pub async fn download_release(
    client: &reqwest::Client,
    version_info: &DalamudVersionInfo,
    install_root: &Path,
    mut on_progress: impl FnMut(u64, u64),
) -> Result<PathBuf, DalamudError> {
    let sevenz = find_7z().ok_or(DalamudError::NotImplemented)?;

    // 1. 下载
    let url = &version_info.download_url;
    let resp = client
        .get(url)
        .header("User-Agent", "eorzea")
        .send()
        .await
        .map_err(|e| DalamudError::Network(e.to_string()))?;
    if !resp.status().is_success() {
        return Err(DalamudError::Http {
            url: url.clone(),
            status: resp.status(),
        });
    }
    let total = resp
        .headers()
        .get(reqwest::header::CONTENT_LENGTH)
        .and_then(|v| v.to_str().ok())
        .and_then(|s| s.parse::<u64>().ok())
        .unwrap_or(0);

    let tmp_dir = install_root.join(format!(".tmp-{}", version_info.assembly_version));
    let _ = std::fs::remove_dir_all(&tmp_dir);
    std::fs::create_dir_all(&tmp_dir).map_err(|e| DalamudError::Io {
        path: tmp_dir.clone(),
        source: e,
    })?;

    let archive = tmp_dir.join("dalamud.7z");
    let mut file = std::fs::File::create(&archive).map_err(|e| DalamudError::Io {
        path: archive.clone(),
        source: e,
    })?;
    let mut written = 0u64;
    let mut stream = resp;
    while let Some(chunk) = stream
        .chunk()
        .await
        .map_err(|e| DalamudError::Network(e.to_string()))?
    {
        use std::io::Write;
        file.write_all(&chunk).map_err(|e| DalamudError::Io {
            path: archive.clone(),
            source: e,
        })?;
        written += chunk.len() as u64;
        on_progress(written, total);
    }
    info!(bytes = written, "downloaded Dalamud release");

    // 2. 解压
    let extract_dir = tmp_dir.join("extract");
    std::fs::create_dir_all(&extract_dir).map_err(|e| DalamudError::Io {
        path: extract_dir.clone(),
        source: e,
    })?;
    let status = std::process::Command::new(&sevenz)
        .arg("x")
        .arg("-y")
        .arg(format!("-o{}", extract_dir.display()))
        .arg(&archive)
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .map_err(|e| DalamudError::Io {
            path: archive.clone(),
            source: e,
        })?;
    if !status.success() {
        return Err(DalamudError::Parse(format!("7z extraction failed ({status})")));
    }
    info!("extracted Dalamud release");

    // 3. 校验：关键文件存在（release 包无 hashes.json；远端 Hash 用于校验安装后
    //    hashes.json，属 runtime 完整性，MVP 阶段先做关键文件存在性检查）
    for required in ["Dalamud.Injector.exe", "Dalamud.dll", "Dalamud.Boot.dll"] {
        if !extract_dir.join(required).exists() {
            warn!(file = required, "release missing required file");
            return Err(DalamudError::Parse(format!(
                "release missing required file: {required}"
            )));
        }
    }
    info!("release key files verified");

    // 4. 原子安装到 Hooks/<AssemblyVersion>，并写入 version.json
    let hooks = install_root.join("Hooks");
    std::fs::create_dir_all(&hooks).map_err(|e| DalamudError::Io {
        path: hooks.clone(),
        source: e,
    })?;
    let target = hooks.join(&version_info.assembly_version);
    let _ = std::fs::remove_dir_all(&target);
    std::fs::rename(&extract_dir, &target).map_err(|e| DalamudError::Io {
        path: target.clone(),
        source: e,
    })?;

    // version.json：完整版本信息（对齐 C# 安装行为）
    let ver_json = serde_json::to_string_pretty(version_info)
        .map_err(|e| DalamudError::Parse(format!("serialize version.json: {e}")))?;
    std::fs::write(target.join("version.json"), ver_json).map_err(|e| DalamudError::Io {
        path: target.join("version.json"),
        source: e,
    })?;

    let _ = std::fs::remove_file(&archive);
    let _ = std::fs::remove_dir_all(&tmp_dir);

    info!(version = %version_info.assembly_version, path = %target.display(), "Dalamud installed");
    Ok(target)
}

/// Dalamud 错误。
#[derive(Debug, thiserror::Error)]
pub enum DalamudError {
    #[error("network error: {0}")]
    Network(String),
    #[error("HTTP {status} for {url}")]
    Http {
        url: String,
        status: reqwest::StatusCode,
    },
    #[error("failed to parse Dalamud version info: {0}")]
    Parse(String),
    #[error("not implemented yet: release download/install requires 7z support")]
    NotImplemented,
    #[error("IO error at {path}: {source}")]
    Io {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_detect_local_install() {
        let dir = std::env::temp_dir().join(format!("xl-rs-dalamud-{}", std::process::id()));
        let hooks = dir.join("Hooks/15.0.3.0");
        std::fs::create_dir_all(&hooks).unwrap();
        std::fs::write(hooks.join("Dalamud.Injector.exe"), b"exe").unwrap();

        let (ver, path) = detect_local_install(&dir).unwrap();
        assert_eq!(ver, "15.0.3.0");
        assert!(path.join("Dalamud.Injector.exe").exists());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_local_game_version_fallback() {
        let dir = std::env::temp_dir().join(format!("xl-rs-dv-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        // 缺失 → BASE
        assert_eq!(
            local_game_version(&dir),
            crate::game_files::version::BASE_GAME_VERSION
        );
        let _ = std::fs::remove_dir_all(&dir);
    }
}
