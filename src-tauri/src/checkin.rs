// 签到与积分查询。移植自 electron/checkin.ts 的 desktop 分支(读桌面凭据调 TRAE API)。

use std::time::Duration;

use serde_json::{json, Value};

use crate::credentials::{decrypt_credential, encrypt_credential};
use crate::error::{AppError, AppResult};
use crate::models::{
    credential_status, Account, CheckinLog, CheckinResult, Credential, EntitlementDetail,
    PointsResult, PublicAccount,
};
use crate::store::{generate_id, AppState};

const STATUS_PATH: &str = "/trae/api/v2/ug/checkin_credits/status";
const CLAIM_PATH: &str = "/trae/api/v2/ug/checkin_credits/claim";

/// 今日已签到时的消息文案,也是日志三态判定的依据
pub const ALREADY_CHECKED_IN: &str = "今日已签到";
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

/// 将回读采纳的凭据加密回写应用存储(应用不主动刷新 token,不回写实例目录)
fn persist_credential(state: &AppState, account: &Account, cred: &Credential) {
    let status = credential_status(cred.expires_at, now_ms());
    let Ok(new_encrypted) = encrypt_credential(cred) else {
        return;
    };
    let mut data = state.data.lock().unwrap();
    data.update_account(
        &account.id,
        json!({ "encryptedCredential": new_encrypted, "credentialStatus": status }),
    );
    let _ = data.save(&state.path);
}

/// 标记账号凭证失效(刷新失败时)
fn mark_credential_expired(state: &AppState, account_id: &str) {
    let mut data = state.data.lock().unwrap();
    data.update_account(account_id, json!({ "credentialStatus": "expired" }));
    let _ = data.save(&state.path);
}

/// 判断是否为鉴权失败:HTTP 401/403,或业务码/消息指向 token 失效/未登录
fn is_auth_failure(http_status: u16, data: Option<&Value>) -> bool {
    if http_status == 401 || http_status == 403 {
        return true;
    }
    let Some(d) = data else {
        return false;
    };
    let code = d
        .get("code")
        .and_then(|v| v.as_i64())
        .or_else(|| {
            d.get("code")
                .and_then(|v| v.as_str())
                .and_then(|s| s.parse::<i64>().ok())
        });
    if code == Some(401) || code == Some(403) {
        return true;
    }
    let msg = d
        .get("message")
        .or_else(|| d.get("msg"))
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_lowercase();
    // 限频/风控不是凭证问题,不当作鉴权失败(避免误标"token 已失效")
    if msg.contains("频繁") || msg.contains("frequent") || msg.contains("too many") {
        return false;
    }
    [
        "unauthorized",
        "token",
        "expired",
        "not login",
        "not logged",
        "登录",
        "鉴权",
    ]
    .iter()
    .any(|k| msg.contains(k))
}

/// 解密凭据并保持最新:先从实例目录回读(TRAE 客户端运行时会自行刷新 token 并写回 storage.json,
/// 应用只负责回读,不主动调 ExchangeToken)。
/// token 已过期时返回明确指引:请打开该账号的 TRAE 实例让客户端刷新后再试。
async fn get_valid_credential(
    account: &Account,
    state: &AppState,
) -> AppResult<Credential> {
    // 多开实例签到设备隔离:签到设备ID由 TraeMate 独立持有并持久化(get_or_create_checkin_device_id),
    // 只用在 x-device-id 请求头,绝不改写客户端 storage.json。客户端手动登录会把设备重置回
    // 机器级 ID(与主账号共用会触发服务端设备维限额 code 9074),若靠改写 storage.json 注入
    // 独立设备会让客户端检测到设备变化而强制要求重新登录。主账号返回 None,沿用回读设备。
    let isolated_device = crate::trae_instance::get_or_create_checkin_device_id(account, state);
    let encrypted = account
        .encrypted_credential
        .as_ref()
        .ok_or_else(|| AppError::Credential("该账号尚未导入 TRAE 桌面凭证".into()))?;
    let mut cred = decrypt_credential(encrypted)?;
    let now = now_ms();
    // 实例目录回读:目录有最新凭据则采纳。目录凭据未过期(即使比快照早)也应采纳——
    // 快照可能是之前误标的 expired,自愈恢复
    let (synced, adopted) = crate::trae_instance::sync_credential_from_instance(account, &cred);
    if synced.expires_at > now || adopted {
        persist_credential(state, account, &synced);
        cred = synced;
    }
    // 多开实例:签到设备ID一律用实例隔离后的设备——快照/回读可能残留被重置的机器级 ID,
    // 即使 token 走快照,设备也必须是指定的独立设备,否则与主账号共用触发设备维限额。
    // 主账号(无独立 data-dir)返回 None,保持原设备不动。
    if let Some(dev) = isolated_device {
        cred.device_id = dev;
    }
    if cred.expires_at <= now {
        mark_credential_expired(state, &account.id);
        return Err(AppError::Credential(
            "token 已过期，请打开该账号的 TRAE 实例（TRAE 会自动刷新），刷新后重试".into(),
        ));
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

/// 手动刷新账号凭证:仅从实例目录回读最新凭据(TRAE 客户端运行时会自行刷新 token 并写回
/// storage.json),采纳更新凭据回写应用存储。应用自身不调 ExchangeToken 刷新。
pub async fn refresh_account_credential(
    account: &Account,
    client: &reqwest::Client,
    state: &AppState,
) -> AppResult<Credential> {
    let _ = client;
    let encrypted = account
        .encrypted_credential
        .as_ref()
        .ok_or_else(|| AppError::Credential("该账号尚未导入 TRAE 桌面凭证".into()))?;
    let cred = decrypt_credential(encrypted)?;
    // 回读实例目录:目录凭据未过期则采纳(修复快照被误标 expired 后无法自愈的问题)
    let (synced, adopted) = crate::trae_instance::sync_credential_from_instance(account, &cred);
    if synced.expires_at > now_ms() || adopted {
        persist_credential(state, account, &synced);
        Ok(synced)
    } else {
        mark_credential_expired(state, &account.id);
        Err(AppError::Credential(
            "token 已过期，请打开该账号的 TRAE 实例（TRAE 会自动刷新），刷新后重试".into(),
        ))
    }
}

/// 单账号签到(桌面凭据模式):凭据由 get_valid_credential 回读自 TRAE 实例目录,
/// 应用不主动刷新 token。鉴权失败仅提示,不做网络刷新。
pub async fn checkin_by_desktop(
    account: &Account,
    client: &reqwest::Client,
    state: &AppState,
) -> CheckinResult {
    let cred = match get_valid_credential(account, state).await {
        Ok(c) => c,
        Err(e) => {
            return CheckinResult {
                success: false,
                message: e.to_string(),
                points: None,
            }
        }
    };
    // 鉴权失败(token 被服务端判失效):应用不刷新,给出指引
    let (result, auth_failed) = checkin_once(&cred, client).await;
    if !auth_failed {
        return result;
    }
    mark_credential_expired(state, &account.id);
    CheckinResult {
        success: false,
        message: "签到接口鉴权失败(token 已失效)，请打开该账号的 TRAE 实例让客户端刷新后再试".into(),
        points: None,
    }
}

/// 凭据指纹(诊断用,仅脱敏前 8 位 token,不含完整密钥)
fn cred_fingerprint(cred: &Credential) -> String {
    let tok: String = cred.token.chars().take(8).collect();
    format!(
        "token={tok}… host={} dev={} uid={} exp={} now={}",
        cred.host,
        cred.device_id,
        cred.user_id,
        cred.expires_at,
        now_ms()
    )
}

/// 用给定凭据执行一次签到(查询状态 -> 领取)。返回 (结果, 是否疑似鉴权失败)
async fn checkin_once(cred: &Credential, client: &reqwest::Client) -> (CheckinResult, bool) {
    let headers = auth_headers(cred);
    let host = &cred.host;

    // 1. 查询签到状态
    let status_url = format!("{}{}", host, STATUS_PATH);
    let resp = match client
        .post(&status_url)
        .headers(headers.clone())
        .json(&json!({}))
        .timeout(Duration::from_secs(30))
        .send()
        .await
    {
        Ok(r) => r,
        Err(e) => {
            return (
                CheckinResult {
                    success: false,
                    message: format!("TRAE 桌面端签到失败: {e}"),
                    points: None,
                },
                false,
            )
        }
    };
    let http_status = resp.status().as_u16();
    let status_data: Value = match resp.json().await {
        Ok(v) => v,
        Err(e) => {
            return (
                CheckinResult {
                    success: false,
                    message: format!("解析签到状态失败: {e}"),
                    points: None,
                },
                false,
            )
        }
    };
    eprintln!(
        "[checkin] 状态查询 HTTP {} {} 凭据={}",
        http_status,
        status_data,
        cred_fingerprint(cred)
    );

    if is_auth_failure(http_status, Some(&status_data)) {
        return (
            CheckinResult {
                success: false,
                message: "签到接口鉴权失败(token 已失效)".into(),
                points: None,
            },
            true,
        );
    }

    if status_data
        .get("checked_in")
        .and_then(|v| v.as_bool())
        .unwrap_or(false)
    {
        return (
            CheckinResult {
                success: true,
                message: ALREADY_CHECKED_IN.into(),
                points: None,
            },
            false,
        );
    }
    if !api_succeeded(&status_data) {
        return (
            CheckinResult {
                success: false,
                message: status_data
                    .get("message")
                    .and_then(|v| v.as_str())
                    .or_else(|| status_data.get("msg").and_then(|v| v.as_str()))
                    .unwrap_or("无法获取 TRAE 签到状态")
                    .to_string(),
                points: None,
            },
            false,
        );
    }

    // 2. 领取签到
    let claim_url = format!("{}{}", host, CLAIM_PATH);
    let resp = match client
        .post(&claim_url)
        .headers(headers)
        .json(&json!({}))
        .timeout(Duration::from_secs(30))
        .send()
        .await
    {
        Ok(r) => r,
        Err(e) => {
            return (
                CheckinResult {
                    success: false,
                    message: format!("TRAE 桌面端签到失败: {e}"),
                    points: None,
                },
                false,
            )
        }
    };
    let http_status = resp.status().as_u16();
    let claim_data: Value = match resp.json().await {
        Ok(v) => v,
        Err(e) => {
            return (
                CheckinResult {
                    success: false,
                    message: format!("解析签到结果失败: {e}"),
                    points: None,
                },
                false,
            )
        }
    };
    eprintln!(
        "[checkin] 领取 HTTP {} {} 凭据={}",
        http_status,
        claim_data,
        cred_fingerprint(cred)
    );

    if is_auth_failure(http_status, Some(&claim_data)) {
        return (
            CheckinResult {
                success: false,
                message: "签到接口鉴权失败(token 已失效)".into(),
                points: None,
            },
            true,
        );
    }

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
        (
            CheckinResult {
                success: true,
                message: msg,
                points,
            },
            false,
        )
    } else {
        (
            CheckinResult {
                success: false,
                message: claim_data
                    .get("message")
                    .and_then(|v| v.as_str())
                    .or_else(|| claim_data.get("msg").and_then(|v| v.as_str()))
                    .unwrap_or("签到失败")
                    .to_string(),
                points: None,
            },
            false,
        )
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

// 从 user_current_entitlement_list 提取各类型可用余额明细
fn extract_entitlement_details(data: &Value) -> Vec<EntitlementDetail> {
    let mut out = Vec::new();
    let Some(packs) = data.get("user_entitlement_pack_list").and_then(|v| v.as_array()) else {
        return out;
    };
    for pack in packs {
        let base = pack.get("entitlement_base_info");
        let limit = base
            .and_then(|b| b.get("quota"))
            .and_then(|q| q.get("credits_limit"))
            .and_then(|v| v.as_i64())
            .unwrap_or(0);
        if limit <= 0 {
            continue;
        }
        let used = pack
            .get("usage")
            .and_then(|u| u.get("credits_amount"))
            .and_then(|v| v.as_i64())
            .unwrap_or(0);
        let remaining = (limit - used).max(0);
        // 剩余 0 的权益包对"可用余额"无意义,直接丢弃,避免前端一堆 0 分噪音
        if remaining <= 0 {
            continue;
        }
        let name = pack
            .get("name")
            .and_then(|v| v.as_str())
            .or_else(|| base.and_then(|b| b.get("name")).and_then(|v| v.as_str()))
            .unwrap_or("积分")
            .to_string();
        let expire_at = pack
            .get("effective_to")
            .and_then(|v| v.as_i64())
            .or_else(|| pack.get("expire_time").and_then(|v| v.as_i64()))
            .or_else(|| base.and_then(|b| b.get("expire_time")).and_then(|v| v.as_i64()))
            .unwrap_or(0);
        out.push(EntitlementDetail {
            name,
            total: limit,
            remaining,
            expire_at,
        });
    }
    out
}

fn extract_usage_summary_points(data: &Value) -> Option<i64> {
    let summary = data.get("usage_summary")?;
    let total = summary.get("total_amount")?.as_f64()?;
    let consumed = summary
        .get("consumed_amount")
        .and_then(|v| v.as_f64())
        .unwrap_or(0.0);
    let available = total - consumed;
    if available <= 0.0 {
        return None;
    }
    Some(available.round() as i64)
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

/// 查询账号总积分:请求遇鉴权失败时,反应式刷新 token 后重试一次
pub async fn get_total_points(
    account: &Account,
    client: &reqwest::Client,
    state: &AppState,
) -> PointsResult {
    let cred = match get_valid_credential(account, state).await {
        Ok(c) => c,
        Err(e) => {
            return PointsResult {
                success: false,
                message: e.to_string(),
                total_points: None,
                details: vec![],
                points_response: None,
            }
        }
    };
    // 鉴权失败(token 被服务端判失效):应用不刷新,给出指引
    let (result, auth_failed) = points_once(&cred, client).await;
    if !auth_failed {
        return result;
    }
    mark_credential_expired(state, &account.id);
    PointsResult {
        success: false,
        message: "积分接口鉴权失败(token 已失效)，请打开该账号的 TRAE 实例让客户端刷新后再试".into(),
        total_points: None,
        details: vec![],
        points_response: None,
    }
}

/// 将积分查询结果转成可写入账号的字段(json),供持久化
pub fn points_update_json(result: &PointsResult) -> serde_json::Value {
    let mut m = serde_json::Map::new();
    if let Some(tp) = result.total_points {
        m.insert("points".into(), serde_json::json!(tp));
        m.insert("pointsUpdatedAt".into(), serde_json::json!(now_ms()));
    }
    if !result.details.is_empty() {
        m.insert(
            "pointsDetails".into(),
            serde_json::to_value(&result.details).unwrap_or_default(),
        );
    }
    if let Some(resp) = &result.points_response {
        m.insert("pointsResponse".into(), serde_json::json!(resp));
    }
    serde_json::Value::Object(m)
}

/// 用给定凭据查询一次总积分(遍历余额接口)。返回 (结果, 是否疑似鉴权失败)
async fn points_once(cred: &Credential, client: &reqwest::Client) -> (PointsResult, bool) {
    let headers = auth_headers(cred);
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
        let http_status = resp.status().as_u16();
        let data: Value = match resp.json().await {
            Ok(v) => v,
            Err(_) => continue,
        };

        // 鉴权失败:token 已失效,不再遍历剩余接口,交由上层刷新重试
        if is_auth_failure(http_status, Some(&data)) {
            return (
                PointsResult {
                    success: false,
                    message: "积分接口鉴权失败(token 已失效)".into(),
                    total_points: None,
                    details: vec![],
                    points_response: None,
                },
                true,
            );
        }

        if path.contains("user_current_entitlement_list") {
            let details = extract_entitlement_details(&data);
            // 优先用 usage_summary 的精确可用积分(total - consumed)
            if let Some(points) = extract_usage_summary_points(&data) {
                return (
                    PointsResult {
                        success: true,
                        message: "获取积分成功".into(),
                        total_points: Some(points),
                        details,
                        points_response: Some(data.to_string()),
                    },
                    false,
                );
            }
            if !details.is_empty() {
                let total = details.iter().map(|d| d.remaining).sum();
                return (
                    PointsResult {
                        success: true,
                        message: "获取积分成功".into(),
                        total_points: Some(total),
                        details,
                        points_response: Some(data.to_string()),
                    },
                    false,
                );
            }
            continue;
        }
        if let Some(points) = extract_points_from_data(&data) {
            return (
                PointsResult {
                    success: true,
                    message: "获取积分成功".into(),
                    total_points: Some(points),
                    details: vec![],
                    points_response: None,
                },
                false,
            );
        }
    }

    (
        PointsResult {
            success: false,
            message: "未能获取到积分信息".into(),
            total_points: None,
            details: vec![],
            points_response: None,
        },
        false,
    )
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
    // 三态:成功签到 / 今日已签到 / 失败
    let status = if result.success && result.message == ALREADY_CHECKED_IN {
        "already"
    } else if result.success {
        "success"
    } else {
        "failed"
    };
    let mut data = state.data.lock().unwrap();
    data.update_account(
        &account.id,
        json!({
            "lastCheckinAt": now,
            "lastCheckinResult": status,
            "lastCheckinMessage": result.message,
            "points": new_points,
        }),
    );
    data.add_log(CheckinLog {
        id: generate_id(),
        account_id: account.id.clone(),
        account_name: account.name.clone(),
        time: now,
        result: status.into(),
        message: result.message.clone(),
        points_gained: result.points,
        points_balance: new_points,
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
    run_all_checkin(client, state, None).await
}

/// 并行错峰签到核心。
pub(crate) async fn run_all_checkin(
    client: &reqwest::Client,
    state: &AppState,
    tx: Option<tokio::sync::mpsc::Sender<(PublicAccount, CheckinResult)>>,
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

    // 每个账号错峰启动 500ms,既并行提速又降低同时打点触发限频的概率
    const STAGGER_MS: u64 = 500;
    let tasks = accounts.into_iter().enumerate().map(|(i, account)| {
        let tx = tx.clone();
        async move {
            if i > 0 {
                tokio::time::sleep(Duration::from_millis(STAGGER_MS * i as u64)).await;
            }
            let mut last_err = String::new();
            let mut result = None;
            for attempt in 0..=retry_count {
                let r = perform_checkin(&account, client, state).await;
                if r.success {
                    result = Some(r);
                    break;
                }
                last_err = r.message;
                if attempt < retry_count {
                    tokio::time::sleep(Duration::from_secs(retry_delay as u64)).await;
                }
            }
            let r = match result {
                Some(r) => r,
                None => CheckinResult {
                    success: false,
                    message: last_err,
                    points: None,
                },
            };
            if let Some(tx) = tx {
                let _ = tx.send((account.clone().into(), r.clone())).await;
            }
            (account.into(), r)
        }
    });
    futures::future::join_all(tasks).await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn auth_failure_detection() {
        // HTTP 401/403 直接判定
        assert!(is_auth_failure(401, None));
        assert!(is_auth_failure(403, None));
        assert!(!is_auth_failure(200, None));
        // 业务码 401/403
        assert!(is_auth_failure(200, Some(&json!({ "code": 401 }))));
        assert!(is_auth_failure(
            200,
            Some(&json!({ "code": "403", "message": "ok" }))
        ));
        // 消息关键词(token 失效/未登录)
        assert!(is_auth_failure(
            200,
            Some(&json!({ "code": 1001, "message": "token expired" }))
        ));
        assert!(is_auth_failure(
            200,
            Some(&json!({ "msg": "请先登录" }))
        ));
        assert!(is_auth_failure(
            200,
            Some(&json!({ "message": "Unauthorized" }))
        ));
        // 正常业务响应不误判
        assert!(!is_auth_failure(
            200,
            Some(&json!({ "code": 0, "message": "success" }))
        ));
        assert!(!is_auth_failure(
            200,
            Some(&json!({ "message": "今日已签到" }))
        ));
        // 限频/风控不是凭证问题,不判为鉴权失败
        assert!(!is_auth_failure(
            200,
            Some(&json!({ "message": "操作太过频繁啦，请稍后尝试" }))
        ));
    }
}

#[cfg(test)]
mod e2e_tests {
    use super::*;
    use crate::credentials::encrypt_credential;
    use crate::models::Account;
    use crate::store::{AppState, StoreData};
    use crate::trae_auth::get_trae_desktop_credentials;
    use rand::Rng;
    use std::sync::Mutex;

    /// 直连 claim:跳过 status 预检,直接 POST claim 接口,打印服务端原始返回。
    /// 用于验证"今天已签过"时 claim 是否接受真实桌面设备(而非提前 return 走不到 claim)。
    #[tokio::test]
    async fn e2e_direct_claim() {
        let cred = match get_trae_desktop_credentials() {
            Ok(c) => c,
            Err(e) => {
                eprintln!("[e2e-direct-claim] 未读取到 TRAE 桌面凭据: {e}");
                return;
            }
        };
        eprintln!(
            "[e2e-direct-claim] 凭据: host={} dev={} uid={} exp={}",
            cred.host, cred.device_id, cred.user_id, cred.expires_at
        );
        let client = reqwest::Client::new();
        let claim_url = format!("{}{}", cred.host, CLAIM_PATH);
        let resp = client
            .post(&claim_url)
            .headers(auth_headers(&cred))
            .json(&json!({}))
            .timeout(Duration::from_secs(30))
            .send()
            .await;
        match resp {
            Ok(r) => {
                let http_status = r.status().as_u16();
                let body: Value = r.json().await.unwrap_or(json!({}));
                eprintln!("[e2e-direct-claim] HTTP {http_status} body={body}");
            }
            Err(e) => {
                eprintln!("[e2e-direct-claim] 请求失败: {e}");
            }
        }
    }

    /// 直连 claim:走正常读取路径(get_trae_desktop_credentials),验证签到使用的是 aha 设备ID
    /// (icube-dc key 后缀)而非废弃的 telemetry.devDeviceId。
    #[tokio::test]
    async fn e2e_direct_claim_aha_device() {
        let cred = match get_trae_desktop_credentials() {
            Ok(c) => c,
            Err(e) => {
                eprintln!("[e2e-aha] 未读取到 TRAE 桌面凭据: {e}");
                return;
            }
        };
        eprintln!(
            "[e2e-aha] 凭据读取的设备ID: {} (应为 icube-dc 后缀,非 bcda0361...) uid={}",
            cred.device_id, cred.user_id
        );
        let client = reqwest::Client::new();
        let claim_url = format!("{}{}", cred.host, CLAIM_PATH);
        let resp = client
            .post(&claim_url)
            .headers(auth_headers(&cred))
            .json(&json!({}))
            .timeout(Duration::from_secs(30))
            .send()
            .await;
        match resp {
            Ok(r) => {
                let http_status = r.status().as_u16();
                let body: Value = r.json().await.unwrap_or(json!({}));
                eprintln!("[e2e-aha] HTTP {http_status} body={body}");
            }
            Err(e) => {
                eprintln!("[e2e-aha] 请求失败: {e}");
            }
        }
    }

    /// 扫描多开实例目录的账号身份与当前 aha 设备ID(icube-dc 后缀),判断设备隔离现状。
    /// 仅打印脱敏信息,不发网络请求。
    #[test]
    fn e2e_inspect_instance_device_ids() {
        let Ok(appdata) = std::env::var("APPDATA") else {
            return;
        };
        let dirs = [
            format!("{appdata}\\TRAE SOLO CN_YOUR_ACCOUNT_1"),
            format!("{appdata}\\TRAE SOLO CN_YOUR_ACCOUNT_2"),
        ];
        for d in dirs {
            let path = std::path::Path::new(&d);
            match crate::trae_auth::read_auth_from_data_dir_loose(
                path,
                &crate::models::Credential::empty(),
            ) {
                Ok(c) => eprintln!(
                    "[inspect] dir={} account={} uid={} device_id={} host={}",
                    d, c.account_name, c.user_id, c.device_id, c.host
                ),
                Err(e) => eprintln!("[inspect] dir={} 读取失败: {e}", d),
            }
        }
    }

    /// 关键验证:用多开实例账号(账号02)的 token + 伪造的独立 aha 设备ID直连 claim,
    /// 判断服务端是否接受"注入的"设备ID——决定多开实例能否靠改写设备ID实现隔离。
    #[tokio::test]
    async fn e2e_claim_fabricated_device_instance() {
        let Ok(appdata) = std::env::var("APPDATA") else {
            return;
        };
        let dir = format!("{appdata}\\TRAE SOLO CN_YOUR_ACCOUNT_1");
        let cred = match crate::trae_auth::read_auth_from_data_dir_loose(
            std::path::Path::new(&dir),
            &crate::models::Credential::empty(),
        ) {
            Ok(c) => c,
            Err(e) => {
                eprintln!("[e2e-fake] 读取实例凭据失败: {e}");
                return;
            }
        };
        let mut rng = rand::thread_rng();
        let fake: u64 = rng.gen_range(1_000_000_000_000_000_u64..9_999_999_999_999_999_u64);
        let mut fake_cred = cred.clone();
        fake_cred.device_id = fake.to_string();
        eprintln!(
            "[e2e-fake] account={} uid={} 原设备={} -> 伪造设备={}",
            cred.account_name, cred.user_id, cred.device_id, fake_cred.device_id
        );
        let client = reqwest::Client::new();
        let claim_url = format!("{}{}", fake_cred.host, CLAIM_PATH);
        let resp = client
            .post(&claim_url)
            .headers(auth_headers(&fake_cred))
            .json(&json!({}))
            .timeout(Duration::from_secs(30))
            .send()
            .await;
        match resp {
            Ok(r) => {
                let http_status = r.status().as_u16();
                let body: Value = r.json().await.unwrap_or(json!({}));
                eprintln!("[e2e-fake] HTTP {http_status} body={body}");
            }
            Err(e) => eprintln!("[e2e-fake] 请求失败: {e}"),
        }
    }

    /// 账号03 签到验证:用多开实例账号(账号03,目录 YOUR_ACCOUNT_2)的 token +
    /// 新随机伪造的设备ID直连 claim。账号03 今日未签到,验证伪造设备能否为其完成签到。
    /// 只读目录凭据,不落盘、不碰账号03 原本存储的设备。
    #[tokio::test]
    async fn e2e_claim_fabricated_micro03() {
        let Ok(appdata) = std::env::var("APPDATA") else {
            return;
        };
        let dir = format!("{appdata}\\TRAE SOLO CN_YOUR_ACCOUNT_2");
        let cred = match crate::trae_auth::read_auth_from_data_dir_loose(
            std::path::Path::new(&dir),
            &crate::models::Credential::empty(),
        ) {
            Ok(c) => c,
            Err(e) => {
                eprintln!("[e2e-micro03] 读取实例凭据失败: {e}");
                return;
            }
        };
        let mut rng = rand::thread_rng();
        let fake: u64 = rng.gen_range(1_000_000_000_000_000_u64..9_999_999_999_999_999_u64);
        let mut fake_cred = cred.clone();
        fake_cred.device_id = fake.to_string();
        eprintln!(
            "[e2e-micro03] account={} uid={} 原设备={} -> 新伪造设备={}",
            cred.account_name, cred.user_id, cred.device_id, fake_cred.device_id
        );
        let client = reqwest::Client::new();
        let claim_url = format!("{}{}", fake_cred.host, CLAIM_PATH);
        let resp = client
            .post(&claim_url)
            .headers(auth_headers(&fake_cred))
            .json(&json!({}))
            .timeout(Duration::from_secs(30))
            .send()
            .await;
        match resp {
            Ok(r) => {
                let http_status = r.status().as_u16();
                let body: Value = r.json().await.unwrap_or(json!({}));
                eprintln!("[e2e-micro03] HTTP {http_status} body={body}");
            }
            Err(e) => eprintln!("[e2e-micro03] 请求失败: {e}"),
        }
    }

    /// 对照实验:用多开实例账号(账号02)的 token + 其自身当前设备ID直连 claim。
    /// 区分 9074 是"伪造设备被拒"还是"当日总配额已满"。
    #[tokio::test]
    async fn e2e_claim_instance_original_device() {
        let Ok(appdata) = std::env::var("APPDATA") else {
            return;
        };
        let dir = format!("{appdata}\\TRAE SOLO CN_YOUR_ACCOUNT_1");
        let cred = match crate::trae_auth::read_auth_from_data_dir_loose(
            std::path::Path::new(&dir),
            &crate::models::Credential::empty(),
        ) {
            Ok(c) => c,
            Err(e) => {
                eprintln!("[e2e-orig] 读取实例凭据失败: {e}");
                return;
            }
        };
        eprintln!(
            "[e2e-orig] account={} uid={} 设备ID={} (实例自身设备,未注入)",
            cred.account_name, cred.user_id, cred.device_id
        );
        let client = reqwest::Client::new();
        let claim_url = format!("{}{}", cred.host, CLAIM_PATH);
        let resp = client
            .post(&claim_url)
            .headers(auth_headers(&cred))
            .json(&json!({}))
            .timeout(Duration::from_secs(30))
            .send()
            .await;
        match resp {
            Ok(r) => {
                let http_status = r.status().as_u16();
                let body: Value = r.json().await.unwrap_or(json!({}));
                eprintln!("[e2e-orig] HTTP {http_status} body={body}");
            }
            Err(e) => eprintln!("[e2e-orig] 请求失败: {e}"),
        }
    }

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
            points_updated_at: None,
            points_details: vec![],
            points_response: None,
            enabled: true,
            desktop_user_id: Some(cred.user_id.clone()),
            encrypted_credential: Some(encrypted),
            credential_status: None,
            data_dir: None,
            machine_id: None,
            checkin_device_id: None,
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
