//! Wiring for the running app: shared state, settings, the real provider
//! factory and the UI notifier.

use std::path::PathBuf;
use std::sync::Arc;

use rusqlite::Connection;
use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Emitter};
use tauri_plugin_notification::NotificationExt;

use crate::accounts::{Account, Provider};
use crate::config;
use crate::db::{self, Db};
use crate::error::AppResult;
use crate::providers::dryrun::DryRunProvider;
use crate::providers::gmail::GmailProvider;
use crate::providers::microsoft::{MicrosoftProvider, TokenCache};
use crate::providers::{AccountInfo, MailProvider, ProviderError};
use crate::scheduler::{Engine, Notifier, ProviderFactory};
use crate::secrets::SecretStore;
use crate::window::SendWindow;

pub struct AppState {
    pub db: Db,
    pub engine: Arc<Engine>,
    pub secrets: Arc<dyn SecretStore>,
    pub tokens: TokenCache,
    pub http: reqwest::Client,
    pub data_dir: PathBuf,
    pub tick_wake: Arc<tokio::sync::Notify>,
    pub poll_wake: Arc<tokio::sync::Notify>,
}

impl AppState {
    pub fn outbox_dir(&self) -> PathBuf {
        outbox_dir(&self.data_dir)
    }
    pub fn attachments_dir(&self) -> PathBuf {
        self.data_dir.join("attachments")
    }
}

pub fn outbox_dir(data_dir: &std::path::Path) -> PathBuf {
    data_dir.join("outbox-dry-run")
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    pub dry_run: bool,
    pub window: SendWindow,
    pub gap_min_secs: i64,
    pub gap_max_secs: i64,
    pub footer: String,
    pub poll_minutes: i64,
    pub welcome_done: bool,
}

impl Default for Settings {
    fn default() -> Self {
        Settings {
            dry_run: config::DRY_RUN_DEFAULT,
            window: SendWindow::default(),
            gap_min_secs: 45,
            gap_max_secs: 120,
            footer: config::DEFAULT_FOOTER.into(),
            poll_minutes: 5,
            welcome_done: false,
        }
    }
}

const SETTINGS_KEY: &str = "app_settings";

pub fn load_settings(conn: &Connection) -> Settings {
    db::get_setting(conn, SETTINGS_KEY)
        .ok()
        .flatten()
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_default()
}

pub fn save_settings(conn: &Connection, s: &Settings) -> AppResult<()> {
    s.window.validate().map_err(crate::error::AppError::User)?;
    if s.gap_min_secs < 5 || s.gap_max_secs < s.gap_min_secs {
        return Err(crate::error::AppError::user("Check the gap between sends: minimum at least 5 seconds, and not above the maximum."));
    }
    if !(1..=120).contains(&s.poll_minutes) {
        return Err(crate::error::AppError::user("Check for replies every 1 to 120 minutes."));
    }
    db::set_setting(conn, SETTINGS_KEY, &serde_json::to_string(s)?)
}

pub struct AppProviders {
    pub db: Db,
    pub secrets: Arc<dyn SecretStore>,
    pub tokens: TokenCache,
    pub http: reqwest::Client,
    pub outbox_dir: PathBuf,
}

impl ProviderFactory for AppProviders {
    fn for_account(&self, a: &Account, allow_dry_run: bool) -> Result<Arc<dyn MailProvider>, ProviderError> {
        if allow_dry_run && load_settings(&self.db.lock()).dry_run {
            return Ok(Arc::new(DryRunProvider {
                dir: self.outbox_dir.clone(),
                info: AccountInfo { email: a.email.clone(), display_name: a.display_name.clone(), account_type: a.account_type.clone() },
            }));
        }
        match a.provider {
            Provider::Gmail => {
                let pw = self
                    .secrets
                    .get(&a.secret_key())
                    .map_err(ProviderError::Temporary)?
                    .ok_or_else(|| ProviderError::Auth("The app password for this account is missing. Reconnect it.".into()))?;
                Ok(Arc::new(GmailProvider { email: a.email.clone(), display_name: a.display_name.clone(), app_password: pw }))
            }
            Provider::Microsoft => Ok(Arc::new(MicrosoftProvider {
                account_id: a.id,
                email: a.email.clone(),
                display_name: a.display_name.clone(),
                account_type: a.account_type.clone(),
                secret_key: a.secret_key(),
                secrets: self.secrets.clone(),
                tokens: self.tokens.clone(),
                http: self.http.clone(),
            })),
        }
    }
}

pub struct TauriNotifier(pub AppHandle);

impl Notifier for TauriNotifier {
    fn changed(&self) {
        let _ = self.0.emit("data-changed", ());
    }
    fn reply(&self, title: &str, body: &str) {
        let _ = self.0.emit("data-changed", ());
        if let Err(e) = self.0.notification().builder().title(title).body(body).show() {
            tracing::warn!("notification failed: {e}");
        }
    }
}
