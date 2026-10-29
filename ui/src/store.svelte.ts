import { getAccent, getSettings, getSystemLanguage, listen, type Settings } from "./ipc";
import { setLang, setSystemLang } from "./i18n.svelte";

/** Settings shared by every window; kept in sync through the `settings-changed` event. */
export const app = $state<{ settings: Settings | null }>({ settings: null });

function applyTheme(s: Settings) {
  const root = document.documentElement;
  if (s.general.theme === "system") delete root.dataset.theme;
  else root.dataset.theme = s.general.theme;
  setLang(s.general.language);
}

/** Black or white, whichever reads on `hex`. */
const onColor = (hex: string) => {
  const [r, g, b] = [1, 3, 5].map((i) => parseInt(hex.slice(i, i + 2), 16));
  return 0.299 * r + 0.587 * g + 0.114 * b > 150 ? "#000" : "#fff";
};

/** The Windows accent colour replaces the built-in blue (see `--sys-accent-*` in app.css). */
async function applyAccent() {
  const accent = await getAccent().catch(() => null);
  const style = document.documentElement.style;
  const set = (name: string, value: string | null) => (value ? style.setProperty(name, value) : style.removeProperty(name));
  set("--sys-accent-light", accent?.light ?? null);
  set("--sys-on-accent-light", accent ? onColor(accent.light) : null);
  set("--sys-accent-dark", accent?.dark ?? null);
  set("--sys-on-accent-dark", accent ? onColor(accent.dark) : null);
}

export async function initSettings() {
  applyAccent();
  listen("system-theme-changed", applyAccent);
  setSystemLang(await getSystemLanguage().catch(() => "en"));
  app.settings = await getSettings();
  applyTheme(app.settings);
  await listen<Settings>("settings-changed", (e) => {
    app.settings = e.payload;
    applyTheme(e.payload);
  });
}
