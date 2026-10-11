//! Connected sending accounts. Secrets are never stored here — see `secrets.rs`.

use chrono::{DateTime, Duration, Utc};
use rusqlite::{params, Connection, OptionalExtension, Row};
use serde::{Deserialize, Serialize};

use crate::clock::{parse_ts, ts};
use crate::error::{AppError, AppResult};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Provider {
    Gmail,
    Microsoft,
}

impl Provider {
    pub fn as_str(self) -> &'static str {
        match self {
            Provider::Gmail => "gmail",
            Provider::Microsoft => "microsoft",
        }
    }
    fn parse(s: &str) -> Self {
        if s == "microsoft" {
            Provider::Microsoft
        } else {
            Provider::Gmail
        }
    }
}

/// Account types and their provider limits (approximate, rolling 24 hours).
pub const ACCOUNT_TYPES: &[(&str, &str, i64, i64)] = &[
    // (id, label, provider limit, our default)
    ("gmail_free", "Gmail (free)", 500, 450),
    ("google_workspace", "Google Workspace", 2000, 1800),
    ("outlook_personal", "Outlook.com / Hotmail / Live", 300, 280),
    ("microsoft_365", "Microsoft 365 (work or school)", 10000, 9000),
];

pub fn default_limit(account_type: &str) -> i64 {
    ACCOUNT_TYPES
        .iter()
        .find(|t| t.0 == account_type)
        .map(|t| t.3)
        .unwrap_or(280)
}

pub fn provider_limit(account_type: &str) -> i64 {
    ACCOUNT_TYPES
        .iter()
        .find(|t| t.0 == account_type)
        .map(|t| t.2)
        .unwrap_or(300)
}

/// Microsoft 365 also caps messages per minute.
pub fn per_minute_limit(account_type: &str) -> Option<i64> {
    (account_type == "microsoft_365").then_some(30)
}

pub fn gmail_type_for(email: &str) -> &'static str {
    let domain = email.rsplit('@').next().unwrap_or("").to_ascii_lowercase();
    if domain == "gmail.com" || domain == "googlemail.com" {
        "gmail_free"
    } else {
        "google_workspace"
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct Account {
    pub id: i64,
    pub provider: Provider,
    pub email: String,
    pub display_name: String,
    pub account_type: String,
    pub daily_limit: i64,
    /// "connected" or "needs_attention".
    pub status: String,
    pub status_message: Option<String>,
    pub paused: bool,
    pub limit_paused_until: Option<String>,
    pub next_send_at: Option<String>,
    pub signature: String,
    pub poll_state: Option<String>,
    pub last_polled_at: Option<String>,
    pub connected_at: String,
    pub created_at: String,
}

impl Account {
    fn from_row(r: &Row) -> rusqlite::Result<Self> {
        Ok(Account {
            id: r.get("id")?,
            provider: Provider::parse(&r.get::<_, String>("provider")?),
            email: r.get("email")?,
            display_name: r.get("display_name")?,
            account_type: r.get("account_type")?,
            daily_limit: r.get("daily_limit")?,
            status: r.get("status")?,
            status_message: r.get("status_message")?,
            paused: r.get("paused")?,
            limit_paused_until: r.get("limit_paused_until")?,
            next_send_at: r.get("next_send_at")?,
            signature: r.get("signature")?,
            poll_state: r.get("poll_state")?,
            last_polled_at: r.get("last_polled_at")?,
            connected_at: r.get("connected_at")?,
            created_at: r.get("created_at")?,
        })
    }

    /// Healthy and not paused, ignoring limits.
    pub fn can_send(&self, now: DateTime<Utc>) -> bool {
        self.status == "connected"
            && !self.paused
            && self
                .limit_paused_until
                .as_deref()
                .and_then(parse_ts)
                .is_none_or(|t| t <= now)
    }

    pub fn secret_key(&self) -> String {
        secret_key(self.id)
    }
}

pub fn secret_key(id: i64) -> String {
    format!("account-{id}")
}

pub fn list(conn: &Connection) -> AppResult<Vec<Account>> {
    let mut stmt = conn.prepare("SELECT * FROM accounts ORDER BY email")?;
    let rows = stmt.query_map([], Account::from_row)?;
    Ok(rows.collect::<Result<_, _>>()?)
}

pub fn get(conn: &Connection, id: i64) -> AppResult<Account> {
    conn.query_row("SELECT * FROM accounts WHERE id = ?1", [id], Account::from_row)
        .optional()?
        .ok_or_else(|| AppError::NotFound("account".into()))
}

pub fn find_by_email(conn: &Connection, email: &str) -> AppResult<Option<Account>> {
    Ok(conn
        .query_row("SELECT * FROM accounts WHERE email = ?1", [email], Account::from_row)
        .optional()?)
}

pub struct NewAccount<'a> {
    pub provider: Provider,
    pub email: &'a str,
    pub display_name: &'a str,
    pub account_type: &'a str,
}

/// Insert, or reconnect an existing account with the same address.
pub fn upsert(conn: &Connection, a: &NewAccount, now: DateTime<Utc>) -> AppResult<Account> {
    if let Some(existing) = find_by_email(conn, a.email)? {
        if existing.provider != a.provider {
            return Err(AppError::user(format!(
                "{} is already connected another way. Remove it first if you want to reconnect it.",
                a.email
            )));
        }
        conn.execute(
            "UPDATE accounts SET status = 'connected', status_message = NULL,
                display_name = CASE WHEN display_name = '' THEN ?2 ELSE display_name END
             WHERE id = ?1",
            params![existing.id, a.display_name],
        )?;
        return get(conn, existing.id);
    }
    let now_s = ts(now);
    conn.execute(
        "INSERT INTO accounts (provider, email, display_name, account_type, daily_limit,
                               connected_at, created_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?6)",
        params![
            a.provider.as_str(),
            a.email,
            a.display_name,
            a.account_type,
            default_limit(a.account_type),
            now_s
        ],
    )?;
    get(conn, conn.last_insert_rowid())
}

#[derive(Debug, Deserialize)]
pub struct AccountUpdate {
    pub display_name: String,
    pub signature: String,
    pub daily_limit: i64,
    pub account_type: String,
    pub paused: bool,
}

pub fn update(conn: &Connection, id: i64, u: &AccountUpdate) -> AppResult<Account> {
    if u.daily_limit < 1 {
        return Err(AppError::user("The daily limit must be at least 1."));
    }
    if !ACCOUNT_TYPES.iter().any(|t| t.0 == u.account_type) {
        return Err(AppError::user("Unknown account type."));
    }
    conn.execute(
        "UPDATE accounts SET display_name = ?2, signature = ?3, daily_limit = ?4,
                account_type = ?5, paused = ?6 WHERE id = ?1",
        params![id, u.display_name.trim(), u.signature, u.daily_limit, u.account_type, u.paused],
    )?;
    get(conn, id)
}

pub fn set_status(conn: &Connection, id: i64, status: &str, message: Option<&str>) -> AppResult<()> {
    conn.execute(
        "UPDATE accounts SET status = ?2, status_message = ?3 WHERE id = ?1",
        params![id, status, message],
    )?;
    Ok(())
}

pub fn delete(conn: &Connection, id: i64) -> AppResult<()> {
    let busy: i64 = conn.query_row(
        "SELECT COUNT(*) FROM campaigns WHERE account_id = ?1 AND status IN ('running', 'paused')",
        [id],
        |r| r.get(0),
    )?;
    if busy > 0 {
        return Err(AppError::user(
            "This account is used by a running or paused campaign. Stop that campaign first.",
        ));
    }
    conn.execute("DELETE FROM accounts WHERE id = ?1", [id])?;
    Ok(())
}

// ---- sending limits -------------------------------------------------------

pub fn sends_since(conn: &Connection, account_id: i64, since: DateTime<Utc>) -> AppResult<i64> {
    Ok(conn.query_row(
        "SELECT COUNT(*) FROM send_log WHERE account_id = ?1 AND sent_at > ?2 AND kind NOT LIKE '%dry_run'",
        params![account_id, ts(since)],
        |r| r.get(0),
    )?)
}

pub fn sends_last_24h(conn: &Connection, account_id: i64, now: DateTime<Utc>) -> AppResult<i64> {
    sends_since(conn, account_id, now - Duration::hours(24))
}

/// Is this account allowed one more send right now, by our own limits?
pub fn under_limits(conn: &Connection, a: &Account, now: DateTime<Utc>) -> AppResult<bool> {
    if sends_last_24h(conn, a.id, now)? >= a.daily_limit {
        return Ok(false);
    }
    if let Some(per_min) = per_minute_limit(&a.account_type) {
        if sends_since(conn, a.id, now - Duration::seconds(60))? >= per_min {
            return Ok(false);
        }
    }
    Ok(true)
}

/// When the rolling 24-hour count drops below the limit again.
pub fn limit_clears_at(conn: &Connection, a: &Account, now: DateTime<Utc>) -> AppResult<DateTime<Utc>> {
    let since = now - Duration::hours(24);
    let count = sends_last_24h(conn, a.id, now)?;
    // The send whose expiry brings us back under the limit.
    let skip = (count - a.daily_limit).max(0);
    let t: Option<String> = conn
        .query_row(
            "SELECT sent_at FROM send_log WHERE account_id = ?1 AND sent_at > ?2 AND kind NOT LIKE '%dry_run'
             ORDER BY sent_at LIMIT 1 OFFSET ?3",
            params![a.id, ts(since), skip],
            |r| r.get(0),
        )
        .optional()?;
    Ok(t.and_then(|s| parse_ts(&s))
        .map(|t| t + Duration::hours(24))
        .unwrap_or(now))
}

pub fn log_send(conn: &Connection, account_id: i64, at: DateTime<Utc>, kind: &str) -> AppResult<()> {
    conn.execute(
        "INSERT INTO send_log (account_id, sent_at, kind) VALUES (?1, ?2, ?3)",
        params![account_id, ts(at), kind],
    )?;
    Ok(())
}

/// The provider said "limit reached": pause until the 24-hour window clears,
/// at least an hour, so we never retry in a loop.
pub fn pause_for_provider_limit(conn: &Connection, a: &Account, now: DateTime<Utc>) -> AppResult<DateTime<Utc>> {
    let oldest: Option<String> = conn
        .query_row(
            "SELECT MIN(sent_at) FROM send_log WHERE account_id = ?1 AND sent_at > ?2 AND kind NOT LIKE '%dry_run'",
            params![a.id, ts(now - Duration::hours(24))],
            |r| r.get(0),
        )
        .optional()?
        .flatten();
    let clears = oldest
        .and_then(|s| parse_ts(&s))
        .map(|t| t + Duration::hours(24))
        .unwrap_or(now + Duration::hours(24));
    let until = clears.max(now + Duration::hours(1));
    conn.execute(
        "UPDATE accounts SET limit_paused_until = ?2 WHERE id = ?1",
        params![a.id, ts(until)],
    )?;
    Ok(until)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::Db;

    fn t(s: &str) -> DateTime<Utc> {
        parse_ts(s).unwrap()
    }

    fn new_gmail(conn: &Connection) -> Account {
        upsert(
            conn,
            &NewAccount {
                provider: Provider::Gmail,
                email: "me@gmail.com",
                display_name: "Me",
                account_type: "gmail_free",
            },
            t("2026-10-01T00:00:00Z"),
        )
        .unwrap()
    }

    #[test]
    fn defaults_just_under_provider_limit() {
        for (id, _, provider, default) in ACCOUNT_TYPES {
            assert!(default < provider, "{id}");
            assert_eq!(default_limit(id), *default);
        }
        assert_eq!(gmail_type_for("x@gmail.com"), "gmail_free");
        assert_eq!(gmail_type_for("x@acme.com"), "google_workspace");
    }

    #[test]
    fn rolling_24h_limit() {
        let db = Db::open_in_memory().unwrap();
        let conn = db.lock();
        let mut a = new_gmail(&conn);
        a.daily_limit = 3;
        let now = t("2026-10-12T12:00:00Z");
        log_send(&conn, a.id, t("2026-10-11T11:00:00Z"), "campaign").unwrap(); // >24h ago
        log_send(&conn, a.id, t("2026-10-11T13:00:00Z"), "campaign").unwrap();
        log_send(&conn, a.id, t("2026-10-12T09:00:00Z"), "campaign").unwrap();
        assert!(under_limits(&conn, &a, now).unwrap());
        log_send(&conn, a.id, t("2026-10-12T11:00:00Z"), "test").unwrap();
        assert!(!under_limits(&conn, &a, now).unwrap());
        // The 13:00 send expires at 13:00 tomorrow... i.e. today 13:00.
        assert_eq!(limit_clears_at(&conn, &a, now).unwrap(), t("2026-10-12T13:00:00Z"));
        assert!(under_limits(&conn, &a, t("2026-10-12T13:00:01Z")).unwrap());
    }

    #[test]
    fn dry_run_sends_do_not_use_the_real_limit() {
        let db = Db::open_in_memory().unwrap();
        let conn = db.lock();
        let a = new_gmail(&conn);
        let now = t("2026-10-12T12:00:00Z");
        for _ in 0..3 {
            log_send(&conn, a.id, now, "campaign_dry_run").unwrap();
            log_send(&conn, a.id, now, "test_dry_run").unwrap();
        }
        log_send(&conn, a.id, now, "test").unwrap();
        assert_eq!(sends_last_24h(&conn, a.id, now + Duration::seconds(1)).unwrap(), 1);
    }

    #[test]
    fn microsoft_365_per_minute_cap() {
        let db = Db::open_in_memory().unwrap();
        let conn = db.lock();
        let mut a = new_gmail(&conn);
        a.account_type = "microsoft_365".into();
        a.daily_limit = 9000;
        let now = t("2026-10-12T12:00:00Z");
        for i in 0..30 {
            log_send(&conn, a.id, now - Duration::seconds(i), "campaign").unwrap();
        }
        assert!(!under_limits(&conn, &a, now).unwrap());
        assert!(under_limits(&conn, &a, now + Duration::seconds(61)).unwrap());
    }

    #[test]
    fn provider_limit_pause_is_at_least_an_hour() {
        let db = Db::open_in_memory().unwrap();
        let conn = db.lock();
        let a = new_gmail(&conn);
        let now = t("2026-10-12T12:00:00Z");
        log_send(&conn, a.id, t("2026-10-11T12:30:00Z"), "campaign").unwrap();
        let until = pause_for_provider_limit(&conn, &a, now).unwrap();
        assert_eq!(until, t("2026-10-12T13:00:00Z"));
        let a = get(&conn, a.id).unwrap();
        assert!(!a.can_send(now));
        assert!(a.can_send(until));
    }

    #[test]
    fn upsert_reconnects_existing() {
        let db = Db::open_in_memory().unwrap();
        let conn = db.lock();
        let a = new_gmail(&conn);
        set_status(&conn, a.id, "needs_attention", Some("x")).unwrap();
        let b = new_gmail(&conn);
        assert_eq!(a.id, b.id);
        assert_eq!(b.status, "connected");
    }
}
