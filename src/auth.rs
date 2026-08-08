//! eorzea 账号配置（`~/.xiv-launcher-rs/auth.toml`）。
//!
//! 与 Wine 配置（`config.toml`，见 `settings.rs`）分开存储。
//! 位置固定：`~/.xiv-launcher-rs/auth.toml`。
//! 旧版 `eorzea.toml` 会在首次加载时自动迁移。
//!
//! ```toml
//! default_account = "12345"
//!
//! [[accounts]]
//! snda_id = "12345"
//! username = "foo@example.com"
//! auto_login_session_key = "..."
//! ```
//!
//! 账号唯一标识为 `snda_id`（扫码/密码/自动登录都能拿到）。

use std::path::{Path, PathBuf};
use serde::{Deserialize, Serialize};
use tracing::{debug, info};

/// 账号记录。
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Account {
    /// SDO 账号 ID（唯一标识）。
    pub snda_id: String,
    /// 登录账号名（密码登录时记录，用于展示）。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub username: Option<String>,
    /// 自动登录 session key（`SdoLoginData.auto_login_session_key`）。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub auto_login_session_key: Option<String>,
}

impl Account {
    /// 展示名：username 优先，否则 snda_id。
    pub fn display_name(&self) -> &str {
        self.username.as_deref().unwrap_or(&self.snda_id)
    }

    /// 是否可以自动登录（有 session key）。
    pub fn can_auto_login(&self) -> bool {
        self.auto_login_session_key.is_some()
    }
}

/// 根配置。
#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq)]
pub struct AuthConfig {
    /// 默认账号标识（优先 username，无 username 时回退 snda_id）。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub default_account: Option<String>,
    /// 全部账号。
    #[serde(default)]
    pub accounts: Vec<Account>,
}

impl AuthConfig {
    /// 按 snda_id 精确查找账号。
    pub fn find(&self, snda_id: &str) -> Option<&Account> {
        self.accounts.iter().find(|a| a.snda_id == snda_id)
    }

    /// 按 username 或 snda_id 查找账号（CLI 交互用）。
    pub fn find_by_identifier(&self, identifier: &str) -> Option<&Account> {
        self.accounts
            .iter()
            .find(|a| a.snda_id == identifier || a.display_name() == identifier)
    }

    /// 默认账号（`default_account` 存 username 优先，snda_id 兜底，两者都匹配）。
    pub fn default_account(&self) -> Option<&Account> {
        self.default_account
            .as_ref()
            .and_then(|id| self.find_by_identifier(id))
    }

    /// 更新（或新增）账号记录，并可选设为默认。
    pub fn upsert(&mut self, account: Account, make_default: bool) {
        match self.accounts.iter_mut().find(|a| a.snda_id == account.snda_id) {
            Some(existing) => {
                if account.username.is_some() {
                    existing.username = account.username.clone();
                }
                if account.auto_login_session_key.is_some() {
                    existing.auto_login_session_key = account.auto_login_session_key.clone();
                }
                if make_default {
                    // 默认标识优先 username（可读），无 username 回退 snda_id
                    self.default_account = Some(
                        existing
                            .username
                            .clone()
                            .unwrap_or_else(|| existing.snda_id.clone()),
                    );
                }
            }
            None => {
                if make_default {
                    self.default_account = Some(
                        account
                            .username
                            .clone()
                            .unwrap_or_else(|| account.snda_id.clone()),
                    );
                }
                self.accounts.push(account);
            }
        }
    }

    /// 删除账号（默认账号被删时清除默认标记）。
    pub fn remove(&mut self, snda_id: &str) -> bool {
        let before = self.accounts.len();
        self.accounts.retain(|a| a.snda_id != snda_id);
        let removed = self.accounts.len() != before;
        if removed {
            // 默认标识可能是 username 或 snda_id；若已指向被删账号则清除
            let default_still_valid = self
                .default_account
                .as_ref()
                .map(|d| self.find_by_identifier(d).is_some())
                .unwrap_or(false);
            if !default_still_valid {
                self.default_account = None;
            }
        }
        removed
    }
}

/// 配置文件路径：`~/.xiv-launcher-rs/auth.toml`。
pub fn config_path() -> PathBuf {
    dirs::home_dir()
        .map(|h| h.join(".xiv-launcher-rs/auth.toml"))
        .unwrap_or_else(|| PathBuf::from("auth.toml"))
}

/// 旧版 `eorzea.toml` 路径（迁移用）。
pub fn legacy_path() -> PathBuf {
    dirs::home_dir()
        .map(|h| h.join(".xiv-launcher-rs/eorzea.toml"))
        .unwrap_or_else(|| PathBuf::from("eorzea.toml"))
}

/// 加载配置（文件不存在时返回空配置，旧文件自动迁移）。
pub fn load(path: &Path) -> AuthConfig {
    if !path.exists() {
        let legacy = legacy_path();
        if legacy.exists() && legacy != *path {
            let cfg = load_from_legacy(&legacy);
            if cfg != AuthConfig::default() {
                info!(path = %legacy.display(), "migrating legacy eorzea.toml to auth.toml");
                let _ = save(path, &cfg);
                return cfg;
            }
        }
    }
    match std::fs::read_to_string(path) {
        Ok(content) => match toml::from_str(&content) {
            Ok(cfg) => {
                debug!(path = %path.display(), "loaded config");
                cfg
            }
            Err(e) => {
                info!(path = %path.display(), error = %e, "config parse failed, using empty");
                AuthConfig::default()
            }
        },
        Err(_) => {
            debug!(path = %path.display(), "config not found, using empty");
            AuthConfig::default()
        }
    }
}

/// 从指定 TOML 路径加载（不迁移）。
pub fn load_from_legacy(path: &Path) -> AuthConfig {
    std::fs::read_to_string(path)
        .ok()
        .and_then(|s| toml::from_str(&s).ok())
        .unwrap_or_default()
}

/// 保存配置到文件（自动创建父目录）。
pub fn save(path: &Path, config: &AuthConfig) -> std::io::Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let content = toml::to_string_pretty(config).map_err(std::io::Error::other)?;
    std::fs::write(path, content)?;
    debug!(path = %path.display(), "saved config");
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_upsert_new_account() {
        let mut cfg = AuthConfig::default();
        cfg.upsert(
            Account {
                snda_id: "1".into(),
                username: Some("user1".into()),
                auto_login_session_key: Some("key1".into()),
            },
            true,
        );
        // 默认标识优先存 username
        assert_eq!(cfg.default_account.as_deref(), Some("user1"));
        assert_eq!(cfg.accounts.len(), 1);
        assert!(cfg.default_account().unwrap().can_auto_login());
        // 也能通过 snda_id 找到
        assert!(cfg.find_by_identifier("1").is_some());
    }

    #[test]
    fn test_upsert_without_username_defaults_to_snda_id() {
        let mut cfg = AuthConfig::default();
        cfg.upsert(
            Account {
                snda_id: "1".into(),
                username: None,
                auto_login_session_key: None,
            },
            true,
        );
        assert_eq!(cfg.default_account.as_deref(), Some("1"));
        assert!(cfg.default_account().is_some());
    }

    #[test]
    fn test_default_account_matches_username_or_snda_id() {
        let mut cfg = AuthConfig::default();
        cfg.upsert(Account { snda_id: "1".into(), username: Some("user1".into()), auto_login_session_key: Some("k1".into()) }, true);
        // 老配置兼容：default 存 snda_id 也能找到
        cfg.default_account = Some("1".into());
        assert_eq!(cfg.default_account().unwrap().display_name(), "user1");
    }

    #[test]
    fn test_upsert_existing_keeps_default() {
        let mut cfg = AuthConfig::default();
        cfg.upsert(Account { snda_id: "1".into(), username: Some("a".into()), auto_login_session_key: Some("k1".into()) }, true);
        cfg.upsert(Account { snda_id: "1".into(), username: None, auto_login_session_key: Some("k2".into()) }, false);
        assert_eq!(cfg.accounts.len(), 1);
        // default 存 username（首次设为默认时）
        assert_eq!(cfg.default_account.as_deref(), Some("a"));
        assert_eq!(cfg.accounts[0].auto_login_session_key.as_deref(), Some("k2"));
        assert_eq!(cfg.accounts[0].username.as_deref(), Some("a")); // 保留旧 username
    }

    #[test]
    fn test_remove_default() {
        let mut cfg = AuthConfig::default();
        cfg.upsert(Account { snda_id: "1".into(), username: None, auto_login_session_key: None }, true);
        assert!(cfg.remove("1"));
        assert!(cfg.default_account.is_none());
        assert!(!cfg.remove("1"));
    }

    #[test]
    fn test_roundtrip() {
        let mut cfg = AuthConfig::default();
        cfg.default_account = Some("1".into());
        cfg.accounts.push(Account {
            snda_id: "1".into(),
            username: Some("user".into()),
            auto_login_session_key: Some("key".into()),
        });
        let toml_str = toml::to_string_pretty(&cfg).unwrap();
        let back: AuthConfig = toml::from_str(&toml_str).unwrap();
        assert_eq!(back, cfg);
    }
}
