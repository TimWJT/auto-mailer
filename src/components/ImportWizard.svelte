<script lang="ts">
  import { open } from '@tauri-apps/plugin-dialog';
  import { api, errorText } from '../lib/api';
  import { toast } from '../lib/state.svelte';
  import type { ContactList, ImportPreview, ImportSummary, Target } from '../lib/types';
  import Modal from './Modal.svelte';

  let { toDnc = false, onclose }: { toDnc?: boolean; onclose: () => void } = $props();

  let path = $state<string | null>(null);
  let preview = $state<ImportPreview | null>(null);
  // One select value per column: "email" | "first_name" | "last_name" | "ignore" | "custom"
  let choice = $state<string[]>([]);
  let customName = $state<string[]>([]);
  let lists = $state<ContactList[]>([]);
  let listId = $state<string>('');
  let newList = $state('');
  let tags = $state('');
  let busy = $state(false);
  let summary = $state<ImportSummary | null>(null);

  async function pick() {
    const p = await open({ multiple: false, filters: [{ name: 'Spreadsheets', extensions: ['csv', 'xlsx', 'xls', 'tsv', 'txt'] }] });
    if (typeof p !== 'string') return;
    busy = true;
    try {
      preview = await api.importPreview(p);
      path = p;
      choice = preview.mapping.map((t) => t.kind);
      customName = preview.mapping.map((t, i) => (t.kind === 'custom' ? t.name : preview!.headers[i]));
      if (!toDnc) lists = await api.listLists();
    } catch (e) {
      toast(errorText(e), 'error');
    } finally {
      busy = false;
    }
  }

  function mapping(): Target[] {
    return choice.map((c, i) => {
      if (toDnc) return c === 'email' ? { kind: 'email' } : { kind: 'ignore' };
      if (c === 'custom') return { kind: 'custom', name: customName[i] || preview!.headers[i] };
      return { kind: c } as Target;
    });
  }

  const emailCount = $derived(choice.filter((c) => c === 'email').length);

  async function run() {
    if (!path || !preview) return;
    if (emailCount !== 1) {
      toast('Choose exactly one column as the email address.', 'error');
      return;
    }
    busy = true;
    try {
      let list: number | null = listId ? Number(listId) : null;
      if (!toDnc && listId === 'new') {
        if (!newList.trim()) {
          toast('Name the new list.', 'error');
          busy = false;
          return;
        }
        list = await api.createList(newList);
      }
      summary = await api.importCommit(path, {
        mapping: mapping(),
        list_id: list,
        tags: tags.split(',').map((t) => t.trim()).filter(Boolean),
        to_dnc: toDnc,
      });
    } catch (e) {
      toast(errorText(e), 'error');
    } finally {
      busy = false;
    }
  }
</script>

<Modal title={toDnc ? 'Import to do-not-contact list' : 'Import contacts'} {onclose} wide>
  {#if summary}
    <div class="notice ok">
      <strong>Import finished.</strong>
      {summary.added} added{#if !toDnc}, {summary.updated} updated{/if}, {summary.duplicates} duplicates skipped, {summary.invalid} invalid.
    </div>
    {#if summary.invalid_rows.length}
      <h3>Rows with an invalid email address (not imported)</h3>
      <table class="small">
        <thead><tr><th>Row</th><th>Value</th></tr></thead>
        <tbody>
          {#each summary.invalid_rows as [row, value]}<tr><td>{row}</td><td class="mono">{value || '(blank)'}</td></tr>{/each}
        </tbody>
      </table>
      {#if summary.invalid > summary.invalid_rows.length}<p class="muted small">…and {summary.invalid - summary.invalid_rows.length} more.</p>{/if}
    {/if}
    <div class="row" style="margin-top: 14px"><button class="primary" onclick={onclose}>Done</button></div>
  {:else if !preview}
    <p>Choose a <strong>CSV</strong> or <strong>Excel (.xlsx)</strong> file. The first row should contain column names, like <em>email</em>, <em>first name</em>, <em>company</em>.</p>
    <button class="primary" onclick={pick} disabled={busy}>{busy ? 'Reading…' : 'Choose file…'}</button>
  {:else}
    <p class="muted">{preview.total_rows} rows found. Check how each column should be used{toDnc ? ' — only the email column matters here' : ''}.</p>
    <div class="scroll">
      <table>
        <thead><tr><th>Column in file</th><th>Use as</th><th>Example values</th></tr></thead>
        <tbody>
          {#each preview.headers as h, i}
            <tr>
              <td><strong>{h || `(column ${i + 1})`}</strong></td>
              <td>
                <div class="row">
                  <select bind:value={choice[i]}>
                    <option value="email">Email address</option>
                    {#if !toDnc}
                      <option value="first_name">First name</option>
                      <option value="last_name">Last name</option>
                      <option value="custom">Custom field</option>
                    {/if}
                    <option value="ignore">Don't import</option>
                  </select>
                  {#if choice[i] === 'custom' && !toDnc}
                    <input bind:value={customName[i]} style="width: 140px" title="Merge field name" />
                  {/if}
                </div>
              </td>
              <td class="muted small">{preview.sample.map((r) => r[i]).filter(Boolean).slice(0, 3).join(' · ')}</td>
            </tr>
          {/each}
        </tbody>
      </table>
    </div>
    {#if emailCount !== 1}<div class="notice bad" style="margin-top: 12px">Choose exactly one column as the email address.</div>{/if}
    {#if !toDnc}
      <div class="grid2" style="margin-top: 14px">
        <label class="field"><span class="label">Add to list</span>
          <select bind:value={listId}>
            <option value="">No list</option>
            {#each lists as l}<option value={String(l.id)}>{l.name} ({l.member_count})</option>{/each}
            <option value="new">New list…</option>
          </select>
          {#if listId === 'new'}<input style="margin-top: 6px" bind:value={newList} placeholder="List name" />{/if}
        </label>
        <label class="field"><span class="label">Tags (optional, comma-separated)</span>
          <input bind:value={tags} placeholder="conference, 2026" /></label>
      </div>
      <p class="small muted">Contacts already in Auto Mailer (same email) are updated: non-empty cells replace what's stored.</p>
    {/if}
    <div class="row">
      <button class="primary" onclick={run} disabled={busy || emailCount !== 1}>{busy ? 'Importing…' : `Import ${preview.total_rows} rows`}</button>
      <button onclick={pick} disabled={busy}>Choose a different file</button>
    </div>
  {/if}
</Modal>

<style>
  .scroll { max-height: 50vh; overflow: auto; }
</style>
