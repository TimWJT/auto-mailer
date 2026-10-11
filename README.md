# Auto Mailer

A free, unofficial desktop app for **Windows and Mac** that sends personalised plain-text email campaigns with automatic follow-ups, and stops following up as soon as someone replies.

- You connect **your own** Gmail or Microsoft accounts.
- Everything runs and is stored **on your computer**. There's no server, no sign-up, and no account with us.
- It only sends **while the app is open** (minimised is fine). When you reopen it, anything overdue goes out gradually, never in a burst.

## Install

Download the right file from the [latest release](https://github.com/TimWJT/auto-mailer/releases/latest):

| Your computer | File |
|---|---|
| Windows | `…_x64-setup.exe` |
| Mac with Apple silicon (M1–M4) | `…_aarch64.dmg` |
| Mac with Intel | `…_x64.dmg` |

The app isn't code-signed (that costs money), so your computer warns you the first time:

- **Windows:** "Windows protected your PC" → click **More info** → **Run anyway**.
- **Mac:** open the app once (it gets blocked), then go to **System Settings → Privacy & Security**, scroll down and click **Open Anyway**.

## Connect an account

### Gmail (app password)

Auto Mailer uses a Gmail **app password**, not "Sign in with Google".

1. Turn on **2-Step Verification**: <https://myaccount.google.com/signinoptions/twosv>
2. Create an **app password** (name it "Auto Mailer"): <https://myaccount.google.com/apppasswords>
3. In Auto Mailer, open **Accounts → Add Gmail**, paste your address and the 16-letter code, and click **Test connection and add**.

Some work/school Google accounts and Advanced Protection accounts don't allow app passwords. The app tells you if that's the problem.

### Microsoft (Outlook.com, Hotmail, Live, Microsoft 365)

Click **Accounts → Sign in with Microsoft** and finish in your browser. Work or school accounts may need their IT admin to approve the app first.

> Builders: this button needs a one-time app registration. See [docs/microsoft-app-registration.md](docs/microsoft-app-registration.md).

Passwords and sign-in tokens are kept in your system's secure store (Windows Credential Manager / macOS Keychain). They are never written to Auto Mailer's files or logs.

## How it works

1. **Contacts:** import a CSV or Excel file. You match its columns (email, first name, company…), and everything except the email becomes a merge field. Duplicates and invalid addresses are flagged, not silently dropped.
2. **Campaign:** choose an account and a list, then write a first email and follow-ups ("if no reply after 3 days"). Use merge fields like `{{first_name}}`, with a fallback like `{{first_name|there}}`. A contact with a blank field and no fallback is skipped and shown in the preview.
3. **Launch checklist:** see who gets emailed, who's skipped and why, roughly how long it will take, and the first email as it will look.
4. **Sending:** inside your send window (default Mon–Fri 9:00–17:00, your local time), one email at a time with a random gap (default 45–120 s), keeping under each account's daily limit.
5. **Replies:** every 5 minutes the app checks each inbox.
   - A real reply (or someone else replying in the thread) stops that contact's sequence.
   - A bounce stops it and adds the address to the do-not-contact list.
   - A "stop" / "unsubscribe" reply does the same.
   - Out-of-office auto-replies are recorded but **don't** stop the sequence.

### Daily limits

Each account defaults to just under its provider's own limit (rolling 24 hours):

| Account | Provider limit (approx.) | Default |
|---|---|---|
| Gmail (free) | 500/day | 450 |
| Google Workspace | 2,000/day | 1,800 |
| Outlook.com / Hotmail / Live | 300/day (lower for new accounts) | 280 |
| Microsoft 365 | 10,000/day, 30/min | 9,000 |

If a provider says the limit has been reached, that account pauses until the 24-hour window clears.

**High volume is risky.** Lots of email from one account, especially cold outreach from a new account, can get it flagged, suspended or sent to spam. Start low and increase gradually. The app warns you, but the decision is yours.

### Dry-run mode

In **Settings**, dry-run mode writes every "sent" email to a folder on your computer instead of sending it. It's on by default in development builds. Real sending is a deliberate switch.

### Your responsibilities

Follow the anti-spam law where you live: use honest sender details and honour opt-outs. Every email gets an opt-out line by default ("If you'd rather not hear from me, just reply 'stop'"), and "stop" replies are handled automatically.

---

## Development

Prerequisites: [Rust](https://rustup.rs), Node.js 22+, and the [Tauri prerequisites](https://tauri.app/start/prerequisites/) for your OS.

```sh
npm install
npm run app:dev      # run the app with hot reload (dry-run on by default)
npm test             # type-check the UI + Rust unit tests
npm run app:build    # release build + installer for this OS
```

Rules for working on it (from the build plan):

- Automated tests **never send real email**. Providers sit behind the `MailProvider` trait and tests use a fake.
- Manual testing against real accounts: use your own test accounts and send to your own addresses only.
- Never log passwords, tokens or email bodies.

### Layout

```
src-tauri/src/
  db/            SQLite + versioned migrations
  providers/     MailProvider trait; gmail.rs (IMAP+SMTP), microsoft.rs (Graph), dryrun.rs, fake.rs (tests)
  scheduler.rs   sending queue (idempotent), reconciliation, reply detection + stop rules
  classify.rs    reply / auto-reply / bounce / unsubscribe classification
  templates.rs   merge fields and fallbacks
  window.rs      send windows and due-date maths
  import.rs      CSV / xlsx reading and column mapping
  accounts.rs, contacts.rs, campaigns.rs, inbox.rs   data access
  oauth.rs       Microsoft sign-in (PKCE + loopback)
  secrets.rs     OS keychain
  commands.rs    Tauri commands the UI calls
  tests/fixtures/  raw .eml samples used by the classifier tests
src/             Svelte 5 + TypeScript UI
```

### Releasing

Push a tag like `v1.0.0`. GitHub Actions builds the Windows installer and both Mac `.dmg` files and attaches them to a **draft** release for you to review and publish. Set the repository variable `AUTO_MAILER_MS_CLIENT_ID` first so Microsoft sign-in works in those builds.

### Still to verify by hand (plan §8)

- **Graph message ids:** after sending, the copy in Sent Items is found by `internetMessageId`, and follow-ups use `createReply`. Check this on a personal and a work account (see the Microsoft doc).
- **Gmail app passwords:** IMAP/SMTP login with an app password still works.
- **Threading:** Gmail follow-ups (`In-Reply-To` / `References` + `Re:`) appear in the same thread for recipients on both Gmail and Outlook.
- **Outlook.com limits** for new accounts: "limit reached" errors pause the account with no retry storm.
- **Mac build:** opens on a real Mac.

## Licence

[PolyForm Noncommercial 1.0.0](LICENSE)
