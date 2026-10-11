// App-wide UI state: navigation, a change counter pages reload on, and toasts.

import type { AppInfo } from './types';

export type Page =
  | { name: 'dashboard' }
  | { name: 'campaigns' }
  | { name: 'campaign'; id: number }
  | { name: 'campaign-edit'; id: number | null }
  | { name: 'contacts' }
  | { name: 'contact'; id: number }
  | { name: 'inbox' }
  | { name: 'accounts' }
  | { name: 'settings' }
  | { name: 'welcome' };

export const ui = $state({
  page: { name: 'dashboard' } as Page,
  /** Bumped whenever the backend says data changed; pages reload on it. */
  version: 0,
  info: null as AppInfo | null,
  unread: 0,
  toasts: [] as { id: number; text: string; kind: 'ok' | 'error' }[],
});

export function go(page: Page) {
  ui.page = page;
  window.scrollTo(0, 0);
}

let toastId = 0;
export function toast(text: string, kind: 'ok' | 'error' = 'ok') {
  const id = ++toastId;
  ui.toasts.push({ id, text, kind });
  setTimeout(() => {
    ui.toasts = ui.toasts.filter((t) => t.id !== id);
  }, kind === 'error' ? 8000 : 3500);
}

export function bump() {
  ui.version++;
}
