<script lang="ts">
  import { onMount, tick } from "svelte";
  import {
    applyProfile, fitFlyout, getMonitors, hideFlyout, isSolid, listen, openSettings, PROFILE_ICONS, MONITOR_ICONS, badgeStyle,
    getScheduleState, refreshMonitors, reloadValues, resumeSchedule, saveCurrentAsProfile, setHdr, setLinkedValue, setValue,
    takeNotice, uncalibrate, updateSettings,
    type FeatureName, type FeatureDto, type MonitorDto, type ScheduleState, type ValueChanged,
  } from "./ipc";
  import { app, initSettings } from "./store.svelte";
  import Slider from "./Slider.svelte";
  import { t } from "./i18n.svelte";

  let monitors = $state<MonitorDto[]>([]);
  let scanning = $state(true);
  let shown = $state(false);
  let alt = $state(false); // alternates so the entrance animation restarts on every show
  let solid = $state(false);
  let hdrEpoch = $state(0);
  let toast = $state("");
  let toastTimer: ReturnType<typeof setTimeout> | undefined;
  let root: HTMLElement;

  let sched = $state<ScheduleState | null>(null);
  let showProfiles = $state(false);
  let newName = $state("");
  let newIcon = $state("sun");
  let adding = $state(false);
  let appliedId = $state<string | null>(null);
  /** Profile under the pointer: its values are previewed on the sliders. */
  let previewId = $state<string | null>(null);
  let appliedTimer: ReturnType<typeof setTimeout> | undefined;
  let common = $state<Record<string, number>>({});
  const dragging: Record<string, boolean> = {};

  const visible = $derived(monitors.filter((m) => !m.hidden));
  const step = $derived(app.settings?.general.step ?? 2);
  const profiles = $derived(app.settings?.profiles ?? []);

  /** Monitors that follow the common slider of a feature. */
  const participants = (f: FeatureName) =>
    monitors.filter((m) => !m.excludedFromLink && !m.stale && m.features.some((x) => x.feature === f));

  /** Linked features that really have something to link (≥ 2 monitors). */
  const linkedFeatures = $derived(
    app.settings?.linked.enabled
      ? app.settings.linked.features.filter((f) => participants(f).length >= 2)
      : [],
  );

  const previewProfile = $derived(profiles.find((p) => p.id === previewId));
  const previewOf = (m: MonitorDto, f: FeatureName): number | null => previewProfile?.values[m.id]?.[f] ?? null;
  /** Common slider preview: mean of the previewed values of the participants, like the common value itself. */
  function previewCommon(f: FeatureName): number | null {
    const vals = participants(f).flatMap((m) => {
      const v = previewOf(m, f);
      if (v === null) return [];
      const c = m.calibration.find((x) => x.feature === f);
      return [uncalibrate(v, c?.min ?? 0, c?.max ?? 100)];
    });
    return vals.length ? Math.round(vals.reduce((a, b) => a + b, 0) / vals.length) : null;
  }

  const isLinked = (m: MonitorDto, f: FeatureName) => linkedFeatures.includes(f) && !m.excludedFromLink;

  const rows = (m: MonitorDto): FeatureDto[] =>
    m.features.filter(
      (f) => !m.hiddenFeatures.includes(f.feature) && (!isLinked(m, f.feature) || app.settings?.linked.showIndividual),
    );

  // Common value = mean of the participants' un-calibrated values, unless it is being dragged.
  $effect(() => {
    for (const f of linkedFeatures) {
      if (dragging[f]) continue;
      const vals = participants(f).map((m) => {
        const c = m.calibration.find((x) => x.feature === f);
        return uncalibrate(m.features.find((x) => x.feature === f)!.percent, c?.min ?? 0, c?.max ?? 100);
      });
      if (vals.length) common[f] = Math.round(vals.reduce((a, b) => a + b, 0) / vals.length);
    }
  });

  function showToast(msg: string, ms = 2500) {
    toast = msg;
    clearTimeout(toastTimer);
    toastTimer = setTimeout(() => (toast = ""), ms);
  }

  // `_deps` is never read: listing the state here makes the effect below re-run when any of it changes.
  async function fit(..._deps: unknown[]) {
    await tick();
    if (root) fitFlyout(Math.ceil(root.getBoundingClientRect().height));
  }

  $effect(() => { fit(visible, scanning, toast, showProfiles, linkedFeatures, profiles, sched); });

  async function saveProfile() {
    const name = newName.trim();
    if (!name) return;
    await saveCurrentAsProfile(name, newIcon);
    newName = "";
    newIcon = "sun";
    adding = false;
    showProfiles = false;
    showToast(t("app.profileSaved", { name }));
  }

  function pickProfile(id: string) {
    applyProfile(id);
    // Confirm the pick on the tile, then fold the drawer away.
    appliedId = id;
    previewId = null;
    clearTimeout(appliedTimer);
    appliedTimer = setTimeout(() => { appliedId = null; showProfiles = false; adding = false; }, 650);
  }

  onMount(() => {
    // Content can change height without any of the tracked state changing (fonts, wrapping, toast).
    const ro = new ResizeObserver(() => fit());
    ro.observe(root);
    initSettings();
    isSolid().then((s) => (solid = s));
    getMonitors().then((s) => { monitors = s.monitors; scanning = s.scanning; });
    takeNotice().then((n) => n && showToast(n, 8000));
    getScheduleState().then((s) => (sched = s));

    const un = Promise.all([
      listen<{ monitors: MonitorDto[]; scanning: boolean }>("monitors-changed", (e) => {
        monitors = e.payload.monitors; scanning = e.payload.scanning;
        hdrEpoch++; // a switch the user flipped may disagree with Windows' answer, which can be unchanged
      }),
      listen<ValueChanged>("values-changed", (e) => {
        const f = monitors.find((m) => m.id === e.payload.id)?.features.find((x) => x.feature === e.payload.feature);
        if (f) f.percent = e.payload.percent;
      }),
      listen<ScheduleState>("schedule-state", (e) => (sched = e.payload)),
      listen("write-failed", () => showToast(t("app.writeFailed"))),
      listen("hdr-failed", () => showToast(t("app.hdrFailed"))),
      listen("flyout-shown", () => { shown = false; alt = !alt; setTimeout(() => (shown = true), 30); reloadValues(); }),
      listen("flyout-hidden", () => { shown = false; showProfiles = false; adding = false; appliedId = null; previewId = null; }),
    ]);
    return () => { ro.disconnect(); un.then((fns) => fns.forEach((f) => f())); };
  });

  const onkey = (e: KeyboardEvent) => {
    if (e.key !== "Escape") return;
    if (adding) adding = false;
    else if (showProfiles) showProfiles = false;
    else hideFlyout();
  };
</script>

<svelte:window onkeydown={onkey} />

<main bind:this={root} class:shown class:solid class:alt class:always={app.settings?.general.showValues === "always"}>
  {#if sched?.enabled && sched.paused}
    <section class="banner stagger" style="--i: 1" role="status">
      <span>{sched.pausedUntil ? t("app.schedulePausedUntil", { time: sched.pausedUntil }) : t("app.schedulePaused")}</span>
      <button class="link" onclick={() => resumeSchedule()}>{t("common.resume")}</button>
    </section>
  {/if}
  {#if scanning && monitors.length === 0}
    {#each [0, 1] as i (i)}
      <section class="card" aria-hidden="true"><div class="bar w40"></div><div class="bar"></div><div class="bar"></div></section>
    {/each}
  {:else if monitors.length === 0}
    <section class="empty">
      <div class="big" aria-hidden="true"></div>
      <p>{t("app.noMonitors")}</p>
      <button onclick={() => refreshMonitors()}>{t("common.refresh")}</button>
    </section>
  {:else if visible.length === 0}
    <section class="empty">
      <p>{t("app.allHidden")}</p>
      <button onclick={() => openSettings()}>{t("app.openSettings")}</button>
    </section>
  {:else}
    {#if linkedFeatures.length > 0}
      <section class="card stagger" style="--i: 1" aria-label={t("app.allMonitors")}>
        <header>
          <span class="badge link" aria-hidden="true"></span>
          <span class="name">{t("app.allMonitors")}</span>
        </header>
        {#each linkedFeatures as f (f)}
          <Slider
            feature={f} label={t("app.sliderAll", { feature: t('feature.' + f) })} {step}
            bind:value={common[f]}
            preview={previewCommon(f)}
            send={(v) => setLinkedValue(f, v)}
            onactive={(a) => (dragging[f] = a)}
          />
        {/each}
      </section>
    {/if}

    {#each visible as m, idx (m.id)}
      {@const shownRows = rows(m)}
      {#if m.features.length === 0 || shownRows.length > 0}
        <section class="card stagger" style="--i: {idx + 1 + (linkedFeatures.length > 0 ? 1 : 0)}" aria-label={m.name}>
          <header>
            {#if m.icon}
              <span class="badge" class:icon={MONITOR_ICONS[m.icon]} style={badgeStyle(m)} aria-hidden="true">{MONITOR_ICONS[m.icon] ?? m.number}</span>
            {/if}
            <span class="name">{m.name}</span>
          </header>
          {#if !m.probed && m.features.length === 0}
            <div class="bar"></div><div class="bar"></div>
          {:else if m.features.length === 0}
            <p class="note">{t("app.noDdc")}</p>
          {:else}
            {#each shownRows as f (f.feature)}
              <Slider
                feature={f.feature} label="{t('feature.' + f.feature)} {m.name}" {step}
                disabled={m.stale}
                bind:value={f.percent}
                preview={previewOf(m, f.feature)}
                send={(v) => setValue(m.id, f.feature, v)}
              />
            {/each}
          {/if}
          {#if m.hdr}
            <!-- Re-created on every snapshot: Svelte skips an unchanged `checked`, which would leave a flipped switch wrong. -->
            {#key hdrEpoch}
              <label class="hdr">
                <span>{t("app.hdr")}</span>
                <input type="checkbox" class="switch" role="switch" aria-label="{t('app.hdr')} {m.name}"
                  checked={m.hdr.enabled} disabled={m.stale}
                  onchange={(e) => setHdr(m.id, e.currentTarget.checked)} />
              </label>
            {/key}
          {/if}
        </section>
      {/if}
    {/each}
  {/if}

  <div class="drawer" class:open={showProfiles} inert={!showProfiles}>
    <div class="drawer-inner">
      <section class="card pcard" aria-label={t("app.profiles")}>
        <div class="ptiles">
          {#each profiles as p, k (p.id)}
            <button class="ptile" class:done={appliedId === p.id} style="--k: {k}" onclick={() => pickProfile(p.id)}
              onpointerenter={() => (previewId = p.id)} onpointerleave={() => (previewId = null)}
              onfocus={() => (previewId = p.id)} onblur={() => (previewId = null)}>
              <span class="pic" aria-hidden="true">{appliedId === p.id ? "" : (PROFILE_ICONS[p.icon] ?? PROFILE_ICONS.sun)}</span>
              <span class="pname">{p.name}</span>
            </button>
          {/each}
          <button class="ptile add" class:on={adding} style="--k: {profiles.length}" aria-expanded={adding}
            onclick={() => (adding = !adding)}>
            <span class="pic" aria-hidden="true">{adding ? "" : ""}</span>
            <span class="pname">{t("app.profileNew")}</span>
          </button>
        </div>
        {#if profiles.length === 0 && !adding}<p class="note">{t("app.noProfiles")}</p>{/if}
        {#if adding}
          <form class="padd" onsubmit={(e) => { e.preventDefault(); saveProfile(); }}>
            <div class="picks" role="radiogroup" aria-label={t("app.profiles")}>
              {#each Object.entries(PROFILE_ICONS) as [id, glyph] (id)}
                <button type="button" class="icon pick" class:on={newIcon === id} role="radio" aria-checked={newIcon === id}
                  aria-label={id} onclick={() => (newIcon = id)}>{glyph}</button>
              {/each}
            </div>
            <div class="save">
              <!-- svelte-ignore a11y_autofocus -->
              <input type="text" autofocus bind:value={newName} placeholder={t("app.profilePlaceholder")} aria-label={t("app.profileNameAria")} maxlength="40" />
              <button type="submit" class="primary" disabled={!newName.trim()}>{t("common.save")}</button>
            </div>
          </form>
        {/if}
      </section>
    </div>
  </div>

  <footer class="stagger" style="--i: {visible.length + 2}">
    <button class="icon" class:on={showProfiles} title={t("app.profiles")} aria-label={t("app.profiles")} aria-expanded={showProfiles}
      onclick={() => (showProfiles = !showProfiles)}></button>
    <button class="icon" class:on={app.settings?.linked.enabled} title={t("app.linkMonitors")} aria-label={t("app.linkMonitors")}
      aria-pressed={app.settings?.linked.enabled ?? false}
      onclick={() => updateSettings({ linked: { enabled: !app.settings?.linked.enabled } })}></button>
    <span class="spacer"></span>
    <button class="icon" title={t("app.refreshMonitors")} aria-label={t("app.refreshMonitors")} onclick={() => refreshMonitors()}></button>
    <button class="icon" title={t("app.settings")} aria-label={t("app.settings")} onclick={() => openSettings()}></button>
  </footer>
  {#if toast}<div class="toast" role="status">{toast}</div>{/if}
</main>
