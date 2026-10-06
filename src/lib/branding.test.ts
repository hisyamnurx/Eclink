/**
 * Guards the application branding.
 *
 * The Tauri CLI ships a set of default icons, and it is easy to forget that a
 * build still points at them after a rebrand — the app then shows the Tauri logo
 * in the taskbar, Start Menu, and installer while the UI shows the real one.
 * `npx tauri icon` overwrites the whole set from a single source, so the way
 * this goes wrong is a stale file left behind, not a missing one.
 *
 * These tests check the wiring and the asset properties rather than the pixels:
 * that the configured icons exist, that they are the right shapes for Windows,
 * that the sidebar mark is present in the build input, and that nothing points
 * outside the repository.
 */

import { existsSync, readFileSync, statSync } from "node:fs";
import { resolve } from "node:path";

import { describe, expect, it } from "vitest";

const repoRoot = resolve(__dirname, "..", "..");

function readText(relativePath: string): string {
  return readFileSync(resolve(repoRoot, relativePath), "utf8");
}

interface TauriConf {
  bundle: {
    icon: string[];
    windows?: { nsis?: { installerIcon?: string } };
    resources?: Record<string, string>;
  };
}

const conf = JSON.parse(readText("src-tauri/tauri.conf.json")) as TauriConf;

/** The 8-byte signature every PNG starts with. */
const PNG_MAGIC = Buffer.from([0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a]);
/** Every Windows .ico starts with this reserved header. */
const ICO_MAGIC = Buffer.from([0x00, 0x00, 0x01, 0x00]);

describe("application icons", () => {
  it("every configured icon exists and is the format its extension claims", () => {
    const configured = [
      ...conf.bundle.icon,
      conf.bundle.windows?.nsis?.installerIcon,
    ].filter((value): value is string => Boolean(value));

    expect(configured.length).toBeGreaterThan(0);
    for (const entry of configured) {
      const path = resolve(repoRoot, "src-tauri", entry);
      expect(existsSync(path), `${entry} is configured but missing`).toBe(true);

      const head = readFileSync(path).subarray(0, 8);
      if (entry.endsWith(".ico")) {
        expect(
          head.subarray(0, 4).equals(ICO_MAGIC),
          `${entry} is not a real .ico file`,
        ).toBe(true);
      } else {
        expect(
          head.equals(PNG_MAGIC),
          `${entry} is not a real PNG file`,
        ).toBe(true);
      }
      // A zero-byte or stub icon would pass a magic-number check alone.
      expect(statSync(path).size, `${entry} looks empty`).toBeGreaterThan(512);
    }
  });

  it("ships the icon sizes Windows actually asks for", () => {
    // The taskbar and Alt-Tab use 32x32, Explorer's large view uses 128, and
    // high-DPI displays use the @2x variant. Tauri generates these, so their
    // absence means the icon set was replaced with something incomplete.
    for (const name of [
      "icons/32x32.png",
      "icons/128x128.png",
      "icons/128x128@2x.png",
      "icons/icon.ico",
    ]) {
      expect(
        existsSync(resolve(repoRoot, "src-tauri", name)),
        `${name} is missing from the icon set`,
      ).toBe(true);
    }
  });

  it("points the installer at an icon, so Setup is not blank", () => {
    expect(conf.bundle.windows?.nsis?.installerIcon).toBe("icons/icon.ico");
  });

  it("references no icon outside the repository", () => {
    // The build must not depend on a path on one developer's machine.
    for (const entry of conf.bundle.icon) {
      expect(entry.startsWith("/"), `${entry} is an absolute path`).toBe(false);
      expect(/^[A-Za-z]:/.test(entry), `${entry} is a Windows absolute path`).toBe(
        false,
      );
    }
    const raw = readText("src-tauri/tauri.conf.json");
    expect(raw).not.toMatch(/[A-Za-z]:\\\\/);
  });
});

describe("in-app branding", () => {
  it("renders the mark in the sidebar", () => {
    const app = readText("src/App.tsx");
    expect(app).toContain("/logo-mark.png");
    // Decorative next to a text wordmark, so it must not be announced twice.
    expect(app).toMatch(/alt=""/);
  });

  it("ships the mark as a build input", () => {
    // `public/` is copied verbatim by Vite, which is what makes the absolute
    // `/logo-mark.png` reference resolve in the packaged app.
    const mark = resolve(repoRoot, "public", "logo-mark.png");
    expect(existsSync(mark), "public/logo-mark.png is missing").toBe(true);
    expect(readFileSync(mark).subarray(0, 8).equals(PNG_MAGIC)).toBe(true);
  });

  it("styles the mark so it cannot stretch the sidebar", () => {
    const css = readText("src/styles/app.css");
    expect(css).toContain(".sidebar__logo");
    // A fixed size and `object-fit: contain` keep the aspect ratio whatever the
    // source image is, so a future logo swap cannot distort the layout.
    expect(css).toMatch(/\.sidebar__logo\s*\{[^}]*width:\s*36px/s);
    expect(css).toMatch(/\.sidebar__logo\s*\{[^}]*object-fit:\s*contain/s);
  });

  it("uses a Content-Security-Policy that permits a local image", () => {
    // The CSP is deliberately strict. `img-src 'self'` is what allows the
    // sidebar mark; without it the logo would silently fail to render.
    const csp = readText("index.html");
    expect(csp).toMatch(/img-src 'self'/);
  });

  it("keeps the documentation wordmark pointing at a committed asset", () => {
    for (const [doc, asset] of [
      ["README.md", "0-Docs/logo.png"],
      ["0-Docs/README.md", "logo.png"],
    ]) {
      expect(readText(doc), `${doc} does not show the logo`).toContain(asset);
      const path = resolve(repoRoot, doc, "..", asset);
      expect(existsSync(path), `${asset} referenced by ${doc} is missing`).toBe(
        true,
      );
    }
  });
});
