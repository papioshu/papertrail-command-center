mod agents;
mod alerts;
mod chat;
mod commands;
mod continuity;
mod db;
mod discovery;
mod gitscan;
mod hub_client;
mod indexer;
mod launcher;
mod phone;
mod registry;
pub mod standby;
mod team;
mod util;

use serde_json::{json, Value};
use std::path::PathBuf;
use std::sync::Mutex;
use tauri::menu::{Menu, MenuItem};
use tauri::tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent};
use tauri::{AppHandle, Emitter, Manager, WindowEvent};

pub struct AppState {
    pub db: Mutex<rusqlite::Connection>,
    pub data_dir: PathBuf,
    pub worker: indexer::Worker,
    pub briefing_since: String,
    pub task_snapshot: Value,
    pub close_to_tray: Mutex<bool>,
    /// Signed-in team profile (memory only: every start shows the login).
    pub user: Mutex<Option<Value>>,
}

/// WebView2 memory target: Low while hidden in the tray, Normal when shown.
fn memory_target(w: &tauri::WebviewWindow, low: bool) {
    #[cfg(windows)]
    let _ = w.with_webview(move |wv| unsafe {
        use webview2_com::Microsoft::Web::WebView2::Win32::*;
        use windows_core::Interface;
        if let Ok(core) = wv.controller().CoreWebView2() {
            if let Ok(c19) = core.cast::<ICoreWebView2_19>() {
                let level = if low { COREWEBVIEW2_MEMORY_USAGE_TARGET_LEVEL_LOW } else { COREWEBVIEW2_MEMORY_USAGE_TARGET_LEVEL_NORMAL };
                let _ = c19.SetMemoryUsageTargetLevel(level);
            }
        }
    });
}

/// Tell WebView2 whether the page is on screen. Invisible = document hidden + rendering stopped
/// (a minimized window otherwise still reports itself visible and focused).
fn webview_visible(w: &tauri::WebviewWindow, visible: bool) {
    #[cfg(windows)]
    let _ = w.with_webview(move |wv| unsafe {
        let _ = wv.controller().SetIsVisible(visible);
    });
}

fn show_main(app: &AppHandle) {
    if let Some(w) = app.get_webview_window("main") {
        memory_target(&w, false);
        webview_visible(&w, true);
        let _ = w.unminimize();
        let _ = w.show();
        let _ = w.set_focus();
    }
}

/// Briefing window: since the previous start, at least 24 h, at most 14 days.
fn briefing_since(conn: &rusqlite::Connection) -> String {
    let now = chrono::Utc::now();
    let last = db::setting(conn, "last_seen")
        .and_then(|v| v.as_str().and_then(|s| chrono::DateTime::parse_from_rfc3339(s).ok()))
        .map(|d| d.with_timezone(&chrono::Utc));
    let floor = now - chrono::Duration::days(14);
    let ceil = now - chrono::Duration::hours(24);
    let t = last.unwrap_or(ceil).min(ceil).max(floor);
    t.to_rfc3339_opts(chrono::SecondsFormat::Secs, true)
}

pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_single_instance::init(|app, _args, _cwd| show_main(app)))
        .plugin(tauri_plugin_notification::init())
        .plugin(tauri_plugin_autostart::init(tauri_plugin_autostart::MacosLauncher::LaunchAgent, None))
        .setup(|app| {
            let data_dir = app.path().app_data_dir()?;
            std::fs::create_dir_all(&data_dir)?;
            util::init_log(&data_dir.join("logs"));
            std::panic::set_hook(Box::new(|info| util::log("panic", info.to_string())));
            util::log("info", format!("start v{} pid {}", env!("CARGO_PKG_VERSION"), std::process::id()));
            let db_path = data_dir.join("papertrail.db");
            let conn = db::open_migrated(&db_path)?;
            let since = briefing_since(&conn);
            let snapshot: Value = db::setting(&conn, "task_snapshot").unwrap_or(json!({}));
            let mut now_snap = serde_json::Map::new();
            for r in util::rows(&conn, "SELECT project_id, count(*) n FROM tasks WHERE status='done' GROUP BY project_id", [])? {
                if let (Some(id), Some(n)) = (r["project_id"].as_str(), r["n"].as_i64()) {
                    now_snap.insert(id.into(), json!(n));
                }
            }
            db::set_setting(&conn, "task_snapshot", &Value::Object(now_snap))?;
            db::set_setting(&conn, "last_seen", &json!(util::now_iso()))?;
            let close_to_tray = registry::Config::load(&conn).close_to_tray;
            standby::hold_app_lock(&data_dir);
            if db::setting(&conn, "phone_enabled").and_then(|v| v.as_bool()) == Some(true) {
                standby::register(true); // so a paired phone can start the dashboard once it's closed (D029)
            }
            let worker = indexer::spawn(app.handle().clone(), db_path.clone());
            worker.send(indexer::Job::Refresh); // cached data shows first; this refreshes behind it
            indexer::spawn_watcher(db_path.clone(), worker.clone());
            app.manage(AppState {
                db: Mutex::new(conn),
                data_dir,
                worker,
                briefing_since: since,
                task_snapshot: snapshot,
                close_to_tray: Mutex::new(close_to_tray),
                user: Mutex::new(None),
            });
            app.manage(phone::spawn(app.handle().clone()));
            // Workspace hub (M3): just a struct until the user joins; the sync thread starts only if joined.
            app.manage(hub_client::HubState::new(&app.path().app_data_dir()?));
            hub_client::start(app.handle());

            let open = MenuItem::with_id(app, "open", "Open Command Center", true, None::<&str>)?;
            let refresh = MenuItem::with_id(app, "refresh", "Refresh index", true, None::<&str>)?;
            let quit = MenuItem::with_id(app, "quit", "Quit", true, None::<&str>)?;
            let menu = Menu::with_items(app, &[&open, &refresh, &quit])?;
            TrayIconBuilder::with_id("main")
                .icon(app.default_window_icon().cloned().ok_or("no icon")?)
                .tooltip("PAPERTRAIL // COMMAND CENTER")
                .menu(&menu)
                .show_menu_on_left_click(false)
                .on_menu_event(|app, ev| match ev.id.as_ref() {
                    "open" => show_main(app),
                    "refresh" => app.state::<AppState>().worker.send(indexer::Job::Refresh),
                    "quit" => app.exit(0),
                    _ => {}
                })
                .on_tray_icon_event(|tray, ev| {
                    if let TrayIconEvent::Click { button: MouseButton::Left, button_state: MouseButtonState::Up, .. } = ev {
                        show_main(tray.app_handle());
                    }
                })
                .build(app)?;
            Ok(())
        })
        .on_window_event(|w, ev| {
            if w.label() == "main" {
                match ev {
                    WindowEvent::Resized(_) => {
                        if let Some(ww) = w.app_handle().get_webview_window("main") {
                            let minimized = ww.is_minimized().unwrap_or(false);
                            webview_visible(&ww, !minimized);
                            let _ = ww.emit("window-minimized", minimized);
                        }
                    }
                    _ => {}
                }
            }
            // Main window only: this handler sees every window, and running it for the chat panel switched the
            // dashboard's drawing off (black dashboard, 2026-11-09).
            if let (WindowEvent::CloseRequested { api, .. }, "main") = (ev, w.label()) {
                let st = w.app_handle().state::<AppState>();
                let to_tray = st.db.lock().map(|c| registry::Config::load(&c).close_to_tray).unwrap_or(true);
                *st.close_to_tray.lock().unwrap() = to_tray;
                if to_tray {
                    api.prevent_close();
                    let _ = w.hide();
                    if let Some(ww) = w.app_handle().get_webview_window("main") {
                        memory_target(&ww, true);
                        webview_visible(&ww, false);
                    }
                }
            }
        })
        .invoke_handler(tauri::generate_handler![
            commands::get_status,
            commands::get_projects,
            commands::get_briefing,
            commands::get_project,
            commands::get_activity,
            commands::get_timeline,
            commands::get_sessions,
            commands::chat_page,
            commands::chat_tail,
            commands::chat_entry,
            commands::chat_list,
            commands::chat_projects,
            commands::get_session_files,
            commands::get_graph,
            commands::search,
            commands::refresh_now,
            commands::discovery_refresh,
            commands::get_discoveries,
            commands::set_discovery,
            commands::add_review_task,
            commands::toggle_review_task,
            commands::bos_preview,
            commands::bos_append,
            commands::preview_handoff,
            commands::launch_agent,
            commands::get_cli_info,
            commands::open_path,
            commands::open_url,
            commands::get_settings,
            commands::save_settings,
            commands::set_project_meta,
            commands::register_project,
            commands::get_diagnostics,
            commands::backup_db,
            commands::get_sparks,
            commands::toggle_chat,
            commands::chat_current,
            commands::get_folders,
            commands::task_toggle_preview,
            commands::task_toggle,
            commands::team_profiles,
            commands::team_create,
            commands::team_login,
            commands::team_logout,
            commands::actions_list,
            commands::action_op,
            phone::phone_status,
            phone::phone_set_enabled,
            phone::phone_pair,
            phone::phone_revoke,
            hub_client::hub_status,
            hub_client::hub_join,
            hub_client::hub_leave,
            hub_client::hub_device_link,
            hub_client::app_update_check,
            hub_client::app_update_install,
            hub_client::hub_sync_now,
            hub_client::hub_set_vault_media,
            hub_client::hub_board,
            hub_client::hub_edit,
            hub_client::hub_resolve,
            hub_client::hub_invite,
            hub_client::hub_new_project,
            hub_client::hub_admin,
            hub_client::hub_link_preview,
            hub_client::hub_link,
            hub_client::hub_upload,
            hub_client::hub_file,
            hub_client::hub_snapshot_preview,
            hub_client::hub_snapshot_publish,
            hub_client::hub_host_status,
            hub_client::hub_open_share_folder,
            hub_client::hub_host_create_workspace,
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
