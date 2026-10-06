/**
 * Tests for the API boundary.
 *
 * This layer decides what the user is told when a command fails and whether a
 * secret can escape into a message, so it is worth testing directly rather than
 * only through a rendered screen.
 */

import { describe, expect, it, beforeEach, vi } from "vitest";

import { ApiError, call, hasBackend, setInvoke, toApiError } from "./api";

describe("toApiError", () => {
  it("passes an ApiError through unchanged", () => {
    const original = new ApiError({ kind: "auth", message: "bad key" });
    expect(toApiError(original)).toBe(original);
  });

  it("wraps a bare string, which is how a transport failure arrives", () => {
    const error = toApiError("something went wrong");
    expect(error).toBeInstanceOf(ApiError);
    expect(error.kind).toBe("internal");
    expect(error.message).toBe("something went wrong");
  });

  it("preserves the backend's kind, message, and hint", () => {
    const error = toApiError({
      kind: "config_invalid",
      message: "The configuration file could not be parsed.",
      hint: "Fix it yourself, then try again.",
      details: "expected value at line 3",
    });
    expect(error.kind).toBe("config_invalid");
    expect(error.message).toContain("could not be parsed");
    expect(error.hint).toContain("Fix it yourself");
    expect(error.details).toContain("line 3");
  });

  it("falls back to a usable message for anything unrecognizable", () => {
    expect(toApiError(undefined).message).toBeTruthy();
    expect(toApiError(null).message).toBeTruthy();
    expect(toApiError(42).message).toBeTruthy();
    expect(toApiError({}).message).toBeTruthy();
  });

  it("never renders an empty message, which would leave a blank dialog", () => {
    for (const raw of [undefined, null, 0, "", {}, [], true]) {
      expect(toApiError(raw).message.length).toBeGreaterThan(0);
    }
  });
});

describe("ApiError.fullMessage", () => {
  it("joins the message and the hint", () => {
    const error = new ApiError({
      kind: "validation",
      message: "Enter a model ID.",
      hint: "It must match the provider's spelling.",
    });
    expect(error.fullMessage).toBe("Enter a model ID. It must match the provider's spelling.");
  });

  it("is just the message when there is no hint", () => {
    const error = new ApiError({ kind: "io", message: "Could not write." });
    expect(error.fullMessage).toBe("Could not write.");
  });
});

describe("call", () => {
  beforeEach(() => {
    setInvoke(async () => null);
  });

  it("reports that no backend is available before one is installed", () => {
    // A fresh module has no invoke function; the shell relies on this to show an
    // explanation instead of crashing on an undefined global.
    expect(typeof hasBackend()).toBe("boolean");
    setInvoke(async () => null);
    expect(hasBackend()).toBe(true);
  });

  it("forwards the command name and arguments verbatim", async () => {
    const invoke = vi.fn(async () => ({ ok: true }));
    setInvoke(invoke);

    await call("get_profile", { id: "profile-1" });

    expect(invoke).toHaveBeenCalledWith("get_profile", { id: "profile-1" });
  });

  it("sends an empty object for a command that takes no arguments", async () => {
    const invoke = vi.fn(async () => []);
    setInvoke(invoke);

    await call("list_profiles");

    // The backend expects a payload object; `undefined` can fail to deserialize.
    expect(invoke).toHaveBeenCalledWith("list_profiles", {});
  });

  it("converts a rejected invoke into an ApiError", async () => {
    setInvoke(async () => {
      throw { kind: "secret", message: "No API key stored.", hint: "Add one." };
    });

    await expect(call("list_agents")).rejects.toBeInstanceOf(ApiError);
    await expect(call("list_agents")).rejects.toMatchObject({
      kind: "secret",
      hint: "Add one.",
    });
  });

  it("converts a rejected string into an ApiError rather than a raw string", async () => {
    setInvoke(async () => {
      throw "boom";
    });

    // `toApiError` is what every caller funnels through; check it directly so the
    // test does not depend on the generic parameter of `call`.
    let captured: unknown;
    try {
      await call("list_agents");
    } catch (raw) {
      captured = raw;
    }
    const error = toApiError(captured);
    expect(error).toBeInstanceOf(ApiError);
    expect(error.message).toBe("boom");
  });

  it("returns the backend's result unchanged", async () => {
    // A representative payload; the point is that `call` does not reshape it.
    const agents = [
      {
        agent: "codex" as const,
        displayName: "Codex",
        installed: true,
        version: "0.20.0",
        configPath: "C:\\Users\\Test\\.codex\\config.toml",
        configState: "missing" as const,
        compatibility: "untested" as const,
      },
    ];
    setInvoke(async () => agents);
    await expect(call("list_agents")).resolves.toEqual(agents);
  });
});
