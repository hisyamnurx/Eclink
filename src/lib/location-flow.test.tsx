/**
 * The install-location flow, driven through the real provider and a fake backend.
 *
 * The backend tests prove resolution and persistence. This proves the wiring the
 * interface depends on: the command is called with the agent that was edited
 * (and only that agent's location), the re-detected summary replaces that agent's
 * card, and a failure leaves both the agent list and the saved locations exactly
 * as they were.
 *
 * The backend is faked through the same dependency-injected `setInvoke` seam the
 * language tests use, so no Tauri runtime is required.
 */

import { act } from "react";
import { createRoot, type Root } from "react-dom/client";
import { afterEach, beforeEach, describe, expect, it } from "vitest";

import {
  setInvoke,
  type AgentSummary,
  type AgentLocation,
  type AppSettings,
  type SettingsView,
} from "./api";
import { AppProvider, useApp } from "../state/AppContext";

function summary(
  agent: AgentSummary["agent"],
  overrides: Partial<AgentSummary> = {},
): AgentSummary {
  return {
    agent,
    displayName: agent === "codex" ? "Codex" : agent === "claude" ? "Claude Code" : "Oh My Pi",
    installed: true,
    version: "1.0.0",
    executable: `C:\\Tools\\${agent}\\${agent}.exe`,
    configPath: `C:\\Users\\Test\\.${agent}\\config`,
    configState: "missing",
    activeLabel: "Default configuration",
    activeKind: "default",
    configWritable: true,
    locationMode: "automatic",
    ...overrides,
  };
}

function baseSettings(overrides: Partial<SettingsView> = {}): SettingsView {
  return {
    schemaVersion: 2,
    updateCheck: false,
    rememberProjectDirectory: true,
    logLevel: "info",
    firstRunComplete: true,
    costNoticeSeen: true,
    insecureEndpointNoticeSeen: true,
    language: "en",
    ompProfile: "default",
    agentLocations: {},
    appVersion: "0.3.0",
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

describe("saving an install location", () => {
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

  interface Probe {
    agents: AgentSummary[];
    settings: SettingsView | null;
    setAgentLocation: (agent: string, location: AgentLocation | null) => Promise<void>;
    /** Save a general preference, exactly as the Settings screen does. */
    saveSettings: (patch: Partial<AppSettings>) => Promise<void>;
  }
  let probe: Probe | null = null;

  function mountProvider() {
    probe = null;
    function Capture() {
      const { agents, settings, setAgentLocation, saveSettings } = useApp();
      probe = { agents, settings, setAgentLocation, saveSettings };
      return null;
    }
    act(() => {
      root = createRoot(container);
      root.render(
        <AppProvider>
          <Capture />
        </AppProvider>,
      );
    });
  }

  async function settle() {
    await act(async () => {
      await Promise.resolve();
      await Promise.resolve();
    });
  }

  /** A fake backend that mirrors the real command's contract. */
  function installFakeBackend(options: { failSave?: boolean; emptyAgents?: boolean } = {}) {
    let storedSettings = baseSettings();
    let storedAgents = options.emptyAgents
      ? []
      : [summary("codex"), summary("claude"), summary("omp")];
    const calls: Array<{ agent: string; location: AgentLocation | null }> = [];

    setInvoke((command, args) => {
      const payload = (args ?? {}) as {
        agent?: AgentSummary["agent"];
        location?: AgentLocation | null;
      };
      switch (command) {
        case "get_settings":
          return Promise.resolve(structuredClone(storedSettings));
        case "list_agents":
          return Promise.resolve(structuredClone(storedAgents));
        case "list_profiles":
          return Promise.resolve([]);
        case "set_agent_location": {
          const agent = payload.agent!;
          const location = payload.location ?? null;
          calls.push({ agent, location });
          if (options.failSave) {
            return Promise.reject({ kind: "storage", message: "Could not write settings." });
          }
          const agentLocations = { ...storedSettings.agentLocations };
          if (location) {
            agentLocations[agent] = location;
          } else {
            delete agentLocations[agent];
          }
          storedSettings = { ...storedSettings, agentLocations };
          const updated = summary(agent, {
            installed: Boolean(location),
            locationMode: location ? location.kind : "automatic",
            configuredLocation: location?.path,
          });
          storedAgents = storedAgents.map((item) => (item.agent === agent ? updated : item));
          return Promise.resolve({
            agent: updated,
            agentLocations,
          });
        }
        default:
          return Promise.resolve(null);
      }
    });

    return { calls, agents: () => storedAgents, settings: () => storedSettings };
  }

  it("saves one agent and replaces only that agent's card", async () => {
    const backend = installFakeBackend();
    mountProvider();
    await settle();

    const claudeBefore = probe!.agents.find((item) => item.agent === "claude");
    const ompBefore = probe!.agents.find((item) => item.agent === "omp");

    await act(async () => {
      await probe!.setAgentLocation("codex", {
        path: "D:\\Tools\\codex",
        kind: "folder",
      });
    });

    expect(backend.calls).toEqual([
      { agent: "codex", location: { path: "D:\\Tools\\codex", kind: "folder" } },
    ]);
    expect(probe!.settings?.agentLocations.codex).toEqual({
      path: "D:\\Tools\\codex",
      kind: "folder",
    });
    expect(probe!.settings?.agentLocations.claude).toBeUndefined();
    expect(probe!.settings?.agentLocations.omp).toBeUndefined();

    const codex = probe!.agents.find((item) => item.agent === "codex");
    expect(codex?.locationMode).toBe("folder");
    expect(codex?.configuredLocation).toBe("D:\\Tools\\codex");

    // The other agents are the same objects: nothing re-detected them.
    expect(probe!.agents.find((item) => item.agent === "claude")).toEqual(claudeBefore);
    expect(probe!.agents.find((item) => item.agent === "omp")).toEqual(ompBefore);
  });

  it("clears a location by sending null and returns the agent to automatic", async () => {
    const backend = installFakeBackend();
    mountProvider();
    await settle();

    // Seed a saved choice first, then reset it.
    await act(async () => {
      await probe!.setAgentLocation("omp", {
        path: "D:\\Tools\\omp\\omp.exe",
        kind: "launcher",
      });
    });
    await act(async () => {
      await probe!.setAgentLocation("omp", null);
    });

    expect(backend.calls[1]).toEqual({ agent: "omp", location: null });
    expect(probe!.settings?.agentLocations.omp).toBeUndefined();
    expect(probe!.agents.find((item) => item.agent === "omp")?.locationMode).toBe("automatic");
  });

  it("adds the re-detected agent when the initial list was empty", async () => {
    // A failed `list_agents` leaves no cards. Saving a location must still make
    // that agent visible rather than discarding the only fresh summary.
    installFakeBackend({ emptyAgents: true });
    mountProvider();
    await settle();
    expect(probe!.agents).toEqual([]);

    await act(async () => {
      await probe!.setAgentLocation("claude", {
        path: "D:\\Tools\\Claude O'Brien",
        kind: "folder",
      });
    });

    expect(probe!.agents).toHaveLength(1);
    expect(probe!.agents[0]?.agent).toBe("claude");
    expect(probe!.agents[0]?.configuredLocation).toBe("D:\\Tools\\Claude O'Brien");
  });

  it("does not let a late response overwrite another agent's newer location", async () => {
    // Two saves overlap. Each response carries the location map as the backend
    // saw it when that request started, so neither includes the other agent's
    // new value. Applying a response wholesale would drop the newer choice.
    const pending: Array<{
      agent: AgentSummary["agent"];
      location: AgentLocation | null;
      release: () => void;
    }> = [];

    setInvoke((command, args) => {
      const payload = (args ?? {}) as {
        agent?: AgentSummary["agent"];
        location?: AgentLocation | null;
      };
      switch (command) {
        case "get_settings":
          return Promise.resolve(baseSettings());
        case "list_agents":
          return Promise.resolve([summary("codex"), summary("claude"), summary("omp")]);
        case "list_profiles":
          return Promise.resolve([]);
        case "set_agent_location": {
          const agent = payload.agent!;
          const location = payload.location ?? null;
          return new Promise((resolve) => {
            pending.push({
              agent,
              location,
              release: () => {
                resolve({
                  agent: summary(agent, {
                    installed: Boolean(location),
                    locationMode: location ? location.kind : "automatic",
                    configuredLocation: location?.path,
                  }),
                  // Deliberately only this agent: that is a stale snapshot.
                  agentLocations: location ? { [agent]: location } : {},
                });
              },
            });
          });
        }
        default:
          return Promise.resolve(null);
      }
    });

    mountProvider();
    await settle();

    const codex: AgentLocation = { path: "D:\\Tools\\codex", kind: "folder" };
    const claude: AgentLocation = { path: "D:\\Tools\\claude", kind: "folder" };
    let codexSave: Promise<void> | undefined;
    let claudeSave: Promise<void> | undefined;

    act(() => {
      codexSave = probe!.setAgentLocation("codex", codex);
      claudeSave = probe!.setAgentLocation("claude", claude);
    });
    expect(pending).toHaveLength(2);

    // The newer save (Claude) answers first; the stale Codex response arrives
    // afterwards without Claude's entry.
    act(() => pending[1]!.release());
    act(() => pending[0]!.release());
    await act(async () => {
      await Promise.all([codexSave!, claudeSave!]);
    });

    expect(probe!.settings?.agentLocations.codex).toEqual(codex);
    expect(probe!.settings?.agentLocations.claude).toEqual(claude);
    expect(probe!.agents.find((item) => item.agent === "codex")?.configuredLocation).toBe(
      codex.path,
    );
    expect(probe!.agents.find((item) => item.agent === "claude")?.configuredLocation).toBe(
      claude.path,
    );
  });

  it("keeps a saved location when a general settings response arrives late", async () => {
    // `update_settings` replies with a whole settings snapshot taken when the
    // backend handled it. A general preference saved before the location change
    // is therefore answered with the old, empty location map. Applying that
    // response wholesale restored a launcher path the user had just set (or
    // removed). The general preference must still land; the location must not
    // move backwards.
    const codex: AgentLocation = { path: "D:\\Tools\\codex", kind: "folder" };
    let releaseUpdate: (() => void) | undefined;

    setInvoke((command, args) => {
      const payload = (args ?? {}) as { patch?: Partial<AppSettings> };
      switch (command) {
        case "get_settings":
          return Promise.resolve(baseSettings({ updateCheck: true }));
        case "list_agents":
          return Promise.resolve([summary("codex"), summary("claude"), summary("omp")]);
        case "list_profiles":
          return Promise.resolve([]);
        case "set_agent_location":
          return Promise.resolve({
            agent: summary("codex", {
              locationMode: codex.kind,
              configuredLocation: codex.path,
            }),
            agentLocations: { codex },
          });
        case "update_settings":
          return new Promise((resolve) => {
            releaseUpdate = () =>
              resolve({
                // The stale snapshot: no saved locations, preference applied.
                ...baseSettings({ updateCheck: payload.patch?.updateCheck ?? true }),
                agentLocations: {},
              });
          });
        default:
          return Promise.resolve(null);
      }
    });

    mountProvider();
    await settle();

    // The general save starts first and is left in flight.
    let generalSave: Promise<void> | undefined;
    act(() => {
      generalSave = probe!.saveSettings({ updateCheck: false });
    });
    expect(releaseUpdate, "update_settings was not called").toBeDefined();

    // The location is saved and rendered while that response is still pending.
    await act(async () => {
      await probe!.setAgentLocation("codex", codex);
    });
    expect(probe!.settings?.agentLocations.codex).toEqual(codex);

    // The old response now arrives.
    await act(async () => {
      releaseUpdate!();
      await generalSave;
    });

    expect(
      probe!.settings?.agentLocations.codex,
      "the late response reverted the saved launcher location",
    ).toEqual(codex);
    expect(probe!.agents.find((item) => item.agent === "codex")?.configuredLocation).toBe(
      codex.path,
    );
    expect(probe!.settings?.updateCheck, "the general preference must still be applied").toBe(
      false,
    );
  });

  it("leaves the list and the saved locations untouched when the save fails", async () => {
    installFakeBackend({ failSave: true });
    mountProvider();
    await settle();

    let failure: unknown;
    await act(async () => {
      try {
        await probe!.setAgentLocation("codex", {
          path: "D:\\Tools\\codex",
          kind: "folder",
        });
      } catch (error) {
        failure = error;
      }
    });

    expect(failure, "a failed save must reject so the caller can report it").toBeDefined();
    expect(probe!.settings?.agentLocations).toEqual({});
    expect(probe!.agents.every((item) => item.locationMode === "automatic")).toBe(true);
  });
});
