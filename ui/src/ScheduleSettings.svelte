<script lang="ts">
  import { onMount } from "svelte";
  import {
    getSchedulePreview, getScheduleState, listen, pauseSchedule, resumeSchedule, updateSettings,
    type MonitorDto, type SchedulePoint, type SchedulePreview, type ScheduleState,
  } from "./ipc";
  import { CITIES, type City } from "./cities";
  import { app } from "./store.svelte";
  import { t } from "./i18n.svelte";
  import { i18n } from "./i18n.svelte";

  let { monitors }: { monitors: MonitorDto[] } = $props();

  let preview = $state<SchedulePreview | null>(null);
  let schedState = $state<ScheduleState | null>(null);

  const sc = $derived(app.settings?.schedule);
  const profiles = $derived(app.settings?.profiles ?? []);

  async function refresh() {
    preview = await getSchedulePreview();
    schedState = await getScheduleState();
  }

  onMount(() => {
    refresh();
    const un = Promise.all([
      listen("settings-changed", () => refresh()),
      listen("monitors-changed", () => refresh()),
      listen<ScheduleState>("schedule-state", (e) => { schedState = e.payload; refresh(); }),
    ]);
    const clock = setInterval(refresh, 60_000); // moves the "now" marker; the window is only open briefly
    return () => { un.then((f) => f.forEach((x) => x())); clearInterval(clock); };
  });

  const set = (patch: Record<string, unknown>) => updateSettings({ schedule: patch });
  const savePoints = (points: SchedulePoint[]) => set({ points: $state.snapshot(points) });

  const hhmm = (m: number | null) => {
    if (m === null) return "—";
    const v = Math.floor(((m % 1440) + 1440) % 1440);
    return `${String(Math.floor(v / 60)).padStart(2, "0")}:${String(v % 60).padStart(2, "0")}`;
  };

  function addPoint() {
    const id = `pt-${Date.now()}`;
    const first = profiles[0];
    const point: SchedulePoint = {
      id,
      trigger: { type: "time", at: "07:00" },
      action: first ? { type: "profile", profileId: first.id } : { type: "brightness", value: 80, monitors: null },
    };
    savePoints([...(sc?.points ?? []), point]);
  }

  const removePoint = (id: string) => savePoints((sc?.points ?? []).filter((p) => p.id !== id));
  const patchPoint = (id: string, patch: Partial<SchedulePoint>) =>
    savePoints((sc?.points ?? []).map((p) => (p.id === id ? { ...$state.snapshot(p), ...patch } : p)));

  function setTriggerKind(p: SchedulePoint, kind: string) {
    patchPoint(p.id, {
      trigger: kind === "time" ? { type: "time", at: "07:00" } : { type: "sun", event: kind as "sunrise" | "sunset", offsetMin: 0 },
    });
  }

  function setActionKind(p: SchedulePoint, kind: string) {
    patchPoint(p.id, {
      action: kind === "profile"
        ? { type: "profile", profileId: profiles[0]?.id ?? "" }
        : { type: "brightness", value: 80, monitors: null },
    });
  }

  const kindOf = (p: SchedulePoint) => (p.trigger.type === "time" ? "time" : p.trigger.event);

  function setCity(name: string) {
    const c = CITIES.find((x) => x.name === name);
    if (c) set({ location: { lat: c.lat, lon: c.lon, label: cityName(c) } });
  }

  function setCoord(which: "lat" | "lon", raw: string) {
    const v = Number(raw.replace(",", "."));
    if (Number.isNaN(v)) return;
    const cur = sc?.location ?? { lat: 0, lon: 0, label: "" };
    set({ location: { ...cur, [which]: v, label: cur.label } });
  }

  const cityName = (c: City) => (i18n.lang === "ru" ? c.name : c.en);
  const cityValue = $derived(CITIES.find((c) => c.lat === sc?.location?.lat && c.lon === sc?.location?.lon)?.name ?? "");
  const needsLocation = $derived((sc?.points ?? []).some((p) => p.trigger.type === "sun") && !sc?.location);

  // ---- day chart geometry
  const W = 600, H = 120, PAD = 6;
  const x = (m: number) => (m / 1440) * W;
  const y = (v: number) => PAD + (1 - v / 100) * (H - 2 * PAD);
  const curvePath = $derived(
    preview && preview.curve.length
      ? preview.curve.map(([m, v], i) => `${i ? "L" : "M"}${x(m).toFixed(1)},${y(v).toFixed(1)}`).join(" ")
      : "",
  );
</script>

<h2>{t("sched.title")}</h2>

<div class="group">
  <label class="line">
    <span><b>{t("sched.enable")}</b><small>{t("sched.enableHint")}</small></span>
    <input type="checkbox" class="switch" role="switch" checked={sc?.enabled ?? false} onchange={(e) => set({ enabled: e.currentTarget.checked })} />
  </label>
  {#if sc?.enabled && schedState?.paused}
    <div class="line">
      <span><b>{schedState.pausedUntil ? t("sched.pausedUntil", { time: schedState.pausedUntil }) : t("sched.paused")}</b></span>
      <button onclick={() => resumeSchedule()}>{t("common.resume")}</button>
    </div>
  {:else if sc?.enabled && schedState?.next}
    <div class="line">
      <span><b>{t("sched.next", { at: schedState.next.at })}</b><small>{schedState.next.label}</small></span>
      <button onclick={() => pauseSchedule()}>{t("sched.pause")}</button>
    </div>
  {/if}
</div>

<div class="group">
  <div class="sched-day">
    {#if preview}
      <svg viewBox="0 0 {W} {H + 18}" role="img" aria-label={t("sched.chartAria")}>
        {#if preview.sunrise !== null && preview.sunset !== null}
          {#if preview.sunrise < preview.sunset}
            <rect class="night" x="0" y={PAD} width={x(preview.sunrise)} height={H - 2 * PAD} opacity=".35" />
            <rect class="night" x={x(preview.sunset)} y={PAD} width={W - x(preview.sunset)} height={H - 2 * PAD} opacity=".35" />
          {:else}
            <rect class="night" x={x(preview.sunset)} y={PAD} width={x(preview.sunrise) - x(preview.sunset)} height={H - 2 * PAD} opacity=".35" />
          {/if}
        {/if}
        <line class="axis" x1="0" y1={y(0)} x2={W} y2={y(0)} />
        <line class="axis" x1="0" y1={y(100)} x2={W} y2={y(100)} />
        {#each [0, 6, 12, 18, 24] as h (h)}
          <text x={x(h * 60)} y={H + 12} text-anchor={h === 0 ? "start" : h === 24 ? "end" : "middle"}>{String(h).padStart(2, "0")}:00</text>
        {/each}
        {#if curvePath}<path class="curve" d={curvePath} />{/if}
        {#each preview.points as p (p.id)}
          <line class="pt" x1={x(p.minute)} y1={y(100)} x2={x(p.minute)} y2={y(0)} />
          <circle class="dot" cx={x(p.minute)} cy={y(100) + 2} r="4"><title>{hhmm(p.minute)} — {p.label}</title></circle>
        {/each}
        <line class="nowline" x1={x(preview.now)} y1={y(100)} x2={x(preview.now)} y2={y(0)} />
      </svg>
      <div class="sched-legend" style="padding: 4px 0 0">
        <span>{t("sched.nowLegend", { time: hhmm(preview.now) })}</span>
        {#if preview.sunrise !== null}<span>{t("sched.sunriseAt", { time: hhmm(preview.sunrise) })}</span>{/if}
        {#if preview.sunset !== null}<span>{t("sched.sunsetAt", { time: hhmm(preview.sunset) })}</span>{/if}
        {#if !preview.curve.length}<span>{t("sched.noCurve")}</span>{/if}
      </div>
    {/if}
  </div>
</div>

<div class="group">
  <div class="line">
    <span><b>{t("sched.points")}</b><small>{t("sched.pointsHint")}</small></span>
    <button onclick={addPoint}>{t("sched.addPoint")}</button>
  </div>
  {#each sc?.points ?? [] as p (p.id)}
    <div class="prow">
      <select aria-label={t("sched.when")} value={kindOf(p)} onchange={(e) => setTriggerKind(p, e.currentTarget.value)}>
        <option value="time">{t("sched.atTime")}</option>
        <option value="sunrise">{t("sched.sunrise")}</option>
        <option value="sunset">{t("sched.sunset")}</option>
      </select>
      {#if p.trigger.type === "time"}
        <input type="time" aria-label={t("sched.time")} value={p.trigger.at}
          onchange={(e) => e.currentTarget.value && patchPoint(p.id, { trigger: { type: "time", at: e.currentTarget.value } })} />
      {:else}
        {@const trig = p.trigger}
        <span class="inline">
          <input type="number" min="-720" max="720" step="5" value={trig.offsetMin} aria-label={t("sched.offsetAria")}
            onchange={(e) => patchPoint(p.id, { trigger: { ...trig, offsetMin: Math.round(e.currentTarget.valueAsNumber) || 0 } })} />
          <span>{t("unit.min")}</span>
        </span>
      {/if}
      <span class="act">
        <select aria-label={t("sched.what")} value={p.action.type} onchange={(e) => setActionKind(p, e.currentTarget.value)}>
          <option value="profile">{t("sched.optProfile")}</option>
          <option value="brightness">{t("sched.optBrightness")}</option>
        </select>
        {#if p.action.type === "profile"}
          {@const a = p.action}
          <select aria-label={t("sched.profileAria")} value={a.profileId} onchange={(e) => patchPoint(p.id, { action: { type: "profile", profileId: e.currentTarget.value } })}>
            {#each profiles as pr (pr.id)}<option value={pr.id}>{pr.name}</option>{/each}
            {#if !profiles.some((pr) => pr.id === a.profileId)}<option value={a.profileId}>{t("sched.profileGone")}</option>{/if}
          </select>
        {:else}
          {@const a = p.action}
          <input type="number" min="0" max="100" value={a.value} aria-label={t("sched.brightnessAria")}
            onchange={(e) => patchPoint(p.id, { action: { ...a, value: Math.max(0, Math.min(100, Math.round(e.currentTarget.valueAsNumber) || 0)) } })} />
          <span>%</span>
          <select aria-label={t("sched.monitorsAria")} value={a.monitors?.[0] ?? ""}
            onchange={(e) => patchPoint(p.id, { action: { ...a, monitors: e.currentTarget.value ? [e.currentTarget.value] : null } })}>
            <option value="">{t("sched.allMonitors")}</option>
            {#each monitors as m (m.id)}<option value={m.id}>{m.number}. {m.name}</option>{/each}
          </select>
        {/if}
      </span>
      <button class="icon" title={t("sched.deletePoint")} aria-label={t("sched.deletePoint")} onclick={() => removePoint(p.id)}></button>
    </div>
  {:else}
    <p class="muted">{t("sched.noPoints")}</p>
  {/each}
</div>

<div class="group">
  <label class="line">
    <span><b>{t("sched.transition")}</b><small>{t("sched.transitionHint")}</small></span>
    <span class="stepper">
      <input type="range" min="0" max="60" step="1" value={sc?.transitionMin ?? 10} style:--fill="{((sc?.transitionMin ?? 10) / 60) * 100}%"
        aria-label={t("sched.transitionAria")} onchange={(e) => set({ transitionMin: e.currentTarget.valueAsNumber })} />
      <output>{sc?.transitionMin ?? 10} {t("unit.min")}</output>
    </span>
  </label>
  <label class="line">
    <span><b>{t("sched.manual")}</b><small>{t("sched.manualHint")}</small></span>
    <select value={sc?.pauseOnManual ?? "untilNext"} onchange={(e) => set({ pauseOnManual: e.currentTarget.value })}>
      <option value="untilNext">{t("sched.pauseUntilNext")}</option>
      <option value="minutes">{t("sched.pauseMinutes")}</option>
      <option value="off">{t("sched.pauseOff")}</option>
    </select>
  </label>
  {#if sc?.pauseOnManual === "minutes"}
    <label class="line">
      <span><b>{t("sched.pauseLength")}</b></span>
      <span class="inline">
        <input type="number" min="1" max="1440" value={sc.pauseMinutes} aria-label={t("sched.minutesAria")}
          onchange={(e) => set({ pauseMinutes: Math.max(1, Math.round(e.currentTarget.valueAsNumber) || 60) })} />
        <span>{t("unit.min")}</span>
      </span>
    </label>
  {/if}
  <label class="line">
    <span><b>{t("sched.skipFullscreen")}</b><small>{t("sched.skipFullscreenHint")}</small></span>
    <input type="checkbox" class="switch" role="switch" checked={sc?.skipFullscreen ?? false} onchange={(e) => set({ skipFullscreen: e.currentTarget.checked })} />
  </label>
</div>

<div class="group">
  <div class="line">
    <span><b>{t("sched.location")}</b><small>{t("sched.locationHint")}</small></span>
    <select aria-label={t("sched.cityAria")} value={cityValue} onchange={(e) => setCity(e.currentTarget.value)}>
      <option value="" disabled>{sc?.location && !cityValue ? (sc.location.label || t("sched.customCoords")) : t("sched.pickCity")}</option>
      {#each CITIES as c (c.name)}<option value={c.name}>{cityName(c)}</option>{/each}
    </select>
  </div>
  <div class="coords">
    <label class="inline">{t("sched.lat")} <input type="number" step="0.0001" min="-90" max="90" value={sc?.location?.lat ?? ""} onchange={(e) => setCoord("lat", e.currentTarget.value)} /></label>
    <label class="inline">{t("sched.lon")} <input type="number" step="0.0001" min="-180" max="180" value={sc?.location?.lon ?? ""} onchange={(e) => setCoord("lon", e.currentTarget.value)} /></label>
    <label class="inline">{t("sched.label")} <input type="text" maxlength="40" value={sc?.location?.label ?? ""}
      onchange={(e) => set({ location: { ...(sc?.location ?? { lat: 0, lon: 0 }), label: e.currentTarget.value } })} /></label>
  </div>
  {#if needsLocation}<p class="muted" role="alert">{t("sched.needLocation")}</p>{/if}
</div>
