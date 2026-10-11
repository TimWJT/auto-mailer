<script lang="ts">
  import { api, errorText } from '../lib/api';
  import { ui, go, toast } from '../lib/state.svelte';
  import Accounts from './Accounts.svelte';

  let step = $state(1);

  async function finish(next: 'contacts' | 'campaign-edit' | 'dashboard') {
    try {
      const s = await api.getSettings();
      await api.saveSettings({ ...s, welcome_done: true });
      ui.info = await api.appInfo();
    } catch (e) {
      toast(errorText(e), 'error');
    }
    go(next === 'campaign-edit' ? { name: 'campaign-edit', id: null } : { name: next });
  }
</script>

<div class="welcome">
  <h1>Welcome to Auto Mailer</h1>
  <p class="muted">Send personal-looking emails to a list, follow up automatically, and stop as soon as someone replies. Everything stays on this computer.</p>

  <div class="steps row">
    <span class:on={step === 1}>1. Connect an account</span>
    <span class:on={step === 2}>2. Import contacts</span>
    <span class:on={step === 3}>3. Create a campaign</span>
  </div>

  {#if step === 1}
    <Accounts onconnected={() => (step = 2)} />
    <div class="row" style="margin-top: 16px">
      <button onclick={() => (step = 2)}>Skip for now</button>
    </div>
  {:else if step === 2}
    <div class="panel">
      <h2>Import your contacts</h2>
      <p>Bring in a CSV or Excel file. You'll match its columns (email, first name, company…) and can put everyone on a list for your campaign.</p>
      <div class="row">
        <button class="primary" onclick={() => finish('contacts')}>Go to Contacts</button>
        <button onclick={() => (step = 3)}>Skip</button>
      </div>
    </div>
  {:else}
    <div class="panel">
      <h2>Create a campaign</h2>
      <p>Write a first email and one or two follow-ups. You'll see a preview for each contact and a checklist before anything is sent.</p>
      {#if ui.info?.dry_run}<p class="notice warn">Dry-run mode is on, so nothing will actually be sent until you switch it off in Settings.</p>{/if}
      <div class="row">
        <button class="primary" onclick={() => finish('campaign-edit')}>New campaign</button>
        <button onclick={() => finish('dashboard')}>Go to the dashboard</button>
      </div>
    </div>
  {/if}
</div>

<style>
  .welcome { max-width: 900px; }
  .steps { margin: 16px 0 20px; gap: 16px; }
  .steps span { color: var(--muted); }
  .steps span.on { color: var(--accent); font-weight: 600; }
</style>
