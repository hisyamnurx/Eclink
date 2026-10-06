/// <reference types="vitest/config" />
import { defineConfig } from "vite";
import react from "@vitejs/plugin-react";

// Tauri serves the frontend from a fixed dev port and needs a static build
// output directory that `tauri.conf.json` -> `build.frontendDist` points at.
// `process` is typed through the vitest/config reference above, which brings in
// Node's globals for this config file only.
const host = process.env.TAURI_DEV_HOST;
const debug = Boolean(process.env.TAURI_ENV_DEBUG);

export default defineConfig({
  plugins: [react()],

  // Vite options tailored for Tauri development.
  clearScreen: false,
  server: {
    port: 1420,
    strictPort: true,
    host: host || false,
    hmr: host
      ? {
          protocol: "ws",
          host,
          port: 1421,
        }
      : undefined,
    watch: {
      // Rust sources are rebuilt by cargo, not Vite.
      ignored: ["**/src-tauri/**"],
    },
  },

  build: {
    // Tauri uses Chromium on Windows (WebView2).
    target: "chrome110",
    // A debug build keeps readable output and source maps; a release build ships
    // minified with no maps, so no source is exposed in the installer.
    minify: debug ? false : "esbuild",
    sourcemap: debug,
    outDir: "dist",
    emptyOutDir: true,
  },

  test: {
    environment: "jsdom",
    include: ["src/**/*.test.{ts,tsx}"],
    globals: true,
    // React 19 only treats `act()` as valid when this flag is set. Without it
    // every component test prints "The current testing environment is not
    // configured to support act(...)" and state updates are not flushed
    // deterministically, which would make the hook tests pass for the wrong
    // reason.
    setupFiles: ["./src/test-setup.ts"],
    // Every test runs offline against a stubbed backend; no paid provider is
    // ever contacted from the test suite.
    restoreMocks: true,
  },
});
