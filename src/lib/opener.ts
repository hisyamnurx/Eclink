/**
 * Native dialog and link helpers.
 *
 * The Tauri plugins are imported lazily so the whole module graph still loads in
 * a plain browser during tests, and so a missing plugin surfaces as a normal
 * error the UI can show rather than a module-resolution crash at startup.
 */

import { ApiError } from "./api";

/** Open an external URL in the user's default browser. */
export async function openUrl(url: string): Promise<void> {
  // Only ever hand a real web URL to the OS. A `file:` or `javascript:` URL
  // arriving from a config file must not become a launch.
  if (!/^https?:\/\//i.test(url)) {
    throw new ApiError({
      kind: "validation",
      message: "That link cannot be opened.",
      hint: "Only http and https links are supported.",
    });
  }

  try {
    const { openUrl: pluginOpenUrl } = await import("@tauri-apps/plugin-opener");
    await pluginOpenUrl(url);
  } catch (error) {
    throw new ApiError({
      kind: "internal",
      message: "Could not open your browser.",
      details: String(error),
    });
  }
}

/** Reveal a folder in File Explorer. */
export async function revealPath(path: string): Promise<void> {
  try {
    const { revealItemInDir } = await import("@tauri-apps/plugin-opener");
    await revealItemInDir(path);
  } catch (error) {
    throw new ApiError({
      kind: "internal",
      message: "Could not open that folder.",
      hint: path,
      details: String(error),
    });
  }
}

/** Show a native folder picker; resolves to null when cancelled. */
export async function pickFolder(defaultPath?: string): Promise<string | null> {
  try {
    const { open } = await import("@tauri-apps/plugin-dialog");
    const selected = await open({
      directory: true,
      multiple: false,
      defaultPath: defaultPath || undefined,
      title: "Choose a project folder",
    });
    if (typeof selected === "string") return selected;
    return null;
  } catch (error) {
    throw new ApiError({
      kind: "internal",
      message: "Could not open the folder picker.",
      details: String(error),
    });
  }
}

/** Write text to the clipboard, falling back to the web API. */
export async function copyToClipboard(text: string): Promise<void> {
  try {
    if (navigator.clipboard?.writeText) {
      await navigator.clipboard.writeText(text);
      return;
    }
  } catch {
    // Fall through to the legacy path below.
  }

  // A hidden textarea plus execCommand still works when the async clipboard API
  // is unavailable, which happens in some webview configurations.
  const area = document.createElement("textarea");
  area.value = text;
  area.setAttribute("readonly", "");
  area.style.position = "fixed";
  area.style.opacity = "0";
  document.body.appendChild(area);
  area.select();
  try {
    const ok = document.execCommand("copy");
    if (!ok) throw new Error("copy rejected");
  } catch (error) {
    throw new ApiError({
      kind: "internal",
      message: "Could not copy to the clipboard.",
      hint: "Select the text and copy it manually.",
      details: String(error),
    });
  } finally {
    document.body.removeChild(area);
  }
}
