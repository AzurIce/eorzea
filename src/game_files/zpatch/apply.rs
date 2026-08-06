//! ZiPatch 补丁应用。
//!
//! 移植自 C# `RemotePatchInstaller.InstallPatch()` + 各 chunk 的 `ApplyChunk()`。
//! 将解析出的 chunk 序列应用到游戏目录（`game/` 或 `boot/` 子目录）。

use std::collections::HashMap;
use std::io::{Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use tracing::{debug, info, warn};

use super::{normalize_path, sqpack_dat_path, sqpack_index_path, FileOperation, Platform, SqpkCommand, ZiPatchChunk, ZiPatchError};

/// 应用配置。
pub struct ApplyContext {
    /// 补丁目标根目录（`game_root/boot` 或 `game_root/game`）。
    pub base_dir: PathBuf,
    /// 当前平台（由 `SQPK:T` 命令设置，默认 Win32）。
    pub platform: Platform,
    /// 缓存打开的写入流（对应 C# `SqexFileStreamStore`）。
    pub store: HashMap<PathBuf, std::fs::File>,
    /// 已应用的文件数（进度用）。
    pub files_written: u64,
}

impl ApplyContext {
    pub fn new(base_dir: impl Into<PathBuf>) -> Self {
        Self {
            base_dir: base_dir.into(),
            platform: Platform::Win32,
            store: HashMap::new(),
            files_written: 0,
        }
    }

    /// 打开（或复用缓存的）目标文件，OpenOrCreate + ReadWrite。
    fn open_stream(&mut self, relative: &str) -> Result<&mut std::fs::File, ZiPatchError> {
        let path = self.base_dir.join(normalize_path(relative));
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).map_err(|e| ZiPatchError::Io {
                path: parent.display().to_string(),
                source: e,
            })?;
        }
        if !self.store.contains_key(&path) {
            let file = std::fs::OpenOptions::new()
                .read(true)
                .write(true)
                .create(true)
                .truncate(false)
                .open(&path)
                .map_err(|e| ZiPatchError::Io {
                    path: path.display().to_string(),
                    source: e,
                })?;
            self.store.insert(path.clone(), file);
        }
        Ok(self.store.get_mut(&path).expect("stream just inserted"))
    }

    fn flush_all(&mut self) {
        for (path, file) in self.store.iter_mut() {
            if let Err(e) = file.flush() {
                warn!(path = %path.display(), error = %e, "failed to flush file");
            }
        }
    }

    /// 写零字节（对应 C# `SqexFileStream.Wipe`）。
    fn wipe(stream: &mut std::fs::File, length: u64) -> Result<(), ZiPatchError> {
        const BUF_LEN: usize = 1 << 16;
        let buf = vec![0u8; BUF_LEN];
        let mut remaining = length;
        while remaining > 0 {
            let n = remaining.min(BUF_LEN as u64) as usize;
            stream.write_all(&buf[..n]).map_err(|e| ZiPatchError::Io {
                path: String::new(),
                source: e,
            })?;
            remaining -= n as u64;
        }
        Ok(())
    }

    /// 写空数据块头（对应 C# `SqpackDatFile.WriteEmptyFileBlockAt`）。
    fn write_empty_block(
        stream: &mut std::fs::File,
        offset: u64,
        block_number: u64,
    ) -> Result<(), ZiPatchError> {
        stream
            .seek(SeekFrom::Start(offset))
            .map_err(|e| ZiPatchError::Io {
                path: String::new(),
                source: e,
            })?;
        Self::wipe(stream, block_number << 7)?;
        stream
            .seek(SeekFrom::Start(offset))
            .map_err(|e| ZiPatchError::Io {
                path: String::new(),
                source: e,
            })?;
        // 头部：i32LE 128, i32LE 0, i32LE 0, i64LE (blockNumber-1), i32LE 0
        stream.write_all(&(1i32 << 7).to_le_bytes()).map_err(|e| ZiPatchError::Io {
            path: String::new(),
            source: e,
        })?;
        stream.write_all(&0i32.to_le_bytes()).map_err(|e| ZiPatchError::Io {
            path: String::new(),
            source: e,
        })?;
        stream.write_all(&0i32.to_le_bytes()).map_err(|e| ZiPatchError::Io {
            path: String::new(),
            source: e,
        })?;
        stream
            .write_all(&(block_number as i64 - 1).to_le_bytes())
            .map_err(|e| ZiPatchError::Io {
                path: String::new(),
                source: e,
            })?;
        stream.write_all(&0i32.to_le_bytes()).map_err(|e| ZiPatchError::Io {
            path: String::new(),
            source: e,
        })?;
        Ok(())
    }

    /// 解压并写入一个压缩块（对应 C# `SqpkCompressedBlock.DecompressInto`）。
    fn write_block(
        stream: &mut std::fs::File,
        block: &super::CompressedBlock,
    ) -> Result<(), ZiPatchError> {
        if block.is_compressed() {
            let mut decoder = flate2::read::DeflateDecoder::new(&block.data[..]);
            std::io::copy(&mut decoder, stream).map_err(|e| ZiPatchError::Io {
                path: String::new(),
                source: e,
            })?;
        } else {
            stream.write_all(&block.data).map_err(|e| ZiPatchError::Io {
                path: String::new(),
                source: e,
            })?;
        }
        Ok(())
    }

    /// 删除文件（带重试，对应 C# `SqexFile.Delete`）。
    fn delete_file(&self, relative: &str) {
        let path = self.base_dir.join(normalize_path(relative));
        if !path.exists() {
            return;
        }
        for attempt in 0..5 {
            match std::fs::remove_file(&path) {
                Ok(()) => return,
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => return,
                Err(e) if attempt < 4 => {
                    warn!(path = %path.display(), error = %e, attempt, "delete failed, retrying");
                    std::thread::sleep(std::time::Duration::from_secs(1));
                }
                Err(e) => {
                    warn!(path = %path.display(), error = %e, "delete failed");
                    return;
                }
            }
        }
    }

    /// 删除某资料片的全部文件（对应 C# `SqpkFile` RemoveAll）。
    fn remove_all_expansion_files(&self, expansion_id: u16) {
        let folder = if expansion_id == 0 {
            "ffxiv".to_string()
        } else {
            format!("ex{expansion_id}")
        };
        for sub in ["sqpack", "movie"] {
            let dir = self.base_dir.join(sub).join(&folder);
            let Ok(entries) = std::fs::read_dir(&dir) else {
                continue;
            };
            for entry in entries.flatten() {
                let name = entry.file_name().to_string_lossy().to_string();
                let keep = name.ends_with(".var")
                    || name.ends_with("00000.bk2")
                    || name.ends_with("00001.bk2")
                    || name.ends_with("00002.bk2")
                    || name.ends_with("00003.bk2");
                if !keep && entry.path().is_file() {
                    debug!(path = %entry.path().display(), "removing file");
                    let _ = std::fs::remove_file(entry.path());
                }
            }
        }
    }

    /// 应用单个 chunk。
    pub fn apply_chunk(&mut self, chunk: &ZiPatchChunk) -> Result<(), ZiPatchError> {
        match chunk {
            ZiPatchChunk::Sqpk(cmd) => self.apply_sqpk(cmd),
            ZiPatchChunk::AddDirectory(dir) => {
                let path = self.base_dir.join(normalize_path(dir));
                std::fs::create_dir_all(&path).map_err(|e| ZiPatchError::Io {
                    path: path.display().to_string(),
                    source: e,
                })?;
                Ok(())
            }
            ZiPatchChunk::DeleteDirectory(dir) => {
                let path = self.base_dir.join(normalize_path(dir));
                match std::fs::remove_dir_all(&path) {
                    Ok(()) => Ok(()),
                    Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
                    Err(e) => Err(ZiPatchError::Io {
                        path: path.display().to_string(),
                        source: e,
                    }),
                }
            }
            // FileHeader / ApplyOption / EndOfFile / Ignored → NOP
            _ => Ok(()),
        }
    }

    fn apply_sqpk(&mut self, cmd: &SqpkCommand) -> Result<(), ZiPatchError> {
        match cmd {
            SqpkCommand::TargetInfo { platform } => {
                debug!(platform = ?platform, "target platform set");
                self.platform = *platform;
                Ok(())
            }

            SqpkCommand::File {
                operation,
                file_offset,
                target_path,
                blocks,
                ..
            } => match operation {
                FileOperation::AddFile => {
                    self.files_written += 1;
                    info!(file = %target_path, offset = file_offset, "applying SqpkFile AddFile");
                    let stream = self.open_stream(target_path)?;
                    if *file_offset == 0 {
                        stream.set_len(0).map_err(|e| ZiPatchError::Io {
                            path: target_path.clone(),
                            source: e,
                        })?;
                    }
                    stream
                        .seek(SeekFrom::Start(*file_offset as u64))
                        .map_err(|e| ZiPatchError::Io {
                            path: target_path.clone(),
                            source: e,
                        })?;
                    for block in blocks {
                        Self::write_block(stream, block)?;
                    }
                    Ok(())
                }
                FileOperation::RemoveAll => {
                    let expansion_id = match cmd {
                        SqpkCommand::File { expansion_id, .. } => *expansion_id,
                        _ => unreachable!(),
                    };
                    info!(expansion = expansion_id, "applying SqpkFile RemoveAll");
                    self.remove_all_expansion_files(expansion_id);
                    Ok(())
                }
                FileOperation::DeleteFile => {
                    info!(file = %target_path, "applying SqpkFile DeleteFile");
                    self.delete_file(target_path);
                    Ok(())
                }
                FileOperation::MakeDirTree => {
                    let path = self.base_dir.join(normalize_path(target_path));
                    std::fs::create_dir_all(&path).map_err(|e| ZiPatchError::Io {
                        path: path.display().to_string(),
                        source: e,
                    })?;
                    Ok(())
                }
            },

            SqpkCommand::AddData {
                main_id,
                sub_id,
                file_id,
                block_offset,
                block_delete_number,
                block_data,
                ..
            } => {
                let rel = sqpack_dat_path(*main_id, *sub_id, *file_id, &self.platform);
                debug!(file = %rel, offset = block_offset, "applying SqpkAddData");
                let stream = self.open_stream(&rel)?;
                stream
                    .seek(SeekFrom::Start(*block_offset))
                    .map_err(|e| ZiPatchError::Io {
                        path: rel.clone(),
                        source: e,
                    })?;
                stream.write_all(block_data).map_err(|e| ZiPatchError::Io {
                    path: rel.clone(),
                    source: e,
                })?;
                if *block_delete_number > 0 {
                    Self::wipe(stream, *block_delete_number)?;
                }
                Ok(())
            }

            SqpkCommand::DeleteData {
                main_id,
                sub_id,
                file_id,
                block_offset,
                block_number,
            } => {
                let rel = sqpack_dat_path(*main_id, *sub_id, *file_id, &self.platform);
                debug!(file = %rel, offset = block_offset, blocks = block_number, "applying SqpkDeleteData");
                let stream = self.open_stream(&rel)?;
                Self::write_empty_block(stream, *block_offset, *block_number)
            }

            SqpkCommand::ExpandData {
                main_id,
                sub_id,
                file_id,
                block_offset,
                block_number,
            } => {
                let rel = sqpack_dat_path(*main_id, *sub_id, *file_id, &self.platform);
                debug!(file = %rel, offset = block_offset, blocks = block_number, "applying SqpkExpandData");
                let stream = self.open_stream(&rel)?;
                Self::write_empty_block(stream, *block_offset, *block_number)
            }

            SqpkCommand::Header {
                file_kind,
                header_kind,
                main_id,
                sub_id,
                file_id,
                header_data,
            } => {
                let rel = if *file_kind == b'D' {
                    sqpack_dat_path(*main_id, *sub_id, *file_id, &self.platform)
                } else {
                    sqpack_index_path(*main_id, *sub_id, *file_id, &self.platform)
                };
                let header_kind_char = (*header_kind as char).to_string();
                debug!(file = %rel, kind = %header_kind_char, "applying SqpkHeader");
                let stream = self.open_stream(&rel)?;
                let offset = if *header_kind == b'V' { 0 } else { 1024 };
                stream
                    .seek(SeekFrom::Start(offset))
                    .map_err(|e| ZiPatchError::Io {
                        path: rel.clone(),
                        source: e,
                    })?;
                stream.write_all(header_data).map_err(|e| ZiPatchError::Io {
                    path: rel.clone(),
                    source: e,
                })?;
                Ok(())
            }

            SqpkCommand::Index | SqpkCommand::PatchInfo => Ok(()), // NOP
        }
    }
}

impl Drop for ApplyContext {
    fn drop(&mut self) {
        self.flush_all();
    }
}

/// 将 ZiPatch 补丁文件应用到 `base_dir`（`game_root/boot` 或 `game_root/game`）。
pub fn apply_patch_file(patch_path: &Path, base_dir: &Path) -> Result<ApplyContext, ZiPatchError> {
    let chunks = super::parse_file(patch_path)?;
    let mut ctx = ApplyContext::new(base_dir);
    for chunk in &chunks {
        ctx.apply_chunk(chunk)?;
    }
    ctx.flush_all();
    Ok(ctx)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_apply_adddata_empty_block_header() {
        // 构造 SqpkCommand::DeleteData 并应用，验证 24 字节头部
        let dir = std::env::temp_dir().join("xl-rs-zpatch-test");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();

        let cmd = SqpkCommand::DeleteData {
            main_id: 0x04,
            sub_id: 0x0104, // ex1
            file_id: 0,
            block_offset: 0,
            block_number: 2, // 2 块
        };

        let mut ctx = ApplyContext::new(&dir);
        ctx.apply_sqpk(&cmd).unwrap();

        let path = dir.join("sqpack/ex1/040104.win32.dat0");
        let data = std::fs::read(&path).unwrap();
        assert_eq!(data.len(), 2 << 7);

        // 头部 24 字节：i32LE 128, 0, 0, i64LE 1, i32LE 0
        assert_eq!(&data[0..4], &128i32.to_le_bytes());
        assert_eq!(&data[4..8], &0i32.to_le_bytes());
        assert_eq!(&data[8..12], &0i32.to_le_bytes());
        assert_eq!(&data[12..20], &1i64.to_le_bytes());
        assert_eq!(&data[20..24], &0i32.to_le_bytes());
        // 其余为零
        assert!(data[24..].iter().all(|&b| b == 0));

        let _ = std::fs::remove_dir_all(&dir);
    }
}
