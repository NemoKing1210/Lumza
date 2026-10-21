<script lang="ts">
  import { onMount } from "svelte";
  import { getCurrentWindow } from "@tauri-apps/api/window";
  import Mark from "./Mark.svelte";
  import { t } from "./i18n.svelte";

  const win = getCurrentWindow();
  let maximized = $state(false);

  onMount(() => {
    const sync = async () => (maximized = await win.isMaximized());
    sync();
    const un = win.onResized(sync);
    return () => { un.then((f) => f()); };
  });
</script>

<header class="titlebar" data-tauri-drag-region>
  <div class="tb-brand">
    <Mark />
    <span class="tb-name">Lumza</span>
  </div>
  <div class="tb-controls">
    <button class="tb-btn" tabindex="-1" aria-label={t("win.minimize")} title={t("win.minimize")} onclick={() => win.minimize()}>
      <svg width="12" height="12" viewBox="0 0 12 12" fill="none" stroke="currentColor" stroke-width="1.1" stroke-linecap="round" aria-hidden="true"><path d="M2.5 6h7" /></svg>
    </button>
    <button class="tb-btn" tabindex="-1" aria-label={t(maximized ? "win.restore" : "win.maximize")} title={t(maximized ? "win.restore" : "win.maximize")} onclick={() => win.toggleMaximize()}>
      <svg width="12" height="12" viewBox="0 0 12 12" fill="none" stroke="currentColor" stroke-width="1.1" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true">
        {#if maximized}<rect x="2.5" y="4" width="5.5" height="5.5" rx="1" /><path d="M4.5 4V3.5a1 1 0 0 1 1-1H9a1 1 0 0 1 1 1V7a1 1 0 0 1-1 1H8.5" />{:else}<rect x="2.5" y="2.5" width="7" height="7" rx="1" />{/if}
      </svg>
    </button>
    <button class="tb-btn tb-close" tabindex="-1" aria-label={t("win.close")} title={t("win.close")} onclick={() => win.close()}>
      <svg width="12" height="12" viewBox="0 0 12 12" fill="none" stroke="currentColor" stroke-width="1.1" stroke-linecap="round" aria-hidden="true"><path d="M2.8 2.8l6.4 6.4M9.2 2.8L2.8 9.2" /></svg>
    </button>
  </div>
</header>
