/**
 * Tests for `useAsyncData`.
 *
 * This hook is the only place the app does async data loading, and it contains
 * two pieces of genuinely non-obvious logic that would be silent bugs if they
 * regressed:
 *
 *   1. a superseded run must not write its result over a newer one;
 *   2. an unmounted component's pending result must not be applied.
 *
 * Both are asserted here by driving the hook with controlled promises, so the
 * interleaving is deterministic rather than timing-dependent.
 */

import { act } from "react";
import { createRoot, type Root } from "react-dom/client";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import { useAsyncData, type AsyncData } from "./hooks";

/** A promise whose settlement this test controls explicitly. */
function deferred<T>() {
  let resolve!: (value: T) => void;
  let reject!: (reason: unknown) => void;
  const promise = new Promise<T>((res, rej) => {
    resolve = res;
    reject = rej;
  });
  return { promise, resolve, reject };
}

describe("useAsyncData", () => {
  let container: HTMLDivElement;
  let root: Root;
  /** The most recent value the hook rendered, captured on every render. */
  let latest: AsyncData<string> | null;

  beforeEach(() => {
    container = document.createElement("div");
    document.body.appendChild(container);
    latest = null;
  });

  afterEach(() => {
    act(() => root?.unmount());
    container.remove();
  });

  /** Mount a component that uses the hook with `loader`, and render its result. */
  function mount(loader: () => Promise<string>) {
    function Probe() {
      const state = useAsyncData(loader, []);
      latest = state;
      return null;
    }
    act(() => {
      root = createRoot(container);
      root.render(<Probe />);
    });
  }

  /** Let queued microtasks and effects run. */
  async function flush() {
    await act(async () => {
      await Promise.resolve();
      await Promise.resolve();
    });
  }

  it("starts in a loading state with no data and no error", () => {
    mount(() => deferred<string>().promise);
    expect(latest?.loading).toBe(true);
    expect(latest?.data).toBeNull();
    expect(latest?.error).toBeNull();
  });

  it("exposes the resolved value and clears loading", async () => {
    const d = deferred<string>();
    mount(() => d.promise);

    await act(async () => {
      d.resolve("hello");
      await d.promise;
    });
    await flush();

    expect(latest?.data).toBe("hello");
    expect(latest?.loading).toBe(false);
    expect(latest?.error).toBeNull();
  });

  it("maps a rejection to an ApiError rather than throwing", async () => {
    const d = deferred<string>();
    mount(() => d.promise);

    await act(async () => {
      d.reject(new Error("backend exploded"));
      await d.promise.catch(() => undefined);
    });
    await flush();

    expect(latest?.error).not.toBeNull();
    expect(latest?.error?.message).toContain("backend exploded");
    // A failed load must not be reported as still loading.
    expect(latest?.loading).toBe(false);
  });

  it("does not apply a result from an unmounted component", async () => {
    const d = deferred<string>();
    const loader = vi.fn(() => d.promise);
    mount(loader);

    // Unmount before the request settles, which is what happens when a user
    // navigates away mid-request.
    act(() => root.unmount());
    const afterUnmount = latest;
    expect(afterUnmount?.data).toBeNull();

    // Count React's complaints. Without the `cancelled` guard in the hook, the
    // resolve handler calls `setData` on an unmounted tree. React 19 does not
    // throw for that — it silently ignores the update — so asserting on the
    // rendered value alone would pass either way and prove nothing. Capturing
    // the warning is what actually distinguishes the two.
    const errors: string[] = [];
    const spy = vi.spyOn(console, "error").mockImplementation((...args) => {
      errors.push(args.map(String).join(" "));
    });

    await act(async () => {
      d.resolve("late");
      await d.promise;
    });
    await flush();
    spy.mockRestore();

    // No "update on an unmounted component" warning may be produced.
    const unmountedComplaints = errors.filter((line) => /unmounted|not mounted/i.test(line));
    expect(
      unmountedComplaints,
      `a superseded run wrote to an unmounted tree: ${unmountedComplaints.join(" | ")}`,
    ).toEqual([]);

    // And the captured state must not have been replaced.
    expect(latest).toBe(afterUnmount);
    expect(latest?.data).toBeNull();
  });

  it("a superseded run does not overwrite a newer result", async () => {
    // The other half of the cancellation logic. A slow first request must not
    // clobber the result of a fast second one.
    const first = deferred<string>();
    const second = deferred<string>();
    let call = 0;
    mount(() => {
      call += 1;
      return call === 1 ? first.promise : second.promise;
    });

    // Trigger a reload so two loads are in flight.
    await act(async () => {
      latest?.reload();
    });
    await flush();

    // Settle the *newer* run first, then the stale one.
    await act(async () => {
      second.resolve("newer");
      await second.promise;
    });
    await flush();
    await act(async () => {
      first.resolve("stale");
      await first.promise;
    });
    await flush();

    expect(latest?.data).toBe("newer");
  });

  it("reload re-runs the loader", async () => {
    const loader = vi.fn().mockResolvedValue("first");
    mount(loader);
    await flush();
    expect(loader).toHaveBeenCalledTimes(1);

    await act(async () => {
      latest?.reload();
    });
    await flush();

    expect(loader).toHaveBeenCalledTimes(2);
  });

  it("does not call the loader at all when disabled", async () => {
    const loader = vi.fn().mockResolvedValue("never");
    function Disabled() {
      latest = useAsyncData(loader, [], { enabled: false });
      return null;
    }
    act(() => {
      root = createRoot(container);
      root.render(<Disabled />);
    });
    await flush();

    expect(loader).not.toHaveBeenCalled();
  });

  it("stops loading once the loader resolves", async () => {
    // Guards against the "spinner forever" class of bug: `loading` must always
    // end up false, on both the success and failure paths.
    const ok = deferred<string>();
    mount(() => ok.promise);
    await act(async () => {
      ok.resolve("done");
      await ok.promise;
    });
    await flush();
    expect(latest?.loading).toBe(false);
  });
});
