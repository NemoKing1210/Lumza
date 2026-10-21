import { readdirSync, readFileSync } from "node:fs";
import { join } from "node:path";
import { describe, expect, it } from "vitest";
import en from "../../locales/en.json";
import ru from "../../locales/ru.json";
import es from "../../locales/es.json";
import de from "../../locales/de.json";
import fr from "../../locales/fr.json";
import pt from "../../locales/pt.json";
import zh from "../../locales/zh.json";
import { format, i18n, setLang, setSystemLang, t } from "./i18n.svelte";
import { uncalibrate } from "./ipc";

const OTHERS: Record<string, Record<string, string>> = { ru, es, de, fr, pt, zh };
const placeholders = (s: string) => [...s.matchAll(/\{(\w+)\}/g)].map((m) => m[1]).sort();

describe("locales", () => {
  it("define exactly the same keys in every language as in en.json", () => {
    for (const [lang, d] of Object.entries(OTHERS)) {
      expect(Object.keys(d).sort(), lang).toEqual(Object.keys(en).sort());
    }
  });

  it("use the same {placeholders} in every language", () => {
    for (const [lang, d] of Object.entries(OTHERS)) {
      for (const [key, text] of Object.entries(d)) {
        expect(placeholders((en as Record<string, string>)[key]), `${lang}: ${key}`).toEqual(placeholders(text));
      }
    }
  });

  it("have no empty strings", () => {
    for (const d of [en, ...Object.values(OTHERS)]) for (const [k, v] of Object.entries(d)) expect(v.trim(), k).not.toBe("");
  });

  it("contain every key the UI source refers to", () => {
    const dir = join(__dirname);
    const files = readdirSync(dir).filter((f) => /\.(svelte|ts)$/.test(f) && !f.endsWith(".test.ts"));
    const known = new Set(Object.keys(en));
    const missing: string[] = [];
    for (const f of files) {
      const src = readFileSync(join(dir, f), "utf8");
      // t("key") / t('key') calls and bare "ns.key" literals (menu tables such as NAV and ACTIONS)
      const keys = [
        ...src.matchAll(/\bt\(\s*["']([\w.+-]+)["']/g),
        ...src.matchAll(/["']((?:set|app|sched|about|common|unit)\.[\w.+-]+)["']/g),
      ].map((m) => m[1]);
      for (const k of keys) if (!k.endsWith(".") && !known.has(k)) missing.push(`${f}: ${k}`);
    }
    expect(missing).toEqual([]);
  });

  it("have a label for every feature", () => {
    for (const f of ["brightness", "contrast", "volume"]) expect(en).toHaveProperty(`feature.${f}`);
  });
});

describe("t()", () => {
  it("substitutes parameters and follows the language", () => {
    setLang("en");
    expect(t("set.profileDefault", { n: 3 })).toBe("Profile 3");
    setLang("ru");
    expect(t("set.profileDefault", { n: 3 })).toBe("Профиль 3");
    expect(i18n.lang).toBe("ru");
    setLang("de");
    expect(t("set.profileDefault", { n: 3 })).toBe("Profil 3");
    setLang("xx");
    expect(i18n.lang).toBe("en");
  });

  it("follows the system language in auto mode, English when it is unsupported", () => {
    setSystemLang("fr");
    setLang("auto");
    expect(i18n.lang).toBe("fr");
    setSystemLang("ja");
    setLang("auto");
    expect(i18n.lang).toBe("en");
    setLang("ru");
    expect(i18n.lang).toBe("ru");
  });

  it("falls back to the key for unknown strings", () => {
    expect(t("no.such.key")).toBe("no.such.key");
    expect(format("a {x} b", { x: 1 })).toBe("a 1 b");
  });
});

describe("uncalibrate", () => {
  it("matches the Rust implementation for the spec example (AOC 10-85)", () => {
    expect(uncalibrate(10, 10, 85)).toBe(0);
    expect(uncalibrate(85, 10, 85)).toBe(100);
    expect(uncalibrate(48, 10, 85)).toBe(51);
    expect(uncalibrate(5, 10, 85)).toBe(0);
    expect(uncalibrate(30, 40, 40)).toBe(0);
  });
});
