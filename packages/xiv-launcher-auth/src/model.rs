use serde::Deserialize;

/// 登录方式。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LoginType {
    /// SDO 密码登录（密码直接提交到 `staticLogin.json`）
    SdoStatic,
    /// SDO 推送/滑动登录（`sendPushMessage` + `pushMessageLogin` 轮询）
    SdoSlide,
    /// SDO 扫码登录（`getCodeKey` 获取二维码 + `codeKeyLogin` 轮询）
    SdoQrCode,
    /// WeGame Token 登录（`thirdPartyLogin`，companyid=310）
    WeGameToken,
    /// WeGame SID 直接构造会话（跳过 SDO 认证）
    WeGameSid,
    /// SDO 自动登录（`autoLogin.json`，使用之前保存的 `auto_login_session_key`）
    AutoLoginSession,
    /// Square Enix 国际服 OAuth 登录
    SquareEnix,
    /// Steam 登录（OAuth 流程附带加密 Steam ticket）
    Steam,
}

/// 登录/会话注册结果状态。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LoginState {
    Unknown,
    /// 登录成功，会话正常
    Ok,
    /// 游戏需要补丁
    NeedsPatchGame,
    /// 启动器需要补丁
    NeedsPatchBoot,
    /// 版本不再提供服务
    NoService,
    /// 未同意条款
    NoTerms,
    /// 需要重试
    NeedRetry,
}

/// OAuth 登录结果（国际服和部分国服登录方式共用）。
///
/// 包含游戏启动所需的核心会话信息：
/// - `session_id`: 游戏会话 ID，用于 `DEV.TestSID` 启动参数（国际服直接使用，
///   国服经 `ssoLogin` 获取 ticket 后使用）
/// - `max_expansion`: 最大资料片等级（用于 `DEV.MaxEntitledExpansionID` 启动参数）
/// - `region`: 区域代码（用于 `SYS.Region` 启动参数）
#[derive(Debug, Clone)]
pub struct OauthLoginResult {
    /// 游戏会话 ID（国际服直接用作 `DEV.TestSID`，国服由 SSO ticket 替代）
    pub session_id: String,
    /// 登录用户名
    pub input_user_id: String,
    /// SDO 账号 ID（国服使用）
    pub snda_id: String,
    /// 区域代码（1=JP, 2=NA, 3=EU 等）
    pub region: i32,
    /// 是否已同意服务条款
    pub terms_accepted: bool,
    /// 账号是否可游玩
    pub playable: bool,
    /// 最大资料片等级（5=Dawntrail）
    pub max_expansion: i32,
    /// 登录方式
    pub login_type: LoginType,
}

/// 登录完整结果。
#[derive(Debug, Clone)]
pub struct LoginResult {
    /// 登录状态
    pub state: LoginState,
    /// OAuth 登录结果（登录成功时非 None）
    pub oauth_login: Option<OauthLoginResult>,
    /// 补丁会话 UID（来自 `X-Patch-Unique-Id` 响应头）
    pub unique_id: Option<String>,
    /// 待下载的补丁列表
    pub pending_patches: Vec<PatchListEntry>,
    /// 选中的国服大区（仅国服使用）
    pub area: Option<SdoArea>,
    /// 所有国服大区列表
    pub areas: Vec<SdoArea>,
    /// DC 跨服传送端口（仅国服使用）
    pub dc_travel_port: Option<i32>,
}

/// 补丁列表条目。
///
/// 对应 C# `PatchListEntry`：TSV 补丁列表的一行。
/// 9 字段行（游戏补丁）带 hash 信息，6 字段行（boot 补丁）只有 `url`。
#[derive(Debug, Clone)]
pub struct PatchListEntry {
    /// 补丁后的目标版本（`VersionId`）。
    pub version: String,
    /// 补丁文件下载地址。
    pub url: String,
    /// 哈希算法（如 `sha1`）。
    pub hash_type: String,
    /// 哈希块大小（字节），按块校验。
    pub hash_block_size: u64,
    /// 逐块哈希列表（与文件按 `hash_block_size` 分块一一对应）。
    pub hashes: Vec<String>,
    /// 补丁文件大小（字节）。
    pub length: u64,
}

/// 国服大区信息，从 `ff.dorado.sdo.com/ff/area/serverlist_new.js` 获取。
///
/// 每个大区提供独立的 lobby、GM、补丁、存档上传服务器地址，
/// 这些地址用于构造游戏启动参数和国服补丁检查。
#[derive(Debug, Clone, Deserialize)]
pub struct SdoArea {
    #[serde(rename = "Areaid")]
    pub area_id: String,
    #[serde(rename = "AreaStat")]
    pub area_stat: i32,
    #[serde(rename = "AreaOrder")]
    pub area_order: i32,
    #[serde(rename = "AreaName")]
    pub area_name: String,
    #[serde(rename = "Areatype")]
    pub area_type: i32,
    #[serde(rename = "AreaLobby")]
    pub area_lobby: String,
    #[serde(rename = "AreaGm")]
    pub area_gm: String,
    #[serde(rename = "AreaPatch")]
    pub area_patch: String,
    #[serde(rename = "AreaConfigUpload")]
    pub area_config_upload: String,
}

/// SDO 登录 API 的返回结构。
///
/// `return_code` 为 0 表示成功，负值表示各种错误（如需验证码、首次登录等）。
/// `data` 中包含登录凭证（`tgt`、`ticket`、`auto_login_session_key` 等），
/// 具体字段取决于登录方式。
#[derive(Debug, Clone, Deserialize)]
pub struct SdoLoginResult {
    #[serde(rename = "return_code")]
    pub return_code: i32,
    #[serde(default)]
    pub error_type: Option<i32>,
    pub data: SdoLoginData,
}

/// SDO 登录返回的详细数据。
///
/// 不同登录方式会填充不同字段：
/// - 密码登录/扫码/推送: `tgt`, `snda_id`
/// - SSO: `ticket`（即游戏 session ID）
/// - 自动登录: `tgt`, `auto_login_session_key`
/// - 推送登录: `push_msg_session_key`（用于轮询）
#[derive(Debug, Clone, Deserialize)]
pub struct SdoLoginData {
    #[serde(default, rename = "failReason", alias = "failReason")]
    pub fail_reason: Option<String>,
    #[serde(default, rename = "nextAction", alias = "nextAction")]
    pub next_action: Option<i32>,
    #[serde(default)]
    pub guid: Option<String>,
    #[serde(default, rename = "dynamicKey", alias = "dynamicKey")]
    pub dynamic_key: Option<String>,
    #[serde(default)]
    pub ticket: Option<String>,
    #[serde(default, rename = "SndaId", alias = "sndaId")]
    pub snda_id: Option<String>,
    #[serde(default)]
    pub tgt: Option<String>,
    #[serde(default, rename = "autoLoginSessionKey", alias = "autoLoginSessionKey")]
    pub auto_login_session_key: Option<String>,
    #[serde(default, rename = "autoLoginMaxAge", alias = "autoLoginMaxAge")]
    pub auto_login_max_age: Option<i32>,
    #[serde(default, rename = "inputUserId", alias = "inputUserId")]
    pub input_user_id: Option<String>,
    #[serde(default, rename = "pushMsgSerialNum", alias = "pushMsgSerialNum")]
    pub push_msg_serial_num: Option<String>,
    #[serde(default, rename = "pushMsgSessionKey", alias = "pushMsgSessionKey")]
    pub push_msg_session_key: Option<String>,
    #[serde(default, rename = "accountArray", alias = "accountArray")]
    pub account_array: Option<Vec<String>>,
    #[serde(default, rename = "SndaIdArray", alias = "sndaIdArray")]
    pub snda_id_array: Option<Vec<String>>,
}
