/**
 * Settings, diagnostics, and updates.
 *
 * Diagnostics exist so a user can solve their own problem: everything shown here
 * is local, copyable, and free of credentials, and the report says so explicitly
 * so nobody hesitates to paste it into an issue.
 */

import { useState } from "react";

import {
  Badge,
  Banner,
  Button,
  EmptyState,
  ErrorState,
  Field,
  LoadingState,
  Meta,
} from "../components/ui";
import { call, type LogLevel, type UpdateStatus } from "../lib/api";
import { relativeTime } from "../lib/format";
import { useAsyncData } from "../lib/hooks";
import { copyToClipboard, openUrl } from "../lib/opener";
import { useAction, useApp } from "../state/AppContext";

export function Settings() {
  const { settings, loading, loadError, saveSettings, notify, reload } = useApp();

  if (loading) return <LoadingState label="Loading settings…" />;
  if (loadError) return <ErrorState error={loadError} onRetry={() => void reload()} />;
  if (!settings) return <ErrorState error="Settings could not be read." />;

  const save = (patch: Parameters<typeof saveSettings>[0], message?: string) =>
    void saveSettings(patch)
      .then(() => {
        if (message) notify("success", message);
      })
      .catch((error: unknown) => {
        const api = error as { message?: string; hint?: string };
        notify("error", api.message ?? "Could not save that setting.", api.hint);
      });

  return (
    <div className="main__inner">
      <header className="page-header">
        <div className="page-header__text">
          <h1>Settings</h1>
          <p className="page-header__subtitle">
            Eclyzer Bridge collects nothing about you. There is no account, no telemetry, and no
            crash reporting.
          </p>
        </div>
      </header>

      <section className="card" aria-labelledby="set-general">
        <h2 id="set-general">General</h2>

        <label className="checkbox">
          <input
            type="checkbox"
            checked={settings.updateCheck}
            onChange={(event) =>
              save(
                { updateCheck: event.target.checked },
                event.target.checked
                  ? "Eclyzer Bridge will check GitHub for updates."
                  : "Update checks are off.",
              )
            }
          />
          <span className="checkbox__text">
            <span>Check for updates automatically</span>
            <span className="checkbox__hint">
              At most once every 24 hours, from GitHub Releases. No information about you or your
              machine is sent; this is an ordinary download request. Updates are never installed
              without your confirmation.
            </span>
          </span>
        </label>

        <label className="checkbox">
          <input
            type="checkbox"
            checked={settings.rememberProjectDirectory}
            onChange={(event) => save({ rememberProjectDirectory: event.target.checked })}
          />
          <span className="checkbox__text">
            <span>Remember the last project folder</span>
            <span className="checkbox__hint">
              Used to suggest a folder when you open a coding agent. Turning this off forgets the
              saved path immediately.
            </span>
          </span>
        </label>

        <Field
          label="Log level"
          htmlFor="set-log"
          hint="Logs stay on this machine and are pruned after 7 days. Higher levels are useful when reporting a bug."
        >
          <select
            id="set-log"
            className="select"
            value={settings.logLevel}
            onChange={(event) =>
              save({ logLevel: event.target.value as LogLevel }, "Log level saved.")
            }
          >
            <option value="error">Error — problems only</option>
            <option value="warn">Warn</option>
            <option value="info">Info — recommended</option>
            <option value="debug">Debug</option>
            <option value="trace">Trace — very verbose</option>
          </select>
        </Field>
      </section>

      <section className="card" aria-labelledby="set-about">
        <h2 id="set-about">About</h2>
        <Meta
          items={[
            { key: "Version", value: settings.appVersion, mono: true },
            { key: "Protected by", value: settings.secretBackend },
            {
              key: "Stored API keys",
              value: settings.secretCount === 0 ? "none" : `${settings.secretCount}`,
            },
            { key: "Provider profiles", value: `${settings.profileCount}` },
            {
              key: "Credential helper",
              value: settings.credentialHelperAvailable
                ? "Available — Codex reads keys without them being written to config"
                : "Not found — Codex uses an environment variable instead",
            },
            { key: "Data folder", value: settings.dataDirectory, mono: true },
          ]}
        />

        <div className="row row--wrap">
          <Button
            onClick={() =>
              void call("reveal_data_directory")
                .then(() => undefined)
                .catch(() => notify("error", "Could not open the data folder."))
            }
          >
            Open data folder
          </Button>
          <Button
            onClick={() =>
              void openUrl(settings.repositoryUrl).catch(() =>
                notify("error", "Could not open your browser."),
              )
            }
          >
            Project page
          </Button>
        </div>

        <p className="muted small">
          Eclyzer Bridge is an independent open-source project and is not affiliated with, endorsed
          by, or sponsored by OpenAI or Anthropic.
        </p>
      </section>

      <UpdatePanel />
      <DiagnosticsPanel />
      <LogsPanel />
    </div>
  );
}

function UpdatePanel() {
  const { settings } = useApp();
  const [status, setStatus] = useState<UpdateStatus | null>(null);
  const action = useAction();

  const check = () =>
    void action
      .run(() => call("check_for_updates"))
      .then((result) => {
        if (result) setStatus(result);
      });

  const install = () => void action.run(() => call("install_update"));

  return (
    <section className="card" aria-labelledby="set-updates">
      <div className="card__header">
        <div className="card__title">
          <h2 id="set-updates">Updates</h2>
          <div className="card__hint">
            {settings?.lastUpdateCheck
              ? `Last checked ${relativeTime(settings.lastUpdateCheck)}`
              : "Not checked yet"}
          </div>
        </div>
        <div className="card__actions">
          <Button onClick={check} busy={action.busy}>
            Check now
          </Button>
        </div>
      </div>

      {status?.error && (
        <Banner tone="warn" title="The update check did not complete">
          {status.error}
        </Banner>
      )}

      {status?.checked && !status.available && !status.error && (
        <Banner tone="info" title="You are up to date">
          Eclyzer Bridge {status.currentVersion} is the latest release.
        </Banner>
      )}

      {status?.available && (
        <div className="stack stack--tight">
          <Banner tone="info" title={`Version ${status.version} is available`}>
            You are running {status.currentVersion}. Updates are downloaded from GitHub Releases and
            verified against a signature before anything is replaced.
          </Banner>
          {status.notes && (
            <>
              <div className="field__label">Release notes</div>
              <pre className="code selectable">{status.notes}</pre>
            </>
          )}
          <div className="row">
            <Button variant="primary" onClick={install} busy={action.busy}>
              Download and install
            </Button>
            <span className="small muted">Eclyzer Bridge restarts when the install finishes.</span>
          </div>
        </div>
      )}
    </section>
  );
}

function DiagnosticsPanel() {
  const { notify } = useApp();
  const {
    data: report,
    error,
    loading,
    reload,
  } = useAsyncData(() => call("build_diagnostics_report"), []);

  return (
    <section className="card" aria-labelledby="set-diagnostics">
      <div className="card__header">
        <div className="card__title">
          <h2 id="set-diagnostics">Diagnostics</h2>
          <div className="card__hint">A summary of your setup you can paste into a bug report.</div>
        </div>
        <div className="card__actions">
          <Button
            variant="secondary"
            onClick={() =>
              void copyToClipboard(report?.text ?? "")
                .then(() => notify("success", "Diagnostics copied."))
                .catch(() => notify("error", "Could not copy the diagnostics."))
            }
            disabled={!report}
          >
            Copy diagnostics
          </Button>
          <Button variant="ghost" onClick={reload}>
            Refresh
          </Button>
        </div>
      </div>

      <Banner tone="info" title="This report never contains your API keys">
        It lists versions, paths, and configuration states only. Credentials are removed even if one
        somehow reached the text.
      </Banner>

      {error ? (
        <ErrorState error={error} onRetry={reload} />
      ) : report ? (
        <pre className="code code--tall selectable">{report.text}</pre>
      ) : (
        <LoadingState label={loading ? "Building the report…" : "No report yet."} />
      )}
    </section>
  );
}

function LogsPanel() {
  const { notify } = useApp();
  const {
    data: lines,
    error,
    loading,
    reload,
  } = useAsyncData(() => call("read_logs", { maxLines: 400 }), []);
  const clearAction = useAction();

  return (
    <section className="card" aria-labelledby="set-logs">
      <div className="card__header">
        <div className="card__title">
          <h2 id="set-logs">Logs</h2>
          <div className="card__hint">Stored locally, redacted, and pruned after 7 days.</div>
        </div>
        <div className="card__actions">
          <Button variant="ghost" onClick={reload}>
            Refresh
          </Button>
          <Button
            onClick={() =>
              void call("reveal_log_directory").catch(() =>
                notify("error", "Could not open the log folder."),
              )
            }
          >
            Open folder
          </Button>
          <Button
            variant="danger"
            busy={clearAction.busy}
            onClick={() =>
              void clearAction
                .run(async () => {
                  await call("clear_logs");
                })
                .then(() => {
                  notify("success", "Logs cleared.");
                  reload();
                })
            }
          >
            Clear logs
          </Button>
        </div>
      </div>

      {error ? (
        <ErrorState error={error} onRetry={reload} />
      ) : loading && !lines ? (
        <LoadingState label="Reading logs…" />
      ) : !lines || lines.length === 0 ? (
        <EmptyState title="No log entries yet">
          Eclyzer Bridge writes here when something happens. An empty log on a fresh install is
          normal.
        </EmptyState>
      ) : (
        <>
          <div className="row">
            <Badge tone="neutral">{lines.length} lines</Badge>
            <span className="small dim">Newest entries are at the bottom.</span>
          </div>
          <pre className="code code--tall selectable">{lines.join("\n")}</pre>
        </>
      )}
    </section>
  );
}

/** Exported so the About screen can show the project's independence notice. */
export const INDEPENDENCE_NOTICE =
  "Eclyzer Bridge is an independent open-source project and is not affiliated with, endorsed by, or sponsored by OpenAI or Anthropic.";
