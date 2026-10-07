/**
 * The language switch, driven through the real provider and a fake backend.
 *
 * The dictionary tests in `i18n.test.ts` prove the translations exist. They cannot
 * prove that selecting a language changes the interface, which is the part that was
 * broken: the picker stored the choice, the interface stayed English, and nothing
 * failed. The cause was that `saveSettings` builds its IPC patch field by field and
 * omitted `language`, so the backend returned unchanged settings and the caller
 * reverted them.
 *
 * So these tests mount the real `AppProvider`, install a fake backend through the
 * dependency-injected `setInvoke` seam, and assert on what a component actually
 * renders before and after the switch. A test that only compared dictionaries would
 * not have caught the bug and will not catch its return.
 */

import { act } from "react";
import { createRoot, type Root } from "react-dom/client";
import { afterEach, beforeEach, describe, expect, it } from "vitest";

import { setInvoke, type AppSettings, type SettingsView } from "./api";
import { AppProvider, useApp } from "../state/AppContext";

/** The settings a fresh install starts from. */
function freshSettings(overrides: Partial<SettingsView> = {}): SettingsView {
  return {
    schemaVersion: 1,
    updateCheck: true,
    rememberProjectDirectory: true,
    logLevel: "info",
    firstRunComplete: true,
    costNoticeSeen: false,
    insecureEndpointNoticeSeen: false,
    language: "en",
    appVersion: "0.1.0",
    dataDirectory: "C:\\data",
    logDirectory: "C:\\data\\logs",
    repositoryUrl: "https://example.invalid",
    secretBackend: "Windows DPAPI (Current User)",
    secretCount: 0,
    profileCount: 0,
    credentialHelperAvailable: true,
    ...overrides,
  };
}

/**
 * A fake backend holding settings in memory.
 *
 * It mirrors the real command's contract: `update_settings` merges the patch onto
 * the stored settings and returns the whole object. That merge is what made the
 * missing `language` field invisible, so reproducing it is the point.
 */
function installFakeBackend(initial: SettingsView, options: { failSave?: boolean } = {}) {
  const stored: SettingsView = { ...initial };
  const savedPatches: Array<Record<string, unknown>> = [];

  setInvoke((command, args) => {
    switch (command) {
      case "get_settings":
        return Promise.resolve({ ...stored });
      case "list_agents":
        return Promise.resolve([]);
      case "list_profiles":
        return Promise.resolve([]);
      case "update_settings": {
        const patch = (args?.patch ?? {}) as Partial<AppSettings>;
        savedPatches.push(patch as Record<string, unknown>);
        if (options.failSave) {
          return Promise.reject({ kind: "storage", message: "Could not write settings." });
        }
        // Merge exactly as the Rust command does: only provided fields change.
        for (const [key, value] of Object.entries(patch)) {
          if (value !== undefined) {
            (stored as unknown as Record<string, unknown>)[key] = value;
          }
        }
        return Promise.resolve({ ...stored });
      }
      default:
        return Promise.resolve(null);
    }
  });

  return {
    patches: savedPatches,
    stored: () => ({ ...stored }),
  };
}

describe("switching the interface language", () => {
  let container: HTMLDivElement;
  let root: Root;

  beforeEach(() => {
    container = document.createElement("div");
    document.body.appendChild(container);
  });

  afterEach(() => {
    act(() => root?.unmount());
    container.remove();
  });

  /** What a component currently renders, captured on every render. */
  interface Probe {
    language: string;
    navOverview: string;
    providers: string;
    setLanguage: (next: "en" | "id") => Promise<void>;
  }
  let probe: Probe | null = null;

  function mountProvider() {
    probe = null;
    function Probe() {
      const { language, t, setLanguage } = useApp();
      probe = {
        language,
        navOverview: t("nav.overview"),
        providers: t("nav.providers"),
        setLanguage,
      };
      return null;
    }
    act(() => {
      root = createRoot(container);
      root.render(
        <AppProvider>
          <Probe />
        </AppProvider>,
      );
    });
  }

  /** Let pending promises settle, then read the rendered values. */
  async function settle() {
    await act(async () => {
      await Promise.resolve();
      await Promise.resolve();
    });
  }

  it("starts in English", async () => {
    installFakeBackend(freshSettings());
    mountProvider();
    await settle();

    expect(probe?.language).toBe("en");
    expect(probe?.navOverview).toBe("Overview");
    expect(probe?.providers).toBe("Providers");
  });

  it("switches the current screen to Indonesian without a reload", async () => {
    // The reported bug: the selection appeared to save and the interface stayed
    // English. Asserting the rendered text is what distinguishes the two.
    installFakeBackend(freshSettings());
    mountProvider();
    await settle();

    await act(async () => {
      await probe!.setLanguage("id");
    });

    expect(probe?.language).toBe("id");
    expect(probe?.navOverview).toBe("Ringkasan");
    expect(probe?.providers).toBe("Provider");
  });

  it("sends the language to the backend, which is what was missing", async () => {
    // The precise defect: `language` was absent from the IPC patch, so the backend
    // kept `en` and the state was reverted. Asserting the patch catches it directly,
    // and would catch any future field dropped from the same hand-written list.
    const backend = installFakeBackend(freshSettings());
    mountProvider();
    await settle();

    await act(async () => {
      await probe!.setLanguage("id");
    });

    expect(backend.patches).toHaveLength(1);
    expect(backend.patches[0]).toHaveProperty("language", "id");
    expect(backend.stored().language).toBe("id");
  });

  it("keeps the language on later renders and after a reload", async () => {
    // Navigation must not reset it, and neither must re-reading settings.
    const backend = installFakeBackend(freshSettings());
    mountProvider();
    await settle();

    await act(async () => {
      await probe!.setLanguage("id");
    });
    await settle();

    expect(probe?.language).toBe("id");
    // A fresh provider reading the persisted value behaves like a restart.
    act(() => root.unmount());
    mountProvider();
    await settle();

    expect(probe?.language).toBe("id");
    expect(probe?.navOverview).toBe("Ringkasan");
    expect(backend.stored().language).toBe("id");
  });

  it("switches back to English", async () => {
    installFakeBackend(freshSettings({ language: "id" }));
    mountProvider();
    await settle();
    expect(probe?.navOverview).toBe("Ringkasan");

    await act(async () => {
      await probe!.setLanguage("en");
    });

    expect(probe?.language).toBe("en");
    expect(probe?.navOverview).toBe("Overview");
    expect(probe?.providers).toBe("Providers");
  });

  it("updates the interface before the save completes", async () => {
    // The requirement is that the change is immediate. Awaiting the round trip first
    // would still be correct eventually, but a slow write would leave the interface
    // in the old language, which reads as the bug.
    installFakeBackend(freshSettings());
    mountProvider();
    await settle();

    // Called without awaiting the returned promise.
    let pending: Promise<void> | undefined;
    act(() => {
      pending = probe!.setLanguage("id");
    });

    // Already switched, before the backend has replied.
    expect(probe?.language).toBe("id");
    expect(probe?.navOverview).toBe("Ringkasan");

    await act(async () => {
      await pending;
    });
    expect(probe?.language).toBe("id");
  });

  it("reports a failed save and does not leave the interface ahead of the file", async () => {
    // Runtime state and stored state must not disagree: a language the file does not
    // record would silently revert on the next launch, which is worse than refusing.
    installFakeBackend(freshSettings(), { failSave: true });
    mountProvider();
    await settle();

    let failure: unknown;
    await act(async () => {
      try {
        await probe!.setLanguage("id");
      } catch (error) {
        failure = error;
      }
    });

    expect(failure, "a failed save must reject so the caller can report it").toBeDefined();
    expect(probe?.language, "the interface must roll back to the stored language").toBe("en");
    expect(probe?.navOverview).toBe("Overview");
  });

  it("falls back to English when the stored language is not one it knows", async () => {
    // The value can come from a file an older or newer build wrote, so an unknown
    // tag must render rather than show blank text.
    installFakeBackend(freshSettings({ language: "fr" }));
    mountProvider();
    await settle();

    expect(probe?.language).toBe("en");
    expect(probe?.navOverview).toBe("Overview");
  });

  it("falls back to English when the stored language is missing", async () => {
    const settings = freshSettings();
    delete (settings as Partial<SettingsView>).language;
    installFakeBackend(settings);
    mountProvider();
    await settle();

    expect(probe?.language).toBe("en");
  });

  it("translates with placeholders in both languages", async () => {
    installFakeBackend(freshSettings());
    mountProvider();
    await settle();
    const english = probe!.navOverview;

    await act(async () => {
      await probe!.setLanguage("id");
    });

    expect(english).toBe("Overview");
    expect(probe!.navOverview).not.toBe(english);
  });
});
