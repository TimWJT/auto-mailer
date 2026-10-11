<script lang="ts">
  import { ask, open, save } from '@tauri-apps/plugin-dialog';
  import { revealItemInDir } from '@tauri-apps/plugin-opener';
  import { api, errorText } from '../lib/api';
  import { ui, toast } from '../lib/state.svelte';
  import { DAY_NAMES, minutesToTime, timeToMinutes } from '../lib/format';
  import type { Settings } from '../lib/types';

  let s = $state<Settings | null>(null);
  let start = $state('09:00');
  let end = $state('17:00');

  $effect(() => {
    api.getSettings().then((x) => {
      s = x;
      start = minutesToTime(x.window.start_min);
      end = minutesToTime(Math.min(x.window.end_min, 1439));
    });
  });

  async function persist(next: Settings, message = 'Saved.') {
    try {
      await api.saveSettings(next);
      ui.info = await api.appInfo();
      toast(message);
    } catch (e) {
      toast(errorText(e), 'error');
    }
  }

  async function saveForm(e: Event) {
    e.preventDefault();
    if (!s) return;
    s.window.start_min = timeToMinutes(start);
    s.window.end_min = timeToMinutes(end);
    await persist({ ...s, gap_min_secs: Number(s.gap_min_secs), gap_max_secs: Number(s.gap_max_secs), poll_minutes: Number(s.poll_minutes) });
  }

  async function toggleDryRun() {
    if (!s) return;
    if (s.dry_run) {
      const ok = await ask(
        'Turn on real sending? Running campaigns will start emailing real people from your accounts. (Emails already written to the dry-run folder are not re-sent.)',
        { title: 'Send real emails', kind: 'warning', okLabel: 'Turn on real sending' },
      );
      if (!ok) return;
    }
    s.dry_run = !s.dry_run;
    await persist(s, s.dry_run ? 'Dry-run mode on: nothing will be sent.' : 'Real sending is on.');
  }

  async function backup() {
    const date = new Date().toISOString().slice(0, 10);
    const path = await save({ defaultPath: `auto-mailer-backup-${date}.sqlite`, filters: [{ name: 'Auto Mailer backup', extensions: ['sqlite'] }] });
    if (!path) return;
    try {
      await api.backup(path);
      toast('Backup saved.');
    } catch (e) {
      toast(errorText(e), 'error');
    }
  }

  async function restore() {
    const path = await open({ multiple: false, filters: [{ name: 'Auto Mailer backup', extensions: ['sqlite'] }] });
    if (typeof path !== 'string') return;
    const ok = await ask(
      'Replace everything in Auto Mailer with this backup? Your current contacts, campaigns and history will be overwritten. Passwords and sign-ins are not in backups, so you may need to reconnect accounts.',
      { title: 'Restore backup', kind: 'warning', okLabel: 'Restore' },
    );
    if (!ok) return;
    try {
      await api.restore(path);
      toast('Backup restored.');
    } catch (e) {
      toast(errorText(e), 'error');
    }
  }
</script>

<h1>Settings</h1>

{#if s}
  <div class="panel">
    <h2>Sending mode</h2>
    {#if s.dry_run}
      <p><span class="badge warn">Dry run</span> Emails are written to a folder on this computer instead of being sent. Use this to check everything looks right.</p>
      <div class="row">
        <button class="primary" onclick={toggleDryRun}>Turn on real sending</button>
        {#if ui.info}<button onclick={() => revealItemInDir(ui.info!.outbox_dir)}>Open the dry-run folder</button>{/if}
      </div>
    {:else}
      <p><span class="badge ok">Real sending</span> Campaigns send real emails.</p>
      <button onclick={toggleDryRun}>Switch to dry run</button>
    {/if}
  </div>

  <form onsubmit={saveForm}>
    <div class="panel">
      <h2>Defaults for new campaigns</h2>
      <div class="field">
        <span class="label">Send on</span>
        <div class="row">
          {#each DAY_NAMES as d, i}<label class="row" style="gap: 4px; margin-right: 6px"><input type="checkbox" bind:checked={s.window.days[i]} /> {d}</label>{/each}
        </div>
      </div>
      <div class="row field">Between <input type="time" bind:value={start} /> and <input type="time" bind:value={end} /></div>
      <div class="row field">
        Wait <input type="number" min="5" style="width: 80px" bind:value={s.gap_min_secs} /> to
        <input type="number" min="5" style="width: 80px" bind:value={s.gap_max_secs} /> seconds between emails
      </div>
      <label class="field"><span class="label">Opt-out line</span><textarea rows="2" bind:value={s.footer}></textarea></label>
      <div class="row field">Check for replies every <input type="number" min="1" max="120" style="width: 70px" bind:value={s.poll_minutes} /> minutes</div>
      <button class="primary" type="submit">Save</button>
    </div>
  </form>

  <div class="panel">
    <h2>Backup</h2>
    <p class="muted">Save all contacts, campaigns and history to one file. Passwords and sign-ins are not included — after restoring on another computer, reconnect your accounts.</p>
    <div class="row">
      <button onclick={backup}>Save a backup…</button>
      <button onclick={restore}>Restore from backup…</button>
    </div>
  </div>

  <div class="panel">
    <h2>About</h2>
    <p class="small">Auto Mailer {ui.info?.version} — free and unofficial. Everything is stored on this computer; there is no server and no account with us.</p>
    <p class="small">Data folder: <span class="mono">{ui.info?.data_dir}</span></p>
    <p class="small muted">You're responsible for following anti-spam law where you live: use honest sender details, and honour opt-outs (Auto Mailer does this automatically for "stop" replies).</p>
  </div>
{/if}
