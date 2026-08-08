# eorzea

FFXIV 国服（SDO/盛趣）启动器，Rust 实现。

- GUI：dioxus-native（Blitz/Vello 原生渲染），入口 `src/main.rs`，UI 在 `src/ui/`
- CLI：`packages/eorzea-cli`（bin `eoz`，`eoz areas/game/auth/dalamud/launch`）
- 认证库：`packages/eorzea-auth`（feature gate：`sdo` 默认，`se` 可选）

```bash
cargo run                # 启动 GUI
cargo run -p eorzea-cli -- --help
```
