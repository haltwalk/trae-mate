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

    // 3. 构建 iCubeAuthInfo 明文(抽为 build_auth_info,launch 写入与刷新回写共用)
    let auth_info = build_auth_info(cred);

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

    // 更新 telemetry(machineId 派生自 machineid;devDeviceId/sqmId 每个实例独立且持久,
    // 首次生成后复用)。绝不能注入主账号的设备 id——服务端按设备维度做限额,
    // 多个账号共用同一设备 id 领取会被判"操作太过频繁"(code 9074)。
    obj.insert(
        "telemetry.machineId".into(),
        Value::String(telemetry_machine_id(&machine_id)),
    );
    let sqm_id = obj
        .get("telemetry.sqmId")
        .and_then(|v| v.as_str())
        .filter(|s| !s.is_empty())
        .map(|s| s.to_string())
        .unwrap_or_else(|| format!("{{{}}}", uuid::Uuid::new_v4().to_string().to_uppercase()));
    obj.insert("telemetry.sqmId".into(), Value::String(sqm_id));
    // 工具实例设备 id:首次生成后持久复用;若已被污染为与主目录相同的设备 id
    // (早期版本误注入主账号设备 id 所致),强制重新生成独立 UUID。
    let main_dev = crate::trae_auth::read_main_telemetry_device_id();
    let dev_device_id = obj
        .get("telemetry.devDeviceId")
        .and_then(|v| v.as_str())
        .filter(|s| !s.is_empty())
        .map(|s| s.to_string())
        .filter(|s| main_dev.as_deref() != Some(s.as_str()))
        .unwrap_or_else(|| uuid::Uuid::new_v4().to_string());
    obj.insert("telemetry.devDeviceId".into(), Value::String(dev_device_id));

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

/// 由凭据构建 iCubeAuthInfo 明文 JSON(launch_multi 写入与刷新回写共用)。
/// 过期时间无效时兜底 now+14/180 天(与 Account Manager 行为一致)。
pub fn build_auth_info(cred: &Credential) -> Value {
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

    json!({
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
    })
}

/// 账号的候选实例目录:绑定了工具目录则只用工具目录(它是该账号的独立设备身份,
/// 回读主目录会把主账号的设备 id 带进来,多个账号共用同一设备领取会触发服务端
/// 设备维度限频"操作太过频繁" code 9074);仅纯主账号(无工具目录)才回读主目录。
fn candidate_dirs(account: &Account) -> Vec<String> {
    let mut dirs = Vec::new();
    if let Some(d) = account.data_dir.as_deref().filter(|s| !s.is_empty()) {
        dirs.push(d.to_string());
        return dirs;
    }
    // 主目录仅在账号 desktop_user_id 与主目录当前登录 userId 一致时纳入(避免读到别的账号)
    if let Some(uid) = account
        .desktop_user_id
        .as_deref()
        .filter(|s| !s.is_empty())
    {
        if crate::trae_auth::read_main_instance_user_id().as_deref() == Some(uid) {
            if let Ok(main_dir) = crate::trae_machine::main_data_dir() {
                dirs.push(main_dir.to_string_lossy().to_string());
            }
        }
    }
    dirs
}

/// 从实例目录回读登录信息:候选目录(本账号工具目录 / 主目录)可读且 userId 匹配时,
/// 目录凭据即为 TRAE 实际使用的 token+设备身份,优先采纳;过期时间相同时也以目录为准
/// (保证 x-device-id 用的是实例真实设备 id,而不是快照里主账号的设备 id)。
/// 返回 (最新凭据, 是否采纳了目录凭据)。读取失败静默跳过(目录不存在/未登录/解密失败不阻断)。
pub fn sync_credential_from_instance(account: &Account, cred: &Credential) -> (Credential, bool) {
    let mut best = cred.clone();
    let mut adopted = false;
    for dir in candidate_dirs(account) {
        let path = PathBuf::from(&dir);
        let Ok(read) = crate::trae_auth::read_auth_from_data_dir_loose(&path, &best) else {
            continue;
        };
        // 工具目录必须与本账号绑定一致;主目录已由 candidate_dirs 保证 userId 匹配
        if let Some(uid) = account
            .desktop_user_id
            .as_deref()
            .filter(|s| !s.is_empty())
        {
            if !read.user_id.is_empty() && read.user_id != uid {
                continue;
            }
        }
        // 目录凭据过期时间 >= 快照则采纳(相等也采纳,换取正确的设备身份)
        if read.expires_at >= best.expires_at {
            best = read;
            adopted = true;
        }
    }
    (best, adopted)
}

/// 已有多开目录的扫描结果(返回前端)
#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct InstanceDirInfo {
    pub data_dir: String,
    pub user_id: String,
    pub account_name: String,
    /// token 过期时间(毫秒),0 表示未知
    pub expires_at: i64,
    /// 目录含设备签名密钥(可刷新 token);false 则 token 过期后需重新登录
    pub has_signing_key: bool,
    /// 实例当前是否在运行
    pub running: bool,
    /// 是否已被应用内账号绑定(同 data_dir 或同 userId)
    pub bound: bool,
}

/// 扫描 %APPDATA% 下已存在的多开/登录临时目录,提取登录信息供导入。
/// 只列含 iCubeAuthInfo 的目录;完整凭据读取失败时退宽松读取(签名密钥缺失标注)。
pub fn scan_instance_dirs() -> Vec<InstanceDirInfo> {
    let Ok(appdata) = std::env::var("APPDATA") else {
        return Vec::new();
    };
    let Ok(entries) = fs::read_dir(&appdata) else {
        return Vec::new();
    };
    let tool_prefix = format!("{DATA_DIR_NAME}_");
    let login_prefix = format!("{DATA_DIR_NAME} login ");

    let mut out = Vec::new();
    for entry in entries.flatten() {
        let name = entry.file_name().to_string_lossy().to_string();
        // 主目录(无后缀)走"导入当前 TRAE 桌面账号";共享插件目录无登录信息
        if name == DATA_DIR_NAME
            || name == SHARED_EXTENSIONS_DIR
            || (!name.starts_with(&tool_prefix) && !name.starts_with(&login_prefix))
        {
            continue;
        }
        let dir = entry.path();
        if !dir.join("User").join("globalStorage").join("storage.json").exists() {
            continue;
        }
        // 完整凭据优先(含签名密钥);失败退宽松读取(密钥缺失)
        let (cred, has_signing_key) =
            match crate::trae_auth::read_credentials_from_data_dir(&dir) {
                Ok(c) => (c, true),
                Err(_) => match crate::trae_auth::read_auth_from_data_dir_loose(
                    &dir,
                    &Credential::empty(),
                ) {
                    Ok(c) => (c, false),
                    Err(_) => continue, // 无登录信息,不列入
                },
            };
        if cred.user_id.is_empty() && cred.token.is_empty() {
            continue;
        }
        let running = crate::trae_machine::is_instance_running(&dir.to_string_lossy()).0;
        out.push(InstanceDirInfo {
            data_dir: dir.to_string_lossy().to_string(),
            user_id: cred.user_id,
            account_name: cred.account_name,
            expires_at: cred.expires_at,
            has_signing_key,
            running,
            bound: false,
        });
    }
    out.sort_by(|a, b| a.data_dir.cmp(&b.data_dir));
    out
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

    /// 测试本地写入 iCubeAuthInfo + telemetry.devDeviceId 到目录(生产代码不回写实例目录)
    fn write_test_credential(data_dir: &str, cred: &Credential) {
        let storage_dir = PathBuf::from(data_dir).join("User").join("globalStorage");
        fs::create_dir_all(&storage_dir).unwrap();
        let auth_plain = serde_json::to_string(&build_auth_info(cred)).unwrap();
        let encrypted = encrypt_trae_auth_info(&auth_plain).unwrap();
        let storage = json!({
            "iCubeAuthInfo://icube.cloudide": encrypted,
            "telemetry.devDeviceId": cred.device_id,
        });
        fs::write(
            storage_dir.join("storage.json"),
            serde_json::to_string_pretty(&storage).unwrap(),
        )
        .unwrap();
    }

    /// 回写 storage.json + 宽松回读 + 同步采纳:目录凭据更新时采纳,更旧/他人账号不采纳
    #[test]
    fn credential_write_read_sync_roundtrip() {
        let base = Credential {
            token: "old-token".into(),
            refresh_token: "rt".into(),
            expires_at: 1_000,
            refresh_expires_at: 2_000,
            device_id: "dev".into(),
            machine_id: "mach".into(),
            private_key_pem: "pk".into(),
            public_key_pem: "pub".into(),
            user_id: "u1".into(),
            account_name: "tester".into(),
            host: "https://example.com".into(),
            email: None,
            avatar_url: None,
            region: Some("CN".into()),
        };
        let tmp_dir = std::env::temp_dir().join("trae-sync-test-u1");
        let _ = std::fs::remove_dir_all(&tmp_dir);
        let dir_str = tmp_dir.to_string_lossy().to_string();

        let account = Account {
            id: "sync-test".into(),
            name: "tester".into(),
            cookie: String::new(),
            created_at: 0,
            last_checkin_at: None,
            last_checkin_result: None,
            last_checkin_message: None,
            points: None,
            enabled: true,
            desktop_user_id: Some("u1".into()),
            encrypted_credential: None,
            credential_status: None,
            data_dir: Some(dir_str.clone()),
            machine_id: None,
        };

        // 目录凭据更新(expiresAt 更晚)-> 采纳
        let mut newer = base.clone();
        newer.token = "new-token".into();
        newer.expires_at = 5_000;
        write_test_credential(&dir_str, &newer);
        let (synced, adopted) = sync_credential_from_instance(&account, &base);
        assert!(adopted, "更新凭据应标记已采纳");
        assert_eq!(synced.token, "new-token", "更目录新凭据应被采纳");
        assert_eq!(synced.expires_at, 5_000);
        // 签名密钥等缺失字段由快照兜底
        assert_eq!(synced.private_key_pem, base.private_key_pem);

        // 目录凭据更旧 -> 不采纳,保留快照
        let mut older = base.clone();
        older.token = "ancient-token".into();
        older.expires_at = 100;
        write_test_credential(&dir_str, &older);
        let (kept, adopted2) = sync_credential_from_instance(&account, &base);
        assert!(!adopted2, "更旧凭据不应被采纳");
        assert_eq!(kept.token, base.token, "更旧凭据不应被采纳");

        // userId 不匹配的目录(他人账号)-> 不采纳
        let mut stranger = base.clone();
        stranger.user_id = "someone-else".into();
        stranger.token = "stranger-token".into();
        stranger.expires_at = 9_000;
        write_test_credential(&dir_str, &stranger);
        let (kept2, adopted3) = sync_credential_from_instance(&account, &base);
        assert!(!adopted3, "他人账号凭据不应被采纳");
        assert_eq!(kept2.token, base.token, "他人账号凭据不应被采纳");

        // 目录凭据过期时间与快照相同但设备 id 不同(工具实例独立设备) -> 应采纳,
        // 保证 x-device-id 用的是实例真实设备而不是快照里主账号的设备
        let mut same_expiry = base.clone();
        same_expiry.device_id = "independent-device".into();
        same_expiry.expires_at = 1_000; // 与 base 相同
        write_test_credential(&dir_str, &same_expiry);
        let (synced2, adopted4) = sync_credential_from_instance(&account, &base);
        assert!(adopted4, "过期时间相同但设备不同应采纳以同步设备身份");
        assert_eq!(synced2.device_id, "independent-device");

        let _ = std::fs::remove_dir_all(&tmp_dir);
    }

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
