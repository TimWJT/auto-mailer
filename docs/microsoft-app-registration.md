# Microsoft app registration (one-time setup)

"Sign in with Microsoft" needs an app registration in Microsoft Entra (Azure). It's free and you do it once. The result is a **client ID**. It is *not* a secret, so it's fine to put it in the source code or in a public CI variable.

You may be asked to create a free Azure account first. You don't need a paid subscription.

## Steps

1. Go to <https://entra.microsoft.com> (or <https://portal.azure.com>) and sign in with your Microsoft account.
2. Open **Applications → App registrations → New registration**.
3. Fill in:
   - **Name:** `Auto Mailer`
   - **Supported account types:** **Accounts in any organizational directory and personal Microsoft accounts** (multitenant + personal)
   - **Redirect URI:** platform **Public client/native (mobile & desktop)**, value `http://localhost`
4. Click **Register**. Copy the **Application (client) ID** from the overview page.
5. Open **Authentication**:
   - Check that `http://localhost` is listed under **Mobile and desktop applications**. Microsoft allows any port on a localhost redirect, which Auto Mailer needs because it picks a free port each time.
   - Under **Advanced settings**, set **Allow public client flows** to **Yes**. Save.
6. Open **API permissions → Add a permission → Microsoft Graph → Delegated permissions** and add:
   - `offline_access`
   - `User.Read` (usually already there)
   - `Mail.Send`
   - `Mail.ReadWrite`

   (`openid` and `profile` are added automatically at sign-in.) Don't click "Grant admin consent". Each user consents for themselves when they sign in.
7. You don't need a client secret or a certificate. Auto Mailer is a public client and uses PKCE.

## Put the client ID into the app

Pick one:

- **CI builds (recommended):** in the GitHub repo, go to **Settings → Secrets and variables → Actions → Variables → New repository variable**. Name it `AUTO_MAILER_MS_CLIENT_ID` and paste the client ID as the value. Both workflows pass it to the build.
- **Local builds:** set the environment variable before building, e.g. in PowerShell:
  ```powershell
  $env:AUTO_MAILER_MS_CLIENT_ID = "00000000-0000-0000-0000-000000000000"
  npm run app:build
  ```
- **Hard-code it:** paste it into `MS_CLIENT_ID` in `src-tauri/src/config.rs`.

If there's no client ID, the app still works, but the "Sign in with Microsoft" button is disabled with a note.

## What users may see

- **Personal accounts (Outlook.com, Hotmail, Live):** a Microsoft consent screen listing the permissions. The app is unverified, so Microsoft may show the publisher as unverified. They click **Accept**.
- **Work or school accounts:** many organisations block apps that their IT admin hasn't approved. Auto Mailer then shows: *"Your organisation needs its IT admin to approve Auto Mailer…"*. The admin can approve it in their Entra admin centre (Enterprise applications → consent requests), or the user can use a different account.
- **Publisher verification** (getting rid of "unverified") needs a Microsoft Partner Network account. It's optional and out of scope for now.

## Things to check after registering (plan risk 1)

Using a test Microsoft account, sending to your own address:

1. Connect the account in Auto Mailer and click **Test connection**.
2. Make a campaign with two steps (follow-up delay 1 day), add yourself as the only contact, and use **Send test to myself** with dry-run **off**.
3. Run the real campaign to your own second address. Check that:
   - the first email arrives and appears in **Sent Items**;
   - the follow-up arrives **in the same conversation** (it's sent with Graph `createReply`, so this needs the Sent Items copy to have been found; the app logs a warning if it wasn't);
   - replying from the second address shows up in Auto Mailer's Inbox and stops the sequence.
4. Repeat with a work/school account if you have one.
