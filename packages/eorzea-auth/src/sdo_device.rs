//! SDO 设备指纹采集模块。
//!
//! 对标 C# `SdoUtils.cs`，为 SDO 服务端提供设备标识参数。
//!
//! # 采集策略（按平台）
//!
//! | 方法 | Linux | macOS |
//! |------|-------|-------|
//! | `mac_address` | `/etc/machine-id` → MD5(hex) | `ioreg -rd1 -c IOPlatformExpertDevice` 取 `IOPlatformSerialNumber` + 系统盘序列号 → MD5(hex) |
//! | `mac_id` (raw) | `/etc/machine-id` 原文 | `IOPlatformSerialNumber` + 系统盘序列号拼接 |
//! | `cpu_id` | `/proc/cpuinfo` 取 `model name` 行 → MD5(hex) | `IOPlatformSerialNumber` → MD5(hex) |
//! | `disk_serial` | `lsblk --nodeps -no SERIAL /dev/sda` 或 udev → MD5(hex) | `diskutil info disk0` 取 Serial Number → MD5(hex) |
//!
//! 最终 `device_id = format!("{}:{}:{}", mac_address, cpu_id, disk_serial)`

use crate::crypto::md5_hex_upper;
use std::process::Command;

/// 采集 MAC 地址的 MD5（大写 hex），用于 SDO `macId` 参数的哈希值。
///
/// - Linux: 读取 `/etc/machine-id` 并对其做 MD5
/// - macOS: 读取 IOPlatformSerialNumber + 系统盘序列号拼接后做 MD5
///
/// C# 参考: `SdoUtils.GetMacAddress()` — Linux 用 `DeviceIdBuilder.OnLinux(AddMachineId)`，
/// macOS 用 `DeviceIdBuilder.OnMac(AddPlatformSerialNumber + AddSystemDriveSerialNumber)`
pub fn get_mac_address_hash() -> String {
    md5_hex_upper(get_mac_id_raw().as_bytes())
}

/// 采集 MAC 原始标识（不做 MD5），用于 SDO `macId` 参数本身及 Cookie 生成。
///
/// - Linux: `/etc/machine-id` 内容
/// - macOS: `IOPlatformSerialNumber` + 系统盘序列号
///
/// C# 参考: `SdoUtils.GetMac()` — 返回原始字符串（非 MD5）
pub fn get_mac_id_raw() -> String {
    if cfg!(target_os = "linux") {
        get_linux_machine_id()
    } else if cfg!(target_os = "macos") {
        let serial = get_macos_platform_serial().unwrap_or_default();
        let disk_serial = get_macos_disk_serial().unwrap_or_default();
        format!("{}{}", serial, disk_serial)
    } else {
        get_fallback_machine_id()
    }
}

/// 采集 CPU ID 的 MD5（大写 hex），用于 `device_id` 的第二段。
///
/// - Linux: `/proc/cpuinfo` 所有 `model name` 行拼接后做 MD5
/// - macOS: `IOPlatformSerialNumber` 做 MD5
///
/// C# 参考: `SdoUtils.GetCPUId()` — Linux 用 `DeviceIdBuilder.OnLinux(AddCpuInfo)`，
/// macOS 用 `DeviceIdBuilder.OnMac(AddPlatformSerialNumber)`
pub fn get_cpu_id_hash() -> String {
    if cfg!(target_os = "linux") {
        let cpu_info = get_linux_cpu_info().unwrap_or_default();
        md5_hex_upper(cpu_info.as_bytes())
    } else if cfg!(target_os = "macos") {
        let serial = get_macos_platform_serial().unwrap_or_default();
        md5_hex_upper(serial.as_bytes())
    } else {
        let fallback = get_fallback_machine_id();
        md5_hex_upper(fallback.as_bytes())
    }
}

/// 采集系统盘序列号的 MD5（大写 hex），用于 `device_id` 的第三段。
///
/// - Linux: `lsblk --nodeps -no SERIAL /dev/sda` 输出
/// - macOS: `diskutil info disk0` 的 Serial Number
///
/// C# 参考: `SdoUtils.GetDiskSerialNumber()` — Linux 用
/// `DeviceIdBuilder.OnLinux(AddSystemDriveSerialNumber)`，
/// macOS 用 `DeviceIdBuilder.OnMac(AddSystemDriveSerialNumber)`
pub fn get_disk_serial_hash() -> String {
    let serial = if cfg!(target_os = "linux") {
        get_linux_disk_serial().unwrap_or_default()
    } else if cfg!(target_os = "macos") {
        get_macos_disk_serial().unwrap_or_default()
    } else {
        get_fallback_machine_id()
    };
    md5_hex_upper(serial.as_bytes())
}

/// 采集完整设备 ID，格式为 `{mac_addr_hash}:{cpu_id_hash}:{disk_serial_hash}`。
///
/// C# 参考: `SdoUtils.GetDeviceId()` — `string.Join(":", GetMacAddress(), GetCPUId(), GetDiskSerialNumber())`
pub fn get_device_id() -> String {
    format!("{}:{}:{}", get_mac_address_hash(), get_cpu_id_hash(), get_disk_serial_hash())
}

// --- Linux ---

fn get_linux_machine_id() -> String {
    std::fs::read_to_string("/etc/machine-id")
        .unwrap_or_else(|_| get_fallback_machine_id())
        .trim()
        .to_string()
}

fn get_linux_cpu_info() -> std::io::Result<String> {
    let content = std::fs::read_to_string("/proc/cpuinfo")?;
    let mut model_names: Vec<String> = content
        .lines()
        .filter(|line| line.starts_with("model name"))
        .filter_map(|line| line.split(':').nth(1))
        .map(|s| s.trim().to_string())
        .collect();
    model_names.sort();
    model_names.dedup();
    Ok(model_names.join(","))
}

fn get_linux_disk_serial() -> std::io::Result<String> {
    let dev = get_root_device().unwrap_or_else(|| "/dev/sda".to_string());
    // 注意：`-o SERIAL`（`--no` 是歧义参数）
    let output = Command::new("lsblk")
        .args(["--nodeps", "-o", "SERIAL", &dev])
        .output()?;
    if output.status.success() {
        let serial = String::from_utf8_lossy(&output.stdout).trim().to_string();
        if !serial.is_empty() {
            return Ok(serial);
        }
    }
    // Fallback: try udev on the same device
    let output = Command::new("udevadm")
        .args(["info", "--query=property", "--name", &dev])
        .output()?;
    let content = String::from_utf8_lossy(&output.stdout);
    for line in content.lines() {
        if line.starts_with("ID_SERIAL=") {
            if let Some(serial) = line.split('=').nth(1) {
                return Ok(serial.to_string());
            }
        }
    }
    Err(std::io::Error::new(std::io::ErrorKind::NotFound, "disk serial not found"))
}

/// 识别根分区所在设备（去掉分区号与 btrfs 子卷后缀）：
/// `/dev/nvme2n1p4` → `/dev/nvme2n1`，`/dev/sda3[/@root]` → `/dev/sda`。
fn get_root_device() -> Option<String> {
    let output = Command::new("findmnt").args(["-n", "-o", "SOURCE", "/"]).output().ok()?;
    if !output.status.success() {
        return None;
    }
    let src = String::from_utf8_lossy(&output.stdout).trim().to_string();
    // btrfs 子卷格式：`/dev/sda3[/@root]` → `/dev/sda3`
    let dev_part = src.split('[').next()?.trim();
    if let Some(dev) = dev_part.strip_prefix("/dev/") {
        // 去尾部数字（分区号），再处理 nvme 的 `p` 分隔符
        let base = dev.trim_end_matches(char::is_numeric).trim_end_matches('p');
        if !base.is_empty() {
            return Some(format!("/dev/{base}"));
        }
    }
    None
}

// --- macOS ---

fn get_macos_platform_serial() -> std::io::Result<String> {
    let output = Command::new("ioreg")
        .args(["-rd1", "-c", "IOPlatformExpertDevice"])
        .output()?;
    let content = String::from_utf8_lossy(&output.stdout);
    for line in content.lines() {
        if line.contains("\"IOPlatformSerialNumber\"") {
            if let Some(serial) = line.split('=').nth(1) {
                return Ok(serial.trim().trim_matches('"').to_string());
            }
        }
    }
    Err(std::io::Error::new(std::io::ErrorKind::NotFound, "IOPlatformSerialNumber not found"))
}

fn get_macos_disk_serial() -> std::io::Result<String> {
    let output = Command::new("diskutil")
        .args(["info", "disk0"])
        .output()?;
    let content = String::from_utf8_lossy(&output.stdout);
    for line in content.lines() {
        let line = line.trim();
        if line.starts_with("Serial Number:") || line.starts_with("Volume UUID:") {
            if let Some(value) = line.split(':').nth(1) {
                let value = value.trim().to_string();
                if !value.is_empty() {
                    return Ok(value);
                }
            }
        }
    }
    Err(std::io::Error::new(std::io::ErrorKind::NotFound, "disk serial not found"))
}

// --- Fallback ---

fn get_fallback_machine_id() -> String {
    let hostname = std::env::var("HOSTNAME")
        .or_else(|_| std::env::var("HOST"))
        .unwrap_or_else(|_| "unknown".to_string());
    let username = std::env::var("USER")
        .or_else(|_| std::env::var("USERNAME"))
        .unwrap_or_else(|_| "unknown".to_string());
    format!("{}-{}", hostname, username)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_device_id_format() {
        let id = get_device_id();
        let parts: Vec<&str> = id.split(':').collect();
        assert_eq!(parts.len(), 3, "device_id should have 3 colon-separated parts");
        for part in &parts {
            assert_eq!(part.len(), 32, "each MD5 hash should be 32 hex chars, got: {}", part);
            assert!(part.chars().all(|c| c.is_ascii_hexdigit() && c.is_ascii_uppercase() || c.is_ascii_digit()),
                "each part should be uppercase hex: {}", part);
        }
    }

    #[test]
    fn test_mac_address_hash_is_uppercase() {
        let hash = get_mac_address_hash();
        assert_eq!(hash, hash.to_uppercase());
        assert_eq!(hash.len(), 32);
    }
}