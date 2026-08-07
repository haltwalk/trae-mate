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

/// 重建系统托盘右键菜单:动态列出正在运行的多开账号(点击聚焦其窗口)+ 一键签到 + 退出。
/// 在启动、多开/删除账号、以及定时(感知外部关闭)时调用。
pub fn rebuild_tray_menu(app: &AppHandle) {
    let accounts = {
        let state = app.state::<store::AppState>();
        let accounts = state.data.lock().unwrap().get_accounts().to_vec();
        accounts
    };

    // 正在运行的多开账号(有 data_dir 且实例存活)
    let running: Vec<_> = accounts
        .iter()
        .filter(|a| {
            a.data_dir
                .as_deref()
                .map_or(false, |d| !d.is_empty() && trae_machine::is_instance_running(d).0)
        })
        .collect();

    let menu = match Menu::new(app) {
        Ok(m) => m,
        Err(_) => return,
    };

    // 多开账号:标题分组(disabled 的标题项作分组头,账号项平铺其下)
    if !running.is_empty() {
        if let Ok(header) =
            MenuItem::with_id(app, "tray_header", "多开账号", false, None::<&str>)
        {
            let _ = menu.append(&header);
        }
        for a in &running {
            let id_str = format!("tray_focus_{}", a.id);
            let title = if a.name.trim().is_empty() {
                format!("  {}", a.id)
            } else {
                format!("  {}", a.name)
            };
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

            // 系统托盘:左键单击显示窗口;右键菜单(动态:多开账号聚焦 + 一键签到 + 退出)由 rebuild_tray_menu 维护
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
                        // 点击多开账号项:聚焦其 TRAE 实例窗口
                        s if s.starts_with("tray_focus_") => {
                            let account_id = &s["tray_focus_".len()..];
                            let state = app.state::<store::AppState>();
                            let data_dir = {
                                let data = state.data.lock().unwrap();
                                data.get_accounts()
                                    .iter()
                                    .find(|a| a.id == account_id)
                                    .and_then(|a| a.data_dir.clone())
                            };
                            if let Some(d) = data_dir {
                                if let Err(e) = trae_machine::focus_instance_window(&d) {
                                    eprintln!("[tray] 聚焦账号 {} 失败: {}", account_id, e);
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
            commands::is_account_instance_running,
            commands::focus_account_instance,
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
