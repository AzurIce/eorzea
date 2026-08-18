use crate::config::WineSettings;
use crate::wine::{build_launch_env, WineTool};
use eorzea_auth::SdoArea;
use std::path::PathBuf;
use tracing::{debug, error, info, instrument, warn};

fn mask_sensitive(value: &str) -> String {
    if value.len() <= 8 {
        "***".to_string()
    } else {
        format!("{}***{}", &value[..4], &value[value.len() - 4..])
    }
}

/// 游戏启动配置。
#[derive(Debug, Clone)]
pub struct GameLaunchConfig {
    /// 游戏可执行文件路径（`ffxiv_dx11.exe` 的完整路径）。
    pub game_path: PathBuf,
    /// 登录获得的 session ID（`DEV.TestSID`）。
    pub session_id: String,
    /// SDO 账号 ID（`XL.SndaId`）。
    pub snda_id: String,
    /// 选中的大区信息。
    pub area: SdoArea,
    /// 所有可用大区列表（用于构建 `XL.LobbyHosts`）。
    pub areas: Vec<SdoArea>,
    /// 最大资料片等级（国服固定为 1）。
    pub max_expansion: i32,
    /// DC 跨服传送端口（`XL.DcTraveler`）。
    pub dc_travel_port: Option<i32>,
    /// 是否重置配置（`resetConfig`，通常为 0）。
    pub reset_config: i32,
    /// 额外启动参数。
    pub additional_args: String,
    /// 启用 Dalamud 时的启动配置（`Some` 时通过 Injector 启动）。
    pub dalamud: Option<DalamudLaunchConfig>,
}

/// Dalamud 启动配置（启用时由 Injector 创建游戏进程）。
#[derive(Debug, Clone)]
pub struct DalamudLaunchConfig {
    /// Injector 可执行文件路径（`Hooks/<AssemblyVersion>/Dalamud.Injector.exe`，Unix 路径）。
    pub injector_exe: PathBuf,
    /// 发行目录（Injector 工作目录）。
    pub install_dir: PathBuf,
    /// `dalamudConfig.json` 路径。
    pub config_path: PathBuf,
    /// Dalamud 日志路径。
    pub log_path: PathBuf,
    /// 插件目录。
    pub plugin_dir: PathBuf,
    /// assets 目录。
    pub asset_dir: PathBuf,
    /// Windows x64 .NET runtime 根目录（`host/fxr`、`shared/...`）。
    ///
    /// `None` 表示未托管 runtime，此时不设置 `DALAMUD_RUNTIME`。
    pub runtime_dir: Option<PathBuf>,
    /// 加载方式（entrypoint / inject）。
    pub load_method: crate::dalamud::model::DalamudLoadMethod,
    pub delay_initialize_ms: u32,
    pub no_plugins: bool,
    pub no_third_party_plugins: bool,
}

/// 游戏启动结果。
#[derive(Debug)]
pub struct GameLaunchResult {
    /// 启动的子进程（direct 模式为游戏进程；Injector 模式为 Injector 进程）。
    pub child: std::process::Child,
    /// 完整的启动命令行。
    pub command: String,
    /// wine/游戏输出日志文件路径（如有）。
    pub log_path: Option<PathBuf>,
    /// 游戏进程的 Wine PID（Injector 模式由 Injector 报告；direct 模式为 child.pid()）。
    pub wine_pid: Option<u32>,
}

/// 默认游戏运行日志路径：`~/.xiv-launcher-rs/logs/game-{unix_ts}.log`。
pub fn default_game_log_path() -> PathBuf {
    let ts = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    dirs::home_dir()
        .map(|h| h.join(format!(".xiv-launcher-rs/logs/game-{ts}.log")))
        .unwrap_or_else(|| PathBuf::from(format!("game-{ts}.log")))
}

/// 构建国服启动参数列表（**逐参数**，不经过 shell 字符串拆分）。
///
/// 参考 C# `SdoLauncher.LaunchGameSdo()` 的参数构造方式。
#[instrument(skip(config))]
pub fn build_sdo_launch_args_vec(config: &GameLaunchConfig) -> Vec<String> {
    let mut args = vec![
        format!("-AppID={}", 100001900),
        format!("-AreaID={}", config.area.area_id),
        format!("Dev.LobbyHost01={}", config.area.area_lobby),
        "Dev.LobbyPort01=54994".to_string(),
        format!("Dev.GMServerHost={}", config.area.area_gm),
        format!("Dev.SaveDataBankHost={}", config.area.area_config_upload),
        format!("resetConfig={}", config.reset_config),
        format!("DEV.MaxEntitledExpansionID={}", config.max_expansion),
        format!("DEV.TestSID={}", config.session_id),
        format!("XL.SndaId={}", config.snda_id),
        // C# MainPage 传 areasInfo=""（空），游戏从服务器获取服务器列表
        "XL.LobbyHosts=".to_string(),
        // C# 总是传 XL.DcTraveler（无 DC 传送时为 0）
        format!("XL.DcTraveler={}", config.dc_travel_port.unwrap_or(0)),
    ];

    if !config.additional_args.is_empty() {
        args.extend(parse_additional_args(&config.additional_args));
    }

    args
}

/// 构建国服启动参数字符串（仅用于展示/兼容旧调用方；启动请使用 [`build_sdo_launch_args_vec`]）。
#[instrument(skip(config))]
pub fn build_sdo_launch_args(config: &GameLaunchConfig) -> String {
    let result = build_sdo_launch_args_vec(config).join(" ");
    let masked = result
        .replace(&config.session_id, &mask_sensitive(&config.session_id))
        .replace(&config.snda_id, &mask_sensitive(&config.snda_id));
    debug!("constructed launch args: {}", masked);

    result
}

/// 把 additional_args 拆成 argv，支持单/双引号和反斜杠转义。
///
/// 游戏参数值可能包含空格（如路径），不能用 `split_whitespace()`。
fn parse_additional_args(input: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut current = String::new();
    let mut quote = None;
    let mut token_started = false;
    let mut chars = input.chars().peekable();

    while let Some(c) = chars.next() {
        match quote {
            Some(q) => {
                if c == q {
                    quote = None;
                } else if c == '\\' && q == '"' {
                    match chars.peek().copied() {
                        Some(next) if next == '"' || next == '\\' => {
                            current.push(next);
                            chars.next();
                        }
                        _ => current.push(c),
                    }
                } else {
                    current.push(c);
                }
            }
            None => match c {
                '"' | '\'' => {
                    quote = Some(c);
                    token_started = true;
                }
                '\\' => {
                    if let Some(next) = chars.next() {
                        current.push(next);
                    }
                    token_started = true;
                }
                c if c.is_whitespace() => {
                    if token_started {
                        out.push(std::mem::take(&mut current));
                        token_started = false;
                    }
                }
                _ => {
                    current.push(c);
                    token_started = true;
                }
            },
        }
    }
    if token_started {
        out.push(current);
    }
    out
}

#[cfg(test)]
fn build_lobby_hosts(areas: &[SdoArea]) -> String {
    areas
        .iter()
        .map(|a| format!("{}:54994", a.area_lobby))
        .collect::<Vec<_>>()
        .join("|")
}

/// 确保登录入口 DLL 是 ottercorp 修改版。
///
/// 对应 C# `SdoLauncher.EnsureLoginEntry()`。原版 `sdologinentry64.dll` 有认证保护，
/// 必须使用修改版才能通过第三方启动器登录。
#[instrument]
pub async fn ensure_login_entry(game_path: &std::path::Path) -> Result<(), GameLaunchError> {
    let game_root = game_path
        .parent()
        .and_then(|p| p.parent())
        .unwrap_or_else(|| std::path::Path::new("."));
    let boot_path = game_root.join("sdo/sdologin");
    let entry_dll = boot_path.join("sdologinentry64.dll");

    info!(game_root = %game_root.display(), "ensuring login entry DLL");

    std::fs::create_dir_all(&boot_path).map_err(GameLaunchError::Io)?;
    debug!(boot_path = %boot_path.display(), "created boot directory");

    if !entry_dll.exists() {
        info!(path = %entry_dll.display(), "DLL not found, downloading ottercorp version");
        let src = download_ottercorp_dll().await?;
        std::fs::copy(&src, &entry_dll).map_err(GameLaunchError::Io)?;
        info!(src = %src.display(), dst = %entry_dll.display(), "copied ottercorp sdologinentry64.dll");
    } else if !is_ottercorp_dll(&entry_dll) {
        info!(path = %entry_dll.display(), "existing DLL is not ottercorp version, replacing");
        let backup = boot_path.join("sdologinentry64.sdo.dll");
        // 已有备份时不要覆盖：备份是修改版 shim 转发的目标，
        // 一旦检测误判再次覆盖备份会永久丢失原版 DLL。
        if !backup.exists() {
            std::fs::copy(&entry_dll, &backup).map_err(GameLaunchError::Io)?;
            info!(backup = %backup.display(), "backed up original DLL");
        } else {
            info!(backup = %backup.display(), "backup already exists, keeping it");
        }
        let src = download_ottercorp_dll().await?;
        std::fs::copy(&src, &entry_dll).map_err(GameLaunchError::Io)?;
        info!("replaced sdologinentry64.dll with ottercorp version");
    } else {
        debug!(path = %entry_dll.display(), "existing DLL is already ottercorp version");
    }

    Ok(())
}

#[instrument]
async fn download_ottercorp_dll() -> Result<std::path::PathBuf, GameLaunchError> {
    const DLL_URL: &str = "https://raw.githubusercontent.com/ottercorp/XIVLauncher.Core/cn/src/XIVLauncher.Core/Resources/binaries/sdologinentry64.dll";

    let tools_dir = dirs::home_dir()
        .map(|h| h.join(".xiv-launcher-rs/tools"))
        .unwrap_or_else(|| std::path::PathBuf::from("./tools"));
    std::fs::create_dir_all(&tools_dir).map_err(GameLaunchError::Io)?;

    let dll_path = tools_dir.join("sdologinentry64.dll");

    if dll_path.exists() {
        debug!(path = %dll_path.display(), "DLL already cached locally");
        return Ok(dll_path);
    }

    info!(
        url = DLL_URL,
        "downloading sdologinentry64.dll from ottercorp"
    );
    let client = reqwest::Client::new();
    let response = client.get(DLL_URL).send().await.map_err(|e| {
        error!(error = %e, "download request failed");
        GameLaunchError::Wine(format!("Download failed: {e}"))
    })?;

    let status = response.status();
    if !status.is_success() {
        error!(status = %status, "download returned non-success status");
        return Err(GameLaunchError::Wine(format!("HTTP {}", status)));
    }
    info!(status = %status, "download request successful");

    let bytes = response.bytes().await.map_err(|e| {
        error!(error = %e, "reading response bytes failed");
        GameLaunchError::Wine(format!("Read failed: {e}"))
    })?;

    let len = bytes.len();
    info!(bytes = len, "downloaded DLL");

    std::fs::write(&dll_path, &bytes).map_err(GameLaunchError::Io)?;
    info!(path = %dll_path.display(), bytes = len, "saved DLL to disk");

    Ok(dll_path)
}

/// 检查 DLL 是否为 ottercorp 修改版（通过 PE 文件 version info 中的 CompanyName）。
///
/// 注意：PE version info 中的字符串以 UTF-16LE 存储（`o\0t\0t\0...`），
/// 直接按字节/UTF-8 搜索 "ottercorp" 永远匹配不到，必须同时检查两种编码。
#[instrument]
fn is_ottercorp_dll(path: &std::path::Path) -> bool {
    debug!(path = %path.display(), "checking if DLL is ottercorp version");
    if let Ok(data) = std::fs::read(path) {
        const ASCII: &[u8] = b"ottercorp";
        // UTF-16LE 编码的 "ottercorp"
        const UTF16LE: &[u8] = b"o\0t\0t\0e\0r\0c\0o\0r\0p\0";
        let found = |needle: &[u8]| data.windows(needle.len()).any(|w| w == needle);
        let result = found(ASCII) || found(UTF16LE);
        debug!(is_ottercorp = result, "DLL check complete");
        return result;
    }
    warn!(path = %path.display(), "failed to read DLL for ottercorp check");
    false
}

/// 启动游戏进程。
///
/// macOS/Linux 通过 Wine 运行，Windows 直接运行。
/// `wine` 参数决定使用哪个 Wine（及 prefix、esync/fsync、DXVK 等设置）。
#[instrument(skip(config, wine))]
pub async fn launch_game(
    config: &GameLaunchConfig,
    wine: &WineSettings,
) -> Result<GameLaunchResult, GameLaunchError> {
    info!(
        game_path = %config.game_path.display(),
        wine_type = ?wine.startup_type,
        "launching game"
    );

    #[cfg(not(target_os = "windows"))]
    validate_wine_game_path(&config.game_path)?;

    // 确保登录 DLL 是修改版
    ensure_login_entry(&config.game_path).await?;

    let args = build_sdo_launch_args(config);
    let arg_list = build_sdo_launch_args_vec(config);
    let game_path = &config.game_path;
    let working_dir = game_path
        .parent()
        .unwrap_or_else(|| std::path::Path::new("."));
    let log_path = default_game_log_path();

    info!(working_dir = %working_dir.display(), "determined working directory");

    let masked_args = args
        .replace(&config.session_id, &mask_sensitive(&config.session_id))
        .replace(&config.snda_id, &mask_sensitive(&config.snda_id));
    info!(command = %format!("{} {}", game_path.display(), masked_args), "full launch command");

    #[cfg(target_os = "windows")]
    {
        let mut cmd = Command::new(game_path);
        cmd.args(&arg_list).current_dir(working_dir);
        redirect_to_log(&mut cmd, &log_path);
        let child = cmd.spawn().map_err(|e| {
            error!(error = %e, "failed to spawn game process");
            GameLaunchError::Io(e)
        })?;

        info!(pid = child.id(), "game process spawned");

        Ok(GameLaunchResult {
            child,
            command: format!("{} {}", game_path.display(), args),
            log_path: Some(log_path),
        })
    }

    #[cfg(not(target_os = "windows"))]
    {
        let tool = WineTool::resolve(wine).await.map_err(|e| {
            error!(error = %e, "failed to resolve Wine");
            GameLaunchError::Wine(format!("{e}"))
        })?;

        // 校验 wine 可执行（wine64 --version）
        if let Err(e) = tool.probe() {
            error!(error = %e, "wine probe failed");
            return Err(GameLaunchError::Wine(format!("wine probe failed: {e}")));
        }

        // 需要 DXVK 时确保已安装
        if wine.dxvk.enabled {
            tool.ensure_dxvk().await.map_err(|e| {
                error!(error = %e, "DXVK setup failed");
                GameLaunchError::Wine(format!("DXVK setup failed: {e}"))
            })?;
            info!("DXVK setup complete");
        }

        let mut env = build_launch_env(wine, &tool);

        // 启用 Dalamud → 通过 Injector 创建游戏；否则直接 wine 运行
        if let Some(d) = &config.dalamud {
            let to_win = |p: &std::path::Path| {
                crate::dalamud::runner::unix_to_wine_path(&tool, p)
                    .map_err(|e| GameLaunchError::Wine(format!("winepath failed: {e}")))
            };

            // Injector 会在 game 进程内创建这些目录；但插件/配置目录缺失会导致
            // 部分版本直接失败，这里预先创建，避免首启报错难以排查。
            for dir in [
                d.config_path.parent(),
                d.log_path.parent(),
                Some(d.plugin_dir.as_path()),
                Some(d.asset_dir.as_path()),
            ]
            .into_iter()
            .flatten()
            {
                std::fs::create_dir_all(dir).map_err(GameLaunchError::Io)?;
            }

            // 托管了 Windows .NET runtime 时设置 DALAMUD_RUNTIME 与 DOTNET_ROOT。
            // 注意：这是 Windows 路径，必须经过 winepath；自定义 env 中的同名项
            // 不做“用户比 launcher 更懂”的假设，统一由检测到的 runtime 覆盖。
            if let Some(runtime_dir) = &d.runtime_dir {
                let runtime_win = to_win(runtime_dir)?;
                env.retain(|(k, _)| {
                    !k.eq_ignore_ascii_case("DALAMUD_RUNTIME")
                        && !k.eq_ignore_ascii_case("DOTNET_ROOT")
                });
                env.push(("DALAMUD_RUNTIME".to_string(), runtime_win.clone()));
                // 让 Injector.exe 的 .NET apphost 直接找到 hostfxr，不依赖 wine-mono。
                env.push(("DOTNET_ROOT".to_string(), runtime_win));
            }

            let start = crate::dalamud::model::DalamudStartInfo {
                game_path: to_win(game_path)?,
                working_directory: to_win(&d.install_dir)?,
                configuration_path: to_win(&d.config_path)?,
                logging_path: to_win(&d.log_path)?,
                plugin_directory: to_win(&d.plugin_dir)?,
                asset_directory: to_win(&d.asset_dir)?,
                client_language: 4,
                delay_initialize_ms: d.delay_initialize_ms,
                no_plugins: d.no_plugins,
                no_third_party_plugins: d.no_third_party_plugins,
            };
            let launch = crate::dalamud::runner::launch_through_injector(
                &tool,
                &d.injector_exe,
                &start,
                d.load_method,
                &arg_list,
                false,
                &env,
            )
            .map_err(|e| {
                error!(error = %e, "failed to launch through Dalamud Injector");
                GameLaunchError::Wine(e.to_string())
            })?;

            info!(
                wine_pid = launch.wine_pid,
                "game spawned through Dalamud Injector"
            );
            return Ok(GameLaunchResult {
                child: launch.injector,
                command: format!("{:?} {:?} {}", tool.wine64_path, d.injector_exe, args),
                log_path: Some(log_path),
                wine_pid: Some(launch.wine_pid),
            });
        }

        let child = tool
            .run(game_path, &arg_list, working_dir, &env, Some(&log_path))
            .map_err(|e| {
                error!(error = %e, "failed to run game through Wine");
                GameLaunchError::Io(e)
            })?;

        let pid = child.id();
        info!(pid, "game process spawned through Wine");

        Ok(GameLaunchResult {
            child,
            command: format!("{:?} {:?} {}", tool.wine64_path, game_path, args),
            log_path: Some(log_path),
            wine_pid: Some(pid),
        })
    }
}

#[cfg(not(target_os = "windows"))]
fn validate_wine_game_path(path: &std::path::Path) -> Result<(), GameLaunchError> {
    if path.to_string_lossy().is_ascii() {
        return Ok(());
    }

    Err(GameLaunchError::UnsupportedWinePath(path.to_path_buf()))
}

#[derive(Debug, thiserror::Error)]
pub enum GameLaunchError {
    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),
    #[error("Wine error: {0}")]
    Wine(String),
    #[error(
        "Wine game path contains non-ASCII characters: {0}. Move the game to an ASCII-only path"
    )]
    UnsupportedWinePath(PathBuf),
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_area(id: &str, name: &str, lobby: &str, gm: &str, config: &str) -> SdoArea {
        SdoArea {
            area_id: id.to_string(),
            area_stat: 1,
            area_order: 1,
            area_name: name.to_string(),
            area_type: 1,
            area_lobby: lobby.to_string(),
            area_gm: gm.to_string(),
            area_patch: format!("patch{}", lobby),
            area_config_upload: config.to_string(),
        }
    }

    #[test]
    fn test_build_sdo_launch_args() {
        let area = make_area(
            "1",
            "陆行鸟",
            "ffxivlobby01.ff14.sdo.com",
            "ffxivgm01.ff14.sdo.com",
            "ffxivsdb01.ff14.sdo.com",
        );
        let areas = vec![area.clone()];

        let config = GameLaunchConfig {
            game_path: PathBuf::from("/path/to/ffxiv_dx11.exe"),
            session_id: "ticket123".to_string(),
            snda_id: "snda456".to_string(),
            area,
            areas,
            max_expansion: 1,
            dc_travel_port: Some(57001),
            reset_config: 0,
            additional_args: String::new(),
            dalamud: None,
        };

        let args = build_sdo_launch_args(&config);

        assert!(args.contains("-AppID=100001900"));
        assert!(args.contains("-AreaID=1"));
        assert!(args.contains("DEV.TestSID=ticket123"));
        assert!(args.contains("XL.SndaId=snda456"));
        assert!(args.contains("XL.DcTraveler=57001"));
        assert!(args.contains("Dev.LobbyHost01=ffxivlobby01.ff14.sdo.com"));
        // C# MainPage 传 areasInfo=""（空），对齐后不携带服务器列表
        assert!(args.contains("XL.LobbyHosts="));
    }

    #[test]
    fn test_parse_additional_args_preserves_quoted_spaces() {
        assert_eq!(
            parse_additional_args("-foo=bar -path=\"C:\\Games\\FFXIV\\game\" --opt='a b'"),
            vec!["-foo=bar", "-path=C:\\Games\\FFXIV\\game", "--opt=a b"]
        );
        assert!(parse_additional_args("").is_empty());
    }

    #[test]
    fn test_build_lobby_hosts() {
        let areas = vec![
            make_area("1", "A", "lobby1.sdo.com", "gm1.sdo.com", "cfg1.sdo.com"),
            make_area("6", "B", "lobby5.sdo.com", "gm5.sdo.com", "cfg5.sdo.com"),
        ];
        let hosts = build_lobby_hosts(&areas);
        assert_eq!(hosts, "lobby1.sdo.com:54994|lobby5.sdo.com:54994");
    }

    #[test]
    fn test_is_ottercorp_dll() {
        let dir = std::env::temp_dir().join(format!("xlrs-dll-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();

        // PE version info 以 UTF-16LE 存储 CompanyName
        let utf16 = dir.join("utf16.dll");
        std::fs::write(&utf16, b"\x00\x01o\0t\0t\0e\0r\0c\0o\0r\0p\0\x00\x01").unwrap();
        assert!(is_ottercorp_dll(&utf16));

        // 兼容纯 ASCII 出现的情况
        let ascii = dir.join("ascii.dll");
        std::fs::write(&ascii, b"prefix ottercorp suffix").unwrap();
        assert!(is_ottercorp_dll(&ascii));

        // 原版 DLL（不含 ottercorp）
        let original = dir.join("original.dll");
        std::fs::write(&original, b"Shanda Games sdologinentry64").unwrap();
        assert!(!is_ottercorp_dll(&original));

        // 真实缓存的修改版 DLL（如存在）必须被识别
        if let Some(home) = dirs::home_dir() {
            let cached = home.join(".xiv-launcher-rs/tools/sdologinentry64.dll");
            if cached.exists() {
                assert!(
                    is_ottercorp_dll(&cached),
                    "cached ottercorp DLL not detected"
                );
            }
        }

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[cfg(not(target_os = "windows"))]
    #[test]
    fn test_validate_wine_game_path() {
        assert!(validate_wine_game_path(std::path::Path::new(
            "/home/user/Games/ffxiv/game/ffxiv_dx11.exe"
        ))
        .is_ok());
        assert!(validate_wine_game_path(std::path::Path::new(
            "/home/user/Games/最终幻想XIV/game/ffxiv_dx11.exe"
        ))
        .is_err());
    }
}

/// 将命令的 stdout/stderr 重定向到日志文件（Windows 直启分支用）。
#[cfg(target_os = "windows")]
fn redirect_to_log(cmd: &mut std::process::Command, log_path: &PathBuf) {
    if let Some(parent) = log_path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    if let Ok(file) = std::fs::File::create(log_path) {
        if let Ok(err_file) = file.try_clone() {
            cmd.stdout(std::process::Stdio::from(file));
            cmd.stderr(std::process::Stdio::from(err_file));
        }
    }
}
