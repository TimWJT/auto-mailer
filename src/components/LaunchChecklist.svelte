<script lang="ts">
  import { api, errorText } from '../lib/api';
  import { ui, go, toast } from '../lib/state.svelte';
  import type { Checklist } from '../lib/types';
  import Modal from './Modal.svelte';

  let { campaignId, onclose }: { campaignId: number; onclose: () => void } = $props();

  let check = $state<Checklist | null>(null);
  let acknowledged = $state(false);
  let busy = $state(false);

  $effect(() => {
    api
      .checklist(campaignId)
      .then((c) => (check = c))
      .catch((e) => toast(errorText(e), 'error'));
  });

  async function launch() {
    busy = true;
    try {
      await api.launch(campaignId);
      toast('Campaign started.');
      go({ name: 'campaign', id: campaignId });
    } catch (e) {
      toast(errorText(e), 'error');
    } finally {
      busy = false;
    }
  }

  const canLaunch = $derived(!!check && check.problems.length === 0 && (!check.volume_warning || acknowledged));
</script>

<Modal title="Ready to start?" {onclose} wide>
  {#if !check}
    <p class="muted">Checking…</p>
  {:else}
    {#each check.problems as p}<div class="notice bad">{p}</div>{/each}
    {#if ui.info?.dry_run}
      <div class="notice warn">
        <strong>Dry-run mode is on.</strong> This campaign will only write emails to a folder. Contacts will still be marked as
        emailed, so use this campaign to rehearse, then make a new one for real sending.
      </div>
    {/if}

    <div class="grid4" style="margin-bottom: 14px">
      <div class="stat"><div class="n">{check.will_send}</div><div class="l">will be emailed</div></div>
      <div class="stat"><div class="n">{check.skipped_dnc + check.skipped_invalid + check.skipped_missing.length}</div><div class="l">skipped</div></div>
      <div class="stat"><div class="n">~{check.per_day}</div><div class="l">emails per sending day</div></div>
      <div class="stat"><div class="n">~{check.est_total_days}</div><div class="l">days until the last follow-up</div></div>
    </div>

    <ul class="facts">
      <li>From <strong>{check.account_email ?? '—'}</strong>, {check.steps} email{check.steps === 1 ? '' : 's'} per contact.</li>
      <li>{check.contacts_total} contacts on the list.
        {#if check.skipped_dnc}{check.skipped_dnc} on the do-not-contact list.{/if}
        {#if check.skipped_invalid}{check.skipped_invalid} with an invalid address.{/if}</li>
      <li>First emails go out over about {check.est_sending_days} sending day{check.est_sending_days === 1 ? '' : 's'}.</li>
      {#if check.active_elsewhere}
        <li class="warn-text">{check.active_elsewhere} contact{check.active_elsewhere === 1 ? ' is' : 's are'} also active in another campaign — they may get emails from both.</li>
      {/if}
      {#if check.has_attachments}<li class="warn-text">This campaign has attachments, which make cold emails more likely to go to spam.</li>{/if}
    </ul>

    {#if check.skipped_missing.length}
      <details>
        <summary>{check.skipped_missing.length} skipped because a merge field is blank (and has no fallback)</summary>
        <table class="small">
          <tbody>
            {#each check.skipped_missing.slice(0, 50) as m}<tr><td>{m.email}</td><td class="mono">{m.fields.join(', ')}</td></tr>{/each}
          </tbody>
        </table>
        <p class="small muted">Add a fallback like <span class="mono">{'{{first_name|there}}'}</span> or fill in those contacts to include them.</p>
      </details>
    {/if}

    {#if check.preview}
      <h3 style="margin-top: 14px">First email, as {check.preview.contact_email} will see it</h3>
      <pre class="email-preview"><strong>Subject: {check.preview.email.subject}</strong>

{check.preview.email.body}</pre>
    {/if}

    {#if check.volume_warning}
      <div class="notice warn" style="margin-top: 14px">
        <strong>About sending volume.</strong> {check.volume_warning}
        <label class="row" style="margin-top: 8px"><input type="checkbox" bind:checked={acknowledged} /> I understand the risk</label>
      </div>
    {/if}

    <div class="row" style="margin-top: 14px">
      <button class="primary" onclick={launch} disabled={!canLaunch || busy}>{busy ? 'Starting…' : `Start sending to ${check.will_send} contacts`}</button>
      <button onclick={onclose}>Not yet</button>
    </div>
    <p class="small muted" style="margin-top: 8px">Emails only go out while Auto Mailer is open (minimised is fine), inside the send window, with a random gap between each.</p>
  {/if}
</Modal>

<style>
  .facts { padding-left: 18px; margin: 0 0 10px; }
  .facts li { margin-bottom: 4px; }
  .warn-text { color: var(--warn); }
  details { margin: 8px 0; }
  summary { cursor: pointer; }
</style>
