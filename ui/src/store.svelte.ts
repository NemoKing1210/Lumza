import { getSettings, getSystemLanguage, listen, type Settings } from "./ipc";
import { setLang, setSystemLang } from "./i18n.svelte";

/** Settings shared by every window; kept in sync through the `settings-changed` event. */
export const app = $state<{ settings: Settings | null }>({ settings: null });

function applyTheme(s: Settings) {
  const root = document.documentElement;
  if (s.general.theme === "system") delete root.dataset.theme;
  else root.dataset.theme = s.general.theme;
  setLang(s.general.language);
}

export async function initSettings() {
  setSystemLang(await getSystemLanguage().catch(() => "en"));
  app.settings = await getSettings();
  applyTheme(app.settings);
  await listen<Settings>("settings-changed", (e) => {
    app.settings = e.payload;
    applyTheme(e.payload);
  });
}
