/**
 * Test Connection results.
 *
 * A test is a sequence of real requests, so the result is a sequence of steps
 * rather than a single pass/fail. Each step shows what was checked, what
 * happened, and how long it took, and every state is labelled in text so the
 * meaning never depends on colour alone.
 */

import { Badge, Button, type StatusTone } from "../components/ui";
import { copyToClipboard } from "../lib/opener";
import { compatibilityLabel, latency } from "../lib/format";
import { useApp } from "../state/AppContext";
import type { TestOutcome, TestStep, TestStepState } from "../lib/api";

const STEP_ICON: Record<TestStepState, string> = {
  passed: "✓",
  warning: "!",
  failed: "×",
  skipped: "–",
};

const STEP_WORD: Record<TestStepState, string> = {
  passed: "Passed",
  warning: "Warning",
  failed: "Failed",
  skipped: "Skipped",
};

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

export function TestResults({
  outcome,
  onDismiss,
}: {
  outcome: TestOutcome;
  onDismiss?: () => void;
}) {
  const { notify } = useApp();

  const passed = outcome.steps.filter((step) => step.state === "passed").length;
  const total = outcome.steps.length;

  return (
    <div className="stack stack--tight" role="group" aria-label="Test results">
      <div className="row row--wrap">
        <Badge tone={toneFor(outcome.status)} dot>
          {compatibilityLabel(outcome.status)}
        </Badge>
        {total > 0 && (
          <span className="small dim">
            {passed} of {total} checks passed
          </span>
        )}
        {outcome.latencyMs !== undefined && (
          <span className="small dim mono">{latency(outcome.latencyMs)}</span>
        )}
        <div className="spacer" />
        {onDismiss && (
          <Button size="sm" variant="ghost" onClick={onDismiss}>
            Hide
          </Button>
        )}
      </div>

      {/* The reason is always shown as text, even when it duplicates a step. */}
      <div className="small">
        <span className="muted">{outcome.reason}</span>
      </div>

      {outcome.warning && (
        <div className="small" style={{ color: "var(--status-warn)" }}>
          {outcome.warning}
        </div>
      )}

      {outcome.steps.length > 0 && (
        <div className="steps">
          {outcome.steps.map((step) => (
            <StepRow key={step.id} step={step} />
          ))}
        </div>
      )}

      <div className="row">
        <Button
          size="sm"
          variant="ghost"
          onClick={() =>
            void copyToClipboard(formatOutcome(outcome))
              .then(() => notify("success", "Test results copied."))
              .catch(() => notify("error", "Could not copy the test results."))
          }
        >
          Copy results
        </Button>
      </div>
    </div>
  );
}

function StepRow({ step }: { step: TestStep }) {
  return (
    <div className="step">
      <span className={`step__state step__state--${step.state}`} aria-hidden="true">
        {STEP_ICON[step.state]}
      </span>
      <div>
        <div className="step__label">
          {step.label}
          {/* The word is available to screen readers and to anyone who cannot
              distinguish the colours. */}
          <span className="visually-hidden">: {STEP_WORD[step.state]}</span>
          <span className="dim small"> — {STEP_WORD[step.state]}</span>
        </div>
        {step.detail && <div className="step__detail">{step.detail}</div>}
      </div>
      <span className="step__latency">{latency(step.latencyMs)}</span>
    </div>
  );
}

/** A plain-text rendering for the clipboard or a bug report. */
export function formatOutcome(outcome: TestOutcome): string {
  const lines: string[] = [];
  lines.push(`Test Connection — ${outcome.agent}`);
  lines.push(`Result: ${compatibilityLabel(outcome.status)}`);
  lines.push(`Reason: ${outcome.reason}`);
  if (outcome.warning) lines.push(`Warning: ${outcome.warning}`);
  if (outcome.latencyMs !== undefined) {
    lines.push(`Total time: ${latency(outcome.latencyMs)}`);
  }
  lines.push("");
  for (const step of outcome.steps) {
    lines.push(`${STEP_ICON[step.state]} ${step.label}: ${STEP_WORD[step.state]}`);
    if (step.detail) lines.push(`    ${step.detail}`);
  }
  return lines.join("\n");
}
