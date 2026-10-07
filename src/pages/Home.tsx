/**
 * Home: what each agent is currently pointed at, and the primary actions.
 *
 * "Active:" only names a profile when Eclyzer Bridge recognizes its own managed
 * configuration. When the user has a custom configuration Bridge did not write,
 * this says "External configuration" and never "Default" - claiming otherwise
 * would be a lie about the user's own file (PRD §20.2).
 *
 * Every string the screen authors itself goes through `t()`. The one exception is
 * `agent.activeLabel`, which the backend builds, and `agent.note`, which is the
 * backend's own wording; translating either here would mean guessing at copy this
 * screen does not own.
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
import { relativeTime } from "../lib/format";
import { compatibilityLabel } from "../lib/compatibility";
import type { MessageKey } from "../lib/i18n";
import { openUrl, pickFolder } from "../lib/opener";
import { useAction, useApp } from "../state/AppContext";

interface HomeProps {
  onConfigure: (agent?: AgentId) => void;
  onHistory: (agent: AgentId) => void;
  onPlayground: (agent: AgentId) => void;
}

/* The status-to-word mapping is not duplicated here: it lives in
 * `lib/compatibility`. This file, TestResults, and the provider list each used to
 * carry their own copy, which is how the badge and the Activate button once came to
 * disagree about whether a provider had been tested. */

export function Home({ onConfigure, onHistory, onPlayground }: HomeProps) {
  const { agents, profiles, loading, loadError, reload, t } = useApp();
  const [launchTarget, setLaunchTarget] = useState<AgentId | null>(null);

  if (loading) {
    return <LoadingState label={t("home.checkingSetup")} />;
  }

  if (loadError) {
    return <ErrorState error={loadError} onRetry={() => void reload()} />;
  }

  const configured = profiles.length > 0;

  return (
    <div className="main__inner">
      <header className="page-header">
        <div className="page-header__text">
          <h1>{t("home.title")}</h1>
          <p className="page-header__subtitle">{t("home.intro")}</p>
        </div>
        <Button variant="primary" onClick={() => onConfigure()}>
          {configured ? t("providers.add") : t("home.setUpProvider")}
        </Button>
      </header>

      {agents.length === 0 && (
        <EmptyState title={t("home.noAgents")}>{t("home.noAgentsLong")}</EmptyState>
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
  const { profiles, refreshAgent, notify, t } = useApp();
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
            {agent.installed
              ? t("home.versionValue", { version: agent.version ?? t("home.versionUnknown") })
              : t("home.notInstalled")}
          </div>
        </div>
        <div className="card__actions">
          <Badge tone={installedTone} dot>
            {agent.installed ? t("status.detected") : t("status.notFound")}
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
              ? t("status.configUnreadable")
              : agent.configState === "valid"
                ? t("status.configOk")
                : t("status.noConfigYet")}
          </Badge>
        </div>
      </div>

      {!agent.installed && (
        <Banner
          tone="warn"
          title={t("home.notFoundTitle", { agent: agent.displayName })}
          actions={
            agent.installGuideUrl ? (
              <Button
                size="sm"
                variant="secondary"
                onClick={() => void openLink(agent.installGuideUrl!, notify, t)}
              >
                {t("home.installGuide")}
              </Button>
            ) : undefined
          }
        >
          {t("home.notFoundBody", { agent: agent.displayName })}
        </Banner>
      )}

      {agent.configState === "invalid" && (
        <Banner tone="bad" title={t("home.invalidConfigTitle")}>
          {t("home.invalidConfigBody", { path: agent.configPath ?? "" })}
        </Banner>
      )}

      {/* `agent.note` is the backend's own sentence, so it is shown as sent. */}
      {agent.note && <Banner tone="warn">{agent.note}</Banner>}

      <Meta
        items={[
          { key: t("action.active"), value: <ActiveValue agent={agent} /> },
          {
            key: t("playground.endpoint"),
            value: agent.activeKind === "managed" && candidate ? candidate.baseUrl : "-",
            mono: true,
          },
          {
            key: t("providers.model"),
            value: agent.activeKind === "managed" && candidate ? candidate.model : "-",
            mono: true,
          },
          {
            key: t("home.lastTested"),
            value: record?.testedAt
              ? `${t(compatibilityLabel(record?.status))} · ${relativeTime(record.testedAt)}`
              : t("status.notTested"),
          },
          {
            key: t("home.configFile"),
            value: agent.configPath ?? t("status.notFound"),
            mono: true,
          },
        ]}
      />

      <div className="row row--wrap">
        <Button variant="primary" onClick={onConfigure} disabled={!agent.installed}>
          {t("home.configure")}
        </Button>
        <Button onClick={onLaunch} disabled={!agent.installed}>
          {t("home.openAgent", { agent: agent.displayName })}
        </Button>
        <Button onClick={onPlayground} disabled={!agent.installed}>
          {t("home.playground")}
        </Button>
        <Button onClick={onHistory} variant="ghost">
          {t("home.history")}
        </Button>
        <div className="spacer" />
        <Button
          variant="ghost"
          onClick={onRefresh}
          busy={refresh.busy}
          aria-label={t("home.refreshAgent", { agent: agent.displayName })}
        >
          {t("home.refresh")}
        </Button>
      </div>
    </section>
  );
}

/** The "Active" value, which must never overstate what Bridge knows. */
function ActiveValue({ agent }: { agent: AgentSummary }) {
  const { t } = useApp();

  if (agent.activeKind === "managed") {
    return (
      <>
        {agent.activeProfileName ?? t("home.activeFallback")}{" "}
        <span className="dim small">({t("home.activeManagedHint")})</span>
      </>
    );
  }
  if (agent.activeKind === "external") {
    return (
      <>
        {t("status.externalConfiguration")}{" "}
        <span className="dim small">({t("home.externalHint")})</span>
      </>
    );
  }
  return (
    <>
      {t("home.defaultConfiguration")} <span className="dim small">({t("home.defaultHint")})</span>
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
  t: (key: MessageKey) => string,
) {
  void openUrl(url).catch((error: unknown) => {
    const api = error instanceof ApiError ? error : null;
    notify("error", api?.message ?? t("home.openLinkFailed"), api?.hint ?? url);
  });
}

/** Launch dialog: choose a project folder, preview the command, then run it. */
function LaunchDialog({ agent, onClose }: { agent: AgentId; onClose: () => void }) {
  const { settings, saveSettings, notify, t } = useApp();
  const [directory, setDirectory] = useState(settings?.lastProjectDirectory ?? "");
  const [preview, setPreview] = useState<string | null>(null);
  const [previewError, setPreviewError] = useState<string | null>(null);
  const action = useAction();
  const remember = settings?.rememberProjectDirectory ?? false;

  const agentName = agent === "codex" ? "Codex" : "Claude Code";

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
      notify("error", api?.message ?? t("launch.folderPickerFailed"));
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
        notify("success", t("launch.openedToast"));
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
      title={t("launch.title", { agent: agentName })}
      onClose={onClose}
      footer={
        <>
          <Button variant="ghost" onClick={onClose}>
            {t("action.cancel")}
          </Button>
          <Button variant="secondary" onClick={doPreview} busy={action.busy} disabled={!directory}>
            {t("launch.preview")}
          </Button>
          <Button variant="primary" onClick={doLaunch} busy={action.busy} disabled={!directory}>
            {t("action.open")}
          </Button>
        </>
      }
    >
      <p className="muted small">{t("launch.body", { agent: agentName })}</p>

      <div className="field">
        <label className="field__label" htmlFor="launch-folder">
          {t("launch.folderLabel")}
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
            {t("action.browse")}
          </Button>
        </div>
        <div className="field__hint" id="launch-folder-hint">
          {t("launch.folderHint", { agent: agentName })}
        </div>
        {previewError && (
          <div className="field__error" role="alert">
            {previewError}
          </div>
        )}
      </div>

      {preview && (
        <div className="stack stack--tight">
          <div className="field__label">{t("launch.previewTitle")}</div>
          <pre className="code selectable">{preview}</pre>
          <div className="field__hint">{t("launch.previewHint")}</div>
        </div>
      )}
    </Modal>
  );
}
