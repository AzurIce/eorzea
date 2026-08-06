//! 补丁下载管理。
//!
//! 对应 C# `PatchManager` 的下载部分（`PatchAcquisition` + `CheckPatchValidity`）：
//! - 并发下载（默认 4 个槽位，同 C# `MAX_DOWNLOADS_AT_ONCE`）
//! - 下载完成后按块做 SHA1 校验（`HashType=sha1`、`HashBlockSize` 分块）
//! - 已下载且校验通过的补丁自动跳过
//!
//! 补丁**应用**（IndexedZiPatch）尚未实现，见 `crate::game_files` 模块说明。

use std::path::{Path, PathBuf};
use std::sync::Arc;
use tracing::{debug, info, warn};
use xiv_launcher_auth::PatchListEntry;

/// 并发下载槽位数，与 C# `PatchManager.MAX_DOWNLOADS_AT_ONCE` 一致。
pub const MAX_DOWNLOADS_AT_ONCE: usize = 4;

/// 补丁下载结果。
#[derive(Debug)]
pub struct DownloadSummary {
    pub downloaded: Vec<PathBuf>,
    pub skipped: usize,
    pub total_bytes: u64,
}

/// 校验文件是否符合补丁条目的逐块 SHA1 哈希。
///
/// 对应 C# `PatchManager.CheckPatchValidity()`。
pub fn verify_patch_sha1(
    path: &Path,
    entry: &PatchListEntry,
) -> Result<bool, PatchDownloadError> {
    if entry.hash_type != "sha1" {
        warn!(
            hash_type = %entry.hash_type,
            "unsupported hash type, skipping verification"
        );
        return Ok(true);
    }
    if entry.hashes.is_empty() || entry.hash_block_size == 0 {
        // boot 补丁无 hash 信息，只校验大小
        return Ok(true);
    }

    let data = std::fs::read(path).map_err(|e| PatchDownloadError::Io {
        path: path.to_path_buf(),
        source: e,
    })?;

    if data.len() as u64 != entry.length {
        return Ok(false);
    }

    use sha1::{Digest, Sha1};

    let block_size = entry.hash_block_size as usize;
    for (i, block) in data.chunks(block_size).enumerate() {
        let expected = entry
            .hashes
            .get(i)
            .ok_or(PatchDownloadError::TooManyBlocks { index: i })?;
        let actual = format!("{:x}", Sha1::digest(block));
        if &actual != expected {
            debug!(
                block = i,
                file = %path.display(),
                "SHA1 mismatch at block {i}"
            );
            return Ok(false);
        }
    }

    Ok(true)
}

/// 并发下载补丁到暂存目录。
///
/// - 已存在且 SHA1 校验通过的文件跳过
/// - 校验失败的文件重新下载（覆盖）
/// - 进度通过 `on_progress(done, total_bytes)` 回调上报
pub async fn download_patches(
    client: &reqwest::Client,
    patches: &[PatchListEntry],
    dest_dir: &Path,
    concurrency: usize,
    mut on_progress: impl FnMut(u64, u64),
) -> Result<DownloadSummary, PatchDownloadError> {
    std::fs::create_dir_all(dest_dir).map_err(|e| PatchDownloadError::Io {
        path: dest_dir.to_path_buf(),
        source: e,
    })?;

    let total_bytes: u64 = patches.iter().map(|p| p.length).sum();
    let semaphore = Arc::new(tokio::sync::Semaphore::new(concurrency.max(1)));
    let mut downloaded_bytes: u64 = 0;
    let mut skipped = 0;
    let mut downloaded = Vec::new();

    let mut tasks = Vec::new();
    for entry in patches.iter() {
        let client = client.clone();
        let semaphore = semaphore.clone();
        let dest_dir = dest_dir.to_path_buf();
        let entry = entry.clone();
        tasks.push(tokio::spawn(async move {
            let _permit = semaphore
                .acquire()
                .await
                .map_err(|_| PatchDownloadError::Semaphore)?;
            download_one(&client, &entry, &dest_dir).await
        }));
    }

    for task in tasks {
        let (bytes, path) = match task.await {
            Ok(Ok(r)) => r,
            Ok(Err(e)) => return Err(e),
            Err(e) => return Err(PatchDownloadError::TaskJoin(e)),
        };
        match bytes {
            Some(b) => {
                downloaded_bytes += b;
                downloaded.push(path);
            }
            None => skipped += 1,
        }
        on_progress(downloaded_bytes, total_bytes);
    }

    Ok(DownloadSummary {
        downloaded,
        skipped,
        total_bytes,
    })
}

/// 下载单个补丁。
///
/// 返回 `(Some(字节数), 文件路径)`；若文件已存在且校验通过则返回 `(None, 路径)`。
async fn download_one(
    client: &reqwest::Client,
    entry: &PatchListEntry,
    dest_dir: &Path,
) -> Result<(Option<u64>, PathBuf), PatchDownloadError> {
    // 目标文件名：取 URL 最后一段，附上版本号避免冲突
    let file_name = entry
        .url
        .rsplit('/')
        .next()
        .filter(|s| !s.is_empty())
        .unwrap_or("patch");
    let dest = dest_dir.join(format!("{}.{}", file_name, entry.version));

    // 已存在且校验通过 → 跳过
    if dest.exists() {
        match verify_patch_sha1(&dest, entry) {
            Ok(true) => {
                info!(file = %dest.display(), "patch already downloaded and verified, skipping");
                return Ok((None, dest));
            }
            Ok(false) => {
                info!(file = %dest.display(), "existing patch failed verification, re-downloading");
            }
            Err(e) => return Err(e),
        }
    }

    info!(
        url = %entry.url,
        bytes = entry.length,
        dest = %dest.display(),
        "downloading patch"
    );

    let mut response = client
        .get(&entry.url)
        .header("User-Agent", "FFXIV_Patch")
        .header("Accept", "*/*")
        .send()
        .await
        .map_err(|e| PatchDownloadError::Request {
            url: entry.url.clone(),
            source: e,
        })?;

    let status = response.status();
    if !status.is_success() {
        return Err(PatchDownloadError::Http {
            url: entry.url.clone(),
            status,
        });
    }

    let mut file = std::fs::File::create(&dest).map_err(|e| PatchDownloadError::Io {
        path: dest.clone(),
        source: e,
    })?;

    let mut bytes = 0u64;
    while let Some(chunk) = response
        .chunk()
        .await
        .map_err(|e| PatchDownloadError::Body {
            url: entry.url.clone(),
            source: e,
        })?
    {
        use std::io::Write;
        file.write_all(&chunk).map_err(|e| PatchDownloadError::Io {
            path: dest.clone(),
            source: e,
        })?;
        bytes += chunk.len() as u64;
    }

    debug!(file = %dest.display(), bytes, "patch downloaded");

    // 校验大小 + SHA1
    if bytes != entry.length {
        warn!(
            file = %dest.display(),
            expected = entry.length,
            actual = bytes,
            "patch size mismatch"
        );
        return Err(PatchDownloadError::SizeMismatch {
            url: entry.url.clone(),
            expected: entry.length,
            actual: bytes,
        });
    }

    match verify_patch_sha1(&dest, entry) {
        Ok(true) => Ok((Some(bytes), dest)),
        Ok(false) => Err(PatchDownloadError::HashMismatch {
            file: dest,
            url: entry.url.clone(),
        }),
        Err(e) => Err(e),
    }
}

/// 补丁下载错误。
#[derive(Debug, thiserror::Error)]
pub enum PatchDownloadError {
    #[error("semaphore acquire failed")]
    Semaphore,

    #[error("task join failed: {0}")]
    TaskJoin(#[from] tokio::task::JoinError),

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

    #[error("failed reading response body for {url}: {source}")]
    Body {
        url: String,
        #[source]
        source: reqwest::Error,
    },

    #[error("IO error at {path}: {source}")]
    Io {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },

    #[error("size mismatch for {url}: expected {expected}, got {actual}")]
    SizeMismatch {
        url: String,
        expected: u64,
        actual: u64,
    },

    #[error("SHA1 verification failed for {file} ({url})")]
    HashMismatch { file: PathBuf, url: String },

    #[error("patch has more data blocks than provided hashes (block {index})")]
    TooManyBlocks { index: usize },
}

#[cfg(test)]
mod tests {
    use super::*;
    use sha1::{Digest, Sha1};
    use std::sync::atomic::{AtomicU32, Ordering};

    static COUNTER: AtomicU32 = AtomicU32::new(0);

    fn entry_for(data: &[u8], block_size: usize) -> PatchListEntry {
        let hashes: Vec<String> = data
            .chunks(block_size)
            .map(|b| format!("{:x}", Sha1::digest(b)))
            .collect();
        PatchListEntry {
            version: "test".to_string(),
            url: "http://test/patch".to_string(),
            hash_type: "sha1".to_string(),
            hash_block_size: block_size as u64,
            hashes,
            length: data.len() as u64,
        }
    }

    fn temp_dir(name: &str) -> std::path::PathBuf {
        let n = COUNTER.fetch_add(1, Ordering::SeqCst);
        std::env::temp_dir().join(format!("xl-rs-verify-test-{n}-{name}"))
    }

    #[test]
    fn test_verify_sha1_ok() {
        let data = b"hello world patch data, more than one block";
        let entry = entry_for(data, 16);
        let dir = temp_dir("xl-rs-verify-test");
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("patch.bin");
        std::fs::write(&path, data).unwrap();
        assert!(verify_patch_sha1(&path, &entry).unwrap());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_verify_sha1_mismatch() {
        let data = b"hello world patch data, more than one block";
        let entry = entry_for(data, 16);
        let dir = temp_dir("xl-rs-verify-test");
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("patch.bin");
        std::fs::write(&path, b"hello world patch data, DIFFERENT block").unwrap();
        assert!(!verify_patch_sha1(&path, &entry).unwrap());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_verify_sha1_wrong_length() {
        let data = b"hello world patch data, more than one block";
        let entry = entry_for(data, 16);
        let dir = temp_dir("xl-rs-verify-test");
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("patch.bin");
        std::fs::write(&path, b"short").unwrap();
        assert!(!verify_patch_sha1(&path, &entry).unwrap());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_verify_boot_patch_no_hash() {
        let dir = temp_dir("xl-rs-verify-test");
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("boot.patch");
        std::fs::write(&path, b"anything").unwrap();
        let entry = PatchListEntry {
            version: "boot".to_string(),
            url: "http://test/boot".to_string(),
            hash_type: String::new(),
            hash_block_size: 0,
            hashes: Vec::new(),
            length: 8,
        };
        // 无 hash 信息 → 跳过校验返回 true
        assert!(verify_patch_sha1(&path, &entry).unwrap());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
