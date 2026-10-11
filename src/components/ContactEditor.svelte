<script lang="ts">
  import { untrack } from 'svelte';
  import { api, errorText } from '../lib/api';
  import { toast } from '../lib/state.svelte';
  import type { Contact, ContactList } from '../lib/types';
  import Modal from './Modal.svelte';

  let { contact = null, onclose, onsaved }: { contact?: Contact | null; onclose: () => void; onsaved?: (c: Contact) => void } = $props();

  // The form is a one-off copy; the editor is re-created for each contact.
  const start = untrack(() => contact);
  let email = $state(start?.email ?? '');
  let first = $state(start?.first_name ?? '');
  let last = $state(start?.last_name ?? '');
  let fields = $state<{ k: string; v: string }[]>(Object.entries(start?.fields ?? {}).map(([k, v]) => ({ k, v })));
  let tags = $state((start?.tags ?? []).join(', '));
  let listIds = $state<number[]>([...(start?.lists ?? [])]);
  let lists = $state<ContactList[]>([]);
  let busy = $state(false);

  $effect(() => {
    api.listLists().then((l) => (lists = l));
  });

  function toggleList(id: number) {
    listIds = listIds.includes(id) ? listIds.filter((x) => x !== id) : [...listIds, id];
  }

  async function save(e: Event) {
    e.preventDefault();
    busy = true;
    try {
      const saved = await api.saveContact({
        id: start?.id ?? 0,
        email,
        first_name: first,
        last_name: last,
        fields: Object.fromEntries(fields.filter((f) => f.k.trim()).map((f) => [f.k.trim(), f.v])),
        tags: tags.split(',').map((t) => t.trim()).filter(Boolean),
        lists: listIds,
        do_not_contact: false,
        created_at: '',
      });
      toast('Saved.');
      onsaved?.(saved);
      onclose();
    } catch (err) {
      toast(errorText(err), 'error');
    } finally {
      busy = false;
    }
  }
</script>

<Modal title={start ? 'Edit contact' : 'Add contact'} {onclose}>
  <form onsubmit={save}>
    <label class="field"><span class="label">Email</span><input type="email" bind:value={email} required /></label>
    <div class="grid2">
      <label class="field"><span class="label">First name</span><input bind:value={first} /></label>
      <label class="field"><span class="label">Last name</span><input bind:value={last} /></label>
    </div>
    <div class="field">
      <span class="label">Custom fields</span>
      {#each fields as f, i}
        <div class="row" style="margin-bottom: 6px">
          <input bind:value={f.k} placeholder="company" style="width: 35%" />
          <input bind:value={f.v} placeholder="Acme Ltd" class="grow" />
          <button type="button" class="link danger" onclick={() => fields.splice(i, 1)}>Remove</button>
        </div>
      {/each}
      <button type="button" class="link" onclick={() => fields.push({ k: '', v: '' })}>+ Add field</button>
      <div class="hint">Use them in emails as <span class="mono">{'{{company}}'}</span>.</div>
    </div>
    <label class="field"><span class="label">Tags (comma-separated)</span><input bind:value={tags} /></label>
    {#if lists.length}
      <div class="field">
        <span class="label">Lists</span>
        <div class="row">
          {#each lists as l}
            <label class="row" style="gap: 4px; margin-right: 10px"><input type="checkbox" checked={listIds.includes(l.id)} onchange={() => toggleList(l.id)} /> {l.name}</label>
          {/each}
        </div>
      </div>
    {/if}
    <div class="row">
      <button class="primary" type="submit" disabled={busy}>Save</button>
      <button type="button" onclick={onclose}>Cancel</button>
    </div>
  </form>
</Modal>
