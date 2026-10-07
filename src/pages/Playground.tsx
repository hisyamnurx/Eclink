/**
 * Playground: a single-turn request against the active provider.
 *
 * Deliberately scoped to one turn with no history and no tools, because its job
 * is to answer "is this endpoint working right now", not to be a chat client.
 * Every request is user-initiated, and the cost notice is repeated here because
 * this is the screen that spends the user's money.
 */

import { useState } from "react";

import { Badge, Banner, Button, EmptyState, Field, LoadingState, Meta } from "../components/ui";
import { call, type AgentId, type PlaygroundResult, type ProfileSummary } from "../lib/api";
import { latency } from "../lib/format";
import { errorText } from "../lib/backend-text";
import { useApp } from "../state/AppContext";

export function Playground({ initialAgent }: { initialAgent?: AgentId }) {
  const { agents, profiles, loading, t } = useApp();
  const installed = agents.filter((agent) => agent.installed);
  const [agent, setAgent] = useState<AgentId>(initialAgent ?? installed[0]?.agent ?? "codex");

  if (loading) return <LoadingState label={t("playground.sending")} />;

  const usable = profiles.filter((profile) => profile.targets.includes(agent) && profile.hasSecret);

  return (
    <div className="main__inner">
      <header className="page-header">
        <div className="page-header__text">
          <h1>{t("playground.title")}</h1>
          <p className="page-header__subtitle">{t("playground.subtitle")}</p>
        </div>
      </header>

      {installed.length > 1 && (
        <div className="tabs" role="tablist" aria-label={t("playground.agentTabsAria")}>
          {installed.map((item) => (
            <button
              key={item.agent}
              role="tab"
              type="button"
              className="tab"
              aria-selected={agent === item.agent}
              onClick={() => setAgent(item.agent)}
            >
              {item.displayName}
            </button>
          ))}
        </div>
      )}

      <Banner tone="info" title={t("playground.billableTitle")}>
        {t("playground.billableBody")}
      </Banner>

      {usable.length === 0 ? (
        <EmptyState title={t("playground.noUsableProvider")}>
          {t("playground.noUsableProviderBody", {
            agent: agent === "codex" ? "Codex" : "Claude Code",
          })}
        </EmptyState>
      ) : (
        <PlaygroundForm profiles={usable} agent={agent} />
      )}
    </div>
  );
}

function PlaygroundForm({
  profiles,
  agent,
}: {
  profiles: ProfileSummary[];
  /** The tab the user chose. Sent with the request so the right API is used. */
  agent: AgentId;
}) {
  const { t, language } = useApp();
  // The caller guarantees a non-empty list, so the first profile is present.
  const [profileId, setProfileId] = useState(profiles[0]!.id);
  const [prompt, setPrompt] = useState("");
  const [result, setResult] = useState<PlaygroundResult | null>(null);
  const [error, setError] = useState<string | null>(null);
  /** The provider's own untranslated words, shown separately. */
  const [detail, setDetail] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);

  const selected = profiles.find((profile) => profile.id === profileId);

  const send = async () => {
    setBusy(true);
    setError(null);
    setDetail(null);
    setResult(null);
    try {
      const response = await call("playground_send", {
        id: profileId,
        prompt: prompt.trim(),
        // The selected tab decides which API is used, so it has to be sent. Without
        // it the backend fell back to the profile's first target, which meant
        // choosing the Claude Code tab for a provider that also targets Codex sent
        // a Codex-shaped request anyway.
        agent,
      });
      setResult(response);
    } catch (raw) {
      const api = raw as { message?: string; hint?: string; code?: string; details?: string };
      // The headline is translated by the backend's stable code, so an app-owned
      // explanation follows the chosen language. When the code is one this build
      // does not know, the backend's English is used rather than showing nothing.
      const headline = errorText(api.code, api.message ?? t("playground.requestFailed"), language);
      setError([headline, api.hint].filter(Boolean).join(" "));
      // The provider's own words are never translated, because doing so would mean
      // inventing meaning. They are shown verbatim under a labelled technical
      // section instead of being mixed into the headline.
      setDetail(api.details ?? null);
    } finally {
      setBusy(false);
    }
  };

  return (
    <section className="card" aria-labelledby="playground-heading">
      <h2 id="playground-heading">{t("playground.formTitle")}</h2>

      <Field label={t("playground.provider")} htmlFor="pg-provider">
        <select
          id="pg-provider"
          className="select"
          value={profileId}
          onChange={(event) => {
            setProfileId(event.target.value);
            setResult(null);
            setError(null);
          }}
        >
          {profiles.map((profile) => (
            <option key={profile.id} value={profile.id}>
              {profile.name} - {profile.model}
            </option>
          ))}
        </select>
      </Field>

      {selected && (
        <Meta
          items={[
            { key: t("playground.endpoint"), value: selected.baseUrl, mono: true },
            { key: t("playground.model"), value: selected.model, mono: true },
          ]}
        />
      )}

      <Field label={t("playground.prompt")} htmlFor="pg-prompt" hint={t("playground.promptHint")}>
        <textarea
          id="pg-prompt"
          className="textarea"
          value={prompt}
          onChange={(event) => setPrompt(event.target.value)}
          placeholder={t("playground.askShort")}
          rows={4}
          maxLength={8000}
          disabled={busy}
        />
      </Field>

      <div className="row">
        <Button
          variant="primary"
          onClick={() => void send()}
          busy={busy}
          disabled={!prompt.trim() || busy}
        >
          {t("playground.send")}
        </Button>
        {busy && <span className="small muted">{t("playground.waiting")}</span>}
      </div>

      {error && (
        <Banner tone="bad" title={t("playground.requestFailed")}>
          {error}
          {/* The provider's own words, kept verbatim and labelled as technical.
              Translating them would mean inventing meaning, and hiding them would
              remove the detail a bug report needs. */}
          {detail && (
            <div className="small dim mono" style={{ marginTop: 6, wordBreak: "break-word" }}>
              {t("playground.technicalDetail")}: {detail}
            </div>
          )}
        </Banner>
      )}

      {result && (
        <div className="stack stack--tight">
          <div className="row row--wrap">
            <Badge tone="ok" dot>
              {t("playground.responseReceived")}
            </Badge>
            <span className="small dim mono">{latency(result.latencyMs)}</span>
            <span className="small dim mono">{result.model}</span>
            {/* How the reply actually arrived. A gateway may stream even though this
                request did not ask it to, so reporting the real transport is more
                useful than reporting what was requested. */}
            {result.framing && (
              <Badge tone={result.framing === "SSE" ? "info" : "neutral"}>
                {t("playground.transport")}: {result.framing}
              </Badge>
            )}
            {result.streaming && <Badge tone="info">{t("playground.streamed")}</Badge>}
            {result.promptTokens !== undefined && (
              <span className="small dim">
                {t("playground.tokensInline", {
                  input: result.promptTokens,
                  output: result.completionTokens ?? "?",
                })}
              </span>
            )}
          </div>
          <pre className="code code--tall selectable">{result.text}</pre>
        </div>
      )}
    </section>
  );
}
