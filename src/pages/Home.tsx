/**
 * Home: what each agent is currently pointed at, and the primary actions.
 *
 * "Active:" only names a profile when Eclyzer Bridge recognizes its own managed
 * configuration. When the user has a custom configuration Bridge did not write,
 * this says "External Configuration" and never "Default" — claiming otherwise
 * would be a lie about the user's own file (PRD §20.2).
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
  type StatusTone,
} from "../components/ui";
import { ApiError, call, type AgentId, type AgentSummary } from "../lib/api";
import { compatibilityLabel } from "../lib/compatibility";
import { relativeTime } from "../lib/format";
import { openUrl, pickFolder } from "../lib/opener";
import { useAction, useApp } from "../state/AppContext";

interface HomeProps {
  onConfigure: (agent?: AgentId) => void;
  onHistory: (agent: AgentId) => void;
  onPlayground: (agent: AgentId) => void;
}

export function Home({ onConfigure, onHistory, onPlayground }: HomeProps) {
  const { agents, profiles, loading, loadError, reload } = useApp();
  const [launchTarget, setLaunchTarget] = useState<AgentId | null>(null);

  if (loading) {
    return <LoadingState label="Checking your setup…" />;
  }

  if (loadError) {
    return <ErrorState error={loadError} onRetry={() => void reload()} />;
  }

  const configured = profiles.length > 0;

  return (
    <div className="main__inner">
      <header className="page-header">
        <div className="page-header__text">
          <h1>Overview</h1>
          <p className="page-header__subtitle">
            Eclyzer Bridge points Codex and Claude Code at the provider you choose, without you
            editing configuration files by hand.
          </p>
        </div>
        <Button variant="primary" onClick={() => onConfigure()}>
          {configured ? "Add provider" : "Set up a provider"}
        </Button>
      </header>

      {agents.length === 0 && (
        <EmptyState title="No coding agents were found">
          Eclyzer Bridge could not find Codex or Claude Code on this machine. Install one, then
          choose Refresh.
        </EmptyState>
      )}

      {agents.map((agent) => (
        <AgentCard
          key={agent.agent}
          agent={agent}
          onConfigure={() => onConfigure(agent.agent)}
          onHistory={() => onHistory(agent.agent)}
          onPlayground={() => onPlayground(agent.agent)}
          onLaunch={() => setLaunchTarget(agent.agent)}
        />
      ))}

      {launchTarget && <LaunchDialog agent={launchTarget} onClose={() => setLaunchTarget(null)} />}
    </div>
  );
}

function AgentCard({
  agent,
  onConfigure,
  onHistory,
  onPlayground,
  onLaunch,
}: {
  agent: AgentSummary;
  onConfigure: () => void;
  onHistory: () => void;
  onPlayground: () => void;
  onLaunch: () => void;
}) {
  const { profiles, refreshAgent, notify } = useApp();
  const refresh = useAction();

  // The profile the user believes is active for this agent, if any targets it.
  const candidate = profiles.find((profile) => profile.targets.includes(agent.agent));
  const record = candidate?.compatibility?.[agent.agent];

  const installedTone: StatusTone = agent.installed ? "ok" : "warn";

  const onRefresh = () => void refresh.run(() => refreshAgent(agent.agent), {});
  return (
    <section className="card" aria-labelledby={`agent-${agent.agent}`}>
      <div className="card__header">
        <div className="card__title">
          <h2 id={`agent-${agent.agent}`}>{agent.displayName}</h2>
          <div className="card__hint">
            {agent.installed ? `Version ${agent.version ?? "unknown"}` : "Not installed"}
          </div>
        </div>
        <div className="card__actions">
          <Badge tone={installedTone} dot>
            {agent.installed ? "Detected" : "Not found"}
          </Badge>
          <Badge
            tone={
              agent.configState === "invalid"
                ? "bad"
                : agent.configState === "valid"
                  ? "ok"
                  : "neutral"
            }
            dot
          >
            {agent.configState === "invalid"
              ? "Config unreadable"
              : agent.configState === "valid"
                ? "Config OK"
                : "No config yet"}
          </Badge>
        </div>
      </div>

      {!agent.installed && (
        <Banner
          tone="warn"
          title={`${agent.displayName} was not found`}
          actions={
            agent.installGuideUrl ? (
              <Button
                size="sm"
                variant="secondary"
                onClick={() => void openLink(agent.installGuideUrl!, notify)}
              >
                Installation guide
              </Button>
            ) : undefined
          }
        >
          Eclyzer Bridge installs nothing on your behalf. Install {agent.displayName}, then choose
          Refresh.
        </Banner>
      )}

      {agent.configState === "invalid" && (
        <Banner tone="bad" title="Eclyzer Bridge will not modify this configuration">
          {agent.configPath} is not valid. Eclyzer Bridge refuses to rewrite a file it cannot parse,
          because doing so would destroy settings it does not understand. Fix the file yourself,
          then choose Refresh.
        </Banner>
      )}

      {agent.note && <Banner tone="warn">{agent.note}</Banner>}

      <Meta
        items={[
          { key: "Active", value: <ActiveValue agent={agent} /> },
          {
            key: "Endpoint",
            value: agent.activeKind === "managed" && candidate ? candidate.baseUrl : "—",
            mono: true,
          },
          {
            key: "Model",
            value: agent.activeKind === "managed" && candidate ? candidate.model : "—",
            mono: true,
          },
          {
            key: "Last tested",
            value: record?.testedAt
              ? `${compatibilityLabel(record.status)} · ${relativeTime(record.testedAt)}`
              : "Not tested",
          },
          {
            key: "Config file",
            value: agent.configPath ?? "not found",
            mono: true,
          },
        ]}
      />

      <div className="row row--wrap">
        <Button variant="primary" onClick={onConfigure} disabled={!agent.installed}>
          Configure
        </Button>
        <Button onClick={onLaunch} disabled={!agent.installed}>
          Open {agent.displayName}
        </Button>
        <Button onClick={onPlayground} disabled={!agent.installed}>
          Playground
        </Button>
        <Button onClick={onHistory} variant="ghost">
          History
        </Button>
        <div className="spacer" />
        <Button
          variant="ghost"
          onClick={onRefresh}
          busy={refresh.busy}
          aria-label={`Refresh ${agent.displayName} detection`}
        >
          Refresh
        </Button>
      </div>
    </section>
  );
}

/** The "Active" value, which must never overstate what Bridge knows. */
function ActiveValue({ agent }: { agent: AgentSummary }) {
  if (agent.activeKind === "managed") {
    return (
      <>
        {agent.activeProfileName ?? "A provider profile"}{" "}
        <span className="dim small">(set by Eclyzer Bridge)</span>
      </>
    );
  }
  if (agent.activeKind === "external") {
    return (
      <>
        External Configuration{" "}
        <span className="dim small">(your own settings; Eclyzer Bridge did not write these)</span>
      </>
    );
  }
  return (
    <>
      Default configuration <span className="dim small">(no custom provider configured)</span>
    </>
  );
}

/**
 * Open an external link in the user's browser and report a failure.
 *
 * The URL is validated by `openUrl` before it reaches the OS, so a link from a
 * provider template can never become a local file launch.
 */
function openLink(
  url: string,
  notify: (tone: "success" | "error" | "info", title: string, text?: string) => void,
) {
  void openUrl(url).catch((error: unknown) => {
    const api = error instanceof ApiError ? error : null;
    notify("error", api?.message ?? "Could not open the link.", api?.hint ?? url);
  });
}

/** Launch dialog: choose a project folder, preview the command, then run it. */
function LaunchDialog({ agent, onClose }: { agent: AgentId; onClose: () => void }) {
  const { settings, saveSettings, notify } = useApp();
  const [directory, setDirectory] = useState(settings?.lastProjectDirectory ?? "");
  const [preview, setPreview] = useState<string | null>(null);
  const [previewError, setPreviewError] = useState<string | null>(null);
  const action = useAction();
  const remember = settings?.rememberProjectDirectory ?? false;

  const chooseFolder = async () => {
    try {
      const picked = await pickFolder(settings?.lastProjectDirectory);
      if (picked) {
        setDirectory(picked);
        setPreview(null);
        setPreviewError(null);
      }
    } catch (error) {
      const api = error instanceof ApiError ? error : null;
      notify("error", api?.message ?? "Could not open the folder picker.");
    }
  };

  const doPreview = () =>
    void action.run(
      async () => {
        setPreviewError(null);
        const result = await call("preview_launch", {
          agent,
          projectDirectory: directory,
        });
        setPreview(result.display);
        return result;
      },
      {
        onError: (api) => {
          // A bad path is the user's to fix, not a toast; show it inline.
          setPreviewError(api.hint ? `${api.message} ${api.hint}` : api.message);
          return true;
        },
      },
    );

  const doLaunch = () =>
    void action.run(
      async () => {
        await call("launch_agent", { agent, projectDirectory: directory });

        // Remembering the folder is a preference, so persist it only when the
        // user asked for that.
        if (remember && settings?.lastProjectDirectory !== directory) {
          await saveSettings({ lastProjectDirectory: directory });
        }
        notify("success", "Opened in a new terminal window.");
        onClose();
      },
      {
        onError: (api) => {
          setPreviewError(api.hint ? `${api.message} ${api.hint}` : api.message);
          return true;
        },
      },
    );

  return (
    <Modal
      title={`Open ${agent === "codex" ? "Codex" : "Claude Code"}`}
      onClose={onClose}
      footer={
        <>
          <Button variant="ghost" onClick={onClose}>
            Cancel
          </Button>
          <Button variant="secondary" onClick={doPreview} busy={action.busy} disabled={!directory}>
            Preview command
          </Button>
          <Button variant="primary" onClick={doLaunch} busy={action.busy} disabled={!directory}>
            Open
          </Button>
        </>
      }
    >
      <p className="muted small">
        The agent opens in a new terminal window in the folder you choose. Eclyzer Bridge exits as
        soon as the window opens — closing Bridge never closes your agent.
      </p>

      <div className="field">
        <label className="field__label" htmlFor="launch-folder">
          Project folder
        </label>
        <div className="row">
          <input
            id="launch-folder"
            className="input mono"
            value={directory}
            onChange={(event) => {
              setDirectory(event.target.value);
              setPreview(null);
              setPreviewError(null);
            }}
            placeholder="C:\Projects\my-app"
            aria-describedby="launch-folder-hint"
            aria-invalid={previewError ? true : undefined}
          />
          <Button onClick={() => void chooseFolder()} className="nowrap">
            Browse…
          </Button>
        </div>
        <div className="field__hint" id="launch-folder-hint">
          The folder must already exist.
        </div>
        {previewError && (
          <div className="field__error" role="alert">
            {previewError}
          </div>
        )}
      </div>

      {preview && (
        <div className="stack stack--tight">
          <div className="field__label">Command that will run</div>
          <pre className="code selectable">{preview}</pre>
          <div className="field__hint">
            The model and provider names are passed as separate arguments, never interpreted as
            shell commands.
          </div>
        </div>
      )}
    </Modal>
  );
}
