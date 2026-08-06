//! SDO 补丁列表（TSV）解析。
//!
//! 对应 C# `PatchListParser.Parse()`：跳过前 5 行，每行 tab 分隔。
//! - 9 字段行（游戏补丁）：`length ... versionId hashType hashBlockSize hashes url`
//! - 6 字段行（boot 补丁，无 hash 信息）：`length ... versionId url`

use xiv_launcher_auth::PatchListEntry;

/// 解析补丁列表文本。
///
/// 与 C# 一致：`START_OFFSET = 5` 跳过前 5 行，最后 2 行忽略（
/// C# 用 `Split('\n')` 保留尾随空串，`len - 2` 截断）。空数据行跳过（比 C# 健壮）。
pub fn parse_patch_list(text: &str) -> Result<Vec<PatchListEntry>, PatchListParseError> {
    // C# StringSplitOptions.None 语义：保留尾随空串
    let lines: Vec<&str> = text.split('\n').collect();

    const START_OFFSET: usize = 5;

    if lines.len() < START_OFFSET + 2 {
        return Err(PatchListParseError::TooShort {
            line_count: lines.len(),
        });
    }

    let mut output = Vec::new();

    for (i, line) in lines.iter().enumerate().take(lines.len() - 2).skip(START_OFFSET) {
        let line = line.trim_end_matches('\r');
        if line.is_empty() {
            continue;
        }
        let fields: Vec<&str> = line.split('\t').collect();

        let length = fields
            .first()
            .ok_or_else(|| PatchListParseError::EmptyLine { line: i + 1 })?
            .parse::<u64>()
            .map_err(|e| PatchListParseError::BadLength {
                line: i + 1,
                value: fields[0].to_string(),
                source: e,
            })?;

        let version = fields
            .get(4)
            .ok_or_else(|| PatchListParseError::FieldCount { line: i + 1 })?
            .to_string();

        // 9 字段 = 游戏补丁（带 hash），6 字段 = boot 补丁（无 hash）
        if fields.len() == 9 {
            let hash_block_size = fields[6]
                .parse::<u64>()
                .map_err(|e| PatchListParseError::BadBlockSize {
                    line: i + 1,
                    value: fields[6].to_string(),
                    source: e,
                })?;
            let hashes = fields[7].split(',').map(|s| s.to_string()).collect();
            output.push(PatchListEntry {
                version,
                url: fields[8].to_string(),
                hash_type: fields[5].to_string(),
                hash_block_size,
                hashes,
                length,
            });
        } else if fields.len() == 6 {
            output.push(PatchListEntry {
                version,
                url: fields[5].to_string(),
                hash_type: String::new(),
                hash_block_size: 0,
                hashes: Vec::new(),
                length,
            });
        } else {
            return Err(PatchListParseError::FieldCount {
                line: i + 1,
            });
        }
    }

    Ok(output)
}

/// 补丁列表解析错误。
#[derive(Debug, thiserror::Error)]
pub enum PatchListParseError {
    #[error("patch list is too short ({line_count} lines, need at least 7)")]
    TooShort { line_count: usize },

    #[error("empty line at line {line}")]
    EmptyLine { line: usize },

    #[error("line {line}: expected 6 or 9 tab-separated fields")]
    FieldCount { line: usize },

    #[error("line {line}: invalid length value '{value}'")]
    BadLength {
        line: usize,
        value: String,
        #[source]
        source: std::num::ParseIntError,
    },

    #[error("line {line}: invalid hash block size '{value}'")]
    BadBlockSize {
        line: usize,
        value: String,
        #[source]
        source: std::num::ParseIntError,
    },
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 模拟真实 SDO 补丁列表（9 字段游戏补丁行）。
    const SAMPLE_GAME_LIST: &str = "\
2012.01.01.0000.0000
2012.01.01.0000.0000
2012.01.01.0000.0000
2012.01.01.0000.0000
2012.01.01.0000.0000
1000\t0\t0\t0\t2024.01.01.0000.0000\tsha1\t8192\tabcdef123456,123456abcdef\t/patch/0001/data
2000\t0\t0\t0\t2024.01.02.0000.0000\tsha1\t8192\t111111,222222\t/patch/0002/data
3000\t0\t0\t0\t2024.01.03.0000.0000\tsha1\t8192\t333333,444444\t/patch/0003/data


";

    /// 6 字段 boot 补丁行。
    const SAMPLE_BOOT_LIST: &str = "\
boot
boot
boot
boot
boot
500\t0\t0\t0\t2024.01.01.0000.0000\t/patch/boot/0001
600\t0\t0\t0\t2024.01.02.0000.0000\t/patch/boot/0002


";

    #[test]
    fn test_parse_game_list() {
        let patches = parse_patch_list(SAMPLE_GAME_LIST).unwrap();
        assert_eq!(patches.len(), 3);

        let first = &patches[0];
        assert_eq!(first.version, "2024.01.01.0000.0000");
        assert_eq!(first.url, "/patch/0001/data");
        assert_eq!(first.hash_type, "sha1");
        assert_eq!(first.hash_block_size, 8192);
        assert_eq!(first.length, 1000);
        assert_eq!(first.hashes, vec!["abcdef123456", "123456abcdef"]);
    }

    #[test]
    fn test_parse_boot_list() {
        let patches = parse_patch_list(SAMPLE_BOOT_LIST).unwrap();
        assert_eq!(patches.len(), 2);
        assert!(patches[0].hashes.is_empty());
        assert_eq!(patches[0].url, "/patch/boot/0001");
    }

    #[test]
    fn test_parse_too_short() {
        assert!(parse_patch_list("a\nb\nc").is_err());
    }
}
