# Agent Guidelines

## Project Overview

`xiv-launcher-rs` is a Rust reimplementation of the FFXIV launcher, porting auth and game-launch logic from `XIVLauncher.Core` (C#). The project uses a Tauri + Rust workspace:

```
xiv-launcher-rs/              # workspace root (tauri crate)
├── packages/
│   └── xiv-launcher-auth/    # auth library (feature-gated)
└── frontend/                 # Tauri frontend (Vite + Svelte 5 + TS, bun)
```

GUI 相关：`src/commands.rs` 是 Tauri 命令层（设置/账号/登录/大区/更新/启动），
`src/lib.rs` 注册命令并管理 `AppState`（Launcher、进行中的扫码/推送会话、token 缓存、大区缓存）。
补丁进度通过 `patch-progress` 事件推送前端。

**Current focus: SDO (中国服/盛趣) authentication and game launch.** The `se` (international) feature is implemented but not the priority.

## Key Directives

1. **Keep TODO.md in sync** — `TODO.md` at the repository root tracks all known gaps between this Rust implementation and the upstream C# reference. Whenever you:
   - implement a missing feature,
   - fix a behavioural discrepancy, or
   - discover a *new* difference not yet documented,
   **you MUST update `TODO.md`** to reflect the current state (check off completed items or add new ones).

2. **Minimal changes** — Make the smallest change possible to achieve the goal. Do not restructure or refactor unrelated code.

3. **Follow existing style** — Match the formatting, naming, and error-handling patterns already present in the crate you are editing.

4. **Test what you change** — Run `cargo check -p xiv-launcher-auth` (with appropriate features) after any change. If you add new functionality, add a test or verify it with an example binary.

## Feature Gates

`xiv-launcher-auth` uses Cargo feature gates to separate server-region logic:

| Feature | Default | Description |
|---------|---------|-------------|
| `sdo` | Yes | 中国服 (SDO/盛趣) 登录：密码、推送、扫码、自动登录、SSO |
| `se` | No | 国际服 (Square Enix) OAuth 登录 |

```toml
# Default build — SDO only
cargo check -p xiv-launcher-auth

# SE only
cargo check -p xiv-launcher-auth --no-default-features --features se

# Both
cargo check -p xiv-launcher-auth --features se
```

Example binaries are also feature-gated:
```bash
cargo run -p xiv-launcher-auth --example sdo_login          # requires feature "sdo" (default)
cargo run -p xiv-launcher-auth --example se_login            # requires feature "se"
```

## Architecture Notes

### `packages/xiv-launcher-auth/`

| File | Feature | Purpose |
|------|---------|---------|
| `lib.rs` | — | Crate root, re-exports, feature-gated module includes |
| `model.rs` | — | Shared data structures (`LoginResult`, `OauthLoginResult`, `SdoArea`, `SdoLoginData`, etc.) |
| `error.rs` | — | `AuthError` enum covering both SDO and SE errors |
| `crypto.rs` | — | Shared crypto utilities (SHA1, MD5, computer ID, SE Base64 mangling) |
| `sdo.rs` | `sdo` | SDO auth client: `static_login`, `slide_login_*`, `qr_code_*`, `auto_login`, `sso_login`, `fetch_server_list` |
| `se.rs` | `se` | SE OAuth client: `login` (top + login.send), `register_session` |

### Reference Implementation (C#)

When in doubt about protocol details, check the corresponding files in `XIVLauncher.Core`:

> **参考仓库默认位置**：`~/Files/repos/XIVLauncher.Core`（主仓库，`cn` 分支）。
> 实际认证/启动代码在其 submodule `lib/FFXIVQuickLauncher/`（CN 分支）下，
> 例如 `SdoLauncher.cs` 位于
> `~/Files/repos/XIVLauncher.Core/lib/FFXIVQuickLauncher/src/XIVLauncher.Common/Game/SdoLauncher.cs`。
> 文档中凡提及 `XIVLauncher.Core/...` 均指该默认位置。

| Rust module | C# reference |
|-------------|--------------|
| `sdo.rs` | `SdoLauncher.cs`, `SdoUtils.cs`, `SdoArea.cs` |
| `se.rs` | `Launcher.cs` (OAuth section) |
| `crypto.rs` | `Ticket.cs`, `ArgumentBuilder.cs`, `SdoUtils.cs` |
| `model.rs` | `SdoArea.cs`, `SdoLoginResult.cs`, various model classes |
