//! The sending queue and reply detection. Runs inside the app while the
//! window is open: `tick` every ~30 seconds, `poll_all` every few minutes.
//!
//! Safety rules enforced here (and tested below):
//! - never send the same step to the same contact twice — an `outbound` row is
//!   written as "sending" *before* the network call, and anything left in
//!   "sending" is checked against the provider's Sent folder before a retry;
//! - never email anyone on the do-not-contact list;
//! - catch-up after reopening goes through the same gaps and limits, never a burst.

use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex};

use chrono::{DateTime, Duration, FixedOffset, Utc};
use rand::RngExt;
use rusqlite::{params, Connection, OptionalExtension};
use serde::Serialize;

use crate::accounts::{self, Account};
use crate::campaigns::{self, Campaign};
use crate::classify::{self, Kind};
use crate::clock::{parse_ts, ts, Clock};
use crate::contacts;
use crate::db::Db;
use crate::error::AppResult;
use crate::import::is_valid_email;
use crate::providers::{
    new_message_id, Attachment, IncomingEmail, MailProvider, OutgoingEmail, ProviderError, SentInfo, ThreadRef,
};
use crate::window::{follow_up_due, SendWindow};

/// Give up on an email after this many failed attempts.
const MAX_ATTEMPTS: i64 = 5;
/// A "sending" row older than this, not in flight, gets checked against Sent.
const RECONCILE_AFTER_SECS: i64 = 60;

/// Which provider to use for an account (real, dry-run, or a test fake).
pub trait ProviderFactory: Send + Sync {
    fn for_account(&self, account: &Account, allow_dry_run: bool) -> Result<Arc<dyn MailProvider>, ProviderError>;
}

/// How the engine tells the UI something changed.
pub trait Notifier: Send + Sync {
    fn changed(&self);
    fn reply(&self, title: &str, body: &str);
}

pub struct NoopNotifier;
impl Notifier for NoopNotifier {
    fn changed(&self) {}
    fn reply(&self, _: &str, _: &str) {}
}

/// Time zone for send windows: the computer's own, or fixed (tests).
#[derive(Clone, Copy)]
pub enum Zone {
    Local,
    Fixed(FixedOffset),
}

impl Zone {
    pub fn next_open(&self, w: &SendWindow, at: DateTime<Utc>) -> Option<DateTime<Utc>> {
        match self {
            Zone::Local => w.next_open(at, &chrono::Local),
            Zone::Fixed(o) => w.next_open(at, o),
        }
    }
    pub fn follow_up_due(&self, prev: DateTime<Utc>, days: i64, w: &SendWindow) -> DateTime<Utc> {
        match self {
            Zone::Local => follow_up_due(prev, days, w, &chrono::Local),
            Zone::Fixed(o) => follow_up_due(prev, days, w, o),
        }
    }
}

pub struct Engine {
    pub db: Db,
    pub clock: Arc<dyn Clock>,
    pub providers: Arc<dyn ProviderFactory>,
    pub notifier: Arc<dyn Notifier>,
    pub zone: Zone,
    in_flight: Mutex<HashSet<i64>>,
    tick_lock: tokio::sync::Mutex<()>,
    poll_lock: tokio::sync::Mutex<()>,
}

#[derive(Debug, Default, Serialize, PartialEq)]
pub struct TickReport {
    pub sent: usize,
    pub skipped: usize,
    pub failed: usize,
}

struct Job {
    enrolment_id: i64,
    campaign_id: i64,
    contact_id: i64,
    step_number: i64,
}

enum Prepared {
    /// Nothing to send for this job (skipped, finished, or repaired).
    Done,
    /// Another attempt is still unresolved; leave it for reconciliation.
    Busy,
    Send {
        outbound_id: i64,
        email: OutgoingEmail,
        gap: (i64, i64),
    },
}

impl Engine {
    pub fn new(
        db: Db,
        clock: Arc<dyn Clock>,
        providers: Arc<dyn ProviderFactory>,
        notifier: Arc<dyn Notifier>,
        zone: Zone,
    ) -> Self {
        Engine {
            db,
            clock,
            providers,
            notifier,
            zone,
            in_flight: Mutex::new(HashSet::new()),
            tick_lock: tokio::sync::Mutex::new(()),
            poll_lock: tokio::sync::Mutex::new(()),
        }
    }

    // ---- sending ----------------------------------------------------------

    /// One pass of the queue: at most one real send per account.
    pub async fn tick(&self) -> AppResult<TickReport> {
        let _guard = self.tick_lock.lock().await;
        self.reconcile().await?;
        let mut report = TickReport::default();
        let accounts = accounts::list(&self.db.lock())?;
        for account in accounts {
            let now = self.clock.now();
            if !account.can_send(now) {
                continue;
            }
            if account.next_send_at.as_deref().and_then(parse_ts).is_some_and(|t| t > now) {
                continue;
            }
            // Skips (do-not-contact, missing fields) don't use up the gap, so
            // keep going until one real send is attempted.
            for _ in 0..500 {
                if !accounts::under_limits(&self.db.lock(), &account, now)? {
                    break;
                }
                let Some(job) = self.next_job(&account, now)? else { break };
                match self.prepare(&account, &job, now)? {
                    Prepared::Done => report.skipped += 1,
                    Prepared::Busy => continue,
                    Prepared::Send { outbound_id, email, gap } => {
                        let ok = self.send(&account, &job, outbound_id, email).await?;
                        if ok {
                            report.sent += 1;
                        } else {
                            report.failed += 1;
                        }
                        let secs = if gap.0 >= gap.1 { gap.0 } else { rand::rng().random_range(gap.0..=gap.1) };
                        let next = self.clock.now() + Duration::seconds(secs);
                        self.db.lock().execute(
                            "UPDATE accounts SET next_send_at = ?2 WHERE id = ?1",
                            params![account.id, ts(next)],
                        )?;
                        break;
                    }
                }
            }
        }
        if report != TickReport::default() {
            self.notifier.changed();
        }
        Ok(report)
    }

    /// Oldest due enrolment for this account whose campaign window is open.
    fn next_job(&self, account: &Account, now: DateTime<Utc>) -> AppResult<Option<Job>> {
        let conn = self.db.lock();
        let mut stmt = conn.prepare(
            "SELECT e.id, e.campaign_id, e.contact_id, e.current_step
             FROM enrolments e JOIN campaigns c ON c.id = e.campaign_id
             WHERE c.account_id = ?1 AND c.status = 'running'
               AND e.status IN ('queued', 'waiting') AND e.next_due_at <= ?2
             ORDER BY e.next_due_at, e.id LIMIT 200",
        )?;
        let rows: Vec<Job> = stmt
            .query_map(params![account.id, ts(now)], |r| {
                Ok(Job { enrolment_id: r.get(0)?, campaign_id: r.get(1)?, contact_id: r.get(2)?, step_number: r.get(3)? })
            })?
            .collect::<Result<_, _>>()?;
        let mut open: HashMap<i64, bool> = HashMap::new();
        for job in rows {
            let is_open = match open.get(&job.campaign_id) {
                Some(v) => *v,
                None => {
                    let c = campaigns::get(&conn, job.campaign_id)?;
                    let v = self.zone.next_open(&c.window, now) == Some(now);
                    open.insert(job.campaign_id, v);
                    v
                }
            };
            if is_open {
                return Ok(Some(job));
            }
        }
        Ok(None)
    }

    /// Everything up to the network call, in one transaction: checks, render,
    /// and the "sending" marker.
    fn prepare(&self, account: &Account, job: &Job, now: DateTime<Utc>) -> AppResult<Prepared> {
        let mut conn = self.db.lock();
        let tx = conn.transaction()?;
        let campaign = campaigns::get(&tx, job.campaign_id)?;
        let Some(step) = campaign.step(job.step_number).cloned() else {
            // Steps were removed while paused: nothing left to send.
            set_enrolment(&tx, job.enrolment_id, "finished", None, None)?;
            campaigns::maybe_finish(&tx, campaign.id, now)?;
            tx.commit()?;
            return Ok(Prepared::Done);
        };
        let contact = contacts::get(&tx, job.contact_id)?;
        if contacts::is_dnc(&tx, &contact.email)? {
            set_enrolment(&tx, job.enrolment_id, "skipped", Some("On the do-not-contact list"), None)?;
            campaigns::maybe_finish(&tx, campaign.id, now)?;
            tx.commit()?;
            return Ok(Prepared::Done);
        }
        if !is_valid_email(&contact.email) {
            set_enrolment(&tx, job.enrolment_id, "skipped", Some("Invalid email address"), None)?;
            campaigns::maybe_finish(&tx, campaign.id, now)?;
            tx.commit()?;
            return Ok(Prepared::Done);
        }

        // Existing attempt for this exact step?
        let existing: Option<(i64, String, String, i64)> = tx
            .query_row(
                "SELECT id, status, message_id, attempts FROM outbound WHERE enrolment_id = ?1 AND step_number = ?2",
                params![job.enrolment_id, step.step_number],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
            )
            .optional()?;
        match existing.as_ref().map(|e| e.1.as_str()) {
            Some("sending") => {
                set_enrolment(&tx, job.enrolment_id, "sending", None, None)?;
                tx.commit()?;
                return Ok(Prepared::Busy);
            }
            Some("sent") => {
                // Already sent (e.g. after a repair): just move on.
                let sent_at: Option<String> = tx.query_row(
                    "SELECT sent_at FROM outbound WHERE id = ?1",
                    [existing.as_ref().unwrap().0],
                    |r| r.get(0),
                )?;
                let at = sent_at.as_deref().and_then(parse_ts).unwrap_or(now);
                advance(&tx, &campaign, job.enrolment_id, step.step_number, at, self.zone)?;
                tx.commit()?;
                return Ok(Prepared::Done);
            }
            Some("failed") => {
                set_enrolment(&tx, job.enrolment_id, "failed", Some("Sending failed"), None)?;
                campaigns::maybe_finish(&tx, campaign.id, now)?;
                tx.commit()?;
                return Ok(Prepared::Done);
            }
            _ => {}
        }

        // Thread: earlier emails to this contact in this campaign.
        let prior: Vec<(i64, String, Option<String>, Option<String>, String, bool)> = {
            let mut stmt = tx.prepare(
                "SELECT step_number, message_id, internet_message_id, provider_message_id, subject, new_thread
                 FROM outbound WHERE enrolment_id = ?1 AND status = 'sent' AND step_number < ?2 ORDER BY step_number",
            )?;
            let rows = stmt
                .query_map(params![job.enrolment_id, step.step_number], |r| {
                    Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?, r.get(5)?))
                })?
                .collect::<Result<_, _>>()?;
            rows
        };
        let in_thread = step.step_number > 1 && step.same_thread && !prior.is_empty();
        let (thread, thread_subject) = if in_thread {
            let root = prior.iter().rposition(|p| p.5).unwrap_or(0);
            let chain = &prior[root..];
            let refs = chain
                .iter()
                .map(|p| p.2.clone().filter(|_| account.provider == accounts::Provider::Microsoft).unwrap_or_else(|| p.1.clone()))
                .collect();
            let last = chain.last().unwrap();
            (
                Some(ThreadRef { references: refs, reply_to_provider_id: last.3.clone() }),
                Some(chain[0].4.clone()),
            )
        } else {
            (None, None)
        };

        let rendered = campaigns::render_step(&campaign, &step, &contact, &account.signature, thread_subject.as_deref());
        if !rendered.missing.is_empty() {
            let reason = format!("Missing: {}", rendered.missing.join(", "));
            set_enrolment(&tx, job.enrolment_id, "skipped", Some(&reason), None)?;
            campaigns::maybe_finish(&tx, campaign.id, now)?;
            tx.commit()?;
            return Ok(Prepared::Done);
        }

        let (outbound_id, message_id) = match existing {
            Some((id, _, mid, _)) => {
                // A retry reuses the same Message-ID.
                tx.execute(
                    "UPDATE outbound SET status = 'sending', attempts = attempts + 1, attempted_at = ?2,
                        subject = ?3, new_thread = ?4, account_id = ?5, error = NULL WHERE id = ?1",
                    params![id, ts(now), rendered.subject, !in_thread, account.id],
                )?;
                (id, mid)
            }
            None => {
                let mid = new_message_id(&account.email);
                tx.execute(
                    "INSERT INTO outbound (enrolment_id, step_number, account_id, message_id, to_email, subject,
                        new_thread, status, attempts, attempted_at)
                     VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, 'sending', 1, ?8)",
                    params![job.enrolment_id, step.step_number, account.id, mid, contact.email, rendered.subject, !in_thread, ts(now)],
                )?;
                (tx.last_insert_rowid(), mid)
            }
        };
        set_enrolment(&tx, job.enrolment_id, "sending", None, None)?;
        tx.commit()?;

        let email = OutgoingEmail {
            from_email: account.email.clone(),
            from_name: account.display_name.clone(),
            to_email: contact.email.clone(),
            to_name: contact.full_name(),
            subject: rendered.subject,
            body: rendered.body,
            message_id,
            thread,
            list_unsubscribe: Some(format!("mailto:{}?subject=unsubscribe", account.email)),
            attachments: step
                .attachments
                .iter()
                .map(|a| Attachment { file_name: a.file_name.clone(), path: a.stored_path.clone().into() })
                .collect(),
        };
        Ok(Prepared::Send { outbound_id, email, gap: (campaign.gap_min_secs, campaign.gap_max_secs) })
    }

    /// The network call and its bookkeeping. Returns whether it was sent.
    async fn send(&self, account: &Account, job: &Job, outbound_id: i64, email: OutgoingEmail) -> AppResult<bool> {
        let provider = match self.providers.for_account(account, true) {
            Ok(p) => p,
            Err(e) => {
                self.handle_failure(account, job, outbound_id, e, true)?;
                return Ok(false);
            }
        };
        self.in_flight.lock().unwrap().insert(outbound_id);
        let result = provider.send(&email).await;
        self.in_flight.lock().unwrap().remove(&outbound_id);
        match result {
            Ok(info) => {
                let now = self.clock.now();
                let mut conn = self.db.lock();
                let tx = conn.transaction()?;
                mark_sent(&tx, outbound_id, &info, now)?;
                accounts::log_send(&tx, account.id, now, if info.dry_run { "campaign_dry_run" } else { "campaign" })?;
                let campaign = campaigns::get(&tx, job.campaign_id)?;
                advance(&tx, &campaign, job.enrolment_id, job.step_number, now, self.zone)?;
                campaigns::maybe_finish(&tx, campaign.id, now)?;
                tx.commit()?;
                tracing::info!(outbound_id, account = account.id, "sent");
                Ok(true)
            }
            Err(e) => {
                self.handle_failure(account, job, outbound_id, e, false)?;
                Ok(false)
            }
        }
    }

    /// `before_network`: the provider was never called, so nothing can have
    /// been sent and a retry is always safe.
    fn handle_failure(&self, account: &Account, job: &Job, outbound_id: i64, e: ProviderError, before_network: bool) -> AppResult<()> {
        let now = self.clock.now();
        let conn = self.db.lock();
        tracing::warn!(outbound_id, account = account.id, kind = ?std::mem::discriminant(&e), "send failed");
        match e {
            ProviderError::Auth(msg) => {
                accounts::set_status(&conn, account.id, "needs_attention", Some(&msg))?;
                retry_later(&conn, outbound_id, job.enrolment_id, &msg, now)?;
            }
            ProviderError::LimitReached(msg) => {
                accounts::pause_for_provider_limit(&conn, account, now)?;
                retry_later(&conn, outbound_id, job.enrolment_id, &msg, now)?;
            }
            ProviderError::Rejected(msg) => {
                conn.execute(
                    "UPDATE outbound SET status = 'failed', error = ?2 WHERE id = ?1",
                    params![outbound_id, msg],
                )?;
                set_enrolment(&conn, job.enrolment_id, "failed", Some(&msg), None)?;
                campaigns::maybe_finish(&conn, job.campaign_id, now)?;
            }
            ProviderError::Temporary(msg) if before_network => {
                retry_later(&conn, outbound_id, job.enrolment_id, &msg, now + Duration::minutes(10))?;
            }
            ProviderError::Temporary(msg) => {
                // The email may or may not have gone out (e.g. a timeout after
                // the server accepted it). Leave it "sending"; reconciliation
                // checks the Sent folder before any retry.
                conn.execute("UPDATE outbound SET error = ?2 WHERE id = ?1", params![outbound_id, msg])?;
            }
        }
        Ok(())
    }

    /// Resolve "sending" rows left by a crash or an uncertain failure.
    pub async fn reconcile(&self) -> AppResult<usize> {
        let now = self.clock.now();
        let cutoff = ts(now - Duration::seconds(RECONCILE_AFTER_SECS));
        let rows: Vec<(i64, i64, i64, String, String, String, i64, Option<i64>)> = {
            let conn = self.db.lock();
            let mut stmt = conn.prepare(
                "SELECT o.id, o.enrolment_id, o.step_number, o.message_id, o.to_email, o.attempted_at, o.attempts, o.account_id
                 FROM outbound o WHERE o.status = 'sending' AND o.attempted_at <= ?1",
            )?;
            let r = stmt
                .query_map([&cutoff], |r| {
                    Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?, r.get(5)?, r.get(6)?, r.get(7)?))
                })?
                .collect::<Result<_, _>>()?;
            r
        };
        let mut resolved = 0;
        for (id, enrolment_id, step_number, message_id, to, attempted_at, attempts, account_id) in rows {
            if self.in_flight.lock().unwrap().contains(&id) {
                continue;
            }
            let Some(account_id) = account_id else { continue };
            let Ok(account) = accounts::get(&self.db.lock(), account_id) else { continue };
            let Ok(provider) = self.providers.for_account(&account, true) else { continue };
            let attempted = parse_ts(&attempted_at).unwrap_or(now);
            match provider.find_sent(&message_id, &to, attempted).await {
                Ok(Some(info)) => {
                    let mut conn = self.db.lock();
                    let tx = conn.transaction()?;
                    mark_sent(&tx, id, &info, attempted)?;
                    accounts::log_send(&tx, account.id, attempted, if info.dry_run { "campaign_dry_run" } else { "campaign" })?;
                    let campaign_id: i64 = tx.query_row("SELECT campaign_id FROM enrolments WHERE id = ?1", [enrolment_id], |r| r.get(0))?;
                    let campaign = campaigns::get(&tx, campaign_id)?;
                    advance(&tx, &campaign, enrolment_id, step_number, attempted, self.zone)?;
                    campaigns::maybe_finish(&tx, campaign_id, now)?;
                    tx.commit()?;
                    resolved += 1;
                }
                Ok(None) => {
                    let conn = self.db.lock();
                    if attempts >= MAX_ATTEMPTS {
                        let msg = "Couldn't send this email after several tries.";
                        conn.execute("UPDATE outbound SET status = 'failed', error = COALESCE(error, ?2) WHERE id = ?1", params![id, msg])?;
                        set_enrolment(&conn, enrolment_id, "failed", Some(msg), None)?;
                    } else {
                        let backoff = now + Duration::minutes(10 * attempts);
                        retry_later(&conn, id, enrolment_id, "Not sent; will try again.", backoff)?;
                    }
                    resolved += 1;
                }
                Err(_) => {} // Provider unreachable: leave it, try again later.
            }
        }
        if resolved > 0 {
            self.notifier.changed();
        }
        Ok(resolved)
    }

    /// Send one step to the account's own address, marked as a test.
    pub async fn send_test(&self, campaign_id: i64, step_number: i64, contact_id: Option<i64>) -> AppResult<String> {
        let now = self.clock.now();
        let (account, email) = {
            let conn = self.db.lock();
            let campaign = campaigns::get(&conn, campaign_id)?;
            let account_id = campaign
                .account_id
                .ok_or_else(|| crate::error::AppError::user("Choose a sending account first."))?;
            let account = accounts::get(&conn, account_id)?;
            let step = campaign
                .step(step_number)
                .ok_or_else(|| crate::error::AppError::NotFound("step".into()))?;
            let contact = match contact_id.or_else(|| campaigns::audience(&conn, &campaign).ok()?.first().copied()) {
                Some(id) => contacts::get(&conn, id)?,
                None => sample_contact(),
            };
            let mut rendered = campaigns::render_step(&campaign, step, &contact, &account.signature, None);
            if !rendered.missing.is_empty() {
                // Show the gaps rather than refusing: it's only going to you.
                let mut c = contact.clone();
                for f in &rendered.missing {
                    c.fields.insert(f.clone(), format!("[{f}]"));
                }
                rendered = campaigns::render_step(&campaign, step, &c, &account.signature, None);
            }
            let email = OutgoingEmail {
                from_email: account.email.clone(),
                from_name: account.display_name.clone(),
                to_email: account.email.clone(),
                to_name: account.display_name.clone(),
                subject: format!("[Test] {}", rendered.subject),
                body: rendered.body,
                message_id: new_message_id(&account.email),
                thread: None,
                list_unsubscribe: None,
                attachments: step
                    .attachments
                    .iter()
                    .map(|a| Attachment { file_name: a.file_name.clone(), path: a.stored_path.clone().into() })
                    .collect(),
            };
            (account, email)
        };
        let provider = self.providers.for_account(&account, true).map_err(|e| crate::error::AppError::User(e.to_string()))?;
        let info = provider.send(&email).await.map_err(|e| crate::error::AppError::User(e.to_string()))?;
        accounts::log_send(&self.db.lock(), account.id, now, if info.dry_run { "test_dry_run" } else { "test" })?;
        Ok(account.email)
    }

    /// Run `f` while no tick or poll is in progress (used by restore).
    pub async fn exclusive<T>(&self, f: impl FnOnce() -> T) -> T {
        let _t = self.tick_lock.lock().await;
        let _p = self.poll_lock.lock().await;
        f()
    }

    // ---- detection --------------------------------------------------------

    /// Check every connected account for replies, bounces and unsubscribes.
    pub async fn poll_all(&self) -> AppResult<usize> {
        let _guard = self.poll_lock.lock().await;
        let accounts = accounts::list(&self.db.lock())?;
        let mut found = 0;
        for a in accounts.into_iter().filter(|a| a.status == "connected") {
            match self.poll_account(&a).await {
                Ok(n) => found += n,
                Err(e) => tracing::warn!(account = a.id, "poll failed: {e}"),
            }
        }
        if found > 0 {
            self.notifier.changed();
        }
        Ok(found)
    }

    async fn poll_account(&self, a: &Account) -> AppResult<usize> {
        let provider = match self.providers.for_account(a, true) {
            Ok(p) => p,
            Err(ProviderError::Auth(m)) => {
                accounts::set_status(&self.db.lock(), a.id, "needs_attention", Some(&m))?;
                return Ok(0);
            }
            Err(_) => return Ok(0),
        };
        let since = a
            .last_polled_at
            .as_deref()
            .or(Some(a.connected_at.as_str()))
            .and_then(parse_ts)
            .unwrap_or_else(|| self.clock.now())
            - Duration::days(1);
        let result = match provider.fetch_new(a.poll_state.as_deref(), since).await {
            Ok(r) => r,
            Err(ProviderError::Auth(m)) => {
                accounts::set_status(&self.db.lock(), a.id, "needs_attention", Some(&m))?;
                return Ok(0);
            }
            Err(e) => {
                tracing::warn!(account = a.id, "fetch failed: {e}");
                return Ok(0);
            }
        };
        let mut found = 0;
        for msg in &result.messages {
            if self.ingest(a, msg)? {
                found += 1;
            }
        }
        self.db.lock().execute(
            "UPDATE accounts SET poll_state = COALESCE(?2, poll_state), last_polled_at = ?3 WHERE id = ?1",
            params![a.id, result.state, ts(self.clock.now())],
        )?;
        Ok(found)
    }

    /// Classify one incoming message, link it to what we sent, and apply the
    /// stop rules. Returns true if it was related to one of our emails.
    pub fn ingest(&self, a: &Account, msg: &IncomingEmail) -> AppResult<bool> {
        let now = self.clock.now();
        let mut conn = self.db.lock();
        let seen: Option<i64> = conn
            .query_row(
                "SELECT id FROM inbound WHERE account_id = ?1 AND provider_id = ?2",
                params![a.id, msg.provider_id],
                |r| r.get(0),
            )
            .optional()?;
        if seen.is_some() {
            return Ok(false);
        }
        let Some(facts) = classify::parse(&msg.raw) else { return Ok(false) };
        if facts.from_email.eq_ignore_ascii_case(&a.email) {
            return Ok(false);
        }
        let kind = classify::classify(&facts);
        if kind == Kind::Ignore {
            return Ok(false);
        }
        let Some((outbound_id, enrolment_id, to_email)) = match_outbound(&conn, a.id, kind, &facts, msg)? else {
            return Ok(false);
        };
        let tx = conn.transaction()?;
        let (campaign_id, contact_id): (i64, i64) = tx.query_row(
            "SELECT campaign_id, contact_id FROM enrolments WHERE id = ?1",
            [enrolment_id],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )?;
        let body = facts.body_text.chars().take(20_000).collect::<String>();
        let snippet: String = classify::new_text(&facts.body_text).chars().take(240).collect();
        let received = msg.received_at.or(facts.received_at).unwrap_or(now);
        tx.execute(
            "INSERT INTO inbound (account_id, provider_id, message_id, from_email, from_name, subject, received_at,
                classification, outbound_id, enrolment_id, contact_id, campaign_id, snippet, body_text, web_link, created_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16)",
            params![
                a.id, msg.provider_id, facts.message_id, facts.from_email, facts.from_name, facts.subject, ts(received),
                kind.as_str(), outbound_id, enrolment_id, contact_id, campaign_id, snippet, body, msg.web_link, ts(now)
            ],
        )?;
        // Stop rules. Replies count even after the sequence finished.
        let stoppable = "('queued', 'waiting', 'sending', 'finished', 'failed')";
        let someone_else = !facts.from_email.eq_ignore_ascii_case(&to_email);
        match kind {
            Kind::Reply => {
                let reason = if someone_else {
                    format!("{} replied in the thread", facts.from_email)
                } else {
                    "Replied".to_string()
                };
                tx.execute(
                    &format!("UPDATE enrolments SET status = 'replied', stop_reason = ?2, next_due_at = NULL, updated_at = ?3 WHERE id = ?1 AND status IN {stoppable}"),
                    params![enrolment_id, reason, ts(now)],
                )?;
            }
            Kind::Unsubscribe => {
                contacts::add_dnc(&tx, &to_email, "unsubscribe_reply", now)?;
                if someone_else {
                    contacts::add_dnc(&tx, &facts.from_email, "unsubscribe_reply", now)?;
                }
                tx.execute(
                    &format!("UPDATE enrolments SET status = 'unsubscribed', stop_reason = 'Asked to stop', next_due_at = NULL, updated_at = ?2 WHERE id = ?1 AND status IN {stoppable}"),
                    params![enrolment_id, ts(now)],
                )?;
            }
            Kind::Bounce => {
                contacts::add_dnc(&tx, &to_email, "bounce", now)?;
                tx.execute(
                    &format!("UPDATE enrolments SET status = 'bounced', stop_reason = 'Email bounced', next_due_at = NULL, updated_at = ?2 WHERE id = ?1 AND status IN {stoppable}"),
                    params![enrolment_id, ts(now)],
                )?;
            }
            Kind::AutoReply | Kind::Ignore => {}
        }
        campaigns::maybe_finish(&tx, campaign_id, now)?;
        tx.commit()?;
        drop(conn);
        if matches!(kind, Kind::Reply | Kind::Unsubscribe) {
            let who = if facts.from_name.is_empty() { facts.from_email.clone() } else { facts.from_name.clone() };
            let title = if kind == Kind::Reply { format!("New reply from {who}") } else { format!("{who} asked to stop") };
            self.notifier.reply(&title, &facts.subject);
        }
        Ok(true)
    }
}

fn sample_contact() -> contacts::Contact {
    contacts::Contact {
        id: 0,
        email: "example@example.com".into(),
        first_name: "Alex".into(),
        last_name: "Example".into(),
        fields: Default::default(),
        tags: vec![],
        lists: vec![],
        do_not_contact: false,
        created_at: String::new(),
    }
}

/// Find which of our emails an incoming message belongs to.
fn match_outbound(
    conn: &Connection,
    account_id: i64,
    kind: Kind,
    f: &classify::MailFacts,
    msg: &IncomingEmail,
) -> AppResult<Option<(i64, i64, String)>> {
    let by_id = |id: &str| -> rusqlite::Result<Option<(i64, i64, String)>> {
        conn.query_row(
            "SELECT id, enrolment_id, to_email FROM outbound
             WHERE account_id = ?1 AND status = 'sent'
               AND (lower(trim(message_id, '<>')) = ?2 OR lower(trim(internet_message_id, '<>')) = ?2)",
            params![account_id, id],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
        )
        .optional()
    };
    let ids: Vec<&String> = if kind == Kind::Bounce {
        f.original_ids.iter().chain(f.thread_ids.iter()).collect()
    } else {
        f.thread_ids.iter().collect()
    };
    for id in ids {
        if let Some(m) = by_id(id)? {
            return Ok(Some(m));
        }
    }
    if let Some(conv) = &msg.conversation_id {
        let m = conn
            .query_row(
                "SELECT id, enrolment_id, to_email FROM outbound WHERE account_id = ?1 AND status = 'sent' AND conversation_id = ?2
                 ORDER BY id DESC LIMIT 1",
                params![account_id, conv],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
            )
            .optional()?;
        if m.is_some() {
            return Ok(m);
        }
    }
    if kind == Kind::Bounce {
        for r in &f.failed_recipients {
            let m = conn
                .query_row(
                    "SELECT id, enrolment_id, to_email FROM outbound WHERE account_id = ?1 AND status = 'sent' AND to_email = ?2
                     ORDER BY sent_at DESC LIMIT 1",
                    params![account_id, r],
                    |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
                )
                .optional()?;
            if m.is_some() {
                return Ok(m);
            }
        }
    }
    Ok(None)
}

fn set_enrolment(conn: &Connection, id: i64, status: &str, reason: Option<&str>, next_due: Option<String>) -> AppResult<()> {
    conn.execute(
        "UPDATE enrolments SET status = ?2, stop_reason = COALESCE(?3, stop_reason), next_due_at = COALESCE(?4, next_due_at),
            updated_at = datetime('now') WHERE id = ?1",
        params![id, status, reason, next_due],
    )?;
    Ok(())
}

fn mark_sent(conn: &Connection, outbound_id: i64, info: &SentInfo, at: DateTime<Utc>) -> AppResult<()> {
    conn.execute(
        "UPDATE outbound SET status = 'sent', sent_at = ?2, error = NULL,
            provider_message_id = COALESCE(?3, provider_message_id), thread_id = COALESCE(?4, thread_id),
            internet_message_id = COALESCE(?5, internet_message_id), conversation_id = COALESCE(?6, conversation_id),
            web_link = COALESCE(?7, web_link), dry_run = ?8
         WHERE id = ?1",
        params![
            outbound_id, ts(at), info.provider_message_id, info.thread_id, info.internet_message_id,
            info.conversation_id, info.web_link, info.dry_run
        ],
    )?;
    Ok(())
}

/// Not sent: put the email back in the queue for `when`.
fn retry_later(conn: &Connection, outbound_id: i64, enrolment_id: i64, error: &str, when: DateTime<Utc>) -> AppResult<()> {
    conn.execute(
        "UPDATE outbound SET status = 'retry', error = ?2 WHERE id = ?1",
        params![outbound_id, error],
    )?;
    conn.execute(
        "UPDATE enrolments SET status = CASE WHEN current_step = 1 THEN 'queued' ELSE 'waiting' END,
            next_due_at = ?2, updated_at = ?2 WHERE id = ?1 AND status = 'sending'",
        params![enrolment_id, ts(when)],
    )?;
    Ok(())
}

/// After step `step_number` went out: schedule the next step or finish.
fn advance(conn: &Connection, c: &Campaign, enrolment_id: i64, step_number: i64, sent_at: DateTime<Utc>, zone: Zone) -> AppResult<()> {
    let next = c.step(step_number + 1);
    let (status, due) = match next {
        Some(s) => ("waiting", Some(ts(zone.follow_up_due(sent_at, s.delay_days, &c.window)))),
        None => ("finished", None),
    };
    // Don't overwrite a reply/bounce that arrived while we were sending.
    conn.execute(
        "UPDATE enrolments SET status = ?2, current_step = ?3, next_due_at = ?4, last_sent_at = ?5, updated_at = ?5
         WHERE id = ?1 AND status IN ('sending', 'queued', 'waiting')",
        params![enrolment_id, status, step_number + 1, due, ts(sent_at)],
    )?;
    conn.execute(
        "UPDATE enrolments SET last_sent_at = ?2 WHERE id = ?1",
        params![enrolment_id, ts(sent_at)],
    )?;
    Ok(())
}

// ---- dashboard queue -------------------------------------------------------

#[derive(Debug, Serialize)]
pub struct QueueAccount {
    pub account_id: i64,
    pub email: String,
    pub due_now: i64,
    pub sent_24h: i64,
    pub daily_limit: i64,
    pub next_send_at: Option<String>,
    pub blocked_reason: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct QueueItem {
    pub campaign: String,
    pub email: String,
    pub step_number: i64,
    pub due_at: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct QueueOverview {
    pub accounts: Vec<QueueAccount>,
    pub upcoming: Vec<QueueItem>,
}

impl Engine {
    pub fn queue_overview(&self) -> AppResult<QueueOverview> {
        let now = self.clock.now();
        let conn = self.db.lock();
        let mut out = Vec::new();
        for a in accounts::list(&conn)? {
            let due_now: i64 = conn.query_row(
                "SELECT COUNT(*) FROM enrolments e JOIN campaigns c ON c.id = e.campaign_id
                 WHERE c.account_id = ?1 AND c.status = 'running' AND e.status IN ('queued','waiting') AND e.next_due_at <= ?2",
                params![a.id, ts(now)],
                |r| r.get(0),
            )?;
            let next_due: Option<(String, i64)> = conn
                .query_row(
                    "SELECT e.next_due_at, e.campaign_id FROM enrolments e JOIN campaigns c ON c.id = e.campaign_id
                     WHERE c.account_id = ?1 AND c.status = 'running' AND e.status IN ('queued','waiting')
                     ORDER BY e.next_due_at LIMIT 1",
                    [a.id],
                    |r| Ok((r.get(0)?, r.get(1)?)),
                )
                .optional()?;
            let sent_24h = accounts::sends_last_24h(&conn, a.id, now)?;
            let mut blocked = None;
            let mut next: Option<DateTime<Utc>> = None;
            if a.status != "connected" {
                blocked = Some("Needs attention".to_string());
            } else if a.paused {
                blocked = Some("Paused".to_string());
            } else if let Some((due, campaign_id)) = next_due {
                let mut t = parse_ts(&due).unwrap_or(now).max(now);
                if let Some(ns) = a.next_send_at.as_deref().and_then(parse_ts) {
                    t = t.max(ns);
                }
                if let Some(lp) = a.limit_paused_until.as_deref().and_then(parse_ts) {
                    if lp > t {
                        t = lp;
                        blocked = Some("Provider limit reached".into());
                    }
                }
                if sent_24h >= a.daily_limit {
                    t = t.max(accounts::limit_clears_at(&conn, &a, now)?);
                    blocked = Some("Daily limit reached".into());
                }
                let c = campaigns::get(&conn, campaign_id)?;
                next = self.zone.next_open(&c.window, t).or(Some(t));
            }
            out.push(QueueAccount {
                account_id: a.id,
                email: a.email.clone(),
                due_now,
                sent_24h,
                daily_limit: a.daily_limit,
                next_send_at: next.map(ts),
                blocked_reason: blocked,
            });
        }
        let mut stmt = conn.prepare(
            "SELECT c.name, ct.email, e.current_step, e.next_due_at FROM enrolments e
             JOIN campaigns c ON c.id = e.campaign_id JOIN contacts ct ON ct.id = e.contact_id
             WHERE c.status = 'running' AND e.status IN ('queued','waiting')
             ORDER BY e.next_due_at, e.id LIMIT 20",
        )?;
        let upcoming = stmt
            .query_map([], |r| Ok(QueueItem { campaign: r.get(0)?, email: r.get(1)?, step_number: r.get(2)?, due_at: r.get(3)? }))?
            .collect::<Result<_, _>>()?;
        Ok(QueueOverview { accounts: out, upcoming })
    }
}

#[cfg(test)]
mod tests;
