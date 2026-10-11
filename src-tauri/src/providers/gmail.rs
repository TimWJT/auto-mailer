//! Gmail with an app password: SMTP (smtp.gmail.com:465) to send, IMAP
//! (imap.gmail.com:993) to read replies and bounces. Gmail saves SMTP-sent
//! mail to Sent by itself, so we never append a copy.

use std::sync::{Arc, OnceLock};
use std::time::Duration;

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use futures::TryStreamExt;
use serde::{Deserialize, Serialize};
use tokio::net::TcpStream;
use tokio_rustls::rustls;
use tokio_rustls::TlsConnector;

use super::*;
use crate::accounts::gmail_type_for;

const IMAP_HOST: &str = "imap.gmail.com";
const SMTP_HOST: &str = "smtp.gmail.com";
const TIMEOUT: Duration = Duration::from_secs(45);
/// Upper bound on messages read in one poll (first poll of a busy inbox).
const MAX_FETCH: usize = 300;

pub struct GmailProvider {
    pub email: String,
    pub display_name: String,
    pub app_password: String,
}

type ImapSession = async_imap::Session<tokio_rustls::client::TlsStream<TcpStream>>;

fn tls() -> TlsConnector {
    static CONFIG: OnceLock<Arc<rustls::ClientConfig>> = OnceLock::new();
    let config = CONFIG.get_or_init(|| {
        let mut roots = rustls::RootCertStore::empty();
        for cert in rustls_native_certs::load_native_certs().certs {
            let _ = roots.add(cert);
        }
        Arc::new(
            rustls::ClientConfig::builder()
                .with_root_certificates(roots)
                .with_no_client_auth(),
        )
    });
    TlsConnector::from(config.clone())
}

/// Turn Gmail's IMAP/SMTP wording into something a person can act on.
pub fn explain_login_error(raw: &str) -> ProviderError {
    let l = raw.to_ascii_lowercase();
    let msg = if l.contains("application-specific password required") || l.contains("534") {
        "Gmail wants an app password, not your normal password. Turn on 2-Step Verification, create an app password, and paste that here."
    } else if l.contains("not enabled for imap") || l.contains("imap access is disabled") || l.contains("imap is disabled") {
        "IMAP is turned off for this Gmail account. In Gmail, open Settings → See all settings → Forwarding and POP/IMAP and turn IMAP on, then try again."
    } else if l.contains("webalert") || l.contains("web login required") || l.contains("please log in via your web browser") {
        "Google blocked the sign-in. Sign in to Gmail in your browser once, approve any security prompt, then try again."
    } else if l.contains("advanced protection") || l.contains("admin") || l.contains("not allowed") || l.contains("disabled by your") {
        "This Google account doesn't allow app passwords (some work/school accounts and Advanced Protection block them). Ask your administrator, or use a different account."
    } else if l.contains("invalid credentials") || l.contains("authenticationfailed") || l.contains("username and password not accepted") || l.contains("535") {
        "Gmail didn't accept that email and app password. Check you copied the 16-letter app password exactly, and that 2-Step Verification is on."
    } else {
        return ProviderError::Temporary(format!("Couldn't reach Gmail: {}", short(raw)));
    };
    ProviderError::Auth(msg.into())
}

fn short(s: &str) -> String {
    let s = s.trim();
    if s.chars().count() > 200 {
        format!("{}…", s.chars().take(200).collect::<String>())
    } else {
        s.to_string()
    }
}

fn smtp_error(e: lettre::transport::smtp::Error) -> ProviderError {
    let raw = e.to_string();
    let l = raw.to_ascii_lowercase();
    if l.contains("5.4.5") || l.contains("sending limit") || l.contains("daily user sending") || l.contains("quota") {
        return ProviderError::LimitReached(
            "Gmail says this account has reached its daily sending limit.".into(),
        );
    }
    if l.contains("535") || l.contains("534") || l.contains("username and password") || l.contains("application-specific") {
        return explain_login_error(&raw);
    }
    if e.is_permanent() {
        return ProviderError::Rejected(format!("Gmail refused this email: {}", short(&raw)));
    }
    ProviderError::Temporary(format!("Couldn't send through Gmail: {}", short(&raw)))
}

fn imap_err(e: async_imap::error::Error) -> ProviderError {
    match e {
        async_imap::error::Error::No(m) | async_imap::error::Error::Bad(m) => explain_login_error(&m),
        other => ProviderError::Temporary(format!("Couldn't read Gmail: {}", short(&other.to_string()))),
    }
}

#[derive(Debug, Serialize, Deserialize)]
struct PollState {
    uid_validity: u32,
    last_uid: u32,
}

impl GmailProvider {
    async fn imap(&self) -> Result<ImapSession, ProviderError> {
        let connect = async {
            let tcp = TcpStream::connect((IMAP_HOST, 993))
                .await
                .map_err(|e| ProviderError::Temporary(format!("Couldn't connect to Gmail: {e}")))?;
            let name = rustls::pki_types::ServerName::try_from(IMAP_HOST).expect("valid host");
            let stream = tls()
                .connect(name, tcp)
                .await
                .map_err(|e| ProviderError::Temporary(format!("Secure connection to Gmail failed: {e}")))?;
            let mut client = async_imap::Client::new(stream);
            client
                .read_response()
                .await
                .map_err(|e| ProviderError::Temporary(format!("Gmail didn't answer: {e}")))?;
            client
                .login(&self.email, &self.app_password)
                .await
                .map_err(|(e, _)| imap_err(e))
        };
        tokio::time::timeout(TIMEOUT, connect)
            .await
            .map_err(|_| ProviderError::Temporary("Gmail took too long to answer.".into()))?
    }

    fn smtp(&self) -> Result<lettre::AsyncSmtpTransport<lettre::Tokio1Executor>, ProviderError> {
        use lettre::transport::smtp::authentication::Credentials;
        Ok(lettre::AsyncSmtpTransport::<lettre::Tokio1Executor>::relay(SMTP_HOST)
            .map_err(smtp_error)?
            .credentials(Credentials::new(self.email.clone(), self.app_password.clone()))
            .timeout(Some(TIMEOUT))
            .build())
    }

    fn web_link(&self, gm_msgid: Option<u64>) -> Option<String> {
        gm_msgid.map(|id| format!("https://mail.google.com/mail/?authuser={}#all/{:x}", self.email, id))
    }
}

pub fn build_message(e: &OutgoingEmail) -> Result<lettre::Message, ProviderError> {
    use lettre::message::header::{ContentType, HeaderName, HeaderValue};
    use lettre::message::{Attachment as MailAttachment, Mailbox, MultiPart, SinglePart};

    let bad = |what: &str| ProviderError::Rejected(format!("{what} isn't a valid email address."));
    let from = Mailbox::new(
        Some(header_safe(&e.from_name)).filter(|n| !n.is_empty()),
        e.from_email.parse().map_err(|_| bad(&e.from_email))?,
    );
    let to = Mailbox::new(
        Some(header_safe(&e.to_name)).filter(|n| !n.is_empty()),
        e.to_email.parse().map_err(|_| bad(&e.to_email))?,
    );
    let mut b = lettre::Message::builder()
        .from(from)
        .to(to)
        .subject(header_safe(&e.subject))
        .message_id(Some(e.message_id.clone()))
        .date_now();
    if let Some(t) = &e.thread {
        if let Some(last) = t.references.last() {
            b = b.in_reply_to(last.clone()).references(t.references.join(" "));
        }
    }
    if let Some(u) = &e.list_unsubscribe {
        b = b.raw_header(HeaderValue::new(
            HeaderName::new_from_ascii_str("List-Unsubscribe"),
            format!("<{u}>"),
        ));
    }
    let msg = if e.attachments.is_empty() {
        b.header(ContentType::TEXT_PLAIN).body(e.body.clone())
    } else {
        let mut mp = MultiPart::mixed().singlepart(SinglePart::plain(e.body.clone()));
        for a in &e.attachments {
            let bytes = std::fs::read(&a.path).map_err(|err| {
                ProviderError::Rejected(format!("Couldn't read the attachment {}: {err}", a.file_name))
            })?;
            let ct = ContentType::parse(mime_for(&a.file_name)).unwrap_or(ContentType::TEXT_PLAIN);
            mp = mp.singlepart(MailAttachment::new(a.file_name.clone()).body(bytes, ct));
        }
        b.multipart(mp)
    };
    msg.map_err(|err| ProviderError::Rejected(format!("Couldn't build the email: {err}")))
}

pub fn mime_for(file_name: &str) -> &'static str {
    let ext = file_name.rsplit('.').next().unwrap_or("").to_ascii_lowercase();
    match ext.as_str() {
        "pdf" => "application/pdf",
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "gif" => "image/gif",
        "txt" => "text/plain",
        "csv" => "text/csv",
        "doc" => "application/msword",
        "docx" => "application/vnd.openxmlformats-officedocument.wordprocessingml.document",
        "xls" => "application/vnd.ms-excel",
        "xlsx" => "application/vnd.openxmlformats-officedocument.spreadsheetml.sheet",
        "ppt" => "application/vnd.ms-powerpoint",
        "pptx" => "application/vnd.openxmlformats-officedocument.presentationml.presentation",
        "zip" => "application/zip",
        _ => "application/octet-stream",
    }
}

#[async_trait]
impl MailProvider for GmailProvider {
    async fn test(&self) -> Result<AccountInfo, ProviderError> {
        let mut s = self.imap().await?;
        s.select("INBOX").await.map_err(imap_err)?;
        let _ = s.logout().await;
        // Prove sending works too (SMTP auth), without sending anything.
        self.smtp()?.test_connection().await.map_err(smtp_error)?;
        Ok(AccountInfo {
            email: self.email.clone(),
            display_name: self.display_name.clone(),
            account_type: gmail_type_for(&self.email).into(),
        })
    }

    async fn send(&self, email: &OutgoingEmail) -> Result<SentInfo, ProviderError> {
        use lettre::AsyncTransport;
        let msg = build_message(email)?;
        self.smtp()?.send(msg).await.map_err(smtp_error)?;
        Ok(SentInfo {
            internet_message_id: Some(email.message_id.clone()),
            ..Default::default()
        })
    }

    async fn find_sent(&self, message_id: &str, _to: &str, _at: DateTime<Utc>) -> Result<Option<SentInfo>, ProviderError> {
        let mut s = self.imap().await?;
        let sent_box = {
            let names: Vec<_> = s
                .list(Some(""), Some("*"))
                .await
                .map_err(imap_err)?
                .try_collect()
                .await
                .map_err(imap_err)?;
            names
                .iter()
                .find(|n| n.attributes().iter().any(|a| matches!(a, async_imap::types::NameAttribute::Sent)))
                .map(|n| n.name().to_string())
                .unwrap_or_else(|| "[Gmail]/Sent Mail".to_string())
        };
        s.select(&sent_box).await.map_err(imap_err)?;
        let id = message_id.replace('"', "");
        let found = s
            .uid_search(format!("HEADER Message-ID \"{id}\""))
            .await
            .map_err(imap_err)?;
        let _ = s.logout().await;
        Ok((!found.is_empty()).then(|| SentInfo {
            internet_message_id: Some(message_id.to_string()),
            ..Default::default()
        }))
    }

    async fn fetch_new(&self, state: Option<&str>, since: DateTime<Utc>) -> Result<FetchResult, ProviderError> {
        let mut s = self.imap().await?;
        let mailbox = s.select("INBOX").await.map_err(imap_err)?;
        let validity = mailbox.uid_validity.unwrap_or(0);
        let prev: Option<PollState> = state.and_then(|st| serde_json::from_str(st).ok());

        let mut uids: Vec<u32> = match &prev {
            Some(p) if p.uid_validity == validity => s
                .uid_search(format!("UID {}:*", p.last_uid + 1))
                .await
                .map_err(imap_err)?
                .into_iter()
                .filter(|u| *u > p.last_uid)
                .collect(),
            _ => s
                .uid_search(format!("SINCE {}", since.format("%d-%b-%Y")))
                .await
                .map_err(imap_err)?
                .into_iter()
                .collect(),
        };
        uids.sort_unstable();
        if uids.len() > MAX_FETCH {
            uids = uids.split_off(uids.len() - MAX_FETCH);
        }
        let last_uid = uids
            .last()
            .copied()
            .or(prev.as_ref().filter(|p| p.uid_validity == validity).map(|p| p.last_uid))
            .unwrap_or(0);

        let mut messages = Vec::new();
        for chunk in uids.chunks(25) {
            let set = chunk.iter().map(|u| u.to_string()).collect::<Vec<_>>().join(",");
            let fetched: Vec<_> = s
                .uid_fetch(&set, "(UID INTERNALDATE X-GM-MSGID BODY.PEEK[])")
                .await
                .map_err(imap_err)?
                .try_collect()
                .await
                .map_err(imap_err)?;
            for f in fetched {
                let Some(body) = f.body() else { continue };
                let gm = f.gmail_msg_id().copied();
                messages.push(IncomingEmail {
                    provider_id: gm.map(|g| g.to_string()).or(f.uid.map(|u| format!("uid-{validity}-{u}"))).unwrap_or_default(),
                    raw: body.to_vec(),
                    conversation_id: None,
                    thread_id: None,
                    web_link: self.web_link(gm),
                    received_at: f.internal_date().map(|d| d.with_timezone(&Utc)),
                });
            }
        }
        let _ = s.logout().await;
        Ok(FetchResult {
            messages,
            state: Some(serde_json::to_string(&PollState { uid_validity: validity, last_uid }).unwrap_or_default()),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn email() -> OutgoingEmail {
        OutgoingEmail {
            from_email: "me@gmail.com".into(),
            from_name: "Me".into(),
            to_email: "ana@example.org".into(),
            to_name: "Ana".into(),
            subject: "Re: Hello".into(),
            body: "Hi Ana\n".into(),
            message_id: "<am-2@gmail.com>".into(),
            thread: Some(ThreadRef { references: vec!["<am-1@gmail.com>".into()], reply_to_provider_id: None }),
            list_unsubscribe: Some("mailto:me@gmail.com?subject=unsubscribe".into()),
            attachments: vec![],
        }
    }

    #[test]
    fn message_has_threading_and_unsubscribe_headers() {
        let raw = String::from_utf8(build_message(&email()).unwrap().formatted()).unwrap();
        assert!(raw.contains("Message-ID: <am-2@gmail.com>"));
        assert!(raw.contains("In-Reply-To: <am-1@gmail.com>"));
        assert!(raw.contains("References: <am-1@gmail.com>"));
        assert!(raw.contains("List-Unsubscribe: <mailto:me@gmail.com?subject=unsubscribe>"));
        assert!(raw.contains("Content-Type: text/plain"));
    }

    #[test]
    fn login_errors_are_explained() {
        assert!(matches!(explain_login_error("[AUTHENTICATIONFAILED] Invalid credentials (Failure)"), ProviderError::Auth(m) if m.contains("app password")));
        assert!(matches!(explain_login_error("[ALERT] Application-specific password required"), ProviderError::Auth(m) if m.contains("2-Step")));
        assert!(matches!(explain_login_error("Your account is not enabled for IMAP use."), ProviderError::Auth(m) if m.contains("IMAP")));
        assert!(matches!(explain_login_error("connection reset"), ProviderError::Temporary(_)));
    }
}
