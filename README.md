# xiv-launcher-rs

FFXIV 国服（SDO/盛趣）启动器的 Rust 实现，移植自 XIVLauncher.Core（C#）。

- GUI：dioxus-native（Blitz/Vello 原生渲染），入口 `src/main.rs`，UI 在 `src/ui/`
- CLI：`src/bin/xlcli.rs`（`xlcli areas/game/auth/dalamud/launch`）
- 认证库：`packages/xiv-launcher-auth`（feature gate：`sdo` 默认，`se` 可选）

```bash
cargo run                # 启动 GUI
cargo run --bin xlcli -- --help
```
