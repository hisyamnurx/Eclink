/**
 * Frontend entry point.
 *
 * The Tauri invoke function is injected here rather than imported at module
 * scope in the API layer, which is what lets the API layer and every screen be
 * exercised in tests with a stub backend.
 */

import { StrictMode } from "react";
import { createRoot } from "react-dom/client";

import { App } from "./App";
import { setInvoke } from "./lib/api";
import "./styles/app.css";

async function main() {
  // In a browser (or a test harness) the Tauri globals are absent; `hasBackend`
  // then reports false and the shell shows an explanatory message instead of
  // crashing on an undefined global.
  if ("__TAURI_INTERNALS__" in window) {
    const { invoke } = await import("@tauri-apps/api/core");
    setInvoke(invoke as unknown as Parameters<typeof setInvoke>[0]);
  }

  const container = document.getElementById("root");
  if (!container) {
    throw new Error("The root element is missing from index.html");
  }

  createRoot(container).render(
    <StrictMode>
      <App />
    </StrictMode>,
  );
}

void main();
