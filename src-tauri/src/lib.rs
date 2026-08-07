// Tauri 应用入口。注册全部命令、通知插件,初始化存储/HTTP/调度器状态,配置系统托盘与关闭到托盘。

mod checkin;
mod commands;
mod credentials;
mod error;
mod models;
mod scheduler;
mod store;
mod trae_auth;

use tauri::{
    menu::{Menu, MenuItem},
    tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent},
    Emitter, Manager,
};

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_notification::init())
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

            // 系统托盘:右键菜单(一键签到 / 退出),左键单击显示窗口
            let checkin_item = MenuItem::with_id(app, "tray_checkin", "一键签到", true, None::<&str>)?;
            let quit_item = MenuItem::with_id(app, "tray_quit", "退出", true, None::<&str>)?;
            let menu = Menu::with_items(app, &[&checkin_item, &quit_item])?;
            let tray_icon = app
                .default_window_icon()
                .cloned()
                .expect("默认窗口图标缺失,无法创建托盘");
            TrayIconBuilder::with_id("main-tray")
                .icon(tray_icon)
                .tooltip("TraeCheck - 每日自动签到")
                .menu(&menu)
                .show_menu_on_left_click(false)
                .on_menu_event(|app, event| match event.id.as_ref() {
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
                    _ => {}
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
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
