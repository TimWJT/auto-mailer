//! Build-time configuration.

/// The Microsoft Entra app registration's client ID (public, not a secret).
/// Paste it here after following docs/microsoft-app-registration.md, or set
/// the `AUTO_MAILER_MS_CLIENT_ID` environment variable when building (CI uses
/// a repository variable of the same name).
const MS_CLIENT_ID: &str = "";

pub fn ms_client_id() -> Option<&'static str> {
    let id = option_env!("AUTO_MAILER_MS_CLIENT_ID").unwrap_or(MS_CLIENT_ID).trim();
    (!id.is_empty()).then_some(id)
}

/// Dry-run is the default in development builds so nobody emails a real
/// person by accident while working on the app.
pub const DRY_RUN_DEFAULT: bool = cfg!(debug_assertions);

/// Attachments above this size are refused (Graph's simple-attachment limit).
pub const MAX_ATTACHMENT_BYTES: u64 = 3 * 1024 * 1024;

/// An account connected less recently than this counts as "new" for the
/// volume warning.
pub const NEW_ACCOUNT_DAYS: i64 = 14;

/// Daily volume above which the launch checklist warns.
pub const VOLUME_WARNING_PER_DAY: i64 = 100;

pub const VOLUME_WARNING: &str = "Sending a lot of email from one account — especially cold outreach from a newly connected account — can get the account flagged, suspended, or sent to spam. Start low (20–50 a day) and increase gradually over a few weeks. You can continue anyway.";

pub const DEFAULT_FOOTER: &str = "If you'd rather not hear from me, just reply 'stop'.";
