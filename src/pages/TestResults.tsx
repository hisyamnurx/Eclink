/**
 * Test Connection results.
 *
 * A test is a sequence of real requests, so the result is a sequence of steps
 * rather than a single pass/fail. Each step shows what was checked, what
 * happened, and how long it took, and every state is labelled in text so the
 * meaning never depends on colour alone.
 *
 * The step state words are the frontend's own mapping of `TestStepState`, so they
 * are localised directly. The step label and the verdict reason also arrive from
 * the backend as English, and are resolved by their stable identifiers through
 * `lib/backend-text.ts` so they follow the chosen language; an identifier this
 * build does not recognise falls back to the backend's own wording.
 *
 * Raw `step.detail` is still shown exactly as sent. It is diagnostic text that
 * names the endpoint's own response, and paraphrasing it would remove the detail a
 * bug report needs.
 */

import { Badge, Button, type StatusTone } from "../components/ui";
import { copyToClipboard } from "../lib/opener";
import { latency } from "../lib/format";
import { reasonText, stepLabel } from "../lib/backend-text";
import { compatibilityLabel } from "../lib/compatibility";
import type { Language, MessageKey } from "../lib/i18n";
import { useApp } from "../state/AppContext";
import type { TestOutcome, TestStep, TestStepState } from "../lib/api";

const STEP_ICON: Record<TestStepState, string> = {
  passed: "✓",
  warning: "!",
  failed: "×",
  // A short hyphen, to match the rest of the interface; this was an en-dash.
  skipped: "-",
};

/** A step state as a message key, resolved through `t()` where it is shown. */
const STEP_WORD: Record<TestStepState, MessageKey> = {
  passed: "test.passed",
  warning: "test.warning",
  failed: "test.failed",
  skipped: "test.skipped",
};

/**
 * A compatibility status as a message key.
 *
 * Delegates to `compatibilityLabel` rather than keeping a second copy of the
 * mapping. This file previously had its own, which is the same shape of
 * duplication that once let the badge and the Activate button disagree about
 * whether a provider had been tested; one mapping means they cannot.
 */
const compatibilityWord = (status: string | undefined): MessageKey => compatibilityLabel(status);

function toneFor(status: string): StatusTone {
  switch (status) {
    case "compatible":
      return "ok";
    case "partially_compatible":
      return "warn";
    case "incompatible":
    case "test_failed":
      return "bad";
    case "unknown":
      return "neutral";
    default:
      return "neutral";
  }
}

type Translate = (key: MessageKey, values?: Record<string, string | number>) => string;

export function TestResults({
  outcome,
  onDismiss,
}: {
  outcome: TestOutcome;
  onDismiss?: () => void;
}) {
  const { notify, t, language } = useApp();

  const passed = outcome.steps.filter((step) => step.state === "passed").length;
  const total = outcome.steps.length;

  return (
    <div className="stack stack--tight" role="group" aria-label={t("test.title")}>
      <div className="row row--wrap">
        <Badge tone={toneFor(outcome.status)} dot>
          {t(compatibilityWord(outcome.status))}
        </Badge>
        {total > 0 && <span className="small dim">{t("test.summary", { passed, total })}</span>}
        {outcome.latencyMs !== undefined && (
          <span className="small dim mono">{latency(outcome.latencyMs)}</span>
        )}
        <div className="spacer" />
        {onDismiss && (
          <Button size="sm" variant="ghost" onClick={onDismiss}>
            {t("action.hide")}
          </Button>
        )}
      </div>

      {/* The reason is always shown as text, even when it duplicates a step.
          Resolved by the stable `reasonCode` rather than the English sentence, so
          the wording follows the chosen language and does not break when the
          backend rephrases. An unknown code falls back to the backend's own text,
          which carries the detail specific to this run. */}
      <div className="small">
        <span className="muted">{reasonText(outcome.reasonCode, outcome.reason, language)}</span>
      </div>

      {outcome.warning && (
        <div className="small" style={{ color: "var(--status-warn)" }}>
          {outcome.warning}
        </div>
      )}

      {outcome.steps.length > 0 && (
        <div className="steps">
          {outcome.steps.map((step) => (
            <StepRow key={step.id} step={step} t={t} language={language} />
          ))}
        </div>
      )}

      <div className="row">
        <Button
          size="sm"
          variant="ghost"
          onClick={() =>
            void copyToClipboard(formatOutcome(outcome, t))
              .then(() => notify("success", t("toast.testResultsCopied")))
              .catch(() => notify("error", t("error.testResultsCopyFailed")))
          }
        >
          {t("test.copyResults")}
        </Button>
      </div>
    </div>
  );
}

function StepRow({ step, t, language }: { step: TestStep; t: Translate; language: Language }) {
  return (
    <div className="step">
      <span className={`step__state step__state--${step.state}`} aria-hidden="true">
        {STEP_ICON[step.state]}
      </span>
      <div>
        <div className="step__label">
          {/* Resolved by the step's stable id, so the label follows the language.
              The backend's own English is kept as the fallback for a step this
              build does not know about. */}
          {stepLabel(step.id, step.label, language)}
          {/* The word is available to screen readers and to anyone who cannot
              distinguish the colours. */}
          <span className="visually-hidden">: {t(STEP_WORD[step.state])}</span>
          <span className="dim small"> - {t(STEP_WORD[step.state])}</span>
        </div>
        {step.detail && <div className="step__detail">{step.detail}</div>}
      </div>
      <span className="step__latency">{latency(step.latencyMs)}</span>
    </div>
  );
}

/**
 * A plain-text rendering for the clipboard or a bug report.
 *
 * Labels follow the chosen language, like the on-screen result, so what a user
 * pastes matches what they read. Raw `step.detail` is kept exactly as sent,
 * because it names the endpoint's own response and is the part a maintainer needs
 * verbatim. `language` is passed in rather than read from context so this stays a
 * pure function that a test can call.
 */
export function formatOutcome(
  outcome: TestOutcome,
  t: Translate,
  language: Language = "en",
): string {
  const lines: string[] = [];
  lines.push(`${t("test.title")} - ${outcome.agent}`);
  lines.push(`${t("history.result")}: ${t(compatibilityWord(outcome.status))}`);
  lines.push(`${t("test.reason")}: ${reasonText(outcome.reasonCode, outcome.reason, language)}`);
  if (outcome.warning) lines.push(`${t("test.warning")}: ${outcome.warning}`);
  if (outcome.latencyMs !== undefined) {
    lines.push(`${t("test.totalTime")}: ${latency(outcome.latencyMs)}`);
  }
  lines.push("");
  for (const step of outcome.steps) {
    const label = stepLabel(step.id, step.label, language);
    lines.push(`${STEP_ICON[step.state]} ${label}: ${t(STEP_WORD[step.state])}`);
    if (step.detail) lines.push(`    ${step.detail}`);
  }
  return lines.join("\n");
}
