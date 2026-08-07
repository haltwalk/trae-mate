// Tauri 命令实现。

use tauri::{AppHandle, Manager, State};

use crate::checkin;
use crate::credentials;
use crate::error::{AppError, AppResult};
use crate::models::{
    credential_status, Account, AppSettings, CheckinLog, CheckinResult, LaunchResult,
    PartialAppSettings, PointsResult, PublicAccount,
};
use crate::scheduler;
use crate::store::{generate_id, AppState};
use crate::trae_auth;
use crate::trae_instance;
use crate::trae_machine;

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
        data_dir: None,
        machine_id: None,
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
pub fn delete_account(id: String, state: State<'_, AppState>, app: AppHandle) -> AppResult<bool> {
    let mut data = state.data.lock().unwrap();
    data.delete_account(&id);
    data.save(&state.path)?;
    drop(data);
    // 刷新托盘菜单:移除已删除账号的快捷聚焦项
    let _ = crate::rebuild_tray_menu(&app);
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

/// 多开启动:用账号凭据加密写入独立 data-dir,启动免登录的 TRAE 实例
#[tauri::command]
pub fn launch_account_multi(
    id: String,
    state: State<'_, AppState>,
    app: AppHandle,
) -> AppResult<LaunchResult> {
    // 1. 取账号
    let account = {
        let data = state.data.lock().unwrap();
        data.get_accounts().iter().find(|a| a.id == id).cloned()
    }
    .ok_or_else(|| AppError::NotFound(id.clone()))?;

    // 2. 解密凭据
    let encrypted = account
        .encrypted_credential
        .as_ref()
        .ok_or_else(|| AppError::Launch("该账号尚未导入 TRAE 桌面凭证,无法多开".into()))?;
    let cred = credentials::decrypt_credential(encrypted)?;

    // 3. 解析 exe 路径(已保存或自动扫描)
    let config_dir = app
        .path()
        .app_config_dir()
        .map_err(|e| AppError::Launch(format!("获取配置目录失败: {e}")))?;
    let exe_path = trae_machine::resolve_trae_path(&config_dir)?;

    // 4. 启动多开(文件 I/O + 进程启动)
    let result = trae_instance::launch_multi(&account, &cred, &exe_path)?;

    // 5. 回写 data_dir/machine_id(首次多开时绑定,后续复用同一目录与机器码)
    {
        let mut data = state.data.lock().unwrap();
        data.update_account(
            &id,
            serde_json::json!({
                "dataDir": result.data_dir,
                "machineId": result.machine_id,
            }),
        );
        let _ = data.save(&state.path);
    }

    // 6. 刷新托盘菜单:新实例已运行,加入快捷聚焦列表
    let _ = crate::rebuild_tray_menu(&app);

    Ok(result)
}

/// 获取已保存的 TRAE exe 路径(未设置返回 None)
#[tauri::command]
pub fn get_trae_exe_path(app: AppHandle) -> AppResult<Option<String>> {
    let config_dir = app
        .path()
        .app_config_dir()
        .map_err(|e| AppError::Launch(format!("获取配置目录失败: {e}")))?;
    trae_machine::get_saved_trae_path(&config_dir)
}

/// 手动设置 TRAE exe 路径
#[tauri::command]
pub fn set_trae_exe_path(path: String, app: AppHandle) -> AppResult<()> {
    let config_dir = app
        .path()
        .app_config_dir()
        .map_err(|e| AppError::Launch(format!("获取配置目录失败: {e}")))?;
    trae_machine::save_trae_path(&config_dir, &path)
}

/// 自动扫描 TRAE exe 路径并保存
#[tauri::command]
pub fn scan_trae_exe_path(app: AppHandle) -> AppResult<String> {
    let scanned = trae_machine::scan_trae_exe_path()?;
    let config_dir = app
        .path()
        .app_config_dir()
        .map_err(|e| AppError::Launch(format!("获取配置目录失败: {e}")))?;
    let _ = trae_machine::save_trae_path(&config_dir, &scanned);
    Ok(scanned)
}

/// 查询账号多开实例是否在运行
#[tauri::command]
pub fn is_account_instance_running(id: String, state: State<'_, AppState>) -> AppResult<bool> {
    let data_dir = {
        let data = state.data.lock().unwrap();
        data.get_accounts()
            .iter()
            .find(|a| a.id == id)
            .and_then(|a| a.data_dir.clone())
    };
    match data_dir {
        Some(d) if !d.is_empty() => Ok(trae_machine::is_instance_running(&d).0),
        _ => Ok(false),
    }
}

/// 聚焦账号多开实例的窗口(提到前台)
#[tauri::command]
pub fn focus_account_instance(id: String, state: State<'_, AppState>) -> AppResult<()> {
    let data_dir = {
        let data = state.data.lock().unwrap();
        data.get_accounts()
            .iter()
            .find(|a| a.id == id)
            .and_then(|a| a.data_dir.clone())
    };
    match data_dir {
        Some(d) if !d.is_empty() => trae_machine::focus_instance_window(&d),
        _ => Err(AppError::Launch("该账号尚未多开,无实例可聚焦".into())),
    }
}
