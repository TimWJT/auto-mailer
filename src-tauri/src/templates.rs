//! Merge fields: `{{first_name}}`, `{{company}}`, any custom field, with an
//! optional fallback: `{{first_name|there}}`. A blank field without a fallback
//! is an error, so the contact is skipped instead of getting "Hi ,".

use std::collections::{BTreeSet, HashMap};

/// Field names are matched case-insensitively and with spaces/dashes treated
/// as underscores, so a CSV header "First Name" fills `{{first_name}}`.
pub fn normalise_key(key: &str) -> String {
    let mut out = String::with_capacity(key.len());
    let mut last_us = false;
    for c in key.trim().chars() {
        let c = c.to_ascii_lowercase();
        if c.is_alphanumeric() {
            out.push(c);
            last_us = false;
        } else if !last_us && !out.is_empty() {
            out.push('_');
            last_us = true;
        }
    }
    while out.ends_with('_') {
        out.pop();
    }
    out
}

#[derive(Debug, Clone, PartialEq)]
pub struct Rendered {
    pub text: String,
    /// Fields that were blank and had no fallback. Non-empty means "don't send".
    pub missing: Vec<String>,
}

/// Render `template` with `fields` (keys already normalised).
pub fn render(template: &str, fields: &HashMap<String, String>) -> Rendered {
    let mut text = String::with_capacity(template.len());
    let mut missing = BTreeSet::new();
    let mut rest = template;
    while let Some(start) = rest.find("{{") {
        text.push_str(&rest[..start]);
        let after = &rest[start + 2..];
        let Some(end) = after.find("}}") else {
            // Unclosed: keep the text literally.
            text.push_str(&rest[start..]);
            rest = "";
            break;
        };
        let inner = &after[..end];
        let (name, fallback) = match inner.split_once('|') {
            Some((n, f)) => (n, Some(f.trim())),
            None => (inner, None),
        };
        let key = normalise_key(name);
        let value = fields.get(&key).map(|v| v.trim()).filter(|v| !v.is_empty());
        match (value, fallback) {
            (Some(v), _) => text.push_str(v),
            (None, Some(f)) => text.push_str(f),
            (None, None) => {
                missing.insert(if key.is_empty() { name.trim().to_string() } else { key });
            }
        }
        rest = &after[end + 2..];
    }
    text.push_str(rest);
    Rendered {
        text,
        missing: missing.into_iter().collect(),
    }
}

/// Every field name a template refers to (for the editor's field list).
pub fn referenced_fields(template: &str) -> Vec<String> {
    let mut out = BTreeSet::new();
    let mut rest = template;
    while let Some(start) = rest.find("{{") {
        let after = &rest[start + 2..];
        let Some(end) = after.find("}}") else { break };
        let name = after[..end].split('|').next().unwrap_or("");
        let key = normalise_key(name);
        if !key.is_empty() {
            out.insert(key);
        }
        rest = &after[end + 2..];
    }
    out.into_iter().collect()
}

/// Put the parts of an outgoing plain-text email together.
pub fn assemble_body(body: &str, signature: &str, footer: &str) -> String {
    let mut out = body.trim_end().to_string();
    for part in [signature, footer] {
        let part = part.trim();
        if !part.is_empty() {
            out.push_str("\n\n");
            out.push_str(part);
        }
    }
    out.push('\n');
    out
}

/// "Re: " for a follow-up in the same thread, without stacking "Re: Re:".
pub fn reply_subject(original: &str) -> String {
    let trimmed = original.trim();
    let lower = trimmed.to_ascii_lowercase();
    if lower.starts_with("re:") {
        trimmed.to_string()
    } else {
        format!("Re: {trimmed}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fields(pairs: &[(&str, &str)]) -> HashMap<String, String> {
        pairs
            .iter()
            .map(|(k, v)| (normalise_key(k), v.to_string()))
            .collect()
    }

    #[test]
    fn fills_fields_case_and_space_insensitively() {
        let f = fields(&[("First Name", "Ana"), ("Company", "Acme")]);
        let r = render("Hi {{first_name}} at {{ COMPANY }}!", &f);
        assert_eq!(r.text, "Hi Ana at Acme!");
        assert!(r.missing.is_empty());
    }

    #[test]
    fn fallback_used_when_blank_or_absent() {
        let f = fields(&[("first_name", "   ")]);
        let r = render("Hi {{first_name|there}}, from {{city | your city}}", &f);
        assert_eq!(r.text, "Hi there, from your city");
        assert!(r.missing.is_empty());
    }

    #[test]
    fn blank_without_fallback_is_reported_missing() {
        let f = fields(&[("first_name", "")]);
        let r = render("Hi {{first_name}} at {{company}} {{company}}", &f);
        assert_eq!(r.missing, vec!["company".to_string(), "first_name".to_string()]);
    }

    #[test]
    fn empty_fallback_is_allowed() {
        let r = render("Hi{{first_name|}}!", &HashMap::new());
        assert_eq!(r.text, "Hi!");
        assert!(r.missing.is_empty());
    }

    #[test]
    fn unclosed_braces_are_left_alone() {
        let r = render("Price {{ is high", &HashMap::new());
        assert_eq!(r.text, "Price {{ is high");
        assert!(r.missing.is_empty());
    }

    #[test]
    fn referenced_fields_lists_unique_normalised_names() {
        assert_eq!(
            referenced_fields("{{First Name|x}} {{company}} {{company}}"),
            vec!["company", "first_name"]
        );
    }

    #[test]
    fn assembles_signature_and_footer() {
        assert_eq!(
            assemble_body("Hello\n\n", "Tim", "Reply stop to opt out."),
            "Hello\n\nTim\n\nReply stop to opt out.\n"
        );
        assert_eq!(assemble_body("Hello", "", ""), "Hello\n");
    }

    #[test]
    fn reply_subject_does_not_stack() {
        assert_eq!(reply_subject("Quick question"), "Re: Quick question");
        assert_eq!(reply_subject("RE: Quick question"), "RE: Quick question");
    }

    #[test]
    fn normalise_key_examples() {
        assert_eq!(normalise_key(" First-Name "), "first_name");
        assert_eq!(normalise_key("Company  Name!"), "company_name");
    }
}
