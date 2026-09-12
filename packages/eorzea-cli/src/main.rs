//! `eoz` — Eorzea（FFXIV 国服启动器）命令行工具。
//!
//! 目前包含游戏文件管理子命令（免登录）：
//!
//! ```text
//! eoz areas                        # 列出所有大区
//! eoz game status   --game-path …  # 显示本地游戏版本
//! eoz game check    --game-path … --area …   # 检查更新（列出待下载补丁）
//! eoz game update   --game-path … --area …   # 下载补丁（暂存，未应用）
//! eoz game verify   --game-path …            # 完整性校验（未实现）
//! eoz config list                            # 列出当前生效配置
//! eoz config get game_path                   # 读取一个配置项
//! eoz config set game_path /games/ffxiv      # 设置一个配置项
//! eoz config set dalamud.enabled true
//! eoz config unset dalamud.enabled
//! ```
//!
//! `--game-path` 指向游戏**根目录**（含 `boot/`、`game/`、`sdo/`）；
//! 省略时读取 `config.toml` 顶层 `game_path`（GUI 设置页会写入）。
//! `eoz config` 采用与 `git config` 类似的 get/set/unset/list/path 子命令。

use std::io::{self, Write};
use std::path::PathBuf;

use clap::{Parser, Subcommand};
use console::style;
use eorzea_auth::sdo::SdoAuth;
use eorzea_lib::game_files::{version, GameFileManager};
use eorzea_lib::launcher::{Launcher, LauncherError};
use eorzea_lib::term_img;

mod ui;

use ui::{banner, die, hint, human_bytes, kv, ok, warn};

#[derive(Parser)]
#[command(
    name = "eoz",
    version,
    about = "Eorzea（FFXIV 国服启动器）命令行工具",
    subcommand_required = true
)]
struct Cli {
    /// 输出更多日志：-v 信息 / -vv 调试 / -vvv 追踪（也可用 RUST_LOG 精确控制）
    #[arg(short, long, action = clap::ArgAction::Count, global = true)]
    verbose: u8,

    /// 不显示进度条（stderr 非终端时自动关闭）
    #[arg(long, global = true)]
    no_progress: bool,

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

    /// 读写 config.toml（类似 git config）
    Config {
        #[command(subcommand)]
        sub: ConfigCommand,
    },

    /// Dalamud 状态/安装/启动（插件框架集成）
    Dalamud {
        #[command(subcommand)]
        sub: DalamudCommand,
    },

    /// 账号管理（多账号 + 默认账号，配置持久化到 auth.toml）
    Auth {
        #[command(subcommand)]
        sub: AuthCommand,
    },

    /// 登录并启动游戏
    Launch {
        /// 游戏根目录（含 boot/、game/、sdo/；缺省读取 config.toml 的 game_path）
        #[arg(long)]
        game_path: Option<PathBuf>,

        /// 大区 ID（用 `eoz areas` 查看；缺省读取 config.toml 的 area）
        #[arg(long)]
        area: Option<String>,

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

        /// 强制启用 Dalamud（覆盖 [dalamud].enabled）
        #[arg(long)]
        dalamud: bool,

        /// 强制禁用 Dalamud（覆盖 [dalamud].enabled）
        #[arg(long, conflicts_with = "dalamud")]
        no_dalamud: bool,

    },
}

#[derive(Subcommand)]
enum GameCommand {
    /// 显示游戏目录的本地版本
    Status {
        /// 游戏根目录（含 boot/、game/、sdo/；缺省读取 config.toml 的 game_path）
        #[arg(long)]
        game_path: Option<PathBuf>,
    },

    /// 检查更新：版本报告 → 补丁服务器 → 待下载补丁列表
    Check {
        /// 游戏根目录（缺省读取 config.toml 的 game_path）
        #[arg(long)]
        game_path: Option<PathBuf>,

        /// 大区 ID（用 `eoz areas` 查看；缺省读取 config.toml 的 area）
        #[arg(long)]
        area: Option<String>,

        /// 版本报告包含的资料片数量（默认 5，对齐 C# Constants.MaxExpansion）
        #[arg(long, default_value_t = 5)]
        max_expansion: i32,

        /// 报告基础版本（全新安装/修复，会返回全部补丁）
        #[arg(long)]
        repair: bool,
    },

    /// 下载待更新补丁到暂存目录（不应用）
    Update {
        /// 游戏根目录（缺省读取 config.toml 的 game_path）
        #[arg(long)]
        game_path: Option<PathBuf>,

        /// 大区 ID（用 `eoz areas` 查看；缺省读取 config.toml 的 area）
        #[arg(long)]
        area: Option<String>,

        /// 版本报告包含的资料片数量
        #[arg(long, default_value_t = 5)]
        max_expansion: i32,

        /// 报告基础版本（全新安装/修复）
        #[arg(long)]
        repair: bool,

        /// 补丁暂存目录（默认 ~/.eorzea/patches）
        #[arg(long)]
        patch_dir: Option<PathBuf>,

        /// 并发下载数（默认 4）
        #[arg(long, default_value_t = 4)]
        concurrency: usize,
    },

    /// 校验游戏文件完整性（存在性 + sqpack 结构）
    Verify {
        /// 游戏根目录（缺省读取 config.toml 的 game_path）
        #[arg(long)]
        game_path: Option<PathBuf>,
    },
}

#[derive(Subcommand)]
enum ConfigCommand {
    /// 读取一个配置项（例如 game_path、dalamud.enabled、dxvk.hud）
    Get {
        /// 配置键（点分路径，env.<NAME> 可读写附加环境变量）
        key: String,
    },

    /// 设置一个配置项并立即写回 config.toml
    Set {
        /// 配置键（点分路径）
        key: String,
        /// 值：布尔用 true/false，数字用十进制，其余为字符串
        value: String,
    },

    /// 删除一个配置项，使其恢复默认值
    Unset {
        /// 配置键（点分路径）
        key: String,
    },

    /// 列出当前生效配置（含默认值）
    List {},

    /// 显示配置文件路径
    Path {},
}

#[derive(Subcommand)]
enum DalamudCommand {
    /// 显示 Dalamud 状态：release 版本、本机安装、游戏版本兼容性
    Status {
        /// 游戏根目录（读取本地游戏版本；缺省读取 config.toml 的 game_path）
        #[arg(long)]
        game_path: Option<PathBuf>,
    },

    /// 预下载安装 Dalamud（release + runtime + assets），与 launch 的版本门控一致
    Install {
        /// 游戏根目录（读取本地游戏版本；缺省读取 config.toml 的 game_path）
        #[arg(long)]
        game_path: Option<PathBuf>,
    },

    /// 通过 Dalamud Injector 启动游戏（版本不匹配时拒绝）
    Launch {
        /// 游戏根目录（缺省读取 config.toml 的 game_path）
        #[arg(long)]
        game_path: Option<PathBuf>,

        /// 大区 ID（缺省读取 config.toml 的 area）
        #[arg(long)]
        area: Option<String>,

        /// 强制启用 Dalamud（覆盖 config.toml [dalamud].enabled）
        #[arg(long)]
        dalamud: bool,

        /// 强制禁用 Dalamud（覆盖 config.toml [dalamud].enabled）
        #[arg(long, conflicts_with = "dalamud")]
        no_dalamud: bool,
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

/// `--game-path` 缺省时从 config.toml 顶层 `game_path` 读取。
fn game_path_or_config(game_path: Option<PathBuf>) -> PathBuf {
    if let Some(path) = game_path {
        return path;
    }
    match eorzea_lib::config::load_app_default().game_path {
        Some(path) => path,
        None => die(
            "未指定 --game-path，且 config.toml 中没有 game_path（用 `eoz config set game_path <游戏根目录>` 或在 GUI 设置页保存）",
        ),
    }
}

/// `--area` 缺省时从 config.toml 顶层 `area` 读取。
fn area_or_config(area: Option<String>) -> String {
    if let Some(area) = area {
        return area;
    }
    match eorzea_lib::config::load_app_default().area {
        Some(area) => area,
        None => die(
            "未指定 --area，且 config.toml 中没有 area（用 `eoz config set area <大区ID>`，或在命令中传 --area）",
        ),
    }
}

#[tokio::main]
async fn main() {
    let cli = Cli::parse();
    // 日志默认静默（warn），用户输出由 ui::* 负责；-v/-vv 才刷库日志
    ui::init(cli.verbose, cli.no_progress);

    match cli.command {
        Command::Areas => cmd_areas().await,
        Command::Config { sub } => cmd_config(sub),
        Command::Dalamud { sub } => match sub {
            DalamudCommand::Status { game_path } => {
                let game_path = game_path_or_config(game_path);
                cmd_dalamud_status(&game_path).await
            }
            DalamudCommand::Install { game_path } => {
                let game_path = game_path_or_config(game_path);
                cmd_dalamud_install(&game_path).await
            }
            DalamudCommand::Launch {
                game_path,
                area,
                dalamud,
                no_dalamud,
            } => {
                let game_path = game_path_or_config(game_path);
                let area = area_or_config(area);
                let override_enabled = if dalamud {
                    Some(true)
                } else if no_dalamud {
                    Some(false)
                } else {
                    None
                };
                cmd_dalamud_launch(&game_path, &area, override_enabled).await
            }
        },
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
            dalamud,
            no_dalamud,
        } => {
            let override_enabled = if dalamud {
                Some(true)
            } else if no_dalamud {
                Some(false)
            } else {
                None
            };
            let game_path = game_path_or_config(game_path);
            let area = area_or_config(area);
            cmd_launch(
                &game_path,
                &area,
                account.as_deref(),
                method.as_deref(),
                username.as_deref(),
                wine.as_deref(),
                qr_file,
                override_enabled,
            )
            .await;
        }
        Command::Game { sub } => match sub {
            GameCommand::Status { game_path } => {
                let game_path = game_path_or_config(game_path);
                cmd_status(&game_path)
            }
            GameCommand::Check {
                game_path,
                area,
                max_expansion,
                repair,
            } => {
                let game_path = game_path_or_config(game_path);
                let area = area_or_config(area);
                cmd_check(&game_path, &area, max_expansion, repair).await
            }
            GameCommand::Update {
                game_path,
                area,
                max_expansion,
                repair,
                patch_dir,
                concurrency,
            } => {
                let game_path = game_path_or_config(game_path);
                let area = area_or_config(area);
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
            GameCommand::Verify { game_path } => {
                let game_path = game_path_or_config(game_path);
                cmd_verify(&game_path).await
            }
        },
    }
}

async fn cmd_areas() {
    let sp = ui::Spinner::new("获取大区列表");
    match SdoAuth::fetch_server_list().await {
        Ok(areas) => {
            sp.finish(format!("大区列表（{} 个）", areas.len()));
            ui::row(
                &["ID".into(), "大区".into(), "lobby / patch".into()],
                &[4, 12, 0],
            );
            for a in &areas {
                ui::row(
                    &[
                        a.area_id.clone(),
                        a.area_name.clone(),
                        ui::dim(format!("{} · {}", a.area_lobby, a.area_patch)).to_string(),
                    ],
                    &[4, 12, 0],
                );
            }
        }
        Err(e) => die(format!("获取大区列表失败: {e}")),
    }
}

fn cmd_status(game_path: &std::path::Path) {
    let mgr = GameFileManager::new();
    let v = mgr.status(game_path);

    banner(format!("本地版本  {}", game_path.display()));
    let mark = |ver: &str| {
        if ver == version::BASE_GAME_VERSION {
            format!("{ver}  {}", style("(未安装)").dim())
        } else {
            ver.to_string()
        }
    };
    kv("boot", mark(&v.boot));
    kv("ffxiv", mark(&v.ffxiv));
    for (n, ver) in [
        (1, &v.ex1),
        (2, &v.ex2),
        (3, &v.ex3),
        (4, &v.ex4),
        (5, &v.ex5),
    ] {
        kv(&format!("ex{n}"), mark(ver));
    }
}

async fn find_area(area_id: &str) -> Result<eorzea_auth::SdoArea, String> {
    let areas = SdoAuth::fetch_server_list()
        .await
        .map_err(|e| format!("获取大区列表失败: {e}"))?;
    areas
        .into_iter()
        .find(|a| a.area_id == area_id)
        .ok_or_else(|| format!("找不到大区 ID '{area_id}'，用 `eoz areas` 查看"))
}

async fn cmd_check(
    game_path: &std::path::Path,
    area_id: &str,
    max_expansion: i32,
    repair: bool,
) {
    let mgr = GameFileManager::new();
    let sp = ui::Spinner::new("解析大区 / 获取服务器列表");
    let area = match find_area(area_id).await {
        Ok(a) => {
            sp.clear();
            a
        }
        Err(e) => die(e),
    };

    banner(format!("检查更新  {}  ·  {}", area.area_name, game_path.display()));
    let sp = ui::Spinner::new("对比本地与服务端版本");
    let result = mgr
        .check_update(&area, game_path, repair, max_expansion)
        .await;
    sp.clear();

    match result {
        Ok(eorzea_lib::game_files::CheckResult::UpToDate { unique_id }) => {
            if unique_id.is_empty() {
                ok("已是最新版本");
            } else {
                ok(format!("已是最新版本（X-Patch-Unique-Id: {unique_id}）"));
            }
        }
        Ok(eorzea_lib::game_files::CheckResult::NeedsPatch {
            patches,
            unique_id,
        }) => {
            let total: u64 = patches.iter().map(|p| p.length).sum();
            ok(format!(
                "需要下载 {} 个补丁，共 {}",
                patches.len(),
                human_bytes(total)
            ));
            ui::line("");
            ui::row(
                &["大小".into(), "版本".into(), "URL".into()],
                &[10, 18, 0],
            );
            for p in &patches {
                ui::row(
                    &[
                        human_bytes(p.length),
                        p.version.to_string(),
                        p.url.clone(),
                    ],
                    &[10, 18, 0],
                );
            }
            kv("X-Patch-Unique-Id", unique_id);
            hint("用 `eoz game update` 下载并应用");
        }
        Ok(eorzea_lib::game_files::CheckResult::NeedsPatchBoot) => {
            warn("服务器指示 boot 需要更新（国服通常不会出现）");
        }
        Err(e) => die(format!("检查更新失败: {e}")),
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
    let sp = ui::Spinner::new("解析大区 / 获取服务器列表");
    let area = match find_area(area_id).await {
        Ok(a) => {
            sp.clear();
            a
        }
        Err(e) => die(e),
    };

    let patch_dir = patch_dir.unwrap_or_else(|| {
        dirs::home_dir()
            .map(|h| h.join(".eorzea/patches"))
            .unwrap_or_else(|| PathBuf::from("./patches"))
    });

    banner(format!("更新游戏  {}  ·  {}", area.area_name, game_path.display()));

    let sp = ui::Spinner::new("对比本地与服务端版本");
    let check = match mgr
        .check_update(&area, game_path, repair, max_expansion)
        .await
    {
        Ok(c) => {
            sp.clear();
            c
        }
        Err(e) => die(format!("检查更新失败: {e}")),
    };

    let patches = match check {
        eorzea_lib::game_files::CheckResult::UpToDate { .. } => {
            ok("已是最新版本，无需更新");
            return;
        }
        eorzea_lib::game_files::CheckResult::NeedsPatch { patches, .. } => patches,
        eorzea_lib::game_files::CheckResult::NeedsPatchBoot => {
            die("boot 需要更新（国服通常不会出现）")
        }
    };

    let total: u64 = patches.iter().map(|p| p.length).sum();
    kv(
        "待下载",
        format!("{} 个补丁，共 {}", patches.len(), human_bytes(total)),
    );
    kv("暂存目录", patch_dir.display());

    let start = std::time::Instant::now();
    let bar = ui::ByteBar::new("下载补丁");
    match mgr
        .download(&patches, &patch_dir, concurrency, bar.callback())
        .await
    {
        Ok(summary) => {
            bar.finish(format!(
                "下载完成：{} 个新下载 / {} 个已存在跳过，用时 {:.1}s",
                summary.downloaded.len(),
                summary.skipped,
                start.elapsed().as_secs_f64()
            ));
            let sp = ui::Spinner::new("应用补丁");
            match mgr.install(&patches, &patch_dir, game_path).await {
                Ok(install_summary) => {
                    let installed = install_summary.installed.len();
                    sp.clear();
                    ok(format!(
                        "安装完成：{installed} 个补丁已应用 / {} 个跳过（缺文件）",
                        install_summary.skipped
                    ));
                    if installed > 0 {
                        hint("版本文件已同步；用 `eoz game status` 确认");
                    }
                }
                Err(e) => die(format!("安装失败: {e}")),
            }
        }
        Err(e) => {
            bar.fail(e.to_string());
            std::process::exit(1);
        }
    }
}


/// `game verify`：校验游戏文件完整性 + 版本状态。
async fn cmd_verify(game_path: &std::path::Path) {
    use eorzea_lib::game_files::verify::{IssueSeverity, verify_game};

    banner(format!("校验游戏文件  {}", game_path.display()));
    let sp = ui::Spinner::new("扫描 SQPack 索引与文件大小");
    let issues = verify_game(game_path, 5);
    sp.clear();

    // 版本状态检查（免登录）：本地 vs 服务器最新
    let sp = ui::Spinner::new("对比服务端版本");
    let version_line = check_update_status(game_path).await;
    sp.clear();
    match version_line {
        Ok(status) => ui::line(format!("  {status}")),
        Err(e) => warn(format!("无法检查版本状态: {e}")),
    }

    if issues.is_empty() {
        ok("文件完整");
        return;
    }

    let mut missing = Vec::new();
    let mut corrupt = Vec::new();
    let mut warnings = Vec::new();
    for i in &issues {
        match i.severity {
            IssueSeverity::Missing => missing.push(i),
            IssueSeverity::Corrupt => corrupt.push(i),
            IssueSeverity::Warning => warnings.push(i),
        }
    }

    if !missing.is_empty() {
        ui::fail(format!("缺失文件（{}）", missing.len()));
        for i in &missing {
            ui::line(format!("    {}", i.path));
        }
    }
    if !corrupt.is_empty() {
        ui::warn(format!("损坏文件（{}）", corrupt.len()));
        for i in &corrupt {
            ui::line(format!("    {} — {}", i.path, i.message));
        }
    }
    if !warnings.is_empty() {
        ui::hint(format!("警告（{}）", warnings.len()));
        for i in &warnings {
            ui::line(format!("    {} — {}", i.path, i.message));
        }
    }

    if !missing.is_empty() || !corrupt.is_empty() {
        ui::line("");
        hint("用 `eoz game update` 重新下载修复");
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
) -> Result<eorzea_lib::launcher::QrCodeSession, LauncherError> {
    let sp = ui::Spinner::new("申请扫码二维码");
    let qr = match launcher.request_qr_code().await {
        Ok(qr) => {
            sp.clear();
            qr
        }
        Err(e) => {
            sp.clear();
            return Err(e);
        }
    };

    let path = qr_file.unwrap_or_else(|| {
        dirs::home_dir()
            .map(|h| h.join("xiv_qr.png"))
            .unwrap_or_else(|| PathBuf::from("xiv_qr.png"))
    });
    std::fs::write(&path, qr.image_data()).expect("failed to save QR image");
    kv("二维码", path.display());

    // 终端直接显示图片（kitty / iTerm2）
    match term_img::display_png(qr.image_data()) {
        Ok(()) => hint("请用叨鱼 App 扫描上方二维码"),
        Err(e) => hint(format!("终端不支持图片显示（{e}），请打开上面的图片文件扫码")),
    }

    Ok(qr)
}

/// 执行登录（不保存账号），返回 LaunchToken。
async fn do_login(
    method: &AuthMethod,
    username: Option<&str>,
    session_key: Option<&str>,
    qr_file: Option<PathBuf>,
) -> Result<eorzea_lib::launcher::LaunchToken, LauncherError> {
    let sp = ui::Spinner::new("初始化登录上下文");
    let launcher = Launcher::new()?;
    sp.clear();
    kv("设备指纹", launcher.device_id());

    match method {
        AuthMethod::Password => {
            let account = username.map(|s| s.to_string()).unwrap_or_else(|| prompt("账号"));
            ui::line("  密码（输入不回显）:");
            let password = rpassword::read_password().map_err(|e| {
                LauncherError::Auth(format!("读取密码失败: {e}"))
            })?;
            let sp = ui::Spinner::new("密码登录 / 换取会话");
            let r = launcher.login_password(&account, &password).await;
            sp.clear();
            r
        }
        AuthMethod::Qr => {
            let qr = show_qr_code(&launcher, qr_file).await?;
            let sp = ui::Spinner::new("等待扫码（最多 300 秒）");
            let r = qr
                .wait_for_scan(Some(std::time::Duration::from_secs(300)))
                .await;
            sp.clear();
            r
        }
        AuthMethod::Auto => {
            let key = session_key
                .map(|s| s.to_string())
                .unwrap_or_else(|| prompt("Auto-login session key"));
            let sp = ui::Spinner::new("自动登录（session key）");
            let r = launcher.login_auto(&key).await;
            sp.clear();
            r
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
        Err(e) => die(format!("登录失败: {e}")),
    };

    // 保存账号
    let cfg_path = eorzea_lib::auth::config_path();
    let mut cfg = eorzea_lib::auth::load(&cfg_path);
    let make_default = cfg.accounts.is_empty(); // 第一个账号自动设为默认
    let username = username
        .map(|s| s.to_string())
        .or_else(|| token.username.clone());
    cfg.upsert(
        eorzea_lib::auth::Account {
            snda_id: token.snda_id.clone(),
            username,
            auto_login_session_key: token.auto_login_session_key.clone(),
        },
        make_default,
    );
    if let Err(e) = eorzea_lib::auth::save(&cfg_path, &cfg) {
        die(format!("保存配置失败: {e}"));
    }

    ok("登录成功，账号已保存");
    kv("snda_id", &token.snda_id);
    kv(
        "session_key",
        token.auto_login_session_key.as_deref().unwrap_or("(无)"),
    );
    // 显示真实默认状态：第一个账号自动默认，或已是配置中的默认账号
    let is_default = cfg
        .default_account()
        .map(|a| a.snda_id == token.snda_id)
        .unwrap_or(false);
    kv("默认账号", if is_default { "是" } else { "否" });
    kv("配置", cfg_path.display());
}

/// `auth status`：显示已保存账号。
fn cmd_auth_status() {
    let cfg_path = eorzea_lib::auth::config_path();
    let cfg = eorzea_lib::auth::load(&cfg_path);

    banner(format!("已保存账号  {}", cfg_path.display()));
    if cfg.accounts.is_empty() {
        hint("（无）用 `eoz auth login qr` 登录一个账号");
        return;
    }
    ui::row(
        &["账号".into(), "snda_id".into(), "自动登录".into(), "".into()],
        &[18, 12, 10, 0],
    );
    for acc in &cfg.accounts {
        // 默认标记：default_account 可能是 username 或 snda_id，统一解析后按 snda_id 比较
        let default = cfg
            .default_account()
            .map(|a| a.snda_id == acc.snda_id)
            .unwrap_or(false);
        ui::row(
            &[
                acc.display_name().to_string(),
                acc.snda_id.clone(),
                if acc.can_auto_login() {
                    "有 session key".to_string()
                } else {
                    "无".to_string()
                },
                if default {
                    style("[默认]").green().to_string()
                } else {
                    String::new()
                },
            ],
            &[18, 12, 10, 0],
        );
    }
    if cfg.default_account.is_none() {
        hint("未设置默认账号，用 `eoz auth default <账号>` 设置");
    }
}

/// `auth default`：设置默认账号。
fn cmd_auth_default(account: &str) {
    let cfg_path = eorzea_lib::auth::config_path();
    let mut cfg = eorzea_lib::auth::load(&cfg_path);

    // 支持 snda_id 或 username 匹配；存储时 username 优先（可读），无则 snda_id
    let found = cfg.find_by_identifier(account).cloned();

    match found {
        Some(acc) => {
            cfg.default_account = Some(
                acc.username.clone().unwrap_or_else(|| acc.snda_id.clone()),
            );
            if let Err(e) = eorzea_lib::auth::save(&cfg_path, &cfg) {
                die(format!("保存配置失败: {e}"));
            }
            ok(format!("默认账号已设为 {account}"));
        }
        None => die(format!(
            "找不到账号 '{account}'，用 `eoz auth status` 查看已保存账号"
        )),
    }
}

/// `auth logout`：删除账号（缺省删默认账号）。
fn cmd_auth_logout(account: Option<&str>) {
    let cfg_path = eorzea_lib::auth::config_path();
    let mut cfg = eorzea_lib::auth::load(&cfg_path);

    let target = match account {
        Some(a) => a.to_string(),
        None => match cfg.default_account.clone() {
            Some(id) => id,
            None => die("没有默认账号，请指定 `--account`"),
        },
    };

    let removed = cfg.remove(&target);
    if !removed {
        // 试试 username 匹配（remove 只认 snda_id）
        let by_name = cfg.find_by_identifier(&target).map(|a| a.snda_id.clone());
        match by_name {
            Some(id) => {
                cfg.remove(&id);
                if let Err(e) = eorzea_lib::auth::save(&cfg_path, &cfg) {
                    die(format!("保存配置失败: {e}"));
                }
                ok(format!("已删除账号 {target}"));
            }
            None => die(format!("找不到账号 '{target}'")),
        }
    } else {
        if let Err(e) = eorzea_lib::auth::save(&cfg_path, &cfg) {
            die(format!("保存配置失败: {e}"));
        }
        ok(format!("已删除账号 {target}"));
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
    dalamud_override: Option<bool>,
) {
    // 先定位游戏 + 解析大区 + 准备 Dalamud，再登录：
    // - 大区 ID 写错时不必先消耗掉一次性 session key
    // - 启用 Dalamud 时准备失败要立刻报错退出，同样发生在换取 SSO ticket 之前
    let exe = game_path.join("game/ffxiv_dx11.exe");
    if !exe.exists() {
        die(format!("未找到游戏可执行文件: {}", exe.display()));
    }

    let mut launcher = Launcher::new()
        .expect("failed to create Launcher")
        .with_wine_settings(eorzea_lib::config::load_settings());
    if let Some(w) = wine {
        launcher = launcher.with_wine_path(w);
    }

    let sp = ui::Spinner::new("解析大区 / 获取服务器列表");
    let area = match find_area(area_id).await {
        Ok(a) => a,
        Err(e) => {
            sp.clear();
            die(e)
        }
    };
    let areas = match SdoAuth::fetch_server_list().await {
        Ok(a) => a,
        Err(e) => {
            sp.clear();
            die(format!("获取大区列表失败: {e}"))
        }
    };
    sp.clear();

    banner(format!(
        "启动游戏  {}  ·  {}",
        area.area_name,
        game_path.display()
    ));

    // Dalamud：三件套按需补齐；下载用进度条，不下载的阶段一闪而过
    let mut bar: Option<ui::ByteBar> = None;
    let mut last_phase = None;
    let mut phase_bar = |phase: eorzea_lib::launcher::DalamudPhase, done: u64, total: u64| {
        if last_phase != Some(phase) {
            if let Some(b) = bar.take() {
                b.finish_ready();
            }
            last_phase = Some(phase);
            bar = Some(ui::ByteBar::new(format!("准备 {}", phase.label())));
        }
        if let Some(b) = bar.as_ref() {
            b.set_progress(done, total);
        }
    };

    let dalamud_cfg = match launcher
        .prepare_dalamud(game_path, dalamud_override, |phase, done, total| {
            phase_bar(phase, done, total)
        })
        .await
    {
        Ok(cfg) => {
            if let Some(b) = bar.take() {
                b.finish_ready();
            }
            cfg
        }
        Err(e) => {
            if let Some(b) = bar.take() {
                b.clear();
            }
            ui::fail(e);
            ui::fail("本次不会启动游戏。修复后重试，或用 `eoz launch --no-dalamud` 本次跳过（也可在设置里关闭 Dalamud）。");
            std::process::exit(1);
        }
    };

    // 确定登录方式：
    // 1. --account 指定（或配置默认账号）且该账号有 session key → auto 登录
    // 2. --method 手动登录
    let cfg_path = eorzea_lib::auth::config_path();
    let cfg = eorzea_lib::auth::load(&cfg_path);

    let token = if let Some(m) = method {
        let method_enum = match m {
            "password" => AuthMethod::Password,
            "qr" => AuthMethod::Qr,
            "auto" => AuthMethod::Auto,
            other => die(format!("未知登录方式 '{other}'，可选: password | qr | auto")),
        };
        match do_login(&method_enum, username, None, qr_file).await {
            Ok(t) => t,
            Err(e) => die(format!("登录失败: {e}")),
        }
    } else {
        // 自动登录：--account > 默认账号
        let target = account.or(cfg.default_account.as_deref());
        match target {
            Some(id) => {
                let acc = cfg.find_by_identifier(id);
                match acc {
                    Some(a) if a.can_auto_login() => {
                        let name = a.display_name().to_string();
                        let key = a.auto_login_session_key.clone().unwrap();
                        let sp = ui::Spinner::new(format!("自动登录 {name}"));
                        match launcher.login_auto(&key).await {
                            Ok(t) => {
                                sp.clear();
                                // autoLogin.json 返回新的 session key（旧 key 立即作废），
                                // 立即更新配置，否则下次自动登录会过期
                                if let Some(new_key) = &t.auto_login_session_key {
                                    let cfg_path = eorzea_lib::auth::config_path();
                                    let mut cfg = eorzea_lib::auth::load(&cfg_path);
                                    if let Some(acc) = cfg
                                        .accounts
                                        .iter_mut()
                                        .find(|a| a.snda_id == t.snda_id)
                                    {
                                        acc.auto_login_session_key = Some(new_key.clone());
                                        if let Err(e) =
                                            eorzea_lib::auth::save(&cfg_path, &cfg)
                                        {
                                            ui::fail(format!("更新 session key 失败: {e}"));
                                        }
                                    }
                                }
                                // 显示剩余有效期（autoLoginMaxAge，秒）
                                if let Some(age) = t.auto_login_max_age {
                                    hint(format!(
                                        "session key 已刷新，剩余有效期 {:.1} 天",
                                        age as f64 / 86400.0
                                    ));
                                }
                                kv("账号", name);
                                t
                            }
                            Err(e) => {
                                sp.clear();
                                ui::fail(format!("自动登录失败（session key 可能过期）: {e}"));
                                die("请重新登录: `eoz auth login qr` 或 `eoz launch --method qr`");
                            }
                        }
                    }
                    Some(_) => die(format!(
                        "账号 '{}' 没有保存 session key，无法自动登录（用 `eoz auth login qr`，或 `eoz launch --method qr`）",
                        a_display(&cfg, id)
                    )),
                    None => die(format!("找不到账号 '{id}'，用 `eoz auth status` 查看")),
                }
            }
            None => die(
                "未指定账号且没有默认账号：用 `eoz auth login qr` 登录并设置默认，或 `eoz launch --method qr`",
            ),
        }
    };

    match &dalamud_cfg {
        Some(cfg) => kv(
            "Dalamud",
            format!(
                "{}（走 Injector）",
                cfg.install_dir
                    .file_name()
                    .map(|n| n.to_string_lossy().to_string())
                    .unwrap_or_else(|| "?".to_string())
            ),
        ),
        None => kv("Dalamud", ui::dim("禁用")),
    }

    let sp = ui::Spinner::new("启动游戏进程");
    match launcher
        .launch_prepared(
            &eorzea_lib::config::load_settings(),
            dalamud_cfg,
            &token,
            area,
            areas,
            &exe,
        )
        .await
    {
        Ok(result) => {
            sp.finish(format!("游戏已启动（PID {}）", result.child.id()));
            if let Some(log) = &result.log_path {
                kv("运行日志", log.display());
            }
        }
        Err(e) => {
            sp.clear();
            // 完整错误（含 Injector/wine stderr）已由 ERROR 日志输出，这里只留首行摘要
            let msg = e.to_string();
            ui::fail(format!(
                "启动失败: {}",
                msg.lines().next().unwrap_or(msg.as_str())
            ));
            if msg.lines().nth(1).is_some() {
                hint("详细输出见上方日志（`-v` 可看更多）");
            }
            std::process::exit(1);
        }
    }
}

/// 辅助：显示账号展示名（找不到时回退为传入的 id）。
fn a_display(cfg: &eorzea_lib::auth::AuthConfig, id: &str) -> String {
    cfg.find(id)
        .map(|a| a.display_name().to_string())
        .unwrap_or_else(|| id.to_string())
}

/// 检查游戏版本状态，返回可读的一行摘要（不带 emoji，样式由调用方决定）。
async fn check_update_status(game_path: &std::path::Path) -> Result<String, String> {
    let mgr = GameFileManager::new();
    let area_id = area_or_config(None);
    let area = find_area(&area_id).await?;

    match mgr.check_update(&area, game_path, false, 5).await {
        Ok(eorzea_lib::game_files::CheckResult::UpToDate { .. }) => {
            Ok("游戏已是最新版本".to_string())
        }
        Ok(eorzea_lib::game_files::CheckResult::NeedsPatch { patches, .. }) => {
            let total: u64 = patches.iter().map(|p| p.length).sum();
            Ok(format!(
                "游戏版本落后：{} 个补丁待更新（{}），用 `eoz game update` 更新",
                patches.len(),
                human_bytes(total)
            ))
        }
        Ok(eorzea_lib::game_files::CheckResult::NeedsPatchBoot) => {
            Ok("boot 需要更新（国服通常不出现）".to_string())
        }
        Err(e) => Err(e.to_string()),
    }
}


/// `dalamud status`：release 版本、本机安装、游戏版本兼容性。
async fn cmd_dalamud_status(game_path: &std::path::Path) {
    use eorzea_lib::dalamud::{InstallState, updater};

    let settings = eorzea_lib::config::load_dalamud_settings();
    let install_root = settings
        .install_root
        .clone()
        .unwrap_or_else(updater::default_install_root);

    let client = reqwest::Client::new();
    let sp = ui::Spinner::new("获取 release 元数据 / 检测本机安装");
    let st = updater::status(&client, &install_root, game_path, &settings.track).await;
    sp.clear();

    banner("Dalamud 状态");
    kv("安装根目录", install_root.display());
    kv("本地游戏", &st.local_game_ver);

    if let Some(remote) = &st.remote {
        kv(
            "release",
            format!(
                "{}  {}",
                ui::em(&remote.assembly_version),
                ui::dim(format!("(支持游戏 {})", remote.supported_game_ver))
            ),
        );
        if st.remote_from_cache {
            warn("远端元数据不可用，以上版本信息来自本地 version.json");
        }
        kv(
            "runtime",
            format!(
                "{}{}",
                remote.runtime_version,
                if remote.runtime_required {
                    " (必需)"
                } else {
                    ""
                }
            ),
        );
    } else {
        kv("release", ui::dim("(无法获取元数据，且本机无记录)"));
    }

    match &st.local_assembly_version {
        Some(v) => kv(
            "本机已安装",
            format!(
                "{v}  {}",
                ui::dim(
                    st.install_path
                        .as_ref()
                        .map(|p| p.display().to_string())
                        .unwrap_or_default()
                )
            ),
        ),
        None => kv("本机已安装", ui::dim("(无)")),
    }

    let state_label = match &st.install_state {
        InstallState::Ready => ui::em(style("就绪（版本匹配，可启动）").green().to_string()),
        InstallState::Missing => {
            if st.remote.is_none() {
                ui::em("未安装（元数据不可用，需联网才能安装）".to_string())
            } else {
                ui::em("未安装（release 匹配游戏版本，可安装）".to_string())
            }
        }
        InstallState::OutOfDate => ui::em(style("已安装但版本不匹配游戏").yellow().to_string()),
        InstallState::Unsupported => {
            ui::em(style("release 尚未支持当前游戏版本（等待发布）").red().to_string())
        }
        InstallState::RuntimeMissing => ui::em("Windows .NET runtime 未安装（启动时自动下载）".to_string()),
        InstallState::AssetsMissing => ui::em("assets 未安装（启动时自动下载）".to_string()),
        InstallState::Failed(msg) => ui::em(style(format!("安装失败: {msg}")).red().to_string()),
    };
    kv("状态", state_label);

    if !st.remote_supported() {
        hint("游戏更新、release 未跟进的期间，`eoz launch` 会直接报错，可用 `--no-dalamud` 先进游戏");
    }
}

/// `dalamud install`：预下载安装 Dalamud 三组件（release 本体、Windows .NET
/// runtime、assets），与 launch 惰性安装走同一套函数和版本门控；区别在于
/// 失败显式报错退出（launch 启用 Dalamud 时同样报错不启动，除非 `--no-dalamud`），
/// 适合提前离线就绪。
async fn cmd_dalamud_install(game_path: &std::path::Path) {
    use eorzea_lib::dalamud::{ensure_assets, ensure_runtime, updater};

    let settings = eorzea_lib::config::load_dalamud_settings();
    let install_root = settings
        .install_root
        .clone()
        .unwrap_or_else(updater::default_install_root);
    let client = reqwest::Client::new();

    banner("Dalamud 安装");
    kv("安装根目录", install_root.display());
    kv("更新通道", &settings.track);

    let sp = ui::Spinner::new("获取 release 元数据");
    let st = updater::status(&client, &install_root, game_path, &settings.track).await;
    sp.clear();
    let Some(remote) = st.remote.as_ref() else {
        die(format!(
            "无法获取 release 元数据（track={}），请检查网络后重试",
            settings.track
        ));
    };
    kv(
        "release",
        format!(
            "{}  {}",
            ui::em(&remote.assembly_version),
            ui::dim(format!("(支持游戏 {})", remote.supported_game_ver))
        ),
    );

    if remote.supported_game_ver != st.local_game_ver {
        die(format!(
            "版本不匹配：release 仅支持 {}，本地游戏 {}；请等 release 跟进后再安装",
            remote.supported_game_ver, st.local_game_ver
        ));
    }
    if st.remote_from_cache {
        warn("远端元数据不可用，本次按本机 version.json 判断版本（无法确认是否有更新）");
    }

    // 1. release 本体（未安装或哈希校验不过时下载；解压为纯 Rust，无需外部 7z）
    let bar = ui::ByteBar::new("release 本体");
    match updater::ensure_release(&client, &install_root, remote, bar.callback()).await {
        Ok(_) => bar.finish_ready(),
        Err(e) => {
            bar.fail(e.to_string());
            std::process::exit(1);
        }
    }

    // 2. Windows .NET runtime（release 要求或配置了 manage_runtime 时必需）
    let runtime_required = remote.runtime_required || settings.manage_runtime;
    if runtime_required {
        let bar = ui::ByteBar::new(format!("runtime {}", remote.runtime_version));
        match ensure_runtime(
            &client,
            &install_root,
            &remote.runtime_version,
            bar.callback(),
        )
        .await
        {
            Ok(_) => bar.finish_ready(),
            Err(e) => {
                bar.fail(e.to_string());
                std::process::exit(1);
            }
        }
    } else if updater::find_usable_runtime_dir(&install_root, Some(&remote.runtime_version))
        .is_some()
    {
        ok("runtime 就绪（非必需）");
    } else {
        hint("runtime 非必需且本机未安装，跳过");
    }

    // 3. Dalamud assets（字体/DATA 级别资源，launch 必需）
    let bar = ui::ByteBar::new("Dalamud assets");
    match ensure_assets(&client, &install_root, bar.callback()).await {
        Ok(_) => bar.finish_ready(),
        Err(e) => {
            bar.fail(e.to_string());
            std::process::exit(1);
        }
    }

    ui::line("");
    ok("Dalamud 已就绪");
    hint("用 `eoz launch` 或 GUI 启动（会自动走 Injector）");
}

/// `dalamud launch`：通过 Injector 启动游戏（版本门控：启用即强制，未就绪直接报错）。
async fn cmd_dalamud_launch(
    game_path: &std::path::Path,
    area_id: &str,
    dalamud_override: Option<bool>,
) {
    cmd_launch(
        game_path,
        area_id,
        None,
        None,
        None,
        None,
        None,
        dalamud_override.or(Some(true)),
    )
    .await;
}


// ── eoz config：类似 git config 的 config.toml 读写 ─────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ConfigValueKind {
    String,
    Bool,
    Uint,
    Table,
}

/// 允许通过 `eoz config` 读写的配置键。
fn config_key_spec(key: &str) -> Option<ConfigValueKind> {
    if let Some(env) = key.strip_prefix("env.") {
        return (!env.is_empty()).then_some(ConfigValueKind::String);
    }

    Some(match key {
        "game_path" | "area" | "startup_type" | "custom_path" | "prefix" | "debug_vars"
        | "dxvk.hud" | "dalamud.load_method" | "dalamud.track" | "dalamud.beta_key"
        | "dalamud.install_root" => ConfigValueKind::String,
        "esync" | "fsync" | "msync" | "gamemode" | "dxvk.enabled" | "dalamud.enabled"
        | "dalamud.no_plugins" | "dalamud.no_third_party_plugins"
        | "dalamud.manage_runtime" => ConfigValueKind::Bool,
        "dxvk.frame_limit" | "dalamud.delay_initialize_ms" => ConfigValueKind::Uint,
        "dalamud" | "dxvk" | "env" => ConfigValueKind::Table,
        _ => return None,
    })
}

fn parse_bool(raw: &str) -> Option<bool> {
    match raw.trim().to_ascii_lowercase().as_str() {
        "true" | "1" | "yes" | "on" => Some(true),
        "false" | "0" | "no" | "off" => Some(false),
        _ => None,
    }
}

fn validate_string_enum(key: &str, value: &str) -> Result<(), String> {
    let valid = match key {
        "startup_type" => ["auto", "managed", "custom", "system"].contains(&value),
        "dalamud.load_method" => ["entrypoint", "dllinject", "aclonly"].contains(&value),
        _ => true,
    };
    if valid {
        Ok(())
    } else {
        Err(format!(
            "{key} 的值 {value:?} 无效（startup_type: auto/managed/custom/system；dalamud.load_method: entrypoint/dllinject/aclonly）"
        ))
    }
}

/// 把命令行字符串按 key 的类型转成 TOML value。
fn parse_config_value(key: &str, raw: &str) -> Result<toml::Value, String> {
    let kind = config_key_spec(key).ok_or_else(|| format!("未知配置键: {key}"))?;
    match kind {
        ConfigValueKind::Bool => parse_bool(raw)
            .map(toml::Value::Boolean)
            .ok_or_else(|| format!("{key} 需要布尔值（true/false），实际是 {raw:?}")),
        ConfigValueKind::Uint => raw
            .trim()
            .parse::<u32>()
            .map(|n| toml::Value::Integer(i64::from(n)))
            .map_err(|e| format!("{key} 需要非负整数: {e}")),
        ConfigValueKind::String => {
            validate_string_enum(key, raw)?;
            Ok(toml::Value::String(raw.to_string()))
        }
        ConfigValueKind::Table => {
            let value = toml::from_str::<toml::Value>(raw)
                .map_err(|e| format!("{key} 需要 TOML 表（例如 '{{ enabled = true }}'）: {e}"))?;
            if value.is_table() {
                Ok(value)
            } else {
                Err(format!("{key} 需要 TOML 表，实际是标量"))
            }
        }
    }
}

fn set_config_path(root: &mut toml::Value, key: &str, value: toml::Value) -> Result<(), String> {
    let parts: Vec<&str> = key.split('.').collect();
    set_config_path_inner(root, &parts, value)
}

fn set_config_path_inner(
    cur: &mut toml::Value,
    parts: &[&str],
    value: toml::Value,
) -> Result<(), String> {
    let Some((head, tail)) = parts.split_first() else {
        return Err("配置键为空".into());
    };
    let table = cur
        .as_table_mut()
        .ok_or_else(|| format!("{head} 的父级不是 TOML 表"))?;
    if tail.is_empty() {
        table.insert((*head).to_string(), value);
        Ok(())
    } else {
        let child = table
            .entry((*head).to_string())
            .or_insert_with(|| toml::Value::Table(toml::map::Map::new()));
        set_config_path_inner(child, tail, value)
    }
}

fn get_config_path<'a>(root: &'a toml::Value, key: &str) -> Option<&'a toml::Value> {
    let mut cur = root;
    for part in key.split('.') {
        cur = cur.get(part)?;
    }
    Some(cur)
}

fn unset_config_path(root: &mut toml::Value, key: &str) -> Result<bool, String> {
    let parts: Vec<&str> = key.split('.').collect();
    unset_config_path_inner(root, &parts)
}

fn unset_config_path_inner(cur: &mut toml::Value, parts: &[&str]) -> Result<bool, String> {
    let Some((head, tail)) = parts.split_first() else {
        return Err("配置键为空".into());
    };
    let table = cur
        .as_table_mut()
        .ok_or_else(|| format!("{head} 的父级不是 TOML 表"))?;
    if tail.is_empty() {
        Ok(table.remove(*head).is_some())
    } else {
        match table.get_mut(*head) {
            Some(child) => unset_config_path_inner(child, tail),
            None => Ok(false),
        }
    }
}

fn display_config_value(value: &toml::Value) -> String {
    match value {
        toml::Value::String(s) => s.clone(),
        other => other.to_string(),
    }
}

/// 当前生效配置（默认值 + 文件覆盖）转成 TOML Value。
fn effective_config_document() -> Result<toml::Value, String> {
    let app = eorzea_lib::config::load_app_default();
    let text = toml::to_string_pretty(&app).map_err(|e| format!("序列化配置失败: {e}"))?;
    toml::from_str(&text).map_err(|e| format!("解析配置失败: {e}"))
}

/// set/unset 前载入磁盘文档。先触发 legacy settings.json 迁移，避免只写一个
/// key 时丢掉旧配置；文件解析失败时直接报错，不覆盖坏文件。
fn load_config_document_for_write() -> Result<toml::Value, String> {
    let _ = eorzea_lib::config::load_app_default();
    let path = eorzea_lib::config::settings_path();
    match std::fs::read_to_string(&path) {
        Ok(text) => toml::from_str::<toml::Value>(&text)
            .map_err(|e| format!("{} 解析失败（未修改文件）: {e}", path.display())),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            Ok(toml::Value::Table(toml::map::Map::new()))
        }
        Err(e) => Err(format!("读取 {} 失败: {e}", path.display())),
    }
}

fn save_config_document(doc: &toml::Value) -> Result<(), String> {
    let path = eorzea_lib::config::settings_path();
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| format!("创建 {} 失败: {e}", parent.display()))?;
    }
    let text = toml::to_string_pretty(doc).map_err(|e| format!("序列化配置失败: {e}"))?;
    std::fs::write(&path, text).map_err(|e| format!("写入 {} 失败: {e}", path.display()))
}

/// 写盘前用 AppConfig 完整反序列化一遍，类型/枚举错误在这里被拦下。
fn validate_config_document(doc: &toml::Value) -> Result<(), String> {
    let text = toml::to_string(doc).map_err(|e| format!("序列化配置失败: {e}"))?;
    toml::from_str::<eorzea_lib::config::AppConfig>(&text)
        .map(|_| ())
        .map_err(|e| format!("配置校验失败（未写盘）: {e}"))
}

fn cmd_config(sub: ConfigCommand) {
    match sub {
        // get/path 输出裸值（可被脚本直接消费），因此不套任何样式与缩进
        ConfigCommand::Get { key } => {
            if config_key_spec(&key).is_none() {
                die(format!("未知配置键: {key}"));
            }
            let doc = effective_config_document().unwrap_or_else(|e| die(e));
            match get_config_path(&doc, &key) {
                Some(value) => ui::line(display_config_value(value)),
                None => die(format!("{key} 未设置")),
            }
        }
        ConfigCommand::Set { key, value } => {
            let parsed = parse_config_value(&key, &value).unwrap_or_else(|e| die(e));
            let mut doc = load_config_document_for_write().unwrap_or_else(|e| die(e));
            if let Err(e) = set_config_path(&mut doc, &key, parsed) {
                die(e);
            }
            if let Err(e) = validate_config_document(&doc) {
                die(e);
            }
            if let Err(e) = save_config_document(&doc) {
                die(e);
            }
            ok(format!("已设置 {key} = {value}"));
        }
        ConfigCommand::Unset { key } => {
            if config_key_spec(&key).is_none() {
                die(format!("未知配置键: {key}"));
            }
            let mut doc = load_config_document_for_write().unwrap_or_else(|e| die(e));
            match unset_config_path(&mut doc, &key) {
                Ok(true) => {
                    if let Err(e) = validate_config_document(&doc) {
                        die(e);
                    }
                    if let Err(e) = save_config_document(&doc) {
                        die(e);
                    }
                    ok(format!("已删除 {key}（恢复默认值）"));
                }
                Ok(false) => hint(format!("{key} 原本就未设置")),
                Err(e) => die(e),
            }
        }
        // list 输出完整 TOML，保持原文（可重定向保存）
        ConfigCommand::List {} => match toml::to_string_pretty(&eorzea_lib::config::load_app_default())
        {
            Ok(text) => ui::line(text.trim_end()),
            Err(e) => die(format!("序列化配置失败: {e}")),
        },
        ConfigCommand::Path {} => ui::line(eorzea_lib::config::settings_path().display()),
    }
}

#[cfg(test)]
mod config_command_tests {
    use super::*;

    #[test]
    fn test_config_key_specs() {
        assert_eq!(config_key_spec("game_path"), Some(ConfigValueKind::String));
        assert_eq!(config_key_spec("dalamud.enabled"), Some(ConfigValueKind::Bool));
        assert_eq!(
            config_key_spec("dalamud.delay_initialize_ms"),
            Some(ConfigValueKind::Uint)
        );
        assert_eq!(config_key_spec("env.FOO"), Some(ConfigValueKind::String));
        assert_eq!(config_key_spec("dalamud"), Some(ConfigValueKind::Table));
        assert_eq!(config_key_spec("dalamud.unknown"), None);
    }

    #[test]
    fn test_parse_config_value() {
        assert_eq!(parse_config_value("esync", "yes").unwrap(), toml::Value::Boolean(true));
        assert_eq!(
            parse_config_value("dalamud.delay_initialize_ms", "250").unwrap(),
            toml::Value::Integer(250)
        );
        assert_eq!(
            parse_config_value("game_path", "/games/ffxiv").unwrap(),
            toml::Value::String("/games/ffxiv".into())
        );
        assert!(parse_config_value("startup_type", "wrong").is_err());
    }

    #[test]
    fn test_display_config_value() {
        assert_eq!(display_config_value(&toml::Value::Boolean(true)), "true");
        assert_eq!(display_config_value(&toml::Value::Integer(60)), "60");
    }

    #[test]
    fn test_set_get_unset_dotted_paths() {
        let mut root = toml::Value::Table(toml::map::Map::new());
        set_config_path(
            &mut root,
            "dalamud.enabled",
            toml::Value::Boolean(true),
        )
        .unwrap();
        set_config_path(
            &mut root,
            "env.WINEESYNC",
            toml::Value::String("1".into()),
        )
        .unwrap();
        assert_eq!(
            get_config_path(&root, "dalamud.enabled"),
            Some(&toml::Value::Boolean(true))
        );
        assert_eq!(
            get_config_path(&root, "env.WINEESYNC"),
            Some(&toml::Value::String("1".into()))
        );
        assert!(unset_config_path(&mut root, "dalamud.enabled").unwrap());
        assert!(get_config_path(&root, "dalamud.enabled").is_none());
    }
}
