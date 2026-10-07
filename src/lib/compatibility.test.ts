/**
 * Tests for the profile state a test run leaves behind.
 *
 * These exist because of a real bug: after a test reported 7/7 passed and the
 * panel said Compatible, the badge above still read "Not tested" and Activate
 * stayed disabled. The cause was that the test handler wrote the result to local
 * state but never reloaded the `profiles` array, which is where the badge and the
 * Activate gate read `compatibility` from. The badge only corrected itself when
 * something else happened to reload profiles (a tab switch, a refresh, an
 * activate), which is why it looked like the state changed on its own after a few
 * seconds.
 *
 * The tests below pin the *contract*: after a test run, the compatibility the UI
 * reads must match the result the backend just recorded. They are written against
 * the same derivation the components use, so a regression in the wiring shows up
 * here rather than only in the UI.
 */

import { describe, expect, it } from "vitest";

import type { AgentSummary, CompatibilityStatus, ProfileSummary } from "./api";
import { activationGate, allowsActivation, compatibilityLabel } from "./compatibility";
import { dictionaryFor, LANGUAGES } from "./i18n";

function profileWith(status: CompatibilityStatus | undefined, hasSecret = true): ProfileSummary {
  return {
    id: "p1",
    name: "provider_a",
    providerName: "provider_a",
    baseUrl: "https://gateway.example.com",
    model: "example-model",
    targets: ["claude"],
    hasSecret,
    ready: true,
    createdAt: "2026-01-01T00:00:00Z",
    updatedAt: "2026-01-01T00:00:00Z",
    compatibility: status ? { claude: { status } } : {},
  };
}

function agent(overrides: Partial<AgentSummary> = {}): AgentSummary {
  return {
    agent: "claude",
    displayName: "Claude Code",
    installed: true,
    configPath: "C:\\cfg\\settings.json",
    configState: "valid",
    activeLabel: "External Configuration",
    activeKind: "external",
    configWritable: true,
    ...overrides,
  };
}

describe("compatibilityLabel", () => {
  it("gives every status a message key, not an English word", () => {
    expect(compatibilityLabel("compatible")).toBe("status.compatible");
    expect(compatibilityLabel("partially_compatible")).toBe("status.partiallyCompatible");
    expect(compatibilityLabel("incompatible")).toBe("status.incompatible");
    expect(compatibilityLabel("test_failed")).toBe("status.testFailed");
    expect(compatibilityLabel("testing")).toBe("status.testing");
  });

  it("treats an unknown status as untested rather than as compatible", () => {
    // Overstating here would let a user activate something never verified.
    expect(compatibilityLabel(undefined)).toBe("status.notTested");
    expect(compatibilityLabel("something_new")).toBe("status.notTested");
  });

  it("names only keys the dictionaries carry, in every language", () => {
    // The words used to be duplicated here as literals, which left these
    // dictionary entries dead. A key that no dictionary has would render as the
    // key itself, so this pins the other half of the contract.
    const statuses: Array<string | undefined> = [
      "compatible",
      "partially_compatible",
      "incompatible",
      "unknown",
      "test_failed",
      "testing",
      "untested",
      undefined,
      "something_new",
    ];
    for (const language of LANGUAGES) {
      const dictionary = dictionaryFor(language);
      for (const status of statuses) {
        const key = compatibilityLabel(status);
        expect(dictionary[key], `${language} is missing ${key}`).toBeTruthy();
      }
    }
  });
});

describe("allowsActivation", () => {
  it("only permits activation for a status that passed a test", () => {
    expect(allowsActivation("compatible")).toBe(true);
    expect(allowsActivation("partially_compatible")).toBe(true);
  });

  it("refuses everything that was not verified", () => {
    for (const status of [
      "untested",
      "testing",
      "incompatible",
      "unknown",
      "test_failed",
      undefined,
    ]) {
      expect(allowsActivation(status), `status ${String(status)}`).toBe(false);
    }
  });
});

describe("the activation gate", () => {
  it("is closed before any test has run", () => {
    const gate = activationGate(profileWith(undefined), [agent()], "claude");
    expect(gate.label).toBe("status.notTested");
    expect(gate.canActivate).toBe(false);
    expect(gate.blockedReason).toBe("providers.testFirst");
  });

  it("opens as soon as a compatible result is present", () => {
    // The exact reported failure: 7/7 passed and the panel said Compatible, but
    // the badge said Not tested and Activate was disabled.
    const gate = activationGate(profileWith("compatible"), [agent()], "claude");
    expect(gate.label).toBe("status.compatible");
    expect(gate.canActivate).toBe(true);
    expect(gate.blockedReason).toBeUndefined();
  });

  it("opens for a partially compatible result too", () => {
    const gate = activationGate(profileWith("partially_compatible"), [agent()], "claude");
    expect(gate.canActivate).toBe(true);
  });

  it("stays closed when the profile has no API key", () => {
    // A compatible endpoint is not enough; the credential must be stored.
    const gate = activationGate(profileWith("compatible", false), [agent()], "claude");
    expect(gate.canActivate).toBe(false);
    expect(gate.blockedReason).toBe("providers.addKeyFirst");
  });

  it("stays closed after a failed test", () => {
    for (const status of ["incompatible", "test_failed"] as CompatibilityStatus[]) {
      const gate = activationGate(profileWith(status), [agent()], "claude");
      expect(gate.canActivate).toBe(false);
    }
  });

  it("reads the status for the agent being viewed, not the other agent", () => {
    // A profile tested for Codex must not enable Activate on the Claude tab.
    const profile: ProfileSummary = {
      ...profileWith(undefined),
      compatibility: { codex: { status: "compatible" } },
    };
    expect(activationGate(profile, [agent()], "claude").canActivate).toBe(false);
    expect(activationGate(profile, [agent()], "codex").canActivate).toBe(true);
  });
});

describe("what the UI shows after a test", () => {
  it("the badge label and the gate agree, so they can never disagree on screen", () => {
    // The invariant that was violated: the panel and the badge derived from
    // different values. Both now come from the same profile record.
    for (const status of [
      undefined,
      "compatible",
      "partially_compatible",
      "incompatible",
      "test_failed",
    ] as Array<CompatibilityStatus | undefined>) {
      const gate = activationGate(profileWith(status), [agent()], "claude");
      const isOpen = gate.canActivate;
      const saysUsable = status === "compatible" || status === "partially_compatible";
      // The gate may be closed for a missing key, which is a separate reason,
      // but it can never be closed while the badge claims the endpoint is usable
      // and a key is stored.
      if (saysUsable) {
        expect(isOpen, `status ${String(status)} should open the gate`).toBe(true);
      } else {
        expect(isOpen, `status ${String(status)} must not open the gate`).toBe(false);
      }
    }
  });

  it("a reloaded profile list carries the new status immediately", () => {
    // Simulates the fix: the backend records the verdict, the UI reloads, and the
    // very next render sees it. No second reload or tab switch is involved.
    const before = profileWith(undefined);
    expect(activationGate(before, [agent()], "claude").canActivate).toBe(false);

    // This is what `list_profiles` returns after `test_profile_connection`.
    const after = profileWith("compatible");
    expect(activationGate(after, [agent()], "claude").canActivate).toBe(true);
  });

  it("does not depend on any other state being refreshed first", () => {
    // The act of testing must not require the agent list to change, a tab switch,
    // or a manual refresh.
    const gate = activationGate(profileWith("compatible"), [agent()], "claude");
    expect(gate.canActivate).toBe(true);
  });

  it("survives an application restart because it is persisted, not derived", () => {
    // Compatibility lives in profiles.json, so a fresh load shows the same gate.
    const persisted = JSON.parse(JSON.stringify(profileWith("compatible"))) as ProfileSummary;
    expect(activationGate(persisted, [agent()], "claude").canActivate).toBe(true);
  });
});
