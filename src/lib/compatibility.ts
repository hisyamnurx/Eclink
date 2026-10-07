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
import type { MessageKey } from "./i18n";

/** Whether a status permits activation. Mirrors the backend rule. */
export function allowsActivation(status: string | undefined): boolean {
  return status === "compatible" || status === "partially_compatible";
}

/**
 * The status word as a message key, for the caller to resolve with `t()`.
 *
 * A key rather than a literal, so the badge reads in the user's language. The
 * words used to be duplicated here and in the dictionary, which is exactly how
 * the two drifted apart.
 */
export function compatibilityLabel(status: string | undefined): MessageKey {
  switch (status) {
    case "compatible":
      return "status.compatible";
    case "partially_compatible":
      return "status.partiallyCompatible";
    case "incompatible":
      return "status.incompatible";
    case "unknown":
      return "status.notDetermined";
    case "test_failed":
      return "status.testFailed";
    case "testing":
      return "status.testing";
    case "untested":
      return "status.notTested";
    default:
      return "status.notTested";
  }
}

/** The compatibility record for one agent, if there is one. */
export function recordFor(profile: ProfileSummary, agent: string) {
  return profile.compatibility?.[agent as keyof typeof profile.compatibility];
}

export interface ActivationGate {
  /** The status word shown in the badge, as a key for `t()`. */
  label: MessageKey;
  /** Whether the Activate button is enabled. */
  canActivate: boolean;
  /** Why activation is blocked, as a key for `t()`, for the button's tooltip. */
  blockedReason?: MessageKey;
  /** Whether the profile is the one currently applied to this agent. */
  isActive: boolean;
}

/**
 * Everything the row needs to render its badge and its Activate button.
 *
 * Computed in one place and from one value, so the badge can never claim the
 * endpoint is usable while the button says it is not. The two text values are
 * message keys rather than sentences, so the row can render them in either
 * language without this module knowing which one is active.
 */
export function activationGate(
  profile: ProfileSummary,
  agents: readonly AgentSummary[],
  agent: string,
): ActivationGate {
  const status = recordFor(profile, agent)?.status;
  const label = compatibilityLabel(status);
  const summary = agents.find((entry) => entry.agent === agent);
  const isActive = summary?.activeKind === "managed" && summary.activeProfileName === profile.name;

  if (!profile.hasSecret) {
    return { label, canActivate: false, blockedReason: "providers.addKeyFirst", isActive };
  }
  if (!allowsActivation(status)) {
    return {
      label,
      canActivate: false,
      blockedReason: "providers.testFirst",
      isActive,
    };
  }
  return { label, canActivate: true, isActive };
}
