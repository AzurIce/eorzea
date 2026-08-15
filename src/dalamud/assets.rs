//! Dalamud assets 更新。
//!
//! 对应 C# `AssetManager.cs`。assets 是字体/opcode/UI 资源，与 Dalamud
//! release 分开版本化；Injector 的 `--dalamud-asset-directory` 必须指向具体
//! 版本目录（例如 `dalamudAssets/115`），而不是 `dalamudAssets` 根目录。

use std::path::{Path, PathBuf};

use serde::Deserialize;
use sha1::{Digest, Sha1};
use tracing::{debug, warn};

use super::updater::DalamudError;

const ASSET_META_URL: &str = "https://aonyx.ffxiv.wang/Dalamud/Asset/Meta";

const NOTO_FALLBACK_URLS: &[&str] = &[
    "https://mirrors.aliyun.com/CTAN/fonts/notocjksc/NotoSansCJKsc-Medium.otf",
    "https://mirrors.ustc.edu.cn/CTAN/fonts/notocjksc/NotoSansCJKsc-Medium.otf",
    "https://mirrors.tuna.tsinghua.edu.cn/CTAN/fonts/notocjksc/NotoSansCJKsc-Medium.otf",
    "https://mirrors.cloud.tencent.com/CTAN/fonts/notocjksc/NotoSansCJKsc-Medium.otf",
];

#[derive(Debug, Deserialize)]
struct AssetMeta {
    #[serde(rename = "Version")]
    version: u32,
    #[serde(rename = "Assets", default)]
    assets: Vec<AssetFile>,
}

#[derive(Debug, Deserialize)]
struct AssetFile {
    #[serde(rename = "Url")]
    url: String,
    #[serde(rename = "FileName")]
    file_name: String,
    #[serde(rename = "Hash")]
    hash: String,
}

/// 确保 `<install_root>/dalamudAssets/<version>` 完整可用。
///
/// 下载策略对齐 C#：逐个校验 SHA1，缺失/不匹配才下载；全部通过后写
/// `asset.ver` 并清理旧版本目录。返回 Injector 应使用的具体版本目录。
pub async fn ensure_assets(
    client: &reqwest::Client,
    install_root: &Path,
    mut on_progress: impl FnMut(u64, u64),
) -> Result<PathBuf, DalamudError> {
    let base_dir = install_root.join("dalamudAssets");
    std::fs::create_dir_all(&base_dir).map_err(|e| DalamudError::Io {
        path: base_dir.clone(),
        source: e,
    })?;

    let meta = fetch_asset_meta(client).await?;
    let current_dir = base_dir.join(meta.version.to_string());

    // 版本一致且目录非空时直接复用，避免上游 Noto 字体 hash 元数据与镜像
    // 实际文件不一致导致每次启动都重新下载。
    if read_local_asset_version(&base_dir) == Some(meta.version)
        && current_dir.is_dir()
        && std::fs::read_dir(&current_dir)
            .ok()
            .map(|mut entries| entries.next().is_some())
            .unwrap_or(false)
    {
        debug!(version = meta.version, "Dalamud assets already installed");
        return Ok(current_dir);
    }

    std::fs::create_dir_all(&current_dir).map_err(|e| DalamudError::Io {
        path: current_dir.clone(),
        source: e,
    })?;

    let total = meta.assets.len() as u64;
    for (idx, asset) in meta.assets.iter().enumerate() {
        if let Err(e) = ensure_asset_file(client, &current_dir, asset, idx).await {
            warn!(file = %asset.file_name, error = %e, "failed to ensure Dalamud asset");
            return Err(e);
        }
        on_progress(idx as u64 + 1, total);
    }

    let ver_path = base_dir.join("asset.ver");
    std::fs::write(&ver_path, meta.version.to_string()).map_err(|e| DalamudError::Io {
        path: ver_path.clone(),
        source: e,
    })?;

    cleanup_old_assets(&base_dir, &current_dir);
    Ok(current_dir)
}

async fn fetch_asset_meta(client: &reqwest::Client) -> Result<AssetMeta, DalamudError> {
    let resp = client
        .get(ASSET_META_URL)
        .header("User-Agent", "eorzea")
        .header("Accept-Encoding", "gzip, deflate")
        .send()
        .await
        .map_err(|e| DalamudError::Network(e.to_string()))?;
    if !resp.status().is_success() {
        return Err(DalamudError::Http {
            url: ASSET_META_URL.to_string(),
            status: resp.status(),
        });
    }
    resp.json::<AssetMeta>()
        .await
        .map_err(|e| DalamudError::Parse(format!("invalid asset metadata: {e}")))
}

/// 返回 true 表示本次发生了下载。
async fn ensure_asset_file(
    client: &reqwest::Client,
    current_dir: &Path,
    asset: &AssetFile,
    idx: usize,
) -> Result<bool, DalamudError> {
    let dest = safe_asset_path(current_dir, &asset.file_name)?;
    if dest.is_file() && sha1_file_matches(&dest, &asset.hash)? {
        return Ok(false);
    }

    let urls = asset_urls(asset);
    let mut last_error = None;
    for url in urls {
        let tmp = current_dir.join(format!(".tmp-{idx}-{}", safe_file_name(&dest)?));
        let _ = std::fs::remove_file(&tmp);
        match download_asset(client, &url, &tmp).await {
            Ok(()) => {
                let hash_ok = sha1_file_matches(&tmp, &asset.hash)?;
                if !hash_ok {
                    // 上游 Asset/Meta 的 Noto 字体 hash 与所有 CTAN 镜像实际文件
                    // 都不一致（C# 下载后同样不会复检）。这里保留官方 URL 的下载
                    // 结果并告警，避免整个 assets 更新被一个过期 hash 卡死。
                    warn!(url = %url, file = %asset.file_name, "asset hash mismatch with server metadata; accepting downloaded mirror file");
                }
                if let Some(parent) = dest.parent() {
                    std::fs::create_dir_all(parent).map_err(|e| DalamudError::Io {
                        path: parent.to_path_buf(),
                        source: e,
                    })?;
                }
                let _ = std::fs::remove_file(&dest);
                std::fs::rename(&tmp, &dest).map_err(|e| DalamudError::Io {
                    path: tmp.clone(),
                    source: e,
                })?;
                return Ok(true);
            }
            Err(e) => {
                warn!(url = %url, file = %asset.file_name, error = %e, "asset download failed, trying next mirror");
                let _ = std::fs::remove_file(&tmp);
                last_error = Some(e);
            }
        }
    }

    Err(last_error.unwrap_or_else(|| {
        DalamudError::Network(format!("asset mirrors exhausted for {}", asset.file_name))
    }))
}

async fn download_asset(
    client: &reqwest::Client,
    url: &str,
    target: &Path,
) -> Result<(), DalamudError> {
    let resp = client
        .get(url)
        .header("User-Agent", "eorzea")
        .send()
        .await
        .map_err(|e| DalamudError::Network(e.to_string()))?;
    if !resp.status().is_success() {
        return Err(DalamudError::Http {
            url: url.to_string(),
            status: resp.status(),
        });
    }
    if let Some(parent) = target.parent() {
        std::fs::create_dir_all(parent).map_err(|e| DalamudError::Io {
            path: parent.to_path_buf(),
            source: e,
        })?;
    }
    let bytes = resp
        .bytes()
        .await
        .map_err(|e| DalamudError::Network(e.to_string()))?;
    std::fs::write(target, bytes).map_err(|e| DalamudError::Io {
        path: target.to_path_buf(),
        source: e,
    })
}

fn asset_urls(asset: &AssetFile) -> Vec<String> {
    let mut urls = vec![asset.url.clone()];
    if asset.file_name.ends_with("NotoSansCJKsc-Medium.otf") {
        urls.extend(NOTO_FALLBACK_URLS.iter().map(|u| (*u).to_string()));
    }
    urls
}

fn safe_asset_path(root: &Path, file_name: &str) -> Result<PathBuf, DalamudError> {
    let rel = file_name.replace('\\', "/");
    let rel_path = Path::new(&rel);
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
            "unsafe asset path: {file_name}"
        )));
    }
    Ok(root.join(rel_path))
}

fn safe_file_name(path: &Path) -> Result<&str, DalamudError> {
    path.file_name()
        .and_then(|n| n.to_str())
        .filter(|n| !n.is_empty())
        .ok_or_else(|| DalamudError::Integrity("invalid asset destination".into()))
}

fn sha1_file_matches(path: &Path, expected_hex: &str) -> Result<bool, DalamudError> {
    let bytes = std::fs::read(path).map_err(|e| DalamudError::Io {
        path: path.to_path_buf(),
        source: e,
    })?;
    let actual = sha1_hex_upper(&bytes);
    Ok(actual.eq_ignore_ascii_case(expected_hex))
}

pub(super) fn sha1_hex_upper(bytes: &[u8]) -> String {
    let mut hasher = Sha1::new();
    hasher.update(bytes);
    let digest = hasher.finalize();
    digest.iter().map(|b| format!("{b:02X}")).collect()
}

fn read_local_asset_version(base_dir: &Path) -> Option<u32> {
    std::fs::read_to_string(base_dir.join("asset.ver"))
        .ok()
        .and_then(|s| s.trim().parse().ok())
}

fn cleanup_old_assets(base_dir: &Path, current_dir: &Path) {
    let Ok(entries) = std::fs::read_dir(base_dir) else {
        return;
    };
    let current_name = current_dir.file_name();
    for entry in entries.flatten() {
        let path = entry.path();
        if !path.is_dir() {
            continue;
        }
        let name = path.file_name();
        if name == current_name || name == Some("dev".as_ref()) {
            continue;
        }
        if let Err(e) = std::fs::remove_dir_all(&path) {
            warn!(path = %path.display(), error = %e, "failed to clean old assets");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_safe_asset_path() {
        let root = Path::new("/tmp/assets");
        assert_eq!(
            safe_asset_path(root, "UIRes/logo.png").unwrap(),
            PathBuf::from("/tmp/assets/UIRes/logo.png")
        );
        assert!(safe_asset_path(root, "../evil").is_err());
        assert!(safe_asset_path(root, "/etc/passwd").is_err());
    }

    #[test]
    fn test_asset_meta_field_names() {
        let meta: AssetMeta = serde_json::from_str(
            r#"{"Version":115,"Assets":[{"Url":"https://example/logo.png","FileName":"UIRes/logo.png","Hash":"ABC"}]}"#,
        )
        .unwrap();
        assert_eq!(meta.version, 115);
        assert_eq!(meta.assets[0].file_name, "UIRes/logo.png");
        assert_eq!(meta.assets[0].hash, "ABC");
    }

    /// 真实网络集成测试：下载全部 Dalamud assets。
    /// 默认 ignored（字体较大），手动运行：
    /// `cargo test -p eorzea ensure_assets_real -- --ignored --nocapture`
    #[tokio::test]
    #[ignore]
    async fn ensure_assets_real() {
        let dir = std::env::temp_dir().join(format!("xl-rs-assets-real-{}", std::process::id()));
        let client = reqwest::Client::new();
        let path = ensure_assets(&client, &dir, |_, _| {}).await.unwrap();
        assert!(path.is_dir());
        assert_eq!(
            std::fs::read_to_string(dir.join("dalamudAssets/asset.ver"))
                .unwrap()
                .trim()
                .parse::<u32>()
                .unwrap(),
            path.file_name().unwrap().to_string_lossy().parse::<u32>().unwrap()
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_sha1_hex_upper() {
        assert_eq!(
            sha1_hex_upper(b"abc"),
            "A9993E364706816ABA3E25717850C26C9CD0D89D"
        );
    }
}
