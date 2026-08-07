//! Tauri 命令层：将 `launcher` / `game_files` / `config` / `auth` 能力暴露给前端。
//!
//! 所有命令返回 `Result<T, String>`，错误消息直接给前端展示。
//! 长耗时操作（扫码等待、补丁下载）通过 async 命令 + 事件（`patch-progress`）实现。

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use base64::Engine;
use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Emitter, State};
use tokio::sync::Mutex;
use tracing::{info, instrument};
use xiv_launcher_auth::sdo::SdoAuth;
use xiv_launcher_auth::SdoArea;

use crate::auth::{self, Account};
use crate::config::{self, WineSettings};
use crate::game_files::{CheckResult, GameFileManager};
use crate::launcher::{Launcher, LaunchToken, PushLoginSession, QrCodeSession};

/// 全局应用状态。
pub struct AppState {
    pub launcher: Launcher,
    /// 进行中的扫码会话（`qr_login_start` 存入，`qr_login_wait` 取出）
    pub qr: Mutex<Option<QrCodeSession>>,
    /// 进行中的推送会话（同上）
    pub push: Mutex<Option<PushLoginSession>>,
    /// 已登录账号的启动凭证（snda_id → token）
    pub tokens: Mutex<HashMap<String, LaunchToken>>,
    /// 大区列表缓存
    pub areas: Mutex<Option<Vec<SdoArea>>>,
}

// ── DTO ─────────────────────────────────────────────────────────────

/// 账号信息（前端展示用）。
#[derive(Debug, Clone, Serialize)]
pub struct AccountInfo {
    pub snda_id: String,
    pub display_name: String,
    pub can_auto_login: bool,
    pub is_default: bool,
}

/// 设置（`config.toml` 的 `WineSettings` + 游戏目录）。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SettingsDto {
    /// 游戏根目录（含 `boot/`、`game/`、`sdo/`）
    pub game_path: Option<String>,
    #[serde(flatten)]
    pub wine: WineSettings,
}

/// 本地游戏版本。
#[derive(Debug, Clone, Serialize)]
pub struct GameStatusDto {
    pub boot: String,
    pub ffxiv: String,
    pub ex1: String,
    pub ex2: String,
    pub ex3: String,
    pub ex4: String,
    pub ex5: String,
}

/// 更新检查结果。
#[derive(Debug, Clone, Serialize)]
pub struct CheckResultDto {
    pub up_to_date: bool,
    pub patch_count: usize,
    pub total_bytes: u64,
}

/// 补丁进度事件（`patch-progress`）负载。
#[derive(Debug, Clone, Serialize)]
pub struct PatchProgress {
    /// `download` / `install` / `done`
    pub stage: String,
    pub downloaded: u64,
    pub total: u64,
}

// ── 内部工具 ─────────────────────────────────────────────────────────

fn auth_path() -> PathBuf {
    auth::config_path()
}

fn account_info(cfg: &auth::AuthConfig, snda_id: &str) -> Option<AccountInfo> {
    let acc = cfg.find(snda_id)?;
    let is_default = cfg
        .default_account()
        .map(|d| d.snda_id == snda_id)
        .unwrap_or(false);
    Some(AccountInfo {
        snda_id: acc.snda_id.clone(),
        display_name: acc.display_name().to_string(),
        can_auto_login: acc.can_auto_login(),
        is_default,
    })
}

/// 登录成功后的统一收尾：持久化账号（含轮换后的 session key）、缓存 token。
async fn finish_login(state: &AppState, token: LaunchToken) -> Result<AccountInfo, String> {
    let path = auth_path();
    let mut cfg = auth::load(&path);
    // 首个账号自动设为默认
    let make_default = cfg.default_account().is_none();
    cfg.upsert(
        Account {
            snda_id: token.snda_id.clone(),
            username: token.username.clone(),
            auto_login_session_key: token.auto_login_session_key.clone(),
        },
        make_default,
    );
    auth::save(&path, &cfg).map_err(|e| format!("保存账号配置失败: {e}"))?;

    let info = account_info(&cfg, &token.snda_id)
        .ok_or_else(|| "账号保存后未找到".to_string())?;
    state.tokens.lock().await.insert(token.snda_id.clone(), token);
    Ok(info)
}

/// 读取游戏根目录（未设置时返回错误提示）。
fn game_root() -> Result<PathBuf, String> {
    config::load_settings()
        .game_path
        .ok_or_else(|| "请先在设置中配置游戏目录".to_string())
}

/// 获取大区列表（带缓存）。
async fn get_areas_cached(state: &AppState) -> Result<Vec<SdoArea>, String> {
    if let Some(areas) = state.areas.lock().await.as_ref() {
        return Ok(areas.clone());
    }
    let areas = SdoAuth::fetch_server_list()
        .await
        .map_err(|e| format!("获取大区列表失败: {e}"))?;
    *state.areas.lock().await = Some(areas.clone());
    Ok(areas)
}

async fn find_area(state: &AppState, area_id: &str) -> Result<(SdoArea, Vec<SdoArea>), String> {
    let areas = get_areas_cached(state).await?;
    let area = areas
        .iter()
        .find(|a| a.area_id == area_id)
        .cloned()
        .ok_or_else(|| format!("未找到大区 {area_id}"))?;
    Ok((area, areas))
}

fn emit_progress(app: &AppHandle, stage: &str, downloaded: u64, total: u64) {
    let _ = app.emit(
        "patch-progress",
        PatchProgress {
            stage: stage.to_string(),
            downloaded,
            total,
        },
    );
}

// ── 设置 ────────────────────────────────────────────────────────────

#[tauri::command]
pub fn get_settings() -> SettingsDto {
    let mut s = config::load_settings();
    // game_path 提升到 DTO 顶层，避免 flatten 后重复序列化
    let game_path = s.game_path.take();
    SettingsDto {
        game_path: game_path.as_ref().map(|p| p.display().to_string()),
        wine: s,
    }
}

#[tauri::command]
pub fn save_settings(settings: SettingsDto) -> Result<(), String> {
    let mut wine = settings.wine;
    wine.game_path = settings.game_path.filter(|s| !s.is_empty()).map(PathBuf::from);
    config::save_settings(&wine).map_err(|e| format!("保存设置失败: {e}"))
}

// ── 账号管理 ─────────────────────────────────────────────────────────

#[tauri::command]
pub fn list_accounts() -> Vec<AccountInfo> {
    let cfg = auth::load(&auth_path());
    cfg.accounts
        .iter()
        .filter_map(|a| account_info(&cfg, &a.snda_id))
        .collect()
}

#[tauri::command]
pub fn set_default_account(snda_id: String) -> Result<(), String> {
    let path = auth_path();
    let mut cfg = auth::load(&path);
    let acc = cfg
        .find(&snda_id)
        .ok_or_else(|| format!("账号 {snda_id} 不存在"))?
        .clone();
    cfg.upsert(acc, true);
    auth::save(&path, &cfg).map_err(|e| format!("保存账号配置失败: {e}"))
}

#[tauri::command]
pub async fn remove_account(
    state: State<'_, AppState>,
    snda_id: String,
) -> Result<(), String> {
    let path = auth_path();
    let mut cfg = auth::load(&path);
    cfg.remove(&snda_id);
    auth::save(&path, &cfg).map_err(|e| format!("保存账号配置失败: {e}"))?;
    state.tokens.lock().await.remove(&snda_id);
    Ok(())
}

// ── 登录 ────────────────────────────────────────────────────────────

/// 扫码登录第一步：获取二维码，返回 base64 PNG。
#[tauri::command]
pub async fn qr_login_start(state: State<'_, AppState>) -> Result<String, String> {
    let qr = state
        .launcher
        .request_qr_code()
        .await
        .map_err(|e| format!("获取二维码失败: {e}"))?;
    let b64 = base64::engine::general_purpose::STANDARD.encode(qr.image_data());
    *state.qr.lock().await = Some(qr);
    Ok(b64)
}

/// 扫码登录第二步：等待扫码确认（默认 300s 超时），成功后返回账号信息。
///
/// 前端应直接 `await invoke(...)`；期间可调用 `qr_login_start` 重新发起以刷新二维码。
#[tauri::command]
pub async fn qr_login_wait(state: State<'_, AppState>) -> Result<AccountInfo, String> {
    let qr = state
        .qr
        .lock()
        .await
        .take()
        .ok_or_else(|| "没有进行中的扫码会话，请先获取二维码".to_string())?;
    let token = qr
        .wait_for_scan(None)
        .await
        .map_err(|e| format!("扫码登录失败: {e}"))?;
    finish_login(&state, token).await
}

/// 推送登录第一步：向叨鱼 App 发起推送，返回验证序号（展示给用户核对）。
#[tauri::command]
pub async fn push_login_start(
    state: State<'_, AppState>,
    account: String,
) -> Result<Option<String>, String> {
    let push = state
        .launcher
        .request_push_login(&account)
        .await
        .map_err(|e| format!("发起推送登录失败: {e}"))?;
    let serial = push.serial_num().map(str::to_string);
    *state.push.lock().await = Some(push);
    Ok(serial)
}

/// 推送登录第二步：等待用户在叨鱼 App 确认（默认 30s 超时）。
#[tauri::command]
pub async fn push_login_wait(state: State<'_, AppState>) -> Result<AccountInfo, String> {
    let push = state
        .push
        .lock()
        .await
        .take()
        .ok_or_else(|| "没有进行中的推送会话".to_string())?;
    let token = push
        .wait_for_confirm(None)
        .await
        .map_err(|e| format!("推送登录失败: {e}"))?;
    finish_login(&state, token).await
}

/// 密码登录。
#[tauri::command]
pub async fn password_login(
    state: State<'_, AppState>,
    account: String,
    password: String,
) -> Result<AccountInfo, String> {
    let token = state
        .launcher
        .login_password(&account, &password)
        .await
        .map_err(|e| format!("密码登录失败: {e}"))?;
    finish_login(&state, token).await
}

/// 自动登录（使用已保存的 session key；成功后自动保存轮换的新 key）。
#[tauri::command]
pub async fn auto_login(state: State<'_, AppState>, snda_id: String) -> Result<AccountInfo, String> {
    let cfg = auth::load(&auth_path());
    let key = cfg
        .find(&snda_id)
        .and_then(|a| a.auto_login_session_key.clone())
        .ok_or_else(|| "该账号没有可用的自动登录凭证，请重新登录".to_string())?;
    let token = state
        .launcher
        .login_auto(&key)
        .await
        .map_err(|e| format!("自动登录失败: {e}"))?;
    finish_login(&state, token).await
}

// ── 大区 ────────────────────────────────────────────────────────────

#[tauri::command]
pub async fn list_areas(state: State<'_, AppState>) -> Result<Vec<SdoArea>, String> {
    get_areas_cached(&state).await
}

// ── 游戏更新 ─────────────────────────────────────────────────────────

#[tauri::command]
pub fn game_status() -> Result<GameStatusDto, String> {
    let root = game_root()?;
    let v = GameFileManager::new().status(&root);
    Ok(GameStatusDto {
        boot: v.boot,
        ffxiv: v.ffxiv,
        ex1: v.ex1,
        ex2: v.ex2,
        ex3: v.ex3,
        ex4: v.ex4,
        ex5: v.ex5,
    })
}

#[tauri::command]
pub async fn check_game(state: State<'_, AppState>, area_id: String) -> Result<CheckResultDto, String> {
    let root = game_root()?;
    let (area, _) = find_area(&state, &area_id).await?;
    let result = GameFileManager::new()
        .check_update(&area, &root, false, 5)
        .await
        .map_err(|e| format!("检查更新失败: {e}"))?;
    match result {
        CheckResult::UpToDate { .. } => Ok(CheckResultDto {
            up_to_date: true,
            patch_count: 0,
            total_bytes: 0,
        }),
        CheckResult::NeedsPatch { patches, .. } => Ok(CheckResultDto {
            up_to_date: false,
            patch_count: patches.len(),
            total_bytes: patches.iter().map(|p| p.length).sum(),
        }),
        CheckResult::NeedsPatchBoot => Err("boot 需要更新（国服暂不支持）".to_string()),
    }
}

/// 下载并应用补丁。进度通过 `patch-progress` 事件推送。
#[tauri::command]
#[instrument(skip(state, app))]
pub async fn update_game(
    state: State<'_, AppState>,
    app: AppHandle,
    area_id: String,
) -> Result<String, String> {
    let root = game_root()?;
    let (area, _) = find_area(&state, &area_id).await?;
    let mgr = GameFileManager::new();

    let result = mgr
        .check_update(&area, &root, false, 5)
        .await
        .map_err(|e| format!("检查更新失败: {e}"))?;

    let patches = match result {
        CheckResult::UpToDate { .. } => {
            emit_progress(&app, "done", 0, 0);
            return Ok("游戏已是最新".to_string());
        }
        CheckResult::NeedsPatch { patches, .. } => patches,
        CheckResult::NeedsPatchBoot => return Err("boot 需要更新（国服暂不支持）".to_string()),
    };

    let patch_dir = dirs::home_dir()
        .map(|h| h.join(".xiv-launcher-rs/patches"))
        .unwrap_or_else(|| PathBuf::from("patches"));

    let total: u64 = patches.iter().map(|p| p.length).sum();
    info!(count = patches.len(), total, "downloading patches");
    let app_dl = app.clone();
    let summary = mgr
        .download(&patches, &patch_dir, 4, move |downloaded, total| {
            emit_progress(&app_dl, "download", downloaded, total);
        })
        .await
        .map_err(|e| format!("补丁下载失败: {e}"))?;

    emit_progress(&app, "install", 0, summary.total_bytes);
    let install = mgr
        .install(&patches, &patch_dir, &root)
        .await
        .map_err(|e| format!("补丁安装失败: {e}"))?;

    emit_progress(&app, "done", total, total);
    Ok(format!(
        "更新完成：安装 {} 个补丁（跳过 {} 个）",
        install.installed.len(),
        install.skipped
    ))
}

// ── 启动游戏 ─────────────────────────────────────────────────────────

/// 启动游戏。账号需已在本会话登录（tokens 缓存中有凭证）。
#[tauri::command]
#[instrument(skip(state))]
pub async fn launch_game(
    state: State<'_, AppState>,
    snda_id: String,
    area_id: String,
) -> Result<String, String> {
    let token = state
        .tokens
        .lock()
        .await
        .get(&snda_id)
        .cloned()
        .ok_or_else(|| "该账号尚未登录，请先登录".to_string())?;

    let root = game_root()?;
    let exe = root.join("game").join("ffxiv_dx11.exe");
    if !exe.exists() {
        return Err(format!("找不到游戏程序 {}", exe.display()));
    }

    let (area, areas) = find_area(&state, &area_id).await?;
    let settings = config::load_settings();

    let result = state
        .launcher
        .launch_with_wine(&settings, &token, area, areas, &exe)
        .await
        .map_err(|e| format!("启动失败: {e}"))?;

    info!(pid = result.child.id(), "game launched from GUI");
    Ok(match result.log_path {
        Some(p) => format!("游戏已启动，日志: {}", p.display()),
        None => "游戏已启动".to_string(),
    })
}

/// 用默认游戏根目录解析补丁目录是否存在（供前端提示，可选）。
#[tauri::command]
pub fn game_root_valid() -> bool {
    game_root()
        .map(|r| Path::new(&r).join("game/ffxiv_dx11.exe").exists())
        .unwrap_or(false)
}
