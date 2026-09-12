//! release 归档解压（纯 Rust，不依赖外部 7z 可执行文件）。
//!
//! 上游 `downloadUrl` 当前给的是 `.7z`（实测 LZMA2 单块），历史/其他 track 也
//! 可能给 zip，因此**按文件头判定格式**而不是看扩展名，两种走同一套安全校验与
//! 进度回调。
//!
//! 安全边界（对齐 `docs/notes/dalamud_integration.md` 的风险清单）：
//! - 归档内路径一律拒绝绝对路径 / `..` / 盘符前缀（zip-slip）
//! - 条目数与解压总字节数设上限（防御 zip bomb / 磁盘占满）
//! - 只写调用方给的临时目录；全部成功后才由调用方原子 rename 到安装位置

use std::fs::File;
use std::io::{Read, Write};
use std::path::{Component, Path, PathBuf};

use tracing::debug;

use super::updater::DalamudError;

/// 7z 文件头 `7z¼¯'`（signature header）。
const SEVENZ_MAGIC: [u8; 6] = [0x37, 0x7A, 0xBC, 0xAF, 0x27, 0x1C];
/// zip 的三种文件头：本地文件头 / 空归档 / 分卷标记。
const ZIP_MAGICS: [[u8; 4]; 3] = [
    [0x50, 0x4B, 0x03, 0x04],
    [0x50, 0x4B, 0x05, 0x06],
    [0x50, 0x4B, 0x07, 0x08],
];

/// 单个归档允许的最大条目数。
const MAX_ENTRIES: usize = 20_000;
/// 单个归档允许的最大解压字节数（Dalamud release 实测约 70 MiB）。
const MAX_TOTAL_BYTES: u64 = 4 * 1024 * 1024 * 1024;
/// 解压读写缓冲区大小。
const COPY_BUF: usize = 256 * 1024;

/// 归档格式（按文件头判定，不信任扩展名）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ArchiveKind {
    SevenZ,
    Zip,
}

/// 按文件头识别归档格式。
pub fn detect_kind(path: &Path) -> Result<ArchiveKind, DalamudError> {
    let mut head = [0u8; 6];
    let mut file = File::open(path).map_err(|e| DalamudError::Io {
        path: path.to_path_buf(),
        source: e,
    })?;
    let read = file.read(&mut head).map_err(|e| DalamudError::Io {
        path: path.to_path_buf(),
        source: e,
    })?;
    if read >= SEVENZ_MAGIC.len() && head == SEVENZ_MAGIC {
        Ok(ArchiveKind::SevenZ)
    } else if read >= 4 && ZIP_MAGICS.contains(&head[..4].try_into().unwrap()) {
        Ok(ArchiveKind::Zip)
    } else {
        Err(DalamudError::Archive(format!(
            "unsupported archive format: {}",
            path.display()
        )))
    }
}

/// 解压 `archive` 到 `dest`，`on_progress(已写出字节, 归档解压总字节)`。
///
/// `dest` 由调用方负责创建与清理（通常是安装根目录下的空临时目录）。
pub fn extract_archive(
    archive: &Path,
    dest: &Path,
    mut on_progress: impl FnMut(u64, u64),
) -> Result<(), DalamudError> {
    let kind = detect_kind(archive)?;
    debug!(archive = %archive.display(), ?kind, "extracting release archive");
    match kind {
        ArchiveKind::SevenZ => extract_sevenz(archive, dest, &mut on_progress),
        ArchiveKind::Zip => extract_zip(archive, dest, &mut on_progress),
    }
}

/// 解压 7z（`sevenz-rust2`，纯 Rust：LZMA/LZMA2/BCJ/COPY/DELTA/PPMd/DEFLATE）。
fn extract_sevenz(
    archive: &Path,
    dest: &Path,
    on_progress: &mut dyn FnMut(u64, u64),
) -> Result<(), DalamudError> {
    let file = File::open(archive).map_err(|e| DalamudError::Io {
        path: archive.to_path_buf(),
        source: e,
    })?;
    let mut reader = sevenz_rust2::ArchiveReader::new(file, sevenz_rust2::Password::empty())
        .map_err(|e| DalamudError::Archive(format!("open 7z {}: {e}", archive.display())))?;

    // 先按表头声明的条目数/解压总量做上限检查，再真正解压
    let (entry_count, total) = {
        let files = &reader.archive().files;
        let total: u64 = files
            .iter()
            .filter(|e| e.has_stream())
            .map(|e| e.size())
            .sum();
        (files.len(), total)
    };
    check_limits(entry_count, total)?;

    let mut written = 0u64;
    reader
        .for_each_entries(|entry, input| {
            let rel = safe_relative_path(entry.name())?;
            let out_path = dest.join(&rel);
            if entry.is_directory() {
                std::fs::create_dir_all(&out_path)?;
                return Ok(true);
            }
            if let Some(parent) = out_path.parent() {
                std::fs::create_dir_all(parent)?;
            }
            let mut out = File::create(&out_path)?;
            copy_with_progress(input, &mut out, &mut written, total, on_progress)?;
            Ok(true)
        })
        .map_err(|e| DalamudError::Archive(format!("extract 7z {}: {e}", archive.display())))?;
    Ok(())
}

/// 解压 zip（`zip` crate；上游 release 目前是 7z，这里作为格式兼容路径）。
fn extract_zip(
    archive: &Path,
    dest: &Path,
    on_progress: &mut dyn FnMut(u64, u64),
) -> Result<(), DalamudError> {
    let file = File::open(archive).map_err(|e| DalamudError::Io {
        path: archive.to_path_buf(),
        source: e,
    })?;
    let mut zip = zip::ZipArchive::new(file)
        .map_err(|e| DalamudError::Archive(format!("open zip {}: {e}", archive.display())))?;

    let total: u64 = (0..zip.len())
        .filter_map(|i| zip.by_index(i).ok().map(|e| e.size()))
        .sum();
    check_limits(zip.len(), total)?;

    let mut written = 0u64;
    for i in 0..zip.len() {
        let mut entry = zip
            .by_index(i)
            .map_err(|e| DalamudError::Archive(format!("read zip entry {i}: {e}")))?;
        let rel = safe_relative_path(entry.name())
            .map_err(|e| DalamudError::Integrity(format!("{}: {e}", archive.display())))?;
        let out_path = dest.join(&rel);
        if entry.is_dir() {
            std::fs::create_dir_all(&out_path).map_err(|e| DalamudError::Io {
                path: out_path.clone(),
                source: e,
            })?;
            continue;
        }
        if let Some(parent) = out_path.parent() {
            std::fs::create_dir_all(parent).map_err(|e| DalamudError::Io {
                path: parent.to_path_buf(),
                source: e,
            })?;
        }
        let mut out = File::create(&out_path).map_err(|e| DalamudError::Io {
            path: out_path.clone(),
            source: e,
        })?;
        let copied = copy_with_progress(&mut entry, &mut out, &mut written, total, on_progress);
        copied.map_err(|e| DalamudError::Archive(format!("extract zip entry {i}: {e}")))?;
    }
    Ok(())
}

/// 条目数 / 解压总量上限检查（表头声明的尺寸，逐块写出时还会再累加复核）。
fn check_limits(entries: usize, total: u64) -> Result<(), DalamudError> {
    if entries > MAX_ENTRIES {
        return Err(DalamudError::Integrity(format!(
            "archive has too many entries: {entries} > {MAX_ENTRIES}"
        )));
    }
    if total > MAX_TOTAL_BYTES {
        return Err(DalamudError::Integrity(format!(
            "archive unpacks to {total} bytes > {MAX_TOTAL_BYTES}"
        )));
    }
    Ok(())
}

/// 分块复制并上报进度；累计写出超过上限立即中止。
fn copy_with_progress(
    input: &mut dyn Read,
    out: &mut File,
    written: &mut u64,
    total: u64,
    on_progress: &mut dyn FnMut(u64, u64),
) -> std::io::Result<()> {
    let mut buf = vec![0u8; COPY_BUF];
    loop {
        let n = input.read(&mut buf)?;
        if n == 0 {
            break;
        }
        *written += n as u64;
        if *written > MAX_TOTAL_BYTES {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                format!("archive exceeds {MAX_TOTAL_BYTES} bytes when unpacking"),
            ));
        }
        out.write_all(&buf[..n])?;
        on_progress(*written, total);
    }
    Ok(())
}

/// 把归档内的相对路径规范化，拒绝绝对路径 / `..` / 盘符前缀（zip-slip）。
fn safe_relative_path(name: &str) -> std::io::Result<PathBuf> {
    let normalized = name.replace('\\', "/");
    let mut out = PathBuf::new();
    for component in Path::new(&normalized).components() {
        match component {
            Component::Normal(part) => out.push(part),
            Component::CurDir => {}
            Component::ParentDir | Component::RootDir | Component::Prefix(_) => {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::InvalidData,
                    format!("unsafe path in archive: {name}"),
                ));
            }
        }
    }
    if out.as_os_str().is_empty() {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            format!("empty path in archive: {name}"),
        ));
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_dir(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("xl-rs-arch-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    /// 用 `ArchiveWriter`（LZMA2）造一个和 release 同格式的 7z。
    fn write_7z(path: &Path, entries: &[(&str, &[u8])]) {
        let mut writer = sevenz_rust2::ArchiveWriter::create(path).unwrap();
        for (name, data) in entries {
            writer
                .push_archive_entry(
                    sevenz_rust2::ArchiveEntry::new_file(name),
                    Some(std::io::Cursor::new(data.to_vec())),
                )
                .unwrap();
        }
        writer.finish().unwrap();
    }

    #[test]
    fn test_detect_kind() {
        let dir = temp_dir("kind");
        let sevenz = dir.join("a.7z");
        write_7z(&sevenz, &[("f.txt", b"x")]);
        assert_eq!(detect_kind(&sevenz).unwrap(), ArchiveKind::SevenZ);

        // 扩展名骗人时必须按文件头判定
        let fake = dir.join("b.7z");
        std::fs::write(&fake, b"not an archive").unwrap();
        assert!(matches!(detect_kind(&fake), Err(DalamudError::Archive(_))));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_extract_sevenz_with_progress_and_layout() {
        let dir = temp_dir("7z");
        let archive = dir.join("rel.7z");
        write_7z(
            &archive,
            &[
                ("Dalamud.Injector.exe", b"injector"),
                ("UIRes/logo.png", b"png-bytes"),
            ],
        );

        let out = dir.join("out");
        std::fs::create_dir_all(&out).unwrap();
        let mut last = (0u64, 0u64);
        extract_archive(&archive, &out, |done, total| last = (done, total)).unwrap();

        assert_eq!(
            std::fs::read(out.join("Dalamud.Injector.exe")).unwrap(),
            b"injector"
        );
        assert_eq!(
            std::fs::read(out.join("UIRes/logo.png")).unwrap(),
            b"png-bytes"
        );
        assert_eq!(last, (17, 17));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_extract_zip() {
        let dir = temp_dir("zip");
        let archive = dir.join("rel.zip");
        {
            use std::io::Write;
            let mut zip = zip::ZipWriter::new(File::create(&archive).unwrap());
            let opts = zip::write::SimpleFileOptions::default();
            zip.start_file("Dalamud.Injector.exe", opts).unwrap();
            zip.write_all(b"injector").unwrap();
            zip.start_file("nested/file.bin", opts).unwrap();
            zip.write_all(b"data").unwrap();
            zip.finish().unwrap();
        }

        let out = dir.join("out");
        std::fs::create_dir_all(&out).unwrap();
        extract_archive(&archive, &out, |_, _| {}).unwrap();
        assert_eq!(
            std::fs::read(out.join("Dalamud.Injector.exe")).unwrap(),
            b"injector"
        );
        assert_eq!(std::fs::read(out.join("nested/file.bin")).unwrap(), b"data");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_safe_relative_path_rejects_escape() {
        assert_eq!(
            safe_relative_path("Hooks/15.0.3.4/Dalamud.dll").unwrap(),
            PathBuf::from("Hooks/15.0.3.4/Dalamud.dll")
        );
        assert_eq!(
            safe_relative_path("runtimes\\win-x64\\native\\nethost.dll").unwrap(),
            PathBuf::from("runtimes/win-x64/native/nethost.dll")
        );
        assert!(safe_relative_path("../evil.dll").is_err());
        assert!(safe_relative_path("a/../../evil.dll").is_err());
        assert!(safe_relative_path("/etc/passwd").is_err());
        assert!(safe_relative_path("C:\\Windows\\evil.dll").is_err());
        assert!(safe_relative_path("").is_err());
    }

    /// 真实网络集成测试：下载上游 release 并解压 + 逐文件校验。
    /// 默认 ignored（约 13 MiB），手动运行：
    /// `cargo test -p eorzea download_release_real -- --ignored --nocapture`
    #[tokio::test]
    #[ignore]
    async fn download_release_real() {
        let dir = std::env::temp_dir().join(format!("xl-rs-release-real-{}", std::process::id()));
        let client = reqwest::Client::new();
        let info = crate::dalamud::updater::fetch_version_info(&client, "release")
            .await
            .unwrap();
        let path = crate::dalamud::updater::download_release(&client, &info, &dir, |_, _| {})
            .await
            .unwrap();
        assert!(path.join("Dalamud.Injector.exe").is_file());
        assert!(crate::dalamud::updater::release_install_is_valid(
            &path, &info.hash
        ));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
