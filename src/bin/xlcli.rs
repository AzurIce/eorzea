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

use std::io::{self, Write};
use std::path::PathBuf;
use std::time::Instant;

use clap::{Parser, Subcommand};
use xiv_launcher_auth::sdo::SdoAuth;
use xiv_launcher_rs_lib::game_files::{version, GameFileManager};
use xiv_launcher_rs_lib::launcher::{Launcher, LauncherError};
use xiv_launcher_rs_lib::term_img;

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

    /// 账号管理（多账号 + 默认账号，配置持久化到 auth.toml）
    Auth {
        #[command(subcommand)]
        sub: AuthCommand,
    },

    /// 登录并启动游戏
    Launch {
        /// 游戏根目录（含 boot/、game/、sdo/）
        #[arg(long)]
        game_path: PathBuf,

        /// 大区 ID（用 `xlcli areas` 查看）
        #[arg(long)]
        area: String,

        /// 指定账号（snda_id 或 username，配置中已保存 session key 时直接自动登录）
        #[arg(long)]
        account: Option<String>,

        /// 登录方式: password | qr | auto（未指定账号/默认账号时使用）
        #[arg(long)]
        method: Option<String>,

        /// 密码登录的账号名
        #[arg(long)]
        username: Option<String>,

        /// 自定义 Wine 路径
        #[arg(long)]
        wine: Option<PathBuf>,

        /// 二维码保存路径（qr 方式，默认 ~/xiv_qr.png）
        #[arg(long)]
        qr_file: Option<PathBuf>,

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

#[derive(Subcommand)]
enum AuthCommand {
    /// 登录并保存账号（qr | password | auto）
    Login {
        /// 登录方式: qr | password | auto
        #[arg(value_enum)]
        method: AuthMethod,

        /// 密码登录的账号名
        #[arg(long)]
        username: Option<String>,

        /// 自动登录 session key（auto 方式，缺省时交互输入）
        #[arg(long)]
        session_key: Option<String>,

        /// 二维码保存路径（qr 方式，默认 ~/xiv_qr.png）
        #[arg(long)]
        qr_file: Option<PathBuf>,

    },

    /// 显示已保存的账号和默认账号
    Status {},

    /// 设置默认账号
    Default {
        /// 账号（snda_id 或 username）
        account: String,
    },

    /// 删除账号（缺省删除默认账号）
    Logout {
        /// 账号（snda_id 或 username）
        #[arg(long)]
        account: Option<String>,
    },
}

#[derive(clap::ValueEnum, Clone, Debug)]
enum AuthMethod {
    Qr,
    Password,
    Auto,
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
        Command::Auth { sub } => match sub {
            AuthCommand::Login {
                method,
                username,
                session_key,
                qr_file,
            } => {
                cmd_auth_login(
                    method,
                    username.as_deref(),
                    session_key.as_deref(),
                    qr_file,
                )
                .await
            }
            AuthCommand::Status {} => cmd_auth_status(),
            AuthCommand::Default { account } => cmd_auth_default(&account),
            AuthCommand::Logout { account } => {
                cmd_auth_logout(account.as_deref())
            }
        },
        Command::Launch {
            game_path,
            area,
            account,
            method,
            username,
            wine,
            qr_file,
        } => {
            cmd_launch(
                &game_path,
                &area,
                account.as_deref(),
                method.as_deref(),
                username.as_deref(),
                wine.as_deref(),
                qr_file,
            )
            .await;
        }
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

fn prompt(label: &str) -> String {
    print!("{label}: ");
    io::stdout().flush().unwrap();
    let mut buf = String::new();
    io::stdin().read_line(&mut buf).unwrap();
    buf.trim().to_string()
}

/// 请求并显示扫码二维码（终端图片协议优先，fallback 保存文件）。
async fn show_qr_code(
    launcher: &Launcher,
    qr_file: Option<PathBuf>,
) -> Result<xiv_launcher_rs_lib::launcher::QrCodeSession, LauncherError> {
    let qr = launcher.request_qr_code().await?;

    let path = qr_file.unwrap_or_else(|| {
        dirs::home_dir()
            .map(|h| h.join("xiv_qr.png"))
            .unwrap_or_else(|| PathBuf::from("xiv_qr.png"))
    });
    std::fs::write(&path, qr.image_data()).expect("failed to save QR image");
    println!("二维码已保存到: {} ({} bytes)", path.display(), qr.image_data().len());

    // 终端直接显示图片（kitty / iTerm2）
    match term_img::display_png(qr.image_data()) {
        Ok(()) => println!("↑ 请用叨鱼 App 扫码"),
        Err(e) => {
            println!("(终端不支持图片显示: {e}，请打开上面的图片文件扫码)");
        }
    }

    Ok(qr)
}

/// 执行登录（不保存账号），返回 LaunchToken。
async fn do_login(
    method: &AuthMethod,
    username: Option<&str>,
    session_key: Option<&str>,
    qr_file: Option<PathBuf>,
) -> Result<xiv_launcher_rs_lib::launcher::LaunchToken, LauncherError> {
    let launcher = Launcher::new()?;
    println!("设备指纹: {}", launcher.device_id());

    match method {
        AuthMethod::Password => {
            let account = username.map(|s| s.to_string()).unwrap_or_else(|| prompt("账号"));
            println!("密码（不显示输入）:");
            let password = rpassword::read_password().unwrap();
            launcher.login_password(&account, &password).await
        }
        AuthMethod::Qr => {
            let qr = show_qr_code(&launcher, qr_file).await?;
            println!("等待扫码（300 秒超时）...");
            qr.wait_for_scan(Some(std::time::Duration::from_secs(300))).await
        }
        AuthMethod::Auto => {
            let key = session_key
                .map(|s| s.to_string())
                .unwrap_or_else(|| prompt("Auto-login session key"));
            launcher.login_auto(&key).await
        }
    }
}

/// `auth login`：登录成功后保存账号到配置。
async fn cmd_auth_login(
    method: AuthMethod,
    username: Option<&str>,
    session_key: Option<&str>,
    qr_file: Option<PathBuf>,
) {
    let token = match do_login(&method, username, session_key, qr_file).await {
        Ok(t) => t,
        Err(e) => {
            eprintln!("登录失败: {e}");
            std::process::exit(1);
        }
    };

    // 保存账号
    let cfg_path = xiv_launcher_rs_lib::auth::config_path();
    let mut cfg = xiv_launcher_rs_lib::auth::load(&cfg_path);
    let make_default = cfg.accounts.is_empty(); // 第一个账号自动设为默认
    let username = username
        .map(|s| s.to_string())
        .or_else(|| token.username.clone());
    cfg.upsert(
        xiv_launcher_rs_lib::auth::Account {
            snda_id: token.snda_id.clone(),
            username,
            auto_login_session_key: token.auto_login_session_key.clone(),
        },
        make_default,
    );
    if let Err(e) = xiv_launcher_rs_lib::auth::save(&cfg_path, &cfg) {
        eprintln!("保存配置失败: {e}");
        std::process::exit(1);
    }

    println!("\n登录成功! 账号已保存到 {}", cfg_path.display());
    println!("  snda_id:      {}", token.snda_id);
    println!("  session_key:  {}", token.auto_login_session_key.as_deref().unwrap_or("(无)"));
    // 显示真实默认状态：第一个账号自动默认，或已是配置中的默认账号
    let is_default = cfg
        .default_account()
        .map(|a| a.snda_id == token.snda_id)
        .unwrap_or(false);
    println!(
        "  默认账号:      {}",
        if is_default { "是" } else { "否" }
    );
}

/// `auth status`：显示已保存账号。
fn cmd_auth_status() {
    let cfg_path = xiv_launcher_rs_lib::auth::config_path();
    let cfg = xiv_launcher_rs_lib::auth::load(&cfg_path);

    println!("=== 已保存账号 ({}) ===", cfg_path.display());
    if cfg.accounts.is_empty() {
        println!("（无）\n用 `xlcli auth login qr` 登录一个账号。");
        return;
    }
    for acc in &cfg.accounts {
        // 默认标记：default_account 可能是 username 或 snda_id，统一解析后按 snda_id 比较
        let default = cfg
            .default_account()
            .map(|a| a.snda_id == acc.snda_id)
            .unwrap_or(false);
        println!(
            "  {}{}  snda_id={}  auto={}",
            if default { "[默认] " } else { "      " },
            acc.display_name(),
            acc.snda_id,
            if acc.can_auto_login() { "✅" } else { "无 session key" }
        );
    }
    if cfg.default_account.is_none() {
        println!("（未设置默认账号，用 `xlcli auth default <账号>` 设置）");
    }
}

/// `auth default`：设置默认账号。
fn cmd_auth_default(account: &str) {
    let cfg_path = xiv_launcher_rs_lib::auth::config_path();
    let mut cfg = xiv_launcher_rs_lib::auth::load(&cfg_path);

    // 支持 snda_id 或 username 匹配；存储时 username 优先（可读），无则 snda_id
    let found = cfg.find_by_identifier(account).cloned();

    match found {
        Some(acc) => {
            cfg.default_account = Some(
                acc.username.clone().unwrap_or_else(|| acc.snda_id.clone()),
            );
            if let Err(e) = xiv_launcher_rs_lib::auth::save(&cfg_path, &cfg) {
                eprintln!("保存配置失败: {e}");
                std::process::exit(1);
            }
            println!("默认账号已设为: {} ({})", account, cfg_path.display());
        }
        None => {
            eprintln!("找不到账号 '{account}'，用 `xlcli auth status` 查看已保存账号。");
            std::process::exit(1);
        }
    }
}

/// `auth logout`：删除账号（缺省删默认账号）。
fn cmd_auth_logout(account: Option<&str>) {
    let cfg_path = xiv_launcher_rs_lib::auth::config_path();
    let mut cfg = xiv_launcher_rs_lib::auth::load(&cfg_path);

    let target = match account {
        Some(a) => a.to_string(),
        None => match cfg.default_account.clone() {
            Some(id) => id,
            None => {
                eprintln!("没有默认账号，请指定 `--account`。");
                std::process::exit(1);
            }
        },
    };

    let removed = cfg.remove(&target);
    if !removed {
        // 试试 username 匹配（remove 只认 snda_id）
        let by_name = cfg.find_by_identifier(&target).map(|a| a.snda_id.clone());
        match by_name {
            Some(id) => {
                cfg.remove(&id);
                let _ = xiv_launcher_rs_lib::auth::save(&cfg_path, &cfg);
                println!("已删除账号: {target} ({})", cfg_path.display());
            }
            None => {
                eprintln!("找不到账号 '{target}'。");
                std::process::exit(1);
            }
        }
    } else {
        if let Err(e) = xiv_launcher_rs_lib::auth::save(&cfg_path, &cfg) {
            eprintln!("保存配置失败: {e}");
            std::process::exit(1);
        }
        println!("已删除账号: {target} ({})", cfg_path.display());
    }
}

#[allow(clippy::too_many_arguments)]
async fn cmd_launch(
    game_path: &std::path::Path,
    area_id: &str,
    account: Option<&str>,
    method: Option<&str>,
    username: Option<&str>,
    wine: Option<&std::path::Path>,
    qr_file: Option<PathBuf>,
) {
    // 确定登录方式：
    // 1. --account 指定（或配置默认账号）且该账号有 session key → auto 登录
    // 2. --method 手动登录
    let cfg_path = xiv_launcher_rs_lib::auth::config_path();
    let cfg = xiv_launcher_rs_lib::auth::load(&cfg_path);

    let token = if let Some(m) = method {
        let method_enum = match m {
            "password" => AuthMethod::Password,
            "qr" => AuthMethod::Qr,
            "auto" => AuthMethod::Auto,
            other => {
                eprintln!("未知登录方式 '{other}'，可选: password | qr | auto");
                std::process::exit(1);
            }
        };
        match do_login(&method_enum, username, None, qr_file).await {
            Ok(t) => t,
            Err(e) => {
                eprintln!("登录失败: {e}");
                std::process::exit(1);
            }
        }
    } else {
        // 自动登录：--account > 默认账号
        let target = account.or(cfg.default_account.as_deref());
        match target {
            Some(id) => {
                let acc = cfg.find_by_identifier(id);
                match acc {
                    Some(a) if a.can_auto_login() => {
                        println!("使用保存的账号自动登录: {}", a.display_name());
                        let key = a.auto_login_session_key.clone().unwrap();
                        let launcher = Launcher::new().expect("failed to create Launcher");
                        match launcher.login_auto(&key).await {
                            Ok(t) => {
                                // autoLogin.json 返回新的 session key（旧 key 立即作废），
                                // 立即更新配置，否则下次自动登录会过期
                                if let Some(new_key) = &t.auto_login_session_key {
                                    let cfg_path =
                                        xiv_launcher_rs_lib::auth::config_path();
                                    let mut cfg = xiv_launcher_rs_lib::auth::load(&cfg_path);
                                    if let Some(acc) = cfg
                                        .accounts
                                        .iter_mut()
                                        .find(|a| a.snda_id == t.snda_id)
                                    {
                                        acc.auto_login_session_key = Some(new_key.clone());
                                        if let Err(e) = xiv_launcher_rs_lib::auth::save(
                                            &cfg_path, &cfg,
                                        ) {
                                            eprintln!("更新 session key 失败: {e}");
                                        }
                                    }
                                }
                                // 显示剩余有效期（autoLoginMaxAge，秒）
                                if let Some(age) = t.auto_login_max_age {
                                    println!(
                                        "session key 已刷新，剩余有效期: {:.1} 天",
                                        age as f64 / 86400.0
                                    );
                                }
                                t
                            }
                            Err(e) => {
                                eprintln!("自动登录失败（session key 可能过期）: {e}");
                                eprintln!("请重新登录: `xlcli auth login qr` 或 `xlcli launch --method qr`");
                                std::process::exit(1);
                            }
                        }
                    }
                    Some(_) => {
                        eprintln!("账号 '{}' 没有保存 session key，无法自动登录。", a_display(&cfg, id));
                        eprintln!("请用 `xlcli auth login qr` 重新登录，或 `xlcli launch --method qr`。");
                        std::process::exit(1);
                    }
                    None => {
                        eprintln!("找不到账号 '{id}'，用 `xlcli auth status` 查看。");
                        std::process::exit(1);
                    }
                }
            }
            None => {
                eprintln!("未指定账号且没有默认账号。");
                eprintln!("请用 `xlcli auth login qr` 登录并设置默认，或 `xlcli launch --method qr` 手动登录。");
                std::process::exit(1);
            }
        }
    };

    let area = match find_area(area_id).await {
        Ok(a) => a,
        Err(e) => {
            eprintln!("{e}");
            std::process::exit(1);
        }
    };
    let areas = match SdoAuth::fetch_server_list().await {
        Ok(a) => a,
        Err(e) => {
            eprintln!("获取大区列表失败: {e}");
            std::process::exit(1);
        }
    };

    let exe = game_path.join("game/ffxiv_dx11.exe");
    if !exe.exists() {
        eprintln!("未找到游戏可执行文件: {}", exe.display());
        std::process::exit(1);
    }

    let mut launcher = Launcher::new()
        .expect("failed to create Launcher")
        .with_wine_settings(xiv_launcher_rs_lib::config::load_settings());
    if let Some(w) = wine {
        launcher = launcher.with_wine_path(w);
    }

    println!("\n启动游戏 ({}) ...", area.area_name);
    match launcher.launch(&token, area, areas, &exe).await {
        Ok(result) => {
            println!("游戏已启动! PID: {}", result.child.id());
            println!("命令行: {}", result.command);
        }
        Err(e) => {
            eprintln!("启动失败: {e}");
            std::process::exit(1);
        }
    }
}

/// 辅助：显示账号展示名（找不到时回退为传入的 id）。
fn a_display(cfg: &xiv_launcher_rs_lib::auth::AuthConfig, id: &str) -> String {
    cfg.find(id)
        .map(|a| a.display_name().to_string())
        .unwrap_or_else(|| id.to_string())
}
