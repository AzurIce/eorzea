//! 游戏文件管理：版本检查、补丁下载、完整性校验。
//!
//! 对应 C# `SdoLauncher.CheckGameUpdate()` + `PatchManager`。
//! 与认证（`xiv-launcher-auth`）解耦——**全部免登录**：
//! 只需大区信息（补丁服务器地址）和游戏目录路径。
//!
//! # 已实现
//!
//! - `status` — 读取本地各 repo 版本（`version.rs`）
//! - `check` — 版本报告 → 补丁服务器 → 待下载补丁列表（免登录）
//! - `update` — 并发下载补丁 + SHA1 校验 + **ZiPatch 应用** + `.ver` 更新（`patch_manager.rs` + `zpatch/`）
//! - `install` — 按顺序应用已下载补丁，写 `.ver` 并备份 `.bck`（对应 C# `RemotePatchInstaller`）
//!
//! # 尚未实现
//!
//! - 完整性校验（对应 C# `PatchVerifier`）——需要 IndexedZiPatch 索引或逐文件 hash
//! - 断点续传（Range）——目前不完整文件整体重下
//! - UID 缓存（`X-Patch-Unique-Id`）——服务器目前返回空值，优先级低

pub mod patch_list;
pub mod patch_manager;
pub mod verify;
pub mod version;
pub mod zpatch;

use std::path::Path;
use tracing::{debug, info, instrument, warn};
use xiv_launcher_auth::{PatchListEntry, SdoArea};

use self::patch_manager::{
    download_patches, patch_cache_path, verify_patch_sha1, DownloadSummary, PatchDownloadError,
};
use self::version::{build_version_report, read_local_versions, LocalVersions};
use self::zpatch::ZiPatchError;

/// SDO 补丁检查 User-Agent（C# `Constants.PatcherUserAgent`，国服硬编码）。
const PATCHER_USER_AGENT: &str = "FFXIV_Patch";

/// 补丁所属仓库（对应 C# `Repository` 枚举，由 URL 判定）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Repo {
    Boot,
    Ffxiv,
    Ex1,
    Ex2,
    Ex3,
    Ex4,
    Ex5,
}

/// 从补丁 URL 判定所属仓库（对应 C# `PatchListEntry.GetRepo()`）。
pub fn repo_from_url(url: &str) -> Repo {
    if url.contains("boot") {
        Repo::Boot
    } else if url.contains("ex1") {
        Repo::Ex1
    } else if url.contains("ex2") {
        Repo::Ex2
    } else if url.contains("ex3") {
        Repo::Ex3
    } else if url.contains("ex4") {
        Repo::Ex4
    } else if url.contains("ex5") {
        Repo::Ex5
    } else {
        Repo::Ffxiv
    }
}

impl Repo {
    /// 补丁应用的目标子目录（`boot` 或 `game`）。
    fn install_subdir(self) -> &'static str {
        match self {
            Repo::Boot => "boot",
            _ => "game",
        }
    }

    /// 版本文件相对路径（对应 C# `GetVerFile()`）。
    fn ver_path(self, game_root: &Path) -> std::path::PathBuf {
        match self {
            Repo::Boot => game_root.join("boot/ffxivboot.ver"),
            Repo::Ffxiv => game_root.join("game/ffxivgame.ver"),
            Repo::Ex1 => game_root.join("game/sqpack/ex1/ex1.ver"),
            Repo::Ex2 => game_root.join("game/sqpack/ex2/ex2.ver"),
            Repo::Ex3 => game_root.join("game/sqpack/ex3/ex3.ver"),
            Repo::Ex4 => game_root.join("game/sqpack/ex4/ex4.ver"),
            Repo::Ex5 => game_root.join("game/sqpack/ex5/ex5.ver"),
        }
    }

    /// bck 版本文件路径（对应 C# `GetVerFile(isBck: true)`）。
    fn bck_path(self, game_root: &Path) -> std::path::PathBuf {
        match self {
            Repo::Boot => game_root.join("boot/ffxivboot.bck"),
            Repo::Ffxiv => game_root.join("game/ffxivgame.bck"),
            Repo::Ex1 => game_root.join("game/sqpack/ex1/ex1.bck"),
            Repo::Ex2 => game_root.join("game/sqpack/ex2/ex2.bck"),
            Repo::Ex3 => game_root.join("game/sqpack/ex3/ex3.bck"),
            Repo::Ex4 => game_root.join("game/sqpack/ex4/ex4.bck"),
            Repo::Ex5 => game_root.join("game/sqpack/ex5/ex5.bck"),
        }
    }
}

/// 安装总结。
#[derive(Debug)]
pub struct InstallSummary {
    pub installed: Vec<String>,
    pub skipped: usize,
}

/// 版本检查结果。
#[derive(Debug)]
pub enum CheckResult {
    /// 无需更新，游戏已是最新。
    UpToDate {
        /// 服务器下发的补丁会话 UID（`X-Patch-Unique-Id`）。
        unique_id: String,
    },
    /// 需要下载补丁。
    NeedsPatch {
        /// 待下载的补丁列表（按顺序应用）。
        patches: Vec<PatchListEntry>,
        /// 补丁会话 UID（`X-Patch-Unique-Id`）。
        unique_id: String,
    },
    /// boot 需要更新（服务器返回 409）。国服理论上不会出现（boot 检查被跳过）。
    NeedsPatchBoot,
}

/// 游戏文件管理器。
pub struct GameFileManager {
    client: reqwest::Client,
}

impl Default for GameFileManager {
    fn default() -> Self {
        Self::new()
    }
}

impl GameFileManager {
    /// 创建管理器（内部共享一个 HTTP client）。
    pub fn new() -> Self {
        Self {
            client: reqwest::Client::builder()
                .user_agent(PATCHER_USER_AGENT)
                .build()
                .expect("failed to build HTTP client"),
        }
    }

    /// 读取游戏目录本地版本。
    pub fn status(&self, game_root: &Path) -> LocalVersions {
        read_local_versions(game_root)
    }

    /// 检查游戏更新（免登录）。
    ///
    /// 对应 C# `SdoLauncher.CheckGameUpdate()`：
    /// POST `http://{area.area_patch}/http/win32/shanda_release_chs_game/{version}`
    /// 请求体为版本报告，响应头 `X-Patch-Unique-Id` + TSV 补丁列表。
    ///
    /// - `force_base_version`：报告基础版本（全新安装/修复，对应 C# `Repair`）
    /// - `max_expansion`：版本报告包含的资料片数量（默认 5，对应 C# `Constants.MaxExpansion`）
    #[instrument(skip(self, area))]
    pub async fn check_update(
        &self,
        area: &SdoArea,
        game_root: &Path,
        force_base_version: bool,
        max_expansion: i32,
    ) -> Result<CheckResult, GameFileError> {
        // 版本号：force_base_version → 基础版本，否则读 game/ffxivgame.ver
        let ffxiv_ver = if force_base_version {
            version::BASE_GAME_VERSION.to_string()
        } else {
            version::read_ver(game_root, version::repo::FFXIV, version::ver_file::FFXIV)
        };

        let url = format!(
            "http://{}/http/win32/shanda_release_chs_game/{}",
            area.area_patch, ffxiv_ver
        );

        let report = build_version_report(game_root, max_expansion, force_base_version);
        debug!(url = %url, "checking game update");

        let response = self
            .client
            .post(&url)
            .header("X-Hash-Check", "enabled")
            .body(report)
            .send()
            .await
            .map_err(|e| GameFileError::Request {
                url: url.clone(),
                source: e,
            })?;

        let status = response.status();

        // 409 → boot 需要更新
        if status == reqwest::StatusCode::CONFLICT {
            warn!("server returned 409, boot needs update");
            return Ok(CheckResult::NeedsPatchBoot);
        }

        if !status.is_success() {
            return Err(GameFileError::Http {
                url: url.clone(),
                status,
            });
        }

        // 补丁会话 UID
        let unique_id = response
            .headers()
            .get("X-Patch-Unique-Id")
            .and_then(|v| v.to_str().ok())
            .map(|s| s.to_string())
            .ok_or_else(|| GameFileError::MissingUniqueId { url: url.clone() })?;

        let text = response
            .text()
            .await
            .map_err(|e| GameFileError::Body {
                url: url.clone(),
                source: e,
            })?;

        // 空响应体 → 已是最新
        if text.trim().is_empty() {
            debug!("empty patch list, game is up to date");
            return Ok(CheckResult::UpToDate { unique_id });
        }

        // 解析 TSV 补丁列表
        let patches = patch_list::parse_patch_list(&text)
            .map_err(|e| GameFileError::Parse { source: e })?;

        debug!(count = patches.len(), "game patching is needed");
        Ok(CheckResult::NeedsPatch { patches, unique_id })
    }

    /// 下载补丁到暂存目录（不应用）。
    ///
    /// `patches` 来自 [`Self::check_update`]。已下载且校验通过的自动跳过。
    pub async fn download(
        &self,
        patches: &[PatchListEntry],
        patch_dir: &Path,
        concurrency: usize,
        mut on_progress: impl FnMut(u64, u64),
    ) -> Result<DownloadSummary, GameFileError> {
        download_patches(&self.client, patches, patch_dir, concurrency, &mut on_progress)
            .await
            .map_err(GameFileError::Download)
    }

    /// 按顺序应用已下载的补丁到游戏目录。
    ///
    /// 对应 C# `RemotePatchInstaller` 的安装流程：
    /// 1. 根据 URL 判定仓库（boot/ffxiv/exN）
    /// 2. 将补丁应用到 `game_root/{boot|game}` 子目录（ZiPatch 解析 + 逐 chunk 应用）
    /// 3. 应用成功后写 `.ver` 文件（对应 `Repository.SetVer`）
    /// 4. 全部完成后将 `.ver` 备份为 `.bck`（对应 `VerToBck`）
    ///
    /// 返回 `(已安装版本列表, 补丁文件是否存在)`。
    #[instrument(skip(self, patches, game_root))]
    pub async fn install(
        &self,
        patches: &[PatchListEntry],
        patch_dir: &Path,
        game_root: &Path,
    ) -> Result<InstallSummary, GameFileError> {
        let mut installed = Vec::new();
        let mut skipped = 0;

        for patch in patches {
            let repo = repo_from_url(&patch.url);
            let patch_file = patch_cache_path(patch_dir, patch);

            if !patch_file.exists() {
                warn!(
                    file = %patch_file.display(),
                    "patch file missing, skipping (run update to download first)"
                );
                skipped += 1;
                continue;
            }

            if !verify_patch_sha1(&patch_file, patch)? {
                return Err(GameFileError::PatchIdentityMismatch {
                    path: patch_file,
                    url: patch.url.clone(),
                });
            }

            let base_dir = game_root.join(repo.install_subdir());
            info!(
                repo = ?repo,
                version = %patch.version,
                file = %patch_file.display(),
                "installing patch"
            );

            zpatch::apply::apply_patch_file(&patch_file, &base_dir).map_err(GameFileError::from)?;

            // 写版本文件（对应 `Repository.SetVer`）
            let ver_path = repo.ver_path(game_root);
            if let Some(parent) = ver_path.parent() {
                std::fs::create_dir_all(parent).map_err(|e| GameFileError::Io {
                    path: parent.to_path_buf(),
                    source: e,
                })?;
            }
            std::fs::write(&ver_path, &patch.version).map_err(|e| GameFileError::Io {
                path: ver_path.clone(),
                source: e,
            })?;
            debug!(path = %ver_path.display(), version = %patch.version, "wrote version file");

            installed.push(patch.version.clone());
        }

        // 全部完成后备份版本文件（对应 `VerToBck`）
        if !installed.is_empty() {
            ver_to_bck(game_root);
        }

        Ok(InstallSummary { installed, skipped })
    }
}

/// 将当前 `.ver` 文件备份为 `.bck`（对应 C# `RemotePatchInstaller.VerToBck`）。
pub fn ver_to_bck(game_root: &Path) {
    for repo in [
        Repo::Boot,
        Repo::Ffxiv,
        Repo::Ex1,
        Repo::Ex2,
        Repo::Ex3,
        Repo::Ex4,
        Repo::Ex5,
    ] {
        let ver = repo.ver_path(game_root);
        if !ver.exists() {
            continue;
        }
        let bck = repo.bck_path(game_root);
        match std::fs::copy(&ver, &bck) {
            Ok(_) => debug!(from = %ver.display(), to = %bck.display(), "backed up ver file"),
            Err(e) => warn!(from = %ver.display(), error = %e, "could not backup ver file"),
        }
    }
}

/// 游戏文件管理错误。
#[derive(Debug, thiserror::Error)]
pub enum GameFileError {
    #[error("HTTP request failed for {url}: {source}")]
    Request {
        url: String,
        #[source]
        source: reqwest::Error,
    },

    #[error("HTTP {status} for {url}")]
    Http {
        url: String,
        status: reqwest::StatusCode,
    },

    #[error("response has no X-Patch-Unique-Id header ({url})")]
    MissingUniqueId { url: String },

    #[error("failed reading response body for {url}: {source}")]
    Body {
        url: String,
        #[source]
        source: reqwest::Error,
    },

    #[error("failed to parse patch list: {source}")]
    Parse {
        #[source]
        source: patch_list::PatchListParseError,
    },

    #[error("download failed: {0}")]
    Download(#[from] PatchDownloadError),

    #[error("ZiPatch install failed: {0}")]
    ZPatch(#[from] ZiPatchError),

    #[error("cached patch does not match {url}: {path}")]
    PatchIdentityMismatch {
        path: std::path::PathBuf,
        url: String,
    },

    #[error("IO error at {path}: {source}")]
    Io {
        path: std::path::PathBuf,
        #[source]
        source: std::io::Error,
    },
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn install_rejects_mismatched_cached_patch_without_advancing_version() {
        let root = std::env::temp_dir().join(format!(
            "xl-rs-install-identity-{}",
            std::process::id()
        ));
        let patch_dir = root.join("patches");
        let game_root = root.join("game-root");
        std::fs::create_dir_all(&patch_dir).unwrap();

        let patch = PatchListEntry {
            version: "2026.07.16.0001.0000".to_string(),
            url: "http://patch/game/D2026.07.16.0001.0000.patch".to_string(),
            hash_type: String::new(),
            hash_block_size: 0,
            hashes: Vec::new(),
            length: 8,
        };
        let cached = patch_cache_path(&patch_dir, &patch);
        std::fs::write(&cached, b"short").unwrap();

        let result = GameFileManager::new()
            .install(std::slice::from_ref(&patch), &patch_dir, &game_root)
            .await;
        assert!(matches!(
            result,
            Err(GameFileError::PatchIdentityMismatch { .. })
        ));
        assert!(!game_root.join("game/ffxivgame.ver").exists());

        let _ = std::fs::remove_dir_all(root);
    }
}
