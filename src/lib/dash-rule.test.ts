/**
 * Enforces the interface's dash rule.
 *
 * The interface uses a short hyphen `-` everywhere. An em-dash (U+2014) or en-dash
 * (U+2013) must not appear in any user-facing text, in either language.
 *
 * This is checked across every frontend source file rather than only the
 * dictionaries, because the offending characters were never in one place: they sat
 * in a formatter's placeholder, in a status glyph table, and in one page's markup,
 * and a per-file check would have to be updated every time a file is added.
 *
 * The rule covers comments too. That is deliberate: it costs nothing to write a
 * hyphen in a comment, and it means this test needs no judgement about which text
 * is "user-facing" - a distinction that is easy to get wrong and impossible to
 * enforce mechanically.
 */

import { readFileSync, readdirSync, statSync } from "node:fs";
import { join, relative, resolve } from "node:path";

import { describe, expect, it } from "vitest";

const SRC = resolve(__dirname, "..");

/** Every source file whose text a user could see. */
function sources(dir: string = SRC): string[] {
  const out: string[] = [];
  for (const entry of readdirSync(dir)) {
    const path = join(dir, entry);
    if (statSync(path).isDirectory()) {
      // Test setup and generated files are not shipped text.
      if (entry === "gen") continue;
      out.push(...sources(path));
      continue;
    }
    if (/\.(ts|tsx|css)$/.test(entry)) {
      out.push(path);
    }
  }
  return out;
}

const LONG_DASH = /[\u2013\u2014]/;

describe("the dash rule", () => {
  it("scans a meaningful number of files", () => {
    // Guards the guard: a broken glob would make every assertion below vacuous.
    const files = sources();
    expect(files.length).toBeGreaterThan(10);
  });

  it("has no em-dash or en-dash in any frontend source file", () => {
    const offenders: string[] = [];
    for (const path of sources()) {
      const lines = readFileSync(path, "utf8").split(/\r?\n/);
      lines.forEach((line, index) => {
        if (LONG_DASH.test(line)) {
          const name = relative(SRC, path).replace(/\\/g, "/");
          // Name the character so the report is unambiguous.
          const which = line.includes("\u2014") ? "em-dash (U+2014)" : "en-dash (U+2013)";
          offenders.push(`${name}:${index + 1}: ${which}: ${line.trim()}`);
        }
      });
    }
    expect(
      offenders,
      `the interface uses a short hyphen, so remove these:\n  ${offenders.join("\n  ")}`,
    ).toEqual([]);
  });

  it("has no em-dash or en-dash in the English or Indonesian dictionaries", () => {
    // Checked separately from the sweep above so a failure names the dictionaries,
    // which is where a translator is most likely to paste one in from a document.
    const i18n = readFileSync(join(SRC, "lib", "i18n.ts"), "utf8");
    const translated = i18n
      .split(/\r?\n/)
      .map((line, index) => ({ line, index: index + 1 }))
      .filter(({ line }) => /^\s*"/.test(line) || /^\s*"[a-z]/.test(line));

    expect(translated.length, "no dictionary lines were found").toBeGreaterThan(200);

    const offenders = translated.filter(({ line }) => LONG_DASH.test(line));
    expect(
      offenders.map(({ line, index }) => `i18n.ts:${index}: ${line.trim()}`),
      "a dictionary value contains a long dash",
    ).toEqual([]);
  });

  it("detects the characters it claims to detect", () => {
    // A sanity check on the pattern itself, so a typo in the escape sequence cannot
    // make the two tests above pass for the wrong reason.
    expect(LONG_DASH.test("a\u2014b")).toBe(true);
    expect(LONG_DASH.test("a\u2013b")).toBe(true);
    expect(LONG_DASH.test("a-b")).toBe(false);
    expect(LONG_DASH.test("a - b")).toBe(false);
  });
});
