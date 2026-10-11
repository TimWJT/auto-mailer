//! Tauri commands: the only way the UI touches data or the network.
//! Thin wrappers; the logic lives in the other modules.

use std::path::PathBuf;

use serde::Serialize;
use tauri::State;

use crate::accounts::{self, Account, AccountUpdate, NewAccount, Provider};
use crate::app::{self, AppState, Settings};
use crate::campaigns::{self, Campaign, CampaignInput, CampaignSummary, Checklist, EnrolmentRow, Preview, Stats};
use crate::config;
use crate::contacts::{self, Contact, ContactList, ContactPage, ContactQuery, DncEntry, ImportRequest, ImportSummary};
use crate::error::{AppError, AppResult};
use crate::import::{self, Target};
use crate::inbox::{self, InboundDetail, InboundRow, InboxFilter, TimelineItem};
use crate::oauth;
use crate::providers::gmail::GmailProvider;
use crate::providers::{MailProvider, ProviderError};
use crate::scheduler::{QueueOverview, TickReport};

type S<'a> = State<'a, AppState>;

fn now(s: &AppState) -> chrono::DateTime<chrono::Utc> {
    s.engine.clock.now()
}

fn changed(s: &AppState) {
    s.engine.notifier.changed();
}

// ---- app ------------------------------------------------------------------

#[derive(Serialize)]
pub struct AppInfo {
    version: String,
    data_dir: String,
    outbox_dir: String,
    dry_run: bool,
    dev_build: bool,
    microsoft_configured: bool,
    welcome_done: bool,
    volume_warning: String,
    volume_warning_per_day: i64,
    account_types: Vec<(String, String, i64, i64)>,
}

#[tauri::command]
pub async fn app_info(s: S<'_>, app: tauri::AppHandle) -> AppResult<AppInfo> {
    let settings = app::load_settings(&s.db.lock());
    Ok(AppInfo {
        version: app.package_info().version.to_string(),
        data_dir: s.data_dir.to_string_lossy().into_owned(),
        outbox_dir: s.outbox_dir().to_string_lossy().into_owned(),
        dry_run: settings.dry_run,
        dev_build: cfg!(debug_assertions),
        microsoft_configured: config::ms_client_id().is_some(),
        welcome_done: settings.welcome_done,
        volume_warning: config::VOLUME_WARNING.into(),
        volume_warning_per_day: config::VOLUME_WARNING_PER_DAY,
        account_types: accounts::ACCOUNT_TYPES
            .iter()
            .map(|(id, label, p, d)| (id.to_string(), label.to_string(), *p, *d))
            .collect(),
    })
}

#[tauri::command]
pub async fn get_settings(s: S<'_>) -> AppResult<Settings> {
    Ok(app::load_settings(&s.db.lock()))
}

#[tauri::command]
pub async fn save_settings(s: S<'_>, settings: Settings) -> AppResult<()> {
    app::save_settings(&s.db.lock(), &settings)?;
    s.poll_wake.notify_one();
    changed(&s);
    Ok(())
}

// ---- accounts -------------------------------------------------------------

#[derive(Serialize)]
pub struct AccountView {
    #[serde(flatten)]
    account: Account,
    sent_24h: i64,
}

#[tauri::command]
pub async fn list_accounts(s: S<'_>) -> AppResult<Vec<AccountView>> {
    let conn = s.db.lock();
    let n = now(&s);
    accounts::list(&conn)?
        .into_iter()
        .map(|a| Ok(AccountView { sent_24h: accounts::sends_last_24h(&conn, a.id, n)?, account: a }))
        .collect()
}

fn provider_err(e: ProviderError) -> AppError {
    AppError::User(e.to_string())
}

#[tauri::command]
pub async fn add_gmail(s: S<'_>, email: String, app_password: String, display_name: String) -> AppResult<Account> {
    let email = import::normalise_email(&email);
    if !import::is_valid_email(&email) {
        return Err(AppError::user("Enter your full Gmail address."));
    }
    let password: String = app_password.chars().filter(|c| !c.is_whitespace()).collect();
    if password.is_empty() {
        return Err(AppError::user("Paste the app password Google gave you."));
    }
    let provider = GmailProvider { email: email.clone(), display_name: display_name.clone(), app_password: password.clone() };
    let info = provider.test().await.map_err(provider_err)?;
    let existed = accounts::find_by_email(&s.db.lock(), &email)?.is_some();
    let account = accounts::upsert(
        &s.db.lock(),
        &NewAccount { provider: Provider::Gmail, email: &email, display_name: display_name.trim(), account_type: &info.account_type },
        now(&s),
    )?;
    if let Err(e) = s.secrets.set(&account.secret_key(), &password) {
        if !existed {
            accounts::delete(&s.db.lock(), account.id)?;
        }
        return Err(AppError::User(e));
    }
    changed(&s);
    Ok(account)
}

#[tauri::command]
pub async fn sign_in_microsoft(s: S<'_>) -> AppResult<Account> {
    let result = oauth::sign_in(&s.http, |url| {
        tauri_plugin_opener::open_url(url, None::<&str>).map_err(|e| e.to_string())
    })
    .await
    .map_err(provider_err)?;
    let existed = accounts::find_by_email(&s.db.lock(), &result.email)?.is_some();
    let account = accounts::upsert(
        &s.db.lock(),
        &NewAccount {
            provider: Provider::Microsoft,
            email: &result.email,
            display_name: &result.display_name,
            account_type: &result.account_type,
        },
        now(&s),
    )?;
    let refresh = result.tokens.refresh_token.clone().unwrap_or_default();
    if let Err(e) = s.secrets.set(&account.secret_key(), &refresh) {
        if !existed {
            accounts::delete(&s.db.lock(), account.id)?;
        }
        return Err(AppError::User(e));
    }
    let exp = chrono::Utc::now() + chrono::Duration::seconds(result.tokens.expires_in.max(60));
    s.tokens.lock().unwrap().insert(account.id, (result.tokens.access_token, exp));
    changed(&s);
    Ok(account)
}

/// Re-enter a Gmail app password for an existing account.
#[tauri::command]
pub async fn update_gmail_password(s: S<'_>, id: i64, app_password: String) -> AppResult<Account> {
    let a = accounts::get(&s.db.lock(), id)?;
    let password: String = app_password.chars().filter(|c| !c.is_whitespace()).collect();
    let provider = GmailProvider { email: a.email.clone(), display_name: a.display_name.clone(), app_password: password.clone() };
    provider.test().await.map_err(provider_err)?;
    s.secrets.set(&a.secret_key(), &password).map_err(AppError::User)?;
    accounts::set_status(&s.db.lock(), id, "connected", None)?;
    s.tick_wake.notify_one();
    changed(&s);
    accounts::get(&s.db.lock(), id)
}

/// Check the connection for real (never dry-run). Updates the status.
#[tauri::command]
pub async fn test_account(s: S<'_>, id: i64) -> AppResult<Account> {
    let a = accounts::get(&s.db.lock(), id)?;
    let provider = s.engine.providers.for_account(&a, false);
    let result = match provider {
        Ok(p) => p.test().await,
        Err(e) => Err(e),
    };
    {
        let conn = s.db.lock();
        match &result {
            Ok(_) => accounts::set_status(&conn, id, "connected", None)?,
            Err(ProviderError::Auth(m)) => accounts::set_status(&conn, id, "needs_attention", Some(m))?,
            Err(_) => {}
        }
    }
    changed(&s);
    if let Err(e) = result {
        if !matches!(e, ProviderError::Auth(_)) {
            return Err(provider_err(e));
        }
    }
    s.tick_wake.notify_one();
    accounts::get(&s.db.lock(), id)
}

#[tauri::command]
pub async fn update_account(s: S<'_>, id: i64, update: AccountUpdate) -> AppResult<Account> {
    let a = accounts::update(&s.db.lock(), id, &update)?;
    s.tick_wake.notify_one();
    changed(&s);
    Ok(a)
}

#[tauri::command]
pub async fn remove_account(s: S<'_>, id: i64) -> AppResult<()> {
    accounts::delete(&s.db.lock(), id)?;
    s.secrets.delete(&accounts::secret_key(id)).map_err(AppError::User)?;
    s.tokens.lock().unwrap().remove(&id);
    changed(&s);
    Ok(())
}

// ---- contacts -------------------------------------------------------------

#[tauri::command]
pub async fn list_contacts(s: S<'_>, query: ContactQuery) -> AppResult<ContactPage> {
    contacts::query(&s.db.lock(), &query)
}

#[tauri::command]
pub async fn get_contact(s: S<'_>, id: i64) -> AppResult<Contact> {
    contacts::get(&s.db.lock(), id)
}

#[tauri::command]
pub async fn save_contact(s: S<'_>, contact: Contact) -> AppResult<Contact> {
    let c = contacts::save(&s.db.lock(), &contact, now(&s))?;
    changed(&s);
    Ok(c)
}

#[tauri::command]
pub async fn delete_contacts(s: S<'_>, ids: Vec<i64>) -> AppResult<usize> {
    let n = contacts::delete(&s.db.lock(), &ids)?;
    changed(&s);
    Ok(n)
}

#[tauri::command]
pub async fn field_names(s: S<'_>) -> AppResult<Vec<String>> {
    contacts::field_names(&s.db.lock())
}

#[tauri::command]
pub async fn list_tags(s: S<'_>) -> AppResult<Vec<String>> {
    contacts::list_tags(&s.db.lock())
}

#[tauri::command]
pub async fn list_lists(s: S<'_>) -> AppResult<Vec<ContactList>> {
    contacts::lists(&s.db.lock())
}

#[tauri::command]
pub async fn create_list(s: S<'_>, name: String) -> AppResult<i64> {
    let id = contacts::create_list(&s.db.lock(), &name, now(&s))?;
    changed(&s);
    Ok(id)
}

#[tauri::command]
pub async fn rename_list(s: S<'_>, id: i64, name: String) -> AppResult<()> {
    contacts::rename_list(&s.db.lock(), id, &name)?;
    changed(&s);
    Ok(())
}

#[tauri::command]
pub async fn delete_list(s: S<'_>, id: i64) -> AppResult<()> {
    contacts::delete_list(&s.db.lock(), id)?;
    changed(&s);
    Ok(())
}

#[tauri::command]
pub async fn add_to_list(s: S<'_>, list_id: i64, contact_ids: Vec<i64>) -> AppResult<()> {
    contacts::add_to_list(&s.db.lock(), list_id, &contact_ids)?;
    changed(&s);
    Ok(())
}

#[tauri::command]
pub async fn remove_from_list(s: S<'_>, list_id: i64, contact_ids: Vec<i64>) -> AppResult<()> {
    contacts::remove_from_list(&s.db.lock(), list_id, &contact_ids)?;
    changed(&s);
    Ok(())
}

#[derive(Serialize)]
pub struct ImportPreview {
    headers: Vec<String>,
    sample: Vec<Vec<String>>,
    total_rows: usize,
    mapping: Vec<Target>,
}

#[tauri::command]
pub async fn import_preview(path: String) -> AppResult<ImportPreview> {
    let table = tauri::async_runtime::spawn_blocking(move || import::read_table(&PathBuf::from(path)))
        .await
        .map_err(|e| AppError::User(e.to_string()))??;
    Ok(ImportPreview {
        mapping: import::suggest_mapping(&table.headers),
        sample: table.rows.iter().take(5).cloned().collect(),
        total_rows: table.rows.len(),
        headers: table.headers,
    })
}

#[tauri::command]
pub async fn import_commit(s: S<'_>, path: String, request: ImportRequest) -> AppResult<ImportSummary> {
    let table = tauri::async_runtime::spawn_blocking(move || import::read_table(&PathBuf::from(path)))
        .await
        .map_err(|e| AppError::User(e.to_string()))??;
    let summary = contacts::import(&mut s.db.lock(), &table, &request, now(&s))?;
    changed(&s);
    Ok(summary)
}

#[tauri::command]
pub async fn dnc_list(s: S<'_>) -> AppResult<Vec<DncEntry>> {
    contacts::dnc_list(&s.db.lock())
}

#[tauri::command]
pub async fn dnc_add(s: S<'_>, emails: Vec<String>) -> AppResult<usize> {
    let conn = s.db.lock();
    let mut n = 0;
    for e in emails {
        let e = import::normalise_email(&e);
        if import::is_valid_email(&e) && contacts::add_dnc(&conn, &e, "manual", now(&s))? {
            n += 1;
        }
    }
    drop(conn);
    changed(&s);
    Ok(n)
}

#[tauri::command]
pub async fn dnc_remove(s: S<'_>, email: String) -> AppResult<()> {
    contacts::remove_dnc(&s.db.lock(), &email)?;
    changed(&s);
    Ok(())
}

#[tauri::command]
pub async fn contact_timeline(s: S<'_>, contact_id: i64) -> AppResult<Vec<TimelineItem>> {
    inbox::timeline(&s.db.lock(), contact_id)
}

// ---- campaigns ------------------------------------------------------------

#[tauri::command]
pub async fn list_campaigns(s: S<'_>) -> AppResult<Vec<CampaignSummary>> {
    campaigns::list(&s.db.lock())
}

#[tauri::command]
pub async fn get_campaign(s: S<'_>, id: i64) -> AppResult<Campaign> {
    campaigns::get(&s.db.lock(), id)
}

#[tauri::command]
pub async fn save_campaign(s: S<'_>, input: CampaignInput) -> AppResult<Campaign> {
    let c = campaigns::save(&mut s.db.lock(), &input, now(&s))?;
    changed(&s);
    Ok(c)
}

#[tauri::command]
pub async fn delete_campaign(s: S<'_>, id: i64) -> AppResult<()> {
    let files = campaigns::delete(&s.db.lock(), id)?;
    for f in files {
        let _ = std::fs::remove_file(f);
    }
    changed(&s);
    Ok(())
}

#[tauri::command]
pub async fn add_attachment(s: S<'_>, step_id: i64, path: String) -> AppResult<()> {
    campaigns::add_attachment(&s.db.lock(), step_id, &PathBuf::from(path), &s.attachments_dir())
}

#[tauri::command]
pub async fn remove_attachment(s: S<'_>, id: i64) -> AppResult<()> {
    if let Some(p) = campaigns::remove_attachment(&s.db.lock(), id)? {
        let _ = std::fs::remove_file(p);
    }
    Ok(())
}

#[tauri::command]
pub async fn preview_step(s: S<'_>, campaign_id: i64, step_number: i64, index: usize) -> AppResult<Option<Preview>> {
    campaigns::preview(&s.db.lock(), campaign_id, step_number, index)
}

#[tauri::command]
pub async fn campaign_checklist(s: S<'_>, id: i64) -> AppResult<Checklist> {
    campaigns::checklist(&s.db.lock(), id, now(&s))
}

#[tauri::command]
pub async fn launch_campaign(s: S<'_>, id: i64) -> AppResult<()> {
    campaigns::launch(&mut s.db.lock(), id, now(&s))?;
    s.tick_wake.notify_one();
    changed(&s);
    Ok(())
}

#[tauri::command]
pub async fn set_campaign_status(s: S<'_>, id: i64, status: String) -> AppResult<()> {
    campaigns::set_status(&s.db.lock(), id, &status, now(&s))?;
    s.tick_wake.notify_one();
    changed(&s);
    Ok(())
}

#[tauri::command]
pub async fn enrol_new_members(s: S<'_>, id: i64) -> AppResult<usize> {
    let n = campaigns::enrol_new_members(&s.db.lock(), id, now(&s))?;
    s.tick_wake.notify_one();
    changed(&s);
    Ok(n)
}

#[tauri::command]
pub async fn campaign_stats(s: S<'_>, id: i64) -> AppResult<Stats> {
    campaigns::stats(&s.db.lock(), id)
}

#[tauri::command]
pub async fn campaign_enrolments(s: S<'_>, id: i64, status: Option<String>) -> AppResult<Vec<EnrolmentRow>> {
    campaigns::enrolments(&s.db.lock(), id, status.as_deref())
}

#[tauri::command]
pub async fn send_test(s: S<'_>, campaign_id: i64, step_number: i64, contact_id: Option<i64>) -> AppResult<String> {
    let to = s.engine.send_test(campaign_id, step_number, contact_id).await?;
    changed(&s);
    Ok(to)
}

// ---- queue & inbox --------------------------------------------------------

#[tauri::command]
pub async fn queue_overview(s: S<'_>) -> AppResult<QueueOverview> {
    s.engine.queue_overview()
}

#[tauri::command]
pub async fn run_queue_now(s: S<'_>) -> AppResult<TickReport> {
    s.engine.tick().await
}

#[tauri::command]
pub async fn check_replies_now(s: S<'_>) -> AppResult<usize> {
    s.engine.poll_all().await
}

#[tauri::command]
pub async fn list_inbox(s: S<'_>, filter: InboxFilter) -> AppResult<Vec<InboundRow>> {
    inbox::list(&s.db.lock(), &filter)
}

#[tauri::command]
pub async fn get_inbound(s: S<'_>, id: i64) -> AppResult<InboundDetail> {
    let d = inbox::get(&s.db.lock(), id)?;
    changed(&s);
    Ok(d)
}

#[tauri::command]
pub async fn unread_count(s: S<'_>) -> AppResult<i64> {
    inbox::unread_count(&s.db.lock())
}

// ---- export & backup ------------------------------------------------------

#[tauri::command]
pub async fn export_campaign_csv(s: S<'_>, id: i64, path: String) -> AppResult<usize> {
    inbox::export_campaign(&s.db.lock(), id, &PathBuf::from(path))
}

#[tauri::command]
pub async fn export_contacts_csv(s: S<'_>, path: String) -> AppResult<usize> {
    inbox::export_contacts(&s.db.lock(), &PathBuf::from(path))
}

#[tauri::command]
pub async fn backup(s: S<'_>, path: String) -> AppResult<()> {
    s.db.backup_to(&PathBuf::from(path))
}

/// Replace all data with a backup. Credentials aren't in backups, so any
/// account without a stored credential on this computer needs reconnecting.
#[tauri::command]
pub async fn restore(s: S<'_>, path: String) -> AppResult<()> {
    let path = PathBuf::from(path);
    let db = s.db.clone();
    s.engine.exclusive(|| db.replace_with(&path)).await?;
    let conn = s.db.lock();
    for a in accounts::list(&conn)? {
        let has = s.secrets.get(&a.secret_key()).ok().flatten().is_some();
        if !has {
            accounts::set_status(&conn, a.id, "needs_attention", Some("Reconnect this account after restoring a backup."))?;
        }
    }
    drop(conn);
    s.tokens.lock().unwrap().clear();
    changed(&s);
    Ok(())
}
