// Tauri 命令实现。

use tauri::{AppHandle, State};

use crate::checkin;
use crate::credentials;
use crate::error::{AppError, AppResult};
use crate::models::{
    credential_status, Account, AppSettings, CheckinLog, CheckinResult, PartialAppSettings,
    PointsResult, PublicAccount,
};
use crate::scheduler;
use crate::store::{generate_id, AppState};
use crate::trae_auth;

#[tauri::command]
pub fn get_accounts(state: State<'_, AppState>) -> Vec<PublicAccount> {
    let data = state.data.lock().unwrap();
    data.get_accounts()
        .iter()
        .cloned()
        .map(PublicAccount::from)
        .collect()
}

/// 导入当前 TRAE 桌面账号:读取桌面凭据 -> DPAPI 加密 -> upsert 存储
#[tauri::command]
pub fn import_desktop_account(state: State<'_, AppState>) -> AppResult<PublicAccount> {
    let cred = trae_auth::get_trae_desktop_credentials()?;
    let encrypted = credentials::encrypt_credential(&cred)?;
    let now = now_ms();
    let status = credential_status(cred.expires_at, now);
    let account = Account {
        id: generate_id(),
        name: cred.account_name.clone(),
        cookie: String::new(),
        created_at: now,
        last_checkin_at: None,
        last_checkin_result: None,
        last_checkin_message: None,
        points: None,
        enabled: true,
        desktop_user_id: Some(cred.user_id.clone()),
        encrypted_credential: Some(encrypted),
        credential_status: Some(status.to_string()),
    };
    let mut data = state.data.lock().unwrap();
    let saved = data.upsert_desktop_account(account);
    let _ = data.save(&state.path);
    Ok(saved.into())
}

#[tauri::command]
pub fn update_account(
    id: String,
    updates: serde_json::Value,
    state: State<'_, AppState>,
) -> AppResult<PublicAccount> {
    let mut data = state.data.lock().unwrap();
    let acc = data
        .update_account(&id, updates)
        .ok_or_else(|| AppError::NotFound(id.clone()))?;
    data.save(&state.path)?;
    Ok(acc.into())
}

#[tauri::command]
pub fn delete_account(id: String, state: State<'_, AppState>) -> AppResult<bool> {
    let mut data = state.data.lock().unwrap();
    data.delete_account(&id);
    data.save(&state.path)?;
    Ok(true)
}

#[tauri::command]
pub async fn checkin_account(
    id: String,
    state: State<'_, AppState>,
    client: State<'_, reqwest::Client>,
) -> AppResult<CheckinResult> {
    let account = {
        let data = state.data.lock().unwrap();
        data.get_accounts().iter().find(|a| a.id == id).cloned()
    };
    let account = account.ok_or_else(|| AppError::NotFound(id.clone()))?;
    Ok(checkin::perform_checkin(&account, client.inner(), state.inner()).await)
}

#[tauri::command]
pub async fn checkin_all(
    state: State<'_, AppState>,
    client: State<'_, reqwest::Client>,
) -> AppResult<Vec<(PublicAccount, CheckinResult)>> {
    Ok(checkin::perform_all_checkin(client.inner(), state.inner()).await)
}

#[tauri::command]
pub async fn get_account_points(
    id: String,
    state: State<'_, AppState>,
    client: State<'_, reqwest::Client>,
) -> AppResult<PointsResult> {
    let account = {
        let data = state.data.lock().unwrap();
        data.get_accounts().iter().find(|a| a.id == id).cloned()
    };
    let account = account.ok_or_else(|| AppError::NotFound(id.clone()))?;
    let result = checkin::get_total_points(&account, client.inner(), state.inner()).await;
    if let (true, Some(tp)) = (result.success, result.total_points) {
        let mut data = state.data.lock().unwrap();
        data.update_account(&id, serde_json::json!({ "points": tp }));
        let _ = data.save(&state.path);
    }
    Ok(result)
}

#[tauri::command]
pub fn get_logs(limit: Option<usize>, state: State<'_, AppState>) -> Vec<CheckinLog> {
    let data = state.data.lock().unwrap();
    data.get_logs(limit.unwrap_or(100))
}

#[tauri::command]
pub fn clear_logs(state: State<'_, AppState>) -> AppResult<bool> {
    let mut data = state.data.lock().unwrap();
    data.clear_logs();
    data.save(&state.path)?;
    Ok(true)
}

#[tauri::command]
pub fn get_settings(state: State<'_, AppState>) -> AppSettings {
    let data = state.data.lock().unwrap();
    data.get_settings()
}

#[tauri::command]
pub fn save_settings(
    settings: PartialAppSettings,
    state: State<'_, AppState>,
    app: AppHandle,
) -> AppResult<AppSettings> {
    let mut data = state.data.lock().unwrap();
    let s = data.save_settings(settings);
    data.save(&state.path)?;
    drop(data);
    // 设置变更后重启定时任务
    scheduler::start_scheduler(app);
    Ok(s)
}

#[tauri::command]
pub fn start_scheduler(app: AppHandle) -> bool {
    scheduler::start_scheduler(app);
    true
}

#[tauri::command]
pub fn stop_scheduler(app: AppHandle) -> bool {
    scheduler::stop_scheduler(&app);
    true
}

#[tauri::command]
pub fn get_next_run_time(app: AppHandle) -> Option<String> {
    scheduler::get_next_run_time(&app)
}

pub fn now_ms() -> i64 {
    use std::time::{SystemTime, UNIX_EPOCH};
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}
