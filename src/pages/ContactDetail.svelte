<script lang="ts">
  import { ask } from '@tauri-apps/plugin-dialog';
  import { api, errorText } from '../lib/api';
  import { ui, go, toast } from '../lib/state.svelte';
  import { when, classLabel } from '../lib/format';
  import type { Contact, ContactList, TimelineItem } from '../lib/types';
  import ContactEditor from '../components/ContactEditor.svelte';

  let { id }: { id: number } = $props();

  let contact = $state<Contact | null>(null);
  let timeline = $state<TimelineItem[]>([]);
  let lists = $state<ContactList[]>([]);
  let editing = $state(false);

  $effect(() => {
    ui.version;
    Promise.all([api.getContact(id), api.contactTimeline(id), api.listLists()])
      .then(([c, t, l]) => {
        contact = c;
        timeline = t;
        lists = l;
      })
      .catch((e) => toast(errorText(e), 'error'));
  });

  async function toggleDnc() {
    if (!contact) return;
    if (contact.do_not_contact) {
      const ok = await ask(`Allow campaigns to email ${contact.email} again?`, { title: 'Remove from do-not-contact', kind: 'warning' });
      if (ok) await api.dncRemove(contact.email);
    } else {
      await api.dncAdd([contact.email]);
    }
  }

  async function remove() {
    if (!contact) return;
    const ok = await ask(`Delete ${contact.email} and their campaign history? This can't be undone.`, { title: 'Delete contact', kind: 'warning', okLabel: 'Delete' });
    if (!ok) return;
    await api.deleteContacts([contact.id]);
    go({ name: 'contacts' });
  }

  function statusText(t: TimelineItem) {
    if (t.direction === 'received') return classLabel(t.status);
    return { sent: 'Sent', sending: 'Sending', retry: 'Will retry', failed: 'Failed' }[t.status] ?? t.status;
  }
</script>

<button class="link" onclick={() => go({ name: 'contacts' })}>← Contacts</button>

{#if contact}
  <div class="row between" style="margin-top: 8px">
    <div>
      <h1>{contact.first_name || contact.last_name ? `${contact.first_name} ${contact.last_name}` : contact.email}</h1>
      <p class="muted">{contact.email} {#if contact.do_not_contact}<span class="badge bad">Do not contact</span>{/if}</p>
    </div>
    <div class="row">
      <button onclick={toggleDnc}>{contact.do_not_contact ? 'Allow emailing again' : 'Add to do-not-contact'}</button>
      <button onclick={() => (editing = true)}>Edit</button>
      <button class="danger" onclick={remove}>Delete</button>
    </div>
  </div>

  <div class="grid2">
    <div class="panel">
      <h2>Details</h2>
      <table class="small">
        <tbody>
          {#each Object.entries(contact.fields) as [k, v]}
            <tr><td class="muted mono">{k}</td><td>{v || '—'}</td></tr>
          {:else}
            <tr><td class="muted">No custom fields.</td></tr>
          {/each}
          <tr><td class="muted">Lists</td><td>{contact.lists.map((l) => lists.find((x) => x.id === l)?.name).filter(Boolean).join(', ') || '—'}</td></tr>
          <tr><td class="muted">Tags</td><td>{contact.tags.join(', ') || '—'}</td></tr>
          <tr><td class="muted">Added</td><td>{when(contact.created_at)}</td></tr>
        </tbody>
      </table>
    </div>
    <div class="panel">
      <h2>Timeline</h2>
      {#if timeline.length === 0}
        <p class="muted">No emails sent or received yet.</p>
      {:else}
        {#each timeline as t}
          <div class="item">
            <div class="row between">
              <span><span class="badge {t.direction === 'sent' ? 'accent' : t.status === 'reply' ? 'ok' : t.status === 'bounce' || t.status === 'unsubscribe' ? 'bad' : ''}">{t.direction === 'sent' ? '→' : '←'} {statusText(t)}</span>
                <strong>{t.subject}</strong></span>
              <span class="muted small">{when(t.at)}</span>
            </div>
            <div class="muted small">
              {#if t.campaign_name}{t.campaign_name}{#if t.step_number} · email {t.step_number}{/if}{/if}
              {#if t.detail} · {t.detail}{/if}
            </div>
          </div>
        {/each}
      {/if}
    </div>
  </div>
{/if}

{#if editing && contact}
  <ContactEditor {contact} onclose={() => (editing = false)} />
{/if}

<style>
  .item { padding: 8px 0; border-bottom: 1px solid var(--border); }
  .item:last-child { border-bottom: none; }
</style>
