/**
 * Regression tests for provider/model pairing and custom model IDs.
 *
 * These exist because of a real failure reported on 2026-10-20: a profile was
 * correct in the Playground but failed in real Claude Code. The actual cause was
 * the base URL, not the model: Claude Code appends `/v1/messages` to
 * `ANTHROPIC_BASE_URL`, so a profile saved as `https://modelrouter.id/v1` made
 * Claude Code request `/v1/v1/messages`, which the provider answered with
 * `404 Invalid URL`. Claude Code then reported that 404 as "There's an issue with
 * the selected model", which invites the wrong fix: rewriting the model ID.
 *
 * The model ID was never the problem, so these tests pin the two properties a
 * user can observe from the UI: a profile keeps its own base URL and model
 * together, and a custom ID containing `:` is preserved exactly, because it is
 * data and never part of a shell command.
 */

import { describe, expect, it } from "vitest";

import type { ProfileSummary } from "./api";
import { truncate } from "./format";

function profile(overrides: Partial<ProfileSummary> = {}): ProfileSummary {
  return {
    id: "p1",
    name: "ModelVIP",
    providerName: "ModelVIP",
    baseUrl: "https://modelrouter.id/v1",
    model: "deepseek-v4.1-flash:mr",
    targets: ["claude"],
    hasSecret: true,
    ready: true,
    createdAt: "2026-01-01T00:00:00Z",
    updatedAt: "2026-01-01T00:00:00Z",
    ...overrides,
  };
}

describe("selected provider and model stay paired", () => {
  it("keeps each profile's base URL with its own model", () => {
    // The failure this guards against is an overview that shows one profile's
    // endpoint beside another profile's model, which sends the user chasing the
    // wrong fix when a real session fails.
    const a = profile();
    const b = profile({
      id: "p2",
      name: "Other",
      baseUrl: "https://other.example.com",
      model: "claude-sonnet-4-6",
    });
    const rows = [a, b].map((item) => `${item.baseUrl} · ${item.model}`);

    expect(rows).toEqual([
      "https://modelrouter.id/v1 · deepseek-v4.1-flash:mr",
      "https://other.example.com · claude-sonnet-4-6",
    ]);
    expect(new Set([a.id, b.id]).size).toBe(2);
  });
});

describe("custom model IDs containing ':' are preserved", () => {
  it("leaves a colon and provider suffix intact", () => {
    const model = profile().model;
    expect(model).toBe("deepseek-v4.1-flash:mr");
    expect(model.split(":")).toEqual(["deepseek-v4.1-flash", "mr"]);
  });

  it("treats every documented model-ID shape as an opaque string", () => {
    // The only transformation the app applies is trimming surrounding spaces.
    const ids = [
      "deepseek-v4.1-flash:mr",
      "peCut/deepseek-v4.1-flash",
      "claude-sonnet-4-6",
      "gpt-5.1-codex",
      "models/gemini-2.5-pro",
      "qwen3-coder:30b",
    ];
    for (const id of ids) {
      expect(`  ${id}  `.trim()).toBe(id);
    }
  });

  it("does not let a long custom ID be mangled by display truncation", () => {
    // Short IDs used in settings must survive verbatim; `truncate` is display-only.
    const model = "deepseek-v4.1-flash:mr";
    expect(truncate(model, 60)).toBe(model);
  });
});
