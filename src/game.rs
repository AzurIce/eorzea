use std::path::PathBuf;
use tracing::{debug, error, info, instrument, warn};
use xiv_launcher_auth::SdoArea;
use crate::wine::WineTool;

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
}

/// 游戏启动结果。
#[derive(Debug)]
pub struct GameLaunchResult {
    /// 启动的子进程。
    pub child: std::process::Child,
    /// 完整的启动命令行。
    pub command: String,
}

/// 构建国服启动参数字符串。
///
/// 参考 C# `SdoLauncher.LaunchGameSdo()` 的参数构造方式。
#[instrument(skip(config))]
pub fn build_sdo_launch_args(config: &GameLaunchConfig) -> String {
    let areas_info = build_lobby_hosts(&config.areas);

    let mut args = Vec::new();

    args.push(format!("-AppID={}", 100001900));
    args.push(format!("-AreaID={}", config.area.area_id));
    args.push(format!("Dev.LobbyHost01={}", config.area.area_lobby));
    args.push("Dev.LobbyPort01=54994".to_string());
    args.push(format!("Dev.GMServerHost={}", config.area.area_gm));
    args.push(format!("Dev.SaveDataBankHost={}", config.area.area_config_upload));
    args.push(format!("resetConfig={}", config.reset_config));
    args.push(format!("DEV.MaxEntitledExpansionID={}", config.max_expansion));
    args.push(format!("DEV.TestSID={}", config.session_id));
    args.push(format!("XL.SndaId={}", config.snda_id));
    args.push(format!("XL.LobbyHosts={}", areas_info));

    if let Some(port) = config.dc_travel_port {
        args.push(format!("XL.DcTraveler={}", port));
    }

    if !config.additional_args.is_empty() {
        args.push(config.additional_args.clone());
    }

    let result = args.join(" ");
    let masked = result
        .replace(&config.session_id, &mask_sensitive(&config.session_id))
        .replace(&config.snda_id, &mask_sensitive(&config.snda_id));
    debug!("constructed launch args: {}", masked);

    result
}

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
        std::fs::copy(&entry_dll, &backup).map_err(GameLaunchError::Io)?;
        info!(backup = %backup.display(), "backed up original DLL");
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

    info!(url = DLL_URL, "downloading sdologinentry64.dll from ottercorp");
    let client = reqwest::Client::new();
    let response = client
        .get(DLL_URL)
        .send()
        .await
        .map_err(|e| {
            error!(error = %e, "download request failed");
            GameLaunchError::Wine(format!("Download failed: {e}"))
        })?;

    let status = response.status();
    if !status.is_success() {
        error!(status = %status, "download returned non-success status");
        return Err(GameLaunchError::Wine(format!("HTTP {}", status)));
    }
    info!(status = %status, "download request successful");

    let bytes = response
        .bytes()
        .await
        .map_err(|e| {
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
#[instrument]
fn is_ottercorp_dll(path: &std::path::Path) -> bool {
    // 简单检查：读取文件前 2KB，搜索 "ottercorp" 字符串
    debug!(path = %path.display(), "checking if DLL is ottercorp version");
    if let Ok(data) = std::fs::read(path) {
        let text = String::from_utf8_lossy(&data);
        let result = text.contains("ottercorp");
        debug!(is_ottercorp = result, "DLL check complete");
        return result;
    }
    warn!(path = %path.display(), "failed to read DLL for ottercorp check");
    false
}

/// 启动游戏进程。
///
/// macOS/Linux 通过 Wine 运行，Windows 直接运行。
#[instrument(skip(config))]
pub async fn launch_game(
    config: &GameLaunchConfig,
    custom_wine_path: Option<&std::path::Path>,
) -> Result<GameLaunchResult, GameLaunchError> {
    info!(
        game_path = %config.game_path.display(),
        "launching game"
    );

    // 确保登录 DLL 是修改版
    ensure_login_entry(&config.game_path).await?;

    let args = build_sdo_launch_args(config);
    let game_path = &config.game_path;
    let working_dir = game_path
        .parent()
        .unwrap_or_else(|| std::path::Path::new("."));

    info!(working_dir = %working_dir.display(), "determined working directory");

    let masked_args = args
        .replace(&config.session_id, &mask_sensitive(&config.session_id))
        .replace(&config.snda_id, &mask_sensitive(&config.snda_id));
    info!(command = %format!("{} {}", game_path.display(), masked_args), "full launch command");

    #[cfg(target_os = "windows")]
    {
        let child = Command::new(game_path)
            .args(args.split_whitespace())
            .current_dir(working_dir)
            .spawn()
            .map_err(|e| {
                error!(error = %e, "failed to spawn game process");
                GameLaunchError::Io(e)
            })?;

        info!(pid = child.id(), "game process spawned");

        Ok(GameLaunchResult {
            child,
            command: format!("{} {}", game_path.display(), args),
        })
    }

    #[cfg(not(target_os = "windows"))]
    {
        let wine = WineTool::ensure(custom_wine_path)
            .await
            .map_err(|e| {
                error!(error = %e, "failed to ensure Wine installation");
                GameLaunchError::Wine(format!("{e}"))
            })?;

        // 确保 DXVK 已安装
        wine.ensure_dxvk().await
            .map_err(|e| {
                error!(error = %e, "DXVK setup failed");
                GameLaunchError::Wine(format!("DXVK setup failed: {e}"))
            })?;
        info!("DXVK setup complete");

        let arg_list: Vec<String> = args.split_whitespace().map(|s| s.to_string()).collect();

        // macOS 需要设置 WINEDLLOVERRIDES 来使用 DXVK
        #[cfg(target_os = "macos")]
        let env = vec![
            ("WINEDLLOVERRIDES".to_string(), "msquic=,mscoree=n,b;d3d11=n;dxgi=n,b".to_string()),
        ];
        #[cfg(not(target_os = "macos"))]
        let env = vec![
            ("WINEDLLOVERRIDES".to_string(), "msquic=,mscoree=n,b;d3d9,d3d11,d3d10core,dxgi=n".to_string()),
        ];

        let child = wine
            .run(game_path, &arg_list, working_dir, &env)
            .map_err(|e| {
                error!(error = %e, "failed to run game through Wine");
                GameLaunchError::Io(e)
            })?;

        info!(pid = child.id(), "game process spawned through Wine");

        Ok(GameLaunchResult {
            child,
            command: format!("{:?} {:?} {}", wine.wine64_path, game_path, args),
        })
    }
}

#[derive(Debug, thiserror::Error)]
pub enum GameLaunchError {
    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),
    #[error("Wine error: {0}")]
    Wine(String),
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
        };

        let args = build_sdo_launch_args(&config);

        assert!(args.contains("-AppID=100001900"));
        assert!(args.contains("-AreaID=1"));
        assert!(args.contains("DEV.TestSID=ticket123"));
        assert!(args.contains("XL.SndaId=snda456"));
        assert!(args.contains("XL.DcTraveler=57001"));
        assert!(args.contains("Dev.LobbyHost01=ffxivlobby01.ff14.sdo.com"));
        assert!(args.contains("XL.LobbyHosts=ffxivlobby01.ff14.sdo.com:54994"));
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
}
