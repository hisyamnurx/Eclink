/**
 * Install-location helpers.
 *
 * The Settings screen and the Overview card must agree about what a saved path
 * means and which sentence explains a failure. Keeping that mapping here means
 * neither screen can grow its own slightly different version of it.
 */

import type {
  AgentId,
  AgentLocation,
  AgentLocations,
  LocationKind,
  LocationMode,
  LocationProblem,
} from "./api";
import type { MessageKey } from "./i18n";

/** Extensions Eclink can execute, directly or through cmd.exe. */
const LAUNCHER_EXTENSION = /\.(exe|cmd|bat|com)$/i;

/**
 * Guess whether typed text names a launcher or a folder.
 *
 * A pasted path has no "kind" attached, so the extension decides. The backend
 * still validates the choice: a folder named `tools.exe`, or a file without an
 * executable extension, is refused with a message that says which it is.
 */
export function inferLocationKind(path: string): LocationKind {
  return LAUNCHER_EXTENSION.test(path.trim()) ? "launcher" : "folder";
}

export function configuredLocation(
  locations: AgentLocations | undefined,
  agent: AgentId,
): AgentLocation | undefined {
  return locations?.[agent];
}

/**
 * The mode to display, derived from the backend when it has reported one and
 * from the saved setting otherwise.
 */
export function effectiveLocationMode(
  mode: LocationMode | undefined,
  configured: AgentLocation | undefined,
): LocationMode {
  if (mode) return mode;
  if (!configured) return "automatic";
  return configured.kind === "launcher" ? "launcher" : "folder";
}

export function locationModeKey(mode: LocationMode): MessageKey {
  switch (mode) {
    case "folder":
      return "settings.locationFolder";
    case "launcher":
      return "settings.locationLauncher";
    default:
      return "settings.locationAutomatic";
  }
}

/**
 * The sentence explaining a failed saved location.
 *
 * Keyed by the backend's stable code rather than by its English message, so the
 * explanation follows the interface language.
 */
export function locationProblemKey(problem: LocationProblem): MessageKey {
  switch (problem) {
    case "missing":
      return "settings.locationProblemMissing";
    case "not_a_launcher":
      return "settings.locationProblemNotLauncher";
    case "timeout":
      return "settings.locationProblemTimeout";
    case "unsupported_path":
      return "settings.locationProblemUnsupported";
    default:
      return "settings.locationProblemUnusable";
  }
}
