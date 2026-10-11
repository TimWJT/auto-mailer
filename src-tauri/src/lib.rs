//! Auto Mailer — personalised plain-text email campaigns with automatic
//! follow-ups that stop when someone replies. Everything runs and is stored
//! on the user's own computer; there is no server.

#![allow(clippy::type_complexity, clippy::large_enum_variant)]

pub mod accounts;
pub mod app;
pub mod campaigns;
pub mod classify;
pub mod clock;
mod commands;
pub mod config;
pub mod contacts;
pub mod db;
pub mod error;
pub mod import;
pub mod inbox;
pub mod oauth;
pub mod providers;
pub mod scheduler;
pub mod secrets;
pub mod templates;
pub mod window;

use std::sync::Arc;
use std::time::Duration;

use tauri::Manager;

use crate::app::{AppProviders, AppState, TauriNotifier};
use crate::clock::SystemClock;
use crate::db::Db;
use crate::scheduler::{Engine, Zone};
use crate::secrets::KeyringStore;

const TICK_EVERY: Duration = Duration::from_secs(30);

pub fn run() {
    tracing_subscriber::fmt().with_target(false).init();

    tauri::Builder::default()
        // First, so a second launch just focuses the existing window and two
        // copies can never send in parallel.
        .plugin(tauri_plugin_single_instance::init(|app, _argv, _cwd| {
            if let Some(w) = app.get_webview_window("main") {
                let _ = w.unminimize();
                let _ = w.set_focus();
            }
        }))
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_notification::init())
        .plugin(tauri_plugin_window_state::Builder::default().build())
        .setup(|app| {
            let data_dir = app.path().app_data_dir()?;
            std::fs::create_dir_all(&data_dir)?;
            let db = Db::open(&data_dir.join("auto-mailer.sqlite"))?;
            let secrets: Arc<dyn secrets::SecretStore> = Arc::new(KeyringStore);
            let tokens = Arc::default();
            let http = reqwest::Client::builder()
                .user_agent(concat!("AutoMailer/", env!("CARGO_PKG_VERSION")))
                .build()?;
            let providers = Arc::new(AppProviders {
                db: db.clone(),
                secrets: secrets.clone(),
                tokens: Arc::clone(&tokens),
                http: http.clone(),
                outbox_dir: app::outbox_dir(&data_dir),
            });
            let engine = Arc::new(Engine::new(
                db.clone(),
                Arc::new(SystemClock),
                providers,
                Arc::new(TauriNotifier(app.handle().clone())),
                Zone::Local,
            ));
            let tick_wake = Arc::new(tokio::sync::Notify::new());
            let poll_wake = Arc::new(tokio::sync::Notify::new());

            // Sending queue: runs only while the app is open.
            {
                let engine = engine.clone();
                let wake = tick_wake.clone();
                tauri::async_runtime::spawn(async move {
                    loop {
                        if let Err(e) = engine.tick().await {
                            tracing::warn!("queue tick failed: {e}");
                        }
                        tokio::select! {
                            _ = tokio::time::sleep(TICK_EVERY) => {}
                            _ = wake.notified() => {}
                        }
                    }
                });
            }
            // Reply detection: once at start-up, then every few minutes.
            {
                let engine = engine.clone();
                let wake = poll_wake.clone();
                let db = db.clone();
                tauri::async_runtime::spawn(async move {
                    loop {
                        if let Err(e) = engine.poll_all().await {
                            tracing::warn!("reply check failed: {e}");
                        }
                        let minutes = app::load_settings(&db.lock()).poll_minutes.clamp(1, 120) as u64;
                        tokio::select! {
                            _ = tokio::time::sleep(Duration::from_secs(minutes * 60)) => {}
                            _ = wake.notified() => {}
                        }
                    }
                });
            }

            app.manage(AppState { db, engine, secrets, tokens, http, data_dir, tick_wake, poll_wake });
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            commands::app_info,
            commands::get_settings,
            commands::save_settings,
            commands::list_accounts,
            commands::add_gmail,
            commands::sign_in_microsoft,
            commands::update_gmail_password,
            commands::test_account,
            commands::update_account,
            commands::remove_account,
            commands::list_contacts,
            commands::get_contact,
            commands::save_contact,
            commands::delete_contacts,
            commands::field_names,
            commands::list_tags,
            commands::list_lists,
            commands::create_list,
            commands::rename_list,
            commands::delete_list,
            commands::add_to_list,
            commands::remove_from_list,
            commands::import_preview,
            commands::import_commit,
            commands::dnc_list,
            commands::dnc_add,
            commands::dnc_remove,
            commands::contact_timeline,
            commands::list_campaigns,
            commands::get_campaign,
            commands::save_campaign,
            commands::delete_campaign,
            commands::add_attachment,
            commands::remove_attachment,
            commands::preview_step,
            commands::campaign_checklist,
            commands::launch_campaign,
            commands::set_campaign_status,
            commands::enrol_new_members,
            commands::campaign_stats,
            commands::campaign_enrolments,
            commands::send_test,
            commands::queue_overview,
            commands::run_queue_now,
            commands::check_replies_now,
            commands::list_inbox,
            commands::get_inbound,
            commands::unread_count,
            commands::export_campaign_csv,
            commands::export_contacts_csv,
            commands::backup,
            commands::restore,
        ])
        .run(tauri::generate_context!())
        .expect("failed to start Auto Mailer");
}
