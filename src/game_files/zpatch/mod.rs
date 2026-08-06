//! ZiPatch 补丁文件解析。
//!
//! 移植自 C# `XIVLauncher.Common/Patching/ZiPatch/`。
//! 文件格式：
//!
//! ```text
//! [12 字节 magic]  (0x50495A91, 0x48435441, 0x0A1A0A0D，小端存储)
//! 循环:
//!   [u32 BE size][4 字节类型][size 字节数据][u32 BE checksum]
//! ```
//!
//! chunk 类型：`FHDR`（文件头）、`APLY`（应用选项）、`SQPK`（Sqpk 命令）、
//! `ADIR`/`DELD`（目录操作）、`EOF_`（结束）、`XXXX`/`APFS`（忽略）。
//! `SQPK` 内部为命令：`T`(TargetInfo) `F`(File) `A`(AddData) `D`(DeleteData)
//! `E`(ExpandData) `H`(Header) `I`(Index) `X`(PatchInfo)。

use std::fmt;

pub mod apply;

/// ZiPatch 文件 magic（小端读取值，即文件字节为 91 5A 49 50 ...）。
pub const ZIPATCH_MAGIC: [u32; 3] = [0x50495A91, 0x48435441, 0x0A1A0A0D];

/// Sqpk 平台 ID（对应 C# `ZiPatchConfig.PlatformId`）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Platform {
    Win32,
    Ps3,
    Ps4,
    Unknown,
}

impl Platform {
    fn from_u16(v: u16) -> Self {
        match v {
            0 => Platform::Win32,
            1 => Platform::Ps3,
            2 => Platform::Ps4,
            _ => Platform::Unknown,
        }
    }

    /// 平台文件名后缀（如 `win32`）。
    pub fn file_suffix(&self) -> &'static str {
        match self {
            Platform::Win32 => "win32",
            Platform::Ps3 => "ps3",
            Platform::Ps4 => "ps4",
            Platform::Unknown => "unknown",
        }
    }
}

/// 文件头 chunk（`FHDR`）。
#[derive(Debug)]
pub struct FileHeaderChunk {
    pub version: u8,
    pub patch_type: String,
    pub entry_files: u32,
    // V3 附加字段（解析但不使用）
    pub repository_name: u32,
    pub commands: u32,
}

/// Sqpk File 命令（`F`）操作。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FileOperation {
    AddFile,
    RemoveAll,
    DeleteFile,
    MakeDirTree,
}

/// 单个压缩数据块（对应 C# `SqpkCompressedBlock`）。
#[derive(Debug, Clone)]
pub struct CompressedBlock {
    pub header_size: i32,
    pub compressed_size: i32,
    pub decompressed_size: i32,
    /// `IsCompressed` 时是 raw deflate 数据；否则是原始数据。
    pub data: Vec<u8>,
}

impl CompressedBlock {
    /// 是否为压缩数据（`CompressedSize != 0x7d00`）。
    pub fn is_compressed(&self) -> bool {
        self.compressed_size != 0x7d00
    }
}

/// Sqpk 命令。
#[derive(Debug)]
pub enum SqpkCommand {
    /// `T` — 目标信息（设置平台）。
    TargetInfo { platform: Platform },
    /// `F` — 文件操作。
    File {
        operation: FileOperation,
        file_offset: i64,
        file_size: i64,
        expansion_id: u16,
        target_path: String,
        blocks: Vec<CompressedBlock>,
    },
    /// `A` — 追加数据到 sqpack dat。
    AddData {
        main_id: u16,
        sub_id: u16,
        file_id: u32,
        block_offset: u64,
        block_number: u64,
        block_delete_number: u64,
        block_data: Vec<u8>,
    },
    /// `D` — 删除（清空）sqpack dat 块。
    DeleteData {
        main_id: u16,
        sub_id: u16,
        file_id: u32,
        block_offset: u64,
        block_number: u64,
    },
    /// `E` — 扩展（清空）sqpack dat 块。
    ExpandData {
        main_id: u16,
        sub_id: u16,
        file_id: u32,
        block_offset: u64,
        block_number: u64,
    },
    /// `H` — 写入 sqpack 文件头（1024 字节）。
    Header {
        file_kind: u8,
        header_kind: u8,
        main_id: u16,
        sub_id: u16,
        file_id: u32,
        header_data: Vec<u8>,
    },
    /// `I` — 索引命令（新版 patcher 的 NOP）。
    Index,
    /// `X` — 补丁信息（NOP）。
    PatchInfo,
}

/// 解析后的 ZiPatch chunk。
#[derive(Debug)]
pub enum ZiPatchChunk {
    FileHeader(FileHeaderChunk),
    ApplyOption { option_kind: u32, value: bool },
    Sqpk(SqpkCommand),
    AddDirectory(String),
    DeleteDirectory(String),
    EndOfFile,
    /// 未知/忽略的 chunk（XXXX、APFS 等）。
    Ignored,
}

impl fmt::Display for ZiPatchChunk {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ZiPatchChunk::FileHeader(h) => write!(f, "FHDR:V{}:{}", h.version, h.patch_type),
            ZiPatchChunk::ApplyOption { option_kind, value } => {
                write!(f, "APLY:{}:{}", option_kind, value)
            }
            ZiPatchChunk::Sqpk(cmd) => match cmd {
                SqpkCommand::TargetInfo { platform } => write!(f, "SQPK:T:{}", platform.file_suffix()),
                SqpkCommand::File {
                    operation,
                    file_offset,
                    target_path,
                    ..
                } => write!(
                    f,
                    "SQPK:F:{:?}:{file_offset}:{target_path}",
                    operation
                ),
                SqpkCommand::AddData { block_offset, .. } => write!(f, "SQPK:A:@{block_offset}"),
                SqpkCommand::DeleteData { block_offset, .. } => {
                    write!(f, "SQPK:D:@{block_offset}")
                }
                SqpkCommand::ExpandData { block_offset, .. } => {
                    write!(f, "SQPK:E:@{block_offset}")
                }
                SqpkCommand::Header { header_kind, .. } => {
                    write!(f, "SQPK:H:{}", *header_kind as char)
                }
                SqpkCommand::Index => write!(f, "SQPK:I"),
                SqpkCommand::PatchInfo => write!(f, "SQPK:X"),
            },
            ZiPatchChunk::AddDirectory(dir) => write!(f, "ADIR:{dir}"),
            ZiPatchChunk::DeleteDirectory(dir) => write!(f, "DELD:{dir}"),
            ZiPatchChunk::EndOfFile => write!(f, "EOF_"),
            ZiPatchChunk::Ignored => write!(f, "IGNORED"),
        }
    }
}

/// ZiPatch 解析错误。
#[derive(Debug, thiserror::Error)]
pub enum ZiPatchError {
    #[error("not a valid ZiPatch file (bad magic)")]
    BadMagic,
    #[error("unknown chunk type '{0}'")]
    UnknownChunkType(String),
    #[error("unknown Sqpk command '{0}'")]
    UnknownSqpkCommand(char),
    #[error("SQPK inner size {inner} does not match chunk size {outer}")]
    SqpkSizeMismatch { inner: i32, outer: u32 },
    #[error("unexpected end of patch data")]
    UnexpectedEof,
    #[error("IO error at {path}: {source}")]
    Io {
        path: String,
        #[source]
        source: std::io::Error,
    },
}

/// 简单的二进制读取器（跟踪当前位置）。
struct Reader<'a> {
    data: &'a [u8],
    pos: usize,
}

impl<'a> Reader<'a> {
    fn new(data: &'a [u8]) -> Self {
        Self { data, pos: 0 }
    }

    fn remaining(&self) -> usize {
        self.data.len() - self.pos
    }

    fn read_bytes(&mut self, n: usize) -> Result<&'a [u8], ZiPatchError> {
        if self.remaining() < n {
            return Err(ZiPatchError::UnexpectedEof);
        }
        let slice = &self.data[self.pos..self.pos + n];
        self.pos += n;
        Ok(slice)
    }

    fn read_u8(&mut self) -> Result<u8, ZiPatchError> {
        Ok(self.read_bytes(1)?[0])
    }

    fn read_bool(&mut self) -> Result<bool, ZiPatchError> {
        Ok(self.read_u8()? != 0)
    }

    fn read_u16_be(&mut self) -> Result<u16, ZiPatchError> {
        let b = self.read_bytes(2)?;
        Ok(u16::from_be_bytes([b[0], b[1]]))
    }

    fn read_i16_be(&mut self) -> Result<i16, ZiPatchError> {
        Ok(self.read_u16_be()? as i16)
    }

    fn read_u32_be(&mut self) -> Result<u32, ZiPatchError> {
        let b = self.read_bytes(4)?;
        Ok(u32::from_be_bytes([b[0], b[1], b[2], b[3]]))
    }

    fn read_i32_be(&mut self) -> Result<i32, ZiPatchError> {
        Ok(self.read_u32_be()? as i32)
    }

    fn read_u32_le(&mut self) -> Result<u32, ZiPatchError> {
        let b = self.read_bytes(4)?;
        Ok(u32::from_le_bytes([b[0], b[1], b[2], b[3]]))
    }

    fn read_i32_le(&mut self) -> Result<i32, ZiPatchError> {
        Ok(self.read_u32_le()? as i32)
    }

    fn read_u64_be(&mut self) -> Result<u64, ZiPatchError> {
        let b = self.read_bytes(8)?;
        let mut arr = [0u8; 8];
        arr.copy_from_slice(b);
        Ok(u64::from_be_bytes(arr))
    }

    fn read_i64_be(&mut self) -> Result<i64, ZiPatchError> {
        let b = self.read_bytes(8)?;
        let mut arr = [0u8; 8];
        arr.copy_from_slice(b);
        Ok(i64::from_be_bytes(arr))
    }

    fn read_u64_le(&mut self) -> Result<u64, ZiPatchError> {
        let b = self.read_bytes(8)?;
        let mut arr = [0u8; 8];
        arr.copy_from_slice(b);
        Ok(u64::from_le_bytes(arr))
    }

    /// 读取定长 ASCII 字符串，去掉尾部 NUL（对应 C# `ReadFixedLengthString`）。
    fn read_fixed_string(&mut self, len: u32) -> Result<String, ZiPatchError> {
        let bytes = self.read_bytes(len as usize)?;
        let s = String::from_utf8_lossy(bytes);
        Ok(s.trim_end_matches('\0').to_string())
    }

    fn skip(&mut self, n: usize) -> Result<(), ZiPatchError> {
        self.read_bytes(n).map(|_| ())
    }
}

/// 读取 SqpackFile 公共字段（MainId/SubId/FileId，共 8 字节）。
///
/// 对应 C# `SqpackFile(BinaryReader)` 构造 + 相对路径 `sqpack/{folder}/`。
fn read_sqpack_ids(reader: &mut Reader) -> Result<(u16, u16, u32), ZiPatchError> {
    let main_id = reader.read_u16_be()?;
    let sub_id = reader.read_u16_be()?;
    let file_id = reader.read_u32_be()?;
    Ok((main_id, sub_id, file_id))
}

/// 计算 sqpack 文件在游戏目录中的相对路径。
///
/// 对应 C# `SqpackFile.GetFileName()`：
/// `sqpack/{ffxiv|exN}/{main_id:x2}{sub_id:x4}.{platform}`。
pub fn sqpack_base_path(main_id: u16, sub_id: u16, platform: &Platform) -> String {
    let expansion = (sub_id >> 8) as u32;
    let folder = if expansion == 0 {
        "ffxiv".to_string()
    } else {
        format!("ex{expansion}")
    };
    format!(
        "sqpack/{folder}/{:02x}{:04x}.{}",
        main_id,
        sub_id,
        platform.file_suffix()
    )
}

/// dat 文件完整相对路径。
pub fn sqpack_dat_path(main_id: u16, sub_id: u16, file_id: u32, platform: &Platform) -> String {
    format!(
        "{}.dat{}",
        sqpack_base_path(main_id, sub_id, platform),
        file_id
    )
}

/// index 文件完整相对路径（FileId==0 → `.index`）。
pub fn sqpack_index_path(main_id: u16, sub_id: u16, file_id: u32, platform: &Platform) -> String {
    let suffix = if file_id == 0 {
        String::new()
    } else {
        file_id.to_string()
    };
    format!(
        "{}.index{}",
        sqpack_base_path(main_id, sub_id, platform),
        suffix
    )
}

/// 规范化路径：`\` → `/`，去掉前导 `/`（对应 C# `NormalizePath`）。
pub fn normalize_path(path: &str) -> String {
    let mut p = path.replace('\\', "/");
    while p.starts_with('/') {
        p.remove(0);
    }
    p
}

/// 读取单个 Sqpk 压缩数据块。
///
/// 对应 C# `SqpkCompressedBlock` 构造。
fn read_compressed_block(reader: &mut Reader) -> Result<CompressedBlock, ZiPatchError> {
    let header_size = reader.read_i32_le()?;
    reader.read_u32_le()?; // pad
    let compressed_size = reader.read_i32_le()?;
    let decompressed_size = reader.read_i32_le()?;

    let is_compressed = compressed_size != 0x7d00;
    let block_length = ((if is_compressed {
        compressed_size
    } else {
        decompressed_size
    }) + 143)
        & !0x7F; // C# 0xFFFF_FF80（i32 = -128），清低 7 位

    let data = if is_compressed {
        reader.read_bytes((block_length - header_size) as usize)?.to_vec()
    } else {
        let data = reader.read_bytes(decompressed_size as usize)?.to_vec();
        reader.skip((block_length - header_size - decompressed_size) as usize)?;
        data
    };

    Ok(CompressedBlock {
        header_size,
        compressed_size,
        decompressed_size,
        data,
    })
}

/// 解析 SQPK 命令数据区（不含 innerSize 和 command 字节）。
fn parse_sqpk_command(
    data: &[u8],
    command: char,
    outer_size: u32,
) -> Result<SqpkCommand, ZiPatchError> {
    let mut reader = Reader::new(data);

    let cmd = match command {
        'T' => {
            reader.skip(3)?;
            let platform = Platform::from_u16(reader.read_u16_be()?);
            reader.read_i16_be()?; // Region
            reader.read_i16_be()?; // IsDebug
            reader.read_u16_be()?; // Version
            reader.read_u64_le()?; // DeletedDataSize
            reader.read_u64_le()?; // SeekCount
            // 剩余 32+64 字节空数据跳过
            SqpkCommand::TargetInfo { platform }
        }
        'F' => {
            let operation = match reader.read_u8()? {
                b'A' => FileOperation::AddFile,
                b'R' => FileOperation::RemoveAll,
                b'D' => FileOperation::DeleteFile,
                b'M' => FileOperation::MakeDirTree,
                other => {
                    return Err(ZiPatchError::UnknownSqpkCommand(other as char));
                }
            };
            reader.skip(2)?; // Alignment
            let file_offset = reader.read_i64_be()?;
            let file_size = reader.read_i64_be()?;
            let path_len = reader.read_u32_be()?;
            let expansion_id = reader.read_u16_be()?;
            reader.skip(2)?;
            let target_path = reader.read_fixed_string(path_len)?;

            let mut blocks = Vec::new();
            if operation == FileOperation::AddFile {
                while reader.remaining() > 0 {
                    blocks.push(read_compressed_block(&mut reader)?);
                }
            }

            SqpkCommand::File {
                operation,
                file_offset,
                file_size,
                expansion_id,
                target_path,
                blocks,
            }
        }
        'A' => {
            reader.skip(3)?;
            let (main_id, sub_id, file_id) = read_sqpack_ids(&mut reader)?;
            let block_offset = (reader.read_u32_be()? as u64) << 7;
            let block_number = (reader.read_u32_be()? as u64) << 7;
            let block_delete_number = (reader.read_u32_be()? as u64) << 7;
            let block_data = reader.read_bytes(block_number as usize)?.to_vec();
            SqpkCommand::AddData {
                main_id,
                sub_id,
                file_id,
                block_offset,
                block_number,
                block_delete_number,
                block_data,
            }
        }
        'D' => {
            reader.skip(3)?;
            let (main_id, sub_id, file_id) = read_sqpack_ids(&mut reader)?;
            let block_offset = (reader.read_u32_be()? as u64) << 7;
            let block_number = reader.read_u32_be()? as u64;
            reader.read_u32_le()?; // Reserved
            SqpkCommand::DeleteData {
                main_id,
                sub_id,
                file_id,
                block_offset,
                block_number,
            }
        }
        'E' => {
            reader.skip(3)?;
            let (main_id, sub_id, file_id) = read_sqpack_ids(&mut reader)?;
            let block_offset = (reader.read_u32_be()? as u64) << 7;
            let block_number = reader.read_u32_be()? as u64;
            reader.read_u32_le()?; // Reserved
            SqpkCommand::ExpandData {
                main_id,
                sub_id,
                file_id,
                block_offset,
                block_number,
            }
        }
        'H' => {
            let file_kind = reader.read_u8()?;
            let header_kind = reader.read_u8()?;
            reader.skip(1)?; // Alignment
            let (main_id, sub_id, file_id) = read_sqpack_ids(&mut reader)?;
            let header_data = reader.read_bytes(1024)?.to_vec();
            SqpkCommand::Header {
                file_kind,
                header_kind,
                main_id,
                sub_id,
                file_id,
                header_data,
            }
        }
        'I' => {
            reader.read_u8()?; // IndexCommand
            reader.read_bool()?; // IsSynonym
            reader.skip(1)?;
            read_sqpack_ids(&mut reader)?;
            reader.read_u64_be()?; // FileHash
            reader.read_u32_be()?; // BlockOffset
            reader.read_u32_be()?; // BlockNumber
            SqpkCommand::Index
        }
        'X' => {
            reader.read_u8()?; // Status
            reader.read_u8()?; // Version
            reader.skip(1)?;
            reader.read_u64_be()?; // InstallSize
            SqpkCommand::PatchInfo
        }
        other => return Err(ZiPatchError::UnknownSqpkCommand(other)),
    };

    // 类型检查：innerSize 应该等于 chunk size（SqpkChunk.GetCommand 已校验，这里防呆）
    let _ = outer_size;
    Ok(cmd)
}

/// 解析 SQPK chunk 数据区。
///
/// 布局：[u32 BE innerSize][1 字节 command][innerSize-5 字节命令数据]。
fn parse_sqpk(data: &[u8], outer_size: u32) -> Result<SqpkCommand, ZiPatchError> {
    let mut reader = Reader::new(data);
    let inner_size = reader.read_i32_be()?;
    if inner_size as u32 != outer_size {
        return Err(ZiPatchError::SqpkSizeMismatch {
            inner: inner_size,
            outer: outer_size,
        });
    }
    let command = reader.read_u8()? as char;
    parse_sqpk_command(&data[5..], command, outer_size)
}

/// 解析单个 chunk 的内容（`chunk_type` 为 4 字节类型，`content` 为其后的内容）。
fn parse_chunk_data(chunk_type: &[u8], content: &[u8]) -> Result<ZiPatchChunk, ZiPatchError> {
    let chunk_type = std::str::from_utf8(chunk_type)
        .map_err(|_| ZiPatchError::UnknownChunkType("<non-utf8>".to_string()))?;

    let body = content;

    match chunk_type {
        "FHDR" => {
            let mut reader = Reader::new(body);
            // Version: LE u32 >> 16（C# ReadUInt32() >> 16）
            let version = (reader.read_u32_le()? >> 16) as u8;
            let patch_type = reader.read_fixed_string(4)?;
            let entry_files = reader.read_u32_be()?;

            let mut repository_name = 0;
            let mut commands = 0;
            if version == 3 {
                reader.read_u32_be()?; // AddDirectories
                reader.read_u32_be()?; // DeleteDirectories
                reader.read_u32_be()?; // DeleteDataSize low
                reader.read_u32_be()?; // DeleteDataSize high
                reader.read_u32_be()?; // MinorVersion
                repository_name = reader.read_u32_be()?;
                commands = reader.read_u32_be()?;
                reader.read_u32_be()?; // SqpkAddCommands
                reader.read_u32_be()?; // SqpkDeleteCommands
                reader.read_u32_be()?; // SqpkExpandCommands
                reader.read_u32_be()?; // SqpkHeaderCommands
                reader.read_u32_be()?; // SqpkFileCommands
            }
            // 剩余未知数据忽略

            Ok(ZiPatchChunk::FileHeader(FileHeaderChunk {
                version,
                patch_type,
                entry_files,
                repository_name,
                commands,
            }))
        }
        "APLY" => {
            let mut reader = Reader::new(body);
            let option_kind = reader.read_u32_be()?;
            reader.skip(4)?;
            let raw = reader.read_u32_be()?;
            let value = match option_kind {
                1 | 2 => raw != 0,
                _ => false,
            };
            Ok(ZiPatchChunk::ApplyOption { option_kind, value })
        }
        "SQPK" => Ok(ZiPatchChunk::Sqpk(parse_sqpk(body, body.len() as u32)?)),
        "ADIR" => {
            let mut reader = Reader::new(body);
            let len = reader.read_u32_be()?;
            Ok(ZiPatchChunk::AddDirectory(reader.read_fixed_string(len)?))
        }
        "DELD" => {
            let mut reader = Reader::new(body);
            let len = reader.read_u32_be()?;
            Ok(ZiPatchChunk::DeleteDirectory(reader.read_fixed_string(len)?))
        }
        "EOF_" => Ok(ZiPatchChunk::EndOfFile),
        "XXXX" | "APFS" => Ok(ZiPatchChunk::Ignored),
        other => Err(ZiPatchError::UnknownChunkType(other.to_string())),
    }
}

/// 从文件读取并解析完整的 ZiPatch chunk 列表。
pub fn parse_file(path: &std::path::Path) -> Result<Vec<ZiPatchChunk>, ZiPatchError> {
    let data = std::fs::read(path).map_err(|e| ZiPatchError::Io {
        path: path.display().to_string(),
        source: e,
    })?;
    parse(&data)
}

/// 从内存数据解析完整的 ZiPatch chunk 列表。
pub fn parse(data: &[u8]) -> Result<Vec<ZiPatchChunk>, ZiPatchError> {
    if data.len() < 12 {
        return Err(ZiPatchError::UnexpectedEof);
    }

    // magic：C# 用 LE ReadUInt32 读取，等价于按字节序解析
    for (i, magic) in ZIPATCH_MAGIC.iter().enumerate() {
        let bytes = &data[i * 4..i * 4 + 4];
        let value = u32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]);
        if &value != magic {
            return Err(ZiPatchError::BadMagic);
        }
    }

    let mut pos = 12;
    let mut chunks = Vec::new();

    loop {
        if pos + 8 > data.len() {
            return Err(ZiPatchError::UnexpectedEof);
        }

        // chunk 布局：[u32BE size][类型 4B][内容 size 字节][u32BE checksum]
        // size = 类型之后的内容字节数
        let size = u32::from_be_bytes([data[pos], data[pos + 1], data[pos + 2], data[pos + 3]]);
        pos += 4;

        if pos + 4 + size as usize + 4 > data.len() {
            return Err(ZiPatchError::UnexpectedEof);
        }

        let chunk_type = &data[pos..pos + 4];
        pos += 4;
        let content = &data[pos..pos + size as usize];
        pos += size as usize;

        let chunk = parse_chunk_data(chunk_type, content)?;
        let is_eof = matches!(chunk, ZiPatchChunk::EndOfFile);

        // checksum 字段（needsChecksum=false 时跳过）
        pos += 4;

        chunks.push(chunk);

        if is_eof {
            break;
        }
    }

    Ok(chunks)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_bad_magic() {
        let data = vec![0u8; 64];
        assert!(matches!(parse(&data), Err(ZiPatchError::BadMagic)));
    }

    #[test]
    fn test_normalize_path() {
        assert_eq!(normalize_path("\\sqpack\\ex1\\"), "sqpack/ex1/");
        assert_eq!(normalize_path("/sqpack/ffxiv/"), "sqpack/ffxiv/");
        assert_eq!(normalize_path("movie/ffxiv"), "movie/ffxiv");
    }

    #[test]
    fn test_sqpack_paths() {
        let p = Platform::Win32;
        assert_eq!(
            sqpack_dat_path(0x04, 0x0104, 0, &p),
            "sqpack/ex1/040104.win32.dat0"
        );
        // SubId=0x0000 → ffxiv
        assert_eq!(
            sqpack_dat_path(0x04, 0x0000, 0, &p),
            "sqpack/ffxiv/040000.win32.dat0"
        );
        assert_eq!(
            sqpack_index_path(0x04, 0x0000, 0, &p),
            "sqpack/ffxiv/040000.win32.index"
        );
    }

    /// 构造一个最小的真实补丁：FHDR + SQPK:T + SQPK:F(A) + EOF_
    fn build_small_patch() -> Vec<u8> {
        let mut out = Vec::new();
        // magic (LE 存储)
        for m in ZIPATCH_MAGIC {
            out.extend_from_slice(&m.to_le_bytes());
        }

        // FHDR chunk：内容 = version(LE u32) + patchtype(4) + entryfiles(4) = 12
        let fhdr_body: Vec<u8> = {
            let mut b = Vec::new();
            b.extend_from_slice(&((2u32) << 16).to_le_bytes()); // version 存高 16 位（LE），>>16 后=2
            b.extend_from_slice(b"TEST"); // patch type
            b.extend_from_slice(&0u32.to_be_bytes()); // entry files
            b
        };
        let fhdr_size = fhdr_body.len() as u32;
        out.extend_from_slice(&fhdr_size.to_be_bytes());
        out.extend_from_slice(b"FHDR");
        out.extend_from_slice(&fhdr_body);
        out.extend_from_slice(&0u32.to_be_bytes()); // checksum

        // SQPK:T chunk（内容 = [innerSize][command][body]，innerSize = 内容总大小）
        let sqpk_body: Vec<u8> = {
            let mut b = Vec::new();
            let body_len = 3 + 2 + 2 + 2 + 2 + 8 + 8; // reserved+platform+region+isdebug+version+2×u64
            let inner_size = 4 + 1 + body_len;
            b.extend_from_slice(&(inner_size as i32).to_be_bytes());
            b.push(b'T');
            b.extend_from_slice(&[0u8; 3]); // reserved
            b.extend_from_slice(&0u16.to_be_bytes()); // platform = Win32
            b.extend_from_slice(&(-1i16).to_be_bytes()); // region = Global
            b.extend_from_slice(&0u16.to_be_bytes()); // is_debug
            b.extend_from_slice(&0u16.to_be_bytes()); // version
            b.extend_from_slice(&0u64.to_le_bytes()); // deleted data size
            b.extend_from_slice(&0u64.to_le_bytes()); // seek count
            b
        };
        let sqpk_size = sqpk_body.len() as u32;
        out.extend_from_slice(&sqpk_size.to_be_bytes());
        out.extend_from_slice(b"SQPK");
        out.extend_from_slice(&sqpk_body);
        out.extend_from_slice(&0u32.to_be_bytes());

        // EOF_（内容 32 字节，真实补丁为全 0）
        out.extend_from_slice(&32u32.to_be_bytes());
        out.extend_from_slice(b"EOF_");
        out.extend_from_slice(&[0u8; 32]);
        out.extend_from_slice(&0u32.to_be_bytes());

        out
    }

    #[test]
    fn test_parse_built_patch() {
        let data = build_small_patch();
        let chunks = parse(&data).unwrap();

        assert_eq!(chunks.len(), 3);
        assert!(matches!(&chunks[0], ZiPatchChunk::FileHeader(h) if h.version == 2));
        assert!(matches!(
            &chunks[1],
            ZiPatchChunk::Sqpk(SqpkCommand::TargetInfo { platform: Platform::Win32 })
        ));
        assert!(matches!(chunks[2], ZiPatchChunk::EndOfFile));
    }
}
