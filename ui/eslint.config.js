import js from "@eslint/js";
import globals from "globals";
import svelte from "eslint-plugin-svelte";
import svelteParser from "svelte-eslint-parser";
import tseslint from "typescript-eslint";

export default [
  { ignores: ["dist/", "node_modules/"] },
  js.configs.recommended,
  ...tseslint.configs.recommended,
  ...svelte.configs["flat/recommended"],
  {
    languageOptions: { globals: { ...globals.browser } },
  },
  {
    files: ["**/*.svelte", "**/*.svelte.ts"],
    languageOptions: { parser: svelteParser, parserOptions: { parser: tseslint.parser } },
  },
  {
    rules: {
      // The UI deliberately re-renders from snapshots of backend state.
      "svelte/require-each-key": "error",
      "svelte/no-at-html-tags": "error",
      // `each` keys over plain arrays and state objects are fine without resolve() (no SvelteKit here).
      "svelte/no-navigation-without-resolve": "off",
      "@typescript-eslint/no-unused-vars": ["error", { argsIgnorePattern: "^_" }],
    },
  },
];
