<script lang="ts">
  import { onMount } from 'svelte';
  import { listen } from '@tauri-apps/api/event';
  import { api, errorText } from './lib/api';
  import { ui, go, bump, toast, type Page } from './lib/state.svelte';
  import Dashboard from './pages/Dashboard.svelte';
  import Campaigns from './pages/Campaigns.svelte';
  import CampaignDetail from './pages/CampaignDetail.svelte';
  import CampaignEditor from './pages/CampaignEditor.svelte';
  import Contacts from './pages/Contacts.svelte';
  import ContactDetail from './pages/ContactDetail.svelte';
  import Inbox from './pages/Inbox.svelte';
  import Accounts from './pages/Accounts.svelte';
  import Settings from './pages/Settings.svelte';
  import Welcome from './pages/Welcome.svelte';

  const nav: { label: string; page: Page['name'] }[] = [
    { label: 'Dashboard', page: 'dashboard' },
    { label: 'Campaigns', page: 'campaigns' },
    { label: 'Contacts', page: 'contacts' },
    { label: 'Inbox', page: 'inbox' },
    { label: 'Accounts', page: 'accounts' },
    { label: 'Settings', page: 'settings' },
  ];

  const section = $derived.by(() => {
    const n = ui.page.name;
    if (n === 'campaign' || n === 'campaign-edit') return 'campaigns';
    if (n === 'contact') return 'contacts';
    return n;
  });

  async function refreshInfo() {
    try {
      ui.info = await api.appInfo();
      ui.unread = await api.unreadCount();
    } catch (e) {
      toast(errorText(e), 'error');
    }
  }

  onMount(() => {
    refreshInfo().then(() => {
      if (ui.info && !ui.info.welcome_done) go({ name: 'welcome' });
    });
    const unlisten = listen('data-changed', () => {
      bump();
      refreshInfo();
    });
    return () => {
      unlisten.then((f) => f());
    };
  });
</script>

<div class="shell">
  <aside>
    <div class="brand">Auto Mailer</div>
    <nav>
      {#each nav as item}
        <button class:active={section === item.page} onclick={() => go({ name: item.page } as Page)}>
          {item.label}
          {#if item.page === 'inbox' && ui.unread > 0}<span class="count">{ui.unread}</span>{/if}
        </button>
      {/each}
    </nav>
    <div class="foot muted small">
      {#if ui.info}v{ui.info.version}{/if}
    </div>
  </aside>

  <main>
    {#if ui.info?.dry_run}
      <div class="notice warn dry">
        <strong>Dry-run mode.</strong> Nothing is really sent — emails are saved to a folder on this computer instead.
        <button class="link" onclick={() => go({ name: 'settings' })}>Change in Settings</button>
      </div>
    {/if}

    {#key ui.page}
      {#if ui.page.name === 'dashboard'}<Dashboard />
      {:else if ui.page.name === 'campaigns'}<Campaigns />
      {:else if ui.page.name === 'campaign'}<CampaignDetail id={ui.page.id} />
      {:else if ui.page.name === 'campaign-edit'}<CampaignEditor id={ui.page.id} />
      {:else if ui.page.name === 'contacts'}<Contacts />
      {:else if ui.page.name === 'contact'}<ContactDetail id={ui.page.id} />
      {:else if ui.page.name === 'inbox'}<Inbox />
      {:else if ui.page.name === 'accounts'}<Accounts />
      {:else if ui.page.name === 'settings'}<Settings />
      {:else if ui.page.name === 'welcome'}<Welcome />
      {/if}
    {/key}
  </main>
</div>

<div class="toasts">
  {#each ui.toasts as t (t.id)}
    <div class="toast {t.kind}">{t.text}</div>
  {/each}
</div>

<style>
  .shell { display: flex; min-height: 100vh; }
  aside {
    width: 200px;
    flex-shrink: 0;
    border-right: 1px solid var(--border);
    background: var(--panel);
    display: flex;
    flex-direction: column;
    position: sticky;
    top: 0;
    height: 100vh;
  }
  .brand { font-weight: 700; font-size: 16px; padding: 18px 18px 14px; }
  nav { display: flex; flex-direction: column; gap: 2px; padding: 0 8px; }
  nav button {
    text-align: left;
    border: none;
    background: none;
    padding: 8px 10px;
    display: flex;
    justify-content: space-between;
    align-items: center;
  }
  nav button:hover { background: var(--bg); }
  nav button.active { background: var(--accent-soft); color: var(--accent); font-weight: 600; }
  .count { background: var(--accent); color: var(--accent-text); border-radius: 999px; font-size: 11px; padding: 0 7px; font-weight: 600; }
  .foot { margin-top: auto; padding: 14px 18px; }
  main { flex: 1; padding: 24px 28px 60px; min-width: 0; max-width: 1200px; }
  .dry { display: flex; gap: 8px; flex-wrap: wrap; align-items: baseline; }
  .toasts { position: fixed; right: 20px; bottom: 20px; display: flex; flex-direction: column; gap: 8px; z-index: 100; }
  .toast { background: var(--panel); border: 1px solid var(--border); box-shadow: var(--shadow); border-radius: var(--radius); padding: 10px 14px; max-width: 420px; }
  .toast.ok { border-left: 4px solid var(--ok); }
  .toast.error { border-left: 4px solid var(--bad); }
</style>
