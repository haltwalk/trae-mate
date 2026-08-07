// 多开实例启动编排:把账号凭据加密写入独立 data-dir,启动免登录的 TRAE 实例。
// 流程移植自 Account Manager 的 launch_product_multi(machine.rs:969-1115),适配签到工具的数据结构。

use std::fs;
use std::path::PathBuf;

use serde_json::{json, Value};

use crate::error::{AppError, AppResult};
use crate::models::{Account, Credential, LaunchResult};
use crate::trae_auth::encrypt_trae_auth_info;
use crate::trae_machine::{
    generate_machine_guid, open_product_with_data_dir, telemetry_machine_id,
    write_window_title_to_dir, DATA_DIR_NAME,
};

/// 默认 token 有效期(兜底,当凭据无有效过期时间时)
const DEFAULT_TOKEN_DAYS: i64 = 14;
const DEFAULT_REFRESH_DAYS: i64 = 180;
/// 多开实例的共享插件目录名(--extensions-dir,省磁盘)
pub const SHARED_EXTENSIONS_DIR: &str = "TRAE SOLO CN_SharedExtensions";

/// 为账号启动多开实例:加密写 storage.json + 启动独立 data-dir 的 TRAE。
/// 返回 data_dir/machine_id(供回写 Account,下次复用同一目录与机器码)。
pub fn launch_multi(account: &Account, cred: &Credential, exe_path: &str) -> AppResult<LaunchResult> {
    let appdata = std::env::var("APPDATA")
        .map_err(|_| AppError::Launch("无法获取 APPDATA 环境变量".into()))?;
    let user_id = if cred.user_id.is_empty() {
        account.desktop_user_id.clone().unwrap_or_default()
    } else {
        cred.user_id.clone()
    };
    if user_id.is_empty() {
        return Err(AppError::Launch("账号缺少 userId,无法创建实例".into()));
    }

    // 1. data-dir:优先复用 account 已绑定的,否则按 userId 生成
    let data_dir = account
        .data_dir
        .clone()
        .filter(|d| !d.is_empty())
        .unwrap_or_else(|| {
            PathBuf::from(&appdata)
                .join(format!("{DATA_DIR_NAME}_{user_id}"))
                .to_string_lossy()
                .to_string()
        });

    // 2. 机器码:优先复用,否则生成新 UUID(每实例独立,不动系统注册表)
    let machine_id = account
        .machine_id
        .clone()
        .filter(|m| !m.is_empty())
        .unwrap_or_else(generate_machine_guid);

    let data_path = PathBuf::from(&data_dir);
    fs::create_dir_all(&data_path)
        .map_err(|e| AppError::Launch(format!("创建实例数据目录失败: {e}")))?;
    fs::write(data_path.join("machineid"), &machine_id)
        .map_err(|e| AppError::Launch(format!("写入机器码失败: {e}")))?;

    // 3. 构建 iCubeAuthInfo 明文
    let now = chrono::Utc::now();
    let expired_at = iso_from_millis(
        cred.expires_at,
        now + chrono::Duration::days(DEFAULT_TOKEN_DAYS),
    );
    let refresh_expired_at = iso_from_millis(
        cred.refresh_expires_at,
        now + chrono::Duration::days(DEFAULT_REFRESH_DAYS),
    );
    let now_iso = now.format("%Y-%m-%dT%H:%M:%S%.3fZ").to_string();
    let region = cred.region.clone().unwrap_or_else(|| "CN".into());
    let host = if cred.host.is_empty() {
        "https://api.trae.cn".to_string()
    } else {
        cred.host.clone()
    };

    let auth_info = json!({
        "token": cred.token,
        "refreshToken": cred.refresh_token,
        "expiredAt": expired_at,
        "refreshExpiredAt": refresh_expired_at,
        "tokenReleaseAt": now_iso,
        "userId": cred.user_id,
        "host": host,
        "userRegion": {
            "region": region,
            "_aiRegion": region
        },
        "account": {
            "username": cred.account_name,
            "iss": "",
            "iat": 0,
            "organization": "",
            "work_country": "",
            "email": cred.email.clone().unwrap_or_default(),
            "avatar_url": cred.avatar_url.clone().unwrap_or_default(),
            "description": "",
            "scope": "marscode",
            "loginScope": "trae",
            "storeCountryCode": "cn",
            "storeCountrySrc": "uid",
            "storeRegion": region,
            "userTag": "row"
        }
    });

    // 4. entitlement(Free 固定模板,照搬 Account Manager)
    let entitlement_info = json!({
        "identityStr": "Free",
        "identity": 0,
        "isPayFreshman": false,
        "isSupportCommercialization": true,
        "hasPackage": false,
        "enableEntitlement": true,
        "detail": {
            "can_gen_solo_code": false,
            "fast_request_per": 1,
            "in_wait": false,
            "permission": 1,
            "toast_read": false,
            "toastRead": false,
            "canGenSoloCode": false,
            "fastRequestPer": 1,
            "inWaitlist": false
        }
    });

    // 5. 加密写 storage.json
    let storage_dir = data_path.join("User").join("globalStorage");
    fs::create_dir_all(&storage_dir)
        .map_err(|e| AppError::Launch(format!("创建 storage 目录失败: {e}")))?;
    let storage_path = storage_dir.join("storage.json");

    let mut json: Value = if storage_path.exists() {
        let content = fs::read_to_string(&storage_path)
            .map_err(|e| AppError::Launch(format!("读取 storage.json 失败: {e}")))?;
        serde_json::from_str(&content).unwrap_or(json!({}))
    } else {
        json!({})
    };
    let obj = json
        .as_object_mut()
        .ok_or_else(|| AppError::Launch("storage.json 格式错误".into()))?;

    // 移除旧登录信息
    obj.remove("iCubeAuthInfo://icube.cloudide");
    obj.remove("iCubeEntitlementInfo://icube.cloudide");
    obj.remove("iCubeServerData://icube.cloudide");
    obj.remove("iCubeAuthInfo://usertag");

    // 更新 telemetry(machineId 派生自 machineid,sqmId/devDeviceId 新随机 UUID)
    obj.insert(
        "telemetry.machineId".into(),
        Value::String(telemetry_machine_id(&machine_id)),
    );
    obj.insert(
        "telemetry.sqmId".into(),
        Value::String(format!("{{{}}}", uuid::Uuid::new_v4().to_string().to_uppercase())),
    );
    obj.insert(
        "telemetry.devDeviceId".into(),
        Value::String(uuid::Uuid::new_v4().to_string()),
    );

    // 加密写 iCubeAuthInfo(entitlement 明文不加密)
    let auth_plain = serde_json::to_string(&auth_info)?;
    let auth_encrypted = encrypt_trae_auth_info(&auth_plain)?;
    obj.insert(
        "iCubeAuthInfo://icube.cloudide".into(),
        Value::String(auth_encrypted),
    );
    obj.insert(
        "iCubeEntitlementInfo://icube.cloudide".into(),
        Value::String(serde_json::to_string(&entitlement_info).unwrap_or_default()),
    );

    fs::write(&storage_path, serde_json::to_string_pretty(&json)?)
        .map_err(|e| AppError::Launch(format!("写入 storage.json 失败: {e}")))?;

    // 6. 窗口标题(账号名,比 --title CLI 参数可靠)
    let _ = write_window_title_to_dir(&data_dir, &account.name);

    // 7. 启动(共享插件目录省磁盘)
    let shared_ext = PathBuf::from(&appdata)
        .join(SHARED_EXTENSIONS_DIR)
        .to_string_lossy()
        .to_string();
    open_product_with_data_dir(exe_path, &data_dir, Some(&shared_ext))?;

    Ok(LaunchResult {
        data_dir,
        machine_id,
        launched: true,
    })
}

/// 毫秒时间戳转 ISO 字符串;无效则用 fallback
fn iso_from_millis(ms: i64, fallback: chrono::DateTime<chrono::Utc>) -> String {
    if ms > 0 {
        if let Some(dt) = chrono::DateTime::from_timestamp_millis(ms) {
            return dt.format("%Y-%m-%dT%H:%M:%S%.3fZ").to_string();
        }
    }
    fallback.format("%Y-%m-%dT%H:%M:%S%.3fZ").to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::trae_auth::{decrypt_trae_auth_info, get_trae_desktop_credentials};

    /// 端到端:用真实桌面凭据调 launch_multi(exe 传不存在路径,避免真启动 TRAE),
    /// 验证 storage.json 已加密写入,且解密回读 token/userId 一致;telemetry 已更新。
    /// 用临时 data-dir,不污染真实多开目录。
    #[test]
    fn launch_multi_writes_encrypted_storage() {
        let cred = match get_trae_desktop_credentials() {
            Ok(c) => c,
            Err(e) => {
                eprintln!("[multi] 无桌面凭据(可能未登录桌面客户端),跳过: {e}");
                return;
            }
        };
        let tmp_dir = std::env::temp_dir().join(format!("trae-multi-test-{}", cred.user_id));
        let _ = std::fs::remove_dir_all(&tmp_dir); // 清理旧残留
        let account = Account {
            id: "multi-test".into(),
            name: cred.account_name.clone(),
            cookie: String::new(),
            created_at: 0,
            last_checkin_at: None,
            last_checkin_result: None,
            last_checkin_message: None,
            points: None,
            enabled: true,
            desktop_user_id: Some(cred.user_id.clone()),
            encrypted_credential: None,
            credential_status: None,
            data_dir: Some(tmp_dir.to_string_lossy().to_string()),
            machine_id: None,
        };

        // exe 不存在 -> 启动失败,但 storage.json 应已写入(步骤在启动前)
        let result = launch_multi(&account, &cred, "Z:/nonexistent/trae.exe");
        assert!(result.is_err(), "不存在 exe 应启动失败,实际: {result:?}");

        let storage_path = tmp_dir.join("User").join("globalStorage").join("storage.json");
        let content = std::fs::read_to_string(&storage_path).expect("storage.json 应已生成");
        let json: Value = serde_json::from_str(&content).expect("storage.json 应为合法 JSON");

        // iCubeAuthInfo 加密,解密回读应与原凭据一致
        let encrypted = json
            .get("iCubeAuthInfo://icube.cloudide")
            .and_then(|v| v.as_str())
            .expect("应有加密的 iCubeAuthInfo");
        let auth = decrypt_trae_auth_info(encrypted).expect("应能用现有 decrypt 解密");
        assert_eq!(auth.get("token").and_then(|v| v.as_str()), Some(cred.token.as_str()));
        assert_eq!(auth.get("userId").and_then(|v| v.as_str()), Some(cred.user_id.as_str()));
        assert!(auth.get("account").is_some(), "应含 account 对象");
        assert!(auth.get("userRegion").is_some(), "应含 userRegion");

        // entitlement 明文写入
        assert!(json.get("iCubeEntitlementInfo://icube.cloudide").is_some());
        // telemetry 已更新
        assert!(json.get("telemetry.machineId").is_some());
        assert!(json.get("telemetry.devDeviceId").is_some());
        // 旧登录字段已移除
        assert!(json.get("iCubeServerData://icube.cloudide").is_none());

        // machineid 文件已写
        assert!(tmp_dir.join("machineid").exists());

        // 清理临时目录
        let _ = std::fs::remove_dir_all(&tmp_dir);
        eprintln!("[multi] storage.json 加密写入 + 解密回读验证通过");
    }
}
