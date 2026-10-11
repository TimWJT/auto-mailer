//! "Sign in with Microsoft": OAuth 2.0 authorisation code flow with PKCE,
//! public client (no secret), system browser + loopback redirect.

use std::time::Duration;

use base64::Engine;
use rand::RngExt;
use serde::Deserialize;
use sha2::{Digest, Sha256};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;

use crate::config;
use crate::providers::ProviderError;

const AUTHORITY: &str = "https://login.microsoftonline.com/common/oauth2/v2.0";
pub const SCOPES: &str = "openid profile offline_access User.Read Mail.Send Mail.ReadWrite";
/// Tenant id Microsoft uses for all personal (Outlook.com/Hotmail/Live) accounts.
const CONSUMER_TENANT: &str = "9188040d-6c67-4c5b-b112-36a304b66dad";
const SIGN_IN_TIMEOUT: Duration = Duration::from_secs(300);

#[derive(Debug, Deserialize)]
pub struct TokenResponse {
    pub access_token: String,
    pub refresh_token: Option<String>,
    pub expires_in: i64,
    pub id_token: Option<String>,
}

#[derive(Debug)]
pub struct SignIn {
    pub tokens: TokenResponse,
    pub email: String,
    pub display_name: String,
    pub account_type: String,
}

fn b64url(bytes: &[u8]) -> String {
    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(bytes)
}

pub fn pkce_pair() -> (String, String) {
    let mut bytes = [0u8; 48];
    rand::rng().fill(&mut bytes);
    let verifier = b64url(&bytes);
    let challenge = b64url(&Sha256::digest(verifier.as_bytes()));
    (verifier, challenge)
}

/// Plain-English explanations for the errors people actually hit.
pub fn explain(error: &str, description: &str) -> ProviderError {
    let d = description.to_ascii_lowercase();
    let e = error.to_ascii_lowercase();
    if d.contains("aadsts65001") || d.contains("aadsts90094") || d.contains("aadsts90095")
        || d.contains("admin") || e == "consent_required"
    {
        return ProviderError::Auth(
            "Your organisation needs its IT admin to approve Auto Mailer before you can use it with this account. Ask them to allow the app, or use a different account.".into(),
        );
    }
    if e == "access_denied" {
        return ProviderError::Auth("Sign-in was cancelled. Try again when you're ready.".into());
    }
    if e == "invalid_grant" || e == "interaction_required" {
        return ProviderError::Auth("Microsoft needs you to sign in again.".into());
    }
    if d.contains("aadsts700016") || d.contains("aadsts7000218") || d.contains("aadsts50011") {
        return ProviderError::Auth(
            "The Microsoft app registration isn't set up correctly. See docs/microsoft-app-registration.md.".into(),
        );
    }
    let first_line = description.lines().next().unwrap_or(description);
    ProviderError::Auth(format!("Microsoft sign-in failed: {first_line}"))
}

async fn token_request(http: &reqwest::Client, form: &[(&str, &str)]) -> Result<TokenResponse, ProviderError> {
    let resp = http
        .post(format!("{AUTHORITY}/token"))
        .form(form)
        .timeout(Duration::from_secs(30))
        .send()
        .await
        .map_err(|e| ProviderError::Temporary(format!("Couldn't reach Microsoft: {e}")))?;
    let status = resp.status();
    let text = resp
        .text()
        .await
        .map_err(|e| ProviderError::Temporary(format!("Couldn't reach Microsoft: {e}")))?;
    if status.is_success() {
        return serde_json::from_str(&text)
            .map_err(|_| ProviderError::Temporary("Microsoft sent an unexpected reply.".into()));
    }
    #[derive(Deserialize, Default)]
    struct Err {
        #[serde(default)]
        error: String,
        #[serde(default)]
        error_description: String,
    }
    let err: Err = serde_json::from_str(&text).unwrap_or_default();
    if status.is_server_error() {
        return Err(ProviderError::Temporary("Microsoft's sign-in service is having trouble. Try again later.".into()));
    }
    Err(explain(&err.error, &err.error_description))
}

pub async fn refresh(http: &reqwest::Client, refresh_token: &str) -> Result<TokenResponse, ProviderError> {
    let client_id = config::ms_client_id().ok_or_else(not_configured)?;
    token_request(
        http,
        &[
            ("client_id", client_id),
            ("grant_type", "refresh_token"),
            ("refresh_token", refresh_token),
            ("scope", SCOPES),
        ],
    )
    .await
}

fn not_configured() -> ProviderError {
    ProviderError::Auth(
        "Microsoft sign-in isn't set up in this build of Auto Mailer yet (no client ID). See docs/microsoft-app-registration.md.".into(),
    )
}

/// Run the whole browser sign-in. `open` launches the system browser.
pub async fn sign_in(http: &reqwest::Client, open: impl FnOnce(&str) -> Result<(), String>) -> Result<SignIn, ProviderError> {
    let client_id = config::ms_client_id().ok_or_else(not_configured)?;
    let (verifier, challenge) = pkce_pair();
    let mut state_bytes = [0u8; 16];
    rand::rng().fill(&mut state_bytes);
    let state = b64url(&state_bytes);

    let v4 = TcpListener::bind("127.0.0.1:0")
        .await
        .map_err(|e| ProviderError::Temporary(format!("Couldn't start sign-in: {e}")))?;
    let port = v4.local_addr().map_err(|e| ProviderError::Temporary(e.to_string()))?.port();
    // Browsers may resolve "localhost" to ::1 first; listen there too if we can.
    let v6 = TcpListener::bind(("::1", port)).await.ok();
    let redirect = format!("http://localhost:{port}");

    let mut url = url::Url::parse(&format!("{AUTHORITY}/authorize")).expect("static url");
    url.query_pairs_mut()
        .append_pair("client_id", client_id)
        .append_pair("response_type", "code")
        .append_pair("redirect_uri", &redirect)
        .append_pair("response_mode", "query")
        .append_pair("scope", SCOPES)
        .append_pair("state", &state)
        .append_pair("code_challenge", &challenge)
        .append_pair("code_challenge_method", "S256")
        .append_pair("prompt", "select_account");
    open(url.as_str()).map_err(|e| ProviderError::Temporary(format!("Couldn't open your browser: {e}")))?;

    let code = tokio::time::timeout(SIGN_IN_TIMEOUT, wait_for_code(&v4, v6.as_ref(), &state))
        .await
        .map_err(|_| ProviderError::Auth("Sign-in timed out. Try again.".into()))??;

    let tokens = token_request(
        http,
        &[
            ("client_id", client_id),
            ("grant_type", "authorization_code"),
            ("code", &code),
            ("redirect_uri", &redirect),
            ("code_verifier", &verifier),
            ("scope", SCOPES),
        ],
    )
    .await?;
    if tokens.refresh_token.is_none() {
        return Err(ProviderError::Auth("Microsoft didn't allow offline access. Try signing in again.".into()));
    }

    #[derive(Deserialize)]
    #[serde(rename_all = "camelCase")]
    struct Me {
        display_name: Option<String>,
        mail: Option<String>,
        user_principal_name: Option<String>,
    }
    let me: Me = http
        .get("https://graph.microsoft.com/v1.0/me?$select=displayName,mail,userPrincipalName")
        .bearer_auth(&tokens.access_token)
        .send()
        .await
        .map_err(|e| ProviderError::Temporary(format!("Couldn't reach Microsoft: {e}")))?
        .json()
        .await
        .map_err(|_| ProviderError::Temporary("Microsoft sent an unexpected reply.".into()))?;
    let email = me
        .mail
        .filter(|m| m.contains('@'))
        .or(me.user_principal_name)
        .unwrap_or_default()
        .to_ascii_lowercase();
    if email.is_empty() {
        return Err(ProviderError::Auth("This Microsoft account has no email address.".into()));
    }
    let tenant = tokens.id_token.as_deref().and_then(tenant_from_id_token);
    let account_type = match tenant.as_deref() {
        Some(CONSUMER_TENANT) => "outlook_personal",
        Some(_) => "microsoft_365",
        None => personal_by_domain(&email),
    };
    Ok(SignIn {
        email,
        display_name: me.display_name.unwrap_or_default(),
        account_type: account_type.into(),
        tokens,
    })
}

fn personal_by_domain(email: &str) -> &'static str {
    let d = email.rsplit('@').next().unwrap_or("");
    let personal = ["outlook.", "hotmail.", "live.", "msn.com", "passport."]
        .iter()
        .any(|p| d.starts_with(p) || d == *p);
    if personal { "outlook_personal" } else { "microsoft_365" }
}

/// Read the `tid` claim. The token came straight from Microsoft over TLS, and
/// we only use it to pick a default daily limit, so no signature check.
pub fn tenant_from_id_token(id_token: &str) -> Option<String> {
    let payload = id_token.split('.').nth(1)?;
    let bytes = base64::engine::general_purpose::URL_SAFE_NO_PAD
        .decode(payload.trim_end_matches('='))
        .ok()?;
    let v: serde_json::Value = serde_json::from_slice(&bytes).ok()?;
    v.get("tid")?.as_str().map(str::to_string)
}

async fn wait_for_code(v4: &TcpListener, v6: Option<&TcpListener>, state: &str) -> Result<String, ProviderError> {
    loop {
        let accepted = match v6 {
            Some(v6) => tokio::select! { a = v4.accept() => a, a = v6.accept() => a },
            None => v4.accept().await,
        };
        let Ok((mut sock, _)) = accepted else { continue };
        let mut buf = vec![0u8; 8192];
        let n = match tokio::time::timeout(Duration::from_secs(10), sock.read(&mut buf)).await {
            Ok(Ok(n)) => n,
            _ => continue,
        };
        let req = String::from_utf8_lossy(&buf[..n]);
        let path = req.split_whitespace().nth(1).unwrap_or("/");
        let Ok(url) = url::Url::parse(&format!("http://localhost{path}")) else { continue };
        let get = |k: &str| url.query_pairs().find(|(n, _)| n == k).map(|(_, v)| v.into_owned());
        let (code, error, desc, got_state) = (get("code"), get("error"), get("error_description"), get("state"));
        if code.is_none() && error.is_none() {
            // Favicon or a stray request.
            let _ = sock.write_all(b"HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\nConnection: close\r\n\r\n").await;
            continue;
        }
        let ok = code.is_some() && got_state.as_deref() == Some(state);
        let page = if ok {
            "<h2>You're signed in.</h2><p>You can close this tab and go back to Auto Mailer.</p>"
        } else {
            "<h2>Sign-in didn't work.</h2><p>Go back to Auto Mailer to see what happened.</p>"
        };
        let html = format!(
            "<!doctype html><html><head><meta charset=utf-8><title>Auto Mailer</title></head><body style=\"font-family:system-ui;margin:3em\">{page}</body></html>"
        );
        let _ = sock
            .write_all(format!("HTTP/1.1 200 OK\r\nContent-Type: text/html; charset=utf-8\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{html}", html.len()).as_bytes())
            .await;
        if let Some(e) = error {
            return Err(explain(&e, desc.as_deref().unwrap_or("")));
        }
        if got_state.as_deref() != Some(state) {
            return Err(ProviderError::Auth("Sign-in reply didn't match. Try again.".into()));
        }
        return Ok(code.unwrap_or_default());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pkce_challenge_is_sha256_of_verifier() {
        let (v, c) = pkce_pair();
        assert!(v.len() >= 43 && v.len() <= 128);
        assert_eq!(c, b64url(&Sha256::digest(v.as_bytes())));
    }

    #[test]
    fn reads_tenant_from_id_token() {
        let payload = b64url(br#"{"tid":"9188040d-6c67-4c5b-b112-36a304b66dad","name":"x"}"#);
        let token = format!("eyJhbGciOiJub25lIn0.{payload}.sig");
        assert_eq!(tenant_from_id_token(&token).as_deref(), Some(CONSUMER_TENANT));
        assert_eq!(tenant_from_id_token("garbage"), None);
    }

    #[test]
    fn admin_consent_error_is_explained() {
        let e = explain("invalid_client", "AADSTS65001: The user or administrator has not consented");
        assert!(matches!(e, ProviderError::Auth(m) if m.contains("IT admin")));
        assert_eq!(personal_by_domain("a@hotmail.co.uk"), "outlook_personal");
        assert_eq!(personal_by_domain("a@contoso.com"), "microsoft_365");
    }
}
