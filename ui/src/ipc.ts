import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";

export type FeatureName = "brightness" | "contrast" | "volume";
export interface FeatureDto { feature: FeatureName; percent: number }
export interface CalibrationDto { feature: FeatureName; min: number; max: number }
export interface MonitorDto {
  id: string; number: number; name: string; originalName: string;
  color: string | null; icon: string | null;
  features: FeatureDto[]; probed: boolean; stale: boolean; hidden: boolean; hiddenFeatures: string[];
  excludedFromLink: boolean; calibration: CalibrationDto[];
}
export interface Snapshot { monitors: MonitorDto[]; scanning: boolean }
export interface ValueChanged { id: string; feature: FeatureName; percent: number }

export interface Range { min: number; max: number }
export interface MonitorSettings {
  customName: string | null; color: string | null; icon: string | null; hidden: boolean; order: number; hiddenFeatures: string[];
  calibration: Partial<Record<FeatureName, Range>>; excludedFromLink: boolean;
}
export interface Profile {
  id: string; name: string; icon: string;
  /** monitor id -> feature -> 0..100 */
  values: Record<string, Partial<Record<FeatureName, number>>>;
}
export type HotkeyAction =
  | { type: "flyout" }
  | { type: "brightnessAll"; delta: 1 | -1 }
  | { type: "brightness"; monitorId: string; delta: 1 | -1 }
  | { type: "volume"; monitorId: string; delta: 1 | -1 }
  | { type: "profile"; profileId: string }
  | { type: "scheduleToggle" };
export interface Hotkey { accelerator: string; action: HotkeyAction; enabled: boolean }
export interface OsdSettings { enabled: boolean; position: "top" | "bottom"; durationMs: number }
export interface OsdPayload { icon: string; label: string; percent: number | null }

export interface Location { lat: number; lon: number; label: string }
export type Trigger =
  | { type: "time"; at: string }
  | { type: "sun"; event: "sunrise" | "sunset"; offsetMin: number };
export type ScheduleAction =
  | { type: "profile"; profileId: string }
  | { type: "brightness"; value: number; monitors: string[] | null };
export interface SchedulePoint { id: string; trigger: Trigger; action: ScheduleAction }
export interface Schedule {
  enabled: boolean; transitionMin: number; pauseOnManual: "untilNext" | "minutes" | "off"; pauseMinutes: number;
  skipFullscreen: boolean; location: Location | null; points: SchedulePoint[];
}
export interface ScheduleState {
  enabled: boolean; paused: boolean; pausedUntil: string | null; next: { at: string; label: string } | null;
}
export interface PreviewPoint { id: string; minute: number; label: string; kind: "time" | "sunrise" | "sunset" }
export interface SchedulePreview {
  points: PreviewPoint[]; curve: [number, number][]; sunrise: number | null; sunset: number | null; now: number;
}

export interface Settings {
  schemaVersion: number;
  general: {
    autostart: boolean; language: string; theme: "system" | "light" | "dark"; step: number;
    showValues: "hover" | "always"; checkUpdates: boolean;
  };
  linked: { enabled: boolean; features: FeatureName[]; showIndividual: boolean };
  profiles: Profile[];
  hotkeys: Hotkey[];
  hotkeysEnabled: boolean;
  osd: OsdSettings;
  schedule: Schedule;
  ddc: { writeDelayMs: number; readDelayMs: number; retries: number };
  templatesSeeded: boolean;
  monitors: Record<string, MonitorSettings>;
}
export interface About { version: string; settingsPath: string; logPath: string; repoUrl: string; autostartRegistered: boolean }
export interface UpdateInfo { available: boolean; version: string | null; notes: string | null }

export const FEATURES: FeatureName[] = ["brightness", "contrast", "volume"];
export const FEATURE_GLYPH: Record<FeatureName, string> = { brightness: "", contrast: "", volume: "" };

/** Fluent glyphs available as profile icons. */
export const PROFILE_ICONS: Record<string, string> = {
  sun: "", moon: "", game: "", eye: "", video: "", star: "",
};

/** Badge colours offered for monitors (all readable with white text). */
export const MONITOR_COLORS = ["#c42b1c", "#d4660a", "#8a6d00", "#107c10", "#038387", "#0067c0", "#6b4fbb", "#c239b3"];

/** Fluent glyphs offered as monitor icons; no icon shows the monitor number. */
export const MONITOR_ICONS: Record<string, string> = {
  home: "", work: "", game: "", tv: "", video: "", mail: "",
  heart: "", star: "", pin: "", globe: "", camera: "", music: "",
};

/** Icon key that shows the monitor number as the badge; no icon at all (the default) hides the badge. */
export const NUMBER_BADGE = "number";

/** Style for a monitor badge: the chosen colour (if any, the badge gradient is built from it) with white text. */
export const badgeStyle = (m: { color: string | null }) =>
  m.color ? `--b:${m.color};color:#fff` : undefined;

export const getMonitors =() => invoke<Snapshot>("get_monitors");
export const refreshMonitors = () => invoke<void>("refresh_monitors");
export const reloadValues = () => invoke<void>("reload_values");
export const setValue = (monitorId: string, feature: FeatureName, value: number) =>
  invoke<void>("set_value", { monitorId, feature, value });
export const setLinkedValue = (feature: FeatureName, value: number) =>
  invoke<void>("set_linked_value", { feature, value });
export const applyProfile = (id: string) => invoke<boolean>("apply_profile", { id });
export const saveCurrentAsProfile = (name: string, icon?: string) =>
  invoke<Settings>("save_current_as_profile", { name, icon });
export const fitFlyout = (height: number) => invoke<void>("fit_flyout", { height });
export const hideFlyout = () => invoke<void>("hide_flyout");
export const isSolid = () => invoke<boolean>("is_solid");
export const getSettings = () => invoke<Settings>("get_settings");
/** JSON merge patch: objects merge, `null` deletes a key, arrays are replaced whole. */
export const updateSettings = (patch: unknown) => invoke<Settings>("update_settings", { patch });
export const openSettings = () => invoke<void>("open_settings");
export const takeNotice = () => invoke<string | null>("take_notice");
export const getScheduleState = () => invoke<ScheduleState>("get_schedule_state");
export const pauseSchedule = (minutes?: number) => invoke<void>("pause_schedule", { minutes });
export const resumeSchedule = () => invoke<void>("resume_schedule");
export const getSchedulePreview = () => invoke<SchedulePreview>("get_schedule_preview");
export const getHotkeyStatus = () => invoke<Record<string, string>>("get_hotkey_status");
export const hotkeysPause = (paused: boolean) => invoke<void>("hotkeys_pause", { paused });
export const osdTakePending = () => invoke<OsdPayload | null>("osd_take_pending");
export const checkForUpdates = () => invoke<UpdateInfo>("check_for_updates");
export const installUpdate = () => invoke<void>("install_update");
export const openRepo = () => invoke<void>("open_repo");
export const exportSettings = () => invoke<boolean>("export_settings");
export const importSettings = () => invoke<boolean>("import_settings");
export const resetSettings = () => invoke<void>("reset_settings");
export const openLogs = () => invoke<void>("open_logs");
export const getAbout = () => invoke<About>("about");

export { listen };

/** Mirror of `lumza_core::scale::uncalibrate`: monitor percent -> common percent. */
export function uncalibrate(percent: number, min: number, max: number): number {
  if (max <= min) return 0;
  const p = Math.min(Math.max(percent, min), max) - min;
  return Math.floor((p * 100 + (max - min) / 2) / (max - min));
}
