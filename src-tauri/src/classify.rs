//! Classify an incoming message: real reply, out-of-office auto-reply,
//! bounce, or unsubscribe request. Pure functions over the raw MIME bytes so
//! they can be tested with real `.eml` samples.
//!
//! When in doubt the answer is "reply": stopping a sequence is the safe default.

use mail_parser::{MessageParser, PartType};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Kind {
    Reply,
    AutoReply,
    Bounce,
    Unsubscribe,
    /// Delivery delays and similar notices: not a reply, not a bounce.
    Ignore,
}

impl Kind {
    pub fn as_str(self) -> &'static str {
        match self {
            Kind::Reply => "reply",
            Kind::AutoReply => "auto_reply",
            Kind::Bounce => "bounce",
            Kind::Unsubscribe => "unsubscribe",
            Kind::Ignore => "ignore",
        }
    }
}

/// Everything the classifier and the matcher need from one message.
#[derive(Debug, Clone, Default)]
pub struct MailFacts {
    pub message_id: Option<String>,
    pub from_email: String,
    pub from_name: String,
    pub subject: String,
    /// Normalised ids from `In-Reply-To` and `References`.
    pub thread_ids: Vec<String>,
    /// For bounces: ids of the original message found in the report.
    pub original_ids: Vec<String>,
    /// For bounces: the recipient(s) that failed.
    pub failed_recipients: Vec<String>,
    pub dsn_actions: Vec<String>,
    pub content_type: String,
    pub report_type: String,
    pub auto_submitted: Option<String>,
    pub has_autoreply_header: bool,
    pub precedence: Option<String>,
    pub body_text: String,
    pub received_at: Option<chrono::DateTime<chrono::Utc>>,
}

/// Message-IDs compare without angle brackets, whitespace or case.
pub fn norm_id(id: &str) -> String {
    id.trim()
        .trim_start_matches('<')
        .trim_end_matches('>')
        .trim()
        .to_ascii_lowercase()
}

pub fn parse(raw: &[u8]) -> Option<MailFacts> {
    let msg = MessageParser::default().parse(raw)?;
    let mut f = MailFacts {
        message_id: msg.message_id().map(norm_id),
        subject: msg.subject().unwrap_or("").trim().to_string(),
        ..Default::default()
    };
    if let Some(addr) = msg.from().and_then(|a| a.first()) {
        f.from_email = addr.address().unwrap_or("").trim().to_ascii_lowercase();
        f.from_name = addr.name().unwrap_or("").trim().to_string();
    }
    for header in [msg.in_reply_to(), msg.references()] {
        if let Some(list) = header.as_text_list() {
            for id in list {
                // Some clients put several ids in one value.
                for part in id.split_whitespace() {
                    let n = norm_id(part);
                    if !n.is_empty() && !f.thread_ids.contains(&n) {
                        f.thread_ids.push(n);
                    }
                }
            }
        }
    }
    if let Some(ct) = msg.header("Content-Type").and_then(|h| h.as_content_type()) {
        f.content_type = format!(
            "{}/{}",
            ct.ctype().to_ascii_lowercase(),
            ct.subtype().unwrap_or("").to_ascii_lowercase()
        );
        f.report_type = ct
            .attribute("report-type")
            .unwrap_or("")
            .to_ascii_lowercase();
    }
    f.auto_submitted = msg
        .header_raw("Auto-Submitted")
        .map(|v| v.trim().to_ascii_lowercase());
    f.has_autoreply_header = msg.header_raw("X-Autoreply").is_some()
        || msg.header_raw("X-Autorespond").is_some()
        || msg.header_raw("X-Autoresponder").is_some();
    f.precedence = msg
        .header_raw("Precedence")
        .map(|v| v.trim().to_ascii_lowercase());
    f.body_text = msg.body_text(0).map(|b| b.into_owned()).unwrap_or_default();
    f.received_at = msg
        .date()
        .and_then(|d| chrono::DateTime::from_timestamp(d.to_timestamp(), 0));

    // Bounce details: embedded original messages / headers and DSN fields.
    let own_id = f.message_id.clone();
    for part in msg.parts.iter().skip(1) {
        match &part.body {
            PartType::Message(inner) => {
                if let Some(id) = inner.message_id() {
                    push_unique(&mut f.original_ids, norm_id(id));
                }
                if let Some(to) = inner.to().and_then(|a| a.first()).and_then(|a| a.address()) {
                    push_unique(&mut f.failed_recipients, to.trim().to_ascii_lowercase());
                }
            }
            PartType::Text(t) => scan_report_text(t, &own_id, &mut f),
            PartType::Binary(b) | PartType::InlineBinary(b) => {
                scan_report_text(&String::from_utf8_lossy(b), &own_id, &mut f)
            }
            _ => {}
        }
    }
    Some(f)
}

fn push_unique(v: &mut Vec<String>, s: String) {
    if !s.is_empty() && !v.contains(&s) {
        v.push(s);
    }
}

/// Pull DSN fields and quoted original headers out of report text.
fn scan_report_text(text: &str, own_id: &Option<String>, f: &mut MailFacts) {
    for line in text.lines() {
        let Some((name, value)) = line.split_once(':') else { continue };
        let name = name.trim().to_ascii_lowercase();
        let value = value.trim();
        match name.as_str() {
            "message-id" => {
                let id = norm_id(value);
                if Some(&id) != own_id.as_ref() {
                    push_unique(&mut f.original_ids, id);
                }
            }
            "final-recipient" | "original-recipient" => {
                // "rfc822; someone@example.com"
                let addr = value.rsplit(';').next().unwrap_or(value);
                push_unique(&mut f.failed_recipients, addr.trim().to_ascii_lowercase());
            }
            "action" => push_unique(&mut f.dsn_actions, value.to_ascii_lowercase()),
            _ => {}
        }
    }
}

const BOUNCE_SENDERS: &[&str] = &[
    "mailer-daemon",
    "postmaster",
    "mail-delivery-subsystem",
    "microsoftexchange329e71ec88ae4615bbc36ab6ce41109e",
];

const BOUNCE_SUBJECTS: &[&str] = &[
    "undeliverable",
    "undelivered mail",
    "delivery status notification",
    "delivery has failed",
    "mail delivery failed",
    "returned mail",
    "failure notice",
    "delivery failure",
    "message not delivered",
    "address not found",
];

const AUTO_REPLY_SUBJECTS: &[&str] = &[
    "automatic reply",
    "auto reply",
    "auto-reply",
    "autoreply",
    "auto:",
    "out of office",
    "out of the office",
    "ooo:",
    "away from the office",
    "on vacation",
    "abwesenheitsnotiz",
    "réponse automatique",
    "respuesta automática",
];

pub fn classify(f: &MailFacts) -> Kind {
    let subject = f.subject.to_ascii_lowercase();
    let local = f.from_email.split('@').next().unwrap_or("");
    let is_report = f.content_type == "multipart/report" && f.report_type == "delivery-status";
    let bounce_sender = BOUNCE_SENDERS.contains(&local);
    let bounce_subject = BOUNCE_SUBJECTS.iter().any(|s| subject.contains(s));

    if is_report || bounce_sender || (bounce_subject && f.auto_submitted.is_some()) {
        // A "delayed" report is only a warning; the message may still arrive.
        let only_delayed = !f.dsn_actions.is_empty()
            && f.dsn_actions.iter().all(|a| a == "delayed" || a == "relayed" || a == "delivered" || a == "expanded");
        if only_delayed || subject.contains("(delay)") || subject.contains("delayed") {
            return Kind::Ignore;
        }
        return Kind::Bounce;
    }

    let auto_header = matches!(f.auto_submitted.as_deref(), Some(v) if v != "no")
        || f.has_autoreply_header
        || matches!(f.precedence.as_deref(), Some("auto_reply"));
    let auto_subject = AUTO_REPLY_SUBJECTS.iter().any(|s| subject.starts_with(s) || subject.contains(&format!("{s}:")));
    if auto_header || auto_subject {
        return Kind::AutoReply;
    }

    if is_unsubscribe(&new_text(&f.body_text)) {
        return Kind::Unsubscribe;
    }
    Kind::Reply
}

/// The part of a reply the person actually wrote: everything above the
/// quoted original.
pub fn new_text(body: &str) -> String {
    let mut out = Vec::new();
    for line in body.lines() {
        let t = line.trim();
        let lower = t.to_ascii_lowercase();
        if t.starts_with('>') {
            continue;
        }
        let is_marker = (lower.starts_with("on ") && lower.ends_with("wrote:"))
            || lower.starts_with("-----original message-----")
            || t.starts_with("________________________________")
            || (lower.starts_with("from:") && !out.is_empty())
            || lower.starts_with("sent from my ");
        if is_marker {
            break;
        }
        out.push(t);
    }
    out.join("\n").trim().to_string()
}

const UNSUB_PHRASES: &[&str] = &[
    "unsubscribe",
    "remove me",
    "take me off",
    "please remove",
    "opt out",
    "opt-out",
    "do not contact",
    "don't contact",
    "do not email",
    "don't email",
    "stop emailing",
    "stop contacting",
    "not interested, remove",
];

pub fn is_unsubscribe(new_text: &str) -> bool {
    let text: String = new_text
        .to_ascii_lowercase()
        .chars()
        .map(|c| if c.is_alphanumeric() || c == '\'' || c == '-' { c } else { ' ' })
        .collect();
    let words: Vec<&str> = text.split_whitespace().collect();
    if words.is_empty() {
        return false;
    }
    let joined = words.join(" ");
    if UNSUB_PHRASES.iter().any(|p| joined.starts_with(p)) {
        return true;
    }
    // "Stop", "STOP!", "stop please" — but not "stop by our stand on Friday".
    if words[0] == "stop" && words.len() <= 3 {
        return true;
    }
    // Short message that is mostly an opt-out ("Please unsubscribe me, thanks").
    words.len() <= 8 && UNSUB_PHRASES.iter().any(|p| joined.contains(p))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture(name: &str) -> MailFacts {
        let path = format!("{}/tests/fixtures/{name}", env!("CARGO_MANIFEST_DIR"));
        let raw = std::fs::read(&path).unwrap_or_else(|_| panic!("missing fixture {path}"));
        parse(&raw).expect("parses")
    }

    #[test]
    fn gmail_reply_is_reply_and_links_thread() {
        let f = fixture("gmail_reply.eml");
        assert_eq!(classify(&f), Kind::Reply);
        assert_eq!(f.from_email, "ana.lopez@example.org");
        assert!(f.thread_ids.contains(&"am-1b2c3d4e@mailer.example.com".to_string()));
    }

    #[test]
    fn outlook_reply_is_reply() {
        let f = fixture("outlook_reply.eml");
        assert_eq!(classify(&f), Kind::Reply);
        assert!(f.thread_ids.contains(&"am-55aa66bb@outlook.com".to_string()));
    }

    #[test]
    fn outlook_out_of_office_is_auto_reply() {
        assert_eq!(classify(&fixture("outlook_ooo.eml")), Kind::AutoReply);
    }

    #[test]
    fn gmail_vacation_responder_is_auto_reply() {
        assert_eq!(classify(&fixture("gmail_vacation.eml")), Kind::AutoReply);
    }

    #[test]
    fn gmail_bounce_extracts_original_and_recipient() {
        let f = fixture("gmail_bounce.eml");
        assert_eq!(classify(&f), Kind::Bounce);
        assert!(f.original_ids.contains(&"am-1b2c3d4e@mailer.example.com".to_string()));
        assert!(f.failed_recipients.contains(&"nobody@example.net".to_string()));
    }

    #[test]
    fn outlook_ndr_is_bounce() {
        let f = fixture("outlook_ndr.eml");
        assert_eq!(classify(&f), Kind::Bounce);
        assert!(f.original_ids.contains(&"am-55aa66bb@outlook.com".to_string()));
    }

    #[test]
    fn delay_notification_is_ignored() {
        assert_eq!(classify(&fixture("gmail_delay.eml")), Kind::Ignore);
    }

    #[test]
    fn stop_reply_is_unsubscribe() {
        assert_eq!(classify(&fixture("gmail_stop.eml")), Kind::Unsubscribe);
    }

    #[test]
    fn unsubscribe_phrases() {
        for yes in ["STOP", "Stop!", "stop please", "Unsubscribe", "Please remove me from your list.",
                    "Take me off this list", "remove me", "Please unsubscribe me, thanks"] {
            assert!(is_unsubscribe(yes), "{yes}");
        }
        for no in ["Stop by our stand on Friday and we can talk more", "Sounds good, let's talk Tuesday",
                   "", "I can't remove the old version, can you help?  It keeps failing on install every time"] {
            assert!(!is_unsubscribe(no), "{no}");
        }
    }

    #[test]
    fn new_text_strips_quotes() {
        let body = "Yes please!\n\nOn Mon, 12 Oct 2026 at 09:00, Tim <tim@x.com> wrote:\n> Hi Ana\n> unsubscribe\n";
        assert_eq!(new_text(body), "Yes please!");
        let outlook = "Not now\n\n________________________________\nFrom: Tim\nSent: Monday\n";
        assert_eq!(new_text(outlook), "Not now");
    }

    #[test]
    fn norm_id_strips_brackets_and_case() {
        assert_eq!(norm_id(" <AbC@Example.com> "), "abc@example.com");
    }
}
