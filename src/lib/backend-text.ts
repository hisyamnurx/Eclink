/**
 * Localised text for values that originate in Rust.
 *
 * The backend describes a test run with two kinds of stable identifier: each step
 * has an `id` such as `authentication`, and an outcome has a `reasonCode` such as
 * `protocol_unsupported`. It also sends its own English `label` and `reason` text
 * for each.
 *
 * Those English strings are what a user reads on the test-result screen, so they
 * have to be translatable. Translating them by posting the English through a
 * dictionary would be fragile, because it breaks the moment the backend rewords a
 * sentence. Translating by the stable identifier does not, and it is why those
 * identifiers exist.
 *
 * This is deliberately a separate module from `i18n.ts`. The UI dictionary holds
 * strings the interface authors, where the set of keys is closed and a missing one
 * is a build error. These keys describe a protocol shared with Rust, where the
 * backend may legitimately send an identifier this build has never heard of, so the
 * lookup has to be fallible and fall back to the backend's own wording rather than
 * showing nothing. Different problem, different shape.
 *
 * The fallback is the important part: an unrecognised identifier renders the
 * backend's English rather than a blank line or a raw `snake_case` token, so a
 * newer backend against an older frontend degrades to readable English instead of
 * looking broken.
 */

import type { Language } from "./i18n";

/** Text for a backend identifier, per language. */
type BackendText = { en: string; id: string };

/**
 * Test step labels, keyed by the step `id` the backend sends.
 *
 * The ids are stable machine values and are asserted against the real backend in
 * `backend-text.test.ts`; the labels are what the user reads.
 */
const STEP_LABELS: Record<string, BackendText> = {
  validation: { en: "Input Validation", id: "Validasi Input" },
  reachability: { en: "Reachability", id: "Keterjangkauan" },
  authentication: { en: "Authentication", id: "Autentikasi" },
  model: { en: "Model Availability", id: "Ketersediaan Model" },
  protocol: { en: "Agent Protocol", id: "Protokol Agent" },
  streaming: { en: "Streaming", id: "Streaming" },
  tools: { en: "Tool Calling", id: "Tool Calling" },
};

/**
 * Verdict reasons, keyed by `reasonCode`.
 *
 * This set is exactly what `testing/pipeline.rs` can put in
 * `TestOutcome::reason_code`, no more and no less, and `backend-text.test.ts`
 * asserts that by reading the Rust source. An invented entry here is not harmless:
 * it looks like coverage while the code the backend really sends goes
 * untranslated.
 *
 * `protocol_unsupported` is the one that matters most in practice: it is what an
 * OpenAI-shaped provider returns when Claude Code is tested against it, and the
 * user needs to read that their provider speaks a different API rather than that
 * their key is wrong.
 */
const REASON_TEXTS: Record<string, BackendText> = {
  compatible: {
    en: "The endpoint passed every check this agent needs.",
    id: "Endpoint lolos semua pemeriksaan yang dibutuhkan agent ini.",
  },
  unauthorized: {
    en: "The endpoint rejected the API key. Check that the key is correct and active.",
    id: "Endpoint menolak API key. Periksa apakah key-nya benar dan masih aktif.",
  },
  unknown_model: {
    en: "The endpoint does not recognize this model. Check the model ID exactly as the provider spells it.",
    id: "Endpoint tidak mengenali model ini. Periksa model ID persis seperti yang ditulis provider.",
  },
  protocol_unsupported: {
    en: "The endpoint does not implement the API format this agent requires.",
    id: "Endpoint tidak menyediakan format API yang dibutuhkan agent ini.",
  },
  endpoint_unsupported: {
    en: "The endpoint did not accept the request format.",
    id: "Endpoint tidak menerima format request ini.",
  },
  unreachable: {
    en: "The endpoint could not be reached. Check the base URL and your connection.",
    id: "Endpoint tidak bisa dijangkau. Periksa base URL dan koneksimu.",
  },
  timed_out: {
    en: "The endpoint did not respond in time.",
    id: "Endpoint tidak merespons tepat waktu.",
  },
  rate_limited: {
    en: "The endpoint is rate limiting requests. Try again shortly.",
    id: "Endpoint sedang membatasi permintaan. Coba lagi sebentar lagi.",
  },
  server_error: {
    en: "The endpoint returned a server error. This is usually temporary.",
    id: "Endpoint mengembalikan error server. Biasanya bersifat sementara.",
  },
  streaming_unsupported: {
    en: "The endpoint works, but it does not stream. Replies will appear all at once.",
    id: "Endpoint berfungsi, tapi tidak mendukung streaming. Balasan muncul sekaligus.",
  },
  tools_unsupported: {
    en: "The endpoint works, but it does not support tool calling.",
    id: "Endpoint berfungsi, tapi tidak mendukung tool calling.",
  },
  streaming_and_tools_unsupported: {
    en: "The endpoint works for plain replies, but it does not stream or support tool calling.",
    id: "Endpoint berfungsi untuk balasan biasa, tapi tidak mendukung streaming maupun tool calling.",
  },
};

/** Look up one of the maps, falling back to `undefined` when unknown. */
function lookup(
  map: Record<string, BackendText>,
  key: string | undefined | null,
  language: Language,
): string | undefined {
  if (!key) return undefined;
  const entry = map[key];
  if (!entry) return undefined;
  return language === "id" ? entry.id : entry.en;
}

/**
 * A step's label in the chosen language.
 *
 * Falls back to whatever the backend sent, which is already English, so an
 * unrecognised step still reads as a sentence rather than as an identifier.
 */
export function stepLabel(
  id: string | undefined,
  backendLabel: string,
  language: Language,
): string {
  return lookup(STEP_LABELS, id, language) ?? backendLabel;
}

/**
 * A verdict's explanation in the chosen language.
 *
 * Falls back to the backend's own `reason`, which carries the specific detail for
 * this run; a generic translated sentence would be worse than the precise English
 * one when the code is one this build does not know.
 */
export function reasonText(
  code: string | undefined,
  backendReason: string,
  language: Language,
): string {
  return lookup(REASON_TEXTS, code, language) ?? backendReason;
}

/** The identifiers this build knows, exported so a test can check them. */
export const KNOWN_STEP_IDS = Object.keys(STEP_LABELS);
export const KNOWN_REASON_CODES = Object.keys(REASON_TEXTS);

/**
 * App-owned error copy, keyed by the stable `code` the backend sends.
 *
 * This exists because an error message from Rust is English, and rendering it
 * directly inside a translated interface produced a screen that was half Indonesian
 * and half English: "Request gagal. The provider's reply was not in the format this
 * endpoint should use."
 *
 * Only errors whose text is Eclink's *own* explanation belong here. A
 * provider's or server's raw words are never in this table, because translating
 * them would mean inventing meaning: they are shown as-is under the technical
 * details, which the interface labels as such.
 */
const ERROR_TEXTS: Record<string, BackendText> = {
  PLAYGROUND_RESPONSE_UNREADABLE: {
    en: "The provider returned a response format that could not be read.",
    id: "Provider mengembalikan format respons yang tidak dapat dibaca.",
  },
  PLAYGROUND_KEY_REJECTED: {
    en: "The provider rejected your API key.",
    id: "Provider menolak API key kamu.",
  },
  PLAYGROUND_ENDPOINT_OR_MODEL_UNKNOWN: {
    en: "The provider does not recognize that endpoint or model.",
    id: "Provider tidak mengenali endpoint atau model tersebut.",
  },
  PLAYGROUND_RATE_LIMITED: {
    en: "The provider is rate limiting this key.",
    id: "Provider sedang membatasi permintaan untuk key ini.",
  },
  PLAYGROUND_SERVER_ERROR: {
    en: "The provider reported a server error.",
    id: "Provider melaporkan error di sisi server.",
  },
  PLAYGROUND_REQUEST_REJECTED: {
    en: "The provider rejected the request.",
    id: "Provider menolak request ini.",
  },
};

/**
 * Translate an app-owned error, keeping the backend's wording as the fallback.
 *
 * `message` is what Rust sent. It is used unchanged when the code is unknown, which
 * keeps a newer backend readable against an older frontend rather than showing a
 * blank or a raw identifier.
 */
export function errorText(
  code: string | undefined,
  backendMessage: string,
  language: Language,
): string {
  if (!code) return backendMessage;
  return lookup(ERROR_TEXTS, code, language) ?? backendMessage;
}

/**
 * Whether this error's text is the application's own, and therefore translatable.
 *
 * Lets a caller decide between showing translated copy and showing the provider's
 * own words, instead of guessing from the message.
 */
export function isAppOwnedError(code: string | undefined): boolean {
  return Boolean(code && ERROR_TEXTS[code]);
}

/** The error codes this build can translate, exported for a test to check. */
export const KNOWN_ERROR_CODES = Object.keys(ERROR_TEXTS);
