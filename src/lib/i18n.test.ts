/**
 * Tests for the dictionaries themselves.
 *
 * Two properties matter and neither is enforced by the type system.
 *
 * The type system guarantees that `id` has every key `en` has, because `id` is
 * typed `Dictionary`. It does not guarantee the converse: a key added only to `id`
 * would be legal if `en` were widened, and more importantly it cannot tell whether
 * a key is used at all. A dictionary that accumulates entries nobody reads is worse
 * than a smaller one, because it makes the key set look authoritative when part of
 * it is dead.
 *
 * So these tests check both: the two dictionaries agree exactly, and every key is
 * referenced somewhere in the interface.
 */

import { readFileSync, readdirSync, statSync } from "node:fs";
import { join, resolve } from "node:path";

import { describe, expect, it } from "vitest";

import { LANGUAGES, dictionaryFor, translate } from "./i18n";

const SRC = resolve(__dirname, "..");
const I18N = resolve(__dirname, "i18n.ts");

/** Every frontend source file except the dictionary itself. */
function consumers(dir: string = SRC): string[] {
  const out: string[] = [];
  for (const entry of readdirSync(dir)) {
    const path = join(dir, entry);
    if (statSync(path).isDirectory()) {
      if (entry === "gen") continue;
      out.push(...consumers(path));
      continue;
    }
    if (!/\.(ts|tsx)$/.test(entry)) continue;
    if (path === I18N) continue;
    out.push(path);
  }
  return out;
}

/** The key names declared in the source, per dictionary block. */
function declaredKeys(): { en: string[]; id: string[] } {
  const source = readFileSync(I18N, "utf8");
  // `en` comes first, then `id`. Splitting on the `id` declaration is enough
  // because both are flat object literals with `"key":` at two-space indent.
  const [beforeId = "", afterId = ""] = source.split(/const id\s*:/);
  const pattern = /^\s{2}"([A-Za-z][A-Za-z0-9.]*)":/gm;
  return {
    en: [...beforeId.matchAll(pattern)].map((m) => m[1] ?? "").filter(Boolean),
    id: [...afterId.matchAll(pattern)].map((m) => m[1] ?? "").filter(Boolean),
  };
}

describe("the dictionaries", () => {
  it("declare exactly the same keys in every language", () => {
    const { en, id } = declaredKeys();
    expect(en.length).toBeGreaterThan(200);

    const onlyInEnglish = en.filter((key) => !id.includes(key)).sort();
    const onlyInIndonesian = id.filter((key) => !en.includes(key)).sort();
    expect(onlyInEnglish, `missing from Bahasa Indonesia: ${onlyInEnglish.join(", ")}`).toEqual([]);
    expect(onlyInIndonesian, `missing from English: ${onlyInIndonesian.join(", ")}`).toEqual([]);
  });

  it("declare each key exactly once per language", () => {
    // A duplicate silently shadows the earlier one, so the visible value is not the
    // one a reader finds first.
    for (const language of LANGUAGES) {
      const keys = language === "en" ? declaredKeys().en : declaredKeys().id;
      const duplicates = keys.filter((key, index) => keys.indexOf(key) !== index);
      expect([...new Set(duplicates)], `${language} declares these more than once`).toEqual([]);
    }
  });

  it("references every declared key from the interface", () => {
    // A key nothing reads is dead weight that makes the dictionary look more
    // complete than it is. Ninety had accumulated before this test existed.
    const { en } = declaredKeys();
    const source = consumers()
      .map((path) => readFileSync(path, "utf8"))
      .join("\n");

    const unused = en.filter((key) => !source.includes(`"${key}"`));
    expect(
      unused,
      `declared but never used, so remove them or use them: ${unused.join(", ")}`,
    ).toEqual([]);
  });

  it("covers every declared key in both languages at runtime", () => {
    // The source scan above reads the file; this reads the loaded objects, so a
    // build step that dropped an entry would be caught here.
    const { en } = declaredKeys();
    for (const language of LANGUAGES) {
      const dictionary = dictionaryFor(language);
      const missing = en.filter((key) => {
        const value = dictionary[key as keyof typeof dictionary];
        return typeof value !== "string" || value.trim() === "";
      });
      expect(missing, `${language} has no value for: ${missing.join(", ")}`).toEqual([]);
    }
  });

  it("keeps the two languages genuinely different", () => {
    // A value pasted into both languages would ship Indonesian users an English
    // interface that looks translated. Brand names and technical nouns are
    // legitimately identical, so they are compared as a group rather than one by one.
    const en = dictionaryFor("en");
    const id = dictionaryFor("id");
    const keys = Object.keys(en) as Array<keyof typeof en>;

    const identical = keys.filter((key) => en[key] === id[key]);
    // Some are expected: "Playground", "Streaming", "Version", and similar.
    expect(
      identical.length,
      `${identical.length} keys are identical in both languages, which is more than ` +
        `brand names and shared technical nouns would explain. Examples: ` +
        identical.slice(0, 10).join(", "),
    ).toBeLessThan(keys.length / 4);
  });

  it("names keys with a dotted screen prefix", () => {
    // The prefix is what makes a key findable and keeps the file grouped.
    for (const key of declaredKeys().en) {
      expect(key, `"${key}" has no screen prefix`).toMatch(/^[a-z]+\.[a-zA-Z]/);
    }
  });
});

describe("translate", () => {
  it("substitutes every placeholder it is given", () => {
    const en = dictionaryFor("en");
    expect(translate(en, "test.summary", { passed: 7, total: 7 })).toBe("7 of 7 checks passed");
  });

  it("leaves an unfilled placeholder visible rather than printing undefined", () => {
    // A formatting mistake should be obvious in the interface, not read as the
    // word "undefined" in the middle of a sentence.
    const en = dictionaryFor("en");
    const out = translate(en, "test.summary", { passed: 1 });
    expect(out).toContain("{total}");
    expect(out).not.toContain("undefined");
  });

  it("returns the key when it is unknown, so a mistake is visible", () => {
    const en = dictionaryFor("en");
    // Deliberately bypass the type to model a value arriving at runtime.
    const out = translate(en, "not.a.real.key" as never);
    expect(out).toBe("not.a.real.key");
  });

  it("returns plain text unchanged when there is nothing to substitute", () => {
    const en = dictionaryFor("en");
    expect(translate(en, "nav.overview")).toBe("Overview");
    expect(translate(dictionaryFor("id"), "nav.overview")).toBe("Ringkasan");
  });
});
