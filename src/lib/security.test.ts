/**
 * Tests for the frontend's security-relevant helpers.
 *
 * These are the rules that decide whether a credential is sent somewhere the
 * user did not intend, so they are tested directly rather than trusted to
 * inspection.
 */

import { describe, expect, it } from "vitest";

import { openUrl } from "./opener";

/**
 * The insecure-endpoint rule lives inside the editor component. It is
 * reproduced here as a standalone predicate so the rule itself is pinned: an
 * endpoint reached over plain HTTP, and not on this machine, must be flagged.
 */
function wouldBeInsecure(baseUrl: string): boolean {
  if (!/^http:\/\//i.test(baseUrl.trim())) return false;
  return !/^http:\/\/(localhost|127\.0\.0\.1|\[::1\]|0\.0\.0\.0|[^/]*\.local)(:|\/|$)/i.test(
    baseUrl.trim(),
  );
}

describe("insecure endpoint detection", () => {
  it("flags plain HTTP to a remote host", () => {
    expect(wouldBeInsecure("http://api.example.com/v1")).toBe(true);
    expect(wouldBeInsecure("http://192.168.1.50:8080/v1")).toBe(true);
  });

  it("does not flag HTTPS", () => {
    expect(wouldBeInsecure("https://api.example.com/v1")).toBe(false);
  });

  it("does not flag a loopback endpoint, where plain HTTP is expected", () => {
    expect(wouldBeInsecure("http://localhost:11434/v1")).toBe(false);
    expect(wouldBeInsecure("http://127.0.0.1:8080/v1")).toBe(false);
    expect(wouldBeInsecure("http://[::1]:8080/v1")).toBe(false);
  });

  it("does not flag a .local host", () => {
    expect(wouldBeInsecure("http://ollama.local:11434/v1")).toBe(false);
  });

  it("is case-insensitive about the scheme and host", () => {
    expect(wouldBeInsecure("HTTP://API.EXAMPLE.COM/v1")).toBe(true);
    expect(wouldBeInsecure("http://LOCALHOST:8080/v1")).toBe(false);
  });

  it("does not treat a host merely starting with 'localhost' as local", () => {
    // "localhost.evil.com" resolves on the public internet.
    expect(wouldBeInsecure("http://localhost.evil.com/v1")).toBe(true);
  });

  it("does not treat 127.0.0.1 as a prefix match for another host", () => {
    expect(wouldBeInsecure("http://127.0.0.1.evil.com/v1")).toBe(true);
  });
});

describe("openUrl", () => {
  it("refuses a non-web scheme", async () => {
    // A file: or javascript: URL arriving from a config file must never become
    // a launch.
    await expect(openUrl("file:///C:/Windows/System32/calc.exe")).rejects.toThrow();
    await expect(openUrl("javascript:alert(1)")).rejects.toThrow();
    await expect(openUrl("ms-settings:privacy")).rejects.toThrow();
  });

  it("refuses a scheme-relative or relative URL", async () => {
    await expect(openUrl("//example.com")).rejects.toThrow();
    await expect(openUrl("/etc/passwd")).rejects.toThrow();
    await expect(openUrl("")).rejects.toThrow();
  });

  it("accepts http and https, reporting a plugin failure rather than a validation one", async () => {
    // No Tauri plugin is present in this environment, so the call fails - but it
    // must fail as "could not open your browser", not as "that link is invalid",
    // which proves the URL passed validation.
    const httpError = await openUrl("https://example.com").catch((raw: unknown) => raw);
    expect(String(httpError)).toMatch(/Could not open your browser/);

    const httpOnly = await openUrl("http://example.com").catch((raw: unknown) => raw);
    expect(String(httpOnly)).toMatch(/Could not open your browser/);
  });
});
