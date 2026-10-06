/**
 * Data-loading hooks.
 *
 * `useAsyncData` exists so screens do not each invent their own
 * loading/error/refetch bookkeeping — and, more importantly, so none of them
 * call `setState` synchronously in an effect body, which causes cascading
 * renders and is a real performance bug rather than a style preference.
 */

import { useCallback, useEffect, useRef, useState } from "react";

import { toApiError, type ApiError } from "./api";

export interface AsyncData<T> {
  data: T | null;
  error: ApiError | null;
  loading: boolean;
  /** Re-run the loader. */
  reload: () => void;
}

/**
 * Run an async loader whenever `deps` change, tracking loading and error state.
 *
 * Results from a superseded run are discarded, so a slow first request cannot
 * overwrite a fast second one.
 */
export function useAsyncData<T>(
  loader: () => Promise<T>,
  deps: readonly unknown[],
  options?: { enabled?: boolean },
): AsyncData<T> {
  const [data, setData] = useState<T | null>(null);
  const [error, setError] = useState<ApiError | null>(null);
  const [loading, setLoading] = useState(true);
  const [nonce, setNonce] = useState(0);

  // Keep the latest loader without making it a dependency, so callers can pass
  // an inline closure without re-running on every render. Writing the ref in an
  // effect (not during render) keeps render pure.
  const loaderRef = useRef(loader);
  useEffect(() => {
    loaderRef.current = loader;
  });

  const enabled = options?.enabled ?? true;

  useEffect(() => {
    if (!enabled) {
      return;
    }

    let cancelled = false;
    // Set inside the async callback rather than synchronously, so the first
    // render is not followed by an immediate cascading re-render.
    void Promise.resolve()
      .then(() => loaderRef.current())
      .then((result) => {
        if (cancelled) return;
        setData(result);
        setError(null);
        setLoading(false);
      })
      .catch((raw: unknown) => {
        if (cancelled) return;
        setError(toApiError(raw));
        setLoading(false);
      });

    return () => {
      cancelled = true;
    };
    // The caller owns the dependency list deliberately; this hook is a wrapper,
    // not a linter.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [enabled, nonce, ...deps]);

  const reload = useCallback(() => setNonce((value) => value + 1), []);

  return { data, error, loading, reload };
}
