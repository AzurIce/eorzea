//! 完整的登录 → 启动链路封装。
//!
//! 将 `xiv-launcher-auth` 的认证能力和 `game.rs` / `wine.rs` 的启动能力
//! 组合成高层 API，供 Tauri 前端或 CLI 示例直接调用。
//!
//! # 设计原则
//!
//! **登录** 和 **启动** 完全解耦：
//! - 登录返回 `LaunchToken`，不依赖游戏路径。
//! - 启动接受 `LaunchToken` + 大区 + 游戏路径。
//!
//! QR 登录为**分步式**：先生成二维码展示，再在后台等待扫码结果。

use std::path::Path;
use std::time::Duration;
use tracing::{debug, info, instrument};
use xiv_launcher_auth::sdo::{PollResult, SdoAuth, SdoContext};
use xiv_launcher_auth::{SdoArea, SdoLoginData};

use crate::game::{GameLaunchConfig, GameLaunchError, GameLaunchResult};
use crate::settings::{WineSettings, WineStartupType};

/// 登录凭证（ticket + snda_id + 可选的自动登录 session_key）。
#[derive(Debug, Clone)]
pub struct LaunchToken {
    /// 游戏 session ID（`DEV.TestSID`）。
    pub ticket: String,
    /// SDO 账号 ID（`XL.SndaId`）。
    pub snda_id: String,
    /// 账号名（`inputUserId`，如有）。
    pub username: Option<String>,
    /// 自动登录 session key（如有）。
    pub auto_login_session_key: Option<String>,
    /// 自动登录 session key 剩余有效期（秒，`autoLoginMaxAge`）。
    pub auto_login_max_age: Option<i32>,
}

/// 扫码登录会话。
///
/// 由 [`Launcher::request_qr_code`] 创建，包含二维码图片和扫码状态轮询能力。
pub struct QrCodeSession {
    code_key: String,
    image_data: Vec<u8>,
    auth: SdoAuth,
    ctx: SdoContext,
}

impl std::fmt::Debug for QrCodeSession {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("QrCodeSession")
            .field("code_key", &self.code_key)
            .field("image_data", &format!("{} bytes", self.image_data.len()))
            .finish()
    }
}

impl QrCodeSession {
    /// 二维码 PNG 图片字节（可直接展示或保存）。
    pub fn image_data(&self) -> &[u8] {
        &self.image_data
    }

    /// 二维码标识（调试用）。
    pub fn code_key(&self) -> &str {
        &self.code_key
    }

    /// 阻塞等待用户扫码并确认。
    ///
    /// - `timeout = None` → 无限等待
    /// - `timeout = Some(d)` → `d` 后返回 `LauncherError::Auth("QR code scan timed out")`
    ///
    /// 前端应在**后台 task**中调用此方法，主线程继续响应 UI。
    #[instrument(skip(self))]
    pub async fn wait_for_scan(
        &self,
        timeout: Option<Duration>,
    ) -> Result<LaunchToken, LauncherError> {
        let start = std::time::Instant::now();
        let poll_interval = tokio::time::Duration::from_secs(3);

        info!(timeout = ?timeout, "waiting for QR scan");

        loop {
            tokio::time::sleep(poll_interval).await;

            if let Some(t) = timeout {
                if start.elapsed() >= t {
                    return Err(LauncherError::Auth("QR code scan timed out".into()));
                }
            }

            match self.auth.qr_code_poll(&self.ctx, &self.code_key, 30).await {
                Ok(PollResult::Success(data)) => {
                    info!("QR code scan confirmed");
                    return self.exchange_for_token(data).await;
                }
                Ok(PollResult::Pending) => {
                    debug!("qr code scan pending...");
                }
                Err(e) => {
                    return Err(LauncherError::Auth(format!("qr_code_poll failed: {e}")));
                }
            }
        }
    }

    async fn exchange_for_token(
        &self,
        data: SdoLoginData,
    ) -> Result<LaunchToken, LauncherError> {
        let snda_id = data
            .snda_id
            .ok_or_else(|| LauncherError::Auth("no snda_id in qr response".into()))?;
        let mut tgt = data
            .tgt
            .ok_or_else(|| LauncherError::Auth("no tgt in qr response".into()))?;

        // 扫码确认：codeKeyLogin 响应已带 auto_login_session_key（autoLoginFlag=1）
        // 再调 accountGroupLogin 刷新 tgt + key（对应 C#；失败不影响登录）
        let mut auto_login_session_key = data.auto_login_session_key;
        let exchange_result: Result<(String, String), LauncherError> = async {
            // 1. 获取账号组（C# 在 AccountGroupLogin 前调用）
            self.auth
                .get_account_group(&tgt, &snda_id)
                .await
                .map_err(|e| LauncherError::Auth(format!("get_account_group failed: {e}")))?;
            // 2. 刷新 tgt + 拿 session key
            self.auth
                .account_group_login(&self.ctx, &tgt, &snda_id, AUTO_LOGIN_KEEP_DAYS)
                .await
                .map_err(|e| LauncherError::Auth(format!("account_group_login failed: {e}")))
        }
        .await;
        match exchange_result {
            Ok((new_tgt, session_key)) => {
                info!("accountGroupLogin successful, refreshed tgt and session key");
                tgt = new_tgt;
                auto_login_session_key = Some(session_key);
            }
            Err(e) => {
                if auto_login_session_key.is_some() {
                    info!(error = %e, "key exchange failed, using codeKeyLogin session key");
                } else {
                    info!(error = %e, "key exchange failed, continuing without auto-login key");
                }
            }
        };

        // 激活权限
        self.auth
            .get_promotion_info(&tgt)
            .await
            .map_err(|e| LauncherError::Auth(format!("get_promotion_info failed: {e}")))?;

        // 换取 ticket
        let ticket = self
            .auth
            .sso_login(&self.ctx, &tgt)
            .await
            .map_err(|e| LauncherError::Auth(format!("sso_login failed: {e}")))?;

        info!(ticket = %mask_sensitive(&ticket), "qr login successful");

        Ok(LaunchToken {
            ticket,
            snda_id,
            username: data.input_user_id,
            auto_login_session_key,
            auto_login_max_age: data.auto_login_max_age,
        })
    }
}

/// 自动登录保持天数（对应 C# `AutoLoginKeepDays`）。
const AUTO_LOGIN_KEEP_DAYS: i32 = 30;

/// 封装完整链路的高层启动器。
pub struct Launcher {
    /// 底层 SDO 认证客户端（如需直接调用底层 API，可通过此字段访问）。
    pub auth: SdoAuth,
    /// 启动游戏时使用的 Wine 配置（可通过 [`Self::with_wine_settings`] 修改，
    /// 或每次启动时用 [`Self::launch_with_wine`] 覆盖）。
    wine_settings: WineSettings,
}

impl Launcher {
    /// 创建新的 Launcher。
    ///
    /// 内部会自动初始化 `SdoAuth` 并采集设备指纹。
    /// Wine 配置默认使用 [`WineSettings::default`]（Auto 模式）。
    #[instrument]
    pub fn new() -> Result<Self, LauncherError> {
        let auth = SdoAuth::new().map_err(|e| LauncherError::Auth(format!("{e}")))?;
        info!("Launcher created, device_id collected");
        Ok(Self {
            auth,
            wine_settings: WineSettings::default(),
        })
    }

    /// 设置 Wine 配置（可选）。
    ///
    /// 如果不设置，启动时使用默认配置（自动检测或下载 Wine）。
    pub fn with_wine_settings(mut self, settings: WineSettings) -> Self {
        self.wine_settings = settings;
        self
    }

    /// 便捷方法：指定自定义 Wine 路径。
    ///
    /// 等价于 `with_wine_settings(WineSettings { startup_type: Custom, custom_path: Some(path), .. })`。
    pub fn with_wine_path(mut self, path: impl AsRef<Path>) -> Self {
        self.wine_settings.startup_type = WineStartupType::Custom;
        self.wine_settings.custom_path = Some(path.as_ref().to_path_buf());
        self
    }

    /// 获取当前 Wine 配置。
    pub fn wine_settings(&self) -> &WineSettings {
        &self.wine_settings
    }

    /// 获取设备 ID（用于调试或展示）。
    pub fn device_id(&self) -> String {
        xiv_launcher_auth::sdo_device::get_device_id()
    }

    /// 获取 MAC ID（用于调试或展示）。
    pub fn mac_id(&self) -> String {
        xiv_launcher_auth::sdo_device::get_mac_address_hash()
    }

    // ── 一步登录（密码 / 自动登录）──────────────────────────────────────

    /// 使用账号密码登录，返回 `LaunchToken`。
    #[instrument(skip(self, account, password))]
    pub async fn login_password(
        &self,
        account: &str,
        password: &str,
    ) -> Result<LaunchToken, LauncherError> {
        let ctx = self.get_context().await?;

        let result = self
            .auth
            .static_login(&ctx, account, password)
            .await
            .map_err(|e| LauncherError::Auth(format!("static_login failed: {e}")))?;

        if result.return_code != 0 {
            return Err(LauncherError::Auth(format!(
                "static_login returned error code: {}",
                result.return_code
            )));
        }

        let snda_id = result
            .data
            .snda_id
            .ok_or_else(|| LauncherError::Auth("no snda_id in response".into()))?;
        let tgt = result
            .data
            .tgt
            .ok_or_else(|| LauncherError::Auth("no tgt in response (captcha required?)".into()))?;

        let mut token = self
            .exchange_tgt_for_token(&ctx, &tgt, &snda_id, result.data.auto_login_session_key)
            .await?;
        token.username = Some(account.to_string());
        Ok(token)
    }

    /// 使用自动登录 session key 登录，返回 `LaunchToken`。
    #[instrument(skip(self, session_key))]
    pub async fn login_auto(
        &self,
        session_key: &str,
    ) -> Result<LaunchToken, LauncherError> {
        let ctx = self.get_context().await?;

        let result = self
            .auth
            .auto_login(&ctx, session_key)
            .await
            .map_err(|e| LauncherError::Auth(format!("auto_login failed: {e}")))?;

        let mut snda_id = result
            .data
            .snda_id
            .ok_or_else(|| LauncherError::Auth("no snda_id in auto_login response".into()))?;
        let mut tgt = result
            .data
            .tgt
            .ok_or_else(|| LauncherError::Auth("no tgt in auto_login response".into()))?;
        let new_session_key = result.data.auto_login_session_key;
        let max_age = result.data.auto_login_max_age;

        // fastLogin 再刷新一次 tgt/snda_id（对应 C# `LoginBySessionKey`）
        match self.auth.fast_login(&ctx, &tgt).await {
            Ok((new_snda_id, new_tgt)) => {
                snda_id = new_snda_id;
                tgt = new_tgt;
            }
            Err(e) => {
                info!(error = %e, "fast_login failed, continuing with autoLogin credentials");
            }
        }

        let mut token = self
            .exchange_tgt_for_token(&ctx, &tgt, &snda_id, new_session_key)
            .await?;
        token.auto_login_max_age = max_age;
        Ok(token)
    }

    // ── 分步扫码登录 ──────────────────────────────────────────────────

    /// 请求二维码，返回 [`QrCodeSession`]。
    ///
    /// 前端应调用 [`QrCodeSession::image_data`] 展示二维码，
    /// 然后在后台调用 [`QrCodeSession::wait_for_scan`] 等待扫码结果。
    #[instrument(skip(self))]
    pub async fn request_qr_code(&self) -> Result<QrCodeSession, LauncherError> {
        let ctx = self.get_context().await?;
        let result = self
            .auth
            .qr_code_request(&ctx)
            .await
            .map_err(|e| LauncherError::Auth(format!("qr_code_request failed: {e}")))?;

        info!(code_key = %result.code_key, bytes = result.image_data.len(), "qr code requested");

        Ok(QrCodeSession {
            code_key: result.code_key,
            image_data: result.image_data,
            auth: self.auth.clone(),
            ctx,
        })
    }

    // ── 启动游戏 ──────────────────────────────────────────────────────

    /// 启动游戏（使用当前持久化的 Wine 配置）。
    ///
    /// `game_path` 应指向 `ffxiv_dx11.exe` 的完整路径。
    #[instrument(skip(self, token, area, areas, game_path))]
    pub async fn launch(
        &self,
        token: &LaunchToken,
        area: SdoArea,
        areas: Vec<SdoArea>,
        game_path: impl AsRef<Path>,
    ) -> Result<GameLaunchResult, LauncherError> {
        self.launch_with_wine(&self.wine_settings, token, area, areas, game_path)
            .await
    }

    /// 启动游戏，单次覆盖 Wine 配置（不写回持久化设置）。
    ///
    /// 用于「本次启动换一个 wine」的场景，例如示例/CLI 传入不同 wine 路径。
    #[instrument(skip(self, wine, token, area, areas, game_path))]
    pub async fn launch_with_wine(
        &self,
        wine: &WineSettings,
        token: &LaunchToken,
        area: SdoArea,
        areas: Vec<SdoArea>,
        game_path: impl AsRef<Path>,
    ) -> Result<GameLaunchResult, LauncherError> {
        let game_path = game_path.as_ref().to_path_buf();

        let config = GameLaunchConfig {
            game_path: game_path.clone(),
            session_id: token.ticket.clone(),
            snda_id: token.snda_id.clone(),
            area,
            areas,
            max_expansion: 1,
            dc_travel_port: None,
            reset_config: 0,
            additional_args: String::new(),
        };

        info!(path = %game_path.display(), wine_type = ?wine.startup_type, "launching game");

        let result = crate::game::launch_game(&config, wine)
            .await
            .map_err(LauncherError::from)?;

        info!(pid = %result.child.id(), "game process started");
        Ok(result)
    }

    // ── internal helpers ─────────────────────────────────────────────

    #[instrument(skip(self))]
    async fn get_context(&self) -> Result<SdoContext, LauncherError> {
        let ctx = self
            .auth
            .get_context()
            .await
            .map_err(|e| LauncherError::Auth(format!("get_context failed: {e}")))?;
        debug!(guid = %ctx.guid, "got login context");
        Ok(ctx)
    }

    #[instrument(skip(self, ctx, tgt))]
    async fn exchange_tgt_for_token(
        &self,
        ctx: &SdoContext,
        tgt: &str,
        snda_id: &str,
        session_key: Option<String>,
    ) -> Result<LaunchToken, LauncherError> {
        // 激活权限
        self.auth
            .get_promotion_info(tgt)
            .await
            .map_err(|e| LauncherError::Auth(format!("get_promotion_info failed: {e}")))?;

        // 换取 ticket
        let ticket = self
            .auth
            .sso_login(ctx, tgt)
            .await
            .map_err(|e| LauncherError::Auth(format!("sso_login failed: {e}")))?;

        info!(ticket = %mask_sensitive(&ticket), "login successful");

        Ok(LaunchToken {
            ticket,
            snda_id: snda_id.to_string(),
            username: None,
            auto_login_session_key: session_key,
            auto_login_max_age: None,
        })
    }
}

impl Default for Launcher {
    fn default() -> Self {
        Self::new().expect("Failed to create Launcher")
    }
}

/// Launcher 层错误。
#[derive(Debug, thiserror::Error)]
pub enum LauncherError {
    #[error("Auth error: {0}")]
    Auth(String),
    #[error("Game launch error: {0}")]
    Game(#[from] GameLaunchError),
}

fn mask_sensitive(value: &str) -> String {
    if value.len() <= 8 {
        "***".to_string()
    } else {
        format!("{}***{}", &value[..3], &value[value.len() - 4..])
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_launch_token_new() {
        let token = LaunchToken {
            ticket: "ULS21-abc123".to_string(),
            snda_id: "12345".to_string(),
            username: None,
            auto_login_session_key: None,
            auto_login_max_age: None,
        };
        assert_eq!(token.ticket, "ULS21-abc123");
        assert_eq!(token.snda_id, "12345");
    }

    #[test]
    fn test_mask_sensitive() {
        assert_eq!(mask_sensitive("short"), "***");
        assert_eq!(mask_sensitive("ULS21-abcdef123456"), "ULS***3456");
    }
}
