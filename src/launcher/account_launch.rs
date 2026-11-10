//! GUI/CLI 共用的「准备 → 重新认证 → 保存凭据 → 启动」流程。

use std::path::Path;

use crate::auth::{self, AuthConfig};
use crate::config::WineSettings;
use crate::game::{GameLaunchError, GameLaunchResult};
use eorzea_auth::SdoArea;

use super::{DalamudPhase, Launcher, LauncherError};

/// 界面提供的启动参数；`game_root` 是包含 `game/` 的游戏根目录。
pub struct LaunchRequest<'a> {
    pub game_root: &'a Path,
    pub auth_path: &'a Path,
    /// 账号名或 snda_id；`None` 使用已保存的默认账号。
    pub account: Option<&'a str>,
    pub area: SdoArea,
    pub areas: Vec<SdoArea>,
    pub wine: &'a WineSettings,
    pub dalamud: Option<bool>,
}

/// 仅包含展示信息，不向界面暴露可缓存的游戏票据。
pub enum LaunchEvent {
    DalamudProgress(DalamudPhase, u64, u64),
    LoggingIn {
        account_name: String,
    },
    SessionRefreshed {
        account_name: String,
        remaining_seconds: Option<i32>,
    },
    Starting {
        dalamud_version: Option<String>,
    },
}

fn load_config(path: &Path) -> Result<AuthConfig, LauncherError> {
    // 启动不能把损坏/不可读的配置当作空配置，再覆盖掉其他账号。
    let content = std::fs::read_to_string(path)
        .map_err(|e| LauncherError::Auth(format!("读取账号配置失败: {e}")))?;
    toml::from_str(&content)
        .map_err(|_| LauncherError::Auth("账号配置格式无效，请检查 auth.toml".into()))
}

impl Launcher {
    /// 使用已保存账号启动游戏。每次调用都换取新 ticket，失败重试也不复用。
    ///
    /// 进度回调在调用方线程执行，可用于终端进度条或 GUI signal。
    pub async fn launch_account(
        &self,
        request: LaunchRequest<'_>,
        mut on_event: impl FnMut(LaunchEvent),
    ) -> Result<GameLaunchResult, LauncherError> {
        let exe = request.game_root.join("game/ffxiv_dx11.exe");
        if crate::game_files::update::has_pending_update(request.game_root) {
            return Err(GameLaunchError::UpdateInProgress.into());
        }
        if !exe.is_file() {
            return Err(GameLaunchError::Io(std::io::Error::new(
                std::io::ErrorKind::NotFound,
                format!("未找到游戏可执行文件: {}", exe.display()),
            ))
            .into());
        }
        #[cfg(not(target_os = "windows"))]
        crate::game::validate_wine_game_path(&exe)?;

        // Dalamud 组件准备在自动登录前完成，避免浪费短期票据。
        let dalamud = self
            .prepare_dalamud(request.game_root, request.dalamud, |phase, done, total| {
                on_event(LaunchEvent::DalamudProgress(phase, done, total));
            })
            .await?;

        // 两个界面同时启动时，不能同时使用同一个会轮换的 session key。
        let lock = std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(request.auth_path.with_extension("launch.lock"))
            .map_err(|e| LauncherError::Auth(format!("打开账号启动锁失败: {e}")))?;
        lock.try_lock().map_err(|e| {
            LauncherError::Auth(format!("账号配置正被另一个启动任务使用，请稍后重试: {e}"))
        })?;

        // 在准备完成后才读盘，CLI 或另一个窗口可能已经轮换过凭据。
        let cfg = load_config(request.auth_path)?;
        let account = match request.account {
            Some(id) => cfg.find_by_identifier(id),
            None => cfg.default_account(),
        }
        .ok_or_else(|| {
            LauncherError::Auth("未找到已保存账号，请先添加账号或设置默认账号".into())
        })?;
        let key = account
            .auto_login_session_key
            .as_deref()
            .filter(|key| !key.is_empty())
            .ok_or_else(|| {
                LauncherError::Auth("该账号没有自动登录凭据，请重新登录并保存账号".into())
            })?;
        let account_name = account.display_name().to_string();
        on_event(LaunchEvent::LoggingIn {
            account_name: account_name.clone(),
        });
        let token = self.login_auto(key).await?;
        if token.snda_id != account.snda_id {
            return Err(LauncherError::Auth(
                "认证返回的账号与所选账号不一致，已中止启动".into(),
            ));
        }
        let new_key = token
            .auto_login_session_key
            .as_ref()
            .filter(|key| !key.is_empty())
            .ok_or_else(|| {
                LauncherError::Auth("自动登录未返回新的 session key，已中止启动".into())
            })?;

        // 登录期间可能修改过默认账号/大区或添加了其他账号，只合并本账号的轮换 key。
        let mut latest = load_config(request.auth_path)?;
        let saved = latest
            .accounts
            .iter_mut()
            .find(|a| a.snda_id == account.snda_id)
            .ok_or_else(|| LauncherError::Auth("登录期间账号已被移除，已中止启动".into()))?;
        if saved.auto_login_session_key.as_deref() != Some(key) {
            return Err(LauncherError::Auth(
                "登录期间凭据已被其他操作更新，请重新启动".into(),
            ));
        }
        saved.auto_login_session_key = Some(new_key.clone());
        auth::save(request.auth_path, &latest).map_err(|e| {
            LauncherError::Auth(format!(
                "保存轮换后的自动登录凭据失败，请检查配置后重新登录: {e}"
            ))
        })?;
        drop(lock);
        on_event(LaunchEvent::SessionRefreshed {
            account_name,
            remaining_seconds: token.auto_login_max_age,
        });
        on_event(LaunchEvent::Starting {
            dalamud_version: dalamud.as_ref().and_then(|d| {
                d.install_dir
                    .file_name()
                    .map(|v| v.to_string_lossy().into_owned())
            }),
        });
        self.launch_prepared(
            request.wine,
            dalamud,
            &token,
            request.area,
            request.areas,
            &exe,
        )
        .await
    }
}

#[cfg(all(test, unix))]
mod tests;
