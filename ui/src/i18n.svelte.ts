import en from "../../locales/en.json";
import ru from "../../locales/ru.json";
import es from "../../locales/es.json";
import de from "../../locales/de.json";
import fr from "../../locales/fr.json";
import pt from "../../locales/pt.json";
import zh from "../../locales/zh.json";

/** Strings live in `locales/*.json` (shared with the Rust side); the UI language is reactive. */
const dicts: Record<string, Record<string, string>> = { en, ru, es, de, fr, pt, zh };

/** Language codes in the order they are offered in settings; each is shown in its own language. */
export const LANGUAGES = [
  { code: "en", name: "English" },
  { code: "ru", name: "Русский" },
  { code: "es", name: "Español" },
  { code: "de", name: "Deutsch" },
  { code: "fr", name: "Français" },
  { code: "pt", name: "Português" },
  { code: "zh", name: "简体中文" },
] as const;

export type Lang = (typeof LANGUAGES)[number]["code"];

const isLang = (code: string): code is Lang => LANGUAGES.some((l) => l.code === code);

function detectLang(): Lang {
  const code = typeof navigator !== "undefined" ? navigator.language.toLowerCase().slice(0, 2) : "en";
  return isLang(code) ? code : "en";
}

export const i18n = $state<{ lang: Lang; system: Lang }>({
  // Until settings arrive, follow the browser language so the first paint is not in the wrong one.
  lang: detectLang(),
  system: detectLang(),
});

export function format(template: string, params?: Record<string, string | number>): string {
  return params ? template.replace(/\{(\w+)\}/g, (_, k: string) => String(params[k] ?? "")) : template;
}

/** Look up `key` for the current language; falls back to English, then to the key itself. */
export function t(key: string, params?: Record<string, string | number>): string {
  return format(dicts[i18n.lang]?.[key] ?? dicts.en[key] ?? key, params);
}

/** The language the system reports (from the backend, so the UI agrees with the tray); English if unsupported. */
export function setSystemLang(lang: string) {
  i18n.system = isLang(lang) ? lang : "en";
}

/** `lang` is a language code, or "auto" for the system language. */
export function setLang(lang: string) {
  i18n.lang = lang === "auto" ? i18n.system : isLang(lang) ? lang : "en";
  if (typeof document !== "undefined") document.documentElement.lang = i18n.lang;
}
