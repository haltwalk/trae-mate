// Tauri 应用入口。注册全部命令、通知插件,初始化存储/HTTP/调度器状态,配置系统托盘与关闭到托盘。

mod checkin;
mod commands;
mod credentials;
mod error;
mod models;
mod scheduler;
mod store;
mod trae_auth;
mod trae_machine;
mod trae_instance;

use tauri::{
    menu::{Menu, MenuItem, PredefinedMenuItem},
    tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent},
    AppHandle, Emitter, Manager,
};

/// 重建系统托盘右键菜单:列出全部账号(● 运行中 / ○ 未运行,点击启动或聚焦)+ 一键签到 + 退出。
/// 在启动、删除账号、以及定时(感知外部关闭)时调用。
pub fn rebuild_tray_menu(app: &AppHandle) {
    let accounts = {
        let state = app.state::<store::AppState>();
        let accounts = state.data.lock().unwrap().get_accounts().to_vec();
        accounts
    };

    let menu = match Menu::new(app) {
        Ok(m) => m,
        Err(_) => return,
    };

    // 账号列表:列出全部账号(★ 主实例 / ● 工具实例 / ○ 未运行),点击聚焦或启动
    if !accounts.is_empty() {
        let main = trae_machine::probe_main_instance();
        if let Ok(header) =
            MenuItem::with_id(app, "tray_header", "账号列表", false, None::<&str>)
        {
            let _ = menu.append(&header);
        }
        for a in &accounts {
            let prefix = match trae_machine::account_state(a, &main) {
                trae_machine::InstanceSource::Main => "★",
                trae_machine::InstanceSource::Tool => "●",
                trae_machine::InstanceSource::None => "○",
            };
            let id_str = format!("tray_account_{}", a.id);
            let name = if a.name.trim().is_empty() {
                a.id.clone()
            } else {
                a.name.clone()
            };
            let title = format!("{} {}", prefix, name);
            if let Ok(item) =
                MenuItem::with_id(app, id_str.as_str(), title.as_str(), true, None::<&str>)
            {
                let _ = menu.append(&item);
            }
        }
        if let Ok(sep) = PredefinedMenuItem::separator(app) {
            let _ = menu.append(&sep);
        }
    }
    if let Ok(item) = MenuItem::with_id(app, "tray_checkin", "一键签到", true, None::<&str>) {
        let _ = menu.append(&item);
    }
    if let Ok(item) = MenuItem::with_id(app, "tray_quit", "退出", true, None::<&str>) {
        let _ = menu.append(&item);
    }

    if let Some(tray) = app.tray_by_id("main-tray") {
        let _ = tray.set_menu(Some(menu));
    }
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_single_instance::init(|app, _args, _cwd| {
            // 已有实例运行:显示并聚焦主窗口,阻止重复打开
            if let Some(window) = app.get_webview_window("main") {
                let _ = window.show();
                let _ = window.set_focus();
            }
        }))
        .plugin(tauri_plugin_notification::init())
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_autostart::init(
            tauri_plugin_autostart::MacosLauncher::LaunchAgent,
            Some(vec!["--minimized"]),
        ))
        .setup(|app| {
            // 存储路径:<app_data_dir>/trae-check-data.json
            let app_data = app.path().app_data_dir()?;
            std::fs::create_dir_all(&app_data)?;
            let path = app_data.join("trae-check-data.json");
            let data = store::StoreData::load(&path).unwrap_or_default();
            app.manage(store::AppState {
                data: std::sync::Mutex::new(data),
                path,
            });
            app.manage(reqwest::Client::new());
            app.manage(scheduler::SchedulerState::default());

            // 静默启动:--minimized 时保持主窗口隐藏到托盘(tauri.conf visible:false 配合,避免闪烁)
            let minimized = std::env::args().any(|a| a == "--minimized");
            if !minimized {
                if let Some(window) = app.get_webview_window("main") {
                    let _ = window.show();
                }
            }

            // 系统托盘:左键单击显示窗口;右键菜单(动态:账号列表启动/聚焦 + 一键签到 + 退出)由 rebuild_tray_menu 维护
            let tray_icon = app
                .default_window_icon()
                .cloned()
                .expect("默认窗口图标缺失,无法创建托盘");
            TrayIconBuilder::with_id("main-tray")
                .icon(tray_icon)
                .tooltip("TraeMate - 签到与多开账号管理")
                .show_menu_on_left_click(false)
                .on_menu_event(|app, event| {
                    let id = event.id.as_ref();
                    match id {
                        "tray_checkin" => {
                            // 显示窗口并通知前端执行一键签到
                            if let Some(window) = app.get_webview_window("main") {
                                let _ = window.show();
                                let _ = window.set_focus();
                            }
                            let _ = app.emit("tray-checkin", ());
                        }
                        "tray_quit" => {
                            app.exit(0);
                        }
                        // 点击账号项:主/工具实例运行则聚焦对应窗口,未运行则启动
                        s if s.starts_with("tray_account_") => {
                            let account_id = s["tray_account_".len()..].to_string();
                            let account = {
                                let state = app.state::<store::AppState>();
                                let data = state.data.lock().unwrap();
                                data.get_accounts()
                                    .iter()
                                    .find(|a| a.id == account_id)
                                    .cloned()
                            };
                            let account = match account {
                                Some(a) => a,
                                None => return, // 账号已不存在,忽略
                            };
                            let main = trae_machine::probe_main_instance();
                            match trae_machine::account_state(&account, &main) {
                                trae_machine::InstanceSource::Tool => {
                                    if let Some(d) =
                                        account.data_dir.as_deref().filter(|s| !s.is_empty())
                                    {
                                        if let Err(e) = trae_machine::focus_instance_window(d) {
                                            eprintln!(
                                                "[tray] 聚焦账号 {} 工具实例失败: {}",
                                                account_id, e
                                            );
                                        }
                                    }
                                }
                                trae_machine::InstanceSource::Main => {
                                    match trae_machine::main_data_dir() {
                                        Ok(d) => {
                                            if let Err(e) = trae_machine::focus_instance_window(
                                                &d.to_string_lossy(),
                                            ) {
                                                eprintln!(
                                                    "[tray] 聚焦账号 {} 主实例失败: {}",
                                                    account_id, e
                                                );
                                            }
                                        }
                                        Err(e) => {
                                            eprintln!("[tray] 获取主实例目录失败: {}", e)
                                        }
                                    }
                                }
                                trae_machine::InstanceSource::None => {
                                    // 后台启动,避免阻塞菜单事件回调
                                    let app_handle = app.clone();
                                    std::thread::spawn(move || {
                                        if let Err(e) =
                                            commands::launch_account_by_id(&app_handle, &account_id)
                                        {
                                            eprintln!("[tray] 启动账号 {} 失败: {}", account_id, e);
                                        }
                                    });
                                }
                            }
                        }
                        _ => {}
                    }
                })
                .on_tray_icon_event(|tray, event| {
                    // 左键单击托盘:显示窗口
                    if let TrayIconEvent::Click {
                        button: MouseButton::Left,
                        button_state: MouseButtonState::Up,
                        ..
                    } = event
                    {
                        let app = tray.app_handle();
                        if let Some(window) = app.get_webview_window("main") {
                            let _ = window.show();
                            let _ = window.set_focus();
                        }
                    }
                })
                .build(app)?;

            // 初始构建动态托盘菜单,并启动定时刷新(感知多开实例外部关闭,10s 一次)
            rebuild_tray_menu(app.handle());
            {
                let app_handle = app.handle().clone();
                std::thread::spawn(move || loop {
                    std::thread::sleep(std::time::Duration::from_secs(10));
                    rebuild_tray_menu(&app_handle);
                });
            }

            // 启动定时任务(若已开启自动签到)
            scheduler::start_scheduler(app.handle().clone());
            Ok(())
        })
        .on_window_event(|window, event| {
            // 关闭主窗口时隐藏到系统托盘,不真正退出(由托盘"退出"菜单退出)
            if let tauri::WindowEvent::CloseRequested { api, .. } = event {
                if window.label() == "main" {
                    let _ = window.hide();
                    api.prevent_close();
                }
            }
        })
        .invoke_handler(tauri::generate_handler![
            commands::get_accounts,
            commands::import_desktop_account,
            commands::update_account,
            commands::delete_account,
            commands::checkin_account,
            commands::checkin_all,
            commands::get_account_points,
            commands::get_logs,
            commands::clear_logs,
            commands::get_settings,
            commands::save_settings,
            commands::start_scheduler,
            commands::stop_scheduler,
            commands::get_next_run_time,
            // 多开实例
            commands::launch_account_multi,
            commands::get_trae_exe_path,
            commands::set_trae_exe_path,
            commands::scan_trae_exe_path,
            commands::get_account_instance_state,
            commands::focus_account_instance,
            commands::open_new_login_instance,
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
