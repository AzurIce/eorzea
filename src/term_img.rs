//! 终端图片显示。
//!
//! 优先使用 kitty graphics protocol（检测 `KITTY_WINDOW_ID` 或 `TERM=kitty`），
//! 其次 iTerm2 内联图片（OSC 1337），最后回退为提示保存文件。
//!
//! # kitty 协议
//!
//! ```text
//! \x1b_Ga=T,f=100,s=W,v=H,m=1;<base64 chunk>\x1b\\   // 分块传输
//! \x1b_Gm=0;\x1b\\                                    // 完成并显示
//! ```
//!
//! PNG（`f=100`）无需 s/v 参数，kitty 从文件头读取尺寸，但显式传更稳。

use base64::Engine;
use tracing::debug;

/// 检测终端是否支持 kitty graphics protocol。
pub fn kitty_supported() -> bool {
    std::env::var_os("KITTY_WINDOW_ID").is_some() || std::env::var("TERM").ok().as_deref() == Some("kitty")
}

/// 检测终端是否支持 iTerm2 内联图片（OSC 1337）。
pub fn iterm_supported() -> bool {
    std::env::var_os("ITERM_SESSION_ID").is_some() || std::env::var("TERM_PROGRAM").ok().as_deref() == Some("iTerm.app")
}

/// 从 PNG 字节解析尺寸（IHDR 头，宽高为大端 u32）。
fn png_size(data: &[u8]) -> Option<(u32, u32)> {
    if data.len() < 24 || &data[..8] != b"\x89PNG\r\n\x1a\n" || &data[12..16] != b"IHDR" {
        return None;
    }
    let w = u32::from_be_bytes([data[16], data[17], data[18], data[19]]);
    let h = u32::from_be_bytes([data[20], data[21], data[22], data[23]]);
    Some((w, h))
}

/// 通过 kitty graphics protocol 显示 PNG。
///
/// 大图分块（每块 ~8000 字节 base64），最后一块 `m=0` 触发显示。
fn display_kitty(data: &[u8]) -> std::io::Result<()> {
    use std::io::Write;

    let b64 = base64::engine::general_purpose::STANDARD.encode(data);
    let (w, h) = png_size(data).unwrap_or((0, 0));

    let mut out = std::io::stdout();
    let chunk_size = 8000;
    let mut written = 0usize;

    while written < b64.len() {
        let end = (written + chunk_size).min(b64.len());
        let chunk = &b64[written..end];
        let is_last = end == b64.len();

        let params = if written == 0 {
            if w > 0 && h > 0 {
                format!("a=T,f=100,s={w},v={h},m={}", if is_last { 0 } else { 1 })
            } else {
                format!("a=T,f=100,m={}", if is_last { 0 } else { 1 })
            }
        } else {
            format!("m={}", if is_last { 0 } else { 1 })
        };

        write!(out, "\x1b_G{params};{chunk}\x1b\\")?;
        out.flush()?;
        written = end;
    }

    // 换行，避免图片和后续输出粘连
    out.write_all(b"\n")?;
    out.flush()?;
    Ok(())
}

/// 通过 iTerm2 OSC 1337 内联图片显示 PNG。
fn display_iterm(data: &[u8]) -> std::io::Result<()> {
    use std::io::Write;

    let b64 = base64::engine::general_purpose::STANDARD.encode(data);
    let (w, h) = png_size(data).unwrap_or((0, 0));

    let mut out = std::io::stdout();
    write!(
        out,
        "\x1b]1337;File=inline=1;preserveAspectRatio=1{};base64,{}\x07",
        if w > 0 && h > 0 {
            format!(";width={}px;height={}px", w, h)
        } else {
            String::new()
        },
        b64
    )?;
    out.write_all(b"\n")?;
    out.flush()?;
    Ok(())
}

/// 在终端显示 PNG 图片。
///
/// 自动选择协议；无协议支持时返回 `Err`（调用方提示保存文件）。
pub fn display_png(data: &[u8]) -> std::io::Result<()> {
    if kitty_supported() {
        debug!("using kitty graphics protocol");
        return display_kitty(data);
    }
    if iterm_supported() {
        debug!("using iTerm2 inline images");
        return display_iterm(data);
    }
    Err(std::io::Error::new(
        std::io::ErrorKind::Unsupported,
        "terminal does not support image display",
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 构造一个最小合法 PNG（1x1 透明像素）。
    fn tiny_png() -> Vec<u8> {
        // 手动构造 1x1 RGBA PNG
        let ihdr_data = {
            let mut d = Vec::new();
            d.extend_from_slice(&13u32.to_be_bytes()); // IHDR 长度
            d.extend_from_slice(b"IHDR");
            d.extend_from_slice(&1u32.to_be_bytes()); // width
            d.extend_from_slice(&1u32.to_be_bytes()); // height
            d.extend_from_slice(&[8, 6, 0, 0, 0]); // bit depth 8, color type 6 (RGBA)
            d
        };
        // 没有 CRC 校验的简化构造——只测 png_size 不依赖完整校验
        let mut png = Vec::new();
        png.extend_from_slice(b"\x89PNG\r\n\x1a\n");
        png.extend_from_slice(&ihdr_data);
        png.extend_from_slice(&[0u8; 4]); // CRC 占位
        png
    }

    #[test]
    fn test_png_size() {
        let png = tiny_png();
        assert_eq!(png_size(&png), Some((1, 1)));
    }

    #[test]
    fn test_png_size_invalid() {
        assert_eq!(png_size(b"not a png"), None);
    }
}
