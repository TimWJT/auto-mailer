//! Unified inbox, contact timeline and CSV exports.

use std::path::Path;

use rusqlite::{params, Connection, OptionalExtension};
use serde::{Deserialize, Serialize};

use crate::error::{AppError, AppResult};

#[derive(Debug, Serialize)]
pub struct InboundRow {
    pub id: i64,
    pub account_email: Option<String>,
    pub from_email: String,
    pub from_name: String,
    pub subject: String,
    pub received_at: String,
    pub classification: String,
    pub contact_id: Option<i64>,
    pub campaign_id: Option<i64>,
    pub campaign_name: Option<String>,
    pub snippet: String,
    pub is_read: bool,
}

#[derive(Debug, Default, Deserialize)]
pub struct InboxFilter {
    pub classification: Option<String>,
    pub campaign_id: Option<i64>,
    #[serde(default)]
    pub unread_only: bool,
}

pub fn list(conn: &Connection, f: &InboxFilter) -> AppResult<Vec<InboundRow>> {
    let mut stmt = conn.prepare(
        "SELECT i.id, a.email, i.from_email, i.from_name, i.subject, i.received_at, i.classification,
                i.contact_id, i.campaign_id, c.name, i.snippet, i.is_read
         FROM inbound i LEFT JOIN accounts a ON a.id = i.account_id LEFT JOIN campaigns c ON c.id = i.campaign_id
         WHERE (?1 IS NULL OR i.classification = ?1) AND (?2 IS NULL OR i.campaign_id = ?2) AND (?3 = 0 OR i.is_read = 0)
         ORDER BY i.received_at DESC LIMIT 1000",
    )?;
    let r = stmt
        .query_map(params![f.classification, f.campaign_id, f.unread_only], |r| {
            Ok(InboundRow {
                id: r.get(0)?,
                account_email: r.get(1)?,
                from_email: r.get(2)?,
                from_name: r.get(3)?,
                subject: r.get(4)?,
                received_at: r.get(5)?,
                classification: r.get(6)?,
                contact_id: r.get(7)?,
                campaign_id: r.get(8)?,
                campaign_name: r.get(9)?,
                snippet: r.get(10)?,
                is_read: r.get(11)?,
            })
        })?
        .collect::<Result<_, _>>()?;
    Ok(r)
}

#[derive(Debug, Serialize)]
pub struct InboundDetail {
    pub row: InboundRow,
    pub body_text: String,
    pub web_link: Option<String>,
}

pub fn get(conn: &Connection, id: i64) -> AppResult<InboundDetail> {
    conn.execute("UPDATE inbound SET is_read = 1 WHERE id = ?1", [id])?;
    let (body_text, web_link, message_id, account_email, provider): (String, Option<String>, Option<String>, Option<String>, Option<String>) = conn
        .query_row(
            "SELECT i.body_text, i.web_link, i.message_id, a.email, a.provider FROM inbound i
             LEFT JOIN accounts a ON a.id = i.account_id WHERE i.id = ?1",
            [id],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?)),
        )
        .optional()?
        .ok_or_else(|| AppError::NotFound("message".into()))?;
    let row = list(conn, &InboxFilter::default())?
        .into_iter()
        .find(|r| r.id == id)
        .ok_or_else(|| AppError::NotFound("message".into()))?;
    // Gmail can always find a message by its Message-ID.
    let web_link = web_link.or_else(|| match (provider.as_deref(), message_id, account_email) {
        (Some("gmail"), Some(mid), Some(acct)) => Some(format!(
            "https://mail.google.com/mail/?authuser={acct}#search/rfc822msgid%3A{}",
            mid.replace('@', "%40")
        )),
        (Some("microsoft"), _, _) => Some("https://outlook.live.com/mail/0/inbox".into()),
        _ => None,
    });
    Ok(InboundDetail { row, body_text, web_link })
}

pub fn unread_count(conn: &Connection) -> AppResult<i64> {
    Ok(conn.query_row(
        "SELECT COUNT(*) FROM inbound WHERE is_read = 0 AND classification IN ('reply', 'unsubscribe')",
        [],
        |r| r.get(0),
    )?)
}

#[derive(Debug, Serialize)]
pub struct TimelineItem {
    /// "sent" or "received"
    pub direction: String,
    pub at: String,
    pub subject: String,
    pub campaign_name: Option<String>,
    pub step_number: Option<i64>,
    /// outbound status, or inbound classification
    pub status: String,
    pub detail: String,
}

pub fn timeline(conn: &Connection, contact_id: i64) -> AppResult<Vec<TimelineItem>> {
    let mut items = Vec::new();
    let mut stmt = conn.prepare(
        "SELECT COALESCE(o.sent_at, o.attempted_at), o.subject, c.name, o.step_number, o.status,
                CASE WHEN o.dry_run = 1 THEN 'Dry run: written to the folder, not really sent' ELSE COALESCE(o.error, '') END
         FROM outbound o JOIN enrolments e ON e.id = o.enrolment_id JOIN campaigns c ON c.id = e.campaign_id
         WHERE e.contact_id = ?1",
    )?;
    for r in stmt.query_map([contact_id], |r| {
        Ok(TimelineItem {
            direction: "sent".into(),
            at: r.get(0)?,
            subject: r.get(1)?,
            campaign_name: r.get(2)?,
            step_number: r.get(3)?,
            status: r.get(4)?,
            detail: r.get(5)?,
        })
    })? {
        items.push(r?);
    }
    let mut stmt = conn.prepare(
        "SELECT i.received_at, i.subject, c.name, i.classification, i.snippet
         FROM inbound i LEFT JOIN campaigns c ON c.id = i.campaign_id WHERE i.contact_id = ?1",
    )?;
    for r in stmt.query_map([contact_id], |r| {
        Ok(TimelineItem {
            direction: "received".into(),
            at: r.get(0)?,
            subject: r.get(1)?,
            campaign_name: r.get(2)?,
            step_number: None,
            status: r.get(3)?,
            detail: r.get(4)?,
        })
    })? {
        items.push(r?);
    }
    items.sort_by(|a, b| b.at.cmp(&a.at));
    Ok(items)
}

fn write_csv(path: &Path, headers: &[&str], rows: Vec<Vec<String>>) -> AppResult<usize> {
    let mut w = csv::Writer::from_path(path).map_err(|e| AppError::user(format!("Couldn't write the file: {e}")))?;
    w.write_record(headers).map_err(|e| AppError::user(e.to_string()))?;
    let n = rows.len();
    for r in rows {
        w.write_record(&r).map_err(|e| AppError::user(e.to_string()))?;
    }
    w.flush()?;
    Ok(n)
}

pub fn export_campaign(conn: &Connection, campaign_id: i64, path: &Path) -> AppResult<usize> {
    let mut stmt = conn.prepare(
        "SELECT c.email, c.first_name, c.last_name, e.status, e.current_step,
                (SELECT COUNT(*) FROM outbound o WHERE o.enrolment_id = e.id AND o.status = 'sent'),
                COALESCE(e.last_sent_at, ''),
                COALESCE((SELECT MAX(received_at) FROM inbound i WHERE i.enrolment_id = e.id AND i.classification IN ('reply','unsubscribe')), ''),
                COALESCE(e.stop_reason, '')
         FROM enrolments e JOIN contacts c ON c.id = e.contact_id WHERE e.campaign_id = ?1 ORDER BY e.id",
    )?;
    let rows = stmt
        .query_map([campaign_id], |r| {
            Ok(vec![
                r.get::<_, String>(0)?,
                r.get(1)?,
                r.get(2)?,
                r.get(3)?,
                r.get::<_, i64>(4)?.to_string(),
                r.get::<_, i64>(5)?.to_string(),
                r.get(6)?,
                r.get(7)?,
                r.get(8)?,
            ])
        })?
        .collect::<Result<_, _>>()?;
    write_csv(
        path,
        &["email", "first_name", "last_name", "status", "next_step", "emails_sent", "last_sent_at", "replied_at", "note"],
        rows,
    )
}

pub fn export_contacts(conn: &Connection, path: &Path) -> AppResult<usize> {
    let names = crate::contacts::field_names(conn)?;
    let custom: Vec<&String> = names.iter().filter(|n| !matches!(n.as_str(), "email" | "first_name" | "last_name")).collect();
    let mut stmt = conn.prepare(
        "SELECT c.email, c.first_name, c.last_name, c.fields,
                (SELECT group_concat(t.name, '; ') FROM contact_tags ct JOIN tags t ON t.id = ct.tag_id WHERE ct.contact_id = c.id),
                (SELECT group_concat(l.name, '; ') FROM list_members m JOIN lists l ON l.id = m.list_id WHERE m.contact_id = c.id),
                EXISTS (SELECT 1 FROM do_not_contact d WHERE d.email = c.email)
         FROM contacts c ORDER BY c.email",
    )?;
    let rows = stmt
        .query_map([], |r| {
            let fields: std::collections::BTreeMap<String, String> =
                serde_json::from_str(&r.get::<_, String>(3)?).unwrap_or_default();
            let mut row = vec![r.get::<_, String>(0)?, r.get(1)?, r.get(2)?];
            for k in &custom {
                row.push(fields.get(*k).cloned().unwrap_or_default());
            }
            row.push(r.get::<_, Option<String>>(4)?.unwrap_or_default());
            row.push(r.get::<_, Option<String>>(5)?.unwrap_or_default());
            row.push(if r.get::<_, bool>(6)? { "yes".into() } else { String::new() });
            Ok(row)
        })?
        .collect::<Result<_, _>>()?;
    let mut headers: Vec<&str> = vec!["email", "first_name", "last_name"];
    headers.extend(custom.iter().map(|s| s.as_str()));
    headers.extend(["tags", "lists", "do_not_contact"]);
    write_csv(path, &headers, rows)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::Db;

    #[test]
    fn exports_contacts_with_custom_fields() {
        let db = Db::open_in_memory().unwrap();
        let conn = db.lock();
        conn.execute(
            "INSERT INTO contacts (email, first_name, last_name, fields, created_at, updated_at)
             VALUES ('a@x.com', 'Ana', 'L', '{\"company\":\"Acme, Inc.\"}', 't', 't')",
            [],
        )
        .unwrap();
        crate::contacts::add_dnc(&conn, "a@x.com", "manual", chrono::Utc::now()).unwrap();
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("c.csv");
        assert_eq!(export_contacts(&conn, &p).unwrap(), 1);
        let text = std::fs::read_to_string(&p).unwrap();
        assert_eq!(text.lines().next().unwrap(), "email,first_name,last_name,company,tags,lists,do_not_contact");
        assert!(text.contains("a@x.com,Ana,L,\"Acme, Inc.\",,,yes"));
    }
}
