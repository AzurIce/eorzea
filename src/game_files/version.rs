//! 游戏本地版本管理。
//!
//! 对应 C# `Repository.cs` / `Launcher.GetVersionReport()`。
//! 负责从游戏目录读取各 repo（boot / game / ex1-ex5）的版本文件，
//! 并生成 SDO 版本检查请求所需的版本报告。

use std::path::Path;

/// 基础游戏版本（游戏文件缺失/为空时的版本号）。
///
/// 对应 C# `Constants.BASE_GAME_VERSION`。
pub const BASE_GAME_VERSION: &str = "2012.01.01.0000.0000";

/// SDO 版本报告第一行的 boot 哈希。
///
/// 对应 C# `GetBootVersionHash()` —— 国服实现里该值为硬编码（FIXME 注释）。
pub const BOOT_HASH: &str = "ffxivboot.exe/149504/5f2a70612aa58378eb347869e75adeb8f5581a1b";

/// 各 repo 在游戏目录中的相对路径。
///
/// 对应 C# `Repository` 枚举 + `GetRepoPath()`。
pub mod repo {
    pub const BOOT: &str = "boot";
    pub const FFXIV: &str = "game";
    pub const EX1: &str = "game/sqpack/ex1";
    pub const EX2: &str = "game/sqpack/ex2";
    pub const EX3: &str = "game/sqpack/ex3";
    pub const EX4: &str = "game/sqpack/ex4";
    pub const EX5: &str = "game/sqpack/ex5";
}

/// 各 repo 的版本文件名。
///
/// 对应 C# `GetVerFile()`。
pub mod ver_file {
    pub const BOOT: &str = "ffxivboot.ver";
    pub const FFXIV: &str = "ffxivgame.ver";
    pub const EX: &str = "exN.ver";
}

fn ex_ver_name(n: i32) -> String {
    format!("ex{n}.ver")
}

/// 读取单个版本文件。
///
/// 文件不存在或内容为空时返回 `BASE_GAME_VERSION`，对应 C# `GetVer()`。
pub fn read_ver(game_root: &Path, repo_dir: &str, file_name: &str) -> String {
    let path = game_root.join(repo_dir).join(file_name);
    match std::fs::read_to_string(&path) {
        Ok(content) => {
            let trimmed = content.trim();
            if trimmed.is_empty() {
                BASE_GAME_VERSION.to_string()
            } else {
                trimmed.to_string()
            }
        }
        Err(_) => BASE_GAME_VERSION.to_string(),
    }
}

/// 本地各 repo 版本。
#[derive(Debug, Clone, Default)]
pub struct LocalVersions {
    pub boot: String,
    pub ffxiv: String,
    pub ex1: String,
    pub ex2: String,
    pub ex3: String,
    pub ex4: String,
    pub ex5: String,
}

/// 读取游戏目录中所有 repo 的本地版本。
pub fn read_local_versions(game_root: &Path) -> LocalVersions {
    LocalVersions {
        boot: read_ver(game_root, repo::BOOT, ver_file::BOOT),
        ffxiv: read_ver(game_root, repo::FFXIV, ver_file::FFXIV),
        ex1: read_ver(game_root, repo::EX1, &ex_ver_name(1)),
        ex2: read_ver(game_root, repo::EX2, &ex_ver_name(2)),
        ex3: read_ver(game_root, repo::EX3, &ex_ver_name(3)),
        ex4: read_ver(game_root, repo::EX4, &ex_ver_name(4)),
        ex5: read_ver(game_root, repo::EX5, &ex_ver_name(5)),
    }
}

/// 生成 SDO 版本检查请求体。
///
/// 对应 C# `GetVersionReport(gamePath, exLevel, forceBaseVersion)`：
/// 第一行为 boot 哈希，随后每行 `exN\t{version}`（按 `max_expansion` 截断）。
/// `force_base_version` 时所有版本报告为基础版本（用于全新安装/修复）。
pub fn build_version_report(
    game_root: &Path,
    max_expansion: i32,
    force_base_version: bool,
) -> String {
    let versions = read_local_versions(game_root);

    let ver = |local: &str| -> String {
        if force_base_version {
            BASE_GAME_VERSION.to_string()
        } else {
            local.to_string()
        }
    };

    let mut report = String::new();
    report.push_str(BOOT_HASH);
    report.push('\n');

    for n in 1..=max_expansion {
        let local = match n {
            1 => &versions.ex1,
            2 => &versions.ex2,
            3 => &versions.ex3,
            4 => &versions.ex4,
            5 => &versions.ex5,
            _ => BASE_GAME_VERSION,
        };
        report.push_str(&format!("ex{n}\t{}\n", ver(local)));
    }

    report
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU32, Ordering};

    static COUNTER: AtomicU32 = AtomicU32::new(0);

    fn temp_game_root() -> std::path::PathBuf {
        let n = COUNTER.fetch_add(1, Ordering::SeqCst);
        std::env::temp_dir().join(format!("xl-rs-test-{}", n))
    }

    #[test]
    fn test_read_ver_missing_returns_base() {
        let dir = temp_game_root();
        assert_eq!(
            read_ver(&dir, repo::FFXIV, ver_file::FFXIV),
            BASE_GAME_VERSION
        );
    }

    #[test]
    fn test_read_ver_reads_content() {
        let dir = temp_game_root();
        let p = dir.join(repo::FFXIV).join(ver_file::FFXIV);
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(&p, "2025.01.01.0000.0000\n").unwrap();
        assert_eq!(
            read_ver(&dir, repo::FFXIV, ver_file::FFXIV),
            "2025.01.01.0000.0000"
        );
    }

    #[test]
    fn test_build_version_report_empty_game() {
        let dir = temp_game_root();
        let report = build_version_report(&dir, 3, false);
        let lines: Vec<&str> = report.lines().collect();
        assert_eq!(lines[0], BOOT_HASH);
        assert_eq!(lines[1], format!("ex1\t{BASE_GAME_VERSION}"));
        assert_eq!(lines[2], format!("ex2\t{BASE_GAME_VERSION}"));
        assert_eq!(lines[3], format!("ex3\t{BASE_GAME_VERSION}"));
        assert_eq!(lines.len(), 4);
    }

    #[test]
    fn test_build_version_report_force_base() {
        let dir = temp_game_root();
        let p = dir.join(repo::FFXIV).join(ver_file::FFXIV);
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(&p, "2025.01.01.0000.0000").unwrap();

        // force_base_version 时 ex1 也应报告基础版本
        let report = build_version_report(&dir, 1, true);
        assert_eq!(
            report.lines().nth(1).unwrap(),
            format!("ex1\t{BASE_GAME_VERSION}")
        );
    }

    #[test]
    fn test_build_version_report_max_expansion() {
        let dir = temp_game_root();
        let report = build_version_report(&dir, 5, false);
        assert_eq!(report.lines().count(), 6); // boot hash + ex1-ex5
    }
}
