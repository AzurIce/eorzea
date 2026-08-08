use thiserror::Error;

#[derive(Error, Debug)]
pub enum AuthError {
    #[error("Network error: {0}")]
    Network(#[from] reqwest::Error),

    #[error("Login failed: {0}")]
    LoginFailed(String),

    #[error("OAuth login failed: {0}")]
    OauthLoginFailed(String),

    /// SDO 返回错误码，保留原始 `return_code` 供调用方判断。
    ///
    /// `remove_auto_login` 标记指示调用方是否应删除本地保存的自动登录 session key。
    #[error("SDO error {code}: {message}")]
    SdoError {
        code: i32,
        message: String,
        remove_auto_login: bool,
    },

    #[error("Steam account not linked")]
    SteamLinkNeeded,

    #[error("Steam wrong account")]
    SteamWrongAccount,

    #[error("Needs patch boot")]
    NeedsPatchBoot,

    #[error("Needs patch game")]
    NeedsPatchGame,

    #[error("No service")]
    NoService,

    #[error("Terms not accepted")]
    NoTerms,

    #[error("Auto login expired")]
    AutoLoginExpired,

    #[error("QR code not scanned")]
    QrNotScanned,

    #[error("Push message not confirmed")]
    PushMessageNotConfirmed,

    #[error("Captcha required")]
    CaptchaRequired,

    #[error("First login on device, use QR or slide login")]
    FirstLoginOnDevice,

    #[error("Invalid response: {0}")]
    InvalidResponse(String),

    #[error("JSON error: {0}")]
    Json(#[from] serde_json::Error),

    #[error(transparent)]
    Other(#[from] anyhow::Error),
}