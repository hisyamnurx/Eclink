/**
 * History and recovery (PRD §26–§30).
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
import { call, type AgentId, type ApplyOutcome, type SnapshotView } from "../lib/api";
import { absoluteTime, bytes, relativeTime } from "../lib/format";
import { useAsyncData } from "../lib/hooks";
import { useAction, useApp } from "../state/AppContext";

export function History({ initialAgent }: { initialAgent?: AgentId }) {
  const { agents } = useApp();
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
        <h1>History</h1>
        <EmptyState title="No coding agent detected">
          History appears here once Eclyzer Bridge can see Codex or Claude Code.
        </EmptyState>
      </div>
    );
  }

  return (
    <div className="main__inner">
      <header className="page-header">
        <div className="page-header__text">
          <h1>History</h1>
          <p className="page-header__subtitle">
            Every change Eclyzer Bridge has made, and every state it can put back. Restoring always
            snapshots what it is about to replace.
          </p>
        </div>
      </header>

      <div className="tabs" role="tablist" aria-label="Coding agent">
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
          { id: "changes", label: "Changes" },
          { id: "snapshots", label: "Snapshots" },
          { id: "recovery", label: "Restore" },
        ]}
        value={tab}
        onChange={setTab}
        label="History view"
      />

      {tab === "changes" && <ChangeLog agent={agent} />}
      {tab === "snapshots" && <SnapshotList agent={agent} />}
      {tab === "recovery" && <RecoveryPanel agent={agent} />}
    </div>
  );
}

function ChangeLog({ agent }: { agent: AgentId }) {
  const {
    data: entries,
    error,
    loading,
    reload,
  } = useAsyncData(() => call("list_history", { agent, limit: 100 }), [agent]);

  if (error) return <ErrorState error={error} onRetry={reload} />;
  if (loading && !entries) return <LoadingState label="Loading change history…" />;

  if (!entries || entries.length === 0) {
    return (
      <EmptyState title="No changes recorded yet">
        Eclyzer Bridge has not modified this agent's configuration. Activating a provider will be
        recorded here.
      </EmptyState>
    );
  }

  return (
    <div className="card card--flush">
      <table className="table">
        <caption className="visually-hidden">Changes Eclyzer Bridge has made to this agent</caption>
        <thead>
          <tr>
            <th scope="col">When</th>
            <th scope="col">Change</th>
            <th scope="col">Result</th>
            <th scope="col">Undoable</th>
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
                    Succeeded
                  </Badge>
                ) : (
                  <>
                    <Badge tone="bad" dot>
                      Failed
                    </Badge>
                    {entry.error && <div className="small dim">{entry.error}</div>}
                  </>
                )}
              </td>
              <td>{entry.success && entry.snapshotId ? "Yes" : <span className="dim">—</span>}</td>
            </tr>
          ))}
        </tbody>
      </table>
    </div>
  );
}

function SnapshotList({ agent }: { agent: AgentId }) {
  const [confirm, setConfirm] = useState<SnapshotView | null>(null);
  const {
    data: snapshots,
    error,
    loading,
    reload,
  } = useAsyncData(() => call("list_snapshots", { agent }), [agent]);

  if (error) return <ErrorState error={error} onRetry={reload} />;
  if (loading && !snapshots) return <LoadingState label="Loading snapshots…" />;

  if (!snapshots || snapshots.length === 0) {
    return (
      <EmptyState title="No snapshots yet">
        A snapshot is saved before every change, so there is always something to return to. None
        have been taken for this agent.
      </EmptyState>
    );
  }

  return (
    <>
      <Banner tone="info" title="The newest 20 snapshots are kept">
        Older ones are removed automatically. Your original configuration is kept separately and is
        never deleted.
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
                Restore
              </Button>
            </div>
          </div>
        ))}
      </div>

      {confirm && (
        <RestoreConfirmDialog
          agent={agent}
          title={`Restore the snapshot from ${absoluteTime(confirm.createdAt)}?`}
          description={`This replaces your current ${agent === "codex" ? "Codex" : "Claude Code"} configuration with the state saved before "${confirm.label}".`}
          onConfirm={() => call("restore_snapshot", { agent, snapshotId: confirm.id })}
          onClose={() => setConfirm(null)}
          onRestored={reload}
        />
      )}
    </>
  );
}

function RecoveryPanel({ agent }: { agent: AgentId }) {
  const [pending, setPending] = useState<"default" | "original" | null>(null);
  const action = useAction();
  const {
    data: status,
    error,
    loading,
    reload,
  } = useAsyncData(() => call("recovery_status", { agent }), [agent]);

  const undo = () =>
    void action
      .run(() => call("undo_last_change", { agent }), {
        success: "The last change was undone.",
      })
      .then(reload);

  if (error) return <ErrorState error={error} onRetry={reload} />;
  if (loading && !status) return <LoadingState label="Checking recovery options…" />;
  if (!status) return <ErrorState error="Recovery options could not be read." />;

  return (
    <>
      {status.externalConfiguration && (
        <Banner tone="warn" title="Your configuration was not written by Eclyzer Bridge">
          This agent is using a custom configuration Eclyzer Bridge did not create. Restoring the
          default would replace it, so the option below is offered with that in mind.
        </Banner>
      )}

      <Meta
        items={[
          {
            key: "Currently",
            value: status.hasManagedConfiguration
              ? "Eclyzer Bridge configuration is active"
              : status.externalConfiguration
                ? "External configuration"
                : "Default configuration",
          },
          {
            key: "Original backup",
            value: status.hasOriginal
              ? "Saved — the state before Eclyzer Bridge first changed anything"
              : "Not saved yet",
          },
          { key: "Snapshots", value: String(status.snapshotCount) },
          {
            key: "Last change",
            value: status.lastChangeLabel ?? "None recorded",
          },
        ]}
      />

      <div className="stack stack--tight">
        <h2>Undo the last change</h2>
        <p className="card__hint">
          Returns this agent to the state just before its most recent change.
          {status.canUndo
            ? " The current state is snapshotted first, so you can undo the undo."
            : " Nothing to undo right now."}
        </p>
        <div>
          <Button onClick={undo} busy={action.busy} disabled={!status.canUndo}>
            Undo last change
          </Button>
        </div>
      </div>

      <hr className="divider" />

      <div className="stack stack--tight">
        <h2>Restore the default configuration</h2>
        <p className="card__hint">
          Removes the provider Eclyzer Bridge added, returning this agent to its own default.
          Everything else in your configuration — comments, other providers, sandbox settings — is
          left untouched.
        </p>
        <div>
          <Button onClick={() => setPending("default")}>Restore default</Button>
        </div>
      </div>

      <hr className="divider" />

      <div className="stack stack--tight">
        <h2>Restore the original configuration</h2>
        <p className="card__hint">
          {status.hasOriginal
            ? "Puts back the exact file that existed before Eclyzer Bridge ever touched this agent."
            : "Available once Eclyzer Bridge has changed this agent at least once."}
        </p>
        <div>
          <Button onClick={() => setPending("original")} disabled={!status.hasOriginal}>
            Restore original
          </Button>
        </div>
      </div>

      {pending && (
        <RestoreConfirmDialog
          agent={agent}
          title={
            pending === "default"
              ? "Restore the default configuration?"
              : "Restore the original configuration?"
          }
          description={
            pending === "default"
              ? "The provider Eclyzer Bridge added is removed. Your other settings stay as they are."
              : "Your configuration file is replaced with the copy saved before Eclyzer Bridge first changed it. Any changes you made since then are lost."
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
  const { notify } = useApp();
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
          notify("success", "Configuration restored.");
        }
      });

  return (
    <Modal
      title={title}
      onClose={outcome ? onClose : onClose}
      footer={
        outcome ? (
          <Button variant="primary" onClick={onClose}>
            Done
          </Button>
        ) : (
          <>
            <Button variant="ghost" onClick={onClose}>
              Cancel
            </Button>
            <Button variant="primary" onClick={go} busy={action.busy}>
              Restore
            </Button>
          </>
        )
      }
    >
      {outcome ? (
        <>
          <Banner tone="info" title="Restored">
            {agent === "codex" ? "Codex" : "Claude Code"} configuration has been written and
            verified.
          </Banner>
          <Meta
            items={[
              { key: "Config file", value: outcome.configPath, mono: true },
              {
                key: "Now active",
                value: outcome.active.label,
              },
              {
                key: "Snapshot taken",
                value: outcome.snapshotId,
                mono: true,
              },
              {
                key: "Keys updated",
                value: outcome.managedKeys.length > 0 ? outcome.managedKeys.join(", ") : "none",
                mono: true,
              },
            ]}
          />
          <p className="muted small">
            The state you just replaced was snapshotted, so this is reversible from the Snapshots
            list.
          </p>
        </>
      ) : (
        <>
          <p className="muted small">{description}</p>
          <Banner tone="warn" title="A snapshot is taken first">
            Whatever is there now is saved, so you can undo this.
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
