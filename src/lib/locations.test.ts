/**
 * Tests for the install-location display rules.
 *
 * The backend decides what is installed; this module decides how that decision
 * is described. Those mappings are small but they are exactly the kind of table
 * that silently loses a case, leaving a user with a blank banner instead of a
 * reason, so each one is exercised here.
 */

import { describe, expect, it } from "vitest";

import type { AgentLocation, LocationMode, LocationProblem } from "./api";
import { dictionaryFor } from "./i18n";
import {
  configuredLocation,
  effectiveLocationMode,
  inferLocationKind,
  locationModeKey,
  locationProblemKey,
} from "./locations";

const en = dictionaryFor("en");
const id = dictionaryFor("id");

const folder: AgentLocation = { path: "D:\\Tools\\codex", kind: "folder" };
const launcher: AgentLocation = { path: "D:\\Tools\\codex\\codex.exe", kind: "launcher" };

describe("inferLocationKind", () => {
  it("treats every executable extension as a launcher", () => {
    for (const path of [
      "D:\\Tools\\codex.exe",
      "D:\\Tools\\codex.CMD",
      "D:\\Tools\\codex.bat",
      "D:\\Tools\\codex.com",
    ]) {
      expect(inferLocationKind(path), path).toBe("launcher");
    }
  });

  it("treats anything else as a folder", () => {
    for (const path of [
      "D:\\Tools\\codex",
      "D:\\Tools\\codex.exe\\",
      "D:\\Program Files\\Oh My Pi",
      "",
    ]) {
      expect(inferLocationKind(path), path).toBe("folder");
    }
  });

  it("ignores surrounding whitespace", () => {
    expect(inferLocationKind("  D:\\Tools\\codex.exe  ")).toBe("launcher");
  });
});

describe("configuredLocation", () => {
  it("returns the saved entry for one agent only", () => {
    const locations = { codex: folder, omp: launcher };
    expect(configuredLocation(locations, "codex")).toEqual(folder);
    expect(configuredLocation(locations, "omp")).toEqual(launcher);
    expect(configuredLocation(locations, "claude")).toBeUndefined();
  });

  it("survives a settings file that predates the field", () => {
    expect(configuredLocation(undefined, "codex")).toBeUndefined();
  });
});

describe("effectiveLocationMode", () => {
  it("prefers the mode the backend reported", () => {
    expect(effectiveLocationMode("automatic", folder)).toBe("automatic");
    expect(effectiveLocationMode("folder", folder)).toBe("folder");
  });

  it("falls back to the saved setting when detection has not replied yet", () => {
    expect(effectiveLocationMode(undefined, undefined)).toBe("automatic");
    expect(effectiveLocationMode(undefined, folder)).toBe("folder");
    expect(effectiveLocationMode(undefined, launcher)).toBe("launcher");
  });
});

describe("locationModeKey", () => {
  it("has a translated label for every mode", () => {
    const modes: LocationMode[] = ["automatic", "folder", "launcher"];
    for (const mode of modes) {
      const key = locationModeKey(mode);
      expect(en[key], `English is missing ${key}`).toBeTruthy();
      expect(id[key], `Indonesian is missing ${key}`).toBeTruthy();
    }
  });
});

describe("locationProblemKey", () => {
  it("has a translated sentence for every problem code", () => {
    const problems: LocationProblem[] = [
      "missing",
      "not_a_launcher",
      "unusable",
      "timeout",
      "unsupported_path",
    ];
    for (const problem of problems) {
      const key = locationProblemKey(problem);
      expect(en[key], `English is missing ${key}`).toBeTruthy();
      expect(id[key], `Indonesian is missing ${key}`).toBeTruthy();
      // The path is always shown, so every sentence must accept one.
      expect(en[key]).toContain("{path}");
    }
  });
});
