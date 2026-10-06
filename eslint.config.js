import js from "@eslint/js";
import globals from "globals";
import reactHooks from "eslint-plugin-react-hooks";
import tseslint from "typescript-eslint";

export default tseslint.config(
  {
    // `target` is the shared Cargo output directory at the repository root; it
    // contains generated Tauri assets that are not source and must not be linted.
    ignores: [
      "dist",
      "node_modules",
      "target",
      "src-tauri/target",
      "coverage",
      "**/*.d.ts",
    ],
  },
  {
    files: ["**/*.{ts,tsx}"],
    extends: [js.configs.recommended, ...tseslint.configs.recommended],
    languageOptions: {
      ecmaVersion: 2022,
      globals: {
        ...globals.browser,
        ...globals.es2021,
      },
    },
    plugins: {
      "react-hooks": reactHooks,
    },
    rules: {
      ...reactHooks.configs.recommended.rules,
      "@typescript-eslint/no-explicit-any": "error",
      "@typescript-eslint/no-unused-vars": [
        "error",
        { argsIgnorePattern: "^_", varsIgnorePattern: "^_" },
      ],
      "no-console": ["error", { allow: ["warn", "error"] }],
      eqeqeq: ["error", "always"],
      "prefer-const": "error",
    },
  },
  {
    // Config and tooling files run in Node, not the browser.
    files: ["*.config.{js,ts}", "vite.config.ts"],
    languageOptions: {
      globals: { ...globals.node },
    },
  },
);
