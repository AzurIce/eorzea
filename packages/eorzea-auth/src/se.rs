use crate::error::AuthError;
use crate::model::*;
use regex::Regex;
use reqwest::Client;

/// Square Enix 国际服 OAuth 认证基地址。
const OAUTH_BASE_URL: &str = "https://ffxiv-login.square-enix.com/oauth/ffxivarr/login";

/// Square Enix 国际服认证客户端。
///
/// 登录流程为两步：
/// 1. **OAuth Top** (`GET .../top`) — 获取 `_STORED_` 令牌
/// 2. **OAuth Login** (`POST .../login.send`) — 提交用户名、密码、OTP，获取 session ID
///
/// 登录成功后，还需调用 [`SeAuth::register_session`] 向游戏服务器注册会话，
/// 获取 `X-Patch-Unique-Id`（补丁 UID）并检查是否需要更新。
///
/// # Computer ID 说明
///
/// SE OAuth 要求在 `User-Agent` 中附带 `computer_id`，格式为 8 位小写十六进制。
/// 原版 C# 计算方式：
/// ```text
/// input = MachineName + UserName + OSVersion + ProcessorCount
/// sha1_bytes = SHA1(UnicodeEncode(input))
/// result[0] = -(sha1_bytes[0..4] 之和) as u8   // 校验和
/// result[1..5] = sha1_bytes[0..4]
/// ```
/// 可使用 [`crate::crypto::make_computer_id`] 生成。
pub struct SeAuth {
    client: Client,
}

impl SeAuth {
    /// 创建 SE 认证客户端（使用 cookie store 保持会话）。
    pub fn new() -> Result<Self, AuthError> {
        Ok(Self {
            client: Client::builder().cookie_store(true).build()?,
        })
    }

    /// 完整的国际服 OAuth 登录流程。
    ///
    /// 依次执行 OAuth Top → OAuth Login，返回 [`LoginResult`]。
    ///
    /// # 参数
    /// - `username`: SQEX 账号
    /// - `password`: 明文密码
    /// - `otp`: 一次性密码（启用了 OTP 的账号），`None` 表示不使用
    /// - `region`: 区域代码（1=JP, 2=NA, 3=EU）
    /// - `is_free_trial`: 是否为免费试用版
    /// - `computer_id`: 设备标识，格式见 [`SeAuth`] 文档
    /// - `accept_language`: `Accept-Language` 请求头值，如 `"en"`
    pub async fn login(
        &self,
        username: &str,
        password: &str,
        otp: Option<&str>,
        region: i32,
        is_free_trial: bool,
        computer_id: &str,
        accept_language: &str,
    ) -> Result<LoginResult, AuthError> {
        let stored_token = self
            .get_oauth_top(region, is_free_trial, computer_id, accept_language)
            .await?;
        let oauth_result = self
            .oauth_login(
                username,
                password,
                otp,
                &stored_token,
                computer_id,
                accept_language,
            )
            .await?;
        Ok(LoginResult {
            state: LoginState::Ok,
            oauth_login: Some(oauth_result),
            unique_id: None,
            pending_patches: vec![],
            area: None,
            areas: vec![],
            dc_travel_port: None,
        })
    }

    async fn get_oauth_top(
        &self,
        region: i32,
        is_free_trial: bool,
        computer_id: &str,
        accept_language: &str,
    ) -> Result<String, AuthError> {
        let url = format!(
            "{}/top?lng=en&rgn={}&isft={}&cssmode=1&isnew=1&launchver=3",
            OAUTH_BASE_URL,
            region,
            if is_free_trial { 1 } else { 0 },
        );

        let response = self
            .client
            .get(&url)
            .header("User-Agent", format!("SQEXAuthor/2.0.0(Windows 6.2; ja-jp; {})", computer_id))
            .header("Accept", "image/gif, image/jpeg, image/pjpeg, application/x-ms-application, application/xaml+xml, application/x-ms-xbap, */*")
            .header("Accept-Language", accept_language)
            .header("Accept-Encoding", "gzip, deflate")
            .header("Connection", "Keep-Alive")
            .header("Cookie", "_rsid=\"\"")
            .send()
            .await?;

        let body = response.text().await?;

        if body.contains("window.external.user(\"restartup\")") {
            return Err(AuthError::SteamLinkNeeded);
        }

        let re = Regex::new(r#"<input\s+name="_STORED_"\s+value="([^"]*)""#).unwrap();
        let stored = re
            .captures(&body)
            .and_then(|c| c.get(1))
            .map(|m| m.as_str().to_string())
            .ok_or_else(|| AuthError::InvalidResponse("Could not find _STORED_ token".into()))?;

        Ok(stored)
    }

    async fn oauth_login(
        &self,
        username: &str,
        password: &str,
        otp: Option<&str>,
        stored_token: &str,
        computer_id: &str,
        accept_language: &str,
    ) -> Result<OauthLoginResult, AuthError> {
        let params = [
            ("_STORED_", stored_token),
            ("sqexid", username),
            ("password", password),
            ("otppw", otp.unwrap_or("")),
        ];

        let response = self
            .client
            .post(format!("{}/login.send", OAUTH_BASE_URL))
            .form(&params)
            .header("User-Agent", format!("SQEXAuthor/2.0.0(Windows 6.2; ja-jp; {})", computer_id))
            .header("Accept", "image/gif, image/jpeg, image/pjpeg, application/x-ms-application, application/xaml+xml, application/x-ms-xbap, */*")
            .header("Accept-Language", accept_language)
            .header("Accept-Encoding", "gzip, deflate")
            .header("Cache-Control", "no-cache")
            .header("Connection", "Keep-Alive")
            .header("Host", "ffxiv-login.square-enix.com")
            .header("Cookie", "_rsid=\"\"")
            .send()
            .await?;

        let body = response.text().await?;

        let re = Regex::new(r#"window\.external\.user\("login=auth,ok,([^"]+)"\)"#).unwrap();
        let caps = re
            .captures(&body)
            .and_then(|c| c.get(1))
            .map(|m| m.as_str().to_string())
            .ok_or_else(|| AuthError::OauthLoginFailed("Could not parse login response".into()))?;

        let parts: Vec<&str> = caps.split(',').collect();
        if parts.len() < 14 {
            return Err(AuthError::OauthLoginFailed(
                "Invalid login response format".into(),
            ));
        }

        let session_id = parts.get(1).map(|s| *s).unwrap_or("").to_string();
        let terms_accepted = *parts.get(3).unwrap_or(&"0") == "1";
        let region: i32 = parts.get(5).unwrap_or(&"0").parse().unwrap_or(0);
        let playable = *parts.get(9).unwrap_or(&"0") == "1";
        let max_expansion: i32 = parts.get(13).unwrap_or(&"0").parse().unwrap_or(0);

        Ok(OauthLoginResult {
            session_id,
            input_user_id: username.to_string(),
            snda_id: String::new(),
            region,
            terms_accepted,
            playable,
            max_expansion,
            login_type: LoginType::SquareEnix,
        })
    }

    /// 注册游戏会话并检查版本（向游戏服务器 POST 版本报告）。
    ///
    /// 登录成功后必须调用此步骤，游戏服务器会：
    /// - 返回 `200` + `X-Patch-Unique-Id` 头：会话正常，无需补丁
    /// - 返回 `409`：启动器需要更新 (`NeedsPatchBoot`)
    /// - 返回 `410`：版本不再提供服务 (`NoService`)
    /// - 返回 `200` + 补丁列表正文：游戏需要补丁 (`NeedsPatchGame`)
    ///
    /// # 参数
    /// - `oauth_result`: 登录返回的 OAuth 结果
    /// - `game_version`: 游戏版本字符串，如 `"2024.11.01.0000.0000"`
    /// - `boot_hash`: 启动器哈希，格式为 `"ffxivboot.exe/{size}/{sha1}"`，
    ///   目前硬编码为 `"ffxivboot.exe/149504/5f2a70612aa58378eb347869e75adeb8f5581a1b"`
    pub async fn register_session(
        &self,
        oauth_result: &OauthLoginResult,
        game_version: &str,
        boot_hash: &str,
    ) -> Result<RegisterSessionResult, AuthError> {
        let url = format!(
            "{}/http/win32/shanda_release_chs_game/{}",
            oauth_result.session_id, game_version
        );

        let version_report = format!(
            "{}\nex1\t{}\nex2\t{}\nex3\t{}\n",
            boot_hash, game_version, game_version, game_version
        );

        let response = self
            .client
            .post(&url)
            .header("Connection", "Keep-Alive")
            .header("User-Agent", "FFXIV PATCH CLIENT")
            .header("X-Hash-Check", "enabled")
            .body(version_report)
            .send()
            .await?;

        let status = response.status();
        let unique_id = response
            .headers()
            .get("X-Patch-Unique-Id")
            .and_then(|v| v.to_str().ok())
            .map(|s| s.to_string());

        if status.as_u16() == 409 {
            let body = response.text().await.ok().unwrap_or_default();
            return Ok(RegisterSessionResult {
                state: LoginState::NeedsPatchBoot,
                unique_id,
                patch_list: Some(body),
            });
        }

        if status.as_u16() == 410 {
            return Ok(RegisterSessionResult {
                state: LoginState::NoService,
                unique_id,
                patch_list: None,
            });
        }

        Ok(RegisterSessionResult {
            state: LoginState::Ok,
            unique_id,
            patch_list: response.text().await.ok(),
        })
    }
}

/// [`register_session`](SeAuth::register_session) 的返回结果。
///
/// - `state`: 会话状态（正常、需补丁、无服务等）
/// - `unique_id`: 从 `X-Patch-Unique-Id` 响应头获取的会话 UID
/// - `patch_list`: 若需要补丁，包含补丁列表正文
pub struct RegisterSessionResult {
    pub state: LoginState,
    pub unique_id: Option<String>,
    pub patch_list: Option<String>,
}
