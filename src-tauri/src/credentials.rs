// 凭据加密存储(DPAPI)与刷新(RSA-SHA256 签名)。移植自 desktop-credentials.cjs。

use base64::{engine::general_purpose, Engine as _};
use rand::RngCore;
use rsa::pkcs1::DecodeRsaPrivateKey;
use rsa::pkcs8::DecodePrivateKey;
use rsa::pkcs1v15::SigningKey;
use rsa::signature::{Signer, SignatureEncoding};
use rsa::RsaPrivateKey;
use serde_json::{json, Value};
use sha2::Sha256;

use crate::error::{AppError, AppResult};
use crate::models::Credential;

const CLIENT_ID: &str = "en1oxy7wnw8j9n";
const REFRESH_PATH: &str = "/trae/api/v3/oauth/ExchangeToken";

// ===== DPAPI(Windows) =====
// NOTE: windows 0.61 未导出 CRYPTPROTECT_FLAGS/LocalFree/HLOCAL(实测 dwflags 为 u32)。
// DPAPI 输出缓冲由 LocalAlloc 分配,理论上应 LocalFree 释放;此处暂不释放
// (每次签到/导入少量字节泄漏,后续确认 windows 0.61 的释放 API 后补上)。

#[cfg(windows)]
fn dpapi_protect(data: &[u8]) -> AppResult<Vec<u8>> {
    use windows::core::PCWSTR;
    use windows::Win32::Security::Cryptography::{CryptProtectData, CRYPT_INTEGER_BLOB};

    unsafe {
        let in_blob = CRYPT_INTEGER_BLOB {
            cbData: data.len() as u32,
            pbData: data.as_ptr() as *mut u8,
        };
        let mut out_blob = CRYPT_INTEGER_BLOB {
            cbData: 0,
            pbData: std::ptr::null_mut(),
        };
        CryptProtectData(&in_blob, PCWSTR::null(), None, None, None, 0, &mut out_blob)
            .map_err(|e| AppError::Dpapi(e.to_string()))?;
        let vec = std::slice::from_raw_parts(out_blob.pbData, out_blob.cbData as usize).to_vec();
        Ok(vec)
    }
}

#[cfg(windows)]
fn dpapi_unprotect(data: &[u8]) -> AppResult<Vec<u8>> {
    use windows::Win32::Security::Cryptography::{CryptUnprotectData, CRYPT_INTEGER_BLOB};

    unsafe {
        let in_blob = CRYPT_INTEGER_BLOB {
            cbData: data.len() as u32,
            pbData: data.as_ptr() as *mut u8,
        };
        let mut out_blob = CRYPT_INTEGER_BLOB {
            cbData: 0,
            pbData: std::ptr::null_mut(),
        };
        CryptUnprotectData(&in_blob, None, None, None, None, 0, &mut out_blob)
            .map_err(|e| AppError::Dpapi(e.to_string()))?;
        let vec = std::slice::from_raw_parts(out_blob.pbData, out_blob.cbData as usize).to_vec();
        Ok(vec)
    }
}

#[cfg(not(windows))]
fn dpapi_protect(_: &[u8]) -> AppResult<Vec<u8>> {
    Err(AppError::Dpapi("DPAPI 仅 Windows 可用".into()))
}
#[cfg(not(windows))]
fn dpapi_unprotect(_: &[u8]) -> AppResult<Vec<u8>> {
    Err(AppError::Dpapi("DPAPI 仅 Windows 可用".into()))
}

// ===== 加密存储 =====

pub fn encrypt_credential(cred: &Credential) -> AppResult<String> {
    let json = serde_json::to_string(cred)?;
    let bytes = dpapi_protect(json.as_bytes())?;
    Ok(general_purpose::STANDARD.encode(&bytes))
}

pub fn decrypt_credential(encoded: &str) -> AppResult<Credential> {
    let bytes = general_purpose::STANDARD
        .decode(encoded)
        .map_err(|e| AppError::Credential(format!("base64 解码失败: {e}")))?;
    let plain = dpapi_unprotect(&bytes)?;
    let cred: Credential = serde_json::from_slice(&plain)?;
    Ok(cred)
}

// ===== 刷新 =====

fn load_private_key(pem: &str) -> AppResult<RsaPrivateKey> {
    RsaPrivateKey::from_pkcs8_pem(pem)
        .or_else(|_| RsaPrivateKey::from_pkcs1_pem(pem))
        .map_err(|e| AppError::Credential(format!("加载 RSA 私钥失败: {e}")))
}

/// 用 RSA-SHA256(PKCS#1 v1.5) 签名,对应原 crypto.sign('sha256', payload, privateKey)
fn sign_sha256(private_key: RsaPrivateKey, payload: &[u8]) -> AppResult<Vec<u8>> {
    let signing_key = SigningKey::<Sha256>::new(private_key);
    let signature = signing_key.sign(payload);
    Ok(signature.to_bytes().to_vec())
}

pub async fn refresh_credential(
    cred: &Credential,
    client: &reqwest::Client,
) -> AppResult<Credential> {
    let timestamp = chrono::Utc::now().timestamp();
    let mut nonce_bytes = [0u8; 16];
    rand::thread_rng().fill_bytes(&mut nonce_bytes);
    let nonce = hex::encode(nonce_bytes);

    // 签名载荷:POST\n{path}\n{clientId}\n{refreshToken}\n{timestamp}\n{nonce}
    let signature_payload = format!(
        "POST\n{}\n{}\n{}\n{}\n{}",
        REFRESH_PATH, CLIENT_ID, cred.refresh_token, timestamp, nonce
    );

    let private_key = load_private_key(&cred.private_key_pem)?;
    let signature = sign_sha256(private_key, signature_payload.as_bytes())?;
    let signature_b64 = general_purpose::STANDARD.encode(&signature);

    let device_name = std::env::var("USERNAME").unwrap_or_else(|_| "trae-check".into());
    let body = json!({
        "ClientID": CLIENT_ID,
        "ClientSecret": "",
        "RefreshToken": cred.refresh_token,
        "DeviceInfo": {
            "DeviceID": cred.device_id,
            "MachineID": cred.machine_id,
            "PlatformCode": "SOLO_PC",
            "DeviceType": "PC",
            "DeviceName": device_name,
            "DeviceModel": "",
            "ClientVersion": "0.1.43",
            "DevicePublicKey": cred.public_key_pem,
            "DeviceBrand": "",
            "DeviceCPU": "",
            "OSInfo": "Windows_NT",
            "OSVersion": "10.0"
        },
        "DeviceProof": {
            "Signature": signature_b64,
            "Timestamp": timestamp,
            "Nonce": nonce
        },
        "IDEVersion": "0.1.43"
    });

    let url = format!("{}{}", cred.host, REFRESH_PATH);
    let resp = client
        .post(&url)
        .header("Content-Type", "application/json")
        .header("x-cloudide-token", &cred.token)
        .json(&body)
        .send()
        .await
        .map_err(|e| AppError::Http(e.to_string()))?;

    let data: Value = resp
        .json()
        .await
        .map_err(|e| AppError::Http(format!("解析刷新响应失败: {e}")))?;

    let result = data
        .get("Result")
        .ok_or_else(|| AppError::Credential("刷新响应缺少 Result".into()))?;
    let token = result
        .get("Token")
        .and_then(|v| v.as_str())
        .ok_or_else(|| AppError::Credential("刷新响应缺少 Token".into()))?;
    let refresh_token = result
        .get("RefreshToken")
        .and_then(|v| v.as_str())
        .ok_or_else(|| AppError::Credential("刷新响应缺少 RefreshToken".into()))?;
    let expires_at = parse_expiry(result.get("TokenExpireAt"))?;
    let refresh_expires_at = parse_expiry(result.get("RefreshExpireAt"))?;

    Ok(Credential {
        token: token.to_string(),
        refresh_token: refresh_token.to_string(),
        expires_at,
        refresh_expires_at,
        ..cred.clone()
    })
}

/// 解析过期时间:数字(毫秒)或 ISO 字符串
fn parse_expiry(v: Option<&Value>) -> AppResult<i64> {
    let v = v.ok_or_else(|| AppError::Credential("缺少过期时间".into()))?;
    if let Some(n) = v.as_i64() {
        return Ok(n);
    }
    if let Some(f) = v.as_f64() {
        return Ok(f as i64);
    }
    if let Some(s) = v.as_str() {
        if let Ok(n) = s.parse::<i64>() {
            return Ok(n);
        }
        if let Ok(dt) = chrono::DateTime::parse_from_rfc3339(s) {
            return Ok(dt.timestamp_millis());
        }
        for fmt in &[
            "%Y-%m-%dT%H:%M:%SZ",
            "%Y-%m-%dT%H:%M:%S%.3fZ",
            "%Y-%m-%dT%H:%M:%S",
        ] {
            if let Ok(ndt) = chrono::NaiveDateTime::parse_from_str(s, fmt) {
                return Ok(ndt.and_utc().timestamp_millis());
            }
        }
    }
    Err(AppError::Credential(format!("无效的过期时间: {v}")))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dpapi_roundtrip() {
        let plain = b"hello trae-check credential 12345";
        let protected = match dpapi_protect(plain) {
            Ok(p) => p,
            Err(e) => {
                eprintln!("[dpapi] 非 Windows 或不可用: {e}");
                return;
            }
        };
        let recovered = dpapi_unprotect(&protected).expect("DPAPI 解密失败");
        assert_eq!(recovered.as_slice(), plain);
        eprintln!("[dpapi] 往返成功,密文 {} 字节", protected.len());
    }

    #[test]
    fn credential_encrypt_decrypt_roundtrip() {
        let cred = Credential {
            token: "tok".into(),
            refresh_token: "rt".into(),
            expires_at: 9_999_999_999_999,
            refresh_expires_at: 9_999_999_999_999,
            device_id: "dev".into(),
            machine_id: "mach".into(),
            private_key_pem: "pk".into(),
            public_key_pem: "pub".into(),
            user_id: "u1".into(),
            account_name: "test".into(),
            host: "https://example.com".into(),
            email: None,
            avatar_url: None,
            region: None,
        };
        let encrypted = match encrypt_credential(&cred) {
            Ok(e) => e,
            Err(e) => {
                eprintln!("[cred] 加密不可用: {e}");
                return;
            }
        };
        let decrypted = decrypt_credential(&encrypted).expect("解密失败");
        assert_eq!(decrypted.token, "tok");
        assert_eq!(decrypted.user_id, "u1");
        assert_eq!(decrypted.host, "https://example.com");
    }
}
