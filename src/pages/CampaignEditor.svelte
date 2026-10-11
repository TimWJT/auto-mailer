<script lang="ts">
  import { untrack } from 'svelte';
  import { ask, open } from '@tauri-apps/plugin-dialog';
  import { api, errorText } from '../lib/api';
  import { ui, go, toast } from '../lib/state.svelte';
  import { DAY_NAMES, minutesToTime, timeToMinutes, bytes } from '../lib/format';
  import type { Account, Campaign, ContactList, Preview, StepAttachment } from '../lib/types';
  import LaunchChecklist from '../components/LaunchChecklist.svelte';

  let { id }: { id: number | null } = $props();

  interface StepForm {
    key: number;
    id: number | null;
    delay_days: number;
    subject: string;
    body: string;
    same_thread: boolean;
    attachments: StepAttachment[];
  }

  // Only the initial id matters: after the first save this page tracks its own id.
  let campaignId = $state<number | null>(untrack(() => id));
  let status = $state<string>('draft');
  let name = $state('');
  let accountId = $state<string>('');
  let listId = $state<string>('');
  let days = $state<boolean[]>([true, true, true, true, true, false, false]);
  let start = $state('09:00');
  let end = $state('17:00');
  let gapMin = $state(45);
  let gapMax = $state(120);
  let footer = $state('');
  let steps = $state<StepForm[]>([]);
  let accounts = $state<Account[]>([]);
  let lists = $state<ContactList[]>([]);
  let fields = $state<string[]>([]);
  let loaded = $state(false);
  let saving = $state(false);
  let dirty = $state(false);
  let showLaunch = $state(false);

  let previewStep = $state(1);
  let previewIndex = $state(0);
  let preview = $state<Preview | null>(null);

  let nextKey = 0;
  let defaultFooter = '';
  const textareas: Record<number, HTMLTextAreaElement> = {};

  function fromCampaign(c: Campaign) {
    status = c.status;
    name = c.name;
    accountId = c.account_id ? String(c.account_id) : '';
    listId = c.list_id ? String(c.list_id) : '';
    days = [...c.window.days];
    start = minutesToTime(c.window.start_min);
    end = minutesToTime(Math.min(c.window.end_min, 1439));
    gapMin = c.gap_min_secs;
    gapMax = c.gap_max_secs;
    footer = c.footer;
    steps = c.steps.map((s) => ({ key: nextKey++, id: s.id, delay_days: s.delay_days, subject: s.subject, body: s.body, same_thread: s.same_thread, attachments: s.attachments }));
  }

  async function init() {
    try {
      const [a, l, f, settings] = await Promise.all([api.listAccounts(), api.listLists(), api.fieldNames(), api.getSettings()]);
      accounts = a;
      lists = l;
      fields = f;
      defaultFooter = settings.footer;
      if (campaignId) {
        fromCampaign(await api.getCampaign(campaignId));
      } else {
        days = [...settings.window.days];
        start = minutesToTime(settings.window.start_min);
        end = minutesToTime(Math.min(settings.window.end_min, 1439));
        gapMin = settings.gap_min_secs;
        gapMax = settings.gap_max_secs;
        footer = settings.footer;
        if (a.length === 1) accountId = String(a[0].id);
        steps = [
          { key: nextKey++, id: null, delay_days: 0, subject: '', body: 'Hi {{first_name|there}},\n\n', same_thread: false, attachments: [] },
          { key: nextKey++, id: null, delay_days: 3, subject: '', body: 'Hi {{first_name|there}},\n\nJust bumping this in case it got buried.\n', same_thread: true, attachments: [] },
        ];
      }
      loaded = true;
    } catch (e) {
      toast(errorText(e), 'error');
    }
  }
  init();

  const editable = $derived(status === 'draft' || status === 'paused');

  async function save(quiet = false): Promise<boolean> {
    if (!footer.trim() && defaultFooter.trim() && !quiet) {
      const ok = await ask(
        "Without the opt-out line, people can't easily ask you to stop, and you're more likely to be marked as spam. Save without it?",
        { title: 'No opt-out line', kind: 'warning', okLabel: 'Save anyway' },
      );
      if (!ok) return false;
    }
    saving = true;
    try {
      const c = await api.saveCampaign({
        id: campaignId,
        name: name || 'Untitled campaign',
        account_id: accountId ? Number(accountId) : null,
        list_id: listId ? Number(listId) : null,
        window: { days, start_min: timeToMinutes(start), end_min: timeToMinutes(end) },
        gap_min_secs: Number(gapMin),
        gap_max_secs: Number(gapMax),
        footer,
        steps: steps.map((s, i) => ({ id: s.id, delay_days: i === 0 ? 0 : Number(s.delay_days), subject: s.subject, body: s.body, same_thread: i > 0 && s.same_thread })),
      });

      campaignId = c.id;
      fromCampaign(c);
      dirty = false;
      if (!quiet) toast('Saved.');
      await refreshPreview();
      return true;
    } catch (e) {
      toast(errorText(e), 'error');
      return false;
    } finally {
      saving = false;
    }
  }

  async function refreshPreview() {
    if (!campaignId) return;
    try {
      preview = await api.previewStep(campaignId, previewStep, previewIndex);
    } catch {
      preview = null;
    }
  }

  $effect(() => {
    previewStep;
    previewIndex;
    if (loaded && campaignId) refreshPreview();
  });

  function addStep() {
    steps.push({ key: nextKey++, id: null, delay_days: 3, subject: '', body: '', same_thread: true, attachments: [] });
    dirty = true;
  }

  function removeStep(i: number) {
    steps.splice(i, 1);
    dirty = true;
  }

  function insertField(key: number, field: string) {
    const ta = textareas[key];
    const step = steps.find((s) => s.key === key);
    if (!ta || !step) return;
    const tag = `{{${field}}}`;
    const pos = ta.selectionStart ?? step.body.length;
    step.body = step.body.slice(0, pos) + tag + step.body.slice(ta.selectionEnd ?? pos);
    dirty = true;
    queueMicrotask(() => {
      ta.focus();
      ta.selectionStart = ta.selectionEnd = pos + tag.length;
    });
  }

  async function addAttachment(s: StepForm) {
    if (!s.id) {
      if (!(await save(true))) return;
      s = steps.find((x) => x.key === s.key) ?? steps[steps.length - 1];
    }
    const stepId = steps.find((x) => x.key === s.key)?.id;
    if (!stepId) return;
    const path = await open({ multiple: false });
    if (typeof path !== 'string') return;
    try {
      await api.addAttachment(stepId, path);
      fromCampaign(await api.getCampaign(campaignId!));
      toast('Attached. Remember: attachments make cold emails more likely to land in spam.');
    } catch (e) {
      toast(errorText(e), 'error');
    }
  }

  async function removeAttachment(aid: number) {
    await api.removeAttachment(aid);
    fromCampaign(await api.getCampaign(campaignId!));
  }

  async function sendTest(stepNumber: number) {
    if (!(await save(true))) return;
    try {
      const to = await api.sendTest(campaignId!, stepNumber, preview?.contact_id ?? null);
      toast(ui.info?.dry_run ? `Test saved to the dry-run folder (would go to ${to}).` : `Test sent to ${to}.`);
    } catch (e) {
      toast(errorText(e), 'error');
    }
  }

  async function launch() {
    if (!(await save(true))) return;
    showLaunch = true;
  }

  async function remove() {
    if (!campaignId) return go({ name: 'campaigns' });
    const ok = await ask('Delete this campaign? This removes its history too.', { title: 'Delete campaign', kind: 'warning', okLabel: 'Delete' });
    if (!ok) return;
    try {
      await api.deleteCampaign(campaignId);
      go({ name: 'campaigns' });
    } catch (e) {
      toast(errorText(e), 'error');
    }
  }

  function stepLabel(i: number) {
    return i === 0 ? 'First email' : `Follow-up ${i}`;
  }
</script>

<button class="link" onclick={() => go(campaignId && status !== 'draft' ? { name: 'campaign', id: campaignId } : { name: 'campaigns' })}>← Back</button>

{#if loaded}
  <div class="row between" style="margin: 8px 0 16px">
    <h1>{campaignId ? (status === 'draft' ? 'Edit campaign' : `Edit ${name}`) : 'New campaign'}</h1>
    <div class="row">
      {#if status === 'draft'}<button class="danger" onclick={remove}>Delete</button>{/if}
      <button onclick={() => save()} disabled={saving || !editable}>{saving ? 'Saving…' : 'Save'}</button>
      {#if status === 'draft'}<button class="primary" onclick={launch} disabled={saving}>Review and start…</button>{/if}
    </div>
  </div>

  {#if !editable}
    <div class="notice warn">This campaign is {status}. Pause it to make changes.</div>
  {:else if status === 'paused'}
    <div class="notice info">Changes apply to emails that haven't been sent yet.</div>
  {/if}

  <fieldset disabled={!editable} oninput={() => (dirty = true)}>
    <div class="panel">
      <div class="grid2">
        <label class="field"><span class="label">Campaign name</span><input bind:value={name} placeholder="October intro emails" /></label>
        <div></div>
        <label class="field"><span class="label">Send from</span>
          <select bind:value={accountId} disabled={status !== 'draft'}>
            <option value="">Choose an account…</option>
            {#each accounts as a}<option value={String(a.id)}>{a.email}</option>{/each}
          </select>
          {#if accounts.length === 0}<div class="hint">No accounts yet. <button type="button" class="link" onclick={() => go({ name: 'accounts' })}>Add one</button></div>{/if}
        </label>
        <label class="field"><span class="label">Send to list</span>
          <select bind:value={listId} disabled={status !== 'draft'}>
            <option value="">Choose a list…</option>
            {#each lists as l}<option value={String(l.id)}>{l.name} ({l.member_count})</option>{/each}
          </select>
          {#if lists.length === 0}<div class="hint">No lists yet. <button type="button" class="link" onclick={() => go({ name: 'contacts' })}>Import contacts</button></div>{/if}
        </label>
      </div>
    </div>

    {#each steps as s, i (s.key)}
      <div class="panel">
        <div class="row between">
          <h2>{stepLabel(i)}</h2>
          <div class="row">
            <button type="button" onclick={() => sendTest(i + 1)} title="Send this email to your own address">Send test to myself</button>
            {#if i > 0}<button type="button" class="danger" onclick={() => removeStep(i)}>Remove</button>{/if}
          </div>
        </div>
        {#if i > 0}
          <div class="row" style="margin-bottom: 12px">
            If no reply after <input type="number" min="1" style="width: 70px" bind:value={s.delay_days} /> day{s.delay_days == 1 ? '' : 's'}
            <label class="row" style="margin-left: 16px; gap: 4px"><input type="checkbox" bind:checked={s.same_thread} /> Reply in the same thread</label>
          </div>
        {/if}
        {#if i === 0 || !s.same_thread}
          <label class="field"><span class="label">Subject</span><input bind:value={s.subject} placeholder="Quick question, {'{{first_name}}'}" /></label>
        {:else}
          <p class="muted small">Subject: "Re: " + the earlier subject, so it appears in the same conversation.</p>
        {/if}
        <label class="field"><span class="label">Message (plain text)</span>
          <textarea rows="8" bind:value={s.body} bind:this={textareas[s.key]}></textarea></label>
        <div class="row small">
          <span class="muted">Insert:</span>
          {#each fields as f}<button type="button" class="chip" onclick={() => insertField(s.key, f)}>{f}</button>{/each}
        </div>
        <div class="row small" style="margin-top: 10px">
          {#each s.attachments as a}
            <span class="badge">{a.file_name} ({bytes(a.size_bytes)}) <button type="button" class="link" onclick={() => removeAttachment(a.id)}>✕</button></span>
          {/each}
          <button type="button" class="link" onclick={() => addAttachment(s)}>+ Attach a file (max 3 MB)</button>
        </div>
      </div>
    {/each}
    <button type="button" onclick={addStep} style="margin-bottom: 16px">+ Add follow-up</button>

    <div class="panel">
      <h2>Opt-out line</h2>
      <textarea rows="2" bind:value={footer}></textarea>
      <div class="hint">Added at the bottom of every email. Replies like "stop" are detected automatically and the person is added to your do-not-contact list.</div>
    </div>

    <div class="panel">
      <h2>When to send</h2>
      <div class="row" style="margin-bottom: 12px">
        {#each DAY_NAMES as d, i}
          <label class="row day" style="gap: 4px"><input type="checkbox" bind:checked={days[i]} /> {d}</label>
        {/each}
      </div>
      <div class="row" style="margin-bottom: 12px">
        Between <input type="time" bind:value={start} /> and <input type="time" bind:value={end} /> <span class="muted">your local time</span>
      </div>
      <div class="row">
        Wait between <input type="number" min="5" style="width: 80px" bind:value={gapMin} /> and
        <input type="number" min="5" style="width: 80px" bind:value={gapMax} /> seconds between emails (random)
      </div>
    </div>
  </fieldset>

  {#if campaignId}
    <div class="panel">
      <div class="row between">
        <h2>Preview</h2>
        <div class="row">
          <select bind:value={previewStep}>
            {#each steps as _, i}<option value={i + 1}>{stepLabel(i)}</option>{/each}
          </select>
          <button disabled={!preview || previewIndex === 0} onclick={() => previewIndex--}>‹</button>
          <span class="muted small">{preview ? `${preview.index + 1} of ${preview.total}` : ''}</span>
          <button disabled={!preview || preview.index + 1 >= preview.total} onclick={() => previewIndex++}>›</button>
        </div>
      </div>
      {#if dirty}<p class="small muted">Save to update the preview.</p>{/if}
      {#if preview}
        <p class="small muted">To: {preview.contact_name ? `${preview.contact_name} <${preview.to}>` : preview.to}</p>
        {#if preview.do_not_contact}<div class="notice bad">This contact is on the do-not-contact list and won't be emailed.</div>{/if}
        {#if preview.email.missing.length}
          <div class="notice bad">Won't be sent to this contact: <strong>{preview.email.missing.join(', ')}</strong> is blank. Add a fallback, e.g. <span class="mono">{`{{${preview.email.missing[0]}|…}}`}</span></div>
        {/if}
        <pre class="email-preview"><strong>Subject: {preview.email.subject}</strong>

{preview.email.body}</pre>
      {:else}
        <p class="muted">Choose a list with contacts to see a preview.</p>
      {/if}
    </div>
  {/if}
{/if}

{#if showLaunch && campaignId}
  <LaunchChecklist campaignId={campaignId} onclose={() => (showLaunch = false)} />
{/if}

<style>
  fieldset { border: none; padding: 0; margin: 0; min-width: 0; }
  .chip { padding: 1px 8px; border-radius: 999px; font-size: 12px; }
  .day { margin-right: 6px; }
</style>
