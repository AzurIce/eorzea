//! # eorzea-auth
//!
//! Eorzea（FFXIV 国服启动器）认证库，支持通过 feature gate 选择启用不同服务器的登录实现：
//!
//! - **`sdo`**（默认启用）— 中国服（盛趣）登录，包含密码、推送、扫码、自动登录等流程
//! - **`se`** — 国际服（Square Enix）OAuth 登录
//!
//! ```toml
//! # 仅启用国服
//! eorzea-auth = { default-features = false, features = ["sdo"] }
//! # 仅启用国际服
//! eorzea-auth = { default-features = false, features = ["se"] }
//! # 两者都启用
//! eorzea-auth = { features = ["sdo", "se"] }
//! ```

pub mod crypto;
pub mod error;
pub mod model;

#[cfg(feature = "sdo")]
pub mod sdo;

#[cfg(feature = "sdo")]
pub mod sdo_device;

#[cfg(feature = "se")]
pub mod se;

pub use error::AuthError;
pub use model::*;
