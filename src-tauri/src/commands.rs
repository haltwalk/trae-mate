// Tauri 命令实现。

use std::path::PathBuf;

use tauri::{AppHandle, Emitter, Manager, State};

use crate::checkin;
use crate::credentials;
use crate::error::{AppError, AppResult};
use crate::models::{
    credential_status, Account, AppSettings, CheckinLog, CheckinResult, Credential, LaunchResult,
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
    // 关闭该账号的工具实例(若在运行),数据目录保留(用户可日后通过"扫描已有多开目录"重新导入)
    {
        let data = state.data.lock().unwrap();
        if let Some(acc) = data.get_accounts().iter().find(|a| a.id == id) {
            if let Some(dir) = acc.data_dir.as_deref().filter(|s| !s.is_empty()) {
                if trae_machine::is_instance_running(dir).0 {
                    let _ = trae_machine::kill_instance(dir);
                }
            }
        }
    }
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
pub fn get_settings(state: State<'_, AppState>, app: AppHandle) -> AppSettings {
    let mut s = {
        let data = state.data.lock().unwrap();
        data.get_settings()
    };
    // 用插件真实状态覆盖(反映系统设置/任务管理器等外部修改)
    use tauri_plugin_autostart::ManagerExt;
    if let Ok(enabled) = app.autolaunch().is_enabled() {
        s.launch_at_login = enabled;
    }
    s
}

#[tauri::command]
pub fn save_settings(
    settings: PartialAppSettings,
    state: State<'_, AppState>,
    app: AppHandle,
) -> AppResult<AppSettings> {
    let has_launch = settings.launch_at_login.is_some();
    let mut data = state.data.lock().unwrap();
    let s = data.save_settings(settings);
    data.save(&state.path)?;
    drop(data);
    // 同步开机自启插件状态(仅在本次提交了该字段时)
    if has_launch {
        use tauri_plugin_autostart::ManagerExt;
        let mgr = app.autolaunch();
        let currently = mgr.is_enabled().unwrap_or(false);
        if s.launch_at_login && !currently {
            let _ = mgr.enable();
        } else if !s.launch_at_login && currently {
            let _ = mgr.disable();
        }
    }
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

/// 启动账号独立实例(前端命令与托盘点击共用核心逻辑):
/// 解密凭据 -> 解析 exe -> 启动 -> 回写 data_dir/machine_id -> 刷新托盘菜单
pub fn launch_account_by_id(app: &AppHandle, account_id: &str) -> AppResult<LaunchResult> {
    let state = app.state::<AppState>();
    // 1. 取账号
    let account = {
        let data = state.data.lock().unwrap();
        data.get_accounts()
            .iter()
            .find(|a| a.id == account_id)
            .cloned()
    }
    .ok_or_else(|| AppError::NotFound(account_id.into()))?;

    // 防护:该账号已在主实例运行则拒绝启动(避免同账号双实例挤掉登录态、丢账号数据)
    let main = trae_machine::probe_main_instance();
    if matches!(
        trae_machine::account_state(&account, &main),
        trae_machine::InstanceSource::Main
    ) {
        return Err(AppError::Launch(
            "该账号已在主实例运行,请聚焦主实例而非重复启动".into(),
        ));
    }

    // 2. 解析 exe 路径(已保存或自动扫描)
    let config_dir = app
        .path()
        .app_config_dir()
        .map_err(|e| AppError::Launch(format!("获取配置目录失败: {e}")))?;
    let exe_path = trae_machine::resolve_trae_path(&config_dir)?;

    // 3. 主实例账号(主目录当前登录账号)用主目录启动,复用主实例登录态;否则独立目录多开。
    //    主目录 userId 取自主目录 storage.json(主实例关闭后仍残留最后登录账号),实现"固定绑定主目录"。
    let is_main_account = main.1.as_deref()
        == account
            .desktop_user_id
            .as_deref()
            .filter(|s| !s.is_empty());
    if is_main_account {
        let appdata = std::env::var("APPDATA")
            .map_err(|_| AppError::Launch("无法获取 APPDATA 环境变量".into()))?;
        let main_dir = PathBuf::from(&appdata).join(trae_machine::DATA_DIR_NAME);
        let shared_ext = PathBuf::from(&appdata)
            .join(trae_instance::SHARED_EXTENSIONS_DIR)
            .to_string_lossy()
            .to_string();
        trae_machine::open_product_with_data_dir(
            &exe_path,
            &main_dir.to_string_lossy(),
            Some(&shared_ext),
        )?;
        let _ = crate::rebuild_tray_menu(app);
        return Ok(LaunchResult {
            data_dir: main_dir.to_string_lossy().to_string(),
            machine_id: String::new(),
            launched: true,
        });
    }

    // 4. 独立目录多开:解密凭据 -> 回读实例目录最新凭据(TRAE 可能自己刷新过,避免旧快照覆盖) -> launch_multi(写凭据到 TRAE SOLO CN_{userId})
    let encrypted = account
        .encrypted_credential
        .as_ref()
        .ok_or_else(|| AppError::Launch("该账号尚未导入 TRAE 桌面凭证,无法启动".into()))?;
    let decrypted = credentials::decrypt_credential(encrypted)?;
    let (cred, adopted) = trae_instance::sync_credential_from_instance(&account, &decrypted);
    // 回读到更新凭据时同步应用存储,下次签到直接用新值
    if adopted {
        let encrypted_new = credentials::encrypt_credential(&cred)?;
        let mut data = state.data.lock().unwrap();
        let _ = data.update_account(
            account_id,
            serde_json::json!({ "encryptedCredential": encrypted_new }),
        );
        let _ = data.save(&state.path);
    }
    let result = trae_instance::launch_multi(&account, &cred, &exe_path)?;

    // 5. 回写 data_dir/machine_id(首次启动时绑定,后续复用同一目录与机器码)
    {
        let mut data = state.data.lock().unwrap();
        data.update_account(
            account_id,
            serde_json::json!({
                "dataDir": result.data_dir,
                "machineId": result.machine_id,
            }),
        );
        let _ = data.save(&state.path);
    }

    // 6. 刷新托盘菜单:新实例已运行,状态前缀更新
    let _ = crate::rebuild_tray_menu(app);

    Ok(result)
}

/// 启动账号独立实例(前端命令入口)
#[tauri::command]
pub fn launch_account_multi(id: String, app: AppHandle) -> AppResult<LaunchResult> {
    launch_account_by_id(&app, &id)
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

/// 账号实例运行状态(返回前端)
#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct InstanceState {
    pub running: bool,
    pub source: trae_machine::InstanceSource,
    pub is_main_account: bool,
}

/// 查询账号实例运行状态:未运行 / 主实例(用户手动启动的 TRAE)/ 工具实例(本应用启动的独立 data-dir)
#[tauri::command]
pub fn get_account_instance_state(
    id: String,
    state: State<'_, AppState>,
) -> AppResult<InstanceState> {
    let account = {
        let data = state.data.lock().unwrap();
        data.get_accounts()
            .iter()
            .find(|a| a.id == id)
            .cloned()
    }
    .ok_or_else(|| AppError::NotFound(id.clone()))?;
    let main = trae_machine::probe_main_instance();
    let source = trae_machine::account_state(&account, &main);
    let is_main_account = main.1.as_deref()
        == account
            .desktop_user_id
            .as_deref()
            .filter(|s| !s.is_empty());
    Ok(InstanceState {
        running: !matches!(source, trae_machine::InstanceSource::None),
        source,
        is_main_account,
    })
}

/// 聚焦账号实例窗口:工具实例运行聚焦其 data-dir 窗口,主实例运行聚焦主目录窗口,未运行则报错。
#[tauri::command]
pub fn focus_account_instance(id: String, state: State<'_, AppState>) -> AppResult<()> {
    let account = {
        let data = state.data.lock().unwrap();
        data.get_accounts()
            .iter()
            .find(|a| a.id == id)
            .cloned()
    }
    .ok_or_else(|| AppError::NotFound(id.clone()))?;
    let main = trae_machine::probe_main_instance();
    match trae_machine::account_state(&account, &main) {
        trae_machine::InstanceSource::Tool => {
            let d = account
                .data_dir
                .as_deref()
                .filter(|s| !s.is_empty())
                .ok_or_else(|| AppError::Launch("该账号无工具实例数据目录".into()))?;
            trae_machine::focus_instance_window(d)
        }
        trae_machine::InstanceSource::Main => {
            let main_dir = trae_machine::main_data_dir()?;
            trae_machine::focus_instance_window(&main_dir.to_string_lossy())
        }
        trae_machine::InstanceSource::None => {
            Err(AppError::Launch("该账号尚未启动,无实例可聚焦".into()))
        }
    }
}

/// 打开新的空白 TRAE 实例供用户登录,后台轮询登录完成后自动导入账号:
/// 检测登录 -> 读凭据 -> 杀实例 -> 改名临时目录为标准 TRAE SOLO CN_{userId} -> upsert 账号绑定 -> emit 事件。
#[tauri::command]
pub fn open_new_login_instance(app: AppHandle) -> AppResult<()> {
    let appdata = std::env::var("APPDATA")
        .map_err(|_| AppError::Launch("无法获取 APPDATA 环境变量".into()))?;
    let config_dir = app
        .path()
        .app_config_dir()
        .map_err(|e| AppError::Launch(format!("获取配置目录失败: {e}")))?;
    let exe_path = trae_machine::resolve_trae_path(&config_dir)?;

    // 临时 data-dir(带 uuid 后缀,避免与标准目录或多次操作冲突)
    let temp_dir = PathBuf::from(&appdata)
        .join(format!(
            "{} login {}",
            trae_machine::DATA_DIR_NAME,
            uuid::Uuid::new_v4()
        ))
        .to_string_lossy()
        .to_string();
    let shared_ext = PathBuf::from(&appdata)
        .join(trae_instance::SHARED_EXTENSIONS_DIR)
        .to_string_lossy()
        .to_string();

    // 启动空白实例(不写凭据,用户自行登录)
    trae_machine::open_product_with_data_dir(&exe_path, &temp_dir, Some(&shared_ext))?;

    // 后台轮询登录 + 导入,完成后 emit 事件
    let app_handle = app.clone();
    let appdata_owned = appdata;
    let temp_dir_owned = temp_dir;
    std::thread::spawn(move || {
        let result = wait_login_and_import(&app_handle, &appdata_owned, &temp_dir_owned);
        let _ = app_handle.emit("login-imported", result);
    });
    Ok(())
}

/// 轮询临时实例登录态,登录后导入账号并改名目录。返回事件 payload。
fn wait_login_and_import(app: &AppHandle, appdata: &str, temp_dir: &str) -> serde_json::Value {
    let storage_path = PathBuf::from(temp_dir)
        .join("User")
        .join("globalStorage")
        .join("storage.json");

    // 轮询 iCubeAuthInfo 出现(2s 一次,最多 10 分钟)
    let mut logged_in = false;
    for _ in 0..300 {
        std::thread::sleep(std::time::Duration::from_secs(2));
        if storage_path.exists() {
            if let Ok(raw) = std::fs::read_to_string(&storage_path) {
                if let Ok(storage) = serde_json::from_str::<serde_json::Value>(&raw) {
                    if storage
                        .get("iCubeAuthInfo://icube.cloudide")
                        .and_then(|v| v.as_str())
                        .is_some()
                    {
                        logged_in = true;
                        break;
                    }
                }
            }
        }
    }
    if !logged_in {
        return serde_json::json!({
            "success": false,
            "error": "登录超时(10 分钟未检测到登录),临时目录已保留供手动处理"
        });
    }

    // 读凭据
    let cred = match trae_auth::read_credentials_from_data_dir(&PathBuf::from(temp_dir)) {
        Ok(c) => c,
        Err(e) => {
            return serde_json::json!({
                "success": false,
                "error": format!("读取登录凭据失败: {e}")
            })
        }
    };
    let user_id = cred.user_id.clone();
    if user_id.is_empty() {
        return serde_json::json!({
            "success": false,
            "error": "登录凭据缺少 userId"
        });
    }

    // 杀实例并等退出(目录占用解除后才能改名)
    let _ = trae_machine::kill_instance(temp_dir);
    for _ in 0..20 {
        if !trae_machine::is_instance_running(temp_dir).0 {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(500));
    }

    // 改名为标准目录 TRAE SOLO CN_{userId}(目标已存在则先删除旧的)
    let target_dir =
        PathBuf::from(appdata).join(format!("{}_{}", trae_machine::DATA_DIR_NAME, user_id));
    if target_dir.exists() {
        let _ = std::fs::remove_dir_all(&target_dir);
    }
    if let Err(e) = std::fs::rename(temp_dir, &target_dir) {
        return serde_json::json!({
            "success": false,
            "error": format!("改名为标准目录失败: {e}")
        });
    }

    // upsert 账号(绑定标准目录 + 机器码 + 加密凭据)
    let encrypted = match credentials::encrypt_credential(&cred) {
        Ok(e) => e,
        Err(e) => {
            return serde_json::json!({
                "success": false,
                "error": format!("加密凭据失败: {e}")
            })
        }
    };
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
        desktop_user_id: Some(user_id.clone()),
        encrypted_credential: Some(encrypted),
        credential_status: Some(status.to_string()),
        data_dir: Some(target_dir.to_string_lossy().to_string()),
        machine_id: Some(cred.machine_id.clone()),
    };
    {
        let state = app.state::<AppState>();
        let mut data = state.data.lock().unwrap();
        data.upsert_desktop_account(account);
        let _ = data.save(&state.path);
    }
    let _ = crate::rebuild_tray_menu(app);
    serde_json::json!({
        "success": true,
        "name": cred.account_name,
        "userId": user_id
    })
}

/// 手动刷新账号凭证(卡片"刷新凭证"按钮):实例目录回读 + ExchangeToken 刷新 + 回写
#[tauri::command]
pub async fn refresh_account_credential(
    id: String,
    state: State<'_, AppState>,
    client: State<'_, reqwest::Client>,
) -> AppResult<PublicAccount> {
    let account = {
        let data = state.data.lock().unwrap();
        data.get_accounts().iter().find(|a| a.id == id).cloned()
    }
    .ok_or_else(|| AppError::NotFound(id.clone()))?;
    checkin::refresh_account_credential(&account, client.inner(), state.inner()).await?;
    let acc = {
        let data = state.data.lock().unwrap();
        data.get_accounts()
            .iter()
            .find(|a| a.id == id)
            .cloned()
            .ok_or_else(|| AppError::NotFound(id.clone()))?
    };
    Ok(acc.into())
}

/// 扫描 %APPDATA% 下已存在的多开/登录临时目录,标注是否已绑定应用内账号
#[tauri::command]
pub fn scan_instance_dirs(state: State<'_, AppState>) -> Vec<trae_instance::InstanceDirInfo> {
    let mut dirs = trae_instance::scan_instance_dirs();
    let accounts = {
        let data = state.data.lock().unwrap();
        data.get_accounts().to_vec()
    };
    for d in &mut dirs {
        d.bound = accounts.iter().any(|a| {
            a.data_dir.as_deref() == Some(d.data_dir.as_str())
                || (!d.user_id.is_empty()
                    && a.desktop_user_id.as_deref() == Some(d.user_id.as_str()))
        });
    }
    dirs
}

/// 从已有多开目录导入账号(扫描列表的"导入"入口):
/// 完整凭据优先,缺失签名密钥时退宽松读取(token 可用但过期后无法自动刷新)。
/// 同 userId 账号已存在则仅更新凭据并绑定目录,保留名称/积分/签到历史。
#[tauri::command]
pub fn import_account_from_dir(
    data_dir: String,
    state: State<'_, AppState>,
    app: AppHandle,
) -> AppResult<PublicAccount> {
    let path = PathBuf::from(&data_dir);
    if !path
        .join("User")
        .join("globalStorage")
        .join("storage.json")
        .exists()
    {
        return Err(AppError::NotFound(format!("目录无 storage.json: {data_dir}")));
    }
    let cred = trae_auth::read_credentials_from_data_dir(&path)
        .or_else(|_| trae_auth::read_auth_from_data_dir_loose(&path, &Credential::empty()))?;
    if cred.user_id.is_empty() {
        return Err(AppError::Credential("目录登录信息缺少 userId".into()));
    }

    let encrypted = credentials::encrypt_credential(&cred)?;
    let now = now_ms();
    let status = credential_status(cred.expires_at, now);
    let machine_id = if cred.machine_id.is_empty() {
        None
    } else {
        Some(cred.machine_id.clone())
    };

    let mut data = state.data.lock().unwrap();
    let existing = data
        .get_accounts()
        .iter()
        .find(|a| a.desktop_user_id.as_deref() == Some(cred.user_id.as_str()))
        .cloned();
    let saved = match existing {
        Some(acc) => data
            .update_account(
                &acc.id,
                serde_json::json!({
                    "encryptedCredential": encrypted,
                    "credentialStatus": status,
                    "dataDir": data_dir,
                    "machineId": machine_id,
                }),
            )
            .ok_or_else(|| AppError::NotFound(acc.id.clone()))?,
        None => {
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
                data_dir: Some(data_dir.clone()),
                machine_id,
            };
            data.upsert_desktop_account(account)
        }
    };
    data.save(&state.path)?;
    drop(data);
    let _ = crate::rebuild_tray_menu(&app);
    Ok(saved.into())
}
