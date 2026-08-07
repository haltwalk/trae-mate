// 签到与积分查询。移植自 electron/checkin.ts 的 desktop 分支(读桌面凭据调 TRAE API)。

use std::time::Duration;

use serde_json::{json, Value};

use crate::credentials::{decrypt_credential, encrypt_credential, refresh_credential};
use crate::error::{AppError, AppResult};
use crate::models::{
    credential_status, Account, CheckinLog, CheckinResult, Credential, PointsResult, PublicAccount,
};
use crate::store::{generate_id, AppState};

const STATUS_PATH: &str = "/trae/api/v2/ug/checkin_credits/status";
const CLAIM_PATH: &str = "/trae/api/v2/ug/checkin_credits/claim";
const CREDITS_BALANCE_PATHS: &[&str] = &[
    "/trae/api/v2/pay/user_current_entitlement_list",
    "/trae/api/v2/ug/credits/balance",
    "/trae/api/v2/ug/wallet/balance",
    "/trae/api/v2/ug/user/info",
    "/trae/api/v2/ug/credits",
    "/trae/api/v3/ug/credits/balance",
    "/trae/api/v3/ug/wallet/balance",
    "/trae/api/v3/ug/user/info",
];

fn now_ms() -> i64 {
    chrono::Utc::now().timestamp_millis()
}

fn api_succeeded(data: &Value) -> bool {
    data.get("code").and_then(|v| v.as_i64()).map(|c| c == 0 || c == 200).unwrap_or(false)
        || data.get("code").and_then(|v| v.as_str()).map(|s| s == "0" || s == "200").unwrap_or(false)
        || data.get("success").and_then(|v| v.as_bool()).unwrap_or(false)
        || data.get("status").and_then(|v| v.as_str()).map(|s| s == "success").unwrap_or(false)
}

/// 解密凭据;若即将过期(<=5min)则刷新并回写。失败时回写 expired 状态。
async fn get_valid_credential(
    account: &Account,
    client: &reqwest::Client,
    state: &AppState,
) -> AppResult<Credential> {
    let encrypted = account
        .encrypted_credential
        .as_ref()
        .ok_or_else(|| AppError::Credential("该账号尚未导入 TRAE 桌面凭证".into()))?;
    let mut cred = decrypt_credential(encrypted)?;
    let now = now_ms();
    if cred.expires_at - now <= 5 * 60 * 1000 {
        match refresh_credential(&cred, client).await {
            Ok(refreshed) => {
                let new_encrypted = encrypt_credential(&refreshed)?;
                let status = credential_status(refreshed.expires_at, now);
                let mut data = state.data.lock().unwrap();
                data.update_account(
                    &account.id,
                    json!({ "encryptedCredential": new_encrypted, "credentialStatus": status }),
                );
                let _ = data.save(&state.path);
                drop(data);
                cred = refreshed;
            }
            Err(_) => {
                let mut data = state.data.lock().unwrap();
                data.update_account(&account.id, json!({ "credentialStatus": "expired" }));
                let _ = data.save(&state.path);
                return Err(AppError::Credential("凭证已失效，请重新导入".into()));
            }
        }
    }
    Ok(cred)
}

fn auth_headers(cred: &Credential) -> reqwest::header::HeaderMap {
    let mut h = reqwest::header::HeaderMap::new();
    let _ = h.insert(
        reqwest::header::CONTENT_TYPE,
        reqwest::header::HeaderValue::from_static("application/json"),
    );
    if let Ok(v) = reqwest::header::HeaderValue::from_str(&format!(
        "Cloud-IDE-JWT {}",
        cred.token
    )) {
        h.insert(reqwest::header::AUTHORIZATION, v);
    }
    if let Ok(v) = reqwest::header::HeaderValue::from_str(&cred.device_id) {
        h.insert("x-device-id", v);
    }
    h
}

/// 单账号签到(桌面凭据模式)
pub async fn checkin_by_desktop(
    account: &Account,
    client: &reqwest::Client,
    state: &AppState,
) -> CheckinResult {
    let cred = match get_valid_credential(account, client, state).await {
        Ok(c) => c,
        Err(e) => {
            return CheckinResult {
                success: false,
                message: e.to_string(),
                points: None,
            }
        }
    };
    let headers = auth_headers(&cred);
    let host = &cred.host;

    // 1. 查询签到状态
    let status_url = format!("{}{}", host, STATUS_PATH);
    let status_resp = match client
        .post(&status_url)
        .headers(headers.clone())
        .json(&json!({}))
        .timeout(Duration::from_secs(30))
        .send()
        .await
    {
        Ok(r) => r,
        Err(e) => {
            return CheckinResult {
                success: false,
                message: format!("TRAE 桌面端签到失败: {e}"),
                points: None,
            }
        }
    };
    let status_data: Value = match status_resp.json().await {
        Ok(v) => v,
        Err(e) => {
            return CheckinResult {
                success: false,
                message: format!("解析签到状态失败: {e}"),
                points: None,
            }
        }
    };

    if status_data
        .get("checked_in")
        .and_then(|v| v.as_bool())
        .unwrap_or(false)
    {
        return CheckinResult {
            success: true,
            message: "今日已签到".into(),
            points: None,
        };
    }
    if !api_succeeded(&status_data) {
        return CheckinResult {
            success: false,
            message: status_data
                .get("message")
                .and_then(|v| v.as_str())
                .or_else(|| status_data.get("msg").and_then(|v| v.as_str()))
                .unwrap_or("无法获取 TRAE 签到状态")
                .to_string(),
            points: None,
        };
    }

    // 2. 领取签到
    let claim_url = format!("{}{}", host, CLAIM_PATH);
    let claim_resp = match client
        .post(&claim_url)
        .headers(headers)
        .json(&json!({}))
        .timeout(Duration::from_secs(30))
        .send()
        .await
    {
        Ok(r) => r,
        Err(e) => {
            return CheckinResult {
                success: false,
                message: format!("TRAE 桌面端签到失败: {e}"),
                points: None,
            }
        }
    };
    let claim_data: Value = match claim_resp.json().await {
        Ok(v) => v,
        Err(e) => {
            return CheckinResult {
                success: false,
                message: format!("解析签到结果失败: {e}"),
                points: None,
            }
        }
    };

    if api_succeeded(&claim_data) {
        let msg = {
            let m = claim_data
                .get("message")
                .and_then(|v| v.as_str())
                .or_else(|| claim_data.get("msg").and_then(|v| v.as_str()));
            match m {
                Some(s) if s == "success" => "签到成功".to_string(),
                Some(s) => s.to_string(),
                None => "签到成功".to_string(),
            }
        };
        let points = claim_data
            .get("data")
            .and_then(|d| d.get("points"))
            .and_then(|v| v.as_i64())
            .or_else(|| claim_data.get("points").and_then(|v| v.as_i64()))
            .or(Some(200));
        CheckinResult {
            success: true,
            message: msg,
            points,
        }
    } else {
        CheckinResult {
            success: false,
            message: claim_data
                .get("message")
                .and_then(|v| v.as_str())
                .or_else(|| claim_data.get("msg").and_then(|v| v.as_str()))
                .unwrap_or("签到失败")
                .to_string(),
            points: None,
        }
    }
}

// ===== 积分提取(移植自 checkin.ts) =====

fn find_all_numbers(obj: &Value, prefix: &str) -> Vec<(String, i64)> {
    let mut out = Vec::new();
    if let Some(o) = obj.as_object() {
        for (k, v) in o {
            let path = if prefix.is_empty() {
                k.clone()
            } else {
                format!("{prefix}.{k}")
            };
            match v {
                Value::Number(n) => {
                    if let Some(i) = n.as_i64() {
                        out.push((path, i));
                    } else if let Some(f) = n.as_f64() {
                        out.push((path, f as i64));
                    }
                }
                Value::String(s) => {
                    if let Ok(n) = s.trim().parse::<i64>() {
                        out.push((path, n));
                    }
                }
                Value::Object(_) | Value::Array(_) => {
                    out.extend(find_all_numbers(v, &path));
                }
                _ => {}
            }
        }
    } else if let Some(arr) = obj.as_array() {
        for (i, v) in arr.iter().enumerate() {
            let path = format!("{prefix}[{i}]");
            out.extend(find_all_numbers(v, &path));
        }
    }
    out
}

fn extract_trae_remaining_credits(data: &Value) -> Option<i64> {
    let packs = data.get("user_entitlement_pack_list")?.as_array()?;
    if packs.is_empty() {
        return None;
    }
    let mut remaining: i64 = 0;
    let mut found = false;
    for pack in packs {
        let limit = pack
            .get("entitlement_base_info")
            .and_then(|b| b.get("quota"))
            .and_then(|q| q.get("credits_limit"))
            .and_then(|v| v.as_i64());
        let used = pack
            .get("usage")
            .and_then(|u| u.get("credits_amount"))
            .and_then(|v| v.as_i64())
            .unwrap_or(0);
        if let Some(limit) = limit {
            if limit > 0 {
                found = true;
                remaining += (limit - used).max(0);
            }
        }
    }
    if found {
        Some(remaining)
    } else {
        None
    }
}

fn extract_points_from_data(data: &Value) -> Option<i64> {
    let keywords = [
        "credit", "point", "balance", "total", "available", "剩余", "积分", "总额",
    ];
    let exclude_kw = [
        "quota", "limit", "usage", "amount", "expire", "end_time", "start_time",
    ];
    let all = find_all_numbers(data, "");

    let mut kw_matches: Vec<(i64, String)> = all
        .iter()
        .filter(|(p, _)| {
            let lp = p.to_lowercase();
            if exclude_kw.iter().any(|k| lp.contains(k)) {
                return false;
            }
            keywords.iter().any(|k| lp.contains(k))
        })
        .map(|(p, v)| (*v, p.clone()))
        .collect();
    if !kw_matches.is_empty() {
        kw_matches.sort_by(|a, b| b.0.cmp(&a.0));
        if kw_matches[0].0 >= 100 {
            return Some(kw_matches[0].0);
        }
    }

    let mut large: Vec<i64> = all
        .iter()
        .filter(|(_, v)| *v >= 100 && *v < 1_000_000)
        .map(|(_, v)| *v)
        .collect();
    if !large.is_empty() {
        large.sort_by(|a, b| b.cmp(a));
        return Some(large[0]);
    }
    None
}

/// 查询账号总积分
pub async fn get_total_points(
    account: &Account,
    client: &reqwest::Client,
    state: &AppState,
) -> PointsResult {
    let cred = match get_valid_credential(account, client, state).await {
        Ok(c) => c,
        Err(e) => {
            return PointsResult {
                success: false,
                message: e.to_string(),
                total_points: None,
            }
        }
    };
    let headers = auth_headers(&cred);
    let host = &cred.host;

    for path in CREDITS_BALANCE_PATHS {
        let url = format!("{}{}", host, path);
        let resp = if path.contains("user_current_entitlement_list") {
            client
                .post(&url)
                .headers(headers.clone())
                .json(&json!({ "require_usage": true }))
                .timeout(Duration::from_secs(15))
                .send()
                .await
        } else {
            client
                .get(&url)
                .headers(headers.clone())
                .timeout(Duration::from_secs(15))
                .send()
                .await
        };
        let resp = match resp {
            Ok(r) => r,
            Err(_) => {
                // 回退到 POST 空 body
                match client
                    .post(&url)
                    .headers(headers.clone())
                    .json(&json!({}))
                    .timeout(Duration::from_secs(15))
                    .send()
                    .await
                {
                    Ok(r) => r,
                    Err(_) => continue,
                }
            }
        };
        let data: Value = match resp.json().await {
            Ok(v) => v,
            Err(_) => continue,
        };

        if path.contains("user_current_entitlement_list") {
            if let Some(remaining) = extract_trae_remaining_credits(&data) {
                return PointsResult {
                    success: true,
                    message: "获取积分成功".into(),
                    total_points: Some(remaining),
                };
            }
            continue;
        }
        if let Some(points) = extract_points_from_data(&data) {
            return PointsResult {
                success: true,
                message: "获取积分成功".into(),
                total_points: Some(points),
            };
        }
    }

    PointsResult {
        success: false,
        message: "未能获取到积分信息".into(),
        total_points: None,
    }
}

// ===== 执行签到(含状态更新与日志) =====

pub async fn perform_checkin(
    account: &Account,
    client: &reqwest::Client,
    state: &AppState,
) -> CheckinResult {
    let result = checkin_by_desktop(account, client, state).await;

    let now = now_ms();
    let new_points = match (result.points, account.points) {
        (Some(gained), Some(base)) => Some(base + gained),
        (Some(gained), None) => Some(gained),
        (None, base) => base,
    };
    let mut data = state.data.lock().unwrap();
    data.update_account(
        &account.id,
        json!({
            "lastCheckinAt": now,
            "lastCheckinResult": if result.success { "success" } else { "failed" },
            "lastCheckinMessage": result.message,
            "points": new_points,
        }),
    );
    data.add_log(CheckinLog {
        id: generate_id(),
        account_id: account.id.clone(),
        account_name: account.name.clone(),
        time: now,
        result: if result.success { "success".into() } else { "failed".into() },
        message: result.message.clone(),
        points_gained: result.points,
    });
    let _ = data.save(&state.path);
    drop(data);

    result
}

/// 执行所有启用账号签到,含重试与间隔
pub async fn perform_all_checkin(
    client: &reqwest::Client,
    state: &AppState,
) -> Vec<(PublicAccount, CheckinResult)> {
    let (accounts, retry_count, retry_delay) = {
        let data = state.data.lock().unwrap();
        let s = data.get_settings();
        let accs = data
            .get_accounts()
            .iter()
            .filter(|a| a.enabled)
            .cloned()
            .collect::<Vec<_>>();
        (accs, s.retry_count, s.retry_delay)
    };

    let mut results = Vec::new();
    for account in accounts {
        let mut last_err = String::new();
        let mut success = false;
        for i in 0..=retry_count {
            let r = perform_checkin(&account, client, state).await;
            if r.success {
                results.push((account.clone().into(), r));
                success = true;
                break;
            }
            last_err = r.message;
            if i < retry_count {
                tokio::time::sleep(Duration::from_secs(retry_delay as u64)).await;
            }
        }
        if !success {
            results.push((
                account.clone().into(),
                CheckinResult {
                    success: false,
                    message: last_err,
                    points: None,
                },
            ));
        }
        // 账号之间间隔 2s
        tokio::time::sleep(Duration::from_secs(2)).await;
    }
    results
}

#[cfg(test)]
mod e2e_tests {
    use super::*;
    use crate::credentials::encrypt_credential;
    use crate::models::Account;
    use crate::store::{AppState, StoreData};
    use crate::trae_auth::get_trae_desktop_credentials;
    use std::sync::Mutex;

    /// 端到端:读取桌面凭据 -> DPAPI 加密 -> perform_checkin 真实签到。
    /// 今日已签到则返回"今日已签到"(无副作用);未签到则执行 claim。
    #[tokio::test]
    async fn e2e_import_and_checkin() {
        let cred = match get_trae_desktop_credentials() {
            Ok(c) => c,
            Err(e) => {
                eprintln!("[e2e] 未读取到 TRAE 桌面凭据(可能未登录桌面客户端): {e}");
                return;
            }
        };
        let encrypted = encrypt_credential(&cred).expect("DPAPI 加密失败");
        let account = Account {
            id: "e2e".into(),
            name: cred.account_name.clone(),
            cookie: String::new(),
            created_at: 0,
            last_checkin_at: None,
            last_checkin_result: None,
            last_checkin_message: None,
            points: None,
            enabled: true,
            desktop_user_id: Some(cred.user_id.clone()),
            encrypted_credential: Some(encrypted),
            credential_status: None,
        };
        let state = AppState {
            data: Mutex::new(StoreData {
                accounts: vec![account.clone()],
                logs: vec![],
                settings: Default::default(),
            }),
            path: std::env::temp_dir().join("trae-check-e2e-test.json"),
        };
        let client = reqwest::Client::new();
        let result = perform_checkin(&account, &client, &state).await;
        eprintln!(
            "[e2e] 签到结果: success={}, message={}, points={:?}",
            result.success, result.message, result.points
        );
        assert!(result.success, "签到失败: {}", result.message);
    }
}
