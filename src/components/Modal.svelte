<script lang="ts">
  import type { Snippet } from 'svelte';

  let { title, onclose, children, wide = false }: { title: string; onclose: () => void; children: Snippet; wide?: boolean } = $props();

  function key(e: KeyboardEvent) {
    if (e.key === 'Escape') onclose();
  }
</script>

<svelte:window onkeydown={key} />

<div class="backdrop" role="presentation" onclick={(e) => e.target === e.currentTarget && onclose()}>
  <div class="modal" class:wide role="dialog" aria-modal="true" aria-label={title}>
    <div class="head">
      <h2>{title}</h2>
      <button class="link close" onclick={onclose} aria-label="Close">✕</button>
    </div>
    {@render children()}
  </div>
</div>

<style>
  .backdrop {
    position: fixed;
    inset: 0;
    background: rgb(0 0 0 / 35%);
    display: flex;
    align-items: flex-start;
    justify-content: center;
    padding: 6vh 16px;
    z-index: 50;
    overflow: auto;
  }
  .modal {
    background: var(--panel);
    border-radius: 12px;
    box-shadow: var(--shadow);
    padding: 20px 22px;
    width: 100%;
    max-width: 560px;
  }
  .modal.wide { max-width: 900px; }
  .head { display: flex; justify-content: space-between; align-items: center; margin-bottom: 8px; }
  .head h2 { margin: 0; }
  .close { color: var(--muted); font-size: 16px; }
</style>
