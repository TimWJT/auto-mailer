//! Mail providers behind one trait, so the scheduler and poller never know
//! whether they're talking to Gmail, Microsoft Graph, the dry-run folder, or
//! a test fake. Automated tests only ever use the fake.

pub mod dryrun;
#[cfg(test)]
pub mod fake;
pub mod gmail;
pub mod microsoft;

use std::path::PathBuf;

use async_trait::async_trait;
use chrono::{DateTime, Utc};

#[derive(Debug, Clone)]
pub struct Attachment {
    pub file_name: String,
    pub path: PathBuf,
}

#[derive(Debug, Clone)]
pub struct OutgoingEmail {
    pub from_email: String,
    pub from_name: String,
    pub to_email: String,
    pub to_name: String,
    pub subject: String,
    pub body: String,
    /// Our own Message-ID, with angle brackets. Generated before sending so a
    /// crash mid-send can be reconciled against the Sent folder.
    pub message_id: String,
    pub thread: Option<ThreadRef>,
    /// `mailto:` target for the List-Unsubscribe header (where allowed).
    pub list_unsubscribe: Option<String>,
    pub attachments: Vec<Attachment>,
}

/// What a follow-up in the same thread needs to know about earlier emails.
#[derive(Debug, Clone)]
pub struct ThreadRef {
    /// Message-IDs of earlier emails in this thread, oldest first.
    pub references: Vec<String>,
    /// Provider id of the most recent sent email (Graph `createReply` target).
    pub reply_to_provider_id: Option<String>,
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct SentInfo {
    pub provider_message_id: Option<String>,
    pub thread_id: Option<String>,
    pub internet_message_id: Option<String>,
    pub conversation_id: Option<String>,
    pub web_link: Option<String>,
    /// Written to the dry-run folder, not really sent.
    pub dry_run: bool,
}

#[derive(Debug, Clone, PartialEq, thiserror::Error)]
pub enum ProviderError {
    /// Credentials rejected or expired: the user must reconnect.
    #[error("{0}")]
    Auth(String),
    /// The provider's own sending limit was hit.
    #[error("{0}")]
    LimitReached(String),
    /// Permanent failure for this message/recipient; don't retry it.
    #[error("{0}")]
    Rejected(String),
    /// Network trouble or throttling; try again later.
    #[error("{0}")]
    Temporary(String),
}

#[derive(Debug, Clone)]
pub struct IncomingEmail {
    pub provider_id: String,
    pub raw: Vec<u8>,
    pub conversation_id: Option<String>,
    pub thread_id: Option<String>,
    pub web_link: Option<String>,
    pub received_at: Option<DateTime<Utc>>,
}

#[derive(Debug, Clone, Default)]
pub struct FetchResult {
    pub messages: Vec<IncomingEmail>,
    /// Opaque cursor to pass to the next fetch.
    pub state: Option<String>,
}

/// Details learnt when connecting or testing an account.
#[derive(Debug, Clone)]
pub struct AccountInfo {
    pub email: String,
    pub display_name: String,
    pub account_type: String,
}

#[async_trait]
pub trait MailProvider: Send + Sync {
    async fn test(&self) -> Result<AccountInfo, ProviderError>;
    async fn send(&self, email: &OutgoingEmail) -> Result<SentInfo, ProviderError>;
    /// Look for an email we may have sent already (after a crash mid-send).
    /// `Ok(None)` means "definitely not in the Sent folder".
    async fn find_sent(
        &self,
        message_id: &str,
        to_email: &str,
        attempted_at: DateTime<Utc>,
    ) -> Result<Option<SentInfo>, ProviderError>;
    /// New messages in the inbox since the last call.
    async fn fetch_new(
        &self,
        state: Option<&str>,
        since: DateTime<Utc>,
    ) -> Result<FetchResult, ProviderError>;
}

/// Generate a Message-ID like `<am-3f2a…@example.com>` using the sender's domain.
pub fn new_message_id(from_email: &str) -> String {
    let domain = from_email.rsplit('@').next().filter(|d| !d.is_empty()).unwrap_or("localhost");
    format!("<am-{}@{}>", uuid::Uuid::new_v4().simple(), domain)
}

/// Strip control characters that could break a header.
pub(crate) fn header_safe(s: &str) -> String {
    s.chars().filter(|c| !c.is_control()).collect::<String>().trim().to_string()
}
