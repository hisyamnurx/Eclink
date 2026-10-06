/**
 * Guards the Tauri capability grants.
 *
 * This exists because of a real finding: `capabilities/default.json` granted
 * `updater:default`, which expands to `allow-check`, `allow-download`,
 * `allow-install`, AND `allow-download-and-install`. The app never calls the
 * updater JS plugin — all update logic goes through the app's own Rust commands,
 * where a user gesture is required — so that grant exposed
 * `plugin:updater|download_and_install` directly to the webview.
 *
 * The practical effect was that "updates are never silent" was enforced only by
 * an `onClick` handler, not by the permission layer, even though the backend
 * treats the frontend as untrusted input. The tests below pin the least-privilege
 * property so it cannot regress unnoticed.
 */

import { readFileSync } from "node:fs";
import { resolve } from "node:path";

import { describe, expect, it } from "vitest";

const repoRoot = resolve(__dirname, "..", "..");

function readText(relativePath: string): string {
  return readFileSync(resolve(repoRoot, relativePath), "utf8");
}

interface Capability {
  identifier: string;
  windows: string[];
  permissions: string[];
}

const capability = JSON.parse(readText("src-tauri/capabilities/default.json")) as Capability;

describe("Tauri capability grants", () => {
  it("grants no wildcard or default permission set", () => {
    // `updater:default`, `fs:default`, `shell:default` and friends expand to
    // several permissions at once, which is how an over-grant hides. Each entry
    // must name the exact permission that is needed.
    const broad = capability.permissions.filter((p) => /:default$/.test(p));
    // `core:default` and `core:event:default` are the documented minimum for a
    // window to render and receive events; anything beyond those is flagged.
    const allowedBroad = ["core:default", "core:event:default"];
    const unexpected = broad.filter((p) => !allowedBroad.includes(p));
    expect(
      unexpected,
      `these grant a whole default permission set: ${unexpected.join(", ")}`,
    ).toEqual([]);
  });

  it("does not allow the webview to install an update", () => {
    // The security-critical one. `allow-download-and-install` would let any
    // script in the webview replace the application without a user gesture.
    expect(capability.permissions).not.toContain("updater:default");
    expect(capability.permissions).not.toContain("updater:allow-download");
    expect(capability.permissions).not.toContain("updater:allow-install");
    expect(capability.permissions).not.toContain("updater:allow-download-and-install");
  });

  it("grants the updater only the check permission the UI needs", () => {
    const updaterGrants = capability.permissions.filter((p) => p.startsWith("updater:"));
    expect(updaterGrants).toEqual(["updater:allow-check"]);
  });

  it("grants no generic filesystem or shell capability", () => {
    // There must be no way for frontend code to read or write an arbitrary path
    // or run an arbitrary command.
    const dangerous = capability.permissions.filter((p) => /^(fs|shell):/.test(p));
    expect(
      dangerous,
      `generic filesystem/shell access must not be granted: ${dangerous.join(", ")}`,
    ).toEqual([]);
  });

  it("targets only the main window", () => {
    // A capability that applies to every window would silently widen if another
    // window is ever added.
    expect(capability.windows).toEqual(["main"]);
  });
});

describe("frontend dependency surface", () => {
  it("does not import the updater JS plugin", () => {
    // The grant above is only safe while nothing calls the plugin from the
    // webview. If this ever changes, the capability must be revisited
    // deliberately rather than by accident.
    const sources = [
      "src/App.tsx",
      "src/main.tsx",
      "src/lib/api.ts",
      "src/lib/hooks.ts",
      "src/lib/opener.ts",
      "src/pages/Settings.tsx",
    ];
    for (const file of sources) {
      const text = readText(file);
      expect(
        /@tauri-apps\/plugin-updater/.test(text),
        `${file} must not import the updater plugin`,
      ).toBe(false);
      expect(/downloadAndInstall/.test(text), `${file} must not call downloadAndInstall`).toBe(
        false,
      );
    }
  });
});
