<script lang="ts">
  import { ask } from '@tauri-apps/plugin-dialog';
  import { openUrl } from '@tauri-apps/plugin-opener';
  import { api, errorText } from '../lib/api';
  import { ui, toast } from '../lib/state.svelte';
  import { when } from '../lib/format';
  import type { Account } from '../lib/types';
  import Modal from '../components/Modal.svelte';

  let { onconnected }: { onconnected?: () => void } = $props();

  let accounts = $state<Account[]>([]);
  let loading = $state(true);
  let showGmail = $state(false);
  let gmail = $state({ email: '', password: '', name: '' });
  let busy = $state(false);
  let editing = $state<Account | null>(null);
  let edit = $state({ display_name: '', signature: '', daily_limit: 0, account_type: '', paused: false });
  let reconnect = $state<Account | null>(null);
  let newPassword = $state('');

  async function load() {
    try {
      accounts = await api.listAccounts();
    } catch (e) {
      toast(errorText(e), 'error');
    } finally {
      loading = false;
    }
  }

  $effect(() => {
    ui.version;
    load();
  });

  function typeLabel(id: string) {
    return ui.info?.account_types.find((t) => t[0] === id)?.[1] ?? id;
  }
  function providerLimit(id: string) {
    return ui.info?.account_types.find((t) => t[0] === id)?.[2] ?? 0;
  }

  async function addGmail(e: Event) {
    e.preventDefault();
    busy = true;
    try {
      const a = await api.addGmail(gmail.email, gmail.password, gmail.name);
      toast(`Connected ${a.email}`);
      showGmail = false;
      gmail = { email: '', password: '', name: '' };
      await load();
      onconnected?.();
    } catch (err) {
      toast(errorText(err), 'error');
    } finally {
      busy = false;
    }
  }

  async function signInMicrosoft() {
    busy = true;
    try {
      toast('Finish signing in in your browser…');
      const a = await api.signInMicrosoft();
      toast(`Connected ${a.email}`);
      await load();
      onconnected?.();
    } catch (err) {
      toast(errorText(err), 'error');
    } finally {
      busy = false;
    }
  }

  async function test(a: Account) {
    busy = true;
    try {
      const r = await api.testAccount(a.id);
      if (r.status === 'connected') toast(`${a.email} is working.`);
      else toast(r.status_message ?? 'This account needs attention.', 'error');
      await load();
    } catch (err) {
      toast(errorText(err), 'error');
    } finally {
      busy = false;
    }
  }

  function openEdit(a: Account) {
    editing = a;
    edit = { display_name: a.display_name, signature: a.signature, daily_limit: a.daily_limit, account_type: a.account_type, paused: a.paused };
  }

  async function saveEdit() {
    if (!editing) return;
    const raising = edit.daily_limit > editing.daily_limit && edit.daily_limit > (ui.info?.volume_warning_per_day ?? 100);
    if (raising) {
      const ok = await ask(ui.info?.volume_warning ?? '', { title: 'Raise the daily limit?', kind: 'warning', okLabel: 'Raise it', cancelLabel: 'Cancel' });
      if (!ok) return;
    }
    const limit = providerLimit(edit.account_type);
    if (limit && edit.daily_limit >= limit) {
      const ok = await ask(
        `That's at or above the provider's own limit of about ${limit} a day. If the provider blocks the account, Auto Mailer pauses it for 24 hours.`,
        { title: 'Above the provider limit', kind: 'warning', okLabel: 'Keep it', cancelLabel: 'Cancel' },
      );
      if (!ok) return;
    }
    try {
      await api.updateAccount(editing.id, edit);
      editing = null;
      toast('Saved.');
      await load();
    } catch (err) {
      toast(errorText(err), 'error');
    }
  }

  async function remove(a: Account) {
    const ok = await ask(`Remove ${a.email}? Its saved password or sign-in is deleted from this computer. Past campaign history is kept.`, {
      title: 'Remove account',
      kind: 'warning',
      okLabel: 'Remove',
    });
    if (!ok) return;
    try {
      await api.removeAccount(a.id);
      toast('Removed.');
      await load();
    } catch (err) {
      toast(errorText(err), 'error');
    }
  }

  async function doReconnect(e: Event) {
    e.preventDefault();
    if (!reconnect) return;
    busy = true;
    try {
      await api.updateGmailPassword(reconnect.id, newPassword);
      toast('Reconnected.');
      reconnect = null;
      newPassword = '';
      await load();
    } catch (err) {
      toast(errorText(err), 'error');
    } finally {
      busy = false;
    }
  }

  function statusBadge(a: Account): [string, string] {
    if (a.status !== 'connected') return ['bad', 'Needs attention'];
    if (a.paused) return ['warn', 'Paused'];
    if (a.limit_paused_until && new Date(a.limit_paused_until) > new Date()) return ['warn', 'Limit reached'];
    return ['ok', 'Connected'];
  }
</script>

<div class="row between">
  <div>
    <h1>Accounts</h1>
    <p class="muted">The email accounts campaigns send from. Each one is used separately and keeps under its own daily limit.</p>
  </div>
  <div class="row">
    <button class="primary" onclick={() => (showGmail = true)} disabled={busy}>Add Gmail</button>
    <button class="primary" onclick={signInMicrosoft} disabled={busy || !ui.info?.microsoft_configured}
      title={ui.info?.microsoft_configured ? '' : 'Microsoft sign-in is not set up in this build'}>Sign in with Microsoft</button>
  </div>
</div>

{#if ui.info && !ui.info.microsoft_configured}
  <p class="small muted">Microsoft sign-in isn't set up in this build yet (it needs an app registration — see docs/microsoft-app-registration.md). Gmail works.</p>
{/if}

{#if loading}
  <p class="muted">Loading…</p>
{:else if accounts.length === 0}
  <div class="panel empty">No accounts yet. Add a Gmail account or sign in with Microsoft to start.</div>
{:else}
  {#each accounts as a (a.id)}
    {@const [cls, label] = statusBadge(a)}
    <div class="panel">
      <div class="row between">
        <div>
          <div class="row"><strong>{a.email}</strong> <span class="badge {cls}">{label}</span></div>
          <div class="muted small">{a.provider === 'gmail' ? 'Gmail (app password)' : 'Microsoft'} · {typeLabel(a.account_type)} · connected {when(a.connected_at)}</div>
        </div>
        <div class="row">
          <button onclick={() => test(a)} disabled={busy}>Test connection</button>
          {#if a.provider === 'gmail'}
            <button onclick={() => (reconnect = a)}>New app password</button>
          {:else}
            <button onclick={signInMicrosoft} disabled={busy || !ui.info?.microsoft_configured}>Sign in again</button>
          {/if}
          <button onclick={() => openEdit(a)}>Edit</button>
          <button class="danger" onclick={() => remove(a)}>Remove</button>
        </div>
      </div>
      {#if a.status !== 'connected' && a.status_message}
        <div class="notice bad" style="margin: 12px 0 0">{a.status_message}</div>
      {/if}
      {#if a.limit_paused_until && new Date(a.limit_paused_until) > new Date()}
        <div class="notice warn" style="margin: 12px 0 0">The provider said this account hit its sending limit. Sending resumes {when(a.limit_paused_until)}.</div>
      {/if}
      <div class="row small" style="margin-top: 10px; gap: 20px">
        <span>Sent in the last 24 hours: <strong>{a.sent_24h}</strong> of {a.daily_limit}</span>
        <span class="muted">Last checked for replies: {when(a.last_polled_at)}</span>
      </div>
    </div>
  {/each}
{/if}

{#if showGmail}
  <Modal title="Add a Gmail account" onclose={() => (showGmail = false)}>
    <ol class="steps">
      <li>Turn on <strong>2-Step Verification</strong> for your Google account.
        <button class="link" onclick={() => openUrl('https://myaccount.google.com/signinoptions/twosv')}>Open Google settings</button></li>
      <li>Create an <strong>app password</strong> (name it "Auto Mailer"). Google shows a 16-letter code.
        <button class="link" onclick={() => openUrl('https://myaccount.google.com/apppasswords')}>Open app passwords</button></li>
      <li>Paste your email address and that code below.</li>
    </ol>
    <form onsubmit={addGmail}>
      <label class="field"><span class="label">Gmail address</span>
        <input type="email" bind:value={gmail.email} placeholder="you@gmail.com" required /></label>
      <label class="field"><span class="label">App password</span>
        <input type="password" bind:value={gmail.password} placeholder="abcd efgh ijkl mnop" required autocomplete="off" />
        <div class="hint">Stored in your computer's secure password store, never in Auto Mailer's files.</div></label>
      <label class="field"><span class="label">Your name (shown to recipients)</span>
        <input bind:value={gmail.name} placeholder="Tim Wang" /></label>
      <div class="row">
        <button class="primary" type="submit" disabled={busy}>{busy ? 'Testing…' : 'Test connection and add'}</button>
        <button type="button" onclick={() => (showGmail = false)}>Cancel</button>
      </div>
    </form>
  </Modal>
{/if}

{#if reconnect}
  <Modal title="New app password for {reconnect.email}" onclose={() => (reconnect = null)}>
    <form onsubmit={doReconnect}>
      <p class="muted">Create a new app password at
        <button class="link" type="button" onclick={() => openUrl('https://myaccount.google.com/apppasswords')}>myaccount.google.com/apppasswords</button> and paste it here.</p>
      <label class="field"><span class="label">App password</span>
        <input type="password" bind:value={newPassword} required autocomplete="off" /></label>
      <button class="primary" type="submit" disabled={busy}>{busy ? 'Testing…' : 'Test and save'}</button>
    </form>
  </Modal>
{/if}

{#if editing}
  <Modal title="Edit {editing.email}" onclose={() => (editing = null)}>
    <label class="field"><span class="label">Your name (shown to recipients)</span>
      <input bind:value={edit.display_name} /></label>
    <label class="field"><span class="label">Signature</span>
      <textarea rows="4" bind:value={edit.signature} placeholder={'Tim Wang\nAcme Ltd'}></textarea>
      <div class="hint">Plain text, added under every email from this account.</div></label>
    <div class="grid2">
      <label class="field"><span class="label">Account type</span>
        <select bind:value={edit.account_type}>
          {#each ui.info?.account_types ?? [] as t}<option value={t[0]}>{t[1]}</option>{/each}
        </select></label>
      <label class="field"><span class="label">Daily limit (rolling 24 hours)</span>
        <input type="number" min="1" bind:value={edit.daily_limit} />
        <div class="hint">Provider limit is about {providerLimit(edit.account_type)}/day.</div></label>
    </div>
    <label class="field row"><input type="checkbox" bind:checked={edit.paused} /> Pause all sending from this account</label>
    <div class="row">
      <button class="primary" onclick={saveEdit}>Save</button>
      <button onclick={() => (editing = null)}>Cancel</button>
    </div>
  </Modal>
{/if}

<style>
  .steps { padding-left: 20px; margin: 0 0 16px; }
  .steps li { margin-bottom: 6px; }
</style>
