//! `xlcli` — XIV Launcher 命令行工具。
//!
//! 目前包含游戏文件管理子命令（免登录）：
//!
//! ```text
//! xlcli areas                        # 列出所有大区
//! xlcli game status   --game-path …  # 显示本地游戏版本
//! xlcli game check    --game-path … --area …   # 检查更新（列出待下载补丁）
//! xlcli game update   --game-path … --area …   # 下载补丁（暂存，未应用）
//! xlcli game verify   --game-path …            # 完整性校验（未实现）
//! ```
//!
//! `--game-path` 指向游戏**根目录**（含 `boot/`、`game/`、`sdo/`）。

use std::path::PathBuf;
use std::time::Instant;

use clap::{Parser, Subcommand};
use xiv_launcher_auth::sdo::SdoAuth;
use xiv_launcher_rs_lib::game_files::{version, GameFileManager};

#[derive(Parser)]
#[command(
    name = "xlcli",
    version,
    about = "XIV Launcher 命令行工具",
    subcommand_required = true
)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// 列出所有大区（用于查询 --area 的 ID）
    Areas,

    /// 游戏文件管理
    Game {
        #[command(subcommand)]
        sub: GameCommand,
    },
}

#[derive(Subcommand)]
enum GameCommand {
    /// 显示游戏目录的本地版本
    Status {
        /// 游戏根目录（含 boot/、game/、sdo/）
        #[arg(long)]
        game_path: PathBuf,
    },

    /// 检查更新：版本报告 → 补丁服务器 → 待下载补丁列表
    Check {
        /// 游戏根目录
        #[arg(long)]
        game_path: PathBuf,

        /// 大区 ID（用 `xlcli areas` 查看）
        #[arg(long)]
        area: String,

        /// 版本报告包含的资料片数量（默认 5，对齐 C# Constants.MaxExpansion）
        #[arg(long, default_value_t = 5)]
        max_expansion: i32,

        /// 报告基础版本（全新安装/修复，会返回全部补丁）
        #[arg(long)]
        repair: bool,
    },

    /// 下载待更新补丁到暂存目录（不应用）
    Update {
        /// 游戏根目录
        #[arg(long)]
        game_path: PathBuf,

        /// 大区 ID（用 `xlcli areas` 查看）
        #[arg(long)]
        area: String,

        /// 版本报告包含的资料片数量
        #[arg(long, default_value_t = 5)]
        max_expansion: i32,

        /// 报告基础版本（全新安装/修复）
        #[arg(long)]
        repair: bool,

        /// 补丁暂存目录（默认 ~/.xiv-launcher-rs/patches）
        #[arg(long)]
        patch_dir: Option<PathBuf>,

        /// 并发下载数（默认 4）
        #[arg(long, default_value_t = 4)]
        concurrency: usize,
    },

    /// 校验游戏文件完整性（尚未实现）
    Verify {
        /// 游戏根目录
        #[arg(long)]
        game_path: PathBuf,
    },
}

fn human_bytes(bytes: u64) -> String {
    const KB: f64 = 1024.0;
    const MB: f64 = KB * 1024.0;
    const GB: f64 = MB * 1024.0;
    let b = bytes as f64;
    if b >= GB {
        format!("{:.2} GiB", b / GB)
    } else if b >= MB {
        format!("{:.2} MiB", b / MB)
    } else if b >= KB {
        format!("{:.2} KiB", b / KB)
    } else {
        format!("{bytes} B")
    }
}

#[tokio::main]
async fn main() {
    tracing_subscriber::fmt::init();

    let cli = Cli::parse();

    match cli.command {
        Command::Areas => cmd_areas().await,
        Command::Game { sub } => match sub {
            GameCommand::Status { game_path } => cmd_status(&game_path),
            GameCommand::Check {
                game_path,
                area,
                max_expansion,
                repair,
            } => cmd_check(&game_path, &area, max_expansion, repair).await,
            GameCommand::Update {
                game_path,
                area,
                max_expansion,
                repair,
                patch_dir,
                concurrency,
            } => {
                cmd_update(
                    &game_path,
                    &area,
                    max_expansion,
                    repair,
                    patch_dir,
                    concurrency,
                )
                .await
            }
            GameCommand::Verify { game_path: _ } => {
                eprintln!("尚未实现：完整性校验依赖 IndexedZiPatch 格式，见 src/game_files 模块说明。");
            }
        },
    }
}

async fn cmd_areas() {
    match SdoAuth::fetch_server_list().await {
        Ok(areas) => {
            println!("=== 大区列表 ===");
            for a in &areas {
                println!(
                    "  [{}] {} (lobby: {}, patch: {})",
                    a.area_id, a.area_name, a.area_lobby, a.area_patch
                );
            }
        }
        Err(e) => {
            eprintln!("获取大区列表失败: {e}");
            std::process::exit(1);
        }
    }
}

fn cmd_status(game_path: &std::path::Path) {
    let mgr = GameFileManager::new();
    let v = mgr.status(game_path);

    println!("=== 本地版本 ({}) ===", game_path.display());
    println!("  boot:  {}", v.boot);
    println!("  ffxiv: {}", v.ffxiv);
    for (n, ver) in [
        (1, &v.ex1),
        (2, &v.ex2),
        (3, &v.ex3),
        (4, &v.ex4),
        (5, &v.ex5),
    ] {
        let installed = ver != version::BASE_GAME_VERSION;
        println!("  ex{n}:   {ver}{}", if installed { "" } else { " (未安装)" });
    }
}

async fn find_area(area_id: &str) -> Result<xiv_launcher_auth::SdoArea, String> {
    let areas = SdoAuth::fetch_server_list()
        .await
        .map_err(|e| format!("获取大区列表失败: {e}"))?;
    areas
        .into_iter()
        .find(|a| a.area_id == area_id)
        .ok_or_else(|| format!("找不到大区 ID '{area_id}'，用 `xlcli areas` 查看"))
}

async fn cmd_check(
    game_path: &std::path::Path,
    area_id: &str,
    max_expansion: i32,
    repair: bool,
) {
    let mgr = GameFileManager::new();
    let area = match find_area(area_id).await {
        Ok(a) => a,
        Err(e) => {
            eprintln!("{e}");
            std::process::exit(1);
        }
    };

    println!(
        "=== 检查更新: {} ({}) ===",
        area.area_name, game_path.display()
    );

    match mgr
        .check_update(&area, game_path, repair, max_expansion)
        .await
    {
        Ok(xiv_launcher_rs_lib::game_files::CheckResult::UpToDate { unique_id }) => {
            println!("游戏已是最新版本。 (X-Patch-Unique-Id: {})", unique_id);
        }
        Ok(xiv_launcher_rs_lib::game_files::CheckResult::NeedsPatch {
            patches,
            unique_id,
        }) => {
            let total: u64 = patches.iter().map(|p| p.length).sum();
            println!("需要下载 {} 个补丁，共 {}:", patches.len(), human_bytes(total));
            for p in &patches {
                println!(
                    "  {:>10}  {}  {}",
                    human_bytes(p.length),
                    p.version,
                    p.url
                );
            }
            println!("(X-Patch-Unique-Id: {})", unique_id);
        }
        Ok(xiv_launcher_rs_lib::game_files::CheckResult::NeedsPatchBoot) => {
            println!("服务器指示 boot 需要更新（国服通常不会出现）。");
        }
        Err(e) => {
            eprintln!("检查更新失败: {e}");
            std::process::exit(1);
        }
    }
}

async fn cmd_update(
    game_path: &std::path::Path,
    area_id: &str,
    max_expansion: i32,
    repair: bool,
    patch_dir: Option<PathBuf>,
    concurrency: usize,
) {
    let mgr = GameFileManager::new();
    let area = match find_area(area_id).await {
        Ok(a) => a,
        Err(e) => {
            eprintln!("{e}");
            std::process::exit(1);
        }
    };

    let patch_dir = patch_dir.unwrap_or_else(|| {
        dirs::home_dir()
            .map(|h| h.join(".xiv-launcher-rs/patches"))
            .unwrap_or_else(|| PathBuf::from("./patches"))
    });

    println!(
        "=== 更新: {} ({}) ===",
        area.area_name, game_path.display()
    );

    let check = match mgr
        .check_update(&area, game_path, repair, max_expansion)
        .await
    {
        Ok(c) => c,
        Err(e) => {
            eprintln!("检查更新失败: {e}");
            std::process::exit(1);
        }
    };

    let patches = match check {
        xiv_launcher_rs_lib::game_files::CheckResult::UpToDate { .. } => {
            println!("游戏已是最新版本，无需更新。");
            return;
        }
        xiv_launcher_rs_lib::game_files::CheckResult::NeedsPatch { patches, .. } => patches,
        xiv_launcher_rs_lib::game_files::CheckResult::NeedsPatchBoot => {
            eprintln!("boot 需要更新（国服通常不会出现）。");
            std::process::exit(1);
        }
    };

    let total: u64 = patches.iter().map(|p| p.length).sum();
    println!(
        "共 {} 个补丁、{}，下载到 {}",
        patches.len(),
        human_bytes(total),
        patch_dir.display()
    );

    let start = Instant::now();
    let mut last_tick = Instant::now();
    let mut last_bytes = 0u64;

    match mgr
        .download(&patches, &patch_dir, concurrency, |done, total| {
            let now = Instant::now();
            if now.duration_since(last_tick) >= std::time::Duration::from_secs(2) || done == total {
                let dt = now.duration_since(last_tick).as_secs_f64().max(0.001);
                let speed = (done - last_bytes) as f64 / dt;
                println!(
                    "  进度 {}/{} ({:.1}%), 速度 {}/s",
                    human_bytes(done),
                    human_bytes(total),
                    done as f64 / total as f64 * 100.0,
                    human_bytes(speed as u64)
                );
                last_tick = now;
                last_bytes = done;
            }
        })
        .await
    {
        Ok(summary) => {
            let elapsed = start.elapsed().as_secs_f64();
            println!(
                "下载完成: {} 个新下载, {} 个已存在跳过, 用时 {:.1}s",
                summary.downloaded.len(),
                summary.skipped,
                elapsed
            );
            println!("开始应用补丁...");
            match mgr.install(&patches, &patch_dir, game_path).await {
                Ok(install_summary) => {
                    println!(
                        "安装完成: {} 个补丁已应用, {} 个跳过（缺文件）",
                        install_summary.installed.len(),
                        install_summary.skipped
                    );
                    if !install_summary.installed.is_empty() {
                        println!("游戏已更新，版本文件已同步。");
                    }
                }
                Err(e) => {
                    eprintln!("安装失败: {e}");
                    std::process::exit(1);
                }
            }
        }
        Err(e) => {
            eprintln!("下载失败: {e}");
            std::process::exit(1);
        }
    }
}
