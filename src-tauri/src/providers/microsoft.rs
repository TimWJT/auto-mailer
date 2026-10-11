//! Microsoft accounts through Microsoft Graph (not IMAP/SMTP).
//!
//! Sending: create a draft (or `createReply` for a same-thread follow-up),
//! send it, then find the copy in Sent Items by `internetMessageId`, because
//! a draft's id can change when it moves to Sent Items.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use async_trait::async_trait;
use base64::Engine;
use chrono::{DateTime, Utc};
use reqwest::Method;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use super::*;
use crate::classify::norm_id;
use crate::clock::ts;
use crate::oauth;
use crate::secrets::SecretStore;

const GRAPH: &str = "https://graph.microsoft.com/v1.0";
const MAX_FETCH: usize = 300;
/// Our id travels in this custom header (Graph only allows `X-` headers).
const ID_HEADER: &str = "X-Auto-Mailer-Id";

/// Access tokens shared by every provider instance, keyed by account id.
pub type TokenCache = Arc<Mutex<HashMap<i64, (String, DateTime<Utc>)>>>;

pub struct MicrosoftProvider {
    pub account_id: i64,
    pub email: String,
    pub display_name: String,
    pub account_type: String,
    pub secret_key: String,
    pub secrets: Arc<dyn SecretStore>,
    pub tokens: TokenCache,
    pub http: reqwest::Client,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct GraphMessage {
    id: String,
    internet_message_id: Option<String>,
    conversation_id: Option<String>,
    web_link: Option<String>,
    received_date_time: Option<String>,
    #[serde(default)]
    to_recipients: Vec<Recipient>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Recipient {
    email_address: EmailAddr,
}

#[derive(Debug, Deserialize)]
struct EmailAddr {
    #[serde(default)]
    address: String,
}

#[derive(Debug, Deserialize)]
struct Page<T> {
    value: Vec<T>,
    #[serde(rename = "@odata.nextLink")]
    next_link: Option<String>,
}

#[derive(Debug, Serialize, Deserialize)]
struct PollState {
    last_received: String,
}

/// Map a Graph error response onto what the scheduler should do.
pub fn graph_error(status: u16, body: &str) -> ProviderError {
    let v: Value = serde_json::from_str(body).unwrap_or(Value::Null);
    let code = v["error"]["code"].as_str().unwrap_or("");
    let message = v["error"]["message"].as_str().unwrap_or("");
    let lc = code.to_ascii_lowercase();
    let lm = message.to_ascii_lowercase();
    if lc.contains("exceededmessagelimit") || lc.contains("submissionquotaexceeded")
        || lc.contains("messagesubmissionblocked") || lm.contains("outboundspam")
        || (lm.contains("exceeded") && (lm.contains("limit") || lm.contains("quota")) && !lc.contains("errorquotaexceeded"))
    {
        return ProviderError::LimitReached("Microsoft says this account has reached its sending limit.".into());
    }
    if lc == "errorquotaexceeded" {
        return ProviderError::Auth("Your mailbox is full. Free up some space in Outlook, then check the account again.".into());
    }
    if lc == "mailboxnotenabledforrestapi" || lc == "mailboxnotsupportedforrestapi" {
        return ProviderError::Auth("This Microsoft account doesn't have a mailbox Auto Mailer can use.".into());
    }
    match status {
        401 => ProviderError::Auth("Microsoft needs you to sign in again.".into()),
        403 => ProviderError::Auth(format!("Microsoft refused access: {}", first_sentence(message))),
        429 | 500..=599 => ProviderError::Temporary("Microsoft is busy right now; will try again shortly.".into()),
        _ => ProviderError::Rejected(format!("Microsoft refused this email: {}", first_sentence(message))),
    }
}

fn first_sentence(s: &str) -> String {
    s.split(". ").next().unwrap_or(s).chars().take(200).collect()
}

fn net(e: reqwest::Error) -> ProviderError {
    ProviderError::Temporary(format!("Couldn't reach Microsoft: {e}"))
}

impl MicrosoftProvider {
    async fn access_token(&self) -> Result<String, ProviderError> {
        if let Some((tok, exp)) = self.tokens.lock().unwrap().get(&self.account_id).cloned() {
            if exp > Utc::now() + chrono::Duration::seconds(60) {
                return Ok(tok);
            }
        }
        let refresh = self
            .secrets
            .get(&self.secret_key)
            .map_err(ProviderError::Temporary)?
            .ok_or_else(|| ProviderError::Auth("Sign in to Microsoft again to reconnect this account.".into()))?;
        let t = oauth::refresh(&self.http, &refresh).await?;
        // Microsoft rotates refresh tokens; keep the newest.
        if let Some(new_refresh) = &t.refresh_token {
            self.secrets.set(&self.secret_key, new_refresh).map_err(ProviderError::Temporary)?;
        }
        let exp = Utc::now() + chrono::Duration::seconds(t.expires_in.max(60));
        self.tokens.lock().unwrap().insert(self.account_id, (t.access_token.clone(), exp));
        Ok(t.access_token)
    }

    async fn call(&self, method: Method, url: &str, body: Option<Value>) -> Result<reqwest::Response, ProviderError> {
        let token = self.access_token().await?;
        let url = if url.starts_with("http") { url.to_string() } else { format!("{GRAPH}{url}") };
        let mut req = self.http.request(method, &url).bearer_auth(token).timeout(Duration::from_secs(60));
        if let Some(b) = body {
            req = req.json(&b);
        }
        let resp = req.send().await.map_err(net)?;
        if resp.status().is_success() {
            return Ok(resp);
        }
        let status = resp.status().as_u16();
        if status == 401 {
            self.tokens.lock().unwrap().remove(&self.account_id);
        }
        let text = resp.text().await.unwrap_or_default();
        Err(graph_error(status, &text))
    }

    async fn json<T: for<'de> Deserialize<'de>>(&self, method: Method, url: &str, body: Option<Value>) -> Result<T, ProviderError> {
        self.call(method, url, body)
            .await?
            .json()
            .await
            .map_err(|_| ProviderError::Temporary("Microsoft sent an unexpected reply.".into()))
    }

    async fn add_attachments(&self, draft_id: &str, atts: &[Attachment]) -> Result<(), ProviderError> {
        for a in atts {
            let bytes = tokio::fs::read(&a.path)
                .await
                .map_err(|e| ProviderError::Rejected(format!("Couldn't read the attachment {}: {e}", a.file_name)))?;
            self.call(
                Method::POST,
                &format!("/me/messages/{draft_id}/attachments"),
                Some(json!({
                    "@odata.type": "#microsoft.graph.fileAttachment",
                    "name": a.file_name,
                    "contentType": super::gmail::mime_for(&a.file_name),
                    "contentBytes": base64::engine::general_purpose::STANDARD.encode(bytes),
                })),
            )
            .await?;
        }
        Ok(())
    }

    /// After sending, find the copy in Sent Items (its id is the stable one).
    async fn locate_sent(&self, internet_message_id: &str) -> Option<GraphMessage> {
        let filter = format!("internetMessageId eq '{}'", internet_message_id.replace('\'', "''"));
        for attempt in 0..5 {
            if attempt > 0 {
                tokio::time::sleep(Duration::from_millis(1500)).await;
            }
            let url = format!(
                "/me/mailFolders/sentitems/messages?$filter={}&$select=id,internetMessageId,conversationId,webLink",
                urlencode(&filter)
            );
            if let Ok(page) = self.json::<Page<GraphMessage>>(Method::GET, &url, None).await {
                if let Some(m) = page.value.into_iter().next() {
                    return Some(m);
                }
            }
        }
        None
    }
}

/// Percent-encode a query value (spaces as %20, which OData expects).
fn urlencode(s: &str) -> String {
    let mut out = String::with_capacity(s.len() * 3);
    for b in s.bytes() {
        if b.is_ascii_alphanumeric() || b"-_.~".contains(&b) {
            out.push(b as char);
        } else {
            out.push_str(&format!("%{b:02X}"));
        }
    }
    out
}

fn recipients(e: &OutgoingEmail) -> Value {
    json!([{ "emailAddress": { "address": e.to_email, "name": header_safe(&e.to_name) } }])
}

#[async_trait]
impl MailProvider for MicrosoftProvider {
    async fn test(&self) -> Result<AccountInfo, ProviderError> {
        #[derive(Deserialize)]
        #[serde(rename_all = "camelCase")]
        struct Me {
            display_name: Option<String>,
        }
        let me: Me = self.json(Method::GET, "/me?$select=displayName", None).await?;
        // Reading the inbox proves Mail.ReadWrite works.
        self.call(Method::GET, "/me/mailFolders/inbox/messages?$top=1&$select=id", None).await?;
        Ok(AccountInfo {
            email: self.email.clone(),
            display_name: me.display_name.unwrap_or_else(|| self.display_name.clone()),
            account_type: self.account_type.clone(),
        })
    }

    async fn send(&self, e: &OutgoingEmail) -> Result<SentInfo, ProviderError> {
        let body = json!({ "contentType": "text", "content": e.body });
        let reply_target = e.thread.as_ref().and_then(|t| t.reply_to_provider_id.clone());
        let draft: GraphMessage = match reply_target {
            Some(target) => {
                let d: GraphMessage = self
                    .json(Method::POST, &format!("/me/messages/{target}/createReply"), Some(json!({})))
                    .await?;
                // Replace the quoted original and make sure it goes to the contact.
                self.json(
                    Method::PATCH,
                    &format!("/me/messages/{}", d.id),
                    Some(json!({ "subject": e.subject, "body": body, "toRecipients": recipients(e) })),
                )
                .await?
            }
            None => {
                self.json(
                    Method::POST,
                    "/me/messages",
                    Some(json!({
                        "subject": e.subject,
                        "body": body,
                        "toRecipients": recipients(e),
                        "internetMessageHeaders": [{ "name": ID_HEADER, "value": e.message_id }],
                    })),
                )
                .await?
            }
        };
        self.add_attachments(&draft.id, &e.attachments).await?;
        self.call(Method::POST, &format!("/me/messages/{}/send", draft.id), None).await?;

        let imid = draft.internet_message_id.clone();
        let sent = match &imid {
            Some(id) => self.locate_sent(id).await,
            None => None,
        };
        Ok(match sent {
            Some(m) => SentInfo {
                provider_message_id: Some(m.id),
                thread_id: None,
                internet_message_id: m.internet_message_id.or(imid),
                conversation_id: m.conversation_id.or(draft.conversation_id),
                web_link: m.web_link,
                dry_run: false,
            },
            None => {
                tracing::warn!("sent message not found in Sent Items yet; follow-ups will start a new thread");
                SentInfo {
                    provider_message_id: None,
                    thread_id: None,
                    internet_message_id: imid,
                    conversation_id: draft.conversation_id,
                    web_link: None,
                    dry_run: false,
                }
            }
        })
    }

    async fn find_sent(&self, message_id: &str, to_email: &str, attempted_at: DateTime<Utc>) -> Result<Option<SentInfo>, ProviderError> {
        let since = ts(attempted_at - chrono::Duration::minutes(2));
        let url = format!(
            "/me/mailFolders/sentitems/messages?$filter={}&$select=id,internetMessageId,conversationId,webLink,toRecipients&$top=50",
            urlencode(&format!("sentDateTime ge {since}"))
        );
        let page: Page<GraphMessage> = self.json(Method::GET, &url, None).await?;
        let want = norm_id(message_id);
        for m in page.value {
            let to_match = m.to_recipients.iter().any(|r| r.email_address.address.eq_ignore_ascii_case(to_email));
            let id_match = m.internet_message_id.as_deref().map(norm_id).as_deref() == Some(want.as_str());
            if !id_match && !to_match {
                continue;
            }
            let mut confirmed = id_match;
            if !confirmed {
                #[derive(Deserialize)]
                #[serde(rename_all = "camelCase")]
                struct Headers {
                    #[serde(default)]
                    internet_message_headers: Vec<Header>,
                }
                #[derive(Deserialize)]
                struct Header {
                    name: String,
                    value: String,
                }
                let h: Headers = self
                    .json(Method::GET, &format!("/me/messages/{}?$select=internetMessageHeaders", m.id), None)
                    .await?;
                let ours = h.internet_message_headers.iter().find(|h| h.name.eq_ignore_ascii_case(ID_HEADER));
                confirmed = match ours {
                    Some(h) => norm_id(&h.value) == want,
                    // Follow-ups (createReply) can't carry our header: a send to
                    // the same person in this time window is taken as ours.
                    None => true,
                };
            }
            if confirmed {
                return Ok(Some(SentInfo {
                    provider_message_id: Some(m.id),
                    thread_id: None,
                    internet_message_id: m.internet_message_id,
                    conversation_id: m.conversation_id,
                    web_link: m.web_link,
                    dry_run: false,
                }));
            }
        }
        Ok(None)
    }

    async fn fetch_new(&self, state: Option<&str>, since: DateTime<Utc>) -> Result<FetchResult, ProviderError> {
        let prev: Option<PollState> = state.and_then(|s| serde_json::from_str(s).ok());
        let cursor = prev.map(|p| p.last_received).unwrap_or_else(|| ts(since));
        let mut url = format!(
            "/me/mailFolders/inbox/messages?$filter={}&$orderby=receivedDateTime%20asc&$select=id,receivedDateTime,conversationId,webLink,internetMessageId&$top=50",
            urlencode(&format!("receivedDateTime gt {cursor}"))
        );
        let mut list = Vec::new();
        loop {
            let page: Page<GraphMessage> = self.json(Method::GET, &url, None).await?;
            list.extend(page.value);
            match page.next_link {
                Some(next) if list.len() < MAX_FETCH => url = next,
                _ => break,
            }
        }
        let mut last = cursor.clone();
        let mut messages = Vec::new();
        for m in list {
            let raw = self
                .call(Method::GET, &format!("/me/messages/{}/$value", m.id), None)
                .await?
                .bytes()
                .await
                .map_err(net)?;
            let received = m.received_date_time.as_deref().and_then(|s| DateTime::parse_from_rfc3339(s).ok()).map(|d| d.with_timezone(&Utc));
            if let Some(r) = received {
                let r = ts(r);
                if r > last {
                    last = r;
                }
            }
            messages.push(IncomingEmail {
                provider_id: m.id,
                raw: raw.to_vec(),
                conversation_id: m.conversation_id,
                thread_id: None,
                web_link: m.web_link,
                received_at: received,
            });
        }
        Ok(FetchResult {
            messages,
            state: Some(serde_json::to_string(&PollState { last_received: last }).unwrap_or_default()),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn graph_errors_map_to_actions() {
        let limit = r#"{"error":{"code":"ErrorExceededMessageLimit","message":"Cannot send mail. Exceeded limit."}}"#;
        assert!(matches!(graph_error(403, limit), ProviderError::LimitReached(_)));
        let blocked = r#"{"error":{"code":"ErrorMessageSubmissionBlocked","message":"Your account has been blocked"}}"#;
        assert!(matches!(graph_error(403, blocked), ProviderError::LimitReached(_)));
        assert!(matches!(graph_error(429, "{}"), ProviderError::Temporary(_)));
        assert!(matches!(graph_error(503, ""), ProviderError::Temporary(_)));
        assert!(matches!(graph_error(401, "{}"), ProviderError::Auth(_)));
        let full = r#"{"error":{"code":"ErrorQuotaExceeded","message":"The process failed to get the correct properties."}}"#;
        assert!(matches!(graph_error(507, full), ProviderError::Auth(_)));
        let bad = r#"{"error":{"code":"ErrorInvalidRecipients","message":"At least one recipient is not valid."}}"#;
        assert!(matches!(graph_error(400, bad), ProviderError::Rejected(_)));
    }
}
