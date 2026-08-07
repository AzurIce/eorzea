//! 游戏文件完整性校验（国服实用版）。
//!
//! 上游 C# 的 `PatchVerifier` 依赖 ottercorp S3 的 `.patch.index`（IndexedZiPatch
//! 索引）——那是**国际服专用**服务（`latest.json` 为 SE 版本，国服文件 404），
//! 国服无法使用。本模块改为**文件级完整性检查**，覆盖常见损坏/缺失场景：
//!
//! - `.ver` 版本文件存在且非空（`boot`/`game`/`ex1-ex5`）
//! - 关键可执行文件/认证 DLL 存在（`ffxiv_dx11.exe`、`sdologinentry64.dll`）
//! - 每个 sqpack 仓库的 `*.dat*`/`*.index*` 文件存在、魔数正确（`SqPack\0\0`）、
//!   header size 合法、大小非零
//! - `movie` 目录存在
//!
//! 不检查内容哈希（国服无 hash 基准服务）；发现的问题返回列表供上层展示/修复。

use std::path::Path;
use tracing::{debug, instrument};

use super::version;

/// 问题严重级别。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IssueSeverity {
    /// 文件缺失（需要重新更新/修复）。
    Missing,
    /// 文件损坏（魔数/结构异常）。
    Corrupt,
    /// 警告（版本缺失等，可能正常）。
    Warning,
}

/// 单个问题。
#[derive(Debug, Clone)]
pub struct GameFileIssue {
    pub severity: IssueSeverity,
    /// 相对游戏根目录的路径。
    pub path: String,
    pub message: String,
}

/// SqPack 文件魔数（dat/index 通用）。
const SQPACK_MAGIC: &[u8; 8] = b"SqPack\x00\x00";

/// 检查单个 sqpack 文件。
///
/// 布局（SqPack 格式）：
/// - `.dat0` / `.index` / `.index2`：以 `SqPack\0\0` 魔数开头，header size 在 offset 12
/// - `.dat1`+：无魔数头，是纯数据块（首块头 u32 @0，应为 128 的倍数）
fn check_sqpack_file(
    path: &Path,
    relative: &str,
    is_first_data: bool, // `.dat0` 或 `.index*` 有魔数头
    issues: &mut Vec<GameFileIssue>,
) {
    let meta = match std::fs::metadata(path) {
        Ok(m) => m,
        Err(_) => {
            issues.push(GameFileIssue {
                severity: IssueSeverity::Missing,
                path: relative.to_string(),
                message: "文件缺失".to_string(),
            });
            return;
        }
    };

    if meta.len() == 0 {
        issues.push(GameFileIssue {
            severity: IssueSeverity::Corrupt,
            path: relative.to_string(),
            message: "文件大小为 0".to_string(),
        });
        return;
    }

    let mut buf = [0u8; 32];
    let read = match std::fs::File::open(path).and_then(|mut f| {
        use std::io::Read;
        f.read(&mut buf)
    }) {
        Ok(n) => n,
        Err(e) => {
            issues.push(GameFileIssue {
                severity: IssueSeverity::Corrupt,
                path: relative.to_string(),
                message: format!("读取失败: {e}"),
            });
            return;
        }
    };

    if read < 16 {
        issues.push(GameFileIssue {
            severity: IssueSeverity::Corrupt,
            path: relative.to_string(),
            message: "文件过短（<16 字节）".to_string(),
        });
        return;
    }

    if is_first_data {
        // 有魔数头：`.dat0` / `.index*`
        if &buf[..8] != SQPACK_MAGIC {
            issues.push(GameFileIssue {
                severity: IssueSeverity::Corrupt,
                path: relative.to_string(),
                message: format!("SqPack 魔数错误: {:02x?}", &buf[..8]),
            });
            return;
        }
        // header size 在 offset 12（u32 LE），合法值通常为 0x400 或 0x800
        let header_size = u32::from_le_bytes([buf[12], buf[13], buf[14], buf[15]]);
        if header_size != 0x400 && header_size != 0x800 {
            issues.push(GameFileIssue {
                severity: IssueSeverity::Corrupt,
                path: relative.to_string(),
                message: format!("header size 异常: 0x{header_size:x}"),
            });
        }
    } else {
        // `.dat1`+：可能是独立 SqPack 文件（有魔数头），也可能是前一个 dat 的延续数据块
        if &buf[..8] == SQPACK_MAGIC {
            let header_size = u32::from_le_bytes([buf[12], buf[13], buf[14], buf[15]]);
            if header_size != 0x400 && header_size != 0x800 {
                issues.push(GameFileIssue {
                    severity: IssueSeverity::Corrupt,
                    path: relative.to_string(),
                    message: format!("header size 异常: 0x{header_size:x}"),
                });
            }
        } else {
            // 延续数据块：首块头 u32 @0，应为 128 的倍数且非零
            let block_size = u32::from_le_bytes([buf[0], buf[1], buf[2], buf[3]]);
            if block_size == 0 || block_size % 128 != 0 {
                issues.push(GameFileIssue {
                    severity: IssueSeverity::Corrupt,
                    path: relative.to_string(),
                    message: format!("数据块头异常: 0x{block_size:x}"),
                });
            }
        }
    }
}

/// 检查一个 sqpack 仓库目录（如 `game/sqpack/ffxiv`、`game/sqpack/ex1`）。
fn check_sqpack_repo(
    repo_dir: &Path,
    relative: &str,
    issues: &mut Vec<GameFileIssue>,
) {
    let entries = match std::fs::read_dir(repo_dir) {
        Ok(e) => e,
        Err(_) => {
            issues.push(GameFileIssue {
                severity: IssueSeverity::Missing,
                path: relative.to_string(),
                message: "sqpack 仓库目录缺失".to_string(),
            });
            return;
        }
    };

    let mut dat_count = 0;
    for entry in entries.flatten() {
        let name = entry.file_name().to_string_lossy().to_string();
        // 只检查 .dat* 和 .index* 文件（跳过 .ver、.bck、临时文件）
        if let Some(idx) = name.find(".win32.dat") {
            // 形如 010000.win32.dat0 / .dat1 / .dat2 ...
            let suffix = &name[idx + ".win32.dat".len()..];
            let is_first = suffix.parse::<u32>().map(|n| n == 0).unwrap_or(false);
            check_sqpack_file(&entry.path(), &format!("{relative}/{name}"), is_first, issues);
            dat_count += 1;
        } else if name.contains(".win32.index") {
            check_sqpack_file(&entry.path(), &format!("{relative}/{name}"), true, issues);
        }
    }

    debug!(repo = relative, dat_files = dat_count, "sqpack repo checked");
}

/// 执行游戏文件完整性校验，返回问题列表。
///
/// `game_root` 为游戏根目录（含 `game/`、`sdo/`）。`max_expansion` 控制检查到哪个
/// 资料片（默认 5）。
#[instrument(skip(game_root))]
pub fn verify_game(game_root: &Path, max_expansion: i32) -> Vec<GameFileIssue> {
    let mut issues = Vec::new();

    // 1. .ver 版本文件
    for (repo_dir, ver_file, label) in [
        (version::repo::FFXIV, version::ver_file::FFXIV, "game"),
        (version::repo::EX1, "ex1.ver", "ex1"),
        (version::repo::EX2, "ex2.ver", "ex2"),
        (version::repo::EX3, "ex3.ver", "ex3"),
        (version::repo::EX4, "ex4.ver", "ex4"),
        (version::repo::EX5, "ex5.ver", "ex5"),
    ] {
        let exp_no: i32 = match label {
            "ex1" => 1,
            "ex2" => 2,
            "ex3" => 3,
            "ex4" => 4,
            _ => 5,
        };
        if exp_no > max_expansion {
            continue;
        }
        let p = game_root.join(repo_dir).join(ver_file);
        let rel = format!("{repo_dir}/{ver_file}");
        match std::fs::read_to_string(&p) {
            Ok(content) if !content.trim().is_empty() => {
                debug!(path = rel, ver = content.trim(), "version ok");
            }
            Ok(_) => issues.push(GameFileIssue {
                severity: IssueSeverity::Warning,
                path: rel,
                message: "版本文件为空".to_string(),
            }),
            Err(_) => issues.push(GameFileIssue {
                severity: IssueSeverity::Warning,
                path: rel,
                message: "版本文件缺失（未安装该部分）".to_string(),
            }),
        }
    }

    // 2. 关键文件
    let key_files = [
        ("game/ffxiv_dx11.exe", "游戏主程序"),
        ("sdo/sdologin/sdologinentry64.dll", "登录认证 DLL"),
    ];
    for (rel, desc) in key_files {
        if !game_root.join(rel).exists() {
            issues.push(GameFileIssue {
                severity: IssueSeverity::Missing,
                path: rel.to_string(),
                message: format!("{desc}缺失"),
            });
        }
    }

    // 3. sqpack 仓库
    check_sqpack_repo(
        &game_root.join("game/sqpack/ffxiv"),
        "game/sqpack/ffxiv",
        &mut issues,
    );
    for n in 1..=max_expansion {
        let rel = format!("game/sqpack/ex{n}");
        check_sqpack_repo(&game_root.join(&rel), &rel, &mut issues);
    }

    // 4. movie 目录（存在性）
    if !game_root.join("game/movie").exists() {
        issues.push(GameFileIssue {
            severity: IssueSeverity::Warning,
            path: "game/movie".to_string(),
            message: "movie 目录缺失".to_string(),
        });
    }

    issues
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU32, Ordering};

    static COUNTER: AtomicU32 = AtomicU32::new(0);

    fn temp_root() -> std::path::PathBuf {
        let n = COUNTER.fetch_add(1, Ordering::SeqCst);
        std::env::temp_dir().join(format!("xl-rs-verify-{}-{}", std::process::id(), n))
    }

    fn write_sqpack(path: &std::path::Path, valid: bool) {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        let data = if valid {
            let mut d = vec![0u8; 1024 + 32];
            d[..8].copy_from_slice(SQPACK_MAGIC);
            d[12..16].copy_from_slice(&0x400u32.to_le_bytes()); // header size @12
            d
        } else {
            b"not a sqpack file".to_vec()
        };
        std::fs::write(path, data).unwrap();
    }

    #[test]
    fn test_verify_ok() {
        let root = temp_root();
        // 版本文件
        std::fs::create_dir_all(root.join("game/sqpack/ex1")).unwrap();
        std::fs::write(root.join("game/ffxivgame.ver"), "2026.07.16.0001.0000").unwrap();
        std::fs::write(root.join("game/sqpack/ex1/ex1.ver"), "2026.07.03.0000.0000").unwrap();
        // 关键文件
        std::fs::create_dir_all(root.join("sdo/sdologin")).unwrap();
        std::fs::write(root.join("game/ffxiv_dx11.exe"), b"exe").unwrap();
        std::fs::write(root.join("sdo/sdologin/sdologinentry64.dll"), b"dll").unwrap();
        // sqpack
        write_sqpack(&root.join("game/sqpack/ffxiv/000000.win32.dat0"), true);
        write_sqpack(&root.join("game/sqpack/ffxiv/000000.win32.index"), true);
        write_sqpack(&root.join("game/sqpack/ex1/000000.win32.dat0"), true);
        // movie
        std::fs::create_dir_all(root.join("game/movie")).unwrap();

        let issues = verify_game(&root, 1);
        let severe: Vec<_> = issues
            .iter()
            .filter(|i| i.severity != IssueSeverity::Warning)
            .collect();
        assert!(severe.is_empty(), "unexpected issues: {issues:?}");
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn test_verify_detects_missing_and_corrupt() {
        let root = temp_root();
        // 空游戏目录
        std::fs::create_dir_all(&root).unwrap();

        let issues = verify_game(&root, 1);
        // 主程序缺失
        assert!(issues.iter().any(|i| i.path.contains("ffxiv_dx11.exe")
            && i.severity == IssueSeverity::Missing));
        // sqpack 仓库缺失
        assert!(issues
            .iter()
            .any(|i| i.path.contains("sqpack/ffxiv") && i.severity == IssueSeverity::Missing));
        // 损坏的 dat（魔数错）
        std::fs::create_dir_all(root.join("game/sqpack/ffxiv")).unwrap();
        std::fs::write(root.join("game/sqpack/ffxiv/000000.win32.dat0"), b"garbage").unwrap();
        let issues2 = verify_game(&root, 1);
        assert!(issues2.iter().any(|i| i.path.contains("000000.win32.dat0")
            && i.severity == IssueSeverity::Corrupt));
        let _ = std::fs::remove_dir_all(&root);
    }
}
