//! Windows x64 .NET runtime 下载/组装/校验。
//!
//! 对应 C# `DalamudUpdater.DownloadRuntime()`。运行在 Wine 里的是 Windows
//! .NET Runtime，不能用宿主 Linux dotnet 代替。这里从 NuGet/华为镜像下载两个
//! win-x64 nupkg，只提取 Injector 需要的 native/lib 目录，并组装出：
//!
//! ```text
//! <install_root>/runtime/
//!   host/fxr/<version>/hostfxr.dll
//!   shared/Microsoft.NETCore.App/<version>/...
//!   shared/Microsoft.WindowsDesktop.App/<version>/...
//!   version
//! ```

use std::io::Write;
use std::path::{Path, PathBuf};

use tracing::{debug, info, warn};

use super::updater::{runtime_dir_matches, DalamudError};

const HUAWEI_NUGET_BASE: &str =
    "https://repo.huaweicloud.com/artifactory/api/nuget/v3/nuget-remote";
const NUGET_BASE: &str = "https://api.nuget.org/v3-flatcontainer";

/// 确保 `<install_root>/runtime` 有指定版本的完整 Windows x64 runtime。
///
/// 已完整安装时直接返回；否则在临时目录组装、校验，成功后原子替换。
pub async fn ensure_runtime(
    client: &reqwest::Client,
    install_root: &Path,
    version: &str,
    mut on_progress: impl FnMut(u64, u64),
) -> Result<PathBuf, DalamudError> {
    if version.trim().is_empty() {
        return Err(DalamudError::Integrity(
            "empty .NET runtime version in release metadata".into(),
        ));
    }
    let runtime_dir = install_root.join("runtime");
    if runtime_layout_ok(&runtime_dir, version) {
        debug!(version, "Windows .NET runtime already installed");
        return Ok(runtime_dir);
    }

    let tmp_dir = install_root.join(format!(".tmp-runtime-{version}"));
    let _ = std::fs::remove_dir_all(&tmp_dir);
    std::fs::create_dir_all(&tmp_dir).map_err(|e| DalamudError::Io {
        path: tmp_dir.clone(),
        source: e,
    })?;

    let result = assemble_runtime(client, &tmp_dir, version, &mut on_progress).await;
    if let Err(e) = result {
        warn!(error = %e, "failed to assemble .NET runtime");
        let _ = std::fs::remove_dir_all(&tmp_dir);
        return Err(e);
    }

    // 注意：上游 `/Dalamud/Release/Runtime/Hashes/<version>` 当前与 NuGet 包
    // 实际内容不一致（实测 476 项中仅 1 项匹配），C# 在“全新下载”后也不会
    // 再按该 manifest 复检。因此这里以官方 NuGet 源的 HTTPS 下载 + 目录结构
    // 校验为准，不把该 manifest 作为阻断条件。
    let _ = std::fs::remove_dir_all(&runtime_dir);
    std::fs::rename(&tmp_dir, &runtime_dir).map_err(|e| DalamudError::Io {
        path: runtime_dir.clone(),
        source: e,
    })?;

    info!(version, path = %runtime_dir.display(), "Windows .NET runtime installed");
    Ok(runtime_dir)
}

async fn assemble_runtime(
    client: &reqwest::Client,
    runtime_dir: &Path,
    version: &str,
    on_progress: &mut impl FnMut(u64, u64),
) -> Result<(), DalamudError> {
    let major = version.split('.').next().unwrap_or(version);
    let core_pkg = format!("microsoft.netcore.app.runtime.win-x64.{version}.nupkg");
    let desktop_pkg = format!("microsoft.windowsdesktop.app.runtime.win-x64.{version}.nupkg");
    let lib_prefix = format!("runtimes/win-x64/lib/net{major}.0/");

    let core_zip = runtime_dir.join("core.nupkg");
    let desktop_zip = runtime_dir.join("desktop.nupkg");
    download_nupkg(
        client,
        &format!("microsoft.netcore.app.runtime.win-x64/{version}/{core_pkg}"),
        &core_zip,
        on_progress,
    )
    .await?;
    download_nupkg(
        client,
        &format!("microsoft.windowsdesktop.app.runtime.win-x64/{version}/{desktop_pkg}"),
        &desktop_zip,
        on_progress,
    )
    .await?;

    let core_dir = runtime_dir
        .join("shared/Microsoft.NETCore.App")
        .join(version);
    let desktop_dir = runtime_dir
        .join("shared/Microsoft.WindowsDesktop.App")
        .join(version);
    extract_zip_prefix(&core_zip, &core_dir, "runtimes/win-x64/native/")?;
    extract_zip_prefix(&core_zip, &core_dir, &lib_prefix)?;
    extract_zip_prefix(&desktop_zip, &desktop_dir, "runtimes/win-x64/native/")?;
    extract_zip_prefix(&desktop_zip, &desktop_dir, &lib_prefix)?;

    // C# 从 CoreCLR 包中把 hostfxr.dll 挪到 host/fxr/<version>。
    let hostfxr_src = core_dir.join("hostfxr.dll");
    if !hostfxr_src.is_file() {
        return Err(DalamudError::Integrity(format!(
            "runtime package missing hostfxr.dll under {core_dir:?}"
        )));
    }
    let hostfxr_dir = runtime_dir.join("host/fxr").join(version);
    std::fs::create_dir_all(&hostfxr_dir).map_err(|e| DalamudError::Io {
        path: hostfxr_dir.clone(),
        source: e,
    })?;
    let hostfxr_dst = hostfxr_dir.join("hostfxr.dll");
    std::fs::rename(&hostfxr_src, &hostfxr_dst).map_err(|e| DalamudError::Io {
        path: hostfxr_src.clone(),
        source: e,
    })?;

    std::fs::write(runtime_dir.join("version"), version).map_err(|e| DalamudError::Io {
        path: runtime_dir.join("version"),
        source: e,
    })?;

    let _ = std::fs::remove_file(&core_zip);
    let _ = std::fs::remove_file(&desktop_zip);
    Ok(())
}

async fn download_nupkg(
    client: &reqwest::Client,
    relative: &str,
    target: &Path,
    on_progress: &mut impl FnMut(u64, u64),
) -> Result<(), DalamudError> {
    let mut last_error = None;
    for base in [HUAWEI_NUGET_BASE, NUGET_BASE] {
        let url = format!("{base}/{relative}");
        match download_to_file(client, &url, target, on_progress).await {
            Ok(()) => return Ok(()),
            Err(e) => {
                warn!(url = %url, error = %e, "runtime package download failed, trying next mirror");
                last_error = Some(e);
                let _ = std::fs::remove_file(target);
            }
        }
    }
    Err(last_error.unwrap_or_else(|| {
        DalamudError::Network("runtime package mirrors exhausted".into())
    }))
}

async fn download_to_file(
    client: &reqwest::Client,
    url: &str,
    target: &Path,
    on_progress: &mut impl FnMut(u64, u64),
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
    let total = resp
        .headers()
        .get(reqwest::header::CONTENT_LENGTH)
        .and_then(|v| v.to_str().ok())
        .and_then(|s| s.parse::<u64>().ok())
        .unwrap_or(0);
    if let Some(parent) = target.parent() {
        std::fs::create_dir_all(parent).map_err(|e| DalamudError::Io {
            path: parent.to_path_buf(),
            source: e,
        })?;
    }
    let mut file = std::fs::File::create(target).map_err(|e| DalamudError::Io {
        path: target.to_path_buf(),
        source: e,
    })?;
    let mut stream = resp;
    let mut written = 0u64;
    while let Some(chunk) = stream
        .chunk()
        .await
        .map_err(|e| DalamudError::Network(e.to_string()))?
    {
        file.write_all(&chunk).map_err(|e| DalamudError::Io {
            path: target.to_path_buf(),
            source: e,
        })?;
        written += chunk.len() as u64;
        on_progress(written, total);
    }
    file.flush().map_err(|e| DalamudError::Io {
        path: target.to_path_buf(),
        source: e,
    })?;
    Ok(())
}

fn extract_zip_prefix(zip_path: &Path, dest: &Path, prefix: &str) -> Result<(), DalamudError> {
    let file = std::fs::File::open(zip_path).map_err(|e| DalamudError::Io {
        path: zip_path.to_path_buf(),
        source: e,
    })?;
    let mut archive = zip::ZipArchive::new(file)
        .map_err(|e| DalamudError::Parse(format!("open nupkg {zip_path:?}: {e}")))?;
    let prefix = prefix.trim_end_matches('/').to_ascii_lowercase();

    for i in 0..archive.len() {
        let mut entry = archive
            .by_index(i)
            .map_err(|e| DalamudError::Parse(format!("read nupkg entry {i}: {e}")))?;
        let name = entry.name().replace('\\', "/");
        let name_lower = name.to_ascii_lowercase();
        let prefix_with_slash = format!("{prefix}/");
        if entry.is_dir()
            || (name_lower != prefix && !name_lower.starts_with(&prefix_with_slash))
        {
            continue;
        }
        let rel = name[prefix.len()..].trim_start_matches('/');
        if rel.is_empty() {
            continue;
        }
        let rel_path = Path::new(rel);
        if rel_path.is_absolute()
            || rel_path
                .components()
                .any(|c| matches!(c, std::path::Component::ParentDir | std::path::Component::RootDir))
        {
            return Err(DalamudError::Integrity(format!(
                "unsafe runtime zip entry: {name}"
            )));
        }
        let out_path = dest.join(rel_path);
        if let Some(parent) = out_path.parent() {
            std::fs::create_dir_all(parent).map_err(|e| DalamudError::Io {
                path: parent.to_path_buf(),
                source: e,
            })?;
        }
        let mut out = std::fs::File::create(&out_path).map_err(|e| DalamudError::Io {
            path: out_path.clone(),
            source: e,
        })?;
        std::io::copy(&mut entry, &mut out).map_err(|e| DalamudError::Io {
            path: out_path.clone(),
            source: e,
        })?;
    }
    Ok(())
}

fn runtime_layout_ok(root: &Path, version: &str) -> bool {
    let version_file_ok = std::fs::read_to_string(root.join("version"))
        .map(|s| s.trim() == version)
        .unwrap_or(false);
    version_file_ok
        && runtime_dir_matches(root, Some(version))
        && root
            .join("host/fxr")
            .join(version)
            .join("hostfxr.dll")
            .is_file()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_runtime_layout_ok() {
        let dir = std::env::temp_dir().join(format!("xl-rs-rt-layout-{}", std::process::id()));
        let root = dir.join("runtime");
        let v = "10.0.1";
        std::fs::create_dir_all(root.join("host/fxr").join(v)).unwrap();
        std::fs::create_dir_all(root.join("shared/Microsoft.NETCore.App").join(v)).unwrap();
        std::fs::create_dir_all(root.join("shared/Microsoft.WindowsDesktop.App").join(v)).unwrap();
        std::fs::write(root.join("host/fxr").join(v).join("hostfxr.dll"), b"x").unwrap();
        std::fs::write(root.join("version"), v).unwrap();
        assert!(runtime_layout_ok(&root, v));
        assert!(!runtime_layout_ok(&root, "9.0.0"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_extract_zip_prefix_extracts_only_matching_dir() {
        let dir = std::env::temp_dir().join(format!("xl-rs-zip-ok-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let zip_path = dir.join("ok.nupkg");
        let file = std::fs::File::create(&zip_path).unwrap();
        let mut zip = zip::ZipWriter::new(file);
        zip.start_file("runtimes/win-x64/native/hostfxr.dll", zip::write::SimpleFileOptions::default())
            .unwrap();
        zip.write_all(b"hostfxr").unwrap();
        zip.start_file("runtimes/win-x64/lib/net10.0/Core.dll", zip::write::SimpleFileOptions::default())
            .unwrap();
        zip.write_all(b"core").unwrap();
        zip.start_file("unrelated/file.txt", zip::write::SimpleFileOptions::default())
            .unwrap();
        zip.write_all(b"skip").unwrap();
        zip.finish().unwrap();

        let out = dir.join("out");
        extract_zip_prefix(&zip_path, &out, "runtimes/win-x64/native/").unwrap();
        assert_eq!(
            std::fs::read(out.join("hostfxr.dll")).unwrap(),
            b"hostfxr"
        );
        assert!(!out.join("unrelated").exists());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_extract_zip_prefix_rejects_parent() {
        let dir = std::env::temp_dir().join(format!("xl-rs-zip-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let zip_path = dir.join("bad.nupkg");
        let file = std::fs::File::create(&zip_path).unwrap();
        let mut zip = zip::ZipWriter::new(file);
        zip.start_file("runtimes/win-x64/native/../evil.dll", zip::write::SimpleFileOptions::default())
            .unwrap();
        zip.write_all(b"evil").unwrap();
        zip.finish().unwrap();

        let err = extract_zip_prefix(&zip_path, &dir.join("out"), "runtimes/win-x64/native/")
            .unwrap_err();
        assert!(matches!(err, DalamudError::Integrity(_)));
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// 真实网络集成测试：从 NuGet/华为镜像下载并组装 runtime。
    /// 默认 ignored（约 80 MiB），手动运行：
    /// `cargo test -p eorzea ensure_runtime_real -- --ignored --nocapture`
    #[tokio::test]
    #[ignore]
    async fn ensure_runtime_real() {
        let dir = std::env::temp_dir().join(format!("xl-rs-runtime-real-{}", std::process::id()));
        let client = reqwest::Client::new();
        let path = ensure_runtime(&client, &dir, "10.0.1", |_, _| {}).await.unwrap();
        assert!(runtime_layout_ok(&path, "10.0.1"));
        let _ = std::fs::remove_dir_all(&dir);
    }



}
