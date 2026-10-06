import { useApp } from "../state/AppContext";

/**
 * Toast region.
 *
 * A single polite live region rather than per-toast alerts: error toasts are
 * assertive because the user needs to know an action failed, success messages
 * are polite so they do not interrupt.
 */
export function Toasts() {
  const { toasts, dismissToast } = useApp();

  return (
    <div className="toast-region">
      {toasts.map((toast) => (
        <div
          key={toast.id}
          className={`toast toast--${toast.tone}`}
          role={toast.tone === "error" ? "alert" : "status"}
          aria-live={toast.tone === "error" ? "assertive" : "polite"}
        >
          <div className="toast__body">
            <div className="toast__title">{toast.title}</div>
            {toast.text && <div className="toast__text">{toast.text}</div>}
          </div>
          <button
            type="button"
            className="btn btn--ghost btn--sm"
            onClick={() => dismissToast(toast.id)}
            aria-label={`Dismiss: ${toast.title}`}
          >
            ×
          </button>
        </div>
      ))}
    </div>
  );
}
