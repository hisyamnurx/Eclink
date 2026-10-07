/**
 * Tests for the backend-identifier translations.
 *
 * The important test here is not that a lookup returns a string. It is that the
 * keys in this module still match what Rust actually sends. A renamed step id or
 * reason code in the backend would otherwise degrade silently to the English
 * fallback, which looks fine in development and is only noticed by a user reading
 * a half-translated screen.
 *
 * So the coverage is checked against the Rust source itself. That is unusual for a
 * frontend test, and it is the point: the two sides share a protocol, and nothing
 * else would catch the drift.
 */

import { readFileSync } from "node:fs";
import { resolve } from "node:path";

import { describe, expect, it } from "vitest";

import { KNOWN_REASON_CODES, KNOWN_STEP_IDS, reasonText, stepLabel } from "./backend-text";

const PIPELINE = readFileSync(
  resolve(__dirname, "../../src-tauri/src/testing/pipeline.rs"),
  "utf8",
);

describe("step labels", () => {
  it("translates a known step into both languages", () => {
    expect(stepLabel("authentication", "Authentication", "en")).toBe("Authentication");
    expect(stepLabel("authentication", "Authentication", "id")).toBe("Autentikasi");
    expect(stepLabel("protocol", "Agent Protocol", "id")).toBe("Protokol Agent");
  });

  it("falls back to the backend's own label for an unknown step", () => {
    // A newer backend may add a step. Showing its English is far better than
    // showing a blank row or a raw identifier.
    expect(stepLabel("brand_new_step", "Some New Check", "id")).toBe("Some New Check");
    expect(stepLabel(undefined, "Unlabelled", "id")).toBe("Unlabelled");
  });

  it("covers every step id the backend can emit", () => {
    // The ids are constructed as `id: "..."` in the pipeline. Reading them from the
    // source means adding a step without translating it fails here.
    const emitted = new Set<string>();
    for (const match of PIPELINE.matchAll(/id:\s*"([a-z_]+)"\.to_string\(\)/g)) {
      emitted.add(match[1] ?? "");
    }
    // Step rows built by `TestStep::new("...")` too.
    for (const match of PIPELINE.matchAll(/TestStep::new\(\s*"([a-z_]+)"/g)) {
      emitted.add(match[1] ?? "");
    }
    for (const match of PIPELINE.matchAll(/TestStep::skipped\(\s*"([a-z_]+)"/g)) {
      emitted.add(match[1] ?? "");
    }
    emitted.delete("");

    expect(emitted.size).toBeGreaterThan(0);
    const untranslated = [...emitted].filter((id) => !KNOWN_STEP_IDS.includes(id));
    expect(
      untranslated,
      `these step ids are emitted by the backend but have no translation, so they ` +
        `would show English in both languages: ${untranslated.join(", ")}`,
    ).toEqual([]);
  });

  it("does not translate a step the backend does not emit", () => {
    // A stale key is dead weight that suggests the two sides are in sync when they
    // are not.
    for (const id of KNOWN_STEP_IDS) {
      expect(
        PIPELINE.includes(`"${id}"`),
        `"${id}" is translated but the backend never emits it`,
      ).toBe(true);
    }
  });
});

describe("reason texts", () => {
  it("translates the reason that matters most", () => {
    // `protocol_unsupported` is what an OpenAI-shaped provider returns for Claude
    // Code. The user must read that their provider speaks a different API rather
    // than that something is broken.
    const english = reasonText("protocol_unsupported", "backend text", "en");
    const indonesian = reasonText("protocol_unsupported", "backend text", "id");
    expect(english).toContain("API format");
    expect(indonesian).toContain("format API");
    expect(indonesian).not.toBe(english);
  });

  it("falls back to the backend's reason for an unknown code", () => {
    // The backend's reason carries the detail specific to the run, so it beats a
    // generic sentence.
    expect(reasonText("not_a_real_code", "The backend explained this.", "id")).toBe(
      "The backend explained this.",
    );
    expect(reasonText(undefined, "Also backend text.", "id")).toBe("Also backend text.");
  });

  it("covers exactly the reason codes the backend emits", () => {
    // Exact equality in both directions. A missing code means the user reads
    // English; an extra code means this module claims coverage it does not have,
    // and the next person trusts it.
    //
    // The search is scoped to `finish` and `verdict`, which are the only places a
    // `TestOutcome::reason_code` is chosen. Searching the whole file would also
    // collect step ids, which are a different namespace with their own test.
    const emitted = new Set<string>();
    for (const helper of ["fn finish(", "fn verdict("]) {
      const after = PIPELINE.split(helper)[1] ?? "";
      expect(after, `${helper} was not found in pipeline.rs`).not.toBe("");
      const helperBody = after.slice(0, after.indexOf("\n}\n"));
      for (const match of helperBody.matchAll(/"([a-z_]+)"/g)) {
        if (match[1]) emitted.add(match[1]);
      }
    }

    expect(emitted.size, "no reason codes were parsed; the test is not checking").toBeGreaterThan(
      5,
    );

    const untranslated = [...emitted].filter((c) => !KNOWN_REASON_CODES.includes(c)).sort();
    expect(
      untranslated,
      `emitted by the backend but untranslated, so they would show English in both ` +
        `languages: ${untranslated.join(", ")}`,
    ).toEqual([]);

    const invented = KNOWN_REASON_CODES.filter((c) => !emitted.has(c)).sort();
    expect(
      invented,
      `translated here but never emitted by the backend: ${invented.join(", ")}`,
    ).toEqual([]);
  });
});

describe("the identifier protocol itself", () => {
  it("keeps the code sets free of duplicates and blanks", () => {
    expect(new Set(KNOWN_STEP_IDS).size).toBe(KNOWN_STEP_IDS.length);
    expect(new Set(KNOWN_REASON_CODES).size).toBe(KNOWN_REASON_CODES.length);
    for (const key of [...KNOWN_STEP_IDS, ...KNOWN_REASON_CODES]) {
      expect(key.trim()).toBe(key);
      expect(key).not.toBe("");
    }
  });

  it("uses snake_case identifiers, matching the backend's convention", () => {
    for (const key of [...KNOWN_STEP_IDS, ...KNOWN_REASON_CODES]) {
      expect(key, `${key} is not snake_case`).toMatch(/^[a-z][a-z0-9_]*$/);
    }
  });

  it("never renders an em-dash or en-dash in either language", () => {
    const dashes = /[\u2013\u2014]/;
    for (const language of ["en", "id"] as const) {
      for (const id of KNOWN_STEP_IDS) {
        expect(stepLabel(id, "", language), `${id}/${language}`).not.toMatch(dashes);
      }
      for (const code of KNOWN_REASON_CODES) {
        expect(reasonText(code, "", language), `${code}/${language}`).not.toMatch(dashes);
      }
    }
  });
});
