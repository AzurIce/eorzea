use base64::Engine;
use md5::{Digest, Md5};
use sha1::Sha1;

pub fn sha1_hex_lower(data: &[u8]) -> String {
    let mut hasher = Sha1::new();
    hasher.update(data);
    hex::encode(hasher.finalize())
}

pub fn md5_hex_upper(data: &[u8]) -> String {
    let mut hasher = Md5::new();
    hasher.update(data);
    hex::encode_upper(hasher.finalize())
}

pub fn make_computer_id(
    machine_name: &str,
    user_name: &str,
    os_version: &str,
    processor_count: usize,
) -> String {
    let hash_string = format!(
        "{}{}{}{}",
        machine_name, user_name, os_version, processor_count
    );
    let hash = sha1_hex_lower(hash_string.as_bytes());
    let hash_bytes = hex::decode(&hash).expect("sha1 hex decode should not fail");

    let mut result = [0u8; 5];
    result[1] = hash_bytes[0];
    result[2] = hash_bytes[1];
    result[3] = hash_bytes[2];
    result[4] = hash_bytes[3];
    let checksum =
        (-(result[1] as i16 + result[2] as i16 + result[3] as i16 + result[4] as i16)) as u8;
    result[0] = checksum;

    hex::encode(result).to_lowercase()
}

pub fn to_mangled_se_base64(input: &[u8]) -> String {
    base64::engine::general_purpose::STANDARD
        .encode(input)
        .replace('+', "-")
        .replace('/', "_")
        .replace('=', "*")
}

const ARGUMENT_CHECKSUM_TABLE: [char; 16] = [
    'f', 'X', '1', 'p', 'G', 't', 'd', 'S', '5', 'C', 'A', 'P', '4', '_', 'V', 'L',
];

pub fn argument_checksum_char(key: u32) -> char {
    let index = (key & 0x000F0000) >> 16;
    ARGUMENT_CHECKSUM_TABLE[index as usize]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_computer_id_format() {
        let id = make_computer_id("MYPC", "user", "Windows 10", 8);
        assert_eq!(id.len(), 10); // 5 bytes = 10 hex chars
        assert!(id.chars().all(|c| c.is_ascii_hexdigit()));
    }
}
