//! Wine 兼容层管理。
//!
//! 对应 C# 的 `CompatibilityTools` 和 `WineSettings`。
//! 负责自动检测、下载和管理 macOS/Linux 上运行 FFXIV 所需的 Wine。

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use tracing::{debug, error, info, warn};

use crate::settings::{WineSettings, WineStartupType};

/// Wine 管理工具。
#[derive(Debug)]
pub struct WineTool {
    /// wine64 可执行文件的完整路径。
    pub wine64_path: PathBuf,
    /// Wine prefix 目录。
    pub prefix_path: PathBuf,
    /// 是否为 XIVLauncher 托管的 wine。
    pub is_managed: bool,
}

impl WineTool {
    /// 检测系统中可用的 wine。
    ///
    /// 优先级：
    /// 1. 用户自定义路径（通过 `custom_path` 参数）
    /// 2. XIVLauncher 已下载的 wine（`~/.xlcore/beta/wine/bin/wine64`）
    /// 3. 系统 wine64（`PATH` 中的 `wine64`）
    #[tracing::instrument]
    pub fn detect(custom_path: Option<&Path>) -> Option<Self> {
        // 1. 自定义路径
        if let Some(path) = custom_path {
            if path.exists() {
                info!(?path, "Found wine at custom path");
                return Some(Self::from_wine64_path(path));
            }
            warn!(?path, "Custom wine path does not exist");
        }

        // 2. XIVLauncher 已下载的 wine
        if let Some(home) = dirs::home_dir() {
            let xlcore_wine = home.join(".xlcore/beta/wine/bin/wine64");
            if xlcore_wine.exists() {
                info!(?xlcore_wine, "Found wine at XIVLauncher managed path");
                return Some(Self::from_xlcore_path(&xlcore_wine));
            }
        }

        // 3. 系统 wine64
        if let Ok(output) = Command::new("which").arg("wine64").output() {
            if output.status.success() {
                let path_str = String::from_utf8_lossy(&output.stdout).trim().to_string();
                if !path_str.is_empty() {
                    let path = PathBuf::from(path_str);
                    info!(?path, "Found system wine64");
                    return Some(Self {
                        wine64_path: path,
                        prefix_path: Self::default_prefix_path(),
                        is_managed: false,
                    });
                }
            }
        }

        info!("Wine not found");
        None
    }

    /// 确保 wine 可用。如果没有检测到，自动下载。
    ///
    /// 返回检测/下载后的 WineTool。
    ///
    /// 等价于 `resolve(&WineSettings { startup_type: Auto, custom_path, .. })`，
    /// 保留以兼容旧调用方。
    #[tracing::instrument]
    pub async fn ensure(custom_path: Option<&Path>) -> Result<Self, WineError> {
        let mut settings = WineSettings::default();
        if let Some(p) = custom_path {
            settings.custom_path = Some(p.to_path_buf());
        }
        Self::resolve(&settings).await
    }

    /// 按配置解析出运行时对象（必要时触发下载）。
    ///
    /// 这是配置 → 运行时的唯一入口：
    /// - `Auto`：自定义路径 → XIVLauncher 托管 → 系统 wine → 下载
    /// - `Managed`：托管 wine（已有则复用，否则下载官方 wine-xiv）
    /// - `Custom`：用户路径（`wine64` 文件或 bin 目录，见 [`Self::normalize_wine64_path`]）
    /// - `System`：PATH 中的 `wine64`
    ///
    /// prefix 一律以 `settings.prefix` 为准（`None` 用默认 `~/.xiv-launcher-rs/prefix`）。
    #[tracing::instrument]
    pub async fn resolve(settings: &WineSettings) -> Result<Self, WineError> {
        match settings.startup_type {
            WineStartupType::Auto => {
                if let Some(tool) = Self::detect(settings.custom_path.as_deref()) {
                    return Ok(Self::apply_prefix(tool, settings));
                }
                let tool = Self::download_and_setup().await?;
                Ok(Self::apply_prefix(tool, settings))
            }
            WineStartupType::Managed => {
                // 1. XIVLauncher 已下载的 wine
                if let Some(home) = dirs::home_dir() {
                    let xlcore_wine = home.join(".xlcore/beta/wine/bin/wine64");
                    if xlcore_wine.exists() {
                        let tool = Self::from_xlcore_path(&xlcore_wine);
                        return Ok(Self::apply_prefix(tool, settings));
                    }
                }
                // 2. 本项目已下载的 wine
                let tools_dir = Self::tools_dir()?;
                let wine64 = tools_dir.join("wine/bin/wine64");
                if wine64.exists() {
                    let tool = Self {
                        wine64_path: wine64,
                        prefix_path: Self::default_prefix_path(),
                        is_managed: true,
                    };
                    return Ok(Self::apply_prefix(tool, settings));
                }
                // 3. 下载
                let tool = Self::download_and_setup().await?;
                Ok(Self::apply_prefix(tool, settings))
            }
            WineStartupType::Custom => {
                let path = settings
                    .custom_path
                    .as_ref()
                    .ok_or_else(|| {
                        WineError::NotFound(
                            "custom_path is required for WineStartupType::Custom".to_string(),
                        )
                    })?;
                let wine64 = Self::normalize_wine64_path(path)?;
                info!(?wine64, "using custom wine");
                let tool = Self {
                    wine64_path: wine64,
                    prefix_path: Self::default_prefix_path(),
                    is_managed: false,
                };
                Ok(Self::apply_prefix(tool, settings))
            }
            WineStartupType::System => {
                if let Ok(output) = Command::new("which").arg("wine64").output() {
                    if output.status.success() {
                        let path_str = String::from_utf8_lossy(&output.stdout).trim().to_string();
                        if !path_str.is_empty() {
                            let tool = Self {
                                wine64_path: PathBuf::from(path_str),
                                prefix_path: Self::default_prefix_path(),
                                is_managed: false,
                            };
                            return Ok(Self::apply_prefix(tool, settings));
                        }
                    }
                }
                Err(WineError::NotFound(
                    "system wine64 not found in PATH".to_string(),
                ))
            }
        }
    }

    /// 校验自定义路径：接受 `wine64` 可执行文件，或含 `wine64`/`bin/wine64` 的目录。
    fn normalize_wine64_path(path: &Path) -> Result<PathBuf, WineError> {
        if path.is_file() {
            return Ok(path.to_path_buf());
        }
        if path.is_dir() {
            for candidate in [path.join("wine64"), path.join("bin/wine64")] {
                if candidate.is_file() {
                    return Ok(candidate);
                }
            }
            return Err(WineError::NotFound(format!(
                "no wine64 found under directory {:?}",
                path
            )));
        }
        Err(WineError::NotFound(format!(
            "custom wine path does not exist: {:?}",
            path
        )))
    }

    fn apply_prefix(tool: Self, settings: &WineSettings) -> Self {
        if let Some(p) = &settings.prefix {
            Self {
                prefix_path: p.clone(),
                ..tool
            }
        } else {
            tool
        }
    }

    /// 运行 `wine64 --version` 校验可执行性，返回版本字符串。
    ///
    /// 在解析后调用，可提前暴露「不可执行 / 指向错误构建」等问题。
    #[tracing::instrument(skip(self))]
    pub fn probe(&self) -> Result<String, WineError> {
        let output = Command::new(&self.wine64_path)
            .arg("--version")
            .output()
            .map_err(WineError::Io)?;
        if output.status.success() {
            let version = String::from_utf8_lossy(&output.stdout).trim().to_string();
            info!(?version, "wine probe ok");
            Ok(version)
        } else {
            Err(WineError::Probe(format!(
                "wine64 --version failed with status {}",
                output.status
            )))
        }
    }

    /// 使用此 wine 工具运行游戏。
    ///
    /// `exe_path`: Windows 可执行文件路径（会被 wine 转换）
    /// `args`: 启动参数列表（每个参数独立传递）
    /// `env`: 附加环境变量（由 [`build_launch_env`] 生成），后应用，可覆盖默认项。
    #[tracing::instrument(skip(self, args, env))]
    pub fn run(
        &self,
        exe_path: &std::path::Path,
        args: &[String],
        working_dir: &Path,
        env: &[(String, String)],
    ) -> Result<std::process::Child, std::io::Error> {
        info!(wine64_path = ?self.wine64_path, ?exe_path, "Starting game with wine");
        let mut cmd = Command::new(&self.wine64_path);
        cmd.arg(exe_path)
            .args(args)
            .current_dir(working_dir)
            // 默认环境（env 参数后应用，可覆盖）
            .env("WINEPREFIX", &self.prefix_path)
            .env("XL_WINEONLINUX", "true");
        #[cfg(target_os = "macos")]
        cmd.env("XL_WINEONMAC", "true");

        for (k, v) in env {
            cmd.env(k, v);
        }

        cmd.spawn()
    }

    /// 下载并设置 wine。
    async fn download_and_setup() -> Result<Self, WineError> {
        let tools_dir = Self::tools_dir()?;
        let wine_dir = tools_dir.join("wine");
        let bin_dir = wine_dir.join("bin");
        let wine64_path = bin_dir.join("wine64");

        if wine64_path.exists() {
            return Ok(Self {
                wine64_path,
                prefix_path: Self::default_prefix_path(),
                is_managed: true,
            });
        }

        info!("Wine not found, downloading...");

        // macOS 专用 wine 下载地址
        #[cfg(target_os = "macos")]
        const WINE_URL: &str =
            "https://s3.ffxiv.wang/xlcore/deps/wine/osx/xom-4.17.1/wine.tar.gz";
        #[cfg(not(target_os = "macos"))]
        const WINE_URL: &str =
            "https://s3.ffxiv.wang/xlcore/deps/wine/ubuntu/wine-xiv-staging-fsync-git-ubuntu-8.5.r4.g4211bac7.tar.xz";

        let client = reqwest::Client::new();
        let response = client.get(WINE_URL).send().await.map_err(|e| {
            error!(error = %e, "Failed to download wine");
            WineError::Download(e.to_string())
        })?;

        if !response.status().is_success() {
            error!(status = %response.status(), "Wine download returned non-success status");
            return Err(WineError::Download(format!("HTTP {}", response.status())));
        }

        let bytes = response.bytes().await.map_err(|e| {
            error!(error = %e, "Failed to read wine download response");
            WineError::Download(e.to_string())
        })?;

        info!(bytes = bytes.len(), "Downloaded wine archive, extracting...");

        // 创建目录
        std::fs::create_dir_all(&bin_dir).map_err(WineError::Io)?;

        // 解压
        let tar_path = tools_dir.join("wine_download.tar.gz");
        std::fs::write(&tar_path, &bytes).map_err(WineError::Io)?;

        // 使用 tar 命令解压（自动检测压缩格式）
        let status = Command::new("tar")
            .args(&["-xf", tar_path.to_str().unwrap(), "-C", tools_dir.to_str().unwrap()])
            .stdout(Stdio::inherit())
            .stderr(Stdio::inherit())
            .status()
            .map_err(WineError::Io)?;

        if !status.success() {
            error!("Wine extraction failed: tar command returned non-zero exit code");
            return Err(WineError::Extract("tar command failed".to_string()));
        }

        // 清理下载的压缩包
        let _ = std::fs::remove_file(&tar_path);

        if !wine64_path.exists() {
            error!(?wine64_path, "wine64 not found after extraction");
            return Err(WineError::Extract(
                format!("wine64 not found after extraction at {:?}", wine64_path)
            ));
        }

        info!(?wine64_path, "Wine setup complete");

        Ok(Self {
            wine64_path,
            prefix_path: Self::default_prefix_path(),
            is_managed: true,
        })
    }

    /// 确保 DXVK 已安装到 wine prefix。
    #[tracing::instrument(skip(self))]
    pub async fn ensure_dxvk(&self) -> Result<(), WineError> {
        let prefix = &self.prefix_path;
        let system32 = prefix.join("drive_c/windows/system32");
        let syswow64 = prefix.join("drive_c/windows/syswow64");

        // 检查是否已安装
        if system32.join("d3d11.dll").exists() {
            info!("DXVK already installed in prefix");
            return Ok(());
        }

        info!("DXVK not found, downloading...");

        #[cfg(target_os = "macos")]
        const DXVK_URL: &str = "https://s3.ffxiv.wang/xlcore/deps/dxvk/osx/dxvk-macOS-async-v1.10.3-20230507-repack.tar.gz";
        #[cfg(not(target_os = "macos"))]
        const DXVK_URL: &str = "https://s3.ffxiv.wang/xlcore/deps/dxvk/linux/dxvk-async-1.10.1.tar.gz";

        let client = reqwest::Client::new();
        let response = client.get(DXVK_URL).send().await.map_err(|e| {
            error!(error = %e, "Failed to download DXVK");
            WineError::Download(e.to_string())
        })?;

        if !response.status().is_success() {
            error!(status = %response.status(), "DXVK download returned non-success status");
            return Err(WineError::Download(format!("HTTP {}", response.status())));
        }

        let bytes = response.bytes().await.map_err(|e| {
            error!(error = %e, "Failed to read DXVK download response");
            WineError::Download(e.to_string())
        })?;
        info!(bytes = bytes.len(), "Downloaded DXVK archive, extracting...");

        let tools_dir = Self::tools_dir()?;
        let dxvk_tar = tools_dir.join("dxvk_download.tar.gz");
        std::fs::write(&dxvk_tar, &bytes).map_err(WineError::Io)?;

        let dxvk_dir = tools_dir.join("dxvk");
        std::fs::create_dir_all(&dxvk_dir).map_err(WineError::Io)?;

        let status = Command::new("tar")
            .args(&["-xf", dxvk_tar.to_str().unwrap(), "-C", dxvk_dir.to_str().unwrap()])
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .map_err(WineError::Io)?;

        if !status.success() {
            error!("DXVK extraction failed: tar command returned non-zero exit code");
            return Err(WineError::Extract("tar dxvk failed".to_string()));
        }

        let _ = std::fs::remove_file(&dxvk_tar);

        // 安装 dxvk dll 到 prefix
        info!("Installing DXVK to wine prefix...");
        Self::install_dxvk_dlls(&dxvk_dir, &system32, &syswow64)?;

        info!("DXVK installed successfully");
        Ok(())
    }

    #[tracing::instrument]
    fn install_dxvk_dlls(dxvk_dir: &Path, system32: &Path, syswow64: &Path) -> Result<(), WineError> {
        // DXVK 目录结构可能是 dxvk/x64 和 dxvk/x32
        // 或者解压后多了一层子目录
        let x64_dir = if dxvk_dir.join("x64").exists() {
            dxvk_dir.join("x64")
        } else {
            // 查找子目录中的 x64
            std::fs::read_dir(dxvk_dir)
                .map_err(WineError::Io)?
                .filter_map(|e| e.ok())
                .map(|e| e.path())
                .find(|p| p.is_dir() && p.join("x64").exists())
                .map(|p| p.join("x64"))
                .unwrap_or_else(|| dxvk_dir.join("x64"))
        };
        let x32_dir = x64_dir.parent().unwrap_or(dxvk_dir).join("x32");

        debug!(?x64_dir, ?x32_dir, "Installing DXVK DLLs");

        std::fs::create_dir_all(system32).map_err(WineError::Io)?;
        std::fs::create_dir_all(syswow64).map_err(WineError::Io)?;

        // 复制 64-bit DLLs 到 system32
        if x64_dir.exists() {
            for entry in std::fs::read_dir(&x64_dir).map_err(WineError::Io)? {
                let entry = entry.map_err(WineError::Io)?;
                let path = entry.path();
                if path.extension().map_or(false, |e| e == "dll") {
                    let dest = system32.join(path.file_name().unwrap());
                    debug!(?path, ?dest, "Copying 64-bit DXVK DLL");
                    std::fs::copy(&path, &dest).map_err(WineError::Io)?;
                }
            }
        }

        // 复制 32-bit DLLs 到 syswow64
        if x32_dir.exists() {
            for entry in std::fs::read_dir(&x32_dir).map_err(WineError::Io)? {
                let entry = entry.map_err(WineError::Io)?;
                let path = entry.path();
                if path.extension().map_or(false, |e| e == "dll") {
                    let dest = syswow64.join(path.file_name().unwrap());
                    debug!(?path, ?dest, "Copying 32-bit DXVK DLL");
                    std::fs::copy(&path, &dest).map_err(WineError::Io)?;
                }
            }
        }

        Ok(())
    }

    fn from_wine64_path(path: &Path) -> Self {
        Self {
            wine64_path: path.to_path_buf(),
            prefix_path: Self::default_prefix_path(),
            is_managed: false,
        }
    }

    fn from_xlcore_path(path: &Path) -> Self {
        Self {
            wine64_path: path.to_path_buf(),
            prefix_path: Self::default_prefix_path(),
            is_managed: true,
        }
    }

    fn tools_dir() -> Result<PathBuf, WineError> {
        let home = dirs::home_dir().ok_or(WineError::NoHomeDir)?;
        let dir = home.join(".xiv-launcher-rs/tools");
        std::fs::create_dir_all(&dir).map_err(WineError::Io)?;
        Ok(dir)
    }

    fn default_prefix_path() -> PathBuf {
        dirs::home_dir()
            .map(|h| h.join(".xiv-launcher-rs/prefix"))
            .unwrap_or_else(|| PathBuf::from("./prefix"))
    }
}

/// 组装启动 Wine 时的环境变量（纯函数，便于测试）。
///
/// 对齐 C# `CompatibilityTools.RunInPrefix` 的环境变量清单。
/// `WINEPREFIX` 与 `XL_WINEONLINUX`/`XL_WINEONMAC` 由 [`WineTool::run`] 兜底设置，
/// 此处也一并生成 `WINEPREFIX`，保证函数输出自洽、可单测。
pub fn build_launch_env(settings: &WineSettings, tool: &WineTool) -> Vec<(String, String)> {
    let mut env: Vec<(String, String)> = Vec::new();

    // prefix
    env.push((
        "WINEPREFIX".to_string(),
        tool.prefix_path.display().to_string(),
    ));

    // DLL overrides：DXVK 开启时 d3d* = n，否则回退 wined3d = b
    let d3d = if settings.dxvk.enabled { "n" } else { "b" };
    #[cfg(target_os = "macos")]
    let overrides = format!("msquic=,mscoree=n,b;d3d11={d3d};dxgi=n,b");
    #[cfg(not(target_os = "macos"))]
    let overrides = format!("msquic=,mscoree=n,b;d3d9,d3d11,d3d10core,dxgi={d3d}");
    env.push(("WINEDLLOVERRIDES".to_string(), overrides));

    // 同步机制
    if settings.esync {
        env.push(("WINEESYNC".to_string(), "1".to_string()));
    }
    if settings.fsync {
        env.push(("WINEFSYNC".to_string(), "1".to_string()));
    }
    #[cfg(target_os = "macos")]
    if settings.msync {
        env.push(("WINEMSYNC".to_string(), "1".to_string()));
    }

    // WINEDEBUG
    if let Some(v) = settings.debug_vars.as_deref() {
        if !v.is_empty() {
            env.push(("WINEDEBUG".to_string(), v.to_string()));
        }
    }

    // DXVK
    if settings.dxvk.enabled {
        env.push(("DXVK_STATE_CACHE_PATH".to_string(), "C:\\".to_string()));
        env.push(("DXVK_CONFIG_FILE".to_string(), "C:\\ffxiv_dx11.conf".to_string()));
        if let Some(hud) = &settings.dxvk.hud {
            env.push(("DXVK_HUD".to_string(), hud.clone()));
        }
        if let Some(limit) = settings.dxvk.frame_limit {
            env.push(("DXVK_FRAME_RATE".to_string(), limit.to_string()));
        }
    }

    // gamemode
    if settings.gamemode {
        let existing = std::env::var("LD_PRELOAD").unwrap_or_default();
        let merged = if existing.is_empty() {
            "libgamemodeauto.so.0".to_string()
        } else if existing.contains("libgamemodeauto.so.0") {
            existing
        } else {
            format!("{existing}:libgamemodeauto.so.0")
        };
        env.push(("LD_PRELOAD".to_string(), merged));
    }

    // 自定义环境变量（最后应用，可覆盖以上所有）
    for (k, v) in &settings.env {
        env.push((k.clone(), v.clone()));
    }

    env
}

#[derive(Debug, thiserror::Error)]
pub enum WineError {
    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),
    #[error("Download failed: {0}")]
    Download(String),
    #[error("Extraction failed: {0}")]
    Extract(String),
    #[error("Wine binary not found: {0}")]
    NotFound(String),
    #[error("Wine probe failed: {0}")]
    Probe(String),
    #[error("Could not determine home directory")]
    NoHomeDir,
}
#[cfg(test)]
mod tests {
    use super::*;

    fn tool() -> WineTool {
        WineTool {
            wine64_path: PathBuf::from("/fake/wine64"),
            prefix_path: PathBuf::from("/fake/prefix"),
            is_managed: false,
        }
    }

    fn env_map(env: &[(String, String)]) -> std::collections::HashMap<String, String> {
        env.iter().cloned().collect()
    }

    #[test]
    fn test_normalize_wine64_path_file() {
        let dir = std::env::temp_dir().join(format!("xlrs-norm-file-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let f = dir.join("wine64");
        std::fs::write(&f, "#!/bin/sh\n").unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&f, std::fs::Permissions::from_mode(0o755)).unwrap();
        }
        assert_eq!(WineTool::normalize_wine64_path(&f).unwrap(), f);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_normalize_wine64_path_dir() {
        let dir = std::env::temp_dir().join(format!("xlrs-norm-dir-{}", std::process::id()));
        let bin = dir.join("bin");
        std::fs::create_dir_all(&bin).unwrap();
        let f = bin.join("wine64");
        std::fs::write(&f, "#!/bin/sh\n").unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&f, std::fs::Permissions::from_mode(0o755)).unwrap();
        }
        assert_eq!(WineTool::normalize_wine64_path(&dir).unwrap(), f);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_normalize_wine64_path_missing() {
        let missing = PathBuf::from("/nonexistent/xlrs-wine");
        assert!(matches!(
            WineTool::normalize_wine64_path(&missing),
            Err(WineError::NotFound(_))
        ));
    }

    #[test]
    fn test_build_launch_env_default() {
        let settings = WineSettings::default();
        let env = env_map(&build_launch_env(&settings, &tool()));

        assert_eq!(env["WINEPREFIX"], "/fake/prefix");
        assert_eq!(env["WINEDLLOVERRIDES"], "msquic=,mscoree=n,b;d3d9,d3d11,d3d10core,dxgi=n");
        assert_eq!(env["DXVK_STATE_CACHE_PATH"], "C:\\");
        assert_eq!(env["DXVK_CONFIG_FILE"], "C:\\ffxiv_dx11.conf");
        // 默认未开启的项不应出现
        assert!(!env.contains_key("WINEESYNC"));
        assert!(!env.contains_key("WINEFSYNC"));
        assert!(!env.contains_key("WINEDEBUG"));
        assert!(!env.contains_key("DXVK_HUD"));
        assert!(!env.contains_key("LD_PRELOAD"));
    }

    #[test]
    fn test_build_launch_env_toggles() {
        let settings = WineSettings {
            esync: true,
            fsync: true,
            debug_vars: Some("+seh".to_string()),
            dxvk: crate::settings::DxvkSettings {
                enabled: false,
                hud: None,
                frame_limit: None,
            },
            env: std::collections::BTreeMap::from([
                ("MY_VAR".to_string(), "1".to_string()),
                ("WINEPREFIX".to_string(), "/override".to_string()),
            ]),
            gamemode: true,
            ..Default::default()
        };
        let env = env_map(&build_launch_env(&settings, &tool()));

        assert_eq!(env["WINEESYNC"], "1");
        assert_eq!(env["WINEFSYNC"], "1");
        assert_eq!(env["WINEDEBUG"], "+seh");
        // DXVK 关闭 → wined3d = b，且无 DXVK_* 变量
        assert_eq!(env["WINEDLLOVERRIDES"], "msquic=,mscoree=n,b;d3d9,d3d11,d3d10core,dxgi=b");
        assert!(!env.contains_key("DXVK_HUD"));
        // 自定义 env 可覆盖 WINEPREFIX（最后应用）
        assert_eq!(env["WINEPREFIX"], "/override");
        assert_eq!(env["MY_VAR"], "1");
        assert!(env["LD_PRELOAD"].contains("libgamemodeauto.so.0"));
    }

    #[test]
    fn test_build_launch_env_dxvk_hud_frame_limit() {
        let settings = WineSettings {
            dxvk: crate::settings::DxvkSettings {
                enabled: true,
                hud: Some("fps".to_string()),
                frame_limit: Some(120),
            },
            ..Default::default()
        };
        let env = env_map(&build_launch_env(&settings, &tool()));
        assert_eq!(env["DXVK_HUD"], "fps");
        assert_eq!(env["DXVK_FRAME_RATE"], "120");
    }

    /// 集成测试：用假 wine64 脚本验证 resolve(Custom) + run() 真的把环境变量传给了子进程。
    #[cfg(unix)]
    #[test]
    fn test_run_fake_wine_passes_env() {
        use std::os::unix::fs::PermissionsExt;

        let dir = std::env::temp_dir().join(format!("xlrs-fakewine-{}", std::process::id()));
        let bin = dir.join("bin");
        std::fs::create_dir_all(&bin).unwrap();
        let out_file = dir.join("env.txt");

        let script = format!(
            "#!/bin/sh\nprintf 'PREFIX=%s\\nOVERRIDES=%s\\nESYNC=%s\\nFSYNC=%s\\nDEBUG=%s\\n' \\\n  \"$WINEPREFIX\" \"$WINEDLLOVERRIDES\" \"$WINEESYNC\" \"$WINEFSYNC\" \"$WINEDEBUG\" > \"{}\"\n",
            out_file.display()
        );
        let wine64 = bin.join("wine64");
        std::fs::write(&wine64, script).unwrap();
        std::fs::set_permissions(&wine64, std::fs::Permissions::from_mode(0o755)).unwrap();

        let settings = WineSettings {
            startup_type: WineStartupType::Custom,
            custom_path: Some(bin.clone()),
            prefix: Some(dir.join("myprefix")),
            esync: true,
            fsync: true,
            debug_vars: Some("+all".to_string()),
            ..Default::default()
        };

        let tokio_rt = tokio::runtime::Runtime::new().unwrap();
        let tool = tokio_rt.block_on(WineTool::resolve(&settings)).unwrap();
        assert_eq!(tool.wine64_path, wine64);
        assert_eq!(tool.prefix_path, dir.join("myprefix"));
        assert!(!tool.is_managed);

        // probe 应输出假版本
        let version = tool.probe().unwrap_or_default();
        assert!(
            version.is_empty() || version.contains("wine"),
            "probe output: {version}"
        );

        // run + build_launch_env
        let env = build_launch_env(&settings, &tool);
        let child = tool
            .run(&PathBuf::from("game.exe"), &[], &dir, &env)
            .expect("spawn fake wine");
        let status = child.wait_with_output().unwrap();
        assert!(status.status.success());

        let content = std::fs::read_to_string(&out_file).unwrap();
        assert!(content.contains(&format!("PREFIX={}", dir.join("myprefix").display())));
        assert!(content.contains("OVERRIDES=msquic=,mscoree=n,b;d3d9,d3d11,d3d10core,dxgi=n"));
        assert!(content.contains("ESYNC=1"));
        assert!(content.contains("FSYNC=1"));
        assert!(content.contains("DEBUG=+all"));

        let _ = std::fs::remove_dir_all(&dir);
    }
}
