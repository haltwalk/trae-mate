// TRAE 客户端进程/路径/机器码/运行检测。移植自 Account Manager 的 machine.rs。
// 仅服务于"多开实例":找 exe、带 --user-data-dir 启动、检测运行状态、生成机器码。
// 不含杀进程/清缓存(那是切换账号才需要的,多开用全新 data-dir 天然干净)。

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use uuid::Uuid;

use crate::error::{AppError, AppResult};
use crate::models::Account;
use crate::trae_auth;

/// TRAE SOLO CN 进程名
pub const PROCESS_NAME: &str = "TRAE SOLO CN.exe";
/// TRAE SOLO CN 数据目录名(默认实例)
pub const DATA_DIR_NAME: &str = "TRAE SOLO CN";
/// exe 路径配置文件名(放在应用 config 目录下)
const EXE_PATH_FILE: &str = "trae_exe_path.txt";

/// Windows 下隐藏子进程控制台窗口(避免 tasklist 弹黑窗)
#[cfg(target_os = "windows")]
pub fn hide_window(mut cmd: Command) -> Command {
    use std::os::windows::process::CommandExt;
    const CREATE_NO_WINDOW: u32 = 0x08000000;
    cmd.creation_flags(CREATE_NO_WINDOW);
    cmd
}

#[cfg(not(target_os = "windows"))]
pub fn hide_window(cmd: Command) -> Command {
    cmd
}

/// 生成新的机器码(UUID v4,与系统 MachineGuid 无关,每实例独立)
pub fn generate_machine_guid() -> String {
    Uuid::new_v4().to_string()
}

/// telemetry.machineId 派生(对应 Account Manager 的 md5_hash,基于 DefaultHasher,非标准 MD5)
pub fn telemetry_machine_id(machine_id: &str) -> String {
    use std::collections::hash_map::DefaultHasher;
    use std::hash::{Hash, Hasher};

    let mut h1 = DefaultHasher::new();
    machine_id.hash(&mut h1);
    let v1 = h1.finish();

    let mut h2 = DefaultHasher::new();
    format!("{}{}", machine_id, v1).hash(&mut h2);
    let v2 = h2.finish();

    let combined = ((v1 as u128) << 64) | (v2 as u128);
    format!("{:x}", combined)
}

// ===== exe 路径 =====

/// 读取已保存的 exe 路径(config_dir 下的 trae_exe_path.txt)
pub fn get_saved_trae_path(config_dir: &Path) -> AppResult<Option<String>> {
    let path = config_dir.join(EXE_PATH_FILE);
    if !path.exists() {
        return Ok(None);
    }
    let raw = fs::read_to_string(&path)?;
    let trimmed = raw.trim().to_string();
    if trimmed.is_empty() || !PathBuf::from(&trimmed).exists() {
        return Ok(None);
    }
    Ok(Some(trimmed))
}

/// 保存 exe 路径
pub fn save_trae_path(config_dir: &Path, exe_path: &str) -> AppResult<()> {
    let p = PathBuf::from(exe_path);
    if !p.exists() {
        return Err(AppError::Launch(format!("路径不存在: {exe_path}")));
    }
    #[cfg(target_os = "windows")]
    if !exe_path.to_lowercase().ends_with(".exe") {
        return Err(AppError::Launch("请选择 .exe 文件".into()));
    }
    fs::create_dir_all(config_dir)?;
    fs::write(config_dir.join(EXE_PATH_FILE), exe_path)?;
    Ok(())
}

/// 解析 exe 路径:已保存且有效则用,否则扫描并保存
pub fn resolve_trae_path(config_dir: &Path) -> AppResult<String> {
    if let Some(p) = get_saved_trae_path(config_dir)? {
        return Ok(p);
    }
    let scanned = scan_trae_exe_path()?;
    let _ = save_trae_path(config_dir, &scanned);
    Ok(scanned)
}

/// 自动扫描 TRAE Work CN 安装路径:常见位置 > 注册表卸载信息
#[cfg(target_os = "windows")]
pub fn scan_trae_exe_path() -> AppResult<String> {
    let appdata_local = std::env::var("LOCALAPPDATA")
        .map_err(|_| AppError::Launch("无法获取 LOCALAPPDATA 环境变量".into()))?;
    let candidates = [
        PathBuf::from(&appdata_local).join("Programs").join(DATA_DIR_NAME).join(PROCESS_NAME),
        PathBuf::from(&appdata_local).join("Programs").join("Trae").join(PROCESS_NAME),
        PathBuf::from(&appdata_local).join(DATA_DIR_NAME).join(PROCESS_NAME),
        PathBuf::from(r"C:\Program Files\TRAE SOLO CN\TRAE SOLO CN.exe"),
        PathBuf::from(r"D:\TRAE SOLO CN\TRAE SOLO CN.exe"),
        PathBuf::from(r"E:\TRAE SOLO CN\TRAE SOLO CN.exe"),
    ];
    for c in &candidates {
        if c.exists() {
            return Ok(c.to_string_lossy().to_string());
        }
    }
    if let Ok(p) = scan_from_registry() {
        return Ok(p);
    }
    Err(AppError::Launch(
        "未找到 TRAE Work CN 安装路径,请在设置中手动选择 TRAE SOLO CN.exe".into(),
    ))
}

#[cfg(not(target_os = "windows"))]
pub fn scan_trae_exe_path() -> AppResult<String> {
    Err(AppError::Launch("自动扫描仅支持 Windows,请手动设置路径".into()))
}

/// 从注册表卸载信息扫描(排除本应用 manager 与 Trae Auto)
#[cfg(target_os = "windows")]
fn scan_from_registry() -> AppResult<String> {
    use winreg::enums::*;
    use winreg::RegKey;

    let hklm = RegKey::predef(HKEY_LOCAL_MACHINE);
    let hkcu = RegKey::predef(HKEY_CURRENT_USER);
    let uninstall_paths = [
        r"SOFTWARE\Microsoft\Windows\CurrentVersion\Uninstall",
        r"SOFTWARE\WOW6432Node\Microsoft\Windows\CurrentVersion\Uninstall",
    ];
    const EXCLUDE: &[&str] = &["manager", "trae auto"];
    const MATCH: &[&str] = &["TRAE SOLO CN", "TRAE Work CN"];

    for root in [hklm, hkcu] {
        for up in &uninstall_paths {
            let key = match root.open_subkey(up) {
                Ok(k) => k,
                Err(_) => continue,
            };
            for name in key.enum_keys().filter_map(Result::ok) {
                let sub = match key.open_subkey(&name) {
                    Ok(k) => k,
                    Err(_) => continue,
                };
                let display_name: String = sub.get_value("DisplayName").unwrap_or_default();
                let install_location: String = sub.get_value("InstallLocation").unwrap_or_default();
                let uninstall_string: String = sub.get_value("UninstallString").unwrap_or_default();
                let display_icon: String = sub.get_value("DisplayIcon").unwrap_or_default();
                let dn_lower = display_name.to_lowercase();
                if EXCLUDE.iter().any(|k| dn_lower.contains(k)) {
                    continue;
                }
                if !MATCH.iter().any(|k| dn_lower.contains(&k.to_lowercase())) {
                    continue;
                }
                let mut dirs: Vec<String> = Vec::new();
                if !install_location.is_empty() {
                    dirs.push(install_location.trim_end_matches('\\').trim_matches('"').to_string());
                }
                if !display_icon.is_empty() {
                    let icon = display_icon
                        .split(',')
                        .next()
                        .unwrap_or("")
                        .trim()
                        .trim_matches('"')
                        .to_string();
                    if let Some(parent) = PathBuf::from(&icon).parent() {
                        dirs.push(parent.to_string_lossy().to_string());
                    }
                }
                if !uninstall_string.is_empty() {
                    let uninst = uninstall_string
                        .split('"')
                        .nth(1)
                        .or_else(|| uninstall_string.split_whitespace().next())
                        .unwrap_or("")
                        .trim()
                        .to_string();
                    if !uninst.is_empty() {
                        if let Some(parent) = PathBuf::from(&uninst).parent() {
                            dirs.push(parent.to_string_lossy().to_string());
                        }
                    }
                }
                for dir in &dirs {
                    let exe = PathBuf::from(dir).join(PROCESS_NAME);
                    if exe.exists() {
                        return Ok(exe.to_string_lossy().to_string());
                    }
                }
                if !display_icon.is_empty() {
                    let icon = display_icon
                        .split(',')
                        .next()
                        .unwrap_or("")
                        .trim()
                        .trim_matches('"')
                        .to_string();
                    let p = PathBuf::from(&icon);
                    if p.exists() && p.extension().and_then(|e| e.to_str()) == Some("exe") {
                        if p.file_name().and_then(|n| n.to_str()) == Some(PROCESS_NAME) {
                            return Ok(icon);
                        }
                    }
                }
            }
        }
    }
    Err(AppError::Launch("注册表中未找到 TRAE Work CN 安装信息".into()))
}

// ===== 启动 =====

/// 多开模式:用指定 data-dir 启动 TRAE(不影响其他实例)
pub fn open_product_with_data_dir(
    exe_path: &str,
    data_dir: &str,
    extensions_dir: Option<&str>,
) -> AppResult<()> {
    let exe = PathBuf::from(exe_path);
    if !exe.exists() {
        return Err(AppError::Launch(format!("TRAE 路径无效: {exe_path}")));
    }
    fs::create_dir_all(data_dir)?;
    let mut cmd = Command::new(&exe);
    cmd.arg("--user-data-dir").arg(data_dir);
    if let Some(ext) = extensions_dir {
        fs::create_dir_all(ext).ok();
        cmd.arg("--extensions-dir").arg(ext);
    }
    // 丢弃子进程 stdout/stderr:避免其写已关闭管道触发 EPIPE,也避免 TRAE 日志污染父进程终端
    cmd.stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null());
    #[cfg(target_os = "windows")]
    {
        cmd = hide_window(cmd);
    }
    cmd.spawn()
        .map_err(|e| AppError::Launch(format!("启动 TRAE 失败: {e}")))?;
    Ok(())
}

// ===== 运行检测 =====

/// 检查 data-dir 对应的实例是否在运行(读 code.lock 拿 PID + tasklist 验活)
pub fn is_instance_running(data_dir: &str) -> (bool, Option<u32>) {
    let lock_path = Path::new(data_dir).join("code.lock");
    if !lock_path.exists() {
        return (false, None);
    }
    let pid_str = match fs::read_to_string(&lock_path) {
        Ok(s) => s.trim().to_string(),
        Err(_) => return (false, None),
    };
    let pid: u32 = match pid_str.parse() {
        Ok(p) => p,
        Err(_) => return (false, None),
    };
    #[cfg(target_os = "windows")]
    {
        let output = hide_window(Command::new("tasklist"))
            .args(["/FI", &format!("PID eq {pid}"), "/NH", "/FO", "CSV"])
            .output();
        if let Ok(out) = output {
            let text = String::from_utf8_lossy(&out.stdout);
            // tasklist 输出含 PID 则存活;中文/英文环境都要排除"无任务"提示
            let running = text.contains(&pid.to_string())
                && !text.contains("没有运行")
                && !text.contains("No tasks")
                && !text.contains("信息: 没有运行");
            return (running, Some(pid));
        }
        (false, Some(pid))
    }
    #[cfg(not(target_os = "windows"))]
    {
        // 非 Windows 不支持运行检测(签到工具本就仅 Windows)
        let _ = pid;
        (false, None)
    }
}

// ===== 主实例(默认 data-dir)检测 =====

/// 主实例 data-dir(%APPDATA%\TRAE SOLO CN,即用户手动启动 TRAE 用的默认目录)
pub fn main_data_dir() -> AppResult<PathBuf> {
    let appdata = std::env::var("APPDATA")
        .map_err(|_| AppError::Launch("无法获取 APPDATA 环境变量".into()))?;
    Ok(PathBuf::from(&appdata).join(DATA_DIR_NAME))
}

/// 账号实例运行来源
#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub enum InstanceSource {
    None,
    Main,
    Tool,
}

/// 探测主实例状态:(是否运行, 登录 userId)。仅在运行时读 storage.json,避免读到残留登录态。
pub fn probe_main_instance() -> (bool, Option<String>) {
    let running = match main_data_dir() {
        Ok(d) => is_instance_running(&d.to_string_lossy()).0,
        Err(_) => false,
    };
    // userId 不管运行都读(主实例关闭后 storage.json 仍残留最后登录账号),供 is_main_account 判断
    let user_id = trae_auth::read_main_instance_user_id();
    (running, user_id)
}

/// 判定账号运行状态:工具实例(账号 data-dir 运行)优先,其次主实例(登录 userId 匹配 desktop_user_id)。
pub fn account_state(account: &Account, main: &(bool, Option<String>)) -> InstanceSource {
    if account
        .data_dir
        .as_deref()
        .map_or(false, |d| !d.is_empty() && is_instance_running(d).0)
    {
        return InstanceSource::Tool;
    }
    if main.0 {
        if let (Some(main_uid), Some(desktop_uid)) = (&main.1, &account.desktop_user_id) {
            if !desktop_uid.is_empty() && desktop_uid == main_uid {
                return InstanceSource::Main;
            }
        }
    }
    InstanceSource::None
}

// ===== 聚焦实例窗口 =====

/// 聚焦指定 data-dir 实例的窗口:读 code.lock 拿 PID,枚举顶层窗口找该进程的可见窗口,提到前台。
#[cfg(target_os = "windows")]
pub fn focus_instance_window(data_dir: &str) -> AppResult<()> {
    use windows::core::BOOL;
    use windows::Win32::Foundation::{HWND, LPARAM};
    use windows::Win32::UI::WindowsAndMessaging::{
        EnumWindows, GetWindowThreadProcessId, IsWindowVisible, SetForegroundWindow, ShowWindow,
        SW_RESTORE,
    };

    let lock_path = Path::new(data_dir).join("code.lock");
    if !lock_path.exists() {
        return Err(AppError::Launch("实例未运行(无 code.lock)".into()));
    }
    let pid: u32 = fs::read_to_string(&lock_path)
        .map_err(|e| AppError::Launch(format!("读取 code.lock 失败: {e}")))?
        .trim()
        .parse()
        .map_err(|_| AppError::Launch("code.lock 的 PID 无效".into()))?;

    struct FindState {
        pid: u32,
        found: Option<HWND>,
    }
    let mut state = FindState { pid, found: None };

    unsafe extern "system" fn enum_proc(hwnd: HWND, lparam: LPARAM) -> BOOL {
        unsafe {
            let state = &mut *(lparam.0 as *mut FindState);
            let mut window_pid: u32 = 0;
            let _ = GetWindowThreadProcessId(hwnd, Some(&mut window_pid as *mut u32));
            if window_pid == state.pid && IsWindowVisible(hwnd).as_bool() {
                state.found = Some(hwnd);
                return BOOL(0); // FALSE 停止枚举
            }
            BOOL(1) // TRUE 继续
        }
    }

    unsafe {
        // EnumWindows 在回调返回 FALSE(找到目标后停止枚举)时返回 Err(0x00000000),并非真实错误,忽略返回值
        let _ = EnumWindows(Some(enum_proc), LPARAM(&mut state as *mut _ as isize));
    }

    let hwnd = state
        .found
        .ok_or_else(|| AppError::Launch("未找到该实例的窗口(可能已最小化到托盘)".into()))?;

    unsafe {
        let _ = ShowWindow(hwnd, SW_RESTORE);
        let _ = SetForegroundWindow(hwnd);
    }
    Ok(())
}

#[cfg(not(target_os = "windows"))]
pub fn focus_instance_window(_data_dir: &str) -> AppResult<()> {
    Err(AppError::Launch("聚焦窗口仅支持 Windows".into()))
}

/// 关闭指定 data-dir 的实例(读 code.lock PID,taskkill /F /T 杀进程树)
#[cfg(target_os = "windows")]
pub fn kill_instance(data_dir: &str) -> AppResult<()> {
    let lock_path = Path::new(data_dir).join("code.lock");
    if !lock_path.exists() {
        return Ok(()); // 无锁,认为未运行
    }
    let pid_str = fs::read_to_string(&lock_path)
        .map_err(|e| AppError::Launch(format!("读取 code.lock 失败: {e}")))?;
    let pid: u32 = pid_str
        .trim()
        .parse()
        .map_err(|_| AppError::Launch("code.lock 的 PID 无效".into()))?;
    let _ = hide_window(Command::new("taskkill"))
        .args(["/PID", &pid.to_string(), "/F", "/T"])
        .output();
    Ok(())
}

#[cfg(not(target_os = "windows"))]
pub fn kill_instance(_data_dir: &str) -> AppResult<()> {
    Err(AppError::Launch("关闭实例仅支持 Windows".into()))
}

// ===== 窗口标题 =====

/// 向 data-dir 的 settings.json 写入 window.title(--title CLI 参数对 TRAE 无效,用配置项更可靠)
pub fn write_window_title_to_dir(data_dir: &str, title: &str) -> AppResult<()> {
    let settings_path = PathBuf::from(data_dir).join("User").join("settings.json");
    let mut json: serde_json::Value = if settings_path.exists() {
        let content = fs::read_to_string(&settings_path)?;
        serde_json::from_str(&content).unwrap_or(serde_json::json!({}))
    } else {
        serde_json::json!({})
    };
    json["window.title"] = serde_json::Value::String(title.to_string());
    fs::write(&settings_path, serde_json::to_string_pretty(&json)?)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn telemetry_machine_id_stable_and_distinct() {
        let a = telemetry_machine_id("abc-123");
        let b = telemetry_machine_id("abc-123");
        assert_eq!(a, b, "同输入应确定性输出");
        let c = telemetry_machine_id("xyz-789");
        assert_ne!(a, c, "不同输入应产生不同输出");
        assert!(!a.is_empty());
    }

    #[test]
    fn generate_machine_guid_is_uuid() {
        let g = generate_machine_guid();
        assert!(Uuid::parse_str(&g).is_ok(), "机器码应为合法 UUID: {g}");
    }
}
