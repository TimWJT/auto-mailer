//! In-memory provider for tests. Records every send; can be told to fail.

use std::collections::VecDeque;
use std::sync::Mutex;

use async_trait::async_trait;
use chrono::{DateTime, Utc};

use super::*;

#[derive(Default)]
pub struct FakeProvider {
    pub sent: Mutex<Vec<OutgoingEmail>>,
    /// Errors to return from the next `send` calls, in order.
    pub fail_next: Mutex<VecDeque<ProviderError>>,
    /// Message-IDs the provider claims are in the Sent folder.
    pub already_sent: Mutex<Vec<String>>,
    /// Inbox to hand out on the next `fetch_new`.
    pub inbox: Mutex<Vec<IncomingEmail>>,
    /// Make find_sent fail (provider unreachable).
    pub find_sent_error: Mutex<Option<ProviderError>>,
    /// Next send "goes out" but reports a timeout, like a dropped connection
    /// after the server accepted the message.
    pub ghost_next: Mutex<bool>,
}

impl FakeProvider {
    pub fn sent_to(&self) -> Vec<String> {
        self.sent.lock().unwrap().iter().map(|e| e.to_email.clone()).collect()
    }
}

#[async_trait]
impl MailProvider for FakeProvider {
    async fn test(&self) -> Result<AccountInfo, ProviderError> {
        Ok(AccountInfo { email: "me@example.com".into(), display_name: "Me".into(), account_type: "gmail_free".into() })
    }

    async fn send(&self, email: &OutgoingEmail) -> Result<SentInfo, ProviderError> {
        if let Some(e) = self.fail_next.lock().unwrap().pop_front() {
            return Err(e);
        }
        self.sent.lock().unwrap().push(email.clone());
        if std::mem::take(&mut *self.ghost_next.lock().unwrap()) {
            return Err(ProviderError::Temporary("timed out".into()));
        }
        Ok(SentInfo {
            provider_message_id: Some(format!("prov-{}", email.message_id)),
            internet_message_id: Some(email.message_id.clone()),
            conversation_id: Some("conv-1".into()),
            ..Default::default()
        })
    }

    async fn find_sent(&self, message_id: &str, _to: &str, _at: DateTime<Utc>) -> Result<Option<SentInfo>, ProviderError> {
        if let Some(e) = self.find_sent_error.lock().unwrap().clone() {
            return Err(e);
        }
        let found = self.already_sent.lock().unwrap().iter().any(|m| m == message_id)
            || self.sent.lock().unwrap().iter().any(|e| e.message_id == message_id);
        Ok(found.then(|| SentInfo {
            internet_message_id: Some(message_id.to_string()),
            ..Default::default()
        }))
    }

    async fn fetch_new(&self, _state: Option<&str>, _since: DateTime<Utc>) -> Result<FetchResult, ProviderError> {
        Ok(FetchResult { messages: std::mem::take(&mut *self.inbox.lock().unwrap()), state: Some("s".into()) })
    }
}
