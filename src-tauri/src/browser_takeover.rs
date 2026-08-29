// 方案2核心:临时接管系统默认浏览器,截获 TRAE 客户端发起登录时打开的登录 URL。
//
// 背景:新客户端 2.3.78099 打开发起登录页时,会生成一个带机器级 device_id
// (YOUR_MACHINE_DEVICE,与主账号共用)的登录 URL,并交给系统默认浏览器打开。登录 URL
// 里的 device 与客户端换 token 时从 storage.json 的 icube-dc 读取的 device 必须一致,
// 否则服务端返回 20403 (Token device not match)。
//
// 本模块做的正是「客户端打开 URL 之前拦截 -> 改写 device -> 同步 storage.json ->
// 恢复原浏览器 -> 用原浏览器打开改写后的 URL」,使登录授权、换 token、签到三个环节
// 使用同一个独立的 16 位数字 device。
//
// Windows 实现:临时写 HKCU\Software\Classes\{http,https}\shell\open\command 指向
// 本程序 `--capture-login "%1" [实例目录]`。纯用户态、可完整恢复的覆盖,不触碰受保护
// (UserChoice 哈希加密)的默认浏览器绑定,风险最低。捕获完成后立刻恢复。

// 应急预案模块:当前默认免劫持方案(客户端机器码登录一次 + 签到时自动生成独立设备)已足够,
// 本模块不再被应用调用,仅保留作日后 TRAE 更新账户/设备策略时的复用件。为保持编译清净抑制
// 未使用告警;日后接回时去掉本行并重新接线即可。
#![allow(dead_code)]

use std::fs;
use std::path::PathBuf;
use std::process::Command;

use serde_json::Value;
use winreg::enums::*;
use winreg::RegKey;

/// 本可执行文件自身路径(劫持 URL 处理器指向它)
pub fn self_exe() -> PathBuf {
    std::env::current_exe().unwrap_or_else(|_| PathBuf::from("trae-mate.exe"))
}

/// URL 捕获命令行字面量在 lib.rs 解析命令行时用来识别
pub const CAPTURE_FLAG: &str = "--capture-login";

/// 备份记录:被改写的完整注册表子键路径 + 原始值(可能无)。
/// key_path 形如 `Software\Classes\MSEdgeHTM\shell\open\command` 或
/// `Software\Classes\https\shell\open\command`(无 UserChoice 默认时)。
type BackSpec = (String, Option<String>);

/// 给定协议(http/https),返回系统实际用来打开它的 ProgId 及该 ProgId 的 open\command 子键。
/// 优先读 UserChoice(HKCU\Software\Microsoft\Windows\Shell\Associations\UrlAssociations\{proto}\UserChoice\ProgId),
/// 因为用户设了默认浏览器后,http/https 打开走 UserChoice 指向的 ProgId(如 MSEdgeHTM/ChromeHTML)。
/// 无 UserChoice 时才退回 `Software\Classes\{proto}` 本身。
/// 返回 (命令子键路径, 当前值)。
fn handler_target(proto: &str) -> (String, Option<String>) {
    let hkcu = RegKey::predef(HKEY_CURRENT_USER);
    // 1) UserChoice 默认浏览器
    let uc_path = format!(
        r"Software\Microsoft\Windows\Shell\Associations\UrlAssociations\{proto}\UserChoice"
    );
    if let Ok(uc) = hkcu.open_subkey(&uc_path) {
        if let Ok(progid) = uc.get_value::<String, _>("ProgId") {
            // ProgId 解析:先在 HKCU\Software\Classes 找,再在 HKLM(经 HKCR)找实际命令
            let cmd_key = format!(r"Software\Classes\{progid}\shell\open\command");
            let cur = ref_command(&cmd_key);
            // 关键:如果 HKCU 里是我们上次残留的 capture 命令(HKCU 覆盖拦截),
            // 绝不能把它当"原始值"备份(否则恢复会写回 capture 命令导致自引用残留)。
            // 此时丢弃 HKCU 值,回退取 HKLM 系统真实命令作为原始值。
            if !is_capture_command(cur.as_deref()) {
                return (cmd_key, cur);
            }
            // HKCU 没覆盖·或是 capture 残留 → 取系统真实命令
            let cur_system = system_command(&progid);
            return (cmd_key, cur_system);
        }
    }
    // 2) 无 UserChoice,退回协议本身
    let cmd_key = handler_key(proto);
    let cur = ref_command(&cmd_key);
    let cur = if is_capture_command(cur.as_deref()) {
        None
    } else {
        cur
    };
    (cmd_key, cur)
}

/// 判断命令是否本程序用于浏览器接管的 capture 命令(残留检测)。
fn is_capture_command(cmd: Option<&str>) -> bool {
    cmd.map(|c| c.contains(CAPTURE_FLAG)).unwrap_or(false)
}

/// 读 HKCU\Software\Classes\<包> 下的 open 命令
fn ref_command(cmd_key: &str) -> Option<String> {
    let hkcu = RegKey::predef(HKEY_CURRENT_USER);
    hkcu.open_subkey(cmd_key)
        .ok()
        .and_then(|k| k.get_value::<String, _>("").ok())
}

/// 读系统级(HKCR 实际)ProgId 的 open 命令,返回该 ProgId 命令文本。
fn system_command(progid: &str) -> Option<String> {
    let hklm = RegKey::predef(HKEY_LOCAL_MACHINE);
    let cmd_key = format!(r"Software\Classes\{progid}\shell\open\command");
    hklm.open_subkey(&cmd_key)
        .ok()
        .and_then(|k| k.get_value::<String, _>("").ok())
}

fn handler_key(protocol: &str) -> String {
    format!(r"Software\Classes\{protocol}\shell\open\command")
}

/// 保存 http/https 实际解析的 open\command,并把它覆盖为指向本程序 `--capture-login`。
/// 返回备份(含被改写的完整子键路径 + 原始值),供 restore_takeover 恢复。可重复调用。
pub fn install_takeover(instance_dir: &str) -> Vec<BackSpec> {
    let exe = self_exe();
    let flag_cmd = format!(
        "\"{}\" {CAPTURE_FLAG} \"%1\" \"{}\"",
        exe.display(),
        instance_dir
    );
    let mut backups: Vec<BackSpec> = Vec::new();
    let hkcu = RegKey::predef(HKEY_CURRENT_USER);
    // https 与 http 分别处理,https 优先写入(客户端用 https 打开)
    for proto in ["https", "http"] {
        let (cmd_key, current) = handler_target(proto);
        backups.push((cmd_key.clone(), current));
        let _ = write_open_command(&hkcu, &cmd_key, &flag_cmd);
    }
    backups
}

fn write_open_command(hkcu: &RegKey, key_path: &str, value: &str) -> std::io::Result<()> {
    let (key, _) = hkcu.create_subkey(key_path)?;
    key.set_value("", &value)
}

/// 恢复系统默认浏览器关联(还原捕获前保存的完整子键值)。
pub fn restore_takeover(backups: &[BackSpec]) {
    let hkcu = RegKey::predef(HKEY_CURRENT_USER);
    for (cmd_key, original) in backups {
        match original {
            Some(orig) => {
                let _ = write_open_command(&hkcu, cmd_key, orig);
            }
            None => {
                // 原本无命令(常见于 HKCU 覆盖键),删除我们写入的键恢复原状
                let _ = hkcu.delete_subkey_all(cmd_key.clone());
            }
        }
    }
}

/// 备份文件:存到实例目录下,供独立捕获子进程读取后恢复。
pub fn backup_path(instance_dir: &str) -> PathBuf {
    PathBuf::from(instance_dir).join("traemate-browser-backup.json")
}

/// 显式把备份写到指定实例目录。
pub fn persist_backup_to(instance_dir: &str, backups: &[BackSpec]) -> std::io::Result<()> {
    let arr: Vec<Value> = backups
        .iter()
        .map(|(k, o)| {
            let o = o
                .as_deref()
                .map(|s| Value::String(s.to_string()))
                .unwrap_or(Value::Null);
            serde_json::json!({ "key": k, "original": o })
        })
        .collect();
    let dir = backup_path(instance_dir);
    if let Some(parent) = dir.parent() {
        let _ = fs::create_dir_all(parent);
    }
    fs::write(dir, serde_json::to_string_pretty(&arr)?)
}

/// 从实例目录读备份并恢复。返回是否恢复成功。
pub fn restore_from_backup(instance_dir: &str) -> bool {
    let path = backup_path(instance_dir);
    let Ok(raw) = fs::read_to_string(&path) else {
        return false;
    };
    let Ok(arr) = serde_json::from_str::<Value>(&raw) else {
        return false;
    };
    let Some(list) = arr.as_array() else {
        return false;
    };
    let backups: Vec<BackSpec> = list
        .iter()
        .filter_map(|v| {
            let cmd_key = v.get("key")?.as_str()?.to_string();
            let original = v.get("original").cloned();
            let original = match original {
                None | Some(Value::Null) => None,
                Some(Value::String(s)) => Some(s),
                _ => None,
            };
            Some((cmd_key, original))
        })
        .collect();
    if backups.is_empty() {
        return false;
    }
    restore_takeover(&backups);
    let _ = fs::remove_file(&path);
    true
}

/// 读当前 URL 处理器命令(测试/状态展示)。
/// 注:现代系统走 UserChoice ProgId,这里读实际解析目标。
pub fn current_handler(protocol: &str) -> Option<String> {
    let (cmd_key, cur) = handler_target(protocol);
    cur.or_else(|| ref_command(&cmd_key))
}

/// 用系统默认浏览器打开 URL(ShellExecute 交给操作系统)。
pub fn open_system_browser(url: &str) -> std::io::Result<()> {
    Command::new("rundll32.exe")
        .args(["url.dll,FileProtocolHandler", url])
        .spawn()
        .map(|_| ())
}

/// 改写 URL 中顶层查询参数(device_id / x_device_id)。参数不存在则原样保留。
/// 注:客户端打开的 login URL 把最终的 authorization URL 编码在 redirect_url 参数里,
/// 设备参数实际上嵌在 redirect_url 内层,这里同时处理两层(见 rewrite_login_url)。
fn replace_query_param(url: &str, param: &str, new_val: &str) -> String {
    let marker = format!("{param}=");
    let mut out = String::with_capacity(url.len() + 16);
    let mut rest = url;
    while let Some(rel) = rest.find(&marker) {
        let start = rel + marker.len();
        // 值到下一个 '&' 或 '#' 为止
        let end = rest[start..]
            .find(['&', '#'])
            .map(|i| start + i)
            .unwrap_or(rest.len());
        out.push_str(&rest[..start]);
        out.push_str(new_val);
        rest = &rest[end..];
        // 若已经到末尾或遇到 '#',直接追加剩余终止
        if rest.starts_with('#') {
            // 把剩余原样追加后返回
            out.push_str(rest);
            return out;
        }
    }
    out.push_str(rest);
    out
}

/// 粗糙的 percent-decode / encode(仅用于 redirect_url 的值域:ASCII + 保留符)。
fn percent_decode(s: &str) -> String {
    let bytes = s.as_bytes();
    let mut out: Vec<u8> = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' && i + 2 < bytes.len() {
            if let (Some(h), Some(l)) = (hex_val(bytes[i + 1]), hex_val(bytes[i + 2])) {
                out.push(h << 4 | l);
                i += 3;
                continue;
            }
        }
        out.push(bytes[i]);
        i += 1;
    }
    // 宽松 UTF-8
    String::from_utf8_lossy(&out).into_owned()
}

fn hex_val(b: u8) -> Option<u8> {
    match b {
        b'0'..=b'9' => Some(b - b'0'),
        b'a'..=b'f' => Some(b - b'a' + 10),
        b'A'..=b'F' => Some(b - b'A' + 10),
        _ => None,
    }
}

fn percent_encode(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for &b in s.as_bytes() {
        // 保留字母数字及 -_.~,其余编码(保守,保证服务端识别)
        if b.is_ascii_alphanumeric() || matches!(b, b'-' | b'.' | b'_' | b'~') {
            out.push(b as char);
        } else {
            out.push_str(&format!("%{:02X}", b));
        }
    }
    out
}

/// 提取 URL(可含顶层查询)中指定查询参数的值。
fn query_param_value(url: &str, param: &str) -> Option<String> {
    let marker = format!("{param}=");
    let rest = url.find(&marker)?;
    let start = rest + marker.len();
    let end = url[start..]
        .find(['&', '#'])
        .map(|i| start + i)
        .unwrap_or(url.len());
    Some(url[start..end].to_string())
}

/// 完整改写登录 URL:把顶层及 redirect_url 内层的 device_id/x_device_id 统一改为 new。
/// 返回改写后的 URL。
pub fn rewrite_login_url(url: &str, new_device: &str) -> String {
    let mut url = url.to_string();
    // 1. 改写顶层
    url = replace_query_param(&url, "device_id", new_device);
    url = replace_query_param(&url, "x_device_id", new_device);
    // 2. 处理 redirect_url 内层
    if let Some(enc) = query_param_value(&url, "redirect_url") {
        let decoded = percent_decode(&enc);
        if decoded.contains("device_id=") {
            let inner = replace_query_param(&decoded, "device_id", new_device);
            let inner = replace_query_param(&inner, "x_device_id", new_device);
            let new_enc = percent_encode(&inner);
            // 用 rewrite 的方式替换 redirect_url 的当前值
            url = replace_redirect_value(&url, &new_enc);
        }
    }
    url
}

/// 替换 redirect_url 参数的值(存在时)。
fn replace_redirect_value(url: &str, new_val: &str) -> String {
    let marker = "redirect_url=";
    let mut out = String::with_capacity(url.len() + new_val.len());
    let mut rest = url;
    while let Some(rel) = rest.find(marker) {
        let start = rel + marker.len();
        let end = rest[start..]
            .find(['&', '#'])
            .map(|i| start + i)
            .unwrap_or(rest.len());
        out.push_str(&rest[..start]);
        out.push_str(new_val);
        rest = &rest[end..];
        if rest.starts_with('#') {
            out.push_str(rest);
            return out;
        }
    }
    out.push_str(rest);
    out
}

/// 读实例 storage.json 里现有的 icube-dc 签名密钥值(任意 icube-dc 键)。
pub fn read_signing_key(storage: &Value) -> Value {
    storage
        .as_object()
        .and_then(|m| {
            m.iter()
                .find(|(k, _)| k.starts_with("iCubeAuthInfo://icube-dc:"))
                .map(|(_, v)| v.clone())
        })
        .unwrap_or(Value::String(String::new()))
}

/// 把独立 device 写入 storage.json 的 icube-dc(迁移签名密钥、置迁移标记)。
pub fn apply_device_to_storage(storage: &mut Value, new_device: &str) {
    // 先在可变借用之前取出签名密钥
    let sign = read_signing_key(storage);
    if let Some(obj) = storage.as_object_mut() {
        let old: Vec<String> = obj
            .keys()
            .filter(|k| k.starts_with("iCubeAuthInfo://icube-dc:"))
            .cloned()
            .collect();
        for k in old {
            obj.remove(&k);
        }
        obj.insert(format!("iCubeAuthInfo://icube-dc:{new_device}"), sign);
        obj.insert(
            "has_device_id_updated_to_aha".to_string(),
            Value::String("true".to_string()),
        );
    }
}

/// storage.json 完整路径
pub fn storage_path(instance_dir: &str) -> PathBuf {
    PathBuf::from(instance_dir)
        .join("User")
        .join("globalStorage")
        .join("storage.json")
}

/// 完整走一遍:改写 URL + 同步 storage.json。返回 (改写后URL, 是否成功改写storage)。
pub fn apply_rewrite(url: &str, instance_dir: &str, new_device: &str) -> (String, bool) {
    let new_url = rewrite_login_url(url, new_device);
    let mut written = false;
    let sp = storage_path(instance_dir);
    if sp.exists() {
        if let Ok(raw) = fs::read_to_string(&sp) {
            if let Ok(mut storage) = serde_json::from_str::<Value>(&raw) {
                apply_device_to_storage(&mut storage, new_device);
                if let Ok(json_str) = serde_json::to_string_pretty(&storage) {
                    written = fs::write(&sp, json_str).is_ok();
                }
            }
        }
    }
    (new_url, written)
}

/// 生成独立的 16 位数字 aha 设备ID
pub fn generate_aha_device_id() -> String {
    use rand::Rng;
    let mut rng = rand::thread_rng();
    rng.gen_range(1_000_000_000_000_000_u64..9_999_999_999_999_999_u64)
        .to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rewrite_top_level_device() {
        let u = "https://www.trae.cn/login?login_platform=solo&device_id=YOUR_MACHINE_DEVICE&x_device_id=YOUR_MACHINE_DEVICE&code_challenge=ABC";
        let r = rewrite_login_url(u, "1234567890123456");
        assert!(r.contains("device_id=1234567890123456"));
        assert!(r.contains("x_device_id=1234567890123456"));
        assert!(r.contains("code_challenge=ABC"));
    }

    #[test]
    fn rewrite_inner_redirect_url() {
        let u = "https://www.trae.cn/login?device_id=YOUR_MACHINE_DEVICE&redirect_url=https%3A%2F%2Fwww.trae.cn%2Fauthorization%3Fdevice_id%3DYOUR_MACHINE_DEVICE%26x_device_id%3DYOUR_MACHINE_DEVICE%26client_id%3Den1oxy";
        let r = rewrite_login_url(u, "9999999999999999");
        // 顶层
        assert!(r.contains("device_id=9999999999999999"));
        // 内层(仍编码)
        let inner = query_param_value(&r, "redirect_url").unwrap();
        let decoded = percent_decode(&inner);
        assert!(decoded.contains("device_id=9999999999999999"), "inner decoded={decoded}");
        assert!(decoded.contains("x_device_id=9999999999999999"));
        assert!(decoded.contains("client_id=en1oxy"));
    }

    #[test]
    fn storage_injection() {
        let mut j: Value = serde_json::json!({
            "iCubeAuthInfo://icube-dc:111": "signkey",
            "telemetry.devDeviceId": "x"
        });
        apply_device_to_storage(&mut j, "1234567890123456");
        let obj = j.as_object().unwrap();
        assert!(obj.contains_key("iCubeAuthInfo://icube-dc:1234567890123456"));
        assert!(!obj.contains_key("iCubeAuthInfo://icube-dc:111"));
        assert_eq!(obj["iCubeAuthInfo://icube-dc:1234567890123456"], "signkey");
        assert_eq!(obj.get("has_device_id_updated_to_aha").unwrap(), "true");
    }

    /// 端到端:安装->校验覆盖->恢复->校验还原。
    /// 会真实改写 UserChoice 指向的默认浏览器(Firefox/Chrome)的 open 命令,
    /// 但"立即恢复"在任一断言前执行,保证测试即使失败也不污染真实浏览器。
    #[test]
    fn takeover_install_restore_pipeline() {
        let temp_dir = std::env::temp_dir().join("traemate_takeover_test");
        let temp_str = temp_dir.to_string_lossy().to_string();
        // 先记原始 handler
        let orig_https = current_handler("https");
        let orig_http = current_handler("http");
        assert!(orig_https.is_some(), "应能解析到默认浏览器");

        let backups = install_takeover(&temp_str);
        persist_backup_to(&temp_str, &backups).expect("备份写盘失败");
        let hijacked_https = current_handler("https").unwrap_or_default();

        // 校验当前 handler 指向本程序 CAPTURE_FLAG
        assert!(
            hijacked_https.contains(crate::browser_takeover::CAPTURE_FLAG),
            "劫持后 https handler 应为捕获命令,实际={hijacked_https}"
        );

        // 恢复(通过备份文件,模拟子进程路径)——在任何可能失败的断言之前执行
        let restored = restore_from_backup(&temp_str);
        assert!(restored, "恢复应从备份成功");
        // 校验还原
        assert_eq!(
            current_handler("https"),
            orig_https,
            "https 应还原成默认浏览器(orig={orig_https:?})"
        );
        assert_eq!(
            current_handler("http"),
            orig_http,
            "http 应还原成默认浏览器(orig={orig_http:?})"
        );
        assert!(!backup_path(&temp_str).exists(), "备份文件应被清理");
        let _ = std::fs::remove_dir_all(&temp_dir);
    }
}