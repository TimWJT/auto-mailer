//! Dry-run mode: "sending" writes the full rendered email to a local folder
//! instead. On by default in development builds.

use std::path::PathBuf;

use async_trait::async_trait;
use chrono::{DateTime, Utc};

use super::*;

pub struct DryRunProvider {
    pub dir: PathBuf,
    pub info: AccountInfo,
}

impl DryRunProvider {
    fn file_for(&self, message_id: &str) -> PathBuf {
        let safe: String = message_id
            .chars()
            .map(|c| if c.is_ascii_alphanumeric() || c == '-' || c == '.' { c } else { '_' })
            .collect();
        self.dir.join(format!("{safe}.eml"))
    }
}

/// Render the email as a readable `.eml` (headers + plain-text body).
pub fn render_eml(e: &OutgoingEmail, now: DateTime<Utc>) -> String {
    let mut s = String::new();
    let from = if e.from_name.is_empty() {
        e.from_email.clone()
    } else {
        format!("{} <{}>", header_safe(&e.from_name), e.from_email)
    };
    let to = if e.to_name.is_empty() {
        e.to_email.clone()
    } else {
        format!("{} <{}>", header_safe(&e.to_name), e.to_email)
    };
    s.push_str("X-Auto-Mailer: dry run - this email was NOT sent\r\n");
    s.push_str(&format!("Date: {}\r\n", now.to_rfc2822()));
    s.push_str(&format!("From: {from}\r\nTo: {to}\r\n"));
    s.push_str(&format!("Subject: {}\r\n", header_safe(&e.subject)));
    s.push_str(&format!("Message-ID: {}\r\n", e.message_id));
    if let Some(t) = &e.thread {
        if let Some(last) = t.references.last() {
            s.push_str(&format!("In-Reply-To: {last}\r\n"));
            s.push_str(&format!("References: {}\r\n", t.references.join(" ")));
        }
    }
    if let Some(u) = &e.list_unsubscribe {
        s.push_str(&format!("List-Unsubscribe: <{u}>\r\n"));
    }
    for a in &e.attachments {
        s.push_str(&format!("X-Attachment: {}\r\n", header_safe(&a.file_name)));
    }
    s.push_str("Content-Type: text/plain; charset=utf-8\r\n\r\n");
    s.push_str(&e.body.replace("\r\n", "\n").replace('\n', "\r\n"));
    s
}

#[async_trait]
impl MailProvider for DryRunProvider {
    async fn test(&self) -> Result<AccountInfo, ProviderError> {
        Ok(self.info.clone())
    }

    async fn send(&self, email: &OutgoingEmail) -> Result<SentInfo, ProviderError> {
        tokio::fs::create_dir_all(&self.dir)
            .await
            .map_err(|e| ProviderError::Temporary(format!("Couldn't create the dry-run folder: {e}")))?;
        let path = self.file_for(&email.message_id);
        tokio::fs::write(&path, render_eml(email, Utc::now()))
            .await
            .map_err(|e| ProviderError::Temporary(format!("Couldn't write the dry-run email: {e}")))?;
        Ok(SentInfo {
            provider_message_id: Some(path.to_string_lossy().into_owned()),
            internet_message_id: Some(email.message_id.clone()),
            dry_run: true,
            ..Default::default()
        })
    }

    async fn find_sent(&self, message_id: &str, _to: &str, _at: DateTime<Utc>) -> Result<Option<SentInfo>, ProviderError> {
        let path = self.file_for(message_id);
        Ok(path.exists().then(|| SentInfo {
            provider_message_id: Some(path.to_string_lossy().into_owned()),
            internet_message_id: Some(message_id.to_string()),
            dry_run: true,
            ..Default::default()
        }))
    }

    async fn fetch_new(&self, _state: Option<&str>, _since: DateTime<Utc>) -> Result<FetchResult, ProviderError> {
        Ok(FetchResult::default())
    }
}
