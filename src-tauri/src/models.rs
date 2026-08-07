// 数据模型,镜像 electron/types.ts。serde camelCase 与前端字段对齐。

use serde::{Deserialize, Serialize};

/// 账号(完整,含加密凭据,仅用于本地存储与内部逻辑)
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Account {
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub cookie: String,
    pub created_at: i64,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub last_checkin_at: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub last_checkin_result: Option<String>, // "success" | "failed" | "pending"
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub last_checkin_message: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub points: Option<i64>,
    pub enabled: bool,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub desktop_user_id: Option<String>,
    /// DPAPI 加密后的 Credential JSON(base64)。仅在本地存储中流转。
    #[serde(default)]
    pub encrypted_credential: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub credential_status: Option<String>, // "valid" | "expiring" | "expired"
    /// 多开实例的独立 data-dir 路径(%APPDATA%\TRAE SOLO CN_{userId}),首次多开时生成并回写
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub data_dir: Option<String>,
    /// 多开实例的机器码(每实例独立 UUID,不动系统注册表)
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub machine_id: Option<String>,
}

/// 账号(脱敏,返回前端)。不含加密凭据。
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PublicAccount {
    pub id: String,
    pub name: String,
    pub cookie: String,
    pub created_at: i64,
    pub last_checkin_at: Option<i64>,
    pub last_checkin_result: Option<String>,
    pub last_checkin_message: Option<String>,
    pub points: Option<i64>,
    pub enabled: bool,
    pub desktop_user_id: Option<String>,
    pub credential_status: Option<String>,
    pub data_dir: Option<String>,
    pub machine_id: Option<String>,
}

impl From<Account> for PublicAccount {
    fn from(a: Account) -> Self {
        Self {
            id: a.id,
            name: a.name,
            cookie: a.cookie,
            created_at: a.created_at,
            last_checkin_at: a.last_checkin_at,
            last_checkin_result: a.last_checkin_result,
            last_checkin_message: a.last_checkin_message,
            points: a.points,
            enabled: a.enabled,
            desktop_user_id: a.desktop_user_id,
            credential_status: a.credential_status,
            data_dir: a.data_dir,
            machine_id: a.machine_id,
        }
    }
}

/// 签到日志
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CheckinLog {
    pub id: String,
    pub account_id: String,
    pub account_name: String,
    pub time: i64,
    pub result: String, // "success" | "failed"
    pub message: String,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub points_gained: Option<i64>,
}

/// 应用设置(仅 desktop 模式相关字段)
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AppSettings {
    pub auto_checkin: bool,
    pub checkin_time: String, // "HH:mm"
    pub retry_count: u32,
    pub retry_delay: u32, // 秒
    pub notify_on_success: bool,
    pub notify_on_failed: bool,
    pub launch_at_login: bool,
}

impl Default for AppSettings {
    fn default() -> Self {
        Self {
            auto_checkin: false,
            checkin_time: "08:00".into(),
            retry_count: 3,
            retry_delay: 60,
            notify_on_success: true,
            notify_on_failed: true,
            launch_at_login: false,
        }
    }
}

/// 设置的部分更新(前端 saveSettings 传 partial)
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PartialAppSettings {
    #[serde(default)]
    pub auto_checkin: Option<bool>,
    #[serde(default)]
    pub checkin_time: Option<String>,
    #[serde(default)]
    pub retry_count: Option<u32>,
    #[serde(default)]
    pub retry_delay: Option<u32>,
    #[serde(default)]
    pub notify_on_success: Option<bool>,
    #[serde(default)]
    pub notify_on_failed: Option<bool>,
    #[serde(default)]
    pub launch_at_login: Option<bool>,
}

impl AppSettings {
    /// 用 partial 覆盖现有设置
    pub fn merge(&mut self, p: PartialAppSettings) {
        if let Some(v) = p.auto_checkin {
            self.auto_checkin = v;
        }
        if let Some(v) = p.checkin_time {
            self.checkin_time = v;
        }
        if let Some(v) = p.retry_count {
            self.retry_count = v;
        }
        if let Some(v) = p.retry_delay {
            self.retry_delay = v;
        }
        if let Some(v) = p.notify_on_success {
            self.notify_on_success = v;
        }
        if let Some(v) = p.notify_on_failed {
            self.notify_on_failed = v;
        }
        if let Some(v) = p.launch_at_login {
            self.launch_at_login = v;
        }
    }
}

/// TRAE 桌面凭证(内部,不序列化到前端)
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Credential {
    pub token: String,
    pub refresh_token: String,
    pub expires_at: i64, // 毫秒时间戳
    pub refresh_expires_at: i64,
    pub device_id: String,
    pub machine_id: String,
    pub private_key_pem: String,
    pub public_key_pem: String,
    pub user_id: String,
    pub account_name: String,
    pub host: String,
    /// 账号邮箱(多开写 storage.json 用,从桌面凭据 account 提取)
    #[serde(default)]
    pub email: Option<String>,
    /// 头像 URL(多开写 storage.json 用)
    #[serde(default)]
    pub avatar_url: Option<String>,
    /// 区域("CN"/"SG",多开写 storage.json 用,从 userRegion 或 host 推断)
    #[serde(default)]
    pub region: Option<String>,
}

/// 签到结果
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CheckinResult {
    pub success: bool,
    pub message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub points: Option<i64>,
}

/// 积分查询结果
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PointsResult {
    pub success: bool,
    pub message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub total_points: Option<i64>,
}

/// 多开启动结果
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LaunchResult {
    pub data_dir: String,
    pub machine_id: String,
    pub launched: bool,
}

/// 凭证状态
pub fn credential_status(expires_at: i64, now_ms: i64) -> &'static str {
    if expires_at <= now_ms {
        "expired"
    } else if expires_at - now_ms <= 15 * 60 * 1000 {
        "expiring"
    } else {
        "valid"
    }
}
