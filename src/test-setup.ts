/**
 * Vitest setup, applied before every test file.
 *
 * React 19 only accepts `act()` when this flag is present in the environment.
 * Without it, component tests print
 *
 *   "The current testing environment is not configured to support act(...)"
 *
 * and - more importantly - state updates are not flushed deterministically, so a
 * hook test can appear to pass while never observing the update it asserts on.
 * Setting it here keeps every suite honest rather than only the ones that
 * remember to set it themselves.
 */

// React reads this flag off the global object rather than off `window`, and it
// is not part of the standard typings, so it is set through a narrow cast here
// instead of declaring a new ambient global.
(globalThis as typeof globalThis & { IS_REACT_ACT_ENVIRONMENT: boolean }).IS_REACT_ACT_ENVIRONMENT =
  true;

// jsdom does not implement these, and a component that reports a failure must
// not crash the test run itself.
if (!window.matchMedia) {
  window.matchMedia = ((query: string) => ({
    matches: false,
    media: query,
    onchange: null,
    addListener: () => {},
    removeListener: () => {},
    addEventListener: () => {},
    removeEventListener: () => {},
    dispatchEvent: () => false,
  })) as typeof window.matchMedia;
}

export {};
