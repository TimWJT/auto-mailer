<script lang="ts">
  import { openUrl } from '@tauri-apps/plugin-opener';
  import { api, errorText } from '../lib/api';
  import { ui, go, toast } from '../lib/state.svelte';
  import { when, classLabel } from '../lib/format';
  import type { InboundDetail, InboundRow } from '../lib/types';

  let rows = $state<InboundRow[]>([]);
  let filter = $state<string>('');
  let openId = $state<number | null>(null);
  let detail = $state<InboundDetail | null>(null);
  let checking = $state(false);

  $effect(() => {
    ui.version;
    filter;
    api
      .listInbox({ classification: filter || null })
      .then((r) => (rows = r))
      .catch((e) => toast(errorText(e), 'error'));
  });

  async function openRow(r: InboundRow) {
    openId = r.id;
    try {
      detail = await api.getInbound(r.id);
    } catch (e) {
      toast(errorText(e), 'error');
    }
  }

  async function checkNow() {
    checking = true;
    try {
      const n = await api.checkRepliesNow();
      toast(n ? `${n} new response(s).` : 'Nothing new.');
    } catch (e) {
      toast(errorText(e), 'error');
    } finally {
      checking = false;
    }
  }

  const cls: Record<string, string> = { reply: 'ok', auto_reply: '', bounce: 'bad', unsubscribe: 'bad' };
</script>

<div class="row between">
  <div>
    <h1>Inbox</h1>
    <p class="muted">Responses to your campaigns from every account, newest first.</p>
  </div>
  <div class="row">
    <select bind:value={filter}>
      <option value="">All responses</option>
      <option value="reply">Replies</option>
      <option value="unsubscribe">Asked to stop</option>
      <option value="auto_reply">Auto-replies</option>
      <option value="bounce">Bounces</option>
    </select>
    <button onclick={checkNow} disabled={checking}>{checking ? 'Checking…' : 'Check now'}</button>
  </div>
</div>

<div class="layout">
  <div class="panel list" style="padding: 0">
    {#each rows as r (r.id)}
      <button class="item" class:open={openId === r.id} class:unread={!r.is_read} onclick={() => openRow(r)}>
        <div class="row between">
          <strong>{r.from_name || r.from_email}</strong>
          <span class="muted small">{when(r.received_at)}</span>
        </div>
        <div class="row small">
          <span class="badge {cls[r.classification]}">{classLabel(r.classification)}</span>
          <span class="muted">{r.campaign_name ?? ''}</span>
        </div>
        <div class="muted small snippet">{r.snippet || r.subject}</div>
      </button>
    {:else}
      <div class="empty">No responses yet. Auto Mailer checks every few minutes while it's open.</div>
    {/each}
  </div>

  <div class="panel reader">
    {#if detail}
      <div class="row between">
        <h2 style="margin: 0">{detail.row.subject || '(no subject)'}</h2>
        {#if detail.web_link}
          <button onclick={() => openUrl(detail!.web_link!)}>Open in {detail.row.account_email?.includes('gmail') ? 'Gmail' : 'web mail'}</button>
        {/if}
      </div>
      <p class="muted small">
        From {detail.row.from_name ? `${detail.row.from_name} <${detail.row.from_email}>` : detail.row.from_email}
        · to {detail.row.account_email} · {when(detail.row.received_at)}
      </p>
      <p class="small">
        <span class="badge {cls[detail.row.classification]}">{classLabel(detail.row.classification)}</span>
        {#if detail.row.classification === 'auto_reply'}<span class="muted">The sequence continues for this contact.</span>{/if}
        {#if detail.row.classification === 'unsubscribe'}<span class="muted">Added to your do-not-contact list.</span>{/if}
        {#if detail.row.classification === 'bounce'}<span class="muted">This address was added to your do-not-contact list.</span>{/if}
        {#if detail.row.contact_id}<button class="link" onclick={() => go({ name: 'contact', id: detail!.row.contact_id! })}>View contact</button>{/if}
      </p>
      <pre class="email-preview">{detail.body_text || detail.row.snippet}</pre>
      <p class="small muted" style="margin-top: 8px">To reply, open it in your email.</p>
    {:else}
      <div class="empty">Choose a message to read it.</div>
    {/if}
  </div>
</div>

<style>
  .layout { display: grid; grid-template-columns: minmax(260px, 2fr) 3fr; gap: 16px; align-items: start; }
  .list { max-height: 75vh; overflow: auto; }
  .item {
    display: block;
    width: 100%;
    text-align: left;
    border: none;
    border-bottom: 1px solid var(--border);
    border-radius: 0;
    padding: 10px 14px;
    background: none;
  }
  .item.open { background: var(--accent-soft); }
  .item.unread strong::before { content: '● '; color: var(--accent); }
  .snippet { overflow: hidden; text-overflow: ellipsis; white-space: nowrap; margin-top: 2px; }
  .reader { position: sticky; top: 16px; }
</style>
