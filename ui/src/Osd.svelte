<script lang="ts">
  import { onMount } from "svelte";
  import { FEATURE_GLYPH, listen, osdTakePending, PROFILE_ICONS, type OsdPayload } from "./ipc";
  import { initSettings } from "./store.svelte";

  let payload = $state<OsdPayload | null>(null);
  let tick = $state(0); // restarts the fade-in on every new event

  const show = (p: OsdPayload) => { payload = p; tick++; };

  const EXTRA: Record<string, string> = { pause: "\uE769", play: "\uE768" };
  const glyph = (icon: string) =>
    (FEATURE_GLYPH as Record<string, string>)[icon] ?? PROFILE_ICONS[icon] ?? EXTRA[icon] ?? PROFILE_ICONS.sun;

  onMount(() => {
    initSettings(); // theme
    osdTakePending().then((p) => p && show(p));
    const un = listen<OsdPayload>("osd-show", (e) => show(e.payload));
    return () => { un.then((f) => f()); };
  });
</script>

{#if payload}
  {#key tick}
    <div class="osdcard" role="status" aria-live="polite">
      {#if payload.icon === "contrast"}
        <svg class="oglyph" viewBox="0 0 16 16" width="26" height="26" aria-hidden="true"><circle cx="8" cy="8" r="6" fill="none" stroke="currentColor" stroke-width="1.2"/><path d="M8 2a6 6 0 0 1 0 12z" fill="currentColor"/></svg>
      {:else}
        <span class="oglyph" aria-hidden="true">{glyph(payload.icon)}</span>
      {/if}
      <div class="obody">
        <div class="olabel">{payload.label}</div>
        {#if payload.percent !== null}
          <div class="obar" aria-hidden="true"><div class="ofill" style:width="{payload.percent}%"></div></div>
        {/if}
      </div>
      {#if payload.percent !== null}<div class="oval">{payload.percent}</div>{/if}
    </div>
  {/key}
{/if}
