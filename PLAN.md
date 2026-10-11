# Auto Mailer — build plan

**Status: plan only, written 12 October 2026. No code exists yet.**

Read this whole file before writing any code. It records decisions already made with Tim; don't re-open them unless something here turns out to be impossible, and if so, stop and ask.

---

## 1. What this app is

A free, unofficial desktop app for **Windows and Mac** that sends personalised email campaigns with automatic follow-ups, and stops following up as soon as someone replies.

- Anyone can download it. It is **not** published to any store and **not** verified by Google, Microsoft or Apple. Install warnings are accepted.
- Each user connects **their own** email accounts. Everything runs and is stored **on the user's own computer**. There is **no server**, no sign-up, no account with us.
- Used for both cold outreach and emailing people who already know the sender.

## 2. Decisions already made

| Topic | Decision |
|---|---|
| Name | **Auto Mailer** (folder `auto-mailer`) |
| Platforms | Windows and Mac. Tim has no Mac — the Mac build is produced by GitHub Actions and tested later by other people. |
| Stack | **Tauri 2** (Rust backend) + **Svelte 5 + TypeScript + Vite** frontend. Same family as Tim's `markdown-viewer` app (`..\markdown-viewer`) — reuse its patterns where they fit, especially `.github/workflows/release.yml`. Chosen for speed: small download, fast start-up, low memory. |
| Gmail connection | **App password** (IMAP + SMTP). No "Sign in with Google" button — that would cap the app at 100 Google users forever and show an "unverified app" screen. |
| Microsoft connection | **"Sign in with Microsoft"** (OAuth, Microsoft Graph API). Outlook.com/Hotmail/Live don't accept app passwords any more. |
| Email format | **Plain text only.** Looks like a personal email; best inbox placement. No HTML designer. |
| Running | **Only while the app window is open** (minimised is fine). No tray icon, no start-with-Windows. On reopening, catch up gradually. |
| Daily volume | **No forced low cap.** Each account defaults to just under its provider's own limit so the provider never locks it out. Show a **warning** about the risks of high volume; the user decides. |
| Tracking | **Replies only.** No open or click tracking (needs a public server). |
| Cost | Free to build and distribute. No code signing, no paid developer accounts. |

## 3. Out of scope (do not build)

- Open/click tracking, link shortening, tracking pixels.
- HTML email designer.
- "Sign in with Google" (OAuth for Gmail).
- Any server, cloud sync, user accounts, licensing, payments, analytics/telemetry.
- Tray icon, auto-start, background service.
- AI-written emails.
- Linux builds (the CI template from `markdown-viewer` builds Linux — drop that matrix entry).

These may come later; don't design anything that makes them impossible, but don't build them.

---

## 4. How users connect accounts

### 4.1 Gmail — app password

In-app guided setup (with links and short instructions):

1. Turn on 2-Step Verification: `https://myaccount.google.com/signinoptions/twosv`
2. Create an app password: `https://myaccount.google.com/apppasswords`
3. Paste email + app password into Auto Mailer → **Test connection**.

Technical:
- IMAP `imap.gmail.com:993` (TLS) for reading replies/bounces.
- SMTP `smtp.gmail.com:465` (TLS) for sending. Gmail saves SMTP-sent mail to Sent automatically — don't append a copy.
- Use Gmail's IMAP extensions where useful (`X-GM-THRID` for thread id, `X-GM-MSGID`).
- Clear error messages for: wrong app password, 2-Step Verification off, IMAP disabled, account type that blocks app passwords (some work/school Google accounts, Advanced Protection).

### 4.2 Microsoft — sign in button

- OAuth 2.0 authorisation code flow with **PKCE**, public client (no client secret), authority `https://login.microsoftonline.com/common` (personal **and** work/school accounts).
- Open the system browser; receive the redirect on a **loopback** listener (`http://localhost:<random port>`).
- Delegated scopes: `offline_access`, `User.Read`, `Mail.Send`, `Mail.ReadWrite`.
- Use **Microsoft Graph** (`https://graph.microsoft.com/v1.0`) for everything — not IMAP/SMTP.
- Refresh tokens silently; if refresh fails, mark the account "needs sign-in again" and pause its sends.
- Work/school accounts may be blocked until their IT admin approves the app — show a clear message when the consent error comes back.

**One-time setup by Tim (document it in `docs/microsoft-app-registration.md`):** register the app in the Microsoft Entra / Azure portal (free; it may ask for a free Azure account) → *App registrations → New* → supported accounts: **"Any organizational directory and personal Microsoft accounts"** → platform **"Mobile and desktop applications"**, redirect `http://localhost` → enable **"Allow public client flows"** → add the delegated Graph permissions above. The resulting **client ID** (not a secret) is compiled into the app via a config constant.

### 4.3 Credentials

- App passwords and Microsoft refresh tokens go in the OS secure store via the Rust `keyring` crate (Windows Credential Manager / macOS Keychain).
- **Never** in SQLite, config files, logs or error messages.

---

## 5. Features

### 5.1 Accounts
- Add several Gmail and Microsoft accounts; each is used independently.
- Test connection; show status (connected / needs attention / paused).
- Per-account daily limit, defaulting to just under the provider limit (rolling 24 hours):

  | Account type | Provider limit (approx.) | Default |
  |---|---|---|
  | Gmail (free) | 500/day | 450 |
  | Google Workspace | 2,000/day | 1,800 |
  | Outlook.com / Hotmail / Live | 300/day (lower for new accounts) | 280 |
  | Microsoft 365 work | 10,000 recipients/day, 30 messages/min | 9,000, max 30/min |

  Detect the type where possible (gmail.com vs custom domain on Google; Graph `/me` + tenant for Microsoft); otherwise let the user pick.
- If the provider rejects a send for hitting its limit, pause that account until the 24-hour window clears; don't retry in a loop.

### 5.2 Contacts
- Import **CSV and Excel** (`.xlsx`). Show a column-mapping step: match columns **by header name**, user confirms. Email column required; any other columns become custom fields.
- Validate email format; flag invalid rows instead of silently dropping them; show an import summary (added / updated / duplicates / invalid).
- De-duplicate by email address (case-insensitive, trimmed).
- Lists and tags; manual add/edit.
- **Do-not-contact list** (global, across all campaigns and accounts). Addresses on it are never emailed. Can import to it and remove from it.
- Delete contacts (with confirmation).

### 5.3 Templates and merge fields
- Plain-text subject and body.
- Merge fields `{{first_name}}`, `{{company}}`, any custom field. Fallback syntax: `{{first_name|there}}`.
- If a field is blank and has no fallback, **don't send** to that contact — flag it in the preview.
- Per-account plain-text signature.
- Preview the rendered email for any contact; step through contacts.
- Attachments: allowed but warn that attachments hurt deliverability on cold emails. Limit **3 MB per file** in v1 (Graph's simple attachment limit).

### 5.4 Campaigns (sequences)
- A campaign = sending account + contact list + steps.
- Step 1 = first email. Steps 2..n = follow-ups "if no reply after N days".
- Follow-ups go **in the same thread** by default (subject `Re: <original>`, `In-Reply-To` / `References` headers on Gmail; `createReply` on Graph). Option per step to start a new thread instead.
- Send window: days of the week + hours, in the user's local time zone. Default Mon–Fri, 9:00–17:00.
- Random gap between sends (default 45–120 seconds, configurable).
- Start, pause, resume, stop. Edit steps of a paused campaign (applies to emails not yet sent).
- **Stop rules:** a contact's sequence stops when they reply, someone else replies in the thread, the email bounces, or they unsubscribe. They're then marked accordingly.
- A contact can't be in the same campaign twice. Warn (don't block) if they're active in another campaign.
- **Launch checklist** before starting: number of contacts, how many skipped (do-not-contact, invalid, missing merge fields), estimated days to finish, preview of first email, **volume warning** (5.8).
- **Send test to myself** for any step.

### 5.5 Scheduler
- Runs inside the app (Rust, tokio task) while the window is open; ticks every ~30 seconds.
- Each tick: for each account that's healthy and under its daily limit, inside its send window, pick the next due email (oldest due first), send it, record it, wait the random gap.
- Next follow-up due time = previous step's sent time + N days, then pushed into the next send window.
- **Catch-up on reopen:** overdue emails are sent through the same queue with the same gaps and limits — never in a burst.
- Sends must be **idempotent**: mark "sending" before the network call, "sent" after; on start-up, anything stuck in "sending" is checked against the provider's Sent folder (by our Message-ID) before deciding whether to resend. Never send the same step to the same contact twice.
- Show "next send at …" and the queue on the dashboard.

### 5.6 Reply, bounce, auto-reply and unsubscribe detection
- Poll each account every **5 minutes** while open, plus once at start-up.
- Every outgoing email gets a Message-ID **we generate and store** (Gmail SMTP). For Graph, store `internetMessageId` and `conversationId` (see 8.1).
- **Reply:** an incoming message whose `In-Reply-To`/`References` contains one of our Message-IDs, or same Gmail thread id / Graph conversationId, from anyone other than the sender.
- **Auto-reply (out of office):** headers `Auto-Submitted: auto-replied`, `X-Autoreply`, `X-Autorespond`, `Precedence: auto_reply`, or subjects like "Automatic reply", "Out of office". Recorded and shown, but **does not** count as a reply — the sequence continues.
- **Bounce:** delivery-status notifications (from `mailer-daemon@`/`postmaster@`, `multipart/report`, "Undeliverable", "Delivery Status Notification"). Parse out the original Message-ID / recipient; mark the contact bounced and stop.
- **Unsubscribe:** a reply whose text starts with or is mostly "stop", "unsubscribe", "remove me", "take me off" etc. → add to do-not-contact and stop. Also treat as a reply.
- Anything ambiguous: count it as a reply (stopping is the safe default).
- Desktop notification when a real reply arrives (Tauri notification plugin).

### 5.7 Inbox and dashboard
- **Inbox:** all replies across all accounts in one list, newest first, with contact, campaign and classification. Open the reply in the app; a button opens it in Gmail/Outlook on the web. (Replying from inside the app is a later feature.)
- **Dashboard per campaign:** sent, replied (and reply rate), bounced, unsubscribed, auto-replies, waiting, finished with no reply, failed.
- **Contact timeline:** every email sent and received for that contact.
- Export campaign results and contacts to CSV.

### 5.8 Warnings and safety
- **Volume warning** at launch (and when raising an account's limit) if the campaign will send more than ~100/day from one account, or the account was connected recently: explain that high volume, especially cold outreach from new accounts, can get the account flagged, suspended or sent to spam, and suggest starting low and increasing gradually. The user can continue.
- Plain-text footer added by default (editable, removable with a warning): e.g. *"If you'd rather not hear from me, just reply 'stop'."*
- Add a `List-Unsubscribe: <mailto:sender?subject=unsubscribe>` header where the provider allows custom headers.
- Show a short note in settings that the user is responsible for following anti-spam law where they live (honest sender details, honouring opt-outs).

### 5.9 App basics
- First-run welcome: connect an account → import contacts → create a campaign.
- Settings: default send window, gaps, footer, poll interval, data folder location (read-only display).
- **Backup and restore**: export/import the SQLite database as one file (credentials excluded — user re-connects accounts after restore).
- Plain, clean UI; light and dark theme following the OS.
- Single instance only (`tauri-plugin-single-instance`) so two copies never send in parallel.

---

## 6. Technical design

### 6.1 Rust crates (suggested — check current versions)
- `tauri` 2, `tauri-plugin-single-instance`, `tauri-plugin-dialog`, `tauri-plugin-opener`, `tauri-plugin-notification`, `tauri-plugin-window-state`
- `tokio`, `serde`, `serde_json`, `thiserror`, `tracing`
- Storage: `rusqlite` (feature `bundled`) with simple embedded, versioned migrations
- Gmail: `async-imap` (tokio + rustls), `lettre` (tokio + rustls) for SMTP
- Microsoft: `reqwest` (rustls) for Graph and the token endpoint; PKCE/loopback done by hand or with the `oauth2` crate
- Parsing incoming mail: `mail-parser`
- Import: `csv`, `calamine` (xlsx)
- Other: `keyring`, `chrono` + `chrono-tz`, `uuid`, `rand`, `email_address`

### 6.2 Data (SQLite, in the Tauri app data directory)
Tables (names indicative):
- `accounts` — id, provider (gmail/microsoft), email, display name, type, daily limit, status, signature. No secrets.
- `contacts` — id, email (unique, normalised), first/last name, custom fields (JSON), created/updated.
- `lists`, `list_members`, `tags`, `contact_tags`
- `do_not_contact` — email, reason (manual / unsubscribe reply / bounce / import), added_at.
- `campaigns` — id, name, account_id, list snapshot, status, send window, gap settings, created/started.
- `campaign_steps` — campaign_id, step number, delay days, subject, body, same_thread flag.
- `enrolments` — campaign_id, contact_id, current step, status (`queued`, `waiting`, `sending`, `replied`, `bounced`, `unsubscribed`, `finished`, `failed`, `skipped`), next_due_at, stop reason.
- `outbound` — enrolment, step, account, our Message-ID, provider ids (Gmail thread id / Graph message id, internetMessageId, conversationId), status, attempted_at, sent_at, error.
- `inbound` — account, provider id, from, subject, received_at, classification (reply / auto_reply / bounce / unsubscribe / unrelated), matched outbound id, snippet.
- `send_log` — per-account sends with timestamps (for the rolling 24-hour limit).
- `settings`, `schema_version`.

### 6.3 Structure
- `src-tauri/src/` — `db/`, `providers/` (a common `MailProvider` trait with `gmail.rs` and `microsoft.rs`), `scheduler.rs`, `classify.rs` (reply/bounce/auto-reply/unsubscribe), `templates.rs`, `import.rs`, `commands.rs` (Tauri commands the UI calls).
- `src/` — Svelte UI: Accounts, Contacts, Campaigns (editor + launch checklist), Inbox, Dashboard, Settings.
- Keep logic in Rust and testable without the UI or network; the UI only calls commands.

### 6.4 Builds and releases
- Copy and adapt `..\markdown-viewer\.github\workflows\release.yml`: Windows (NSIS installer) + macOS `aarch64` and `x86_64` (`.dmg`); drop Linux. Draft release on version tag.
- Unsigned. The README tells users how to get past the warnings:
  - Windows: "Windows protected your PC" → **More info → Run anyway**.
  - Mac: open once, then **System Settings → Privacy & Security → Open Anyway**.
- Needs a GitHub repo (Tim's account `TimWJT`). Ask Tim before creating it or pushing.
- Auto-update (`tauri-plugin-updater`, as in `markdown-viewer`) is a later milestone.

---

## 7. Rules for whoever implements this

1. **Never send real email from automated tests.** Providers are behind the `MailProvider` trait; tests use a fake.
2. Build a **dry-run mode** (on by default in dev builds): "sending" writes the full rendered email to a local `outbox-dry-run/` folder instead. Real sending must be a deliberate switch.
3. Manual testing against real accounts uses **Tim's own test accounts sending to his own addresses** only — never a real contact list.
4. Never store or log passwords, tokens or email bodies in logs. Logs may contain ids and statuses.
5. Never send the same step to the same contact twice (see 5.5 idempotency). This must have tests.
6. Never email anyone on the do-not-contact list. This must have tests.
7. Unit-test with fixtures and a fake clock: template rendering and fallbacks, send-window and due-date maths, daily limits, catch-up behaviour, reply/auto-reply/bounce/unsubscribe classification (use real-world raw `.eml` samples from Gmail and Outlook), CSV/xlsx import.
8. Plain English in the UI. Short labels, clear error messages that say what to do next.

---

## 8. Things to verify early (risks)

1. **Graph message ids after sending.** A draft's id can change when it moves to Sent Items. Plan: create the draft (`POST /me/messages` or `/createReply`), record `internetMessageId` + `conversationId`, send it (`POST /me/messages/{id}/send`), then find the sent copy by `internetMessageId`. Prove this works for both personal and work accounts before building on it.
2. **Gmail app passwords** still available and IMAP/SMTP working with them (Google has been tightening this). If they are retired, the fallback is "Sign in with Google" with its 100-user cap — ask Tim before switching.
3. **Thread matching** for follow-ups in Gmail via SMTP (correct `References`/`In-Reply-To` + `Re:` subject) — confirm follow-ups appear in the same thread for the recipient on both Gmail and Outlook.
4. **Outlook.com limits for new accounts** may be well below 300/day; make sure "limit reached" errors are recognised and handled (pause account, no retry storm).
5. **Mac build** compiles in CI and the app opens on a real Mac (Tim will ask testers once Windows works).

---

## 9. Milestones

Build in this order. Each milestone ends working and tested before the next starts.

1. **Scaffold** — Tauri 2 + Svelte + TS app opens on Windows; SQLite with migrations; single instance; dry-run mode switch; CI workflow builds Windows and Mac.
2. **Accounts** — Gmail app-password connect + test; Microsoft sign-in + test; credentials in keyring; account list UI; Microsoft registration doc.
3. **Contacts** — CSV/xlsx import with column mapping and summary; validation; de-duplication; lists/tags; do-not-contact list.
4. **Templates and single send** — editor, merge fields with fallbacks, preview, signature, footer, send test to myself (real and dry-run).
5. **Campaigns and scheduler** — steps, send window, gaps, daily limits, launch checklist with volume warning, pause/resume, catch-up, idempotent sending, same-thread follow-ups. Resolve risks 1 and 3 here.
6. **Detection** — polling, reply / auto-reply / bounce / unsubscribe classification, stop rules, notifications.
7. **Inbox and dashboard** — unified inbox, campaign stats, contact timeline, CSV export, backup/restore.
8. **Release** — installers from CI, README with install and setup instructions (including the Windows/Mac warnings and Gmail app-password steps), version 1.0.0 draft release.
