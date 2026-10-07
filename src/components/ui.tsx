/**
 * Shared UI primitives.
 *
 * These exist so accessibility behaviour (focus management, ARIA wiring, live
 * regions, status text) is implemented once rather than re-derived per screen.
 */

import { useEffect, useId, useRef, type ReactNode, type ButtonHTMLAttributes } from "react";

import { ApiError } from "../lib/api";
import { useApp } from "../state/AppContext";

/* --- Buttons ---------------------------------------------------------- */

type ButtonVariant = "primary" | "secondary" | "ghost" | "danger";

interface ButtonProps extends ButtonHTMLAttributes<HTMLButtonElement> {
  variant?: ButtonVariant;
  size?: "md" | "sm";
  busy?: boolean;
  block?: boolean;
}

export function Button({
  variant = "secondary",
  size = "md",
  busy = false,
  block = false,
  children,
  disabled,
  className = "",
  ...rest
}: ButtonProps) {
  const classes = [
    "btn",
    `btn--${variant}`,
    size === "sm" ? "btn--sm" : "",
    block ? "btn--block" : "",
    className,
  ]
    .filter(Boolean)
    .join(" ");

  return (
    <button
      {...rest}
      className={classes}
      // A busy button stays in the tab order but cannot be activated twice, so a
      // double click cannot start two conflicting writes.
      disabled={disabled || busy}
      aria-busy={busy || undefined}
    >
      {busy && <span className="spinner spinner--sm" aria-hidden="true" />}
      {children}
    </button>
  );
}

/* --- Form fields ------------------------------------------------------ */

interface FieldProps {
  label: string;
  htmlFor?: string;
  hint?: ReactNode;
  error?: string;
  required?: boolean;
  children: ReactNode;
}

export function Field({ label, htmlFor, hint, error, required = false, children }: FieldProps) {
  return (
    <div className="field">
      <label className="field__label" htmlFor={htmlFor}>
        {label}
        {required && (
          <span className="field__required" aria-hidden="true">
            *
          </span>
        )}
      </label>
      {children}
      {/* The hint is associated by the caller through aria-describedby; it is
          rendered here so the structure stays consistent. */}
      {hint && !error && <div className="field__hint">{hint}</div>}
      {error && (
        <div className="field__error" role="alert">
          {error}
        </div>
      )}
    </div>
  );
}

/* --- Status badge ----------------------------------------------------- */

export type StatusTone = "neutral" | "ok" | "warn" | "bad" | "info";

interface BadgeProps {
  tone?: StatusTone;
  children: ReactNode;
  /** Shown as a dot; the text label always carries the meaning too. */
  dot?: boolean;
}

export function Badge({ tone = "neutral", children, dot = false }: BadgeProps) {
  return (
    <span className={`badge${tone === "neutral" ? "" : ` badge--${tone}`}`}>
      {dot && <span className="badge__dot" aria-hidden="true" />}
      {children}
    </span>
  );
}

/* --- Banner ----------------------------------------------------------- */

interface BannerProps {
  tone?: "info" | "warn" | "bad";
  title?: string;
  children: ReactNode;
  actions?: ReactNode;
}

export function Banner({ tone = "info", title, children, actions }: BannerProps) {
  const icon = tone === "warn" ? "!" : tone === "bad" ? "×" : "i";
  return (
    <div className={`banner banner--${tone}`} role={tone === "info" ? undefined : "alert"}>
      <span className="banner__icon" aria-hidden="true">
        {icon}
      </span>
      <div className="banner__body">
        {title && <div className="banner__title">{title}</div>}
        <div className="banner__text">{children}</div>
      </div>
      {actions && <div className="card__actions">{actions}</div>}
    </div>
  );
}

/* --- States ----------------------------------------------------------- */

export function LoadingState({ label = "Loading…" }: { label?: string }) {
  return (
    <div className="state" role="status">
      <span className="spinner" aria-hidden="true" />
      <div className="state__text">{label}</div>
    </div>
  );
}

export function EmptyState({
  title,
  children,
  action,
}: {
  title: string;
  children?: ReactNode;
  action?: ReactNode;
}) {
  return (
    <div className="state">
      <div className="state__title">{title}</div>
      {children && <div className="state__text">{children}</div>}
      {action}
    </div>
  );
}

export function ErrorState({ error, onRetry }: { error: unknown; onRetry?: () => void }) {
  const { t } = useApp();
  const api = error instanceof ApiError ? error : null;
  return (
    <div className="state" role="alert">
      <div className="state__title">{t("error.generic")}</div>
      <div className="state__text">{api ? api.message : String(error)}</div>
      {api?.hint && <div className="state__text dim">{api.hint}</div>}
      {onRetry && (
        <Button variant="secondary" size="sm" onClick={onRetry}>
          {t("action.retry")}
        </Button>
      )}
    </div>
  );
}

/* --- Meta list -------------------------------------------------------- */

export function Meta({
  items,
}: {
  items: Array<{ key: string; value: ReactNode; mono?: boolean }>;
}) {
  return (
    <div className="meta">
      {items.map((item) => (
        <div key={item.key} style={{ display: "contents" }}>
          <div className="meta__key">{item.key}</div>
          <div className={`meta__value${item.mono ? " meta__value--mono" : ""}`}>{item.value}</div>
        </div>
      ))}
    </div>
  );
}

/* --- Modal ------------------------------------------------------------ */

interface ModalProps {
  title: string;
  onClose: () => void;
  children: ReactNode;
  footer?: ReactNode;
  wide?: boolean;
}

/**
 * A modal dialog with real focus management.
 *
 * Focus moves into the dialog on open, Escape closes it, Tab is trapped, and
 * focus returns to whatever was focused before. Without this, a keyboard user
 * ends up on a control they cannot see.
 */
export function Modal({ title, onClose, children, footer, wide = false }: ModalProps) {
  const dialogRef = useRef<HTMLDivElement>(null);
  const titleId = useId();

  useEffect(() => {
    const previouslyFocused = document.activeElement as HTMLElement | null;

    // Focus the first focusable element, or the dialog itself.
    const focusables = () => {
      const root = dialogRef.current;
      if (!root) return [] as HTMLElement[];
      return Array.from(
        root.querySelectorAll<HTMLElement>(
          'button:not([disabled]), [href], input:not([disabled]), select:not([disabled]), textarea:not([disabled]), [tabindex]:not([tabindex="-1"])',
        ),
      );
    };

    const first = focusables()[0];
    if (first) {
      first.focus();
    } else {
      dialogRef.current?.focus();
    }

    const onKeyDown = (event: KeyboardEvent) => {
      if (event.key === "Escape") {
        event.preventDefault();
        onClose();
        return;
      }
      if (event.key !== "Tab") return;

      const items = focusables();
      if (items.length === 0) return;

      const firstItem = items[0]!;
      const lastItem = items[items.length - 1]!;
      const active = document.activeElement;

      if (event.shiftKey && active === firstItem) {
        event.preventDefault();
        lastItem.focus();
      } else if (!event.shiftKey && active === lastItem) {
        event.preventDefault();
        firstItem.focus();
      }
    };

    document.addEventListener("keydown", onKeyDown, true);
    return () => {
      document.removeEventListener("keydown", onKeyDown, true);
      previouslyFocused?.focus?.();
    };
  }, [onClose]);

  return (
    <div
      className="modal-backdrop"
      // Clicking the backdrop closes; clicking inside must not.
      onMouseDown={(event) => {
        if (event.target === event.currentTarget) onClose();
      }}
    >
      <div
        className={`modal${wide ? " modal--wide" : ""}`}
        role="dialog"
        aria-modal="true"
        aria-labelledby={titleId}
        ref={dialogRef}
        tabIndex={-1}
      >
        <div className="modal__header">
          <h2 id={titleId}>{title}</h2>
          <Button variant="ghost" size="sm" onClick={onClose} aria-label="Close dialog">
            Close
          </Button>
        </div>
        <div className="modal__body">{children}</div>
        {footer && <div className="modal__footer">{footer}</div>}
      </div>
    </div>
  );
}

/* --- Tabs ------------------------------------------------------------- */

export function Tabs<T extends string>({
  tabs,
  value,
  onChange,
  label,
}: {
  tabs: Array<{ id: T; label: string }>;
  value: T;
  onChange: (id: T) => void;
  label: string;
}) {
  return (
    <div className="tabs" role="tablist" aria-label={label}>
      {tabs.map((tab) => (
        <button
          key={tab.id}
          role="tab"
          type="button"
          className="tab"
          aria-selected={value === tab.id}
          tabIndex={value === tab.id ? 0 : -1}
          onClick={() => onChange(tab.id)}
        >
          {tab.label}
        </button>
      ))}
    </div>
  );
}
