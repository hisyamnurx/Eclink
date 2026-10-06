/**
 * Formatting helpers shared across screens.
 *
 * Small and dependency-free: the UI must not pull in a date library for four
 * functions, and every formatter here is defensive so a malformed value from a
 * config file cannot crash a render.
 */

/** Format an ISO timestamp for display, e.g. "2 minutes ago". */
export function relativeTime(iso: string | undefined, now = Date.now()): string {
  if (!iso) return "never";
  const then = Date.parse(iso);
  if (Number.isNaN(then)) return "unknown";

  const seconds = Math.round((now - then) / 1000);
  if (seconds < 0) return "just now";
  if (seconds < 45) return "just now";
  if (seconds < 90) return "1 minute ago";

  const minutes = Math.round(seconds / 60);
  if (minutes < 60) return `${minutes} minutes ago`;
  if (minutes < 90) return "1 hour ago";

  const hours = Math.round(minutes / 60);
  if (hours < 24) return `${hours} hours ago`;
  if (hours < 36) return "yesterday";

  const days = Math.round(hours / 24);
  if (days < 30) return `${days} days ago`;

  const months = Math.round(days / 30);
  if (months < 12) return `${months} months ago`;

  const years = Math.round(months / 12);
  return years <= 1 ? "1 year ago" : `${years} years ago`;
}

/** Format an absolute timestamp, e.g. "12 Mar 2025, 14:05". */
export function absoluteTime(iso: string | undefined): string {
  if (!iso) return "unknown";
  const date = new Date(iso);
  if (Number.isNaN(date.getTime())) return "unknown";

  const day = date.getDate();
  const month = date.toLocaleString(undefined, { month: "short" });
  const year = date.getFullYear();
  const hours = String(date.getHours()).padStart(2, "0");
  const minutes = String(date.getMinutes()).padStart(2, "0");
  return `${day} ${month} ${year}, ${hours}:${minutes}`;
}

/** Format a latency in milliseconds for display. */
export function latency(ms: number | undefined): string {
  if (ms === undefined || ms === null || Number.isNaN(ms)) return "—";
  if (ms < 1000) return `${Math.round(ms)} ms`;
  return `${(ms / 1000).toFixed(1)} s`;
}

/** Format a byte count for display. */
export function bytes(count: number): string {
  if (!Number.isFinite(count) || count < 0) return "—";
  if (count < 1024) return `${count} B`;
  if (count < 1024 * 1024) return `${(count / 1024).toFixed(1)} KB`;
  return `${(count / (1024 * 1024)).toFixed(1)} MB`;
}

/** A short, human status word for a compatibility status. */
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

/**
 * Whether activation is permitted for a status.
 *
 * Mirrors `CompatibilityStatus::allows_activation` on the backend; kept in sync
 * deliberately so the button state matches what the command will actually allow.
 */
export function allowsActivation(status: string | undefined): boolean {
  return status === "compatible" || status === "partially_compatible";
}

/** Truncate long text for a single-line display. */
export function truncate(text: string, max = 60): string {
  if (text.length <= max) return text;
  return `${text.slice(0, max - 1)}…`;
}

/** The last path segment, for showing a folder name compactly. */
export function folderName(path: string): string {
  const trimmed = path.replace(/[\\/]+$/, "");
  const parts = trimmed.split(/[\\/]/);
  return parts[parts.length - 1] || trimmed;
}
