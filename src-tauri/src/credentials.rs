// 凭据加密存储(DPAPI)。token 的刷新由 TRAE 客户端负责(运行时会自行写回 storage.json),
// 本应用仅通过 checkin/trae_instance 回读,不主动调 ExchangeToken。

use base64::{engine::general_purpose, Engine as _};

use crate::error::{AppError, AppResult};
use crate::models::Credential;

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
