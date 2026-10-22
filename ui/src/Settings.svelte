<script lang="ts">
  import { onMount } from "svelte";
  import {
    applyProfile, FEATURES, checkForUpdates, exportSettings, getAbout, importSettings, installUpdate, openLogs, openRepo, resetSettings, getHotkeyStatus, getMonitors, hotkeysPause, listen, PROFILE_ICONS, MONITOR_ICONS, MONITOR_COLORS, NUMBER_BADGE, badgeStyle,
    saveCurrentAsProfile, updateSettings,
    type About, type FeatureName, type Hotkey, type HotkeyAction, type MonitorDto, type Profile,
  } from "./ipc";
  import { app, initSettings } from "./store.svelte";
  import ScheduleSettings from "./ScheduleSettings.svelte";
  import { LANGUAGES, t } from "./i18n.svelte";
  import TitleBar from "./TitleBar.svelte";

  type Section = "general" | "link" | "monitors" | "profiles" | "schedule" | "hotkeys" | "advanced" | "about";
  const NAV: { id: Section; label: string; glyph: string }[] = [
    { id: "general", label: "set.nav.general", glyph: "" },
    { id: "monitors", label: "set.nav.monitors", glyph: "" },
    { id: "link", label: "set.nav.link", glyph: "" },
    { id: "profiles", label: "set.nav.profiles", glyph: "" },
    { id: "schedule", label: "set.nav.schedule", glyph: "\uE787" },
    { id: "hotkeys", label: "set.nav.hotkeys", glyph: "" },
    { id: "advanced", label: "set.nav.advanced", glyph: "\uE90F" },
    { id: "about", label: "set.nav.about", glyph: "" },
  ];

  let section = $state<Section>("general");
  let monitors = $state<MonitorDto[]>([]);
  let about = $state<About | null>(null);
  let openProfile = $state<string | null>(null);
  let openMonitors = $state<string[]>([]);
  const toggleMonitor = (id: string) => (openMonitors = openMonitors.includes(id) ? openMonitors.filter((x) => x !== id) : [...openMonitors, id]);
  let hkStatus = $state<Record<string, string>>({});
  let recording = $state<number | null>(null);

  onMount(() => {
    initSettings();
    getMonitors().then((s) => (monitors = s.monitors));
    getAbout().then((a) => (about = a));
    getHotkeyStatus().then((st) => (hkStatus = st));
    const un = Promise.all([
      listen<{ monitors: MonitorDto[] }>("monitors-changed", (e) => (monitors = e.payload.monitors)),
      listen<Record<string, string>>("hotkey-status", (e) => (hkStatus = e.payload)),
    ]);
    return () => { un.then((fns) => fns.forEach((f) => f())); endRecording(); };
  });

  const g = $derived(app.settings?.general);
  const linked = $derived(app.settings?.linked);
  const profiles = $derived(app.settings?.profiles ?? []);
  const setGeneral = (patch: Record<string, unknown>) => updateSettings({ general: patch });
  const setLinked = (patch: Record<string, unknown>) => updateSettings({ linked: patch });
  const setMonitor = (id: string, patch: Record<string, unknown>) => updateSettings({ monitors: { [id]: patch } });

  function commitOrder(ordered: MonitorDto[]) {
    monitors = ordered; // show the new order at once; the backend echoes it back through monitors-changed
    updateSettings({ monitors: Object.fromEntries(ordered.map((m, idx) => [m.id, { order: idx }])) });
  }

  // ---- drag to reorder (pointer events: the window's native file-drop handling swallows HTML5 drag-and-drop)
  type Drag = { from: number; to: number; dy: number; slot: number };
  let drag = $state<Drag | null>(null);
  let rects: { top: number; height: number }[] = [];

  function dragStart(e: PointerEvent, i: number) {
    if (e.button !== 0) return;
    const cards = [...document.querySelectorAll<HTMLElement>(".content > .monitor")];
    if (cards.length !== monitors.length) return;
    rects = cards.map((c) => { const r = c.getBoundingClientRect(); return { top: r.top, height: r.height }; });
    const gap = rects.length > 1 ? rects[1].top - (rects[0].top + rects[0].height) : 0;
    drag = { from: i, to: i, dy: 0, slot: rects[i].height + gap };
    const handle = e.currentTarget as HTMLElement;
    handle.setPointerCapture(e.pointerId);
    const startY = e.clientY;
    const move = (ev: PointerEvent) => {
      if (!drag) return;
      const dy = ev.clientY - startY;
      const centre = rects[drag.from].top + rects[drag.from].height / 2 + dy;
      // Its new index is the number of other cards whose centre it has passed.
      const to = rects.filter((r, k) => k !== drag!.from && r.top + r.height / 2 < centre).length;
      drag = { ...drag, dy, to };
    };
    const end = (commit: boolean) => {
      handle.removeEventListener("pointermove", move);
      handle.removeEventListener("pointerup", up);
      handle.removeEventListener("pointercancel", cancel);
      handle.removeEventListener("keydown", key);
      const d = drag;
      drag = null;
      if (commit && d && d.to !== d.from) {
        const ordered = [...monitors];
        ordered.splice(d.to, 0, ordered.splice(d.from, 1)[0]);
        commitOrder(ordered);
      }
    };
    const up = () => end(true);
    const cancel = () => end(false);
    const key = (ev: KeyboardEvent) => { if (ev.key === "Escape") end(false); };
    handle.addEventListener("pointermove", move);
    handle.addEventListener("pointerup", up);
    handle.addEventListener("pointercancel", cancel);
    handle.addEventListener("keydown", key);
  }

  /** Visual offset of card `k` while a drag is in progress: the dragged card follows the pointer, the ones it passes make room. */
  function dragShift(k: number): number {
    if (!drag) return 0;
    if (k === drag.from) return drag.dy;
    if (drag.from < drag.to && k > drag.from && k <= drag.to) return -drag.slot;
    if (drag.to < drag.from && k >= drag.to && k < drag.from) return drag.slot;
    return 0;
  }

  function toggleFeature(m: MonitorDto, feature: string, show: boolean) {
    const hidden = m.hiddenFeatures.filter((x) => x !== feature);
    if (!show) hidden.push(feature);
    setMonitor(m.id, { hiddenFeatures: hidden });
  }

  function rename(m: MonitorDto, value: string) {
    const v = value.trim();
    setMonitor(m.id, { customName: v === "" || v === m.originalName ? null : v });
  }

  function toggleLinkedFeature(f: FeatureName, on: boolean) {
    const next = (linked?.features ?? []).filter((x) => x !== f);
    if (on) next.push(f);
    setLinked({ features: FEATURES.filter((x) => next.includes(x)) });
  }

  const range = (m: MonitorDto, f: FeatureName) => app.settings?.monitors[m.id]?.calibration?.[f] ?? { min: 0, max: 100 };

  function setRange(m: MonitorDto, f: FeatureName, which: "min" | "max", raw: string) {
    const cur = range(m, f);
    let v = Math.max(0, Math.min(100, Math.round(Number(raw))));
    if (Number.isNaN(v)) return;
    const next = { ...cur, [which]: v };
    if (next.min > next.max) { if (which === "min") next.max = next.min; else next.min = next.max; }
    setMonitor(m.id, { calibration: { [f]: next } });
  }

  // ---- profiles
  const saveProfiles = (list: Profile[]) => updateSettings({ profiles: $state.snapshot(list) });
  const patchProfile = (id: string, patch: Partial<Profile>) =>
    saveProfiles(profiles.map((p) => (p.id === id ? { ...$state.snapshot(p), ...patch } : p)));

  function setProfileValue(p: Profile, monitorId: string, f: FeatureName, value: number | null) {
    const values = structuredClone($state.snapshot(p.values));
    values[monitorId] ??= {};
    if (value === null) delete values[monitorId][f]; else values[monitorId][f] = Math.max(0, Math.min(100, Math.round(value)));
    if (Object.keys(values[monitorId]).length === 0) delete values[monitorId];
    patchProfile(p.id, { values });
  }

  /** Replace the profile's values with what the monitors show right now. */
  function captureInto(p: Profile) {
    const values: Profile["values"] = structuredClone($state.snapshot(p.values));
    for (const m of monitors) {
      if (m.stale || !m.probed) continue;
      for (const f of m.features) (values[m.id] ??= {})[f.feature] = f.percent;
    }
    patchProfile(p.id, { values });
  }

  async function addProfile() {
    const s = await saveCurrentAsProfile(t("set.profileDefault", { n: profiles.length + 1 }));
    openProfile = s.profiles[s.profiles.length - 1]?.id ?? null;
  }

  function removeProfile(id: string) {
    if (openProfile === id) openProfile = null;
    saveProfiles(profiles.filter((p) => p.id !== id));
  }

  /** Features to edit for one monitor: what it supports now, or what an unplugged monitor had saved. */
  const profileFeatures = (p: Profile, mid: string, mon: MonitorDto | undefined): FeatureName[] =>
    mon ? mon.features.map((f) => f.feature) : (Object.keys(p.values[mid] ?? {}) as FeatureName[]);

  // ---- hotkeys
  const hotkeys = $derived(app.settings?.hotkeys ?? []);
  const hotkeysEnabled = $derived(app.settings?.hotkeysEnabled ?? false);
  const osd = $derived(app.settings?.osd);
  const setOsd = (patch: Record<string, unknown>) => updateSettings({ osd: patch });
  const saveHotkeys = (list: Hotkey[]) => updateSettings({ hotkeys: $state.snapshot(list) });

  const ACTIONS: { key: string; label: string }[] = [
    { key: "flyout", label: "set.act.flyout" },
    { key: "brightnessAll:1", label: "set.act.brightnessAll+" },
    { key: "brightnessAll:-1", label: "set.act.brightnessAll-" },
    { key: "brightness:1", label: "set.act.brightness+" },
    { key: "brightness:-1", label: "set.act.brightness-" },
    { key: "volume:1", label: "set.act.volume+" },
    { key: "volume:-1", label: "set.act.volume-" },
    { key: "profile", label: "set.act.profile" },
    { key: "scheduleToggle", label: "set.act.scheduleToggle" },
  ];

  const actionKey = (a: HotkeyAction) => ("delta" in a ? `${a.type}:${a.delta}` : a.type);
  const needsMonitor = (a: HotkeyAction): a is Extract<HotkeyAction, { monitorId: string }> => "monitorId" in a;

  function actionFromKey(key: string, prev: HotkeyAction): HotkeyAction {
    const [type, d] = key.split(":");
    const delta = (d === "-1" ? -1 : 1) as 1 | -1;
    const monitorId = needsMonitor(prev) ? prev.monitorId : (monitors[0]?.id ?? "");
    switch (type) {
      case "brightnessAll": return { type, delta };
      case "brightness": case "volume": return { type, monitorId, delta };
      case "profile": return { type, profileId: prev.type === "profile" ? prev.profileId : (profiles[0]?.id ?? "") };
      case "scheduleToggle": return { type: "scheduleToggle" };
      default: return { type: "flyout" };
    }
  }

  const setHotkey = (i: number, patch: Partial<Hotkey>) =>
    saveHotkeys(hotkeys.map((h, idx) => (idx === i ? { ...$state.snapshot(h), ...patch } : h)));

  const addHotkey = () => saveHotkeys([...hotkeys, { accelerator: "", action: { type: "brightnessAll", delta: 1 }, enabled: true }]);
  const removeHotkey = (i: number) => saveHotkeys(hotkeys.filter((_, idx) => idx !== i));
  const resetHotkeys = () =>
    saveHotkeys([
      { accelerator: "Ctrl+Alt+ArrowUp", action: { type: "brightnessAll", delta: 1 }, enabled: true },
      { accelerator: "Ctrl+Alt+ArrowDown", action: { type: "brightnessAll", delta: -1 }, enabled: true },
      { accelerator: "Ctrl+Alt+B", action: { type: "flyout" }, enabled: true },
    ]);

  const pretty = (accel: string) =>
    accel.replace("ArrowUp", "↑").replace("ArrowDown", "↓").replace("ArrowLeft", "←").replace("ArrowRight", "→");

  /** KeyboardEvent.code -> accelerator key name understood by the global-shortcut parser. */
  function keyName(code: string): string | null {
    if (/^Key[A-Z]$/.test(code)) return code.slice(3);
    if (/^Digit\d$/.test(code)) return code.slice(5);
    if (/^F([1-9]|1\d|2[0-4])$/.test(code)) return code;
    if (/^Arrow(Up|Down|Left|Right)$/.test(code)) return code;
    if (/^Numpad\d$/.test(code)) return code;
    const direct = ["Space", "Enter", "Tab", "Home", "End", "PageUp", "PageDown", "Insert", "Delete",
      "Minus", "Equal", "BracketLeft", "BracketRight", "Backslash", "Semicolon", "Quote", "Comma", "Period", "Slash", "Backquote"];
    return direct.includes(code) ? code : null;
  }

  function startRecording(i: number) {
    endRecording();
    recording = i;
    hotkeysPause(true); // otherwise a registered combination would fire instead of reaching this page
    window.addEventListener("keydown", onRecordKey, true);
  }

  function endRecording() {
    if (recording === null) return;
    recording = null;
    window.removeEventListener("keydown", onRecordKey, true);
    hotkeysPause(false);
  }

  function onRecordKey(e: KeyboardEvent) {
    if (recording === null) return;
    e.preventDefault();
    e.stopPropagation();
    if (["ControlLeft", "ControlRight", "AltLeft", "AltRight", "ShiftLeft", "ShiftRight", "MetaLeft", "MetaRight"].includes(e.code)) return;
    const i = recording;
    if (e.code === "Escape") return endRecording();
    if (e.code === "Backspace") { endRecording(); setHotkey(i, { accelerator: "" }); return; }
    const key = keyName(e.code);
    if (!key) return;
    const mods = [e.ctrlKey && "Ctrl", e.altKey && "Alt", e.shiftKey && "Shift", e.metaKey && "Super"].filter(Boolean) as string[];
    if (mods.length === 0 && !/^F\d+$/.test(key)) return; // a bare letter would hijack typing everywhere
    endRecording();
    setHotkey(i, { accelerator: [...mods, key].join("+") });
  }

  // ---- advanced
  let advMsg = $state("");
  let confirmReset = $state(false);
  const ddc = $derived(app.settings?.ddc);
  const setDdc = (patch: Record<string, unknown>) => updateSettings({ ddc: patch });

  async function advance(run: () => Promise<boolean | void>, okKey: string, cancelled = "") {
    advMsg = "";
    try {
      const r = await run();
      advMsg = r === false ? cancelled : t(okKey);
    } catch {
      advMsg = t(okKey === "set.adv.imported" ? "set.adv.importFailed" : "set.adv.failed");
    }
  }

  async function doReset() {
    if (!confirmReset) { confirmReset = true; return; }
    confirmReset = false;
    await advance(() => resetSettings(), "set.adv.resetDone");
  }

  // ---- about / updates
  type Phase = "idle" | "checking" | "uptodate" | "available" | "installing" | "error";
  let upd = $state<{ phase: Phase; version?: string }>({ phase: "idle" });

  async function check() {
    upd = { phase: "checking" };
    try {
      const r = await checkForUpdates();
      upd = r.available ? { phase: "available", version: r.version ?? "" } : { phase: "uptodate" };
    } catch {
      upd = { phase: "error" };
    }
  }

  async function install() {
    upd = { ...upd, phase: "installing" };
    try { await installUpdate(); } catch { upd = { phase: "error" }; }
  }

  const updText = $derived(
    upd.phase === "checking" ? t("about.checking")
    : upd.phase === "uptodate" ? t("about.upToDate")
    : upd.phase === "available" ? t("about.available", { version: upd.version ?? "" })
    : upd.phase === "installing" ? t("about.installing")
    : upd.phase === "error" ? t("about.failed")
    : t("about.check"),
  );

  const monitorName = (id: string) => monitors.find((m) => m.id === id)?.name ?? t("set.unplugged");
</script>

<div class="window">
<TitleBar />
<div class="shell">
  <nav aria-label={t("set.navAria")}>
    {#each NAV as n (n.id)}
      <button class="navitem" class:current={section === n.id} aria-current={section === n.id ? "page" : undefined} onclick={() => (section = n.id)}>
        <span class="navglyph" aria-hidden="true">{n.glyph}</span>{t(n.label)}
      </button>
    {/each}
  </nav>

  {#key section}
  <section class="content">
    {#if !g || !linked}
      <p class="muted">{t("common.loading")}</p>
    {:else if section === "general"}
      <h2>{t("set.nav.general")}</h2>
      <div class="group">
        <label class="line">
          <span><b>{t("set.autostart")}</b><small>{t("set.autostartHint")}</small></span>
          <input type="checkbox" class="switch" role="switch" checked={g.autostart} onchange={(e) => setGeneral({ autostart: e.currentTarget.checked })} />
        </label>
        <label class="line">
          <span><b>{t("set.theme")}</b><small>{t("set.themeHint")}</small></span>
          <select value={g.theme} onchange={(e) => setGeneral({ theme: e.currentTarget.value })}>
            <option value="system">{t("set.themeSystem")}</option>
            <option value="light">{t("set.themeLight")}</option>
            <option value="dark">{t("set.themeDark")}</option>
          </select>
        </label>
        <label class="line">
          <span><b>{t("set.language")}</b><small>{t("set.languageHint")}</small></span>
          <select value={g.language} onchange={(e) => setGeneral({ language: e.currentTarget.value })}>
            <option value="auto">{t("set.langAuto")}</option>
            {#each LANGUAGES as l (l.code)}
              <option value={l.code}>{l.name}</option>
            {/each}
          </select>
        </label>
        <label class="line">
          <span><b>{t("set.step")}</b><small>{t("set.stepHint")}</small></span>
          <span class="stepper">
            <input type="range" min="1" max="10" step="1" value={g.step} style:--fill="{(g.step - 1) / 9 * 100}%" aria-label={t("set.stepAria")}
              onchange={(e) => setGeneral({ step: e.currentTarget.valueAsNumber })} />
            <output>{g.step}%</output>
          </span>
        </label>
        <label class="line">
          <span><b>{t("set.showValues")}</b><small>{t("set.showValuesHint")}</small></span>
          <select value={g.showValues} onchange={(e) => setGeneral({ showValues: e.currentTarget.value })}>
            <option value="hover">{t("set.onHover")}</option>
            <option value="always">{t("set.always")}</option>
          </select>
        </label>
      </div>
    {:else if section === "monitors"}
      <h2>{t("set.nav.monitors")}</h2>
      {#if monitors.length === 0}
        <p class="muted">{t("set.noMonitors")}</p>
      {/if}
      {#each monitors as m, i (m.id)}
        {@const open = openMonitors.includes(m.id)}
        <div class="group monitor" class:dragging={drag?.from === i} class:shifting={drag !== null && drag.from !== i}
          style:transform={drag ? `translateY(${dragShift(i)}px)` : undefined}>
          <div class="mhead">
            <button class="icon grip" title={t("set.drag")} aria-label={t("set.drag")} onpointerdown={(e) => dragStart(e, i)}>&#xE700;</button>
            {#if m.icon}
              <span class="badge" class:icon={MONITOR_ICONS[m.icon]} style={badgeStyle(m)} aria-hidden="true">{MONITOR_ICONS[m.icon] ?? m.number}</span>
            {/if}
            <input class="name" type="text" value={m.name} placeholder={m.originalName} maxlength="80" aria-label={t("set.monitorNameAria", { name: m.originalName })}
              onchange={(e) => rename(m, e.currentTarget.value)} />
            <span class="order">
              <button class="icon" title={open ? t("set.collapse") : t("set.expand")} aria-label={t("set.monitorSettingsAria", { name: m.name })} aria-expanded={open}
                onclick={() => toggleMonitor(m.id)}>{open ? "" : ""}</button>
            </span>
          </div>
          {#if open}
          <div class="features icons" role="radiogroup" aria-label={t("set.monitorColorAria", { name: m.name })}>
            <b>{t("set.monitorColor")}</b>
            <button class="swatch none" class:on={!m.color} role="radio" aria-checked={!m.color} aria-label={t("set.monitorDefault")} title={t("set.monitorDefault")}
              onclick={() => setMonitor(m.id, { color: null })}></button>
            {#each MONITOR_COLORS as c (c)}
              <button class="swatch" class:on={m.color === c} role="radio" aria-checked={m.color === c} aria-label={c} style:background={c}
                onclick={() => setMonitor(m.id, { color: c })}></button>
            {/each}
          </div>
          <div class="features icons" role="radiogroup" aria-label={t("set.monitorIconAria", { name: m.name })}>
            <b>{t("set.icon")}</b>
            <button class="icon pick" class:on={!m.icon} role="radio" aria-checked={!m.icon} aria-label={t("set.monitorIconNone")} title={t("set.monitorIconNone")}
              onclick={() => setMonitor(m.id, { icon: null })}>&#xE711;</button>
            <button class="icon pick num" class:on={m.icon === NUMBER_BADGE} role="radio" aria-checked={m.icon === NUMBER_BADGE} aria-label={t("set.monitorIconNumber")} title={t("set.monitorIconNumber")}
              onclick={() => setMonitor(m.id, { icon: NUMBER_BADGE })}>{m.number}</button>
            {#each Object.entries(MONITOR_ICONS) as [key, glyph] (key)}
              <button class="icon pick" class:on={m.icon === key} role="radio" aria-checked={m.icon === key} aria-label={key}
                onclick={() => setMonitor(m.id, { icon: key })}>{glyph}</button>
            {/each}
          </div>
          <label class="line">
            <span><b>{t("set.showInFlyout")}</b></span>
            <input type="checkbox" class="switch" role="switch" checked={!m.hidden} onchange={(e) => setMonitor(m.id, { hidden: !e.currentTarget.checked })} />
          </label>
          {#if m.features.length > 0}
            <div class="features">
              <b>{t("set.availableFeatures")}</b>
              {#each m.features as f (f.feature)}
                <label class="check">
                  <input type="checkbox" checked={!m.hiddenFeatures.includes(f.feature)} onchange={(e) => toggleFeature(m, f.feature, e.currentTarget.checked)} />
                  {t('feature.' + f.feature)}
                </label>
              {/each}
            </div>
            <label class="line">
              <span><b>{t("set.inLink")}</b><small>{t("set.inLinkHint")}</small></span>
              <input type="checkbox" class="switch" role="switch" checked={!m.excludedFromLink} onchange={(e) => setMonitor(m.id, { excludedFromLink: !e.currentTarget.checked })} />
            </label>
            {#each m.features.filter((f) => linked.features.includes(f.feature)) as f (f.feature)}
              {@const r = range(m, f.feature)}
              <div class="line calib">
                <span><b>{t("set.calibTitle", { feature: t('feature.' + f.feature) })}</b><small>{t("set.calibHint")}</small></span>
                <span class="range">
                  <input type="number" min="0" max="100" value={r.min} aria-label={t("set.min")} onchange={(e) => setRange(m, f.feature, "min", e.currentTarget.value)} />
                  <span aria-hidden="true">–</span>
                  <input type="number" min="0" max="100" value={r.max} aria-label={t("set.max")} onchange={(e) => setRange(m, f.feature, "max", e.currentTarget.value)} />
                  <span>%</span>
                </span>
              </div>
            {/each}
          {:else if m.probed}
            <p class="muted">{t("set.noDdc")}</p>
          {/if}
          {/if}
        </div>
      {/each}
    {:else if section === "link"}
      <h2>{t("set.link.title")}</h2>
      <div class="group">
        <label class="line">
          <span><b>{t("set.link.enable")}</b><small>{t("set.link.enableHint")}</small></span>
          <input type="checkbox" class="switch" role="switch" checked={linked.enabled} onchange={(e) => setLinked({ enabled: e.currentTarget.checked })} />
        </label>
        <div class="features">
          <b>{t("set.link.features")}</b>
          {#each FEATURES as f (f)}
            <label class="check">
              <input type="checkbox" checked={linked.features.includes(f)} onchange={(e) => toggleLinkedFeature(f, e.currentTarget.checked)} />
              {t('feature.' + f)}
            </label>
          {/each}
        </div>
        <label class="line">
          <span><b>{t("set.link.individual")}</b><small>{t("set.link.individualHint")}</small></span>
          <input type="checkbox" class="switch" role="switch" checked={linked.showIndividual} onchange={(e) => setLinked({ showIndividual: e.currentTarget.checked })} />
        </label>
      </div>
      <p class="muted">{t("set.link.note")}</p>
    {:else if section === "profiles"}
      <h2>{t("set.nav.profiles")}</h2>
      {#each profiles as p (p.id)}
        {@const open = openProfile === p.id}
        <div class="group profile-card">
          <div class="mhead">
            <span class="glyph big-glyph" aria-hidden="true">{PROFILE_ICONS[p.icon] ?? PROFILE_ICONS.sun}</span>
            <input class="name" type="text" value={p.name} aria-label={t("set.profileNameAria")} maxlength="40"
              onchange={(e) => e.currentTarget.value.trim() && patchProfile(p.id, { name: e.currentTarget.value.trim() })} />
            <button onclick={() => applyProfile(p.id)}>{t("common.apply")}</button>
            <button class="icon" title={open ? t("set.collapse") : t("set.editValues")} aria-label={t("set.editValues")} aria-expanded={open}
              onclick={() => (openProfile = open ? null : p.id)}>{open ? "" : ""}</button>
          </div>
          {#if open}
            <div class="features icons" role="radiogroup" aria-label={t("set.iconAria")}>
              <b>{t("set.icon")}</b>
              {#each Object.entries(PROFILE_ICONS) as [key, glyph] (key)}
                <button class="icon pick" class:on={p.icon === key} role="radio" aria-checked={p.icon === key} aria-label={key}
                  onclick={() => patchProfile(p.id, { icon: key })}>{glyph}</button>
              {/each}
            </div>
            {#each [...new Set([...Object.keys(p.values), ...monitors.map((m) => m.id)])] as mid (mid)}
              {@const mon = monitors.find((m) => m.id === mid)}
              <div class="pvalues">
                <b>{monitorName(mid)}</b>
                {#each profileFeatures(p, mid, mon) as f (f)}
                  {@const v = p.values[mid]?.[f]}
                  <div class="pvrow">
                    <label class="check">
                      <input type="checkbox" checked={v !== undefined} onchange={(e) => setProfileValue(p, mid, f, e.currentTarget.checked ? (mon?.features.find((x) => x.feature === f)?.percent ?? 50) : null)} />
                      {t('feature.' + f)}
                    </label>
                    <input type="range" min="0" max="100" value={v ?? 0} disabled={v === undefined} style:--fill="{v ?? 0}%"
                      aria-label="{t('feature.' + f)}, {monitorName(mid)}" onchange={(e) => setProfileValue(p, mid, f, e.currentTarget.valueAsNumber)} />
                    <output>{v === undefined ? t("set.keep") : v + "%"}</output>
                  </div>
                {/each}
              </div>
            {/each}
            <div class="line actions">
              <button onclick={() => captureInto(p)}>{t("set.captureCurrent")}</button>
              <button class="danger" onclick={() => removeProfile(p.id)}>{t("set.deleteProfile")}</button>
            </div>
          {/if}
        </div>
      {:else}
        <p class="muted">{t("set.noProfiles")}</p>
      {/each}
      <div><button onclick={addProfile}>{t("set.newProfile")}</button></div>
      <p class="muted">{t("set.profileNote")}</p>
    {:else if section === "schedule"}
      <ScheduleSettings {monitors} />
    {:else if section === "hotkeys"}
      <h2>{t("set.nav.hotkeys")}</h2>
      <div class="group">
        <label class="line">
          <span><b>{t("set.hotkeys.enable")}</b><small>{t("set.hotkeys.enableHint")}</small></span>
          <input type="checkbox" class="switch" role="switch" checked={hotkeysEnabled}
            onchange={(e) => updateSettings({ hotkeysEnabled: e.currentTarget.checked })} />
        </label>
      </div>
      <div class="group" class:off={!hotkeysEnabled}>
        {#each hotkeys as h, i (i)}
          <div class="hkrow" class:off={!h.enabled}>
            <input type="checkbox" class="switch" role="switch" aria-label={t("set.hotkeys.rowEnable")} checked={h.enabled}
              onchange={(e) => setHotkey(i, { enabled: e.currentTarget.checked })} />
            <select aria-label={t("set.action")} value={actionKey(h.action)} onchange={(e) => setHotkey(i, { action: actionFromKey(e.currentTarget.value, h.action) })}>
              {#each ACTIONS as a (a.key)}<option value={a.key}>{t(a.label)}</option>{/each}
            </select>
            {#if needsMonitor(h.action)}
              {@const mid = h.action.monitorId}
              <select aria-label={t("set.monitor")} value={h.action.monitorId}
                onchange={(e) => setHotkey(i, { action: { ...h.action, monitorId: e.currentTarget.value } as HotkeyAction })}>
                {#each monitors as m (m.id)}<option value={m.id}>{m.number}. {m.name}</option>{/each}
                {#if !monitors.some((m) => m.id === mid)}<option value={mid}>{monitorName(mid)}</option>{/if}
              </select>
            {:else if h.action.type === "profile"}
              <select aria-label={t("set.profile")} value={h.action.profileId}
                onchange={(e) => setHotkey(i, { action: { type: "profile", profileId: e.currentTarget.value } })}>
                {#each profiles as p (p.id)}<option value={p.id}>{p.name}</option>{/each}
              </select>
            {:else}
              <span></span>
            {/if}
            <button class="recorder" class:rec={recording === i} class:unset={!h.accelerator && recording !== i}
              aria-label={t("set.recordAria")} title={t("set.recordTitle")} onclick={() => (recording === i ? endRecording() : startRecording(i))}>
              {recording === i ? t("set.pressKeys") : h.accelerator ? pretty(h.accelerator) : t("set.unassigned")}
            </button>
            <button class="icon" title={t("common.delete")} aria-label={t("set.deleteHotkey")} onclick={() => removeHotkey(i)}></button>
            {#if h.accelerator && hkStatus[h.accelerator]}<div class="hkerr" role="alert">{hkStatus[h.accelerator]}</div>{/if}
          </div>
        {:else}
          <p class="muted">{t("set.noHotkeys")}</p>
        {/each}
        <div class="hkfoot">
          <button onclick={addHotkey}>{t("common.add")}</button>
          <button onclick={resetHotkeys}>{t("set.resetDefaults")}</button>
        </div>
      </div>
      <p class="muted">{t("set.hotkeyNote")}</p>

      <h2>OSD</h2>
      {#if osd}
        <div class="group">
          <label class="line">
            <span><b>{t("set.osd.enable")}</b><small>{t("set.osd.hint")}</small></span>
            <input type="checkbox" class="switch" role="switch" checked={osd.enabled} onchange={(e) => setOsd({ enabled: e.currentTarget.checked })} />
          </label>
          <label class="line">
            <span><b>{t("set.osd.position")}</b></span>
            <select value={osd.position} onchange={(e) => setOsd({ position: e.currentTarget.value })}>
              <option value="bottom">{t("set.osd.bottom")}</option>
              <option value="top">{t("set.osd.top")}</option>
            </select>
          </label>
          <label class="line">
            <span><b>{t("set.osd.duration")}</b></span>
            <span class="stepper">
              <input type="range" min="500" max="5000" step="250" value={osd.durationMs} style:--fill="{(osd.durationMs - 500) / 45}%"
                aria-label={t("set.osd.durationAria")} onchange={(e) => setOsd({ durationMs: e.currentTarget.valueAsNumber })} />
              <output>{(osd.durationMs / 1000).toFixed(1)} {t("unit.sec")}</output>
            </span>
          </label>
        </div>
      {/if}
    {:else if section === "advanced"}
      <h2>{t("set.nav.advanced")}</h2>
      {#if ddc}
        <div class="group">
          <div class="line"><span><b>{t("set.adv.ddc")}</b><small>{t("set.adv.ddcHint")}</small></span></div>
          <label class="line">
            <span><b>{t("set.adv.writeDelay")}</b></span>
            <input type="number" min="20" max="500" step="10" value={ddc.writeDelayMs}
              onchange={(e) => setDdc({ writeDelayMs: Math.round(e.currentTarget.valueAsNumber) || 50 })} />
          </label>
          <label class="line">
            <span><b>{t("set.adv.readDelay")}</b></span>
            <input type="number" min="20" max="500" step="10" value={ddc.readDelayMs}
              onchange={(e) => setDdc({ readDelayMs: Math.round(e.currentTarget.valueAsNumber) || 40 })} />
          </label>
          <label class="line">
            <span><b>{t("set.adv.retries")}</b></span>
            <input type="number" min="0" max="5" step="1" value={ddc.retries}
              onchange={(e) => setDdc({ retries: Math.max(0, Math.round(e.currentTarget.valueAsNumber) || 0) })} />
          </label>
        </div>
      {/if}
      <div class="group">
        <div class="line">
          <span><b>{t("about.logs")}</b><small>{t("set.adv.logsHint")}</small><small class="mono">{about?.logPath ?? ""}</small></span>
          <button onclick={() => openLogs()}>{t("set.adv.openLogs")}</button>
        </div>
        <div class="line">
          <span><b>{t("set.adv.export")}</b><small>{t("set.adv.exportHint")}</small></span>
          <button onclick={() => advance(() => exportSettings(), "set.adv.exported")}>{t("set.adv.exportBtn")}</button>
        </div>
        <div class="line">
          <span><b>{t("set.adv.import")}</b><small>{t("set.adv.importHint")}</small></span>
          <button onclick={() => advance(() => importSettings(), "set.adv.imported")}>{t("set.adv.importBtn")}</button>
        </div>
        <div class="line">
          <span><b>{t("set.adv.reset")}</b><small>{t("set.adv.resetHint")}</small></span>
          <button class="danger" onclick={doReset} onblur={() => (confirmReset = false)}>{confirmReset ? t("set.adv.resetConfirm") : t("set.adv.resetBtn")}</button>
        </div>
      </div>
      {#if advMsg}<p class="muted" role="status">{advMsg}</p>{/if}
    {:else}
      <h2>{t("set.nav.about")}</h2>
      <div class="group">
        <div class="line"><span><b>Lumza</b><small>{t("set.about.tagline")}</small></span><span class="muted">{t("about.version")} {about?.version ?? ""}</span></div>
        <div class="line">
          <span><b>{updText}</b></span>
          {#if upd.phase === "available"}
            <button onclick={install}>{t("about.install")}</button>
          {:else}
            <button onclick={check} disabled={upd.phase === "checking" || upd.phase === "installing"}>{t("about.check")}</button>
          {/if}
        </div>
        <label class="line">
          <span><b>{t("about.autoCheck")}</b><small>{t("about.autoCheckHint")}</small></span>
          <input type="checkbox" class="switch" role="switch" checked={g.checkUpdates} onchange={(e) => setGeneral({ checkUpdates: e.currentTarget.checked })} />
        </label>
        <div class="line"><span><b>{t("set.about.settingsFile")}</b><small class="mono">{about?.settingsPath ?? ""}</small></span></div>
        <div class="line"><span><b>{t("about.logs")}</b><small class="mono">{about?.logPath ?? ""}</small></span></div>
        <div class="line">
          <span><b>GitHub</b><small>{t("about.license")}</small></span>
          <button class="link" onclick={() => openRepo()}>{t("about.github")}</button>
        </div>
      </div>
    {/if}
  </section>
  {/key}
</div>
</div>
