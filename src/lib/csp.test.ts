/**
 * Guards the Content Security Policy.
 *
 * Tauri injects `app.security.csp` from `src-tauri/tauri.conf.json` at runtime
 * in a packaged build, while the meta tag in `index.html` governs `vite dev`.
 * Two copies of a security policy drift, and when they do the policy that is
 * actually enforced is no longer the one anybody reviewed. This test pins them
 * together and pins the properties that matter.
 */

import { readFileSync } from "node:fs";
import { resolve } from "node:path";

import { describe, expect, it } from "vitest";

const root = resolve(__dirname, "..", "..");

function indexHtmlPolicy(): string {
  const html = readFileSync(resolve(root, "index.html"), "utf8");
  const match = /http-equiv="Content-Security-Policy"\s+content="([^"]+)"/.exec(html);
  if (!match?.[1]) throw new Error("index.html has no CSP meta tag");
  return match[1];
}

function tauriConfigPolicy(): string {
  const raw = readFileSync(resolve(root, "src-tauri", "tauri.conf.json"), "utf8");
  const config = JSON.parse(raw) as { app?: { security?: { csp?: string } } };
  const csp = config.app?.security?.csp;
  if (typeof csp !== "string") throw new Error("tauri.conf.json declares no CSP");
  return csp;
}

/** The directives of a policy, as a name to value map. */
function directives(policy: string): Map<string, string> {
  const map = new Map<string, string>();
  for (const part of policy.split(";")) {
    const trimmed = part.trim();
    if (!trimmed) continue;
    const [name, ...rest] = trimmed.split(/\s+/);
    if (name) map.set(name, rest.join(" "));
  }
  return map;
}

describe("Content Security Policy", () => {
  it("is identical in index.html and tauri.conf.json", () => {
    expect(indexHtmlPolicy()).toBe(tauriConfigPolicy());
  });

  it("never allows scripts from anywhere but the bundle", () => {
    const csp = tauriConfigPolicy();
    // Remote script is the single worst thing a desktop app's CSP can permit.
    expect(csp).toContain("script-src 'self'");
    expect(csp).not.toMatch(/script-src[^;]*unsafe-eval/);
    expect(csp).not.toMatch(/script-src[^;]*unsafe-inline/);
    expect(csp).not.toMatch(/script-src[^;]*\*/);
    expect(csp).not.toMatch(/script-src[^;]*https?:/);
  });

  it("blocks objects, framing, and form posts", () => {
    const csp = indexHtmlPolicy();
    expect(csp).not.toContain("object-src");
    // These are set by Tauri's own defaults; what matters is that nothing in the
    // policy *loosens* object, frame, or form handling to a remote origin.
    expect(csp).not.toMatch(/object-src[^;]*https?:/);
    expect(csp).not.toMatch(/frame-src[^;]*https?:/);
  });

  it("restricts connections to the IPC channel", () => {
    const csp = tauriConfigPolicy();
    const connect = directives(csp).get("connect-src") ?? "";
    // Provider HTTP calls and the update check happen in Rust, so the webview
    // needs no network origin at all.
    expect(connect).toContain("ipc:");
    expect(connect).not.toContain("https:");
    expect(connect).not.toContain("*");
  });

  it("keeps a restrictive default-src", () => {
    const csp = tauriConfigPolicy();
    expect(directives(csp).get("default-src")).toBe("'self'");
  });

  it("does not permit remote styles or fonts", () => {
    const csp = tauriConfigPolicy();
    const style = directives(csp).get("style-src") ?? "";
    const font = directives(csp).get("font-src") ?? "";
    // Vite's dev server and React inline styles need 'unsafe-inline' for styles;
    // that is a known, bounded concession. Remote styles are not.
    expect(style).not.toContain("https:");
    expect(style).not.toContain("*");
    expect(font).not.toContain("https:");
  });
});
