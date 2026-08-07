// 读取并解密 TRAE 桌面客户端凭据。忠实移植自 electron/trae-auth.cjs。
//
// 算法:
// 1. 读取 %APPDATA%/TRAE SOLO CN/User/globalStorage/storage.json
// 2. 取 iCubeAuthInfo://icube.cloudide(base64 信封)
// 3. 信封 = HEADER(6) + randomKey(32) + 密文
//    secret = LEFT_SECRET ⊕ RIGHT_SECRET(64)
//    derived = SHA512( SHA512(randomKey) ++ secret )(64)
//    AES-128-CBC 解密: key=derived[0..16], iv=derived[16..32]
//    明文 = digest(64) + payload(JSON);校验 digest == SHA512(payload)

use std::env;
use std::path::{Path, PathBuf};

use aes::cipher::{block_padding::Pkcs7, BlockDecryptMut, KeyIvInit};
use base64::{engine::general_purpose, Engine as _};
use serde_json::Value;
use sha2::{Digest, Sha512};

use crate::error::{AppError, AppResult};
use crate::models::Credential;

/// 信封头(6 字节)
const HEADER: [u8; 6] = [116, 99, 5, 16, 0, 0];

/// 左密钥(64 字节)
const LEFT_SECRET: [u8; 64] = [
    82, 9, 106, 213, 48, 54, 165, 56, 191, 64, 163, 158, 129, 243, 215, 251, 124, 227, 57, 130,
    155, 47, 255, 135, 52, 142, 67, 68, 196, 222, 233, 203, 84, 123, 148, 50, 166, 194, 35, 61,
    238, 76, 149, 11, 66, 250, 195, 78, 8, 46, 161, 102, 40, 217, 36, 178, 118, 91, 162, 73, 109,
    139, 209, 37,
];

/// 右密钥(64 字节)
const RIGHT_SECRET: [u8; 64] = [
    31, 221, 168, 51, 136, 7, 199, 49, 177, 18, 16, 89, 39, 128, 236, 95, 96, 81, 127, 169, 25,
    181, 74, 13, 45, 229, 122, 159, 147, 201, 156, 239, 160, 224, 59, 77, 174, 42, 245, 176, 200,
    235, 187, 60, 131, 83, 153, 97, 23, 43, 4, 126, 186, 119, 214, 38, 225, 105, 20, 99, 85, 33,
    12, 125,
];

type Aes128CbcDec = cbc::Decryptor<aes::Aes128>;

fn sha512(data: &[u8]) -> [u8; 64] {
    let mut h = Sha512::new();
    h.update(data);
    let out = h.finalize();
    let mut arr = [0u8; 64];
    arr.copy_from_slice(out.as_slice());
    arr
}

/// 解密 TRAE 桌面客户端的加密信封(base64),返回 payload JSON
pub fn decrypt_trae_auth_info(encoded: &str) -> AppResult<Value> {
    let envelope = general_purpose::STANDARD
        .decode(encoded)
        .map_err(|e| AppError::Credential(format!("base64 decode failed: {e}")))?;

    if envelope.len() <= 38 || envelope.get(0..6) != Some(&HEADER) {
        return Err(AppError::Credential(
            "Invalid TRAE desktop credential envelope".into(),
        ));
    }

    let random_key = &envelope[6..38];

    let mut secret = [0u8; 64];
    for i in 0..64 {
        secret[i] = LEFT_SECRET[i] ^ RIGHT_SECRET[i];
    }

    let mut derived_input = Vec::with_capacity(128);
    derived_input.extend_from_slice(&sha512(random_key));
    derived_input.extend_from_slice(&secret);
    let derived = sha512(&derived_input);

    let key = &derived[0..16];
    let iv = &derived[16..32];

    let mut buf = envelope[38..].to_vec();
    let plaintext = Aes128CbcDec::new(key.into(), iv.into())
        .decrypt_padded_mut::<Pkcs7>(&mut buf)
        .map_err(|e| AppError::Credential(format!("AES decrypt failed: {e}")))?;

    if plaintext.len() < 64 {
        return Err(AppError::Credential(
            "decrypted payload too short".into(),
        ));
    }
    let expected_digest = &plaintext[0..64];
    let payload = &plaintext[64..];
    let actual_digest = sha512(payload);
    if expected_digest != actual_digest.as_slice() {
        return Err(AppError::Credential(
            "TRAE desktop credential integrity check failed".into(),
        ));
    }

    let json: Value = serde_json::from_slice(payload)?;
    Ok(json)
}

/// 加密 TRAE 桌面客户端的加密信封(与 decrypt_trae_auth_info 互为逆运算)。
/// 用于多开实例:把账号登录信息加密写入独立 data-dir 的 storage.json。
/// 信封 = HEADER(6) + 随机 embedded_key(32) + AES-128-CBC( HMAC_SHA512(64) ‖ plaintext+PKCS7 ),Base64。
pub fn encrypt_trae_auth_info(plaintext: &str) -> AppResult<String> {
    use aes::cipher::{BlockEncryptMut, KeyIvInit};
    use rand::RngCore;

    // 1. 随机 32 字节 embedded_key(对应 decrypt 的 random_key)
    let mut embedded_key = [0u8; 32];
    rand::thread_rng().fill_bytes(&mut embedded_key);

    // 2. secret = LEFT_SECRET ⊕ RIGHT_SECRET(与 decrypt 一致)
    let mut secret = [0u8; 64];
    for i in 0..64 {
        secret[i] = LEFT_SECRET[i] ^ RIGHT_SECRET[i];
    }

    // 3. 派生 AES-128 key/iv:SHA512( SHA512(embedded_key) ‖ secret )
    let mut derived_input = Vec::with_capacity(128);
    derived_input.extend_from_slice(&sha512(&embedded_key));
    derived_input.extend_from_slice(&secret);
    let derived = sha512(&derived_input);
    let key: [u8; 16] = derived[0..16].try_into().unwrap();
    let iv: [u8; 16] = derived[16..32].try_into().unwrap();

    // 4. HMAC = SHA512(plaintext),前置拼接
    let plaintext_bytes = plaintext.as_bytes();
    let hmac = sha512(plaintext_bytes);
    let mut data = Vec::with_capacity(64 + plaintext_bytes.len());
    data.extend_from_slice(&hmac);
    data.extend_from_slice(plaintext_bytes);

    // 5. PKCS7 填充到 16 字节倍数
    let pad_len = 16 - (data.len() % 16);
    let pad_byte = pad_len as u8;
    for _ in 0..pad_len {
        data.push(pad_byte);
    }

    // 6. AES-128-CBC 加密(逐块,encrypt_block_b2b_mut 内部维护 CBC 链)
    let mut ciphertext = vec![0u8; data.len()];
    let mut encryptor = cbc::Encryptor::<aes::Aes128>::new(&key.into(), &iv.into());
    for (chunk, out_chunk) in data.chunks(16).zip(ciphertext.chunks_mut(16)) {
        let mut block = aes::Block::default();
        block.copy_from_slice(chunk);
        let mut out_block = aes::Block::default();
        encryptor.encrypt_block_b2b_mut(&block, &mut out_block);
        out_chunk.copy_from_slice(&out_block);
    }

    // 7. 拼装信封:HEADER(6) + embedded_key(32) + ciphertext,Base64
    let mut output = Vec::with_capacity(6 + 32 + ciphertext.len());
    output.extend_from_slice(&HEADER);
    output.extend_from_slice(&embedded_key);
    output.extend_from_slice(&ciphertext);
    Ok(general_purpose::STANDARD.encode(&output))
}

/// 读取并解密当前 TRAE 桌面客户端凭据(主目录 %APPDATA%\TRAE SOLO CN)
pub fn get_trae_desktop_credentials() -> AppResult<Credential> {
    let appdata = env::var("APPDATA")
        .map_err(|_| AppError::Credential("Windows APPDATA directory is unavailable".into()))?;
    let data_dir = PathBuf::from(&appdata).join("TRAE SOLO CN");
    read_credentials_from_data_dir(&data_dir)
}

/// 读取并解密指定 data-dir 的凭据(通用:主目录或独立实例目录均可)
pub fn read_credentials_from_data_dir(data_dir: &Path) -> AppResult<Credential> {
    let storage_path = data_dir
        .join("User")
        .join("globalStorage")
        .join("storage.json");
    let raw = std::fs::read_to_string(&storage_path)?;
    let storage: Value = serde_json::from_str(&raw)?;

    let encrypted = storage
        .get("iCubeAuthInfo://icube.cloudide")
        .and_then(|v| v.as_str())
        .ok_or_else(|| AppError::Credential("TRAE desktop login information was not found".into()))?;
    let auth_info = decrypt_trae_auth_info(encrypted)?;

    let token = auth_info
        .get("token")
        .and_then(|v| v.as_str())
        .filter(|s| !s.is_empty())
        .ok_or_else(|| AppError::Credential("TRAE desktop login token is invalid".into()))?;

    let device_id = storage
        .get("telemetry.devDeviceId")
        .and_then(|v| v.as_str())
        .filter(|s| !s.is_empty())
        .ok_or_else(|| AppError::Credential("TRAE desktop device ID is unavailable".into()))?;

    let machine_id = storage
        .get("telemetry.machineId")
        .and_then(|v| v.as_str())
        .filter(|s| !s.is_empty())
        .ok_or_else(|| AppError::Credential("TRAE desktop machine ID is unavailable".into()))?;

    // 签名密钥:iCubeAuthInfo://icube-dc:* (同样解密)
    let signing_key_encoded = storage
        .as_object()
        .and_then(|obj| {
            obj.iter().find_map(|(k, v)| {
                if k.starts_with("iCubeAuthInfo://icube-dc:") {
                    v.as_str()
                } else {
                    None
                }
            })
        })
        .ok_or_else(|| AppError::Credential("TRAE desktop signing key is unavailable".into()))?;
    let signing_key = decrypt_trae_auth_info(signing_key_encoded)?;
    let private_key_pem = signing_key
        .get("privateKeyPEM")
        .and_then(|v| v.as_str())
        .ok_or_else(|| AppError::Credential("TRAE desktop signing key is invalid".into()))?;
    let public_key_pem = signing_key
        .get("publicKeyPEM")
        .and_then(|v| v.as_str())
        .ok_or_else(|| AppError::Credential("TRAE desktop signing key is invalid".into()))?;

    let expires_at = parse_time_ms(auth_info.get("expiredAt"))?;
    let refresh_expires_at = parse_time_ms(auth_info.get("refreshExpiredAt"))?;

    let user_id = auth_info
        .get("userId")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();
    let account_name = auth_info
        .get("account")
        .and_then(|a| a.get("username"))
        .and_then(|v| v.as_str())
        .map(|s| s.to_string())
        .unwrap_or_else(|| format!("TRAE 用户 {}", user_id));
    let host = auth_info
        .get("host")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();
    let refresh_token = auth_info
        .get("refreshToken")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();

    // 多开写 storage.json 需要的账号资料(从 account 对象提取)
    let account_obj = auth_info.get("account");
    let email = account_obj
        .and_then(|a| a.get("email"))
        .and_then(|v| v.as_str())
        .map(|s| s.to_string());
    let avatar_url = account_obj
        .and_then(|a| a.get("avatar_url"))
        .and_then(|v| v.as_str())
        .map(|s| s.to_string());
    let region = auth_info
        .get("userRegion")
        .and_then(|r| r.get("region"))
        .and_then(|v| v.as_str())
        .map(|s| s.to_string())
        .or_else(|| {
            // 兜底:按 host 推断(中国版 api.trae.cn => CN,其余 => SG)
            if host.contains("api.trae.cn") {
                Some("CN".into())
            } else {
                Some("SG".into())
            }
        });

    Ok(Credential {
        token: token.to_string(),
        refresh_token,
        expires_at,
        refresh_expires_at,
        device_id: device_id.to_string(),
        machine_id: machine_id.to_string(),
        private_key_pem: private_key_pem.to_string(),
        public_key_pem: public_key_pem.to_string(),
        user_id,
        account_name,
        host,
        email,
        avatar_url,
        region,
    })
}

/// 读取主目录实例当前登录的 userId(读 %APPDATA%\TRAE SOLO CN\...\storage.json 解密 iCubeAuthInfo)。
/// 仅用于判断主实例登录账号:失败返回 None(主实例未登录/异常时不阻断状态判定)。
pub fn read_main_instance_user_id() -> Option<String> {
    let appdata = env::var("APPDATA").ok()?;
    let storage_path = PathBuf::from(&appdata)
        .join("TRAE SOLO CN")
        .join("User")
        .join("globalStorage")
        .join("storage.json");
    let raw = std::fs::read_to_string(&storage_path).ok()?;
    let storage: Value = serde_json::from_str(&raw).ok()?;
    let encrypted = storage
        .get("iCubeAuthInfo://icube.cloudide")
        .and_then(|v| v.as_str())?;
    let auth_info = decrypt_trae_auth_info(encrypted).ok()?;
    let uid = auth_info
        .get("userId")
        .and_then(|v| v.as_str())?
        .to_string();
    if uid.is_empty() {
        None
    } else {
        Some(uid)
    }
}

/// 解析时间字符串为毫秒时间戳(对应原 Date.parse)
fn parse_time_ms(v: Option<&Value>) -> AppResult<i64> {
    let s = v
        .and_then(|v| v.as_str())
        .ok_or_else(|| AppError::Credential("TRAE desktop credential expiry is invalid".into()))?;
    // 尝试 RFC3339(ISO8601 带 Z/偏移)
    if let Ok(dt) = chrono::DateTime::parse_from_rfc3339(s) {
        return Ok(dt.timestamp_millis());
    }
    // 尝试常见 ISO 格式
    for fmt in &["%Y-%m-%dT%H:%M:%SZ", "%Y-%m-%dT%H:%M:%S%.3fZ", "%Y-%m-%dT%H:%M:%S"] {
        if let Ok(ndt) = chrono::NaiveDateTime::parse_from_str(s, fmt) {
            return Ok(ndt.and_utc().timestamp_millis());
        }
    }
    Err(AppError::Credential(format!(
        "TRAE desktop credential expiry is invalid: {s}"
    )))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// encrypt -> decrypt 往返:加密结果必须能被现有解密链路还原(decrypt 会 JSON 解析 payload)
    #[test]
    fn encrypt_decrypt_roundtrip() {
        let cases: Vec<String> = vec![
            "{}".into(),
            "{\"a\":1}".into(),
            "{\"token\":\"abc\",\"userId\":\"u1\"}".into(),
            "{\"msg\":\"hello 多开实例\"}".into(),
            format!("{{\"data\":\"{}\"}}", "x".repeat(100)),
        ];
        for plain in &cases {
            let encrypted = encrypt_trae_auth_info(plain).expect("加密失败");
            let decrypted = decrypt_trae_auth_info(&encrypted).expect("解密失败");
            let expected: Value = serde_json::from_str(plain).unwrap();
            assert_eq!(decrypted, expected, "roundtrip 失败: {plain}");
        }
    }

    /// 同明文每次加密应产生不同密文(embedded_key 随机)
    #[test]
    fn encrypt_produces_different_ciphertext() {
        let a = encrypt_trae_auth_info("{}").unwrap();
        let b = encrypt_trae_auth_info("{}").unwrap();
        assert_ne!(a, b, "embedded_key 随机,同明文密文应不同");
    }

    /// 用本机真实 storage.json 验证解密链路。不输出任何敏感值,
    /// 仅断言能解出非空 token/userId/host(说明算法正确)。
    #[test]
    fn decrypt_real_trae_storage() {
        let appdata = match env::var("APPDATA") {
            Ok(v) => v,
            Err(_) => {
                eprintln!("[trae_auth] 非 Windows 环境,跳过");
                return;
            }
        };
        let path = PathBuf::from(&appdata)
            .join("TRAE SOLO CN")
            .join("User")
            .join("globalStorage")
            .join("storage.json");
        if !path.exists() {
            eprintln!("[trae_auth] 未找到 TRAE storage.json,跳过");
            return;
        }
        let cred = match get_trae_desktop_credentials() {
            Ok(c) => c,
            Err(e) => panic!("[trae_auth] 解密失败: {e}"),
        };
        assert!(!cred.token.is_empty(), "token 不应为空");
        assert!(!cred.user_id.is_empty(), "userId 不应为空");
        assert!(!cred.host.is_empty(), "host 不应为空");
        assert!(!cred.device_id.is_empty(), "deviceId 不应为空");
        assert!(!cred.private_key_pem.is_empty(), "privateKeyPEM 不应为空");
        assert!(
            cred.private_key_pem.contains("BEGIN"),
            "privateKeyPEM 应为 PEM 格式"
        );
        assert!(cred.expires_at > 0, "expiresAt 应为有效时间戳");
        // 不打印任何凭据值
        eprintln!("[trae_auth] 解密成功,accountName 长度={}, host 长度={}", cred.account_name.len(), cred.host.len());
    }
}
