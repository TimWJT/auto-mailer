<script lang="ts">
  import { api, errorText } from '../lib/api';
  import { ui, go, toast } from '../lib/state.svelte';
  import { when, pct, classLabel } from '../lib/format';
  import type { Account, CampaignSummary, InboundRow, QueueOverview } from '../lib/types';

  let queue = $state<QueueOverview | null>(null);
  let campaigns = $state<CampaignSummary[]>([]);
  let accounts = $state<Account[]>([]);
  let recent = $state<InboundRow[]>([]);

  async function load() {
    try {
      [queue, campaigns, accounts, recent] = await Promise.all([
        api.queueOverview(),
        api.listCampaigns(),
        api.listAccounts(),
        api.listInbox({ classification: 'reply' }),
      ]);
    } catch (e) {
      toast(errorText(e), 'error');
    }
  }

  $effect(() => {
    ui.version;
    load();
  });

  // "Next send at" moves as time passes; refresh every half minute.
  $effect(() => {
    const t = setInterval(load, 30_000);
    return () => clearInterval(t);
  });

  const active = $derived(campaigns.filter((c) => c.status === 'running' || c.status === 'paused'));
  const attention = $derived(accounts.filter((a) => a.status !== 'connected'));
</script>

<h1>Dashboard</h1>
<p class="muted">Emails go out only while Auto Mailer is open. Minimising it is fine.</p>

{#each attention as a}
  <div class="notice bad">
    <strong>{a.email} needs attention.</strong> {a.status_message ?? ''} Sending from it is paused.
    <button class="link" onclick={() => go({ name: 'accounts' })}>Fix it</button>
  </div>
{/each}

{#if accounts.length === 0}
  <div class="panel">
    <h2>Get started</h2>
    <ol>
      <li><button class="link" onclick={() => go({ name: 'accounts' })}>Connect an email account</button></li>
      <li><button class="link" onclick={() => go({ name: 'contacts' })}>Import your contacts</button></li>
      <li><button class="link" onclick={() => go({ name: 'campaign-edit', id: null })}>Create a campaign</button></li>
    </ol>
  </div>
{/if}

{#if queue && queue.accounts.length}
  <div class="panel">
    <h2>Sending</h2>
    <table>
      <thead><tr><th>Account</th><th>Next send</th><th>Due now</th><th>Sent in last 24h</th></tr></thead>
      <tbody>
        {#each queue.accounts as q}
          <tr>
            <td>{q.email}</td>
            <td>
              {#if q.next_send_at}{when(q.next_send_at)}{:else}<span class="muted">Nothing scheduled</span>{/if}
              {#if q.blocked_reason}<span class="badge warn">{q.blocked_reason}</span>{/if}
            </td>
            <td>{q.due_now}</td>
            <td>{q.sent_24h} / {q.daily_limit}</td>
          </tr>
        {/each}
      </tbody>
    </table>
  </div>
{/if}

<div class="grid2">
  <div class="panel">
    <div class="row between"><h2>Active campaigns</h2><button class="link" onclick={() => go({ name: 'campaigns' })}>All campaigns</button></div>
    {#each active as c}
      <button class="line" onclick={() => go({ name: 'campaign', id: c.id })}>
        <div class="row between">
          <strong>{c.name}</strong>
          <span class="badge {c.status === 'running' ? 'ok' : 'warn'}">{c.status === 'running' ? 'Running' : 'Paused'}</span>
        </div>
        <div class="muted small">{c.stats.emails_sent} sent · {c.stats.replied + c.stats.unsubscribed} replied ({pct(c.stats.reply_rate)}) · {c.stats.waiting} waiting</div>
      </button>
    {:else}
      <p class="muted">No campaigns running.</p>
    {/each}
  </div>

  <div class="panel">
    <div class="row between"><h2>Latest replies</h2><button class="link" onclick={() => go({ name: 'inbox' })}>Inbox</button></div>
    {#each recent.slice(0, 6) as r}
      <button class="line" onclick={() => go({ name: 'inbox' })}>
        <div class="row between"><strong>{r.from_name || r.from_email}</strong><span class="muted small">{when(r.received_at)}</span></div>
        <div class="muted small snippet">{classLabel(r.classification)} · {r.snippet}</div>
      </button>
    {:else}
      <p class="muted">No replies yet.</p>
    {/each}
  </div>
</div>

{#if queue && queue.upcoming.length}
  <div class="panel">
    <h2>Coming up</h2>
    <table class="small">
      <thead><tr><th>Due</th><th>To</th><th>Campaign</th><th>Email</th></tr></thead>
      <tbody>
        {#each queue.upcoming as u}
          <tr><td>{when(u.due_at)}</td><td>{u.email}</td><td>{u.campaign}</td><td>{u.step_number === 1 ? 'First email' : `Follow-up ${u.step_number - 1}`}</td></tr>
        {/each}
      </tbody>
    </table>
    <p class="small muted">Due times are pushed into each campaign's send window and spaced out by the gap between emails.</p>
  </div>
{/if}

<style>
  .line { display: block; width: 100%; text-align: left; border: none; border-bottom: 1px solid var(--border); border-radius: 0; background: none; padding: 8px 0; }
  .line:last-child { border-bottom: none; }
  .snippet { overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
  ol { margin: 0; padding-left: 20px; }
  ol li { margin-bottom: 6px; }
</style>
