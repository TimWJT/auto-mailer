<script lang="ts">
  import { ask, save } from '@tauri-apps/plugin-dialog';
  import { api, errorText } from '../lib/api';
  import { ui, go, toast } from '../lib/state.svelte';
  import { pct, when, enrolmentLabel, campaignStatusLabel, classLabel } from '../lib/format';
  import type { Campaign, EnrolmentRow, InboundRow, Stats } from '../lib/types';

  let { id }: { id: number } = $props();

  let campaign = $state<Campaign | null>(null);
  let stats = $state<Stats | null>(null);
  let rows = $state<EnrolmentRow[]>([]);
  let replies = $state<InboundRow[]>([]);
  let filter = $state<string>('');

  async function load() {
    try {
      [campaign, stats, rows, replies] = await Promise.all([
        api.getCampaign(id),
        api.campaignStats(id),
        api.campaignEnrolments(id, filter || null),
        api.listInbox({ campaign_id: id }),
      ]);
    } catch (e) {
      toast(errorText(e), 'error');
    }
  }

  $effect(() => {
    ui.version;
    filter;
    load();
  });

  async function setStatus(s: 'running' | 'paused' | 'stopped') {
    if (s === 'stopped') {
      const ok = await ask('Stop this campaign for good? Nobody else will be emailed. You can still see its results.', {
        title: 'Stop campaign',
        kind: 'warning',
        okLabel: 'Stop',
      });
      if (!ok) return;
    }
    try {
      await api.setCampaignStatus(id, s);
      toast(s === 'paused' ? 'Paused.' : s === 'running' ? 'Resumed. Anything overdue goes out gradually.' : 'Stopped.');
    } catch (e) {
      toast(errorText(e), 'error');
    }
  }

  async function exportCsv() {
    const path = await save({ defaultPath: `${campaign?.name ?? 'campaign'} results.csv`, filters: [{ name: 'CSV', extensions: ['csv'] }] });
    if (!path) return;
    try {
      const n = await api.exportCampaignCsv(id, path);
      toast(`Exported ${n} rows.`);
    } catch (e) {
      toast(errorText(e), 'error');
    }
  }

  async function addNew() {
    try {
      const n = await api.enrolNewMembers(id);
      toast(n ? `Added ${n} new contact(s) from the list.` : 'No new contacts on the list.');
    } catch (e) {
      toast(errorText(e), 'error');
    }
  }

  const badge: Record<string, string> = { running: 'ok', paused: 'warn', stopped: 'bad', finished: 'accent', draft: '' };
  const enrolBadge: Record<string, string> = { replied: 'ok', bounced: 'bad', unsubscribed: 'bad', failed: 'bad', skipped: 'warn', finished: '', queued: 'accent', waiting: 'accent', sending: 'accent' };
</script>

<button class="link" onclick={() => go({ name: 'campaigns' })}>← Campaigns</button>

{#if campaign && stats}
  <div class="row between" style="margin: 8px 0 16px">
    <div>
      <h1>{campaign.name} <span class="badge {badge[campaign.status]}">{campaignStatusLabel(campaign.status)}</span></h1>
      <p class="muted">{campaign.steps.length} email{campaign.steps.length === 1 ? '' : 's'} · started {when(campaign.started_at)}</p>
    </div>
    <div class="row">
      {#if campaign.status === 'running'}<button onclick={() => setStatus('paused')}>Pause</button>{/if}
      {#if campaign.status === 'paused'}<button class="primary" onclick={() => setStatus('running')}>Resume</button>{/if}
      {#if campaign.status === 'paused' || campaign.status === 'draft'}<button onclick={() => go({ name: 'campaign-edit', id })}>Edit emails</button>{/if}
      {#if campaign.status === 'running' || campaign.status === 'paused'}
        <button onclick={addNew} title="Add contacts that joined the list after the campaign started">Add new list contacts</button>
        <button class="danger" onclick={() => setStatus('stopped')}>Stop</button>
      {/if}
      <button onclick={exportCsv}>Export CSV</button>
    </div>
  </div>

  {#if stats.dry_run_sent}
    <div class="notice warn">
      {stats.dry_run_sent} of these emails were dry-run only: they were written to a folder, not really sent, but those contacts
      count as emailed in this campaign. To email them for real, turn off dry-run and use a new campaign.
    </div>
  {/if}

  <div class="grid4" style="margin-bottom: 16px">
    <div class="stat"><div class="n">{stats.emails_sent}</div><div class="l">emails sent to {stats.contacted} people</div></div>
    <div class="stat"><div class="n">{stats.replied + stats.unsubscribed} <span class="muted small">{pct(stats.reply_rate)}</span></div><div class="l">replied</div></div>
    <div class="stat"><div class="n">{stats.waiting}</div><div class="l">still in the sequence</div></div>
    <div class="stat"><div class="n">{stats.finished_no_reply}</div><div class="l">finished with no reply</div></div>
    <div class="stat"><div class="n">{stats.bounced}</div><div class="l">bounced</div></div>
    <div class="stat"><div class="n">{stats.unsubscribed}</div><div class="l">asked to stop</div></div>
    <div class="stat"><div class="n">{stats.auto_replies}</div><div class="l">auto-replies (sequence continues)</div></div>
    <div class="stat"><div class="n">{stats.failed + stats.skipped}</div><div class="l">failed or skipped</div></div>
  </div>

  {#if replies.length}
    <div class="panel">
      <h2>Responses</h2>
      {#each replies.slice(0, 10) as r}
        <div class="row between resp">
          <span><span class="badge {r.classification === 'reply' ? 'ok' : r.classification === 'auto_reply' ? '' : 'bad'}">{classLabel(r.classification)}</span>
            <strong>{r.from_name || r.from_email}</strong> <span class="muted">{r.snippet}</span></span>
          <span class="muted small">{when(r.received_at)}</span>
        </div>
      {/each}
      <button class="link" onclick={() => go({ name: 'inbox' })}>Open inbox</button>
    </div>
  {/if}

  <div class="panel" style="padding: 0">
    <div class="row between" style="padding: 12px 14px">
      <h2 style="margin: 0">Contacts</h2>
      <select bind:value={filter}>
        <option value="">Everyone ({stats.contacts})</option>
        {#each ['queued', 'waiting', 'replied', 'bounced', 'unsubscribed', 'finished', 'failed', 'skipped'] as s}
          <option value={s}>{enrolmentLabel(s)}</option>
        {/each}
      </select>
    </div>
    <table>
      <thead><tr><th>Contact</th><th>Status</th><th>Next email</th><th>Last sent</th><th>Note</th></tr></thead>
      <tbody>
        {#each rows as r (r.id)}
          <tr class="clickable" onclick={() => go({ name: 'contact', id: r.contact_id })}>
            <td>{r.email}<div class="muted small">{r.name}</div></td>
            <td><span class="badge {enrolBadge[r.status] ?? ''}">{enrolmentLabel(r.status)}</span></td>
            <td>{#if r.status === 'queued' || r.status === 'waiting'}Email {r.current_step} · {when(r.next_due_at)}{:else}—{/if}</td>
            <td>{when(r.last_sent_at)}</td>
            <td class="muted small">{r.stop_reason ?? ''}</td>
          </tr>
        {:else}
          <tr><td colspan="5" class="muted">Nobody here.</td></tr>
        {/each}
      </tbody>
    </table>
  </div>
{/if}

<style>
  .resp { padding: 6px 0; border-bottom: 1px solid var(--border); }
</style>
