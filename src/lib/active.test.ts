/**
 * Tests for active-profile derivation.
 *
 * These exist because of a real bug: the "which profile is active" question was
 * answered from cached compatibility test results - the first profile that had
 * ever passed a test for an agent - instead of from what the backend reports as
 * applied. The two agree only until a user activates a second profile.
 *
 * The property that matters is that the answer comes from `activeKind` /
 * `activeProfileName`, so an "external" config never claims to be one of ours.
 */

import { describe, expect, it } from "vitest";

import type { AgentSummary, ProfileSummary } from "./api";
import { findActiveProfile, isProfileActive } from "./active";

function profile(name: string, compatibility?: ProfileSummary["compatibility"]): ProfileSummary {
  return {
    id: `id-${name}`,
    name,
    providerName: "Prov",
    baseUrl: "https://gw.example.com/v1",
    model: "m",
    targets: ["codex"],
    hasSecret: true,
    ready: true,
    createdAt: "2026-01-01T00:00:00Z",
    updatedAt: "2026-01-01T00:00:00Z",
    compatibility,
  };
}

function agent(overrides: Partial<AgentSummary> = {}): AgentSummary {
  return {
    agent: "codex",
    displayName: "Codex",
    installed: true,
    configPath: "C:\\cfg\\config.toml",
    configState: "valid",
    activeLabel: "OpenAI Default",
    activeKind: "default",
    configWritable: true,
    ...overrides,
  };
}

describe("findActiveProfile", () => {
  it("returns the profile the backend names as active", () => {
    const profiles = [profile("First"), profile("Second")];
    const agents = [agent({ activeKind: "managed", activeProfileName: "Second" })];
    expect(findActiveProfile(profiles, agents, "codex")?.name).toBe("Second");
  });

  it("does not return a merely-tested profile", () => {
    // The regression. `First` has a passing test record and comes first in the
    // list; the naive implementation returned it even though `Second` is the
    // profile actually applied.
    const profiles = [
      profile("First", { codex: { status: "compatible" } }),
      profile("Second", { codex: { status: "compatible" } }),
    ];
    const agents = [agent({ activeKind: "managed", activeProfileName: "Second" })];
    const active = findActiveProfile(profiles, agents, "codex");
    expect(active?.name).toBe("Second");
    expect(active?.name).not.toBe("First");
  });

  it("returns undefined when the config is external", () => {
    // The config exists but was not written by this app, so no profile of ours
    // is responsible for it. Claiming otherwise would be a lie.
    const profiles = [profile("P", { codex: { status: "compatible" } })];
    const agents = [agent({ activeKind: "external" })];
    expect(findActiveProfile(profiles, agents, "codex")).toBeUndefined();
  });

  it("returns undefined for the default configuration", () => {
    const profiles = [profile("P", { codex: { status: "compatible" } })];
    expect(
      findActiveProfile(profiles, [agent({ activeKind: "default" })], "codex"),
    ).toBeUndefined();
  });

  it("returns undefined when a managed config names no profile", () => {
    // A managed config whose provider entry vanished from under us: the backend
    // reports managed but cannot name a profile.
    const profiles = [profile("P")];
    const agents = [agent({ activeKind: "managed" })];
    expect(findActiveProfile(profiles, agents, "codex")).toBeUndefined();
  });

  it("returns undefined when the named profile was deleted", () => {
    const agents = [agent({ activeKind: "managed", activeProfileName: "Gone" })];
    expect(findActiveProfile([profile("Other")], agents, "codex")).toBeUndefined();
  });

  it("returns undefined when the agent is not reported at all", () => {
    expect(findActiveProfile([profile("P")], [], "codex")).toBeUndefined();
  });

  it("does not confuse the two agents", () => {
    const profiles = [profile("ForCodex"), profile("ForClaude")];
    const agents = [
      agent({ agent: "codex", activeKind: "managed", activeProfileName: "ForCodex" }),
      agent({
        agent: "claude",
        activeKind: "managed",
        activeProfileName: "ForClaude",
      }),
    ];
    expect(findActiveProfile(profiles, agents, "codex")?.name).toBe("ForCodex");
    expect(findActiveProfile(profiles, agents, "claude")?.name).toBe("ForClaude");
  });
});

describe("isProfileActive", () => {
  it("is true only for the profile the backend names", () => {
    const first = profile("First");
    const second = profile("Second");
    const agents = [agent({ activeKind: "managed", activeProfileName: "Second" })];

    expect(isProfileActive(second, agents, "codex")).toBe(true);
    expect(isProfileActive(first, agents, "codex")).toBe(false);
  });

  it("is false for every profile when the config is external", () => {
    const agents = [agent({ activeKind: "external" })];
    expect(isProfileActive(profile("P"), agents, "codex")).toBe(false);
  });

  it("does not match on a shared name across agents", () => {
    const shared = profile("Shared");
    const agents = [
      agent({ agent: "codex", activeKind: "default" }),
      agent({ agent: "claude", activeKind: "managed", activeProfileName: "Shared" }),
    ];
    // Active for claude, not for codex, even though the name matches both.
    expect(isProfileActive(shared, agents, "claude")).toBe(true);
    expect(isProfileActive(shared, agents, "codex")).toBe(false);
  });
});
