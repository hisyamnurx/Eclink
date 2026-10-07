/**
 * Tests for the display helpers.
 *
 * These are the functions that decide whether the UI tells the truth about what
 * Eclyzer Bridge knows, so the interesting cases are the ones where a careless
 * implementation would overstate certainty.
 */

import { describe, expect, it } from "vitest";

import { absoluteTime, bytes, folderName, latency, relativeTime, truncate } from "./format";

describe("relativeTime", () => {
  const now = Date.parse("2025-10-06T12:00:00Z");

  it("describes a recent time as 'just now'", () => {
    expect(relativeTime("2025-10-06T11:59:50Z", now)).toBe("just now");
  });

  it("counts minutes, hours, and days", () => {
    expect(relativeTime("2025-10-06T11:55:00Z", now)).toBe("5 minutes ago");
    expect(relativeTime("2025-10-06T09:00:00Z", now)).toBe("3 hours ago");
    expect(relativeTime("2025-10-03T12:00:00Z", now)).toBe("3 days ago");
  });

  it("says 'never' for a missing timestamp rather than inventing one", () => {
    expect(relativeTime(undefined, now)).toBe("never");
    expect(relativeTime("", now)).toBe("never");
  });

  it("does not crash on an unparsable timestamp", () => {
    expect(relativeTime("not a date", now)).toBe("unknown");
  });

  it("does not report a future timestamp as a negative age", () => {
    // A clock skew between the app and a config file must not render "-3 hours".
    const result = relativeTime("2025-10-06T13:00:00Z", now);
    expect(result).not.toContain("-");
    expect(result).toBe("just now");
  });
});

describe("absoluteTime", () => {
  it("formats a valid timestamp", () => {
    const result = absoluteTime("2025-03-12T14:05:00Z");
    expect(result).toMatch(/12 Mar 2025/);
    expect(result).toMatch(/\d{2}:\d{2}/);
  });

  it("returns a readable placeholder for a bad value", () => {
    expect(absoluteTime(undefined)).toBe("unknown");
    expect(absoluteTime("garbage")).toBe("unknown");
  });
});

describe("latency", () => {
  it("shows milliseconds below a second", () => {
    expect(latency(240)).toBe("240 ms");
    expect(latency(999)).toBe("999 ms");
  });

  it("switches to seconds at and above a second", () => {
    expect(latency(1000)).toBe("1.0 s");
    expect(latency(12_340)).toBe("12.3 s");
  });

  it("shows a short dash rather than NaN when there is no measurement", () => {
    // A plain hyphen, not an em-dash. The interface uses short dashes throughout,
    // and an em-dash here was the only long one in the product; it also rendered as
    // mojibake once, because the file was rewritten through a non-UTF-8 round trip.
    expect(latency(undefined)).toBe("-");
    expect(latency(Number.NaN)).toBe("-");
  });

  it("never returns an em-dash or en-dash", () => {
    // Asserted as a rule rather than only at the two call sites above, so a future
    // placeholder cannot reintroduce one.
    const longDashes = /[\u2013\u2014]/;
    for (const value of [undefined, Number.NaN, 0, 1, 999, 1000, 12_345]) {
      expect(latency(value), `latency(${String(value)})`).not.toMatch(longDashes);
    }
  });
});

describe("bytes", () => {
  it("scales the unit", () => {
    expect(bytes(512)).toBe("512 B");
    expect(bytes(2048)).toBe("2.0 KB");
    expect(bytes(5 * 1024 * 1024)).toBe("5.0 MB");
  });

  it("does not render a negative or impossible size", () => {
    expect(bytes(-1)).toBe("-");
    expect(bytes(Number.POSITIVE_INFINITY)).toBe("-");
  });

  it("never returns an em-dash or en-dash", () => {
    const longDashes = /[\u2013\u2014]/;
    for (const value of [-1, 0, 1, 1024, 1024 * 1024, Number.NaN, Number.POSITIVE_INFINITY]) {
      expect(bytes(value), `bytes(${String(value)})`).not.toMatch(longDashes);
    }
  });
});

/* The `compatibilityLabel` and `allowsActivation` suites moved to
 * compatibility.test.ts, alongside the rest of the compatibility rules. Keeping
 * them here would have meant two files asserting the same contract from two
 * different modules. */

describe("truncate", () => {
  it("leaves short text alone", () => {
    expect(truncate("short", 20)).toBe("short");
  });

  it("adds an ellipsis when it cuts", () => {
    const result = truncate("a".repeat(100), 10);
    expect(result).toHaveLength(10);
    expect(result.endsWith("…")).toBe(true);
  });
});

describe("folderName", () => {
  it("returns the last segment of a Windows path", () => {
    expect(folderName("C:\\Projects\\my-app")).toBe("my-app");
  });

  it("ignores a trailing separator", () => {
    expect(folderName("C:\\Projects\\my-app\\")).toBe("my-app");
  });

  it("handles a Unix-style path, which can appear in a config file", () => {
    expect(folderName("/home/user/project")).toBe("project");
  });

  it("returns the input when there is nothing to split", () => {
    expect(folderName("my-app")).toBe("my-app");
  });
});
