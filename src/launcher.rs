//! 完整的登录 → 启动链路封装。
//!
//! 将 `xiv-launcher-auth` 的认证能力和 `game.rs` / `wine.rs` 的启动能力
//! 组合成高层 API，供 Tauri 前端或 CLI 示例直接调用。

use std::path::{Path, PathBuf};
use std::time::Duration;
use tracing::{debug, info, instrument};
use xiv_launcher_auth::sdo::{PollResult, SdoAuth, SdoContext};
use xiv_launcher_auth::{SdoArea, SdoLoginData};

use crate::game::{GameLaunchConfig, GameLaunchError, GameLaunchResult};

/// 登录凭证（ticket + snda_id + 可选的自动登录 session_key）。
#[derive(Debug, Clone)]
pub struct LaunchToken {
    /// 游戏 session ID（`DEV.TestSID`）。
    pub ticket: String,
    /// SDO 账号 ID（`XL.SndaId`）。
    pub snda_id: String,
    /// 自动登录 session key（如有）。
    pub auto_login_session_key: Option<String>,
}

/// 登录方式。
pub enum LoginMethod<'a> {
    /// 叨鱼 APP 扫码登录。
    ///
    /// `timeout` 控制扫码等待的最大时长，`None` 表示无限等待。
    QrCode { timeout: Option<Duration> },
    /// 账号密码登录。
    Password { account: &'a str, password: &'a str },
    /// 自动登录（使用之前保存的 session key）。
    AutoLogin { session_key: &'a str },
}

/// 封装完整链路的高层启动器。
pub struct Launcher {
    /// 底层 SDO 认证客户端（如需直接调用底层 API，可通过此字段访问）。
    pub auth: SdoAuth,
    custom_wine_path: Option<PathBuf>,
}

impl Launcher {
    /// 创建新的 Launcher。
    ///
    /// 内部会自动初始化 `SdoAuth` 并采集设备指纹。
    #[instrument]
    pub fn new() -> Result<Self, LauncherError> {
        let auth = SdoAuth::new().map_err(|e| LauncherError::Auth(format!("{e}")))?;
        info!("Launcher created, device_id collected");
        Ok(Self {
            auth,
            custom_wine_path: None,
        })
    }

    /// 设置自定义 Wine 路径（可选）。
    ///
    /// 如果不设置，启动时会自动检测或下载 Wine。
    pub fn with_wine_path(mut self, path: impl AsRef<Path>) -> Self {
        self.custom_wine_path = Some(path.as_ref().to_path_buf());
        self
    }

    /// 获取设备 ID（用于调试或展示）。
    pub fn device_id(&self) -> String {
        xiv_launcher_auth::sdo_device::get_device_id()
    }

    /// 获取 MAC ID（用于调试或展示）。
    pub fn mac_id(&self) -> String {
        xiv_launcher_auth::sdo_device::get_mac_address_hash()
    }

    /// 获取登录上下文（guid 等）。
    #[instrument(skip(self))]
    pub async fn get_context(&self) -> Result<SdoContext, LauncherError> {
        let ctx = self
            .auth
            .get_context()
            .await
            .map_err(|e| LauncherError::Auth(format!("get_context failed: {e}")))?;
        debug!(guid = %ctx.guid, "got login context");
        Ok(ctx)
    }

    /// 使用指定方式登录，返回 `LaunchToken`。
    ///
    /// 这是高层封装，内部处理了从登录到换取 ticket 的完整流程。
    #[instrument(skip(self, method))]
    pub async fn login(&self, method: LoginMethod<'_>) -> Result<LaunchToken, LauncherError> {
        let ctx = self.get_context().await?;

        let (snda_id, tgt, session_key) = match method {
            LoginMethod::QrCode { timeout } => {
                info!(timeout = ?timeout, "starting QR code login flow");
                self.qr_code_flow(&ctx, timeout).await?
            }
            LoginMethod::Password { account, password } => {
                info!("starting password login flow");
                self.password_flow(&ctx, account, password).await?
            }
            LoginMethod::AutoLogin { session_key } => {
                info!("starting auto-login flow");
                self.auto_login_flow(&ctx, session_key).await?
            }
        };

        // 激活权限
        info!("activating promotion info");
        self.auth
            .get_promotion_info(&tgt)
            .await
            .map_err(|e| LauncherError::Auth(format!("get_promotion_info failed: {e}")))?;

        // 换取 ticket
        info!("exchanging tgt for ticket");
        let ticket = self
            .auth
            .sso_login(&ctx, &tgt)
            .await
            .map_err(|e| LauncherError::Auth(format!("sso_login failed: {e}")))?;

        info!(ticket = %mask_sensitive(&ticket), "login successful");

        Ok(LaunchToken {
            ticket,
            snda_id,
            auto_login_session_key: session_key,
        })
    }

    /// 启动游戏。
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

        info!(path = %game_path.display(), "launching game");

        let result = crate::game::launch_game(&config, self.custom_wine_path.as_deref())
            .await
            .map_err(LauncherError::from)?;

        info!(pid = %result.child.id(), "game process started");
        Ok(result)
    }

    /// 获取二维码数据（用于 UI 展示）。
    ///
    /// 返回 `(code_key, png_bytes)`。
    #[instrument(skip(self, ctx))]
    pub async fn request_qr_code(&self, ctx: &SdoContext) -> Result<(String, Vec<u8>), LauncherError> {
        let result = self
            .auth
            .qr_code_request(ctx)
            .await
            .map_err(|e| LauncherError::Auth(format!("qr_code_request failed: {e}")))?;
        info!(code_key = %result.code_key, bytes = result.image_data.len(), "qr code requested");
        Ok((result.code_key, result.image_data))
    }

    /// 轮询 QR 码扫码状态。
    ///
    /// - `Ok(Some(data))` → 扫码成功，返回登录数据
    /// - `Ok(None)` → 仍在等待
    /// - `Err(...)` → 发生错误
    #[instrument(skip(self, ctx))]
    pub async fn poll_qr_code(
        &self,
        ctx: &SdoContext,
        code_key: &str,
    ) -> Result<Option<SdoLoginData>, LauncherError> {
        match self.auth.qr_code_poll(ctx, code_key, 30).await {
            Ok(PollResult::Success(data)) => {
                info!("qr code scan confirmed");
                Ok(Some(data))
            }
            Ok(PollResult::Pending) => Ok(None),
            Err(e) => Err(LauncherError::Auth(format!("qr_code_poll failed: {e}"))),
        }
    }

    // ── internal helpers ─────────────────────────────────────────────

    async fn password_flow(
        &self,
        ctx: &SdoContext,
        account: &str,
        password: &str,
    ) -> Result<(String, String, Option<String>), LauncherError> {
        let result = self
            .auth
            .static_login(ctx, account, password)
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

        Ok((snda_id, tgt, result.data.auto_login_session_key))
    }

    async fn qr_code_flow(
        &self,
        ctx: &SdoContext,
        timeout: Option<Duration>,
    ) -> Result<(String, String, Option<String>), LauncherError> {
        let qr = self
            .auth
            .qr_code_request(ctx)
            .await
            .map_err(|e| LauncherError::Auth(format!("qr_code_request failed: {e}")))?;

        info!(code_key = %qr.code_key, "waiting for qr scan...");

        let start = std::time::Instant::now();
        let poll_interval = tokio::time::Duration::from_secs(3);

        loop {
            tokio::time::sleep(poll_interval).await;

            if let Some(t) = timeout {
                if start.elapsed() >= t {
                    return Err(LauncherError::Auth("QR code scan timed out".into()));
                }
            }

            match self.auth.qr_code_poll(ctx, &qr.code_key, 30).await {
                Ok(PollResult::Success(data)) => {
                    let snda_id = data
                        .snda_id
                        .ok_or_else(|| LauncherError::Auth("no snda_id in qr response".into()))?;
                    let tgt = data
                        .tgt
                        .ok_or_else(|| LauncherError::Auth("no tgt in qr response".into()))?;
                    return Ok((snda_id, tgt, data.auto_login_session_key));
                }
                Ok(PollResult::Pending) => {
                    debug!("qr code scan pending...");
                }
                Err(e) => return Err(LauncherError::Auth(format!("qr_code_poll failed: {e}"))),
            }
        }
    }

    async fn auto_login_flow(
        &self,
        ctx: &SdoContext,
        session_key: &str,
    ) -> Result<(String, String, Option<String>), LauncherError> {
        let result = self
            .auth
            .auto_login(ctx, session_key)
            .await
            .map_err(|e| LauncherError::Auth(format!("auto_login failed: {e}")))?;

        let snda_id = result
            .data
            .snda_id
            .ok_or_else(|| LauncherError::Auth("no snda_id in auto_login response".into()))?;
        let tgt = result
            .data
            .tgt
            .ok_or_else(|| LauncherError::Auth("no tgt in auto_login response".into()))?;

        Ok((snda_id, tgt, result.data.auto_login_session_key))
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
            auto_login_session_key: None,
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
