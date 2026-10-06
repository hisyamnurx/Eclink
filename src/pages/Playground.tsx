/**
 * Playground: a single-turn request against the active provider.
 *
 * Deliberately scoped to one turn with no history and no tools, because its job
 * is to answer "is this endpoint working right now", not to be a chat client.
 * Every request is user-initiated, and the cost notice is repeated here because
 * this is the screen that spends the user's money.
 */

import { useState } from "react";

import {
  Badge,
  Banner,
  Button,
  EmptyState,
  Field,
  LoadingState,
  Meta,
} from "../components/ui";
import { call, type AgentId, type PlaygroundResult, type ProfileSummary } from "../lib/api";
import { latency } from "../lib/format";
import { useApp } from "../state/AppContext";

export function Playground({ initialAgent }: { initialAgent?: AgentId }) {
  const { agents, profiles, loading } = useApp();
  const installed = agents.filter((agent) => agent.installed);
  const [agent, setAgent] = useState<AgentId>(
    initialAgent ?? installed[0]?.agent ?? "codex",
  );

  if (loading) return <LoadingState label="Loading providers…" />;

  const usable = profiles.filter(
    (profile) => profile.targets.includes(agent) && profile.hasSecret,
  );

  return (
    <div className="main__inner">
      <header className="page-header">
        <div className="page-header__text">
          <h1>Playground</h1>
          <p className="page-header__subtitle">
            Send one message to a provider and see the real response. This talks
            directly to your provider using your key.
          </p>
        </div>
      </header>

      {installed.length > 1 && (
        <div className="tabs" role="tablist" aria-label="Coding agent">
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

      <Banner tone="info" title="This request is billable">
        The message below is sent to your provider and may count towards your usage
        or cost. Nothing is sent unless you press Send.
      </Banner>

      {usable.length === 0 ? (
        <EmptyState title="No usable provider for this agent">
          Add a provider for {agent === "codex" ? "Codex" : "Claude Code"} and enter
          an API key, then come back.
        </EmptyState>
      ) : (
        <PlaygroundForm profiles={usable} />
      )}
    </div>
  );
}

function PlaygroundForm({ profiles }: { profiles: ProfileSummary[] }) {
  // The caller guarantees a non-empty list, so the first profile is present.
  const [profileId, setProfileId] = useState(profiles[0]!.id);
  const [prompt, setPrompt] = useState("");
  const [result, setResult] = useState<PlaygroundResult | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);

  const selected = profiles.find((profile) => profile.id === profileId);

  const send = async () => {
    setBusy(true);
    setError(null);
    setResult(null);
    try {
      const response = await call("playground_send", {
        id: profileId,
        prompt: prompt.trim(),
      });
      setResult(response);
    } catch (raw) {
      const api = raw as { message?: string; hint?: string };
      setError(
        [api.message ?? "The request failed.", api.hint].filter(Boolean).join(" "),
      );
    } finally {
      setBusy(false);
    }
  };

  return (
    <section className="card" aria-labelledby="playground-heading">
      <h2 id="playground-heading">Send a message</h2>

      <Field label="Provider" htmlFor="pg-provider">
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
              {profile.name} — {profile.model}
            </option>
          ))}
        </select>
      </Field>

      {selected && (
        <Meta
          items={[
            { key: "Endpoint", value: selected.baseUrl, mono: true },
            { key: "Model", value: selected.model, mono: true },
          ]}
        />
      )}

      <Field
        label="Message"
        htmlFor="pg-prompt"
        hint="One turn only. The provider's reply is shown exactly as it arrives."
      >
        <textarea
          id="pg-prompt"
          className="textarea"
          value={prompt}
          onChange={(event) => setPrompt(event.target.value)}
          placeholder="Reply with the single word: ready"
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
          Send
        </Button>
        {busy && <span className="small muted">Waiting for the provider…</span>}
      </div>

      {error && (
        <Banner tone="bad" title="The request failed">
          {error}
        </Banner>
      )}

      {result && (
        <div className="stack stack--tight">
          <div className="row row--wrap">
            <Badge tone="ok" dot>
              Response received
            </Badge>
            <span className="small dim mono">{latency(result.latencyMs)}</span>
            <span className="small dim mono">{result.model}</span>
            {result.streaming && (
              <Badge tone="info">Streamed</Badge>
            )}
            {result.promptTokens !== undefined && (
              <span className="small dim">
                {result.promptTokens} in / {result.completionTokens ?? "?"} out
                tokens
              </span>
            )}
          </div>
          <pre className="code code--tall selectable">{result.text}</pre>
        </div>
      )}
    </section>
  );
}
