//! Wine 兼容层管理。
//!
//! 对应 C# 的 `CompatibilityTools` 和 `WineSettings`。
//! 负责自动检测、下载和管理 macOS/Linux 上运行 FFXIV 所需的 Wine。

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use tracing::{debug, error, info, warn};

/// Wine 管理工具。
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
    #[tracing::instrument]
    pub async fn ensure(custom_path: Option<&Path>) -> Result<Self, WineError> {
        if let Some(tool) = Self::detect(custom_path) {
            return Ok(tool);
        }

        // 需要下载
        Self::download_and_setup().await
    }

    /// 使用此 wine 工具运行游戏。
    ///
    /// `exe_path`: Windows 可执行文件路径（会被 wine 转换）
    /// `args`: 启动参数列表（每个参数独立传递）
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
            .env("WINEPREFIX", &self.prefix_path)
            .env("XL_WINEONLINUX", "true")
            .env("XL_WINEONMAC", "true");

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

#[derive(Debug, thiserror::Error)]
pub enum WineError {
    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),
    #[error("Download failed: {0}")]
    Download(String),
    #[error("Extraction failed: {0}")]
    Extract(String),
    #[error("Could not determine home directory")]
    NoHomeDir,
}