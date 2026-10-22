<script lang="ts">
  import { onDestroy } from "svelte";
  import { FEATURE_GLYPH, type FeatureName } from "./ipc";

  let { feature, label, value = $bindable(), send, step = 2, disabled = false, onactive, preview = null }: {
    feature: FeatureName; label: string; value: number;
    /** Called with the value to write (throttled while dragging, final value on release). */
    send: (v: number) => void;
    /** Mouse-wheel step in percent (settings). */
    step?: number;
    /** Values are from the disk cache and the monitor has not answered yet. */
    disabled?: boolean;
    /** Reports drag start/end so the owner does not overwrite the value mid-drag. */
    onactive?: (active: boolean) => void;
    /** A value to show instead of the real one (hovering a profile); nothing is written. */
    preview?: number | null;
  } = $props();

  const THROTTLE_MS = 100; // spec §3.2: ≤ 1 write / 100 ms while dragging, final value on release

  let timer: ReturnType<typeof setTimeout> | undefined;
  let queued: number | null = null;
  let active = $state(false);

  const shownValue = $derived(preview ?? value);

  const setActive = (a: boolean) => { active = a; onactive?.(a); };

  function push(v: number) {
    value = v;
    if (timer === undefined) {
      send(v);
      timer = setTimeout(() => {
        timer = undefined;
        if (queued !== null) { const q = queued; queued = null; push(q); }
      }, THROTTLE_MS);
    } else {
      queued = v;
    }
  }

  function commit() {
    clearTimeout(timer); timer = undefined; queued = null;
    send(value);
  }

  // A throttle timer outliving its slider (monitor unplugged mid-drag) would still fire a write.
  onDestroy(() => { clearTimeout(timer); timer = undefined; queued = null; });

  function onwheel(e: WheelEvent) {
    e.preventDefault();
    if (disabled) return;
    const v = Math.max(0, Math.min(100, value + (e.deltaY < 0 ? step : -step)));
    if (v !== value) { value = v; commit(); }
  }
</script>

<div class="row" class:active class:disabled class:previewing={preview !== null}>
  {#if feature === "contrast"}
    <svg class="glyph" viewBox="0 0 16 16" width="16" height="16" aria-hidden="true"><circle cx="8" cy="8" r="6" fill="none" stroke="currentColor" stroke-width="1.4"/><path d="M8 2a6 6 0 0 1 0 12z" fill="currentColor"/></svg>
  {:else}
    <span class="glyph" aria-hidden="true">{FEATURE_GLYPH[feature]}</span>
  {/if}
  <input
    type="range" min="0" max="100" step="1" value={shownValue} {disabled}
    aria-label={label}
    style:--fill="{shownValue}%"
    oninput={(e) => push(e.currentTarget.valueAsNumber)}
    onchange={() => { commit(); setActive(false); }}
    onpointerdown={() => setActive(true)}
    onpointerup={() => setActive(false)}
    {onwheel}
  />
  <span class="val">{shownValue}</span>
</div>
