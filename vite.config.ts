import { defineConfig } from "vite";
import react from "@vitejs/plugin-react";

// Tauri serves the frontend from a fixed dev port and needs a static build
// output directory that `tauri.conf.json` -> `build.frontendDist` points at.
const host = process.env.TAURI_DEV_HOST;

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
    minify: process.env.TAURI_ENV_DEBUG ? false : "esbuild",
    sourcemap: !!process.env.TAURI_ENV_DEBUG,
    outDir: "dist",
    emptyOutDir: true,
  },

  test: {
    environment: "jsdom",
    include: ["src/**/*.test.{ts,tsx}"],
    globals: true,
  },
});
