// 定时签到调度器。用 generation 计数控制任务生命周期(无需 abort),
// tokio 分段 sleep 以便及时响应 stop。时区 Asia/Shanghai。

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

use chrono_tz::Asia::Shanghai;
use tauri::{AppHandle, Manager};
use tauri_plugin_notification::NotificationExt;

use crate::checkin::perform_all_checkin;
use crate::models::AppSettings;
use crate::store::AppState;

pub struct SchedulerState {
    pub generation: Arc<AtomicU64>,
}

impl Default for SchedulerState {
    fn default() -> Self {
        SchedulerState {
            generation: Arc::new(AtomicU64::new(0)),
        }
    }
}

/// 启动定时任务(若已开启自动签到)。每次调用增 generation,旧任务自行退出。
pub fn start_scheduler(app: AppHandle) {
    let gen = app.state::<SchedulerState>().inner().generation.clone();
    let my_gen = gen.fetch_add(1, Ordering::SeqCst) + 1;

    let settings = app
        .state::<AppState>()
        .inner()
        .data
        .lock()
        .unwrap()
        .get_settings();
    if !settings.auto_checkin {
        return;
    }

    let app_clone = app.clone();
    tauri::async_runtime::spawn(async move {
        loop {
            if !is_current(&app_clone, my_gen) {
                break;
            }
            let dur = match next_run_duration(&app_clone) {
                Some(d) => d,
                None => break,
            };
            // 分段 sleep,每 15s 检查 generation,及时响应 stop/restart
            let mut remaining = dur;
            while remaining > std::time::Duration::ZERO {
                if !is_current(&app_clone, my_gen) {
                    break;
                }
                let step = remaining.min(std::time::Duration::from_secs(15));
                tokio::time::sleep(step).await;
                remaining = remaining.saturating_sub(step);
            }
            if !is_current(&app_clone, my_gen) {
                break;
            }
            run_auto_checkin(app_clone.clone()).await;
        }
    });
}

/// 停止定时任务(增 generation 使当前任务退出)
pub fn stop_scheduler(app: &AppHandle) {
    app.state::<SchedulerState>()
        .inner()
        .generation
        .fetch_add(1, Ordering::SeqCst);
}

/// 下次执行时间(ISO8601),未启用返回 None
pub fn get_next_run_time(app: &AppHandle) -> Option<String> {
    let settings = app
        .state::<AppState>()
        .inner()
        .data
        .lock()
        .unwrap()
        .get_settings();
    if !settings.auto_checkin {
        return None;
    }
    next_run_instant(&settings).map(|dt| dt.to_rfc3339())
}

fn is_current(app: &AppHandle, gen: u64) -> bool {
    app.state::<SchedulerState>()
        .inner()
        .generation
        .load(Ordering::SeqCst)
        == gen
}

async fn run_auto_checkin(app: AppHandle) {
    let state = app.state::<AppState>();
    let client = app.state::<reqwest::Client>();
    let settings = state.inner().data.lock().unwrap().get_settings();

    let results = perform_all_checkin(client.inner(), state.inner()).await;
    // 定时签到后:对签到涉及的每个账号重查真实总积分并落库(与手动签到积分一致,
    // 避免界面只显示"签到前估算值 base+gained").
    for (account, r) in &results {
        if !r.success {
            continue;
        }
        let acct = state
            .inner()
            .data
            .lock()
            .unwrap()
            .get_accounts()
            .iter()
            .find(|a| a.id == account.id)
            .cloned();
        let Some(acct) = acct else {
            continue;
        };
        let pr = crate::checkin::get_total_points(&acct, client.inner(), state.inner()).await;
        if pr.success {
            let mut data = state.inner().data.lock().unwrap();
            data.update_account(
                &acct.id,
                crate::checkin::points_update_json(&pr),
            );
            let _ = data.save(&state.inner().path);
        }
    }

    let success = results.iter().filter(|(_, r)| r.success).count();
    let failed = results.len() - success;

    if settings.notify_on_success && success > 0 {
        let body = format!(
            "成功签到 {} 个账号{}",
            success,
            if failed > 0 {
                format!("，失败 {} 个", failed)
            } else {
                String::new()
            }
        );
        let _ = app
            .notification()
            .builder()
            .title("Trae 签到成功")
            .body(body)
            .show();
    }
    if settings.notify_on_failed && failed > 0 {
        let _ = app
            .notification()
            .builder()
            .title("Trae 签到失败")
            .body(format!("{} 个账号签到失败，请查看日志", failed))
            .show();
    }
}

fn next_run_instant(settings: &AppSettings) -> Option<chrono::DateTime<chrono::Utc>> {
    let (h, m) = parse_hhmm(&settings.checkin_time)?;
    let now = chrono::Utc::now().with_timezone(&Shanghai);
    let today = now.date_naive().and_hms_opt(h, m, 0)?;
    let today = today.and_local_timezone(Shanghai).single()?;
    let next = if today > now {
        today
    } else {
        today + chrono::Duration::days(1)
    };
    Some(next.with_timezone(&chrono::Utc))
}

fn next_run_duration(app: &AppHandle) -> Option<std::time::Duration> {
    let settings = app
        .state::<AppState>()
        .inner()
        .data
        .lock()
        .unwrap()
        .get_settings();
    let next = next_run_instant(&settings)?;
    let now = chrono::Utc::now();
    (next - now).to_std().ok()
}

fn parse_hhmm(s: &str) -> Option<(u32, u32)> {
    let mut parts = s.split(':');
    let h: u32 = parts.next()?.parse().ok()?;
    let m: u32 = parts.next()?.parse().ok()?;
    Some((h, m))
}
