/**
 * The single source of truth for what a compatibility status means in the UI.
 *
 * This module exists because of a real bug. The badge and the Activate button
 * previously derived their state independently in the component, and the test
 * handler updated only one of them, so a run that reported 7/7 could leave the
 * badge reading "Not tested" with Activate disabled. Deriving both from one
 * function makes that class of disagreement impossible to express.
 *
 * It also keeps the client rule in step with the backend's
 * `CompatibilityStatus::allows_activation`: if they disagree, the button is
 * enabled for an action the command will refuse.
 */

import type { AgentSummary, ProfileSummary } from "./api";

/** Whether a status permits activation. Mirrors the backend rule. */
export function allowsActivation(status: string | undefined): boolean {
  return status === "compatible" || status === "partially_compatible";
}

/** A short, human status word. */
export function compatibilityLabel(status: string | undefined): string {
  switch (status) {
    case "compatible":
      return "Compatible";
    case "partially_compatible":
      return "Partially compatible";
    case "incompatible":
      return "Incompatible";
    case "unknown":
      return "Not determined";
    case "test_failed":
      return "Test failed";
    case "testing":
      return "Testing…";
    case "untested":
      return "Not tested";
    default:
      return "Not tested";
  }
}

/** The compatibility record for one agent, if there is one. */
export function recordFor(profile: ProfileSummary, agent: string) {
  return profile.compatibility?.[agent as keyof typeof profile.compatibility];
}

export interface ActivationGate {
  /** The status word shown in the badge. */
  label: string;
  /** Whether the Activate button is enabled. */
  canActivate: boolean;
  /** Why activation is blocked, for the button's tooltip. */
  blockedReason?: string;
  /** Whether the profile is the one currently applied to this agent. */
  isActive: boolean;
}

/**
 * Everything the row needs to render its badge and its Activate button.
 *
 * Computed in one place and from one value, so the badge can never claim the
 * endpoint is usable while the button says it is not.
 */
export function activationGate(
  profile: ProfileSummary,
  agents: readonly AgentSummary[],
  agent: string,
): ActivationGate {
  const status = recordFor(profile, agent)?.status;
  const label = compatibilityLabel(status);
  const summary = agents.find((entry) => entry.agent === agent);
  const isActive =
    summary?.activeKind === "managed" && summary.activeProfileName === profile.name;

  if (!profile.hasSecret) {
    return { label, canActivate: false, blockedReason: "Add an API key before activating.", isActive };
  }
  if (!allowsActivation(status)) {
    return {
      label,
      canActivate: false,
      blockedReason: "Test this provider against the agent first.",
      isActive,
    };
  }
  return { label, canActivate: true, isActive };
}
