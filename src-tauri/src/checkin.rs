// 签到与积分查询。移植自 electron/checkin.ts 的 desktop 分支(读桌面凭据调 TRAE API)。

use std::time::Duration;

use base64::{engine::general_purpose, Engine as _};
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

// ===== 自动刷新 token(ExchangeToken)=====
// TRAE 客户端刷新接口:POST {host}/trae/api/v3/oauth/ExchangeToken。
// 请求体 = ClientID + RefreshToken + DeviceInfo + DeviceProof(RSA-SHA256 签名,与客户端 qDe 一致)。
// 成功返回 Result{Token, RefreshToken, TokenExpireAt, TokenExpireDuration, RefreshExpireAt};
// 失败时 ResponseMetadata.Error.Code 为服务端错误码(20324/20101/... 表示刷新令牌失效需重登)。

/// ExchangeToken 刷新接口路径
const EXCHANGE_TOKEN_PATH: &str = "/trae/api/v3/oauth/ExchangeToken";
/// TRAE SOLO 客户端 ClientID(product.json iCubeApp.authConfig,稳定版)
const CLIENT_ID_SOLO: &str = "en1oxy7wnw8j9n";
/// 自动刷新阈值:token 剩余有效期 ≤ 该值且刷新令牌仍有效时,签到前主动调 ExchangeToken 续期
const AUTO_REFRESH_BEFORE_MS: i64 = 6 * 60 * 60 * 1000; // 6 小时
/// 客户端版本号。注意:这是 product.json iCubeApp.appVersion(如 "0.1.63"),不是
/// 构建版本号——服务端 ExchangeToken 的 BoundDeviceID 由整个 DeviceInfo 计算,
/// 刷新时任何字段(含 ClientVersion/DeviceName/OSInfo 等)与登录签发时不符都会 20403。
const IDE_VERSION: &str = "0.1.63";
/// 本机设备信息(与客户端登录请求一致,从 TRAE 登录日志提取):
/// 服务端设备绑定(BoundDeviceID)包含这些字段,刷新时必须逐字段一致。
const DEVICE_NAME: &str = "halt的电脑";
const DEVICE_MODEL: &str = "HP Pro Tower ZHAN 99 G9 Desktop PC";
const DEVICE_BRAND: &str = "HP";
const DEVICE_CPU: &str = "Intel(R) Core(TM) i7-14700";
const OS_INFO: &str = "windows";
const OS_VERSION: &str = "Windows 11 Home China";
/// 刷新令牌失效(需重新登录)的服务端错误码,与客户端 YP 常量一致
const REFRESH_TOKEN_INVALID_CODES: &[&str] = &[
    "20324", "20101", "20315", "20125", "20126", "20401", "20403",
];

/// 生成 DeviceProof(与客户端 qDe 完全一致):签名密钥为 EC P-256(PKCS8 PEM),
/// message = "{method} {path} {client_id} {refresh_token} {timestamp} {nonce}",
/// ECDSA-SHA256 签名输出 DER 编码后 base64(Node crypto.sign 对 EC 私钥即输出 DER)。
/// 返回 (signature_base64, timestamp_i64, nonce)——服务端要求 Timestamp 为整数。
fn device_proof(
    method: &str,
    path: &str,
    client_id: &str,
    refresh_token: &str,
    private_key_pem: &str,
) -> AppResult<(String, i64, String)> {
    use ecdsa::signature::Signer;
    use p256::ecdsa::SigningKey;
    use p256::pkcs8::DecodePrivateKey;

    let signing_key = SigningKey::from_pkcs8_pem(private_key_pem)
        .map_err(|e| AppError::Credential(format!("解析 EC 私钥失败: {e}")))?;
    let timestamp = chrono::Utc::now().timestamp();
    let nonce = hex::encode(rand::random::<[u8; 16]>());
    // 与客户端 qDe 一致:六段用【换行符】连接(客户端 `[m,p,id,rt,ts,nonce].join("\n")`,
    // 反引号模板字符串跨行),不是空格。分隔符错了服务端验签失败 → 20403 设备不匹配。
    let message = format!(
        "{method}\n{path}\n{client_id}\n{refresh_token}\n{timestamp}\n{nonce}"
    );
    let signature: p256::ecdsa::Signature = signing_key.sign(message.as_bytes());
    let sig_b64 = general_purpose::STANDARD.encode(signature.to_der().as_bytes());
    Ok((sig_b64, timestamp, nonce))
}

/// 宽松解析过期时间为毫秒时间戳(ExchangeToken 响应用,解析失败返回 0)。
/// 兼容两种格式:ISO 字符串(客户端登录响应)与数字毫秒(刷新/轮换响应,如 1789900046482)。
fn parse_exp_ms(v: Option<&Value>) -> i64 {
    let Some(v) = v else {
        return 0;
    };
    if let Some(n) = v.as_i64() {
        return n;
    }
    if let Some(s) = v.as_str() {
        return chrono::DateTime::parse_from_rfc3339(s)
            .map(|d| d.timestamp_millis())
            .unwrap_or(0);
    }
    0
}

/// 调用 TRAE 客户端 ExchangeToken 接口刷新 access token(自动续期,解决账号约 7 天过期)。
/// 使用 sync 后的实例真实设备身份发起刷新——服务端要求"授权设备 == 换 token 设备"，
/// 多开签到设备(checkin_device_id)仅用于签到请求头,不参与刷新。
async fn exchange_token_refresh(
    cred: &Credential,
    client: &reqwest::Client,
) -> AppResult<Credential> {
    if cred.private_key_pem.is_empty() || cred.refresh_token.is_empty() {
        return Err(AppError::Credential(
            "凭据缺少 refresh_token 或签名密钥,无法自动刷新,请重新登录该账号".into(),
        ));
    }
    let host = if cred.host.is_empty() {
        "https://api.trae.cn".to_string()
    } else {
        cred.host.clone()
    };
    let url = format!("{host}{EXCHANGE_TOKEN_PATH}");
    let client_id = CLIENT_ID_SOLO;
    let (signature, timestamp, nonce) = device_proof(
        "POST",
        EXCHANGE_TOKEN_PATH,
        client_id,
        &cred.refresh_token,
        &cred.private_key_pem,
    )?;
    let device_info = json!({
        "DeviceID": cred.device_id,
        "MachineID": cred.machine_id,
        "PlatformCode": "SOLO_PC",
        "DeviceType": "PC",
        "DeviceName": DEVICE_NAME,
        "DeviceModel": DEVICE_MODEL,
        "ClientVersion": IDE_VERSION,
        "DevicePublicKey": cred.public_key_pem,
        "DeviceBrand": DEVICE_BRAND,
        "DeviceCPU": DEVICE_CPU,
        "OSInfo": OS_INFO,
        "OSVersion": OS_VERSION,
    });
    let body = json!({
        "ClientID": client_id,
        "ClientSecret": "",
        "RefreshToken": cred.refresh_token,
        "DeviceInfo": device_info,
        "DeviceProof": {
            "Signature": signature,
            "Timestamp": timestamp,
            "Nonce": nonce,
        },
        "IDEVersion": IDE_VERSION,
    });
    let resp = client
        .post(&url)
        .header(reqwest::header::CONTENT_TYPE, "application/json")
        .header("x-cloudide-token", &cred.token)
        .json(&body)
        .send()
        .await
        .map_err(|e| AppError::Network(format!("ExchangeToken 请求失败: {e}")))?;
    let status = resp.status();
    let data: Value = resp
        .json()
        .await
        .map_err(|e| AppError::Network(format!("ExchangeToken 响应解析失败: {e}")))?;
    // 服务端错误码(客户端约定:这些码表示刷新令牌失效,需重新登录)
    if let Some(code) = data
        .pointer("/ResponseMetadata/Error/Code")
        .and_then(|v| v.as_str())
    {
        let msg = data
            .pointer("/ResponseMetadata/Error/Message")
            .and_then(|v| v.as_str())
            .unwrap_or("");
        if REFRESH_TOKEN_INVALID_CODES.contains(&code) {
            return Err(AppError::Credential(format!(
                "刷新令牌已失效({code}): {msg} [dev={} machine={}]",
                cred.device_id, cred.machine_id
            )));
        }
        return Err(AppError::Credential(format!(
            "ExchangeToken 被服务端拒绝: code={code} msg={msg}"
        )));
    }
    if !status.is_success() {
        return Err(AppError::Network(format!(
            "ExchangeToken 返回 HTTP {status}: {data}"
        )));
    }
    let result = data
        .get("Result")
        .ok_or_else(|| AppError::Credential("ExchangeToken 响应缺少 Result".into()))?;
    let token = result
        .get("Token")
        .and_then(|v| v.as_str())
        .filter(|s| !s.is_empty())
        .ok_or_else(|| AppError::Credential("ExchangeToken 响应缺少新 Token".into()))?;
    let refresh_token = result
        .get("RefreshToken")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();
    // 与客户端 WDe 一致:TokenExpireAt 已过时若给了 TokenExpireDuration(毫秒)则用 now+duration
    let token_expire_at = parse_exp_ms(result.get("TokenExpireAt"));
    let duration_ms = result
        .get("TokenExpireDuration")
        .and_then(|v| v.as_i64())
        .unwrap_or(0);
    let now = now_ms();
    let expires_at = if token_expire_at > 0 && now > token_expire_at && duration_ms > 0 {
        now + duration_ms
    } else {
        token_expire_at
    };
    let refresh_expires_at = parse_exp_ms(result.get("RefreshExpireAt"));
    let mut fresh = cred.clone();
    fresh.token = token.to_string();
    fresh.refresh_token = refresh_token;
    fresh.expires_at = expires_at;
    fresh.refresh_expires_at = refresh_expires_at;
    Ok(fresh)
}

/// 今日已签到时的消息文案,也是日志三态判定的依据
pub const ALREADY_CHECKED_IN: &str = "今日已签到";

/// 服务端业务码 9074:设备未在本机登记/设备冲突(device 维限额)。
/// 此时重试无意义(设备没登记多少次都不行),需用户在本机重登一次让客户端把设备登记到该账号。
pub const DEVICE_UNREGISTERED_CODE: i64 = 9074;

/// 对已知「设备未登记/冲突」错误附加可操作指引;其余错误原样返回(保留服务端原文)。
fn device_msg(raw: &str, code: Option<i64>) -> String {
    if code == Some(DEVICE_UNREGISTERED_CODE) {
        format!(
                    "设备未在本机登记或设备冲突(9074)，签到被服务端拒绝。请先在本机打开该账号的 TRAE 实例登录一次刷新设备登记后再签到。原由：{raw}"
                )
    } else {
        raw.to_string()
    }
}
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

/// 提取服务端响应中的业务错误码(如 9074 设备忙 / 9095 当日已签),无则 None
fn extract_error_code(data: Option<&Value>) -> Option<i64> {
    let d = data?;
    d.get("code").and_then(|v| v.as_i64()).or_else(|| {
        d.get("code")
            .and_then(|v| v.as_str())
            .and_then(|s| s.parse::<i64>().ok())
    })
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
    client: &reqwest::Client,
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
    // 自动刷新:token 已过期或即将过期,且刷新令牌(refresh_token)仍有效时,主动调
    // ExchangeToken 续期,解决账号约 7 天过期后需手动打开 TRAE 刷新/重登的问题。
    // 设备:用 sync 后的实例真实设备发起刷新("授权设备 == 换 token 设备"),
    // 多开签到设备(checkin_device_id)仅用于签到请求头,不参与刷新。
    if !cred.refresh_token.is_empty()
        && cred.refresh_expires_at > now
        && cred.expires_at <= now + AUTO_REFRESH_BEFORE_MS
    {
        match exchange_token_refresh(&cred, client).await {
            Ok(fresh) => {
                // 回写实例目录(客户端下次启动免重新登录)与应用快照
                crate::trae_instance::write_back_auth_to_instance(account, &fresh);
                persist_credential(state, account, &fresh);
                cred = fresh;
            }
            Err(e) => {
                // token 已过期且刷新失败:终止签到,给出明确指引
                if cred.expires_at <= now {
                    mark_credential_expired(state, &account.id);
                    return Err(AppError::Credential(format!(
                        "自动刷新 token 失败,请重新登录该账号。原因: {e}"
                    )));
                }
                // 仅临近过期:刷新失败不阻断,继续用当前 token 签到(剩余有效期可能不足,签到失败自会提示)
            }
        }
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

/// 手动刷新账号凭证:回读实例目录最新凭据;token 已过期/临近过期且有刷新令牌时,主动调
/// ExchangeToken 续期(自动刷新,解决约 7 天过期需手动重登),采纳后回写实例目录与应用存储。
pub async fn refresh_account_credential(
    account: &Account,
    client: &reqwest::Client,
    state: &AppState,
) -> AppResult<Credential> {
    let encrypted = account
        .encrypted_credential
        .as_ref()
        .ok_or_else(|| AppError::Credential("该账号尚未导入 TRAE 桌面凭证".into()))?;
    let cred = decrypt_credential(encrypted)?;
    // 回读实例目录:目录凭据未过期则采纳(修复快照被误标 expired 后无法自愈的问题)
    let (synced, adopted) = crate::trae_instance::sync_credential_from_instance(account, &cred);
    if synced.expires_at > now_ms() || adopted {
        persist_credential(state, account, &synced);
    }
    let now = now_ms();
    // 自动刷新:token 已过期/临近过期且刷新令牌仍有效 → ExchangeToken 续期并回写
    if !synced.refresh_token.is_empty()
        && synced.refresh_expires_at > now
        && synced.expires_at <= now + AUTO_REFRESH_BEFORE_MS
    {
        let fresh = exchange_token_refresh(&synced, client).await?;
        crate::trae_instance::write_back_auth_to_instance(account, &fresh);
        persist_credential(state, account, &fresh);
        return Ok(fresh);
    }
    if synced.expires_at <= now {
        mark_credential_expired(state, &account.id);
        Err(AppError::Credential(
            "token 已过期，请打开该账号的 TRAE 实例（TRAE 会自动刷新），刷新后重试".into(),
        ))
    } else {
        Ok(synced)
    }
}

/// 强制刷新账号 token(手动调试/续期按钮):无论当前 token 是否过期,一律调 ExchangeToken
/// 续期并回写实例目录与应用存储。返回详细结果(新有效期/设备信息),便于定位刷新链路问题
/// (如 20403 设备不匹配时,错误信息会带上 code/msg/设备 ID)。
pub async fn force_refresh_account_token(
    account: &Account,
    client: &reqwest::Client,
    state: &AppState,
) -> AppResult<Value> {
    let encrypted = account
        .encrypted_credential
        .as_ref()
        .ok_or_else(|| AppError::Credential("该账号尚未导入 TRAE 桌面凭证".into()))?;
    let mut cred = decrypt_credential(encrypted)?;
    // 回读实例目录最新凭据:客户端可能已自行刷新过,用最新 refresh_token 发起刷新
    let (synced, adopted) = crate::trae_instance::sync_credential_from_instance(account, &cred);
    if synced.expires_at > now_ms() || adopted {
        persist_credential(state, account, &synced);
        cred = synced;
    }
    let fresh = exchange_token_refresh(&cred, client).await?;
    crate::trae_instance::write_back_auth_to_instance(account, &fresh);
    persist_credential(state, account, &fresh);
    Ok(json!({
        "success": true,
        "message": "token 刷新成功",
        "expiresAt": fresh.expires_at,
        "refreshExpiresAt": fresh.refresh_expires_at,
        "deviceId": cred.device_id,
        "machineId": cred.machine_id,
    }))
}

/// 单账号签到(桌面凭据模式):凭据由 get_valid_credential 回读自 TRAE 实例目录,
/// 应用不主动刷新 token。鉴权失败仅提示,不做网络刷新。
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
                error_code: None,
                points: None,
points_extra: None,
                trace: None,
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
        error_code: None,
        points: None,
points_extra: None,
        trace: None,
    }
}

/// 强制签到(直连领取):跳过 status 预检,直接 POST claim。
/// 场景:状态预检异常/风控但用户仍想直接尝试领取,或一次性手工补签。
/// 复用与 checkin_by_desktop 一致的凭据回读与鉴权失败降级。
pub async fn force_checkin_by_desktop(
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
                error_code: None,
                points: None,
points_extra: None,
                trace: None,
            }
        }
    };
    let (result, auth_failed) = force_checkin_once(&cred, client).await;
    if !auth_failed {
        return result;
    }
    mark_credential_expired(state, &account.id);
    CheckinResult {
        success: false,
        message: "签到接口鉴权失败(token 已失效)，请打开该账号的 TRAE 实例让客户端刷新后再试".into(),
        error_code: None,
        points: None,
points_extra: None,
        trace: None,
    }
}

/// 直接 POST claim(不做 status 预检),采集 claim 一步出入参到 trace。
/// 返回 (结果, 是否疑似鉴权失败)。via Handle,不回读状态。
async fn force_checkin_once(cred: &Credential, client: &reqwest::Client) -> (CheckinResult, bool) {
    let headers = auth_headers(cred);
    let host = &cred.host;
    let mut trace: Vec<ApiTraceEntry> = Vec::new();

    let claim_url = format!("{}{}", host, CLAIM_PATH);
    let claim_headers_json = headers_to_value(&headers);
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
                    message: format!("TRAE 桌面端强制签到失败: {e}"),
                    error_code: None,
                    points: None,
points_extra: None,
                    trace: Some(trace_to_string(&trace)),
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
                    error_code: None,
                    points: None,
points_extra: None,
                    trace: Some(trace_to_string(&trace)),
                },
                false,
            )
        }
    };
    trace.push(ApiTraceEntry {
        name: "claim",
        method: "POST",
        url: claim_url,
        headers: claim_headers_json,
        request: json!({}),
        http_status,
        response: claim_data.clone(),
    });
    eprintln!(
        "[checkin] 强制领取 HTTP {} {} 凭据={}",
        http_status,
        claim_data,
        cred_fingerprint(cred)
    );

    if is_auth_failure(http_status, Some(&claim_data)) {
        return (
            CheckinResult {
                success: false,
                message: "签到接口鉴权失败(token 已失效)".into(),
                error_code: extract_error_code(Some(&claim_data)),
                points: None,
points_extra: None,
                trace: Some(trace_to_string(&trace)),
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
                Some(s) if s == "success" => "强制签到成功".to_string(),
                Some(s) => s.to_string(),
                None => "强制签到成功".to_string(),
            }
        };
        // 本次获得积分拆为 (每日基础 credits, 额外加成 extra_credits),claim 响应缺失时回退 status 步骤。
        let (points, points_extra) = extract_earned_points(&claim_data, &trace)
            .map(|(b, e)| (Some(b), Some(e)))
            .unwrap_or((None, None));
        (
            CheckinResult {
                success: true,
                message: msg,
                error_code: extract_error_code(Some(&claim_data)),
                points,
                points_extra,
                trace: Some(trace_to_string(&trace)),
            },
            false,
        )
    } else {
        let code = extract_error_code(Some(&claim_data));
        let raw_msg = claim_data
            .get("message")
            .and_then(|v| v.as_str())
            .or_else(|| claim_data.get("msg").and_then(|v| v.as_str()))
            .unwrap_or("签到失败")
            .to_string();
        (
            CheckinResult {
                success: false,
                message: device_msg(&raw_msg, code),
                error_code: code,
                points: None,
points_extra: None,
                trace: Some(trace_to_string(&trace)),
            },
            false,
        )
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

/// 一步接口调用记录:URL、方法、请求头、请求出入参与响应
struct ApiTraceEntry {
    name: &'static str,
    method: &'static str,
    url: String,
    headers: Value,
    request: Value,
    http_status: u16,
    response: Value,
}

impl ApiTraceEntry {
    fn to_json(&self) -> Value {
        json!({
            "name": self.name,
            "method": self.method,
            "url": self.url,
            "headers": self.headers,
            "request": self.request,
            "httpStatus": self.http_status,
            "response": self.response,
        })
    }
}

/// 将请求头序列化为 JSON,敏感值(token)脱敏、保留设备码等诊断关键信息
fn headers_to_value(headers: &reqwest::header::HeaderMap) -> Value {
    let mut map = serde_json::Map::new();
    for (k, v) in headers.iter() {
        let key = k.as_str().to_string();
        let value = match v.to_str() {
            Ok(s) => {
                // token 脱敏:保留前缀与尾部,中间打码
                if key.eq_ignore_ascii_case("authorization") || key == "cloud-ide-jwt" {
                    if s.len() > 12 {
                        format!("{}****{}", &s[..6], &s[s.len() - 4..])
                    } else {
                        "***".to_string()
                    }
                } else {
                    s.to_string()
                }
            }
            Err(_) => "<binary>".to_string(),
        };
        map.insert(key, Value::String(value));
    }
    Value::Object(map)
}

fn trace_to_string(entries: &[ApiTraceEntry]) -> String {
    let arr: Vec<Value> = entries.iter().map(ApiTraceEntry::to_json).collect();
    serde_json::to_string_pretty(&arr).unwrap_or_else(|_| "[]".to_string())
}

/// 提取本次签到获得的积分,返回 (每日基础 credits, 额外加成 extra_credits)。
/// claim 响应缺失该字段时,回退读取流程中 status 步骤的响应(每次签到都会先查 status)。
fn extract_earned_points(claim: &Value, trace: &[ApiTraceEntry]) -> Option<(i64, i64)> {
    let read = |v: &Value| -> Option<(i64, i64)> {
        let data = v.get("data");
        let base = data
            .and_then(|d| d.get("credits").or_else(|| d.get("points")))
            .or_else(|| v.get("credits").or_else(|| v.get("points")))
            .and_then(|x| x.as_i64());
        base.map(|b| {
            let extra = data
                .and_then(|d| d.get("extra_credits"))
                .or_else(|| v.get("extra_credits"))
                .and_then(|x| x.as_i64())
                .unwrap_or(0);
            (b, extra)
        })
    };
    if let Some(p) = read(claim) {
        return Some(p);
    }
    trace.iter().find(|s| s.name == "status").and_then(|s| read(&s.response))
}

#[cfg(test)]
mod earned_points_tests {
    use super::*;

    #[test]
    fn extract_earned_points_from_status_fallback() {
        // claim 响应无 credits 时,回退读取 status 步骤响应(真实账号响应样例)
        let claim = serde_json::json!({ "code": 0, "message": "success" });
        let trace = [
            ApiTraceEntry {
                name: "status",
                method: "POST",
                url: "https://api.trae.cn/trae/api/v1/checkin/status".into(),
                headers: json!({}),
                request: json!({}),
                http_status: 200,
                response: serde_json::json!({
                    "checked_in": true,
                    "code": 0,
                    "credits": 150,
                    "did_checked_in": true,
                    "enable": true,
                    "extra_credits": 50,
                    "message": "success"
                }),
            },
            ApiTraceEntry {
                name: "claim",
                method: "POST",
                url: "https://api.trae.cn/trae/api/v1/checkin/claim".into(),
                headers: json!({}),
                request: json!({}),
                http_status: 200,
                response: claim.clone(),
            },
        ];
        assert_eq!(extract_earned_points(&claim, &trace), Some((150, 50)));
    }

    #[test]
    fn extract_earned_points_prefers_claim() {
        // claim 响应带 credits 时以 claim 为准,不读 status
        let claim = serde_json::json!({ "code": 0, "credits": 120, "extra_credits": 30 });
        let trace = [];
        assert_eq!(extract_earned_points(&claim, &trace), Some((120, 30)));
    }

    #[test]
    fn extract_earned_points_extra_zero_when_missing() {
        // 响应只有 credits 没有 extra_credits 时,额外加成为 0
        let claim = serde_json::json!({ "code": 0, "credits": 100 });
        let trace = [];
        assert_eq!(extract_earned_points(&claim, &trace), Some((100, 0)));
    }

    #[test]
    fn extract_earned_points_none_when_missing() {
        let claim = serde_json::json!({ "code": 0, "message": "success" });
        let trace = [];
        assert_eq!(extract_earned_points(&claim, &trace), None);
    }
}

/// 用给定凭据执行一次签到(查询状态 -> 领取)。返回 (结果, 是否疑似鉴权失败)。
/// 同时采集每一步接口的出入参(请求/响应),写入结果的 trace 字段供前端展示。
async fn checkin_once(cred: &Credential, client: &reqwest::Client) -> (CheckinResult, bool) {
    let headers = auth_headers(cred);
    let host = &cred.host;
    let mut trace: Vec<ApiTraceEntry> = Vec::new();

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
                    error_code: None,
                    points: None,
points_extra: None,
                    trace: None,
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
                    error_code: None,
                    points: None,
points_extra: None,
                    trace: None,
                },
                false,
            )
        }
    };
    trace.push(ApiTraceEntry {
        name: "status",
        method: "POST",
        url: status_url,
        headers: headers_to_value(&headers),
        request: json!({}),
        http_status,
        response: status_data.clone(),
    });
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
                error_code: extract_error_code(Some(&status_data)),
                points: None,
points_extra: None,
                trace: Some(trace_to_string(&trace)),
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
                error_code: extract_error_code(Some(&status_data)),
                points: None,
points_extra: None,
                trace: Some(trace_to_string(&trace)),
            },
            false,
        );
    }
    if !api_succeeded(&status_data) {
        let code = extract_error_code(Some(&status_data));
        let raw_msg = status_data
            .get("message")
            .and_then(|v| v.as_str())
            .or_else(|| status_data.get("msg").and_then(|v| v.as_str()))
            .unwrap_or("无法获取 TRAE 签到状态")
            .to_string();
        return (
            CheckinResult {
                success: false,
                message: device_msg(&raw_msg, code),
                error_code: code,
                points: None,
points_extra: None,
                trace: Some(trace_to_string(&trace)),
            },
            false,
        );
    }

    // 2. 领取签到
    let claim_url = format!("{}{}", host, CLAIM_PATH);
    let claim_headers_json = headers_to_value(&headers);
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
                    error_code: None,
                    points: None,
points_extra: None,
                    trace: Some(trace_to_string(&trace)),
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
                    error_code: None,
                    points: None,
points_extra: None,
                    trace: Some(trace_to_string(&trace)),
                },
                false,
            )
        }
    };
    trace.push(ApiTraceEntry {
        name: "claim",
        method: "POST",
        url: claim_url,
        headers: claim_headers_json,
        request: json!({}),
        http_status,
        response: claim_data.clone(),
    });
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
                error_code: extract_error_code(Some(&claim_data)),
                points: None,
points_extra: None,
                trace: Some(trace_to_string(&trace)),
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
        // 本次获得积分拆为 (每日基础 credits, 额外加成 extra_credits),claim 响应缺失时回退 status 步骤。
        let (points, points_extra) = extract_earned_points(&claim_data, &trace)
            .map(|(b, e)| (Some(b), Some(e)))
            .unwrap_or((None, None));
        (
            CheckinResult {
                success: true,
                message: msg,
                error_code: extract_error_code(Some(&claim_data)),
                points,
                points_extra,
                trace: Some(trace_to_string(&trace)),
            },
            false,
        )
    } else {
        let code = extract_error_code(Some(&claim_data));
        let raw_msg = claim_data
            .get("message")
            .and_then(|v| v.as_str())
            .or_else(|| claim_data.get("msg").and_then(|v| v.as_str()))
            .unwrap_or("签到失败")
            .to_string();
        (
            CheckinResult {
                success: false,
                message: device_msg(&raw_msg, code),
                error_code: code,
                points: None,
points_extra: None,
                trace: Some(trace_to_string(&trace)),
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
    let cred = match get_valid_credential(account, client, state).await {
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
    perform_checkin_inner(account, client, state, false).await
}

/// 强制签到(直连领取,跳过 status 预检),同样回写最后一次签到与日志。
pub async fn perform_force_checkin(
    account: &Account,
    client: &reqwest::Client,
    state: &AppState,
) -> CheckinResult {
    perform_checkin_inner(account, client, state, true).await
}

async fn perform_checkin_inner(
    account: &Account,
    client: &reqwest::Client,
    state: &AppState,
    force_direct: bool,
) -> CheckinResult {
    let result = if force_direct {
        force_checkin_by_desktop(account, client, state).await
    } else {
        checkin_by_desktop(account, client, state).await
    };

    let now = now_ms();
    // 余额只用服务端真实查询值,不做本地 +gained 累加估算,避免漂移/虚数
    let (real_points, points_update) = if result.success {
        let pr = get_total_points(account, client, state).await;
        (pr.total_points, points_update_json(&pr))
    } else {
        (None, serde_json::Value::Null)
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
    let mut acc_upd = json!({
        "lastCheckinAt": now,
        "lastCheckinResult": status,
        "lastCheckinMessage": result.message,
        "lastCheckinTrace": result.trace,
    });
    if let Some(tp) = real_points {
        acc_upd["points"] = json!(tp);
        // 合并 points_details / points_response 等真实积分详情
        if let serde_json::Value::Object(pu) = &points_update {
            if let Some(obj) = acc_upd.as_object_mut() {
                for (k, v) in pu {
                    if k != "points" {
                        obj.insert(k.clone(), v.clone());
                    }
                }
            }
        }
    }
    data.update_account(&account.id, acc_upd);
    data.add_log(CheckinLog {
        id: generate_id(),
        account_id: account.id.clone(),
        account_name: account.name.clone(),
        time: now,
        result: status.into(),
        message: result.message.clone(),
        error_code: result.error_code,
        points_gained: result.points,
        points_extra: result.points_extra,
        points_balance: real_points,
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
            let mut last_code = None;
            let mut result = None;
            for attempt in 0..=retry_count {
                let r = perform_checkin(&account, client, state).await;
                let code = r.error_code;
                if r.success {
                    result = Some(r);
                    break;
                }
                last_err = r.message;
                last_code = r.error_code;
                // 设备未登记/冲突(9074):重试无意义,提前终止并给出可操作指引
                if code == Some(DEVICE_UNREGISTERED_CODE) {
                    result = Some(CheckinResult {
                        success: false,
                        message: last_err.clone(),
                        error_code: last_code,
                        points: None,
points_extra: None,
                        trace: r.trace,
                    });
                    break;
                }
                if attempt < retry_count {
                    tokio::time::sleep(Duration::from_secs(retry_delay as u64)).await;
                }
            }
            let r = match result {
                Some(r) => r,
                None => CheckinResult {
                    success: false,
                    message: last_err,
                    error_code: last_code,
                    points: None,
points_extra: None,
                    trace: None,
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

    #[test]
    fn device_proof_signature_verifies_with_public_key() {
        // 与客户端 qDe 一致:EC P-256 私钥对 message 做 ECDSA-SHA256 签名,输出 DER 编码后 base64。
        // 用公钥验证 DER 签名,确保格式正确(可被服务端验签)。
        use ecdsa::signature::Verifier;
        use p256::ecdsa::{Signature as EcdsaSignature, VerifyingKey};
        use p256::pkcs8::{
            DecodePublicKey, EncodePrivateKey, EncodePublicKey, LineEnding,
        };

        let secret = p256::SecretKey::random(&mut rand::thread_rng());
        let private_pem = secret
            .to_pkcs8_pem(LineEnding::LF)
            .unwrap()
            .to_string();
        let public_key = secret.public_key();
        let verifying: VerifyingKey = VerifyingKey::from(&public_key);
        let public_pem = verifying.to_public_key_pem(LineEnding::LF).unwrap();
        // 走真实签名入口(解析 PEM → 签名 → base64 DER)
        let (sig_b64, timestamp, nonce) = device_proof(
            "POST",
            EXCHANGE_TOKEN_PATH,
            CLIENT_ID_SOLO,
            "test-refresh-token",
            &private_pem,
        )
        .expect("device_proof 应成功");
        let message = format!(
            "POST\n{}\n{}\ntest-refresh-token\n{}\n{}",
            EXCHANGE_TOKEN_PATH, CLIENT_ID_SOLO, timestamp, nonce
        );
        let pub_key = VerifyingKey::from_public_key_pem(&public_pem).unwrap();
        let sig_bytes = general_purpose::STANDARD.decode(&sig_b64).unwrap();
        let sig = EcdsaSignature::from_der(&sig_bytes).unwrap();
        pub_key
            .verify(message.as_bytes(), &sig)
            .expect("签名应能被公钥验证(与客户端 ECDSA-SHA256 一致)");
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
            last_checkin_trace: None,
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
                account_order: vec![],
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

    /// 诊断:真实调用 ExchangeToken 刷新主账号 token(仅手动 --ignored 运行,会轮换 token)。
    /// 验证 ClientID/签名/请求体/响应解析与 TRAE 客户端一致。
    #[tokio::test]
    #[ignore]
    async fn e2e_exchange_token_refresh_real() {
        let cred = match crate::trae_auth::get_trae_desktop_credentials() {
            Ok(c) => c,
            Err(e) => {
                eprintln!("[e2e] 未读取到 TRAE 桌面凭据: {e}");
                return;
            }
        };
        eprintln!(
            "[e2e] 刷新前: host={} dev={} machine={} uid={} exp={} now={} refresh_exp={}",
            cred.host,
            cred.device_id,
            cred.machine_id,
            cred.user_id,
            cred.expires_at,
            now_ms(),
            cred.refresh_expires_at
        );
        let client = reqwest::Client::new();
        // 循环尝试不同的 DeviceID/MachineID 组合,定位 20403 设备不匹配的根因
        let candidates: Vec<(String, String)> = vec![
            (cred.device_id.clone(), cred.machine_id.clone()),
            (cred.machine_id.clone(), cred.machine_id.clone()),
            (cred.machine_id.clone(), cred.device_id.clone()),
            (cred.device_id.clone(), cred.device_id.clone()),
            (String::new(), cred.machine_id.clone()),
        ];
        let mut last_err = String::new();
        for (dev, machine) in candidates {
            let mut c = cred.clone();
            c.device_id = dev;
            c.machine_id = machine;
            match exchange_token_refresh(&c, &client).await {
                Ok(f) => {
                    eprintln!(
                        "[e2e] 刷新成功! DeviceID={} MachineID={} 新exp={} refresh_exp={}",
                        c.device_id, c.machine_id, f.expires_at, f.refresh_expires_at
                    );
                    assert!(f.token != cred.token, "token 应已轮换");
                    return;
                }
                Err(e) => {
                    eprintln!("[e2e] 失败 DeviceID={} MachineID={}: {e}", c.device_id, c.machine_id);
                    last_err = e.to_string();
                }
            }
        }
        assert!(false, "所有组合均刷新失败,最后错误: {last_err}");
    }
}
