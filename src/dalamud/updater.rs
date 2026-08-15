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

/// 查找可用的 Windows x64 .NET runtime。
///
/// 优先本项目 `<install_root>/runtime`；其次兼容读取已有 XIVLauncher CN
/// 安装的 `~/.xlcore_cn/runtime`（迁移/排障过渡用）。
pub fn find_usable_runtime_dir(
    install_root: &Path,
    expected_version: Option<&str>,
) -> Option<PathBuf> {
    let mut candidates = vec![install_root.join("runtime")];
    if let Some(home) = dirs::home_dir() {
        candidates.push(home.join(".xlcore_cn/runtime"));
    }
    candidates
        .into_iter()
        .find(|p| runtime_dir_matches(p, expected_version))
}

/// 查找可用的 Dalamud assets 版本目录。
///
/// `--dalamud-asset-directory` 需要具体版本目录（例如 `dalamudAssets/115`）。
/// 这里读取 `asset.ver` 定位当前版本，并确认该目录非空；不做全局 fallback，
/// 避免把其他安装的 assets 版本误传给 Injector。
pub fn find_usable_asset_dir(install_root: &Path) -> Option<PathBuf> {
    let base = install_root.join("dalamudAssets");
    let version = std::fs::read_to_string(base.join("asset.ver"))
        .ok()
        .and_then(|s| s.trim().parse::<u32>().ok())?;
    let dir = base.join(version.to_string());
    let usable = dir.is_dir()
        && std::fs::read_dir(&dir)
            .ok()
            .and_then(|mut entries| entries.next().map(|e| e.is_ok()))
            .unwrap_or(false);
    usable.then_some(dir)
}

pub(super) fn runtime_dir_matches(root: &Path, expected_version: Option<&str>) -> bool {
    if !root.join("host/fxr").is_dir()
        || !root.join("shared/Microsoft.NETCore.App").is_dir()
        || !root.join("shared/Microsoft.WindowsDesktop.App").is_dir()
    {
        return false;
    }
    match expected_version {
        Some(version) => {
            root.join("host/fxr").join(version).join("hostfxr.dll").is_file()
                && root
                    .join("shared/Microsoft.NETCore.App")
                    .join(version)
                    .is_dir()
                && root
                    .join("shared/Microsoft.WindowsDesktop.App")
                    .join(version)
                    .is_dir()
        }
        None => true,
    }
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
    let (local_assembly_version, install_path) = local_install
        .map(|(v, p)| (Some(v), Some(p)))
        .unwrap_or((None, None));

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

    // status 只做只读检测：即使 release 已就绪，runtime/assets 缺失也先
    // 明确报告；真正启动时 `build_dalamud_config` 会自动下载补齐。
    let install_state = if matches!(install_state, InstallState::Ready | InstallState::Missing)
        && remote.as_ref().is_some_and(|r| r.runtime_required)
        && find_usable_runtime_dir(
            install_root,
            remote.as_ref().map(|r| r.runtime_version.as_str()),
        )
        .is_none()
    {
        warn!(
            runtime_version = %remote.as_ref().map(|r| r.runtime_version.as_str()).unwrap_or("?"),
            "Dalamud requires Windows .NET runtime but it is not installed"
        );
        InstallState::RuntimeMissing
    } else {
        install_state
    };

    // assets 同样由独立 updater 管理；目录缺失/为空时不能声称 Ready。
    let install_state = if matches!(install_state, InstallState::Ready | InstallState::Missing)
        && remote.is_some()
        && find_usable_asset_dir(install_root).is_none()
    {
        warn!("Dalamud assets are not installed");
        InstallState::AssetsMissing
    } else {
        install_state
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
        if Command::new("which")
            .arg(name)
            .output()
            .map(|o| o.status.success())
            .unwrap_or(false)
        {
            return Some(name.to_string());
        }
    }
    None
}

/// 校验 release 的 `hashes.json`。
///
/// 上游协议：`version_info.hash` 是 `hashes.json` 文件本身的 MD5（十六进制），
/// hashes.json 内是 release 各文件的相对路径 → MD5。文件可能为 UTF-16LE/UTF-16BE
/// （带 BOM），serde_json 只认 UTF-8，因此先做编码探测再解析。
fn verify_release_hashes(
    extract_dir: &Path,
    expected_manifest_hash: &str,
) -> Result<(), DalamudError> {
    if expected_manifest_hash.trim().is_empty() {
        return Err(DalamudError::Integrity(
            "release metadata has empty Hash".into(),
        ));
    }

    let manifest_path = extract_dir.join("hashes.json");
    let manifest_bytes = std::fs::read(&manifest_path).map_err(|e| DalamudError::Io {
        path: manifest_path.clone(),
        source: e,
    })?;

    let actual_manifest_hash = format!("{:x}", md5::compute(&manifest_bytes));
    if !actual_manifest_hash.eq_ignore_ascii_case(expected_manifest_hash) {
        return Err(DalamudError::Integrity(format!(
            "hashes.json MD5 mismatch: expected {expected_manifest_hash}, got {actual_manifest_hash}"
        )));
    }

    let hashes: std::collections::BTreeMap<String, String> =
        serde_json::from_str(&decode_manifest(&manifest_bytes)?)
            .map_err(|e| DalamudError::Integrity(format!("invalid hashes.json: {e}")))?;
    if hashes.is_empty() {
        return Err(DalamudError::Integrity("hashes.json is empty".into()));
    }

    for (rel, expected_hash) in hashes {
        let rel = rel.replace('\\', "/");
        let rel_path = std::path::Path::new(&rel);
        if rel_path.is_absolute()
            || rel_path.components().any(|c| {
                matches!(
                    c,
                    std::path::Component::ParentDir
                        | std::path::Component::RootDir
                        | std::path::Component::Prefix(_)
                )
            })
        {
            return Err(DalamudError::Integrity(format!(
                "unsafe path in hashes.json: {rel}"
            )));
        }
        let file_path = extract_dir.join(rel_path);
        let file_bytes = std::fs::read(&file_path).map_err(|e| DalamudError::Io {
            path: file_path.clone(),
            source: e,
        })?;
        let actual_hash = format!("{:x}", md5::compute(&file_bytes));
        if !actual_hash.eq_ignore_ascii_case(&expected_hash) {
            return Err(DalamudError::Integrity(format!(
                "file hash mismatch for {rel}: expected {expected_hash}, got {actual_hash}"
            )));
        }
    }

    Ok(())
}

/// 已安装 release 是否通过上游完整性校验。
pub(crate) fn release_install_is_valid(
    install_dir: &Path,
    expected_manifest_hash: &str,
) -> bool {
    for required in ["Dalamud.Injector.exe", "Dalamud.dll", "ImGuiScene.dll"] {
        if !install_dir.join(required).is_file() {
            warn!(file = required, "installed Dalamud release missing required file");
            return false;
        }
    }
    match verify_release_hashes(install_dir, expected_manifest_hash) {
        Ok(()) => true,
        Err(e) => {
            warn!(error = %e, "installed Dalamud release failed integrity check");
            false
        }
    }
}

/// 解码 UTF-16/UTF-8 JSON manifest（hashes.json / runtime hashes）。
pub(super) fn decode_manifest(bytes: &[u8]) -> Result<String, DalamudError> {
    if bytes.starts_with(&[0xFF, 0xFE]) && bytes.len() % 2 == 0 {
        let units: Vec<u16> = bytes[2..]
            .chunks_exact(2)
            .map(|c| u16::from_le_bytes([c[0], c[1]]))
            .collect();
        String::from_utf16(&units)
            .map_err(|e| DalamudError::Integrity(format!("invalid UTF-16LE hashes.json: {e}")))
    } else if bytes.starts_with(&[0xFE, 0xFF]) && bytes.len() % 2 == 0 {
        let units: Vec<u16> = bytes[2..]
            .chunks_exact(2)
            .map(|c| u16::from_be_bytes([c[0], c[1]]))
            .collect();
        String::from_utf16(&units)
            .map_err(|e| DalamudError::Integrity(format!("invalid UTF-16BE hashes.json: {e}")))
    } else {
        String::from_utf8(bytes.to_vec())
            .map_err(|e| DalamudError::Integrity(format!("invalid UTF-8 hashes.json: {e}")))
    }
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
        return Err(DalamudError::Parse(format!(
            "7z extraction failed ({status})"
        )));
    }
    info!("extracted Dalamud release");

    // 3. 校验：关键文件存在 + hashes.json 完整性（上游协议的最低线）
    for required in ["Dalamud.Injector.exe", "Dalamud.dll", "ImGuiScene.dll"] {
        if !extract_dir.join(required).is_file() {
            warn!(file = required, "release missing required file");
            return Err(DalamudError::Integrity(format!(
                "release missing required file: {required}"
            )));
        }
    }
    verify_release_hashes(&extract_dir, &version_info.hash)?;
    info!("release integrity verified");

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
    #[error("Dalamud release integrity check failed: {0}")]
    Integrity(String),
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
    fn test_verify_release_hashes_utf16le() {
        let dir = std::env::temp_dir().join(format!("xl-rs-hash-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();

        let file_bytes = b"fake injector";
        std::fs::write(dir.join("Dalamud.Injector.exe"), file_bytes).unwrap();
        let file_hash = format!("{:x}", md5::compute(file_bytes));
        let json = format!("{{\"Dalamud.Injector.exe\":\"{file_hash}\"}}");

        let mut utf16: Vec<u8> = vec![0xFF, 0xFE];
        for unit in json.encode_utf16() {
            utf16.extend_from_slice(&unit.to_le_bytes());
        }
        let manifest_hash = format!("{:x}", md5::compute(&utf16));
        std::fs::write(dir.join("hashes.json"), &utf16).unwrap();

        verify_release_hashes(&dir, &manifest_hash).unwrap();

        // 文件被篡改后必须拒绝
        std::fs::write(dir.join("Dalamud.Injector.exe"), b"evil").unwrap();
        assert!(matches!(
            verify_release_hashes(&dir, &manifest_hash),
            Err(DalamudError::Integrity(_))
        ));

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_find_usable_asset_dir_requires_version_dir() {
        let dir = std::env::temp_dir().join(format!("xl-rs-assets-{}", std::process::id()));
        let base = dir.join("dalamudAssets");
        std::fs::create_dir_all(base.join("115/UIRes")).unwrap();
        // 没有 asset.ver 时不可用
        assert_eq!(find_usable_asset_dir(&dir), None);
        std::fs::write(base.join("asset.ver"), "115").unwrap();
        assert_eq!(find_usable_asset_dir(&dir), Some(base.join("115")));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_find_usable_runtime_dir() {
        let dir = std::env::temp_dir().join(format!("xl-rs-rt-{}", std::process::id()));
        let rt = dir.join("runtime");
        std::fs::create_dir_all(rt.join("host/fxr/10.0.1")).unwrap();
        std::fs::write(rt.join("host/fxr/10.0.1/hostfxr.dll"), b"x").unwrap();
        std::fs::create_dir_all(rt.join("shared/Microsoft.NETCore.App/10.0.1")).unwrap();
        std::fs::create_dir_all(rt.join("shared/Microsoft.WindowsDesktop.App/10.0.1")).unwrap();

        assert_eq!(
            find_usable_runtime_dir(&dir, Some("10.0.1")),
            Some(rt.clone())
        );
        assert_eq!(find_usable_runtime_dir(&dir, Some("9.0.0")), None);
        assert_eq!(find_usable_runtime_dir(&dir, None), Some(rt));
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
