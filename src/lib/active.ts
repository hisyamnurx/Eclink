/**
 * Deriving which profile is currently active for an agent.
 *
 * This lives in its own module, as a pure function, because there was a real bug
 * here: the value was previously computed from cached *test results* — the first
 * profile that had ever passed a compatibility test for that agent — rather than
 * from what the backend reports as applied. Those are different questions, and
 * the answer diverges the moment a user activates a second profile.
 *
 * The backend is the only authority: `AgentSummary.activeKind` is `"managed"`
 * only when the app recognises its own config, and `activeProfileName` is the
 * name it recognises. Deriving from anywhere else would mean showing a profile
 * as active that the agent is not actually using.
 */

import type { AgentSummary, ProfileSummary } from "./api";

/** The profile currently applied to `agent`, or undefined if none is recognised. */
export function findActiveProfile(
  profiles: readonly ProfileSummary[],
  agents: readonly AgentSummary[],
  agent: string,
): ProfileSummary | undefined {
  const summary = agents.find((entry) => entry.agent === agent);
  // "external" means the config exists but was not written by this app, so no
  // profile of ours is responsible for it. Reporting one would be a lie.
  if (!summary || summary.activeKind !== "managed") return undefined;

  const name = summary.activeProfileName;
  if (!name) return undefined;

  return profiles.find((profile) => profile.name === name);
}

/** Whether `profile` is the one currently applied to `agent`. */
export function isProfileActive(
  profile: ProfileSummary,
  agents: readonly AgentSummary[],
  agent: string,
): boolean {
  const summary = agents.find((entry) => entry.agent === agent);
  return summary?.activeKind === "managed" && summary.activeProfileName === profile.name;
}
