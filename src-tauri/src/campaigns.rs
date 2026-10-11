//! Campaigns (sequences): steps, launch checklist, enrolment and stats.

use std::collections::HashMap;
use std::path::Path;

use chrono::{DateTime, Duration, Utc};
use rusqlite::{params, Connection, OptionalExtension, Row};
use serde::{Deserialize, Serialize};

use crate::accounts::{self, Account};
use crate::clock::{parse_ts, ts};
use crate::config;
use crate::contacts::{self, Contact};
use crate::error::{AppError, AppResult};
use crate::import::is_valid_email;
use crate::templates::{assemble_body, render, reply_subject};
use crate::window::SendWindow;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StepAttachment {
    pub id: i64,
    pub file_name: String,
    pub size_bytes: i64,
    #[serde(skip)]
    pub stored_path: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Step {
    pub id: i64,
    pub step_number: i64,
    pub delay_days: i64,
    pub subject: String,
    pub body: String,
    pub same_thread: bool,
    pub attachments: Vec<StepAttachment>,
}

#[derive(Debug, Clone, Serialize)]
pub struct Campaign {
    pub id: i64,
    pub name: String,
    pub account_id: Option<i64>,
    pub list_id: Option<i64>,
    /// draft, running, paused, stopped, finished
    pub status: String,
    pub window: SendWindow,
    pub gap_min_secs: i64,
    pub gap_max_secs: i64,
    pub footer: String,
    pub created_at: String,
    pub started_at: Option<String>,
    pub finished_at: Option<String>,
    pub steps: Vec<Step>,
}

impl Campaign {
    fn from_row(r: &Row) -> rusqlite::Result<Self> {
        Ok(Campaign {
            id: r.get("id")?,
            name: r.get("name")?,
            account_id: r.get("account_id")?,
            list_id: r.get("list_id")?,
            status: r.get("status")?,
            window: SendWindow::from_db(
                &r.get::<_, String>("window_days")?,
                r.get("window_start")?,
                r.get("window_end")?,
            ),
            gap_min_secs: r.get("gap_min_secs")?,
            gap_max_secs: r.get("gap_max_secs")?,
            footer: r.get("footer")?,
            created_at: r.get("created_at")?,
            started_at: r.get("started_at")?,
            finished_at: r.get("finished_at")?,
            steps: Vec::new(),
        })
    }

    pub fn step(&self, n: i64) -> Option<&Step> {
        self.steps.iter().find(|s| s.step_number == n)
    }
}

pub fn get(conn: &Connection, id: i64) -> AppResult<Campaign> {
    let mut c = conn
        .query_row("SELECT * FROM campaigns WHERE id = ?1", [id], Campaign::from_row)
        .optional()?
        .ok_or_else(|| AppError::NotFound("campaign".into()))?;
    c.steps = steps(conn, id)?;
    Ok(c)
}

fn steps(conn: &Connection, campaign_id: i64) -> AppResult<Vec<Step>> {
    let mut stmt = conn.prepare(
        "SELECT id, step_number, delay_days, subject, body, same_thread FROM campaign_steps
         WHERE campaign_id = ?1 ORDER BY step_number",
    )?;
    let mut list: Vec<Step> = stmt
        .query_map([campaign_id], |r| {
            Ok(Step {
                id: r.get(0)?,
                step_number: r.get(1)?,
                delay_days: r.get(2)?,
                subject: r.get(3)?,
                body: r.get(4)?,
                same_thread: r.get(5)?,
                attachments: Vec::new(),
            })
        })?
        .collect::<Result<_, _>>()?;
    let mut att = conn.prepare(
        "SELECT id, file_name, size_bytes, stored_path FROM step_attachments WHERE step_id = ?1 ORDER BY id",
    )?;
    for s in &mut list {
        s.attachments = att
            .query_map([s.id], |r| {
                Ok(StepAttachment { id: r.get(0)?, file_name: r.get(1)?, size_bytes: r.get(2)?, stored_path: r.get(3)? })
            })?
            .collect::<Result<_, _>>()?;
    }
    Ok(list)
}

#[derive(Debug, Serialize)]
pub struct CampaignSummary {
    pub id: i64,
    pub name: String,
    pub status: String,
    pub account_email: Option<String>,
    pub list_name: Option<String>,
    pub steps: i64,
    pub stats: Stats,
    pub created_at: String,
}

pub fn list(conn: &Connection) -> AppResult<Vec<CampaignSummary>> {
    let mut stmt = conn.prepare(
        "SELECT c.id, c.name, c.status, a.email, l.name, c.created_at,
                (SELECT COUNT(*) FROM campaign_steps s WHERE s.campaign_id = c.id)
         FROM campaigns c LEFT JOIN accounts a ON a.id = c.account_id LEFT JOIN lists l ON l.id = c.list_id
         ORDER BY CASE c.status WHEN 'running' THEN 0 WHEN 'paused' THEN 1 WHEN 'draft' THEN 2 ELSE 3 END, c.created_at DESC",
    )?;
    let rows: Vec<(i64, String, String, Option<String>, Option<String>, String, i64)> = stmt
        .query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?, r.get(5)?, r.get(6)?)))?
        .collect::<Result<_, _>>()?;
    rows.into_iter()
        .map(|(id, name, status, account_email, list_name, created_at, steps)| {
            Ok(CampaignSummary { id, name, status, account_email, list_name, steps, stats: stats(conn, id)?, created_at })
        })
        .collect()
}

#[derive(Debug, Deserialize)]
pub struct StepInput {
    pub id: Option<i64>,
    pub delay_days: i64,
    pub subject: String,
    pub body: String,
    pub same_thread: bool,
}

#[derive(Debug, Deserialize)]
pub struct CampaignInput {
    pub id: Option<i64>,
    pub name: String,
    pub account_id: Option<i64>,
    pub list_id: Option<i64>,
    pub window: SendWindow,
    pub gap_min_secs: i64,
    pub gap_max_secs: i64,
    pub footer: String,
    pub steps: Vec<StepInput>,
}

/// Create or edit a campaign. Only drafts and paused campaigns can be edited;
/// edits to a paused campaign apply to emails not yet sent.
pub fn save(conn: &mut Connection, input: &CampaignInput, now: DateTime<Utc>) -> AppResult<Campaign> {
    if input.name.trim().is_empty() {
        return Err(AppError::user("Give the campaign a name."));
    }
    input.window.validate().map_err(AppError::User)?;
    if input.gap_min_secs < 5 || input.gap_max_secs < input.gap_min_secs || input.gap_max_secs > 3600 {
        return Err(AppError::user(
            "The gap between sends must be at least 5 seconds, at most an hour, and the minimum can't be above the maximum.",
        ));
    }
    if input.steps.is_empty() {
        return Err(AppError::user("A campaign needs at least one email."));
    }
    for (i, s) in input.steps.iter().enumerate().skip(1) {
        if s.delay_days < 1 {
            return Err(AppError::user(format!("Follow-up {} must wait at least 1 day.", i)));
        }
    }
    let tx = conn.transaction()?;
    let id = match input.id {
        Some(id) => {
            let current = get(&tx, id)?;
            if !matches!(current.status.as_str(), "draft" | "paused") {
                return Err(AppError::user("Pause the campaign before editing it."));
            }
            if current.status != "draft" && (current.account_id != input.account_id || current.list_id != input.list_id) {
                return Err(AppError::user(
                    "The sending account and contact list can't be changed after a campaign has started.",
                ));
            }
            tx.execute(
                "UPDATE campaigns SET name = ?2, account_id = ?3, list_id = ?4, window_days = ?5, window_start = ?6,
                    window_end = ?7, gap_min_secs = ?8, gap_max_secs = ?9, footer = ?10 WHERE id = ?1",
                params![
                    id, input.name.trim(), input.account_id, input.list_id, input.window.days_to_db(),
                    input.window.start_min, input.window.end_min, input.gap_min_secs, input.gap_max_secs, input.footer
                ],
            )?;
            id
        }
        None => {
            tx.execute(
                "INSERT INTO campaigns (name, account_id, list_id, window_days, window_start, window_end,
                    gap_min_secs, gap_max_secs, footer, created_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
                params![
                    input.name.trim(), input.account_id, input.list_id, input.window.days_to_db(),
                    input.window.start_min, input.window.end_min, input.gap_min_secs, input.gap_max_secs,
                    input.footer, ts(now)
                ],
            )?;
            tx.last_insert_rowid()
        }
    };

    // Steps: keep ids (and their attachments) where given; renumber in order.
    let existing: Vec<i64> = steps(&tx, id)?.iter().map(|s| s.id).collect();
    let keep: Vec<i64> = input.steps.iter().filter_map(|s| s.id).filter(|i| existing.contains(i)).collect();
    for old in existing.iter().filter(|i| !keep.contains(i)) {
        tx.execute("DELETE FROM campaign_steps WHERE id = ?1", [old])?;
    }
    tx.execute("UPDATE campaign_steps SET step_number = -step_number WHERE campaign_id = ?1", [id])?;
    for (i, s) in input.steps.iter().enumerate() {
        let n = i as i64 + 1;
        let same_thread = n > 1 && s.same_thread;
        let delay = if n == 1 { 0 } else { s.delay_days };
        match s.id.filter(|sid| keep.contains(sid)) {
            Some(sid) => {
                tx.execute(
                    "UPDATE campaign_steps SET step_number = ?2, delay_days = ?3, subject = ?4, body = ?5, same_thread = ?6 WHERE id = ?1",
                    params![sid, n, delay, s.subject, s.body, same_thread],
                )?;
            }
            None => {
                tx.execute(
                    "INSERT INTO campaign_steps (campaign_id, step_number, delay_days, subject, body, same_thread)
                     VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
                    params![id, n, delay, s.subject, s.body, same_thread],
                )?;
            }
        }
    }
    tx.commit()?;
    get(conn, id)
}

pub fn delete(conn: &Connection, id: i64) -> AppResult<Vec<String>> {
    let c = get(conn, id)?;
    if c.status == "running" {
        return Err(AppError::user("Stop the campaign before deleting it."));
    }
    let files = c.steps.iter().flat_map(|s| s.attachments.iter().map(|a| a.stored_path.clone())).collect();
    conn.execute("DELETE FROM campaigns WHERE id = ?1", [id])?;
    Ok(files)
}

pub fn add_attachment(conn: &Connection, step_id: i64, source: &Path, attachments_dir: &Path) -> AppResult<()> {
    let meta = std::fs::metadata(source)?;
    if meta.len() > config::MAX_ATTACHMENT_BYTES {
        return Err(AppError::user("Attachments can be at most 3 MB each."));
    }
    let name = source
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("attachment")
        .to_string();
    std::fs::create_dir_all(attachments_dir)?;
    let dest = attachments_dir.join(format!("{}-{}", uuid::Uuid::new_v4().simple(), name));
    std::fs::copy(source, &dest)?;
    conn.execute(
        "INSERT INTO step_attachments (step_id, file_name, stored_path, size_bytes) VALUES (?1, ?2, ?3, ?4)",
        params![step_id, name, dest.to_string_lossy(), meta.len() as i64],
    )?;
    Ok(())
}

pub fn remove_attachment(conn: &Connection, id: i64) -> AppResult<Option<String>> {
    let path: Option<String> = conn
        .query_row("SELECT stored_path FROM step_attachments WHERE id = ?1", [id], |r| r.get(0))
        .optional()?;
    conn.execute("DELETE FROM step_attachments WHERE id = ?1", [id])?;
    Ok(path)
}

// ---- rendering ------------------------------------------------------------

#[derive(Debug, Clone, Serialize)]
pub struct RenderedEmail {
    pub subject: String,
    pub body: String,
    pub missing: Vec<String>,
}

/// Render one step for one contact. `thread_subject` is the subject actually
/// sent at the start of the thread (for "Re:" follow-ups); when previewing,
/// pass `None` and the earlier step's template is used.
pub fn render_step(
    campaign: &Campaign,
    step: &Step,
    contact: &Contact,
    signature: &str,
    thread_subject: Option<&str>,
) -> RenderedEmail {
    let fields = contact.merge_fields();
    let mut missing = Vec::new();
    let subject = if step.step_number > 1 && step.same_thread {
        let original = match thread_subject {
            Some(s) => s.to_string(),
            None => {
                // The most recent step at or before this one that starts a thread.
                let root = campaign
                    .steps
                    .iter()
                    .rev()
                    .find(|s| s.step_number < step.step_number && (s.step_number == 1 || !s.same_thread));
                let r = render(root.map(|s| s.subject.as_str()).unwrap_or(""), &fields);
                missing.extend(r.missing);
                r.text
            }
        };
        reply_subject(&original)
    } else {
        let r = render(&step.subject, &fields);
        missing.extend(r.missing);
        r.text
    };
    let body = render(&step.body, &fields);
    missing.extend(body.missing);
    missing.sort();
    missing.dedup();
    RenderedEmail {
        subject,
        body: assemble_body(&body.text, signature, &campaign.footer),
        missing,
    }
}

/// Fields every step needs that this contact lacks.
pub fn missing_for(campaign: &Campaign, contact: &Contact) -> Vec<String> {
    let mut all: Vec<String> = campaign
        .steps
        .iter()
        .flat_map(|s| render_step(campaign, s, contact, "", None).missing)
        .collect();
    all.sort();
    all.dedup();
    all
}

#[derive(Debug, Serialize)]
pub struct Preview {
    pub contact_id: i64,
    pub contact_email: String,
    pub contact_name: String,
    pub index: usize,
    pub total: usize,
    pub step_number: i64,
    pub to: String,
    pub from: String,
    pub email: RenderedEmail,
    pub do_not_contact: bool,
}

pub fn audience(conn: &Connection, c: &Campaign) -> AppResult<Vec<i64>> {
    if c.status == "draft" {
        match c.list_id {
            Some(l) => contacts::list_member_ids(conn, l),
            None => Ok(Vec::new()),
        }
    } else {
        let mut stmt = conn.prepare("SELECT contact_id FROM enrolments WHERE campaign_id = ?1 ORDER BY id")?;
        let r = stmt.query_map([c.id], |r| r.get(0))?.collect::<Result<_, _>>()?;
        Ok(r)
    }
}

pub fn preview(conn: &Connection, campaign_id: i64, step_number: i64, index: usize) -> AppResult<Option<Preview>> {
    let c = get(conn, campaign_id)?;
    let step = c.step(step_number).ok_or_else(|| AppError::NotFound("step".into()))?;
    let ids = audience(conn, &c)?;
    if ids.is_empty() {
        return Ok(None);
    }
    let index = index.min(ids.len() - 1);
    let contact = contacts::get(conn, ids[index])?;
    let account = match c.account_id {
        Some(a) => Some(accounts::get(conn, a)?),
        None => None,
    };
    let signature = account.as_ref().map(|a| a.signature.as_str()).unwrap_or("");
    let email = render_step(&c, step, &contact, signature, None);
    Ok(Some(Preview {
        contact_id: contact.id,
        contact_email: contact.email.clone(),
        contact_name: contact.full_name(),
        index,
        total: ids.len(),
        step_number,
        to: contact.email.clone(),
        from: account.map(|a| a.email).unwrap_or_default(),
        email,
        do_not_contact: contact.do_not_contact,
    }))
}

// ---- launch checklist -----------------------------------------------------

#[derive(Debug, Serialize)]
pub struct MissingRow {
    pub contact_id: i64,
    pub email: String,
    pub fields: Vec<String>,
}

#[derive(Debug, Serialize)]
pub struct Checklist {
    pub campaign_id: i64,
    pub account_email: Option<String>,
    pub contacts_total: usize,
    pub will_send: usize,
    pub skipped_dnc: usize,
    pub skipped_invalid: usize,
    pub skipped_missing: Vec<MissingRow>,
    pub active_elsewhere: usize,
    pub steps: usize,
    pub per_day: i64,
    pub est_sending_days: i64,
    pub est_total_days: i64,
    pub preview: Option<Preview>,
    pub volume_warning: Option<String>,
    pub has_attachments: bool,
    /// Problems that block launching.
    pub problems: Vec<String>,
}

/// Emails per sending day this campaign can manage on this account.
pub fn per_day_capacity(c: &Campaign, a: &Account) -> i64 {
    let avg_gap = ((c.gap_min_secs + c.gap_max_secs) as f64 / 2.0).max(1.0);
    let by_window = (c.window.minutes_per_day() as f64 * 60.0 / avg_gap).floor() as i64;
    by_window.min(a.daily_limit).max(1)
}

pub fn checklist(conn: &Connection, campaign_id: i64, now: DateTime<Utc>) -> AppResult<Checklist> {
    let c = get(conn, campaign_id)?;
    let mut problems = Vec::new();
    let account = match c.account_id {
        Some(id) => accounts::get(conn, id).ok(),
        None => None,
    };
    match &account {
        None => problems.push("Choose which account sends this campaign.".to_string()),
        Some(a) if a.status != "connected" => problems.push(format!(
            "{} needs attention: {}",
            a.email,
            a.status_message.clone().unwrap_or_else(|| "check the account on the Accounts page.".into())
        )),
        _ => {}
    }
    if c.list_id.is_none() {
        problems.push("Choose a contact list.".into());
    }
    for s in &c.steps {
        let label = if s.step_number == 1 { "The first email".to_string() } else { format!("Follow-up {}", s.step_number - 1) };
        if (s.step_number == 1 || !s.same_thread) && s.subject.trim().is_empty() {
            problems.push(format!("{label} needs a subject."));
        }
        if s.body.trim().is_empty() {
            problems.push(format!("{label} is empty."));
        }
    }
    if c.steps.is_empty() {
        problems.push("Add at least one email.".into());
    }

    let ids = match c.list_id {
        Some(l) => contacts::list_member_ids(conn, l)?,
        None => Vec::new(),
    };
    let mut will_send = 0;
    let (mut dnc, mut invalid) = (0, 0);
    let mut missing_rows = Vec::new();
    let mut active_elsewhere = 0;
    for id in &ids {
        let contact = contacts::get(conn, *id)?;
        if contact.do_not_contact {
            dnc += 1;
            continue;
        }
        if !is_valid_email(&contact.email) {
            invalid += 1;
            continue;
        }
        let missing = missing_for(&c, &contact);
        if !missing.is_empty() {
            missing_rows.push(MissingRow { contact_id: contact.id, email: contact.email.clone(), fields: missing });
            continue;
        }
        will_send += 1;
        let elsewhere: i64 = conn.query_row(
            "SELECT COUNT(*) FROM enrolments e JOIN campaigns c ON c.id = e.campaign_id
             WHERE e.contact_id = ?1 AND e.campaign_id != ?2 AND c.status IN ('running', 'paused')
               AND e.status IN ('queued', 'waiting', 'sending')",
            params![id, c.id],
            |r| r.get(0),
        )?;
        if elsewhere > 0 {
            active_elsewhere += 1;
        }
    }
    if ids.is_empty() && c.list_id.is_some() {
        problems.push("That contact list is empty.".into());
    } else if will_send == 0 && !ids.is_empty() {
        problems.push("Every contact on the list would be skipped.".into());
    }

    let per_day = account.as_ref().map(|a| per_day_capacity(&c, a)).unwrap_or(1);
    let est_sending_days = ((will_send as f64) / per_day as f64).ceil() as i64;
    let sending_days_per_week = c.window.days.iter().filter(|d| **d).count().max(1) as f64;
    let calendar = |sending_days: i64| ((sending_days as f64) * 7.0 / sending_days_per_week).ceil() as i64;
    let delays: i64 = c.steps.iter().skip(1).map(|s| s.delay_days).sum();
    let est_total_days = calendar(est_sending_days) + delays;

    let new_account = account
        .as_ref()
        .and_then(|a| parse_ts(&a.connected_at))
        .is_some_and(|t| now - t < Duration::days(config::NEW_ACCOUNT_DAYS));
    let daily_volume = per_day.min(will_send as i64);
    let volume_warning = (daily_volume > config::VOLUME_WARNING_PER_DAY || (new_account && will_send > 20)).then(|| {
        let mut w = String::new();
        if daily_volume > config::VOLUME_WARNING_PER_DAY {
            w.push_str(&format!("This campaign could send up to {daily_volume} emails a day from one account. "));
        }
        if new_account {
            w.push_str("This account was connected recently. ");
        }
        w.push_str(config::VOLUME_WARNING);
        w
    });

    let preview = if c.steps.is_empty() { None } else { preview(conn, c.id, 1, 0)? };
    Ok(Checklist {
        campaign_id: c.id,
        account_email: account.map(|a| a.email),
        contacts_total: ids.len(),
        will_send,
        skipped_dnc: dnc,
        skipped_invalid: invalid,
        skipped_missing: missing_rows,
        active_elsewhere,
        steps: c.steps.len(),
        per_day,
        est_sending_days,
        est_total_days,
        preview,
        volume_warning,
        has_attachments: c.steps.iter().any(|s| !s.attachments.is_empty()),
        problems,
    })
}

/// Enrol the list and start sending.
pub fn launch(conn: &mut Connection, campaign_id: i64, now: DateTime<Utc>) -> AppResult<Checklist> {
    let check = checklist(conn, campaign_id, now)?;
    if let Some(p) = check.problems.first() {
        return Err(AppError::User(p.clone()));
    }
    let c = get(conn, campaign_id)?;
    if c.status != "draft" {
        return Err(AppError::user("This campaign has already been started."));
    }
    let tx = conn.transaction()?;
    enrol(&tx, &c, now)?;
    tx.execute(
        "UPDATE campaigns SET status = 'running', started_at = ?2 WHERE id = ?1",
        params![campaign_id, ts(now)],
    )?;
    tx.commit()?;
    Ok(check)
}

/// Add list members not yet in the campaign. Returns how many were added.
pub fn enrol(conn: &Connection, c: &Campaign, now: DateTime<Utc>) -> AppResult<usize> {
    let Some(list) = c.list_id else { return Ok(0) };
    let mut added = 0;
    for id in contacts::list_member_ids(conn, list)? {
        let contact = contacts::get(conn, id)?;
        let (status, reason): (&str, Option<String>) = if contact.do_not_contact {
            ("skipped", Some("On the do-not-contact list".into()))
        } else if !is_valid_email(&contact.email) {
            ("skipped", Some("Invalid email address".into()))
        } else {
            let missing = missing_for(c, &contact);
            if missing.is_empty() {
                ("queued", None)
            } else {
                ("skipped", Some(format!("Missing: {}", missing.join(", "))))
            }
        };
        added += conn.execute(
            "INSERT OR IGNORE INTO enrolments (campaign_id, contact_id, status, next_due_at, stop_reason, created_at, updated_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?6)",
            params![c.id, id, status, ts(now), reason, ts(now)],
        )?;
    }
    Ok(added)
}

pub fn enrol_new_members(conn: &Connection, campaign_id: i64, now: DateTime<Utc>) -> AppResult<usize> {
    let c = get(conn, campaign_id)?;
    if !matches!(c.status.as_str(), "running" | "paused") {
        return Err(AppError::user("Only running or paused campaigns can take new contacts."));
    }
    let n = enrol(conn, &c, now)?;
    Ok(n)
}

pub fn set_status(conn: &Connection, campaign_id: i64, to: &str, now: DateTime<Utc>) -> AppResult<()> {
    let c = get(conn, campaign_id)?;
    let ok = matches!(
        (c.status.as_str(), to),
        ("running", "paused") | ("paused", "running") | ("running", "stopped") | ("paused", "stopped")
    );
    if !ok {
        return Err(AppError::user(format!("A {} campaign can't be {}.", c.status, match to {
            "running" => "resumed",
            "paused" => "paused",
            _ => "stopped",
        })));
    }
    conn.execute("UPDATE campaigns SET status = ?2 WHERE id = ?1", params![campaign_id, to])?;
    if to == "stopped" {
        conn.execute("UPDATE campaigns SET finished_at = ?2 WHERE id = ?1", params![campaign_id, ts(now)])?;
    }
    Ok(())
}

/// Mark a running campaign finished once nobody is left to email.
pub fn maybe_finish(conn: &Connection, campaign_id: i64, now: DateTime<Utc>) -> AppResult<()> {
    let open: i64 = conn.query_row(
        "SELECT COUNT(*) FROM enrolments WHERE campaign_id = ?1 AND status IN ('queued', 'waiting', 'sending')",
        [campaign_id],
        |r| r.get(0),
    )?;
    if open == 0 {
        conn.execute(
            "UPDATE campaigns SET status = 'finished', finished_at = ?2 WHERE id = ?1 AND status = 'running'",
            params![campaign_id, ts(now)],
        )?;
    }
    Ok(())
}

// ---- stats ----------------------------------------------------------------

#[derive(Debug, Default, Serialize, PartialEq)]
pub struct Stats {
    pub contacts: i64,
    pub emails_sent: i64,
    pub contacted: i64,
    pub replied: i64,
    pub reply_rate: f64,
    pub bounced: i64,
    pub unsubscribed: i64,
    pub auto_replies: i64,
    pub waiting: i64,
    pub finished_no_reply: i64,
    pub failed: i64,
    pub skipped: i64,
    /// Emails that only went to the dry-run folder.
    pub dry_run_sent: i64,
}

pub fn stats(conn: &Connection, campaign_id: i64) -> AppResult<Stats> {
    let mut s = Stats::default();
    let mut stmt = conn.prepare("SELECT status, COUNT(*) FROM enrolments WHERE campaign_id = ?1 GROUP BY status")?;
    let counts: HashMap<String, i64> = stmt
        .query_map([campaign_id], |r| Ok((r.get(0)?, r.get(1)?)))?
        .collect::<Result<_, _>>()?;
    let n = |k: &str| counts.get(k).copied().unwrap_or(0);
    s.contacts = counts.values().sum();
    s.replied = n("replied");
    s.bounced = n("bounced");
    s.unsubscribed = n("unsubscribed");
    s.waiting = n("queued") + n("waiting") + n("sending");
    s.finished_no_reply = n("finished");
    s.failed = n("failed");
    s.skipped = n("skipped");
    s.emails_sent = conn.query_row(
        "SELECT COUNT(*) FROM outbound o JOIN enrolments e ON e.id = o.enrolment_id WHERE e.campaign_id = ?1 AND o.status = 'sent'",
        [campaign_id],
        |r| r.get(0),
    )?;
    s.contacted = conn.query_row(
        "SELECT COUNT(DISTINCT o.enrolment_id) FROM outbound o JOIN enrolments e ON e.id = o.enrolment_id
         WHERE e.campaign_id = ?1 AND o.status = 'sent'",
        [campaign_id],
        |r| r.get(0),
    )?;
    s.dry_run_sent = conn.query_row(
        "SELECT COUNT(*) FROM outbound o JOIN enrolments e ON e.id = o.enrolment_id
         WHERE e.campaign_id = ?1 AND o.status = 'sent' AND o.dry_run = 1",
        [campaign_id],
        |r| r.get(0),
    )?;
    s.auto_replies = conn.query_row(
        "SELECT COUNT(*) FROM inbound WHERE campaign_id = ?1 AND classification = 'auto_reply'",
        [campaign_id],
        |r| r.get(0),
    )?;
    // Unsubscribe replies are replies too.
    let responders = s.replied + s.unsubscribed;
    s.reply_rate = if s.contacted > 0 { responders as f64 / s.contacted as f64 } else { 0.0 };
    Ok(s)
}

#[derive(Debug, Serialize)]
pub struct EnrolmentRow {
    pub id: i64,
    pub contact_id: i64,
    pub email: String,
    pub name: String,
    pub status: String,
    pub current_step: i64,
    pub next_due_at: Option<String>,
    pub last_sent_at: Option<String>,
    pub stop_reason: Option<String>,
}

pub fn enrolments(conn: &Connection, campaign_id: i64, status: Option<&str>) -> AppResult<Vec<EnrolmentRow>> {
    let mut stmt = conn.prepare(
        "SELECT e.id, e.contact_id, c.email, trim(c.first_name || ' ' || c.last_name), e.status, e.current_step,
                e.next_due_at, e.last_sent_at, e.stop_reason
         FROM enrolments e JOIN contacts c ON c.id = e.contact_id
         WHERE e.campaign_id = ?1 AND (?2 IS NULL OR e.status = ?2)
         ORDER BY e.id",
    )?;
    let r = stmt
        .query_map(params![campaign_id, status], |r| {
            Ok(EnrolmentRow {
                id: r.get(0)?,
                contact_id: r.get(1)?,
                email: r.get(2)?,
                name: r.get(3)?,
                status: r.get(4)?,
                current_step: r.get(5)?,
                next_due_at: r.get(6)?,
                last_sent_at: r.get(7)?,
                stop_reason: r.get(8)?,
            })
        })?
        .collect::<Result<_, _>>()?;
    Ok(r)
}
