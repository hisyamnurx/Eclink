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
import { LANGUAGES, LANGUAGE_NAMES, isLanguage, type MessageKey } from "../lib/i18n";
import { copyToClipboard, openUrl } from "../lib/opener";
import { useAction, useApp } from "../state/AppContext";

export function Settings() {
  const { settings, loading, loadError, saveSettings, notify, reload, t, language, setLanguage } =
    useApp();

  if (loading) return <LoadingState label={t("settings.checking")} />;
  if (loadError) return <ErrorState error={loadError} onRetry={() => void reload()} />;
  if (!settings) return <ErrorState error={t("error.loadFailed")} />;

  const save = (patch: Parameters<typeof saveSettings>[0], message?: string) =>
    void saveSettings(patch)
      .then(() => {
        if (message) notify("success", message);
      })
      .catch((error: unknown) => {
        const api = error as { message?: string; hint?: string };
        notify("error", api.message ?? t("settings.couldNotSave"), api.hint);
      });

  return (
    <div className="main__inner">
      <header className="page-header">
        <div className="page-header__text">
          <h1>{t("settings.title")}</h1>
          <p className="page-header__subtitle">{t("settings.subtitle")}</p>
        </div>
      </header>

      <section className="card" aria-labelledby="set-general">
        <h2 id="set-general">{t("settings.generalSection")}</h2>

        <Field
          label={t("settings.language")}
          htmlFor="set-language"
          hint={t("settings.languageHint")}
        >
          <select
            id="set-language"
            className="select"
            value={language}
            onChange={(event) => {
              // The select only ever offers values from LANGUAGES, so this is a
              // narrowing of a value already known to be valid rather than a
              // check the user could fail.
              const next = event.target.value;
              if (!isLanguage(next)) return;
              // Reported rather than swallowed. A silent failure here is
              // indistinguishable from a language that refuses to change, which is
              // exactly how the previous bug presented itself.
              void setLanguage(next).catch((error: unknown) => {
                const api = error as { message?: string; hint?: string };
                notify("error", api.message ?? t("settings.couldNotSave"), api.hint);
              });
            }}
          >
            {LANGUAGES.map((code) => (
              <option key={code} value={code}>
                {LANGUAGE_NAMES[code]}
              </option>
            ))}
          </select>
        </Field>

        <label className="checkbox">
          <input
            type="checkbox"
            checked={settings.updateCheck}
            onChange={(event) =>
              save(
                { updateCheck: event.target.checked },
                event.target.checked ? t("settings.updateCheckOn") : t("settings.updateCheckOff"),
              )
            }
          />
          <span className="checkbox__text">
            <span>{t("settings.updateCheck")}</span>
            <span className="checkbox__hint">{t("settings.updateCheckBody")}</span>
          </span>
        </label>

        <label className="checkbox">
          <input
            type="checkbox"
            checked={settings.rememberProjectDirectory}
            onChange={(event) => save({ rememberProjectDirectory: event.target.checked })}
          />
          <span className="checkbox__text">
            <span>{t("settings.rememberFolder")}</span>
            <span className="checkbox__hint">{t("settings.rememberFolderBody")}</span>
          </span>
        </label>

        <Field label={t("settings.logLevel")} htmlFor="set-log" hint={t("settings.logLevelBody")}>
          <select
            id="set-log"
            className="select"
            value={settings.logLevel}
            onChange={(event) =>
              save({ logLevel: event.target.value as LogLevel }, t("settings.logLevelSaved"))
            }
          >
            <option value="error">{t("settings.logLevelError")}</option>
            <option value="warn">{t("settings.logLevelWarn")}</option>
            <option value="info">{t("settings.logLevelInfo")}</option>
            <option value="debug">{t("settings.logLevelDebug")}</option>
            <option value="trace">{t("settings.logLevelTrace")}</option>
          </select>
        </Field>
      </section>

      <section className="card" aria-labelledby="set-about">
        <h2 id="set-about">{t("settings.aboutSection")}</h2>
        <Meta
          items={[
            { key: t("home.version"), value: settings.appVersion, mono: true },
            { key: t("settings.protectedBy"), value: settings.secretBackend },
            {
              key: t("settings.storedApiKeys"),
              value: settings.secretCount === 0 ? t("settings.none") : `${settings.secretCount}`,
            },
            { key: t("settings.providerProfiles"), value: `${settings.profileCount}` },
            {
              key: t("settings.credentialHelper"),
              value: settings.credentialHelperAvailable
                ? t("settings.credentialHelperAvailable")
                : t("settings.credentialHelperNotFound"),
            },
            { key: t("settings.dataFolder"), value: settings.dataDirectory, mono: true },
          ]}
        />

        <div className="row row--wrap">
          <Button
            onClick={() =>
              void call("reveal_data_directory")
                .then(() => undefined)
                .catch(() => notify("error", t("settings.couldNotOpenDataFolder")))
            }
          >
            {t("settings.openDataFolder")}
          </Button>
          <Button
            onClick={() =>
              void openUrl(settings.repositoryUrl).catch(() =>
                notify("error", t("settings.couldNotOpenBrowser")),
              )
            }
          >
            {t("settings.openProjectPage")}
          </Button>
        </div>

        <p className="muted small">{t("settings.independence")}</p>
      </section>

      <UpdatePanel />
      <DiagnosticsPanel />
      <LogsPanel />
    </div>
  );
}

function UpdatePanel() {
  const { settings, t } = useApp();
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
          <h2 id="set-updates">{t("settings.updates")}</h2>
          <div className="card__hint">
            {settings?.lastUpdateCheck
              ? t("settings.lastChecked", { when: relativeTime(settings.lastUpdateCheck) })
              : t("settings.notCheckedYet")}
          </div>
        </div>
        <div className="card__actions">
          <Button onClick={check} busy={action.busy}>
            {t("settings.checkNow")}
          </Button>
        </div>
      </div>

      {status?.error && (
        <Banner tone="warn" title={t("settings.updateCheckIncomplete")}>
          {status.error}
        </Banner>
      )}

      {status?.checked && !status.available && !status.error && (
        <Banner tone="info" title={t("settings.upToDateTitle")}>
          {t("settings.latestRelease", { version: status.currentVersion })}
        </Banner>
      )}

      {status?.available && (
        <div className="stack stack--tight">
          <Banner
            tone="info"
            title={t("settings.updateAvailableTitle", { version: status.version ?? "" })}
          >
            {t("settings.updateBody", { current: status.currentVersion })}
          </Banner>
          {status.notes && (
            <>
              <div className="field__label">{t("settings.releaseNotes")}</div>
              <pre className="code selectable">{status.notes}</pre>
            </>
          )}
          <div className="row">
            <Button variant="primary" onClick={install} busy={action.busy}>
              {t("settings.updateInstall")}
            </Button>
            <span className="small muted">{t("settings.restartOnInstall")}</span>
          </div>
        </div>
      )}
    </section>
  );
}

function DiagnosticsPanel() {
  const { notify, t } = useApp();
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
          <h2 id="set-diagnostics">{t("settings.diagnostics")}</h2>
          <div className="card__hint">{t("settings.diagnosticsBody")}</div>
        </div>
        <div className="card__actions">
          <Button
            variant="secondary"
            onClick={() =>
              void copyToClipboard(report?.text ?? "")
                .then(() => notify("success", t("toast.diagnosticsCopied")))
                .catch(() => notify("error", t("error.copyDiagnosticsFailed")))
            }
            disabled={!report}
          >
            {t("action.copyDiagnostics")}
          </Button>
          <Button variant="ghost" onClick={reload}>
            {t("action.refresh")}
          </Button>
        </div>
      </div>

      <Banner tone="info" title={t("settings.reportNoKeysTitle")}>
        {t("settings.reportNoKeysBody")}
      </Banner>

      {error ? (
        <ErrorState error={error} onRetry={reload} />
      ) : report ? (
        <pre className="code code--tall selectable">{report.text}</pre>
      ) : (
        <LoadingState label={loading ? t("settings.buildingReport") : t("settings.noReport")} />
      )}
    </section>
  );
}

function LogsPanel() {
  const { notify, t } = useApp();
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
          <h2 id="set-logs">{t("settings.logs")}</h2>
          <div className="card__hint">{t("settings.logsLocalHint")}</div>
        </div>
        <div className="card__actions">
          <Button variant="ghost" onClick={reload}>
            {t("action.refresh")}
          </Button>
          <Button
            onClick={() =>
              void call("reveal_log_directory").catch(() =>
                notify("error", t("settings.couldNotOpenLogFolder")),
              )
            }
          >
            {t("action.openFolder")}
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
                  notify("success", t("toast.logsCleared"));
                  reload();
                })
            }
          >
            {t("action.clearLogs")}
          </Button>
        </div>
      </div>

      {error ? (
        <ErrorState error={error} onRetry={reload} />
      ) : loading && !lines ? (
        <LoadingState label={t("settings.readingLogs")} />
      ) : !lines || lines.length === 0 ? (
        <EmptyState title={t("settings.noLogEntries")}>{t("settings.noLogEntriesBody")}</EmptyState>
      ) : (
        <>
          <div className="row">
            <Badge tone="neutral">{t("settings.lineCount", { count: lines.length })}</Badge>
            <span className="small dim">{t("settings.newestAtBottom")}</span>
          </div>
          <pre className="code code--tall selectable">{lines.join("\n")}</pre>
        </>
      )}
    </section>
  );
}

/**
 * Exported so the About screen can show the project's independence notice.
 *
 * Kept as a key rather than a string: the notice is user-visible copy, and a
 * second English copy of it would drift from the dictionary the moment either
 * one is edited. Callers render it with `t(INDEPENDENCE_NOTICE)`.
 */
export const INDEPENDENCE_NOTICE = "settings.independence" satisfies MessageKey;
