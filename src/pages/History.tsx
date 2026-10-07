/**
 * History and recovery (PRD §26-§30).
 *
 * Every action here is reversible, and the UI says so before the user commits:
 * restoring a snapshot snapshots the current state first, so a wrong choice can
 * itself be undone.
 */

import { useState } from "react";

import {
  Badge,
  Banner,
  Button,
  EmptyState,
  ErrorState,
  LoadingState,
  Meta,
  Modal,
  Tabs,
} from "../components/ui";
import {
  call,
  type AgentId,
  type ApplyOutcome,
  type SnapshotView,
  type StockResetPreview,
} from "../lib/api";
import { absoluteTime, bytes, relativeTime } from "../lib/format";
import { useAsyncData } from "../lib/hooks";
import type { MessageKey } from "../lib/i18n";
import { useAction, useApp } from "../state/AppContext";

export function History({ initialAgent }: { initialAgent?: AgentId }) {
  const { agents, t } = useApp();
  const installed = agents.filter((agent) => agent.installed);
  const [tab, setTab] = useState<"changes" | "snapshots" | "recovery">("changes");

  // Derived, not synchronised: if the preferred agent is not installed, fall
  // back to the first available one in the same render.
  const [preferredAgent, setPreferredAgent] = useState<AgentId>(initialAgent ?? "codex");
  const agent =
    installed.find((item) => item.agent === preferredAgent)?.agent ??
    installed[0]?.agent ??
    preferredAgent;

  if (installed.length === 0) {
    return (
      <div className="main__inner">
        <h1>{t("history.title")}</h1>
        <EmptyState title={t("error.noAgents")}>{t("history.noAgentsBody")}</EmptyState>
      </div>
    );
  }

  return (
    <div className="main__inner">
      <header className="page-header">
        <div className="page-header__text">
          <h1>{t("history.title")}</h1>
          <p className="page-header__subtitle">{t("history.subtitle")}</p>
        </div>
      </header>

      <div className="tabs" role="tablist" aria-label={t("playground.agentTabsAria")}>
        {installed.map((item) => (
          <button
            key={item.agent}
            role="tab"
            type="button"
            className="tab"
            aria-selected={agent === item.agent}
            onClick={() => setPreferredAgent(item.agent)}
          >
            {item.displayName}
          </button>
        ))}
      </div>

      <Tabs
        tabs={[
          { id: "changes", label: t("history.tabChanges") },
          { id: "snapshots", label: t("history.tabSnapshots") },
          { id: "recovery", label: t("history.tabRecovery") },
        ]}
        value={tab}
        onChange={setTab}
        label={t("history.viewLabel")}
      />

      {tab === "changes" && <ChangeLog agent={agent} />}
      {tab === "snapshots" && <SnapshotList agent={agent} />}
      {tab === "recovery" && <RecoveryPanel agent={agent} />}
    </div>
  );
}

function ChangeLog({ agent }: { agent: AgentId }) {
  const { t } = useApp();
  const {
    data: entries,
    error,
    loading,
    reload,
  } = useAsyncData(() => call("list_history", { agent, limit: 100 }), [agent]);

  if (error) return <ErrorState error={error} onRetry={reload} />;
  if (loading && !entries) return <LoadingState label={t("history.loadingChanges")} />;

  if (!entries || entries.length === 0) {
    return <EmptyState title={t("history.empty")}>{t("history.emptyBody")}</EmptyState>;
  }

  return (
    <div className="card card--flush">
      <table className="table">
        <caption className="visually-hidden">{t("history.tableCaption")}</caption>
        <thead>
          <tr>
            <th scope="col">{t("history.when")}</th>
            <th scope="col">{t("history.what")}</th>
            <th scope="col">{t("history.result")}</th>
            <th scope="col">{t("history.undoable")}</th>
          </tr>
        </thead>
        <tbody>
          {entries.map((entry) => (
            <tr key={entry.id}>
              <td className="nowrap">
                <span title={absoluteTime(entry.createdAt)}>{relativeTime(entry.createdAt)}</span>
              </td>
              <td>
                <strong>{entry.label}</strong>
                {entry.profileName && <div className="small dim">{entry.profileName}</div>}
              </td>
              <td>
                {entry.success ? (
                  <Badge tone="ok" dot>
                    {t("history.succeeded")}
                  </Badge>
                ) : (
                  <>
                    <Badge tone="bad" dot>
                      {t("history.failed")}
                    </Badge>
                    {entry.error && <div className="small dim">{entry.error}</div>}
                  </>
                )}
              </td>
              <td>
                {entry.success && entry.snapshotId ? (
                  t("history.yes")
                ) : (
                  <span className="dim">-</span>
                )}
              </td>
            </tr>
          ))}
        </tbody>
      </table>
    </div>
  );
}

function SnapshotList({ agent }: { agent: AgentId }) {
  const { t } = useApp();
  const [confirm, setConfirm] = useState<SnapshotView | null>(null);
  const {
    data: snapshots,
    error,
    loading,
    reload,
  } = useAsyncData(() => call("list_snapshots", { agent }), [agent]);

  if (error) return <ErrorState error={error} onRetry={reload} />;
  if (loading && !snapshots) return <LoadingState label={t("history.loadingSnapshots")} />;

  if (!snapshots || snapshots.length === 0) {
    return <EmptyState title={t("history.noSnapshots")}>{t("history.noSnapshotsBody")}</EmptyState>;
  }

  return (
    <>
      <Banner tone="info" title={t("history.newestKept")}>
        {t("history.newestKeptBody")}
      </Banner>

      <div className="list">
        {snapshots.map((snapshot) => (
          <div className="list-row" key={snapshot.id}>
            <div className="list-row__main">
              <div className="list-row__title">{snapshot.label}</div>
              <div className="list-row__sub">
                {absoluteTime(snapshot.createdAt)} · {relativeTime(snapshot.createdAt)} ·{" "}
                {bytes(snapshot.bytes)}
              </div>
              <div className="list-row__sub mono">{snapshot.id}</div>
            </div>
            <div className="list-row__actions">
              <Button size="sm" onClick={() => setConfirm(snapshot)}>
                {t("history.restore")}
              </Button>
            </div>
          </div>
        ))}
      </div>

      {confirm && (
        <RestoreConfirmDialog
          agent={agent}
          title={t("history.confirmRestoreSnapshot", { when: absoluteTime(confirm.createdAt) })}
          description={t("history.confirmRestoreSnapshotBody", {
            agent: agent === "codex" ? "Codex" : "Claude Code",
            label: confirm.label,
          })}
          onConfirm={() => call("restore_snapshot", { agent, snapshotId: confirm.id })}
          onClose={() => setConfirm(null)}
          onRestored={reload}
        />
      )}
    </>
  );
}

function RecoveryPanel({ agent }: { agent: AgentId }) {
  const [pending, setPending] = useState<"default" | "original" | "stock" | null>(null);
  const { agents, t } = useApp();
  const agentSummary = agents.find((item) => item.agent === agent);
  const action = useAction();
  const {
    data: status,
    error,
    loading,
    reload,
  } = useAsyncData(() => call("recovery_status", { agent }), [agent]);

  // What a stock reset would remove. Loaded eagerly rather than on demand so the
  // warning is available the moment the user reaches for the button, and so the
  // confirmation dialog never has to open on an empty list it might render as
  // "nothing will change".
  const { data: preview } = useAsyncData<StockResetPreview | null>(
    () => call("preview_stock_reset", { agent }),
    [agent],
  );

  const undo = () =>
    void action
      .run(() => call("undo_last_change", { agent }), {
        success: t("toast.undoDone"),
      })
      .then(reload);

  if (error) return <ErrorState error={error} onRetry={reload} />;
  if (loading && !status) return <LoadingState label={t("history.checkingRecovery")} />;
  if (!status) return <ErrorState error={t("history.recoveryUnreadable")} />;

  return (
    <>
      {status.externalConfiguration && (
        <Banner tone="warn" title={t("history.externalTitle")}>
          {t("history.externalBody")}
        </Banner>
      )}

      <Meta
        items={[
          {
            key: t("history.currently"),
            value: status.hasManagedConfiguration
              ? t("history.currentManaged")
              : status.externalConfiguration
                ? t("status.externalConfiguration")
                : t("home.defaultConfiguration"),
          },
          {
            key: t("history.originalBackup"),
            value: status.hasOriginal ? t("history.originalSaved") : t("history.originalMissing"),
          },
          { key: t("history.tabSnapshots"), value: String(status.snapshotCount) },
          {
            key: t("history.lastChange"),
            value: status.lastChangeLabel ?? t("history.empty"),
          },
        ]}
      />

      <div className="stack stack--tight">
        <h2>{t("history.undoTitle")}</h2>
        <p className="card__hint">
          {t("history.undoBody")}
          {status.canUndo ? t("history.undoReversible") : t("history.undoNothing")}
        </p>
        <div>
          <Button onClick={undo} busy={action.busy} disabled={!status.canUndo}>
            {t("history.undoButton")}
          </Button>
        </div>
      </div>

      <hr className="divider" />

      <div className="stack stack--tight">
        <h2>{t("history.restoreDefaultTitle")}</h2>
        <p className="card__hint">{t("history.restoreDefaultBody")}</p>
        <div>
          <Button onClick={() => setPending("default")}>{t("history.restoreDefaultButton")}</Button>
        </div>
      </div>

      <hr className="divider" />

      <div className="stack stack--tight">
        <h2>{t("history.restoreOriginalTitle")}</h2>
        <p className="card__hint">
          {status.hasOriginal
            ? t("history.restoreOriginalBody")
            : t("history.restoreOriginalUnavailable")}
        </p>
        <div>
          <Button onClick={() => setPending("original")} disabled={!status.hasOriginal}>
            {t("history.restoreOriginalButton")}
          </Button>
        </div>
      </div>

      <hr className="divider" />

      <div className="stack stack--tight">
        <h2>{t("history.resetTitle", { agent: agentSummary?.displayName ?? agent })}</h2>
        <p className="card__hint">
          {t("history.resetBody", { agent: agentSummary?.displayName ?? agent })}
        </p>
        {preview?.blockedByManaged?.length ? (
          <p className="field__error" role="alert">
            {t("history.managedWarning", { paths: preview.blockedByManaged.join(", ") })}
          </p>
        ) : null}
        <div>
          <Button variant="danger" onClick={() => setPending("stock")}>
            {t("history.resetButton")}
          </Button>
        </div>
      </div>

      {pending === "stock" ? (
        <StockResetDialog
          agent={agent}
          displayName={agentSummary?.displayName ?? agent}
          preview={preview}
          onClose={() => setPending(null)}
          onReset={reload}
        />
      ) : null}

      {pending && pending !== "stock" && (
        <RestoreConfirmDialog
          agent={agent}
          title={
            pending === "default"
              ? t("history.confirmRestoreDefault")
              : t("history.confirmRestoreOriginal")
          }
          description={
            pending === "default"
              ? t("history.confirmRestoreDefaultBody")
              : t("history.confirmRestoreOriginalBody")
          }
          onConfirm={() =>
            call(pending === "default" ? "restore_default" : "restore_original", {
              agent,
            })
          }
          onClose={() => setPending(null)}
          onRestored={reload}
        />
      )}
    </>
  );
}

/**
 * Confirm a stock reset, showing exactly which keys will go.
 *
 * The list comes from the backend, computed from the same key lists the reset
 * uses, so it cannot drift from what actually happens. A generic "this is
 * destructive" warning would be less useful than naming what disappears.
 */
function StockResetDialog({
  agent,
  displayName,
  preview,
  onClose,
  onReset,
}: {
  agent: AgentId;
  displayName: string;
  preview: StockResetPreview | null;
  onClose: () => void;
  onReset: () => void;
}) {
  const { notify, t } = useApp();
  const action = useAction();
  const [failure, setFailure] = useState<string | null>(null);

  const rows: Array<[MessageKey, Array<[string, string]>]> = preview
    ? [
        ["history.resetGroupProvider", preview.envKeys],
        ["history.resetGroupAliases", preview.aliasKeys],
        ["history.resetGroupModel", preview.rootKeys],
      ]
    : [];
  const shown = rows.filter(([, entries]) => entries.length > 0);

  const go = () =>
    void action
      .run(() => call("reset_to_stock_defaults", { agent }), {
        onError: (api) => {
          setFailure(api.hint ? `${api.message} ${api.hint}` : api.message);
          return true;
        },
      })
      .then((result) => {
        if (result) {
          notify(
            "success",
            t("toast.resetDone", { agent: displayName }),
            result.managedKeys.length > 0
              ? t(
                  result.managedKeys.length === 1 ? "toast.resetRemovedOne" : "toast.resetRemoved",
                  { count: result.managedKeys.length },
                )
              : undefined,
          );
          onReset();
          onClose();
        }
      });

  return (
    <Modal title={t("history.resetConfirmTitle", { agent: displayName })} onClose={onClose}>
      <div className="stack stack--tight">
        <p>{t("history.resetConfirmBody")}</p>

        {shown.length > 0 ? (
          <div className="stack stack--tight">
            <p className="card__hint">{t("history.resetWillRemove")}</p>
            {shown.map(([label, entries]) => (
              <div key={label}>
                <div className="field__label">{t(label)}</div>
                <ul className="list-plain mono small">
                  {entries.map(([key, value]) => (
                    <li key={key}>
                      {key}
                      {value ? ` = ${value}` : ""}
                    </li>
                  ))}
                </ul>
              </div>
            ))}
          </div>
        ) : (
          <p className="card__hint">{t("history.resetNothing")}</p>
        )}

        <p className="card__hint">{t("history.resetSnapshot")}</p>

        {failure ? (
          <p className="field__error" role="alert">
            {failure}
          </p>
        ) : null}

        <div className="row row--end">
          <Button variant="ghost" onClick={onClose} disabled={action.busy}>
            {t("action.cancel")}
          </Button>
          <Button variant="danger" onClick={go} busy={action.busy}>
            {t("history.resetAction", { agent: displayName })}
          </Button>
        </div>
      </div>
    </Modal>
  );
}

/** Confirm a restore, and report exactly what changed afterwards. */
function RestoreConfirmDialog({
  agent,
  title,
  description,
  onConfirm,
  onClose,
  onRestored,
}: {
  agent: AgentId;
  title: string;
  description: string;
  onConfirm: () => Promise<ApplyOutcome>;
  onClose: () => void;
  onRestored: () => void;
}) {
  const { notify, t } = useApp();
  const action = useAction();
  const [outcome, setOutcome] = useState<ApplyOutcome | null>(null);
  const [failure, setFailure] = useState<string | null>(null);

  const go = () =>
    void action
      .run(onConfirm, {
        onError: (api) => {
          setFailure(api.hint ? `${api.message} ${api.hint}` : api.message);
          return true;
        },
      })
      .then((result) => {
        if (result) {
          setOutcome(result);
          onRestored();
          notify("success", t("history.restored"));
        }
      });

  return (
    <Modal
      title={title}
      onClose={outcome ? onClose : onClose}
      footer={
        outcome ? (
          <Button variant="primary" onClick={onClose}>
            {t("action.done")}
          </Button>
        ) : (
          <>
            <Button variant="ghost" onClick={onClose}>
              {t("action.cancel")}
            </Button>
            <Button variant="primary" onClick={go} busy={action.busy}>
              {t("history.restore")}
            </Button>
          </>
        )
      }
    >
      {outcome ? (
        <>
          <Banner tone="info" title={t("history.restored")}>
            {t("history.restoredBody", { agent: agent === "codex" ? "Codex" : "Claude Code" })}
          </Banner>
          <Meta
            items={[
              { key: t("home.configFile"), value: outcome.configPath, mono: true },
              {
                key: t("history.nowActive"),
                value: outcome.active.label,
              },
              {
                key: t("history.snapshotTaken"),
                value: outcome.snapshotId,
                mono: true,
              },
              {
                key: t("history.keysUpdated"),
                value:
                  outcome.managedKeys.length > 0
                    ? outcome.managedKeys.join(", ")
                    : t("settings.none"),
                mono: true,
              },
            ]}
          />
          <p className="muted small">{t("history.reversibleNote")}</p>
        </>
      ) : (
        <>
          <p className="muted small">{description}</p>
          <Banner tone="warn" title={t("history.snapshotFirstTitle")}>
            {t("history.snapshotFirstBody")}
          </Banner>
          {failure && (
            <div className="field__error" role="alert">
              {failure}
            </div>
          )}
        </>
      )}
    </Modal>
  );
}
