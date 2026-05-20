//! SDO (盛趣) 国服认证客户端。
//!
//! 本模块实现了 FFXIV 中国服（盛趣运营）的全部登录方式，对应上游 C# 实现中的
//! `SdoLauncher.cs`。所有登录流程最终都需要通过 [`SdoAuth::sso_login`] 换取
//! 游戏会话 ID（`ticket`，即 `DEV.TestSID` 启动参数）。
//!
//! # 登录方式总览
//!
//! ## 1. 密码登录 (`LoginType::SdoStatic`)
//!
//! 最直接的方式，用户输入账号和密码。
//!
//! ```text
//! get_context ──▶ static_login ──▶ [得到 tgt] ──▶ get_promotion_info ──▶ sso_login ──▶ ticket (session_id)
//! ```
//!
//! - **入口**: [`SdoAuth::static_login`]
//! - **风控**: 首次在新设备登录或触发安全策略时，会返回 [`AuthError::CaptchaRequired`]
//!   或 [`AuthError::FirstLoginOnDevice`]，此时需改用扫码或推送方式。
//! - **注意**: `static_login` 返回的 `SdoLoginResult` 中包含 `tgt`（临时授权令牌），
//!   而非最终 `ticket`。必须继续调用 [`SdoAuth::sso_login`] 才能获取游戏启动用的 SID。
//!
//! ## 2. 滑动/推送登录 (`LoginType::SdoSlide`)
//!
//! 使用叨鱼 APP 的一键登录/滑动验证功能。
//!
//! ```text
//! get_context ──▶ cancelPushMessageLogin ──▶ slide_login_request ──▶ [得到 push_msg_session_key]
//!                                                                                │
//!                                                                                ▼ (轮询)
//!                                                             slide_login_poll (每 ~1s)
//!                                                                                │
//!                                                                     ┌───── Pending (继续轮询)
//!                                                                     ▼
//!                                                              Success (得到 tgt)
//!                                                                     │
//!                                                                     ▼
//!                                                    get_promotion_info ──▶ sso_login ──▶ ticket
//! ```
//!
//! - **入口**: [`SdoAuth::slide_login_request`] + [`SdoAuth::slide_login_poll`]
//! - **轮询**: 以约 1 秒间隔反复调用 `slide_login_poll`，直到返回 [`PollResult::Success`]。
//!   返回码 `-10516808` 表示用户尚未确认，需继续等待。
//! - **超时**: 上游 C# 在 `sendPushMessage` 时创建 30 秒超时 `CancellationTokenSource`。
//!   本模块不管理超时，由调用方控制轮询生命周期。
//!
//! ## 3. 扫码登录 (`LoginType::SdoQrCode`)
//!
//! 使用叨鱼 APP 扫描二维码登录。
//!
//! ```text
//! get_context ──▶ qr_code_request ──▶ [得到二维码图片 + code_key]
//!                                              │
//!                                              ▼ (展示给用户)
//!                                        qr_code_poll (每 ~2-3s)
//!                                              │
//!                                   ┌───── Pending (继续轮询)
//!                                   ▼
//!                            Success (得到 tgt + snda_id)
//!                                   │
//!                                   ▼
//!              getAccountGroup ──▶ accountGroupLogin ──▶ get_promotion_info ──▶ sso_login ──▶ ticket
//! ```
//!
//! - **入口**: [`SdoAuth::qr_code_request`] + [`SdoAuth::qr_code_poll`]
//! - **二维码**: `qr_code_request` 返回 PNG 格式的二维码图片字节，可直接展示。
//!   `code_key` 从响应的 `Set-Cookie: CODEKEY=...` 中提取。
//! - **轮询**: 以约 2-3 秒间隔调用 `qr_code_poll`。返回码 `-10515805` 表示尚未扫描。
//! - **AccountGroup**: 扫码成功后，上游 C# 会调用 `getAccountGroup` 校验账号，然后
//!   调用 `accountGroupLogin` 刷新 `tgt` 并获取 `auto_login_session_key`。
//!   **本模块目前缺失这两个步骤**（见 [`crate::TODO`] 或 `TODO.md`）。
//!
//! ## 4. 自动登录 (`LoginType::AutoLoginSession`)
//!
//! 使用之前保存的 `auto_login_session_key` 快速登录，无需再次输入密码。
//!
//! ```text
//! get_context ──▶ auto_login ──▶ [刷新 tgt + 新的 session_key]
//!                                      │
//!                                      ▼
//!                              fast_login ──▶ [刷新 snda_id + tgt]
//!                                      │
//!                                      ▼
//!                         get_promotion_info ──▶ sso_login ──▶ ticket
//! ```
//!
//! - **入口**: [`SdoAuth::auto_login`]
//! - **session key 过期**: 若返回码为 `-10515005`，表示自动登录已失效，需重新用密码/扫码/推送方式登录。
//! - **fastLogin**: 上游 C# 在 `auto_login` 之后还会调用 `fastLogin.json` 进一步刷新凭证。
//!   **本模块目前缺失此步骤**。
//!
//! ## 5. WeGame Token 登录 (`LoginType::WeGameToken`)
//!
//! 使用 WeGame 抓包获取的 token 进行第三方登录。
//!
//! ```text
//! get_context ──▶ thirdPartyLogin (companyid=310) ──▶ [得到 tgt] ──▶ get_promotion_info ──▶ sso_login ──▶ ticket
//! ```
//!
//! - **状态**: `LoginType::WeGameToken` 已在枚举中定义，**但 `SdoAuth` 中尚无对应实现方法**。
//!
//! ## 6. WeGame SID 登录 (`LoginType::WeGameSid`)
//!
//! 直接使用 WeGame 提供的 SID 构造会话，完全跳过 SDO 认证流程。
//! 此方法不需要调用 `SdoAuth` 中的任何方法，直接在应用层构造 [`OauthLoginResult`] 即可。
//!
//! # 设备标识 (`device_id` / `mac_id`)
//!
//! SDO 服务端要求每次请求附带设备标识，用于风控和防多开：
//!
//! - **`device_id`**: `{MD5(MAC地址)}:{MD5(CPU_ID)}:{MD5(硬盘序列号)}`，每部分为大写十六进制。
//! - **`mac_id`**: 第一个非空网卡 MAC 地址的 MD5（大写十六进制）。
//!
//! 设备标识由 [`crate::sdo_device`] 模块自动采集（MAC/CPU/硬盘序列号的 MD5 哈希），
//! 创建 `SdoAuth` 时自动初始化，无需调用方手动提供。
//!
//! # 公共后续步骤
//!
//! 所有成功获取 `tgt` 的登录方式，都需要继续执行：
//!
//! 1. [`SdoAuth::get_promotion_info`] — 激活 TGT 的登录权限（必须调用，无需处理返回值）
//! 2. [`SdoAuth::sso_login`] — 用 TGT 换取最终 `ticket`（即游戏 `session_id`）
//!
//! # 已知缺失（与上游 C# 的差异）
//!
//! - **WeGame 登录**: `thirdPartyLogin`、`LoginBySid` 未实现。
//! - **AccountGroup**: `getAccountGroup`、`accountGroupLogin` 未实现。
//! - **fastLogin**: 自动登录流程中缺少 `fastLogin.json` 步骤。
//! - **DcTraveler**: 跨服传送功能（`GetDcTravelSessionId`）未实现。
//! - **域名故障转移**: 目前需手动通过 `with_fallback_url()` 设置备用域名，不自动重试。
//! - **日志脱敏**: 缺少 `MaskMiddleConverter` 对敏感字段的日志脱敏。
//!
//! 完整差异列表见仓库根目录 `TODO.md`。

use crate::error::AuthError;
use crate::model::*;
use reqwest::Client;
use serde::Deserialize;
use tracing::{debug, error, info, instrument, warn};

/// SDO (盛趣) 认证 API 基地址。
const SDO_BASE_URL: &str = "https://cas.sdo.com/authen";
/// SDO 备用认证地址（主地址不可达时使用）。
#[allow(dead_code)]
const SDO_FALLBACK_URL: &str = "https://n1.cas.sdo.com/authen";
/// 国服服务器列表地址。
const SERVER_LIST_URL: &str = "https://ff.dorado.sdo.com/ff/area/serverlist_new.js";
/// SDO 应用 ID（FFXIV 国服固定为 100001900）。
const SDO_APP_ID: &str = "100001900";

/// SDO 认证客户端，用于中国服（盛趣）登录。
///
/// 支持以下登录方式：
/// - 密码登录 (`static_login`)
/// - 滑动验证/推送登录 (`slide_login_request` + `slide_login_poll`)
/// - 扫码登录 (`qr_code_request` + `qr_code_poll`)
/// - 自动登录 (`auto_login`)，使用之前保存的 session key
/// - SSO 登录 (`sso_login`)，用 TGT 换取最终 ticket
///
/// # 设备标识
///
/// 创建时自动采集本机设备指纹（MAC/CPU/硬盘序列号的 MD5 哈希），
/// 无需调用方手动提供。详见 [`crate::sdo_device`] 模块文档。
pub struct SdoAuth {
    client: Client,
    base_url: String,
    device_id: String,
    mac_id: String,
}

/// SDO 登录上下文，由 [`SdoAuth::get_context`] 返回。
///
/// 包含一次登录流程所需的会话标识：
/// - `guid`: SDO 分配的会话 GUID，后续所有请求需携带
/// - `dynamic_key`: 动态密钥（当前流程未使用，预留给加密密码等场景）
/// - `device_id` / `mac_id`: 设备标识，随请求发送至 SDO 服务端
pub struct SdoContext {
    pub guid: String,
    pub dynamic_key: Option<String>,
    pub device_id: String,
    pub mac_id: String,
}

impl SdoAuth {
    /// 创建 SDO 认证客户端，自动采集设备指纹。
    ///
    /// 设备指纹通过 [`crate::sdo_device`] 模块采集，
    /// 包括 `device_id`（`MAC:CPU:Disk` 三段 MD5）和 `mac_id`（MAC 的 MD5）。
    ///
    /// 默认连接主域名 `cas.sdo.com`，若不可达可在创建后通过
    /// [`SdoAuth::with_fallback_url`] 设置备用域名。
    pub fn new() -> Result<Self, AuthError> {
        info!("Creating SdoAuth client");
        Ok(Self {
            client: Client::builder().cookie_store(true).build()?,
            base_url: SDO_BASE_URL.to_string(),
            device_id: crate::sdo_device::get_device_id(),
            mac_id: crate::sdo_device::get_mac_address_hash(),
        })
    }

    /// 设置备用域名（主域名不可达时使用）。
    ///
    /// 上游 C# 在 `GetJsonAsSdoClient` 中对每个请求先尝试主域名，
    /// 失败后自动回退到备用域名 `n1.cas.sdo.com`。
    pub fn with_fallback_url(mut self, url: &str) -> Self {
        info!(fallback_url = url, "Setting fallback URL");
        self.base_url = url.to_string();
        self
    }

    /// 获取登录会话上下文（调用 `getGuid.json`）。
    ///
    /// 返回的 `SdoContext` 需要传入后续所有登录方法。
    ///
    /// # 设备标识参数
    ///
    /// - `device_id`: 格式为 `{MD5(MAC)}:{MD5(CPU_ID)}:{MD5(硬盘序列号)}`，
    ///   每部分大写 hex。原版读取真实硬件，测试可用固定值如
    ///   `"AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA:BBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBB:CCCCCCCCCCCCCCCCCCCCCCCCCCCCCCCC"`。
    /// - `mac_id`: MAC 地址的 MD5（大写 hex），如 `"AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA"`。
    #[instrument(skip(self), fields(base_url = %self.base_url), err)]
    pub async fn get_context(&self) -> Result<SdoContext, AuthError> {
        let url = format!(
            "{}/getGuid.json?generateDynamicKey=1&{}",
            self.base_url,
            self.common_query(),
        );
        debug!(url = %url, "Requesting login context (getGuid)");

        let resp: SdoResponse<SdoGuidData> = self.get_json_with_cookies(&url).await?;
        debug!(return_code = resp.return_code, "getGuid response");
        if resp.return_code != 0 {
            error!(return_code = resp.return_code, "getGuid failed");
            return Err(AuthError::SdoError {
                code: resp.return_code,
                message: "getGuid failed".to_string(),
                remove_auto_login: false,
            });
        }

        info!("Login context obtained successfully");
        Ok(SdoContext {
            guid: resp.data.guid,
            dynamic_key: resp.data.dynamic_key,
            device_id: self.device_id.clone(),
            mac_id: self.mac_id.clone(),
        })
    }

    /// 密码登录（调用 `staticLogin.json`）。
    ///
    /// 可能返回的错误：
    /// - [`AuthError::CaptchaRequired`]: 触发风控，需要验证码
    /// - [`AuthError::FirstLoginOnDevice`]: 首次在该设备登录，需用扫码或推送方式
    #[instrument(skip(self, ctx, password), fields(account = %account), err)]
    pub async fn static_login(
        &self,
        ctx: &SdoContext,
        account: &str,
        password: &str,
    ) -> Result<SdoLoginResult, AuthError> {
        let masked_url = format!(
            "{}/staticLogin.json?checkCodeFlag=1&encryptFlag=0&inputUserId={}&password=***&mac={}&guid={}&inputUserType=0&accountDomain=1&autoLoginFlag=0&autoLoginKeepTime=0&supportPic=2&{}",
            self.base_url,
            urlencoding::encode(account),
            urlencoding::encode(&ctx.mac_id),
            urlencoding::encode(&ctx.guid),
            self.common_query(),
        );
        debug!(url = %masked_url, "Password login request (staticLogin)");

        let url = format!(
            "{}/staticLogin.json?checkCodeFlag=1&encryptFlag=0&inputUserId={}&password={}&mac={}&guid={}&inputUserType=0&accountDomain=1&autoLoginFlag=0&autoLoginKeepTime=0&supportPic=2&{}",
            self.base_url,
            urlencoding::encode(account),
            urlencoding::encode(password),
            urlencoding::encode(&ctx.mac_id),
            urlencoding::encode(&ctx.guid),
            self.common_query(),
        );

        let result: SdoLoginResult = self.get_json_raw(&url).await?;
        debug!(return_code = result.return_code, "staticLogin response");
        self.check_sdo_error(&result)?;

        info!(
            snda_id = ?result.data.snda_id,
            auto_login_session_key = ?result.data.auto_login_session_key,
            auto_login_max_age_h = result.data.auto_login_max_age.map(|s| s as f32 / 3600.0),
            "Password login successful"
        );
        Ok(result)
    }

    /// 发送推送登录请求（调用 `sendPushMessage.json`），用于滑动验证/一键登录。
    ///
    /// 先调用此方法发起推送，再轮询 [`SdoAuth::slide_login_poll`] 等待确认。
    /// 返回的 `push_msg_session_key` 用于后续轮询。
    #[instrument(skip(self, ctx), fields(account = %account), err)]
    pub async fn slide_login_request(
        &self,
        ctx: &SdoContext,
        account: &str,
    ) -> Result<SdoLoginData, AuthError> {
        let cancel_url = format!(
            "{}/cancelPushMessageLogin.json?pushMsgSessionKey=&guid={}&{}",
            self.base_url,
            urlencoding::encode(&ctx.guid),
            self.common_query(),
        );
        debug!(url = %cancel_url, "Cancelling previous push login");
        let _ = self
            .get_json_raw::<SdoResponse<serde_json::Value>>(&cancel_url)
            .await;

        let url = format!(
            "{}/sendPushMessage.json?inputUserId={}&{}",
            self.base_url,
            urlencoding::encode(account),
            self.common_query(),
        );
        debug!(url = %url, "Requesting push login (sendPushMessage)");

        let resp: SdoResponse<SdoLoginData> = self.get_json_with_cookies(&url).await?;
        debug!(return_code = resp.return_code, "sendPushMessage response");
        if resp.return_code != 0 {
            error!(return_code = resp.return_code, "sendPushMessage failed");
            return Err(AuthError::SdoError {
                code: resp.return_code,
                message: "sendPushMessage failed".to_string(),
                remove_auto_login: false,
            });
        }

        info!("Push login request sent successfully");
        Ok(resp.data)
    }

    /// 轮询推送登录状态（调用 `pushMessageLogin.json`）。
    ///
    /// 应以约 1 秒间隔反复调用，直到返回 [`PollResult::Success`]。
    /// - [`PollResult::Success`]: 用户已确认，登录成功
    /// - [`PollResult::Pending`]: 用户尚未确认，继续等待
    #[instrument(skip(self, ctx, push_msg_session_key), err)]
    pub async fn slide_login_poll(
        &self,
        ctx: &SdoContext,
        push_msg_session_key: &str,
    ) -> Result<PollResult, AuthError> {
        let url = format!(
            "{}/pushMessageLogin.json?pushMsgSessionKey=***&guid={}&autoLoginFlag=1&autoLoginKeepTime=30&{}",
            self.base_url,
            urlencoding::encode(&ctx.guid),
            self.common_query(),
        );
        debug!(url = %url, "Polling push login status");

        let full_url = format!(
            "{}/pushMessageLogin.json?pushMsgSessionKey={}&guid={}&autoLoginFlag=1&autoLoginKeepTime=30&{}",
            self.base_url,
            urlencoding::encode(push_msg_session_key),
            urlencoding::encode(&ctx.guid),
            self.common_query(),
        );

        let result: SdoLoginResult = self.get_json_raw(&full_url).await?;
        debug!(return_code = result.return_code, "pushMessageLogin response");
        match result.return_code {
            0 => {
                if result.data.next_action == Some(0) {
                    info!(
                        snda_id = ?result.data.snda_id,
                        auto_login_session_key = ?result.data.auto_login_session_key,
                        auto_login_max_age_h = result.data.auto_login_max_age.map(|s| s as f32 / 3600.0),
                        "Push login confirmed by user"
                    );
                    Ok(PollResult::Success(result.data))
                } else {
                    debug!("Push login pending");
                    Ok(PollResult::Pending)
                }
            }
            -10516808 => {
                debug!("Push login pending (user not yet confirmed)");
                Ok(PollResult::Pending)
            }
            _ => {
                error!(
                    return_code = result.return_code,
                    fail_reason = ?result.data.fail_reason,
                    "Push login failed"
                );
                Err(AuthError::SdoError {
                    code: result.return_code,
                    message: format!("pushMessageLogin: {:?}", result.data.fail_reason),
                    remove_auto_login: false,
                })
            }
        }
    }

    /// 请求扫码登录二维码（调用 `getCodeKey.json`）。
    ///
    /// 返回二维码图片数据和对应的 `code_key`，后者用于轮询扫码状态。
    /// 图片为 PNG 格式，可直接保存到文件展示给用户。
    #[instrument(skip(self, _ctx), err)]
    pub async fn qr_code_request(&self, _ctx: &SdoContext) -> Result<QrCodeResult, AuthError> {
        let url = format!(
            "{}/getCodeKey.json?maxsize=89&{}",
            self.base_url,
            self.common_query(),
        );
        debug!(url = %url, "Requesting QR code (getCodeKey)");

        let response = self
            .client
            .get(&url)
            .header("User-Agent", SDO_USER_AGENT)
            .header("Cache-Control", "no-cache")
            .send()
            .await?;

        let code_key = response
            .headers()
            .get("Set-Cookie")
            .and_then(|v| v.to_str().ok())
            .and_then(|s| s.split("CODEKEY=").nth(1))
            .and_then(|s| s.split(';').next())
            .map(|s| s.to_string())
            .ok_or_else(|| {
                error!("No CODEKEY cookie in QR response");
                AuthError::InvalidResponse("No CODEKEY cookie in QR response".into())
            })?;

        let image_data = response.bytes().await?;
        info!(code_key = %code_key, "QR code obtained successfully");

        Ok(QrCodeResult {
            code_key,
            image_data: image_data.to_vec(),
        })
    }

    /// 轮询扫码登录状态（调用 `codeKeyLogin.json`）。
    ///
    /// 应以约 2-3 秒间隔反复调用，直到返回 [`PollResult::Success`]。
    /// - [`PollResult::Success`]: 扫码确认成功
    /// - [`PollResult::Pending`]: 尚未扫描，继续等待
    #[instrument(skip(self, ctx), fields(auto_login_days, code_key = %code_key), err)]
    pub async fn qr_code_poll(
        &self,
        ctx: &SdoContext,
        code_key: &str,
        auto_login_days: i32,
    ) -> Result<PollResult, AuthError> {
        let url = format!(
            "{}/codeKeyLogin.json?codeKey={}&guid={}&autoLoginFlag=1&autoLoginKeepTime={}&{}",
            self.base_url,
            urlencoding::encode(code_key),
            urlencoding::encode(&ctx.guid),
            auto_login_days,
            self.common_query(),
        );
        debug!(url = %url, "Polling QR code login status");

        let result: SdoLoginResult = self.get_json_raw(&url).await?;
        debug!(return_code = result.return_code, "codeKeyLogin response");
        match result.return_code {
            0 => {
                info!(
                    snda_id = ?result.data.snda_id,
                    auto_login_session_key = ?result.data.auto_login_session_key,
                    auto_login_max_age_h = result.data.auto_login_max_age.map(|s| s as f32 / 3600.0),
                    "QR code login successful"
                );
                Ok(PollResult::Success(result.data))
            }
            -10515805 => {
                debug!("QR code not yet scanned");
                Ok(PollResult::Pending)
            }
            _ => {
                error!(
                    return_code = result.return_code,
                    fail_reason = ?result.data.fail_reason,
                    "QR code login failed"
                );
                Err(AuthError::SdoError {
                    code: result.return_code,
                    message: format!("codeKeyLogin: {:?}", result.data.fail_reason),
                    remove_auto_login: false,
                })
            }
        }
    }

    /// 自动登录（调用 `autoLogin.json`），使用之前登录返回的 `auto_login_session_key`。
    ///
    /// 快速登录方式，无需再次输入密码或扫码。
    /// 如果 session key 已过期则返回 [`AuthError::AutoLoginExpired`]，需重新登录。
    #[instrument(skip(self, ctx, session_key), err)]
    pub async fn auto_login(
        &self,
        ctx: &SdoContext,
        session_key: &str,
    ) -> Result<SdoLoginResult, AuthError> {
        let masked_url = format!(
            "{}/autoLogin.json?autoLoginSessionKey=***&guid={}&{}",
            self.base_url,
            urlencoding::encode(&ctx.guid),
            self.common_query(),
        );
        debug!(url = %masked_url, "Auto login request");

        let url = format!(
            "{}/autoLogin.json?autoLoginSessionKey={}&guid={}&{}",
            self.base_url,
            urlencoding::encode(session_key),
            urlencoding::encode(&ctx.guid),
            self.common_query(),
        );

        let result: SdoLoginResult = self.get_json_raw(&url).await?;
        debug!(return_code = result.return_code, "autoLogin response");
        if result.return_code == -10515005 {
            warn!("Auto login session expired");
            return Err(AuthError::AutoLoginExpired);
        }

        info!(
            snda_id = ?result.data.snda_id,
            new_auto_login_session_key = ?result.data.auto_login_session_key,
            auto_login_max_age_h = result.data.auto_login_max_age.map(|s| s as f32 / 3600.0),
            "Auto login successful"
        );
        Ok(result)
    }

    /// SSO 登录（调用 `ssoLogin.json`），用 TGT 换取最终 ticket。
    ///
    /// 完整登录流程的最后一步：先通过某种方式获取 `tgt`，再调用此方法换取 `ticket`，
    /// `ticket` 即为游戏的 session ID（`DEV.TestSID` 参数）。
    #[instrument(skip(self, ctx, tgt), err)]
    pub async fn sso_login(&self, ctx: &SdoContext, tgt: &str) -> Result<String, AuthError> {
        let masked_url = format!(
            "{}/ssoLogin.json?tgt=***&guid={}&{}",
            self.base_url,
            urlencoding::encode(&ctx.guid),
            self.common_query(),
        );
        debug!(url = %masked_url, "SSO login request (ssoLogin)");

        let url = format!(
            "{}/ssoLogin.json?tgt={}&guid={}&{}",
            self.base_url,
            urlencoding::encode(tgt),
            urlencoding::encode(&ctx.guid),
            self.common_query(),
        );

        let resp: SdoResponse<serde_json::Value> = self.get_json_with_cookies(&url).await?;
        debug!(return_code = resp.return_code, "ssoLogin response");
        if resp.return_code != 0 {
            error!(return_code = resp.return_code, "ssoLogin failed");
            return Err(AuthError::SdoError {
                code: resp.return_code,
                message: "ssoLogin failed".to_string(),
                remove_auto_login: false,
            });
        }

        let ticket = resp
            .data
            .get("ticket")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string())
            .ok_or_else(|| {
                error!("No ticket in ssoLogin response");
                AuthError::InvalidResponse("No ticket in ssoLogin response".into())
            })?;

        info!(ticket = %ticket, "SSO login successful, ticket obtained");
        Ok(ticket)
    }

    /// 激活 ticket 权限（调用 `getPromotionInfo.json`）。
    ///
    /// 在 SSO 登录之前调用，用于激活 TGT 对应的登录权限。
    /// 此步骤为 SDO 协议要求，调用即可，无需处理返回值。
    #[instrument(skip(self, tgt), err)]
    pub async fn get_promotion_info(&self, tgt: &str) -> Result<(), AuthError> {
        let masked_url = format!(
            "{}/getPromotionInfo.json?tgt=***&serviceUrl=http%3A%2F%2Fwww.sdo.com",
            self.base_url,
        );
        debug!(url = %masked_url, "Getting promotion info");

        let url = format!(
            "{}/getPromotionInfo.json?tgt={}&serviceUrl=http%3A%2F%2Fwww.sdo.com",
            self.base_url,
            urlencoding::encode(tgt),
        );

        let _ = self
            .client
            .get(&url)
            .header("User-Agent", SDO_USER_AGENT)
            .header("Cache-Control", "no-cache")
            .header("Host", "cas.sdo.com")
            .header("Cookie", format!("CASTGC={}; CAS_LOGIN_STATE=1", tgt))
            .send()
            .await?;
        info!("Promotion info activated");
        Ok(())
    }

    /// 获取国服服务器列表（从 `ff.dorado.sdo.com` 获取）。
    ///
    /// 返回所有大区的信息，包括区名、lobby 服务器、GM 服务器、
    /// 补丁服务器等地址，用于后续游戏连接参数。
    #[instrument(err)]
    pub async fn fetch_server_list() -> Result<Vec<SdoArea>, AuthError> {
        let client = Client::new();
        debug!(url = %SERVER_LIST_URL, "Fetching server list");
        let response = client
            .get(SERVER_LIST_URL)
            .header("Accept", "*/*")
            .header("Host", "ff.dorado.sdo.com")
            .send()
            .await?;

        let body = response.text().await?;
        let json_str = body
            .trim()
            .strip_prefix("var servers=")
            .and_then(|s| s.strip_suffix(';'))
            .ok_or_else(|| {
                error!("Invalid server list format");
                AuthError::InvalidResponse("Invalid server list format".into())
            })?;

        let areas: Vec<SdoArea> = serde_json::from_str(json_str)?;
        info!(count = areas.len(), "Server list fetched successfully");
        Ok(areas)
    }

    #[instrument(skip(self))]
    fn common_query(&self) -> String {
        format!(
            "authenSource=1&appId={}&areaId=1&appIdSite={}&locale=zh_CN&productId=4&frameType=1&endpointOS=1&version=21&customSecurityLevel=2&deviceId={}&thirdLoginExtern=0&macId={}&productVersion=1.9.7.10&tag=0",
            SDO_APP_ID, SDO_APP_ID,
            urlencoding::encode(&self.device_id),
            urlencoding::encode(&self.mac_id),
        )
    }

    /// 构建 Cookie 头，包含自动生成的 `CASCID` 和 `SECURE_CASCID`。
    ///
    /// 上游 C# 在 `GetSdoHttpRequestMessage` 中：若本地无 `CASCID` Cookie，
    /// 则自动注入 `CID{MD5(mac_id)}` 作为 `CASCID` 和 `SECURE_CASCID`。
    fn build_cookie_header(&self) -> String {
        let cid = format!("CID{}", self.mac_id);
        format!("CASCID={}; SECURE_CASCID={}; _rsid=\"\"", cid, cid)
    }

    /// 发送带 Cookie 和 Host 头的 GET 请求，解析为 `SdoResponse<T>`。
    ///
    /// 所有 SDO API 调用都应使用此方法，确保 Cookie 和 Host 头一致。
    #[instrument(skip(self, url), fields(url = %url))]
    async fn get_json_with_cookies<T: serde::de::DeserializeOwned>(
        &self,
        url: &str,
    ) -> Result<SdoResponse<T>, AuthError> {
        debug!("Sending GET request with cookies");
        let resp = self
            .client
            .get(url)
            .header("User-Agent", SDO_USER_AGENT)
            .header("Cache-Control", "no-cache")
            .header("Host", "cas.sdo.com")
            .header("Cookie", self.build_cookie_header())
            .send()
            .await?;

        resp.json::<SdoResponse<T>>().await.map_err(Into::into)
    }

    /// 发送 GET 请求（不带 SDO Cookie），解析为 `SdoResponse<T>`。
    ///
    /// 仅用于不涉及登录会话的请求（如服务器列表）。
    #[allow(dead_code)]
    #[instrument(skip(self, url), fields(url = %url))]
    async fn get_json<T: serde::de::DeserializeOwned>(
        &self,
        url: &str,
    ) -> Result<SdoResponse<T>, AuthError> {
        debug!("Sending GET request without cookies");
        let resp = self
            .client
            .get(url)
            .header("User-Agent", SDO_USER_AGENT)
            .header("Cache-Control", "no-cache")
            .send()
            .await?;

        resp.json::<SdoResponse<T>>().await.map_err(Into::into)
    }

    /// 发送带 Cookie 和 Host 头的 GET 请求，解析为原始 `T`（不包装 `SdoResponse`）。
    #[instrument(skip(self, url), fields(url = %url))]
    async fn get_json_raw<T: serde::de::DeserializeOwned>(
        &self,
        url: &str,
    ) -> Result<T, AuthError> {
        debug!("Sending GET request (raw response)");
        let resp = self
            .client
            .get(url)
            .header("User-Agent", SDO_USER_AGENT)
            .header("Cache-Control", "no-cache")
            .header("Host", "cas.sdo.com")
            .header("Cookie", self.build_cookie_header())
            .send()
            .await?;

        resp.json::<T>().await.map_err(Into::into)
    }

    #[instrument(skip(self, result), fields(return_code = result.return_code))]
    fn check_sdo_error(&self, result: &SdoLoginResult) -> Result<(), AuthError> {
        // C#: if (result.ReturnCode != 0 || result.ErrorType != 0) throw
        if result.return_code != 0 || result.error_type.unwrap_or(0) != 0 {
            match result.return_code {
                -10386188 => {
                    warn!("Captcha required");
                    return Err(AuthError::CaptchaRequired);
                }
                -10242296 => {
                    warn!("First login on device");
                    return Err(AuthError::FirstLoginOnDevice);
                }
                -10515005 => {
                    warn!("Auto login expired");
                    return Err(AuthError::AutoLoginExpired);
                }
                code => {
                    let remove_auto = matches!(result.data.auto_login_session_key.as_deref(), Some("0"));
                    error!(
                        code = code,
                        fail_reason = ?result.data.fail_reason,
                        remove_auto_login = remove_auto,
                        "SDO login error"
                    );
                    return Err(AuthError::SdoError {
                        code,
                        message: result.data.fail_reason.as_deref().unwrap_or("unknown").to_string(),
                        remove_auto_login: remove_auto,
                    });
                }
            }
        }

        // return_code == 0 && error_type == 0, but Tgt is empty → need captcha (risk control)
        if result.data.tgt.is_none() || result.data.tgt.as_ref().unwrap().is_empty() {
            warn!("TGT is empty, captcha required (risk control)");
            return Err(AuthError::CaptchaRequired);
        }

        debug!("SDO login result valid");
        Ok(())
    }
}

const SDO_USER_AGENT: &str = "Mozilla/4.0 (compatible; MSIE 8.0; Windows NT 5.1; Trident/4.0; .NET CLR 2.0.50727; .NET4.0C; .NET4.0E)";

#[derive(Debug, Deserialize)]
struct SdoResponse<T> {
    #[serde(rename = "return_code")]
    return_code: i32,
    data: T,
}

#[derive(Debug, Deserialize)]
struct SdoGuidData {
    guid: String,
    #[serde(default)]
    dynamic_key: Option<String>,
}

pub enum PollResult {
    Success(SdoLoginData),
    Pending,
}

pub struct QrCodeResult {
    pub code_key: String,
    pub image_data: Vec<u8>,
}
