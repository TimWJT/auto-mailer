<script lang="ts">
  import { api, errorText } from '../lib/api';
  import { ui, go, toast } from '../lib/state.svelte';
  import { pct, campaignStatusLabel, when } from '../lib/format';
  import type { CampaignSummary } from '../lib/types';

  let campaigns = $state<CampaignSummary[]>([]);
  let loaded = $state(false);

  $effect(() => {
    ui.version;
    api
      .listCampaigns()
      .then((c) => {
        campaigns = c;
        loaded = true;
      })
      .catch((e) => toast(errorText(e), 'error'));
  });

  const badge: Record<string, string> = { running: 'ok', paused: 'warn', draft: '', stopped: 'bad', finished: 'accent' };
</script>

<div class="row between">
  <div>
    <h1>Campaigns</h1>
    <p class="muted">A campaign sends a first email to a list, then follow-ups to anyone who hasn't replied.</p>
  </div>
  <button class="primary" onclick={() => go({ name: 'campaign-edit', id: null })}>New campaign</button>
</div>

{#if loaded && campaigns.length === 0}
  <div class="panel empty">No campaigns yet.</div>
{:else if campaigns.length}
  <div class="panel" style="padding: 0">
    <table>
      <thead>
        <tr><th>Campaign</th><th>Status</th><th>From</th><th>List</th><th>Sent</th><th>Replied</th><th>Waiting</th></tr>
      </thead>
      <tbody>
        {#each campaigns as c (c.id)}
          <tr class="clickable" onclick={() => go(c.status === 'draft' ? { name: 'campaign-edit', id: c.id } : { name: 'campaign', id: c.id })}>
            <td><strong>{c.name}</strong><div class="muted small">{c.steps} email{c.steps === 1 ? '' : 's'} · created {when(c.created_at)}</div></td>
            <td><span class="badge {badge[c.status]}">{campaignStatusLabel(c.status)}</span></td>
            <td>{c.account_email ?? '—'}</td>
            <td>{c.list_name ?? '—'}</td>
            <td>{c.stats.emails_sent}</td>
            <td>{c.stats.replied + c.stats.unsubscribed} {#if c.stats.contacted}<span class="muted">({pct(c.stats.reply_rate)})</span>{/if}</td>
            <td>{c.stats.waiting}</td>
          </tr>
        {/each}
      </tbody>
    </table>
  </div>
{/if}
