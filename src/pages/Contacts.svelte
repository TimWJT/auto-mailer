<script lang="ts">
  import { ask, save } from '@tauri-apps/plugin-dialog';
  import { api, errorText } from '../lib/api';
  import { ui, go, toast } from '../lib/state.svelte';
  import { when } from '../lib/format';
  import type { Contact, ContactList, DncEntry } from '../lib/types';
  import ImportWizard from '../components/ImportWizard.svelte';
  import ContactEditor from '../components/ContactEditor.svelte';
  import Modal from '../components/Modal.svelte';

  let tab = $state<'contacts' | 'lists' | 'dnc'>('contacts');

  // contacts tab
  let search = $state('');
  let listFilter = $state<string>('');
  let tagFilter = $state<string>('');
  let rows = $state<Contact[]>([]);
  let total = $state(0);
  let page = $state(0);
  const PAGE = 200;
  let selected = $state<number[]>([]);
  let lists = $state<ContactList[]>([]);
  let tags = $state<string[]>([]);
  let showImport = $state<'contacts' | 'dnc' | null>(null);
  let showEditor = $state(false);
  let addToListId = $state('');

  // lists tab
  let newListName = $state('');

  // dnc tab
  let dnc = $state<DncEntry[]>([]);
  let dncInput = $state('');

  async function loadContacts() {
    try {
      const res = await api.listContacts({
        search,
        list_id: listFilter ? Number(listFilter) : null,
        tag: tagFilter || null,
        offset: page * PAGE,
        limit: PAGE,
      });
      rows = res.rows;
      total = res.total;
      selected = selected.filter((id) => rows.some((r) => r.id === id));
    } catch (e) {
      toast(errorText(e), 'error');
    }
  }

  async function loadMeta() {
    [lists, tags, dnc] = await Promise.all([api.listLists(), api.listTags(), api.dncList()]);
  }

  $effect(() => {
    ui.version;
    search;
    listFilter;
    tagFilter;
    page;
    loadContacts();
  });
  $effect(() => {
    ui.version;
    loadMeta();
  });

  function toggle(id: number) {
    selected = selected.includes(id) ? selected.filter((x) => x !== id) : [...selected, id];
  }
  function toggleAll() {
    selected = selected.length === rows.length ? [] : rows.map((r) => r.id);
  }

  async function deleteSelected() {
    const ok = await ask(`Delete ${selected.length} contact(s)? Their campaign history is deleted too. This can't be undone.`, {
      title: 'Delete contacts',
      kind: 'warning',
      okLabel: 'Delete',
    });
    if (!ok) return;
    try {
      const n = await api.deleteContacts(selected);
      toast(`Deleted ${n} contact(s).`);
      selected = [];
    } catch (e) {
      toast(errorText(e), 'error');
    }
  }

  async function addSelectedToList() {
    if (!addToListId) return;
    await api.addToList(Number(addToListId), selected);
    toast(`Added ${selected.length} to the list.`);
    addToListId = '';
  }

  async function removeSelectedFromList() {
    await api.removeFromList(Number(listFilter), selected);
    toast(`Removed ${selected.length} from the list.`);
    selected = [];
  }

  async function exportCsv() {
    const path = await save({ defaultPath: 'contacts.csv', filters: [{ name: 'CSV', extensions: ['csv'] }] });
    if (!path) return;
    try {
      const n = await api.exportContactsCsv(path);
      toast(`Exported ${n} contacts.`);
    } catch (e) {
      toast(errorText(e), 'error');
    }
  }

  async function createList(e: Event) {
    e.preventDefault();
    try {
      await api.createList(newListName);
      newListName = '';
    } catch (err) {
      toast(errorText(err), 'error');
    }
  }

  let renaming = $state<ContactList | null>(null);
  let renameTo = $state('');

  function renameList(l: ContactList) {
    renaming = l;
    renameTo = l.name;
  }

  async function doRename(e: Event) {
    e.preventDefault();
    if (!renaming) return;
    try {
      await api.renameList(renaming.id, renameTo);
      renaming = null;
    } catch (err) {
      toast(errorText(err), 'error');
    }
  }

  async function deleteList(l: ContactList) {
    const ok = await ask(`Delete the list "${l.name}"? The contacts themselves are kept.`, { title: 'Delete list', kind: 'warning', okLabel: 'Delete' });
    if (ok) await api.deleteList(l.id);
  }

  async function addDnc(e: Event) {
    e.preventDefault();
    const emails = dncInput.split(/[\s,;]+/).filter(Boolean);
    const n = await api.dncAdd(emails);
    toast(`Added ${n} address(es).`);
    dncInput = '';
  }

  async function removeDnc(email: string) {
    const ok = await ask(`Remove ${email} from the do-not-contact list? Campaigns will be able to email this address again.`, {
      title: 'Remove from do-not-contact',
      kind: 'warning',
      okLabel: 'Remove',
    });
    if (ok) await api.dncRemove(email);
  }

  function dncReason(r: string) {
    return { manual: 'Added by you', unsubscribe_reply: 'Replied asking to stop', bounce: 'Bounced', import: 'Imported' }[r] ?? r;
  }
</script>

<div class="row between">
  <div>
    <h1>Contacts</h1>
    <p class="muted">{total} contact{total === 1 ? '' : 's'}{listFilter || tagFilter || search ? ' match' : ''}</p>
  </div>
  <div class="row">
    <button onclick={exportCsv}>Export CSV</button>
    <button onclick={() => (showEditor = true)}>Add contact</button>
    <button class="primary" onclick={() => (showImport = 'contacts')}>Import CSV / Excel</button>
  </div>
</div>

<div class="tabs">
  <button class:active={tab === 'contacts'} onclick={() => (tab = 'contacts')}>All contacts</button>
  <button class:active={tab === 'lists'} onclick={() => (tab = 'lists')}>Lists ({lists.length})</button>
  <button class:active={tab === 'dnc'} onclick={() => (tab = 'dnc')}>Do-not-contact ({dnc.length})</button>
</div>

{#if tab === 'contacts'}
  <div class="row" style="margin-bottom: 12px">
    <input class="grow" placeholder="Search name, email or any field" bind:value={search} oninput={() => (page = 0)} />
    <select bind:value={listFilter} onchange={() => (page = 0)}>
      <option value="">All lists</option>
      {#each lists as l}<option value={String(l.id)}>{l.name}</option>{/each}
    </select>
    <select bind:value={tagFilter} onchange={() => (page = 0)}>
      <option value="">All tags</option>
      {#each tags as t}<option value={t}>{t}</option>{/each}
    </select>
  </div>

  {#if selected.length}
    <div class="panel row" style="padding: 10px 14px">
      <strong>{selected.length} selected</strong>
      <select bind:value={addToListId}>
        <option value="">Add to list…</option>
        {#each lists as l}<option value={String(l.id)}>{l.name}</option>{/each}
      </select>
      <button onclick={addSelectedToList} disabled={!addToListId}>Add</button>
      {#if listFilter}<button onclick={removeSelectedFromList}>Remove from this list</button>{/if}
      <button class="danger" onclick={deleteSelected}>Delete</button>
    </div>
  {/if}

  {#if rows.length === 0}
    <div class="panel empty">
      {#if total === 0 && !search && !listFilter && !tagFilter}
        No contacts yet. Import a CSV or Excel file to get started.
      {:else}
        Nothing matches.
      {/if}
    </div>
  {:else}
    <div class="panel" style="padding: 0">
      <table>
        <thead>
          <tr>
            <th style="width: 30px"><input type="checkbox" checked={selected.length === rows.length} onchange={toggleAll} /></th>
            <th>Email</th><th>Name</th><th>Fields</th><th>Tags</th>
          </tr>
        </thead>
        <tbody>
          {#each rows as c (c.id)}
            <tr class="clickable" onclick={() => go({ name: 'contact', id: c.id })}>
              <td onclick={(e) => e.stopPropagation()}><input type="checkbox" checked={selected.includes(c.id)} onchange={() => toggle(c.id)} /></td>
              <td>{c.email} {#if c.do_not_contact}<span class="badge bad">Do not contact</span>{/if}</td>
              <td>{c.first_name} {c.last_name}</td>
              <td class="muted small">{Object.entries(c.fields).filter(([, v]) => v).map(([k, v]) => `${k}: ${v}`).join(' · ')}</td>
              <td>{#each c.tags as t}<span class="badge" style="margin-right: 4px">{t}</span>{/each}</td>
            </tr>
          {/each}
        </tbody>
      </table>
    </div>
    {#if total > PAGE}
      <div class="row">
        <button disabled={page === 0} onclick={() => page--}>Previous</button>
        <span class="muted">{page * PAGE + 1}–{Math.min(total, (page + 1) * PAGE)} of {total}</span>
        <button disabled={(page + 1) * PAGE >= total} onclick={() => page++}>Next</button>
      </div>
    {/if}
  {/if}
{:else if tab === 'lists'}
  <form class="row" style="margin-bottom: 14px" onsubmit={createList}>
    <input bind:value={newListName} placeholder="New list name" />
    <button type="submit" disabled={!newListName.trim()}>Create list</button>
  </form>
  {#if lists.length === 0}
    <div class="panel empty">No lists yet. Campaigns send to a list, so create one or choose a list when importing.</div>
  {:else}
    <div class="panel" style="padding: 0">
      <table>
        <thead><tr><th>List</th><th>Contacts</th><th></th></tr></thead>
        <tbody>
          {#each lists as l (l.id)}
            <tr>
              <td><button class="link" onclick={() => { listFilter = String(l.id); tab = 'contacts'; }}>{l.name}</button></td>
              <td>{l.member_count}</td>
              <td style="text-align: right">
                <button onclick={() => renameList(l)}>Rename</button>
                <button class="danger" onclick={() => deleteList(l)}>Delete</button>
              </td>
            </tr>
          {/each}
        </tbody>
      </table>
    </div>
  {/if}
{:else}
  <p class="muted">Addresses here are <strong>never</strong> emailed by any campaign from any account. People who reply "stop" or whose emails bounce are added automatically.</p>
  <div class="panel">
    <form onsubmit={addDnc}>
      <label class="field"><span class="label">Add addresses</span>
        <textarea rows="3" bind:value={dncInput} placeholder="one@example.com, two@example.com"></textarea></label>
      <div class="row">
        <button type="submit" disabled={!dncInput.trim()}>Add</button>
        <button type="button" onclick={() => (showImport = 'dnc')}>Import from file…</button>
      </div>
    </form>
  </div>
  {#if dnc.length}
    <div class="panel" style="padding: 0">
      <table>
        <thead><tr><th>Email</th><th>Why</th><th>Added</th><th></th></tr></thead>
        <tbody>
          {#each dnc as d (d.email)}
            <tr>
              <td>{d.email}</td>
              <td>{dncReason(d.reason)}</td>
              <td class="muted">{when(d.added_at)}</td>
              <td style="text-align: right"><button class="danger" onclick={() => removeDnc(d.email)}>Remove</button></td>
            </tr>
          {/each}
        </tbody>
      </table>
    </div>
  {/if}
{/if}

{#if showImport}
  <ImportWizard toDnc={showImport === 'dnc'} onclose={() => (showImport = null)} />
{/if}
{#if showEditor}
  <ContactEditor onclose={() => (showEditor = false)} />
{/if}
{#if renaming}
  <Modal title="Rename list" onclose={() => (renaming = null)}>
    <form onsubmit={doRename}>
      <label class="field"><span class="label">Name</span><input bind:value={renameTo} required /></label>
      <button class="primary" type="submit">Save</button>
    </form>
  </Modal>
{/if}
