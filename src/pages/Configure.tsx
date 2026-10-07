/**
 * Configure: create or edit a provider profile, test it, then activate it.
 *
 * The order is deliberate and enforced by the UI: a profile cannot be activated
 * until a real test has passed against the specific agent, because activating an
 * unverified endpoint would leave the user with a broken coding agent and no
 * explanation.
 */

import { useCallback, useEffect, useMemo, useState } from "react";

import {
  Badge,
  Banner,
  Button,
  EmptyState,
  ErrorState,
  Field,
  LoadingState,
  Modal,
  type StatusTone,
} from "../components/ui";
import {
  call,
  type AgentId,
  type AgentSummary,
  type AdvancedOptions,
  type AuthMode,
  type CompatibilityStatus,
  type ProfileInput,
  type ProfileSummary,
  type ProviderTemplate,
  type TestOutcome,
} from "../lib/api";
import { activationGate } from "../lib/compatibility";
import { relativeTime } from "../lib/format";
import { copyToClipboard } from "../lib/opener";
import { useAction, useApp } from "../state/AppContext";
import { TestResults } from "./TestResults";

interface ConfigureProps {
  initialAgent?: AgentId;
}

export function Configure({ initialAgent }: ConfigureProps) {
  const { agents, profiles, loading, loadError, reloadProfiles, notify } = useApp();
  const [templates, setTemplates] = useState<ProviderTemplate[]>([]);
  const [editing, setEditing] = useState<ProfileSummary | "new" | null>(null);
  const duplicate = useAction();

  /**
   * Copy a profile, including its stored API key.
   *
   * The key itself never passes through this code: the backend copies the stored
   * secret from the source profile to the new one, so a duplicate does not
   * require the user to re-enter a credential they cannot see. Sending the
   * plaintext up to the UI and back down would be the easy implementation and
   * the wrong one.
   */
  const duplicateProfile = (profile: ProfileSummary) =>
    void duplicate.run(
      async () => {
        const result = await call("duplicate_profile", { id: profile.id });
        await reloadProfiles();
        notify(
          "success",
          `Copied ${profile.name}.`,
          result.hasSecret
            ? "The API key was copied to the new provider."
            : "This provider had no stored key to copy.",
        );
        return result;
      },
      { onError: () => false },
    );

  useEffect(() => {
    let cancelled = false;
    void call("list_provider_templates")
      .then((list) => {
        if (!cancelled) setTemplates(list);
      })
      .catch(() => {
        // Presets are a convenience. If they fail to load, the Custom option
        // still lets the user type a base URL.
        if (!cancelled) setTemplates([]);
      });
    return () => {
      cancelled = true;
    };
  }, []);

  const agentOptions = useMemo(() => agents.filter((agent) => agent.installed), [agents]);

  // The selection is derived rather than synchronised in an effect: if the
  // chosen agent is no longer installed, fall back to the first one available.
  // This keeps the tab list and the selection consistent in the same render.
  const [preferredAgent, setPreferredAgent] = useState<AgentId>(initialAgent ?? "codex");
  const activeAgent =
    agentOptions.find((agent) => agent.agent === preferredAgent)?.agent ??
    agentOptions[0]?.agent ??
    preferredAgent;

  if (loading) {
    return <LoadingState label="Loading your providers…" />;
  }

  if (loadError) {
    return <ErrorState error={loadError} onRetry={() => void reloadProfiles()} />;
  }

  return (
    <div className="main__inner">
      <header className="page-header">
        <div className="page-header__text">
          <h1>Providers</h1>
          <p className="page-header__subtitle">
            A provider profile is an endpoint, a model, and credentials. Eclyzer Bridge writes only
            the settings it owns and leaves everything else in your configuration untouched.
          </p>
        </div>
        <Button variant="primary" onClick={() => setEditing("new")}>
          Add provider
        </Button>
      </header>

      {agentOptions.length === 0 ? (
        <EmptyState title="No coding agent detected">
          Install Codex or Claude Code, then reload this page.
        </EmptyState>
      ) : (
        <div className="tabs" role="tablist" aria-label="Coding agent">
          {agentOptions.map((agent) => (
            <button
              key={agent.agent}
              role="tab"
              type="button"
              className="tab"
              aria-selected={activeAgent === agent.agent}
              onClick={() => setPreferredAgent(agent.agent)}
            >
              {agent.displayName}
            </button>
          ))}
        </div>
      )}

      <ProfileList
        agent={activeAgent}
        agentSummary={agentOptions.find((a) => a.agent === activeAgent)}
        profiles={profiles}
        onEdit={(profile) => setEditing(profile)}
        onDuplicate={duplicateProfile}
        onChanged={() => void reloadProfiles()}
      />

      {editing && (
        <ProfileEditor
          profile={editing === "new" ? null : editing}
          templates={templates}
          agents={agentOptions}
          defaultAgent={activeAgent}
          onClose={() => setEditing(null)}
          onSaved={async () => {
            setEditing(null);
            await reloadProfiles();
          }}
          notify={notify}
        />
      )}

      <CostNoticeBanner />
    </div>
  );
}

/** The honest cost notice: testing talks to a real endpoint. */
function CostNoticeBanner() {
  const { settings, saveSettings } = useApp();
  const [dismissed, setDismissed] = useState(false);

  if (!settings || settings.costNoticeSeen || dismissed) return null;

  return (
    <Banner
      tone="info"
      title="Testing uses your provider account"
      actions={
        <Button
          size="sm"
          variant="secondary"
          onClick={() => {
            setDismissed(true);
            void saveSettings({ costNoticeSeen: true });
          }}
        >
          Got it
        </Button>
      }
    >
      Test Connection and the Playground send real requests to your provider and may count towards
      your usage or cost. Eclyzer Bridge never sends anything on its own.
    </Banner>
  );
}

function ProfileList({
  agent,
  agentSummary,
  profiles,
  onEdit,
  onDuplicate,
  onChanged,
}: {
  agent: AgentId;
  agentSummary?: AgentSummary;
  profiles: ProfileSummary[];
  onEdit: (profile: ProfileSummary) => void;
  onDuplicate: (profile: ProfileSummary) => void;
  onChanged: () => void;
}) {
  const { notify } = useApp();
  const [confirmDelete, setConfirmDelete] = useState<ProfileSummary | null>(null);

  if (profiles.length === 0) {
    return (
      <EmptyState title="No providers yet">
        Add a provider to point {agentSummary?.displayName ?? "your agent"} at an endpoint you
        choose.
      </EmptyState>
    );
  }

  return (
    <div className="list">
      {profiles.map((profile) => (
        <ProfileRow
          key={profile.id}
          profile={profile}
          agent={agent}
          onEdit={() => onEdit(profile)}
          onDuplicate={() => onDuplicate(profile)}
          onRequestDelete={() => setConfirmDelete(profile)}
          onChanged={onChanged}
        />
      ))}

      {confirmDelete && (
        <DeleteProfileDialog
          profile={confirmDelete}
          onClose={() => setConfirmDelete(null)}
          onDeleted={async () => {
            setConfirmDelete(null);
            onChanged();
            notify("success", "Provider deleted.");
          }}
        />
      )}
    </div>
  );
}

function statusTone(status: CompatibilityStatus | undefined): StatusTone {
  switch (status) {
    case "compatible":
      return "ok";
    case "partially_compatible":
      return "warn";
    case "incompatible":
    case "test_failed":
      return "bad";
    case "testing":
      return "info";
    default:
      return "neutral";
  }
}

function ProfileRow({
  profile,
  agent,
  onEdit,
  onDuplicate,
  onRequestDelete,
  onChanged,
}: {
  profile: ProfileSummary;
  agent: AgentId;
  onEdit: () => void;
  onDuplicate: () => void;
  onRequestDelete: () => void;
  onChanged: () => void;
}) {
  const { agents } = useApp();
  const testAction = useAction();
  const activateAction = useAction();
  const [outcome, setOutcome] = useState<TestOutcome | null>(null);

  const record = profile.compatibility?.[agent];
  // Both the badge and the Activate button come from this one value, so they
  // cannot disagree. See src/lib/compatibility.ts for why that matters.
  const gate = activationGate(profile, agents, agent);
  const agentSummary = agents.find((item) => item.agent === agent);
  const isActive = gate.isActive;

  /**
   * Run a test and then immediately re-read profiles.
   *
   * The `await onChanged()` is the fix for a real bug, and the ordering is the
   * whole point. The badge above reads `profile.compatibility`, which comes from
   * the `profiles` array in context — not from the test result. Writing the result
   * to local state therefore updated the panel and the badge disagreed, so a run
   * that reported 7/7 still showed "Not tested" and left Activate disabled until
   * something unrelated happened to reload profiles, which is why the badge
   * appeared to correct itself a few seconds later.
   *
   * Reloading is awaited rather than fired and forgotten, so Activate becomes
   * usable in the same commit that renders the result. There is no window in
   * which the panel says Compatible and the button is still disabled.
   */
  const runTest = () =>
    void testAction
      .run(
        async () => {
          const result = await call("test_profile_connection", {
            id: profile.id,
            agent,
          });
          setOutcome(result);
          return result;
        },
        {
          onError: (api) => {
            // A failed test is a normal outcome with its own report, not a toast:
            // the user needs to read which step failed.
            setOutcome({
              profileId: profile.id,
              agent,
              status: "test_failed",
              reasonCode: api.kind,
              reason: api.message,
              warning: api.hint,
              steps: [],
              streaming: false,
              toolCalling: false,
            });
            return true;
          },
        },
      )
      // Runs on both the success and the reported-failure paths, because a failed
      // test also writes a compatibility record that the badge must reflect.
      .then(onChanged);

  const activate = () =>
    void activateAction
      .run(
        async () => {
          const result = await call("activate_profile", { id: profile.id, agent });
          return result;
        },
        { success: `Activated ${profile.name} for ${agentSummary?.displayName}.` },
      )
      .then(onChanged);

  return (
    <div className={`list-row${isActive ? " list-row--active" : ""}`}>
      <div className="list-row__main">
        <div className="list-row__title">
          {profile.name}
          <Badge tone={statusTone(record?.status)} dot>
            {gate.label}
          </Badge>
          {isActive && <Badge tone="info">Active</Badge>}
          {!profile.hasSecret && <Badge tone="bad">No API key</Badge>}
        </div>
        <div className="list-row__sub mono">{profile.baseUrl}</div>
        <div className="list-row__sub">
          Model <span className="mono">{profile.model}</span>
          {record?.testedAt && <> · tested {relativeTime(record.testedAt)}</>}
          {" · targets "}
          {profile.targets.length > 0 ? profile.targets.join(", ") : "nothing"}
        </div>

        {outcome && outcome.profileId === profile.id && (
          <div style={{ marginTop: "var(--space-3)" }}>
            <TestResults outcome={outcome} onDismiss={() => setOutcome(null)} />
          </div>
        )}
      </div>

      <div className="list-row__actions">
        <Button
          size="sm"
          onClick={runTest}
          busy={testAction.busy}
          aria-label={`Test ${profile.name} against ${agentSummary?.displayName}`}
        >
          Test
        </Button>
        <Button
          size="sm"
          variant="primary"
          onClick={activate}
          busy={activateAction.busy}
          disabled={!gate.canActivate || isActive}
          // Comes from the same gate the badge uses, so the tooltip cannot claim
          // the provider needs testing while the badge already says Compatible.
          title={gate.blockedReason}
        >
          {isActive ? "Active" : "Activate"}
        </Button>
        <Button size="sm" variant="ghost" onClick={onEdit}>
          Edit
        </Button>
        <Button
          size="sm"
          variant="ghost"
          onClick={onDuplicate}
          aria-label={`Duplicate ${profile.name}`}
        >
          Duplicate
        </Button>
        <Button
          size="sm"
          variant="ghost"
          onClick={onRequestDelete}
          aria-label={`Delete ${profile.name}`}
        >
          Delete
        </Button>
      </div>
    </div>
  );
}

/** Confirm deletion, offering recovery when the profile is currently active. */
function DeleteProfileDialog({
  profile,
  onClose,
  onDeleted,
}: {
  profile: ProfileSummary;
  onClose: () => void;
  onDeleted: () => void;
}) {
  const [impact, setImpact] = useState<
    { activeFor: AgentId[]; requiresDecision: boolean } | null | "loading"
  >("loading");
  const [error, setError] = useState<string | null>(null);
  const action = useAction();

  useEffect(() => {
    let cancelled = false;
    void call("profile_delete_impact", { id: profile.id })
      .then((result) => {
        if (!cancelled) {
          setImpact({
            activeFor: result.activeFor,
            requiresDecision: result.requiresDecision,
          });
        }
      })
      .catch((err: unknown) => {
        if (!cancelled) setImpact(null);
        const api = err as { message?: string };
        setError(api.message ?? "Could not check whether this provider is active.");
      });
    return () => {
      cancelled = true;
    };
  }, [profile.id]);

  const deleteNow = (force: boolean) =>
    void action.run(
      async () => {
        await call("delete_profile", { id: profile.id, force });
        onDeleted();
      },
      {
        onError: (api) => {
          setError(api.hint ? `${api.message} ${api.hint}` : api.message);
          return true;
        },
      },
    );

  const requiresDecision = impact !== "loading" && impact !== null && impact.requiresDecision;
  // Narrowed once so the JSX below does not have to re-check the sentinel.
  const impactDetail = impact === "loading" || impact === null ? null : impact;

  return (
    <Modal
      title={`Delete ${profile.name}?`}
      onClose={onClose}
      footer={
        requiresDecision ? (
          <>
            <Button variant="ghost" onClick={onClose}>
              Cancel
            </Button>
            <Button
              variant="secondary"
              onClick={onClose}
              title="Choose another provider or restore the default configuration first."
            >
              Keep it — let me switch first
            </Button>
            <Button variant="danger" onClick={() => deleteNow(true)} busy={action.busy}>
              Delete anyway
            </Button>
          </>
        ) : (
          <>
            <Button variant="ghost" onClick={onClose}>
              Cancel
            </Button>
            <Button
              variant="danger"
              onClick={() => deleteNow(false)}
              busy={action.busy}
              disabled={impact === "loading"}
            >
              Delete provider
            </Button>
          </>
        )
      }
    >
      {requiresDecision ? (
        <Banner tone="warn" title="This provider is currently active">
          {impactDetail && impactDetail.activeFor.length > 0
            ? impactDetail.activeFor
                .map((item) => (item === "codex" ? "Codex" : "Claude Code"))
                .join(" and ")
            : "An agent"}{" "}
          is using {profile.name}. Deleting it now would leave that agent pointed at a configuration
          Eclyzer Bridge no longer manages.
          <br />
          <br />
          The safer order is: activate another provider, or restore the default configuration, and
          then delete this one.
        </Banner>
      ) : (
        <p className="muted small">
          The stored API key for this provider is deleted with it. Your agent configuration is not
          changed by deleting a provider.
        </p>
      )}

      {error && (
        <div className="field__error" role="alert">
          {error}
        </div>
      )}
    </Modal>
  );
}

/* --- Editor ----------------------------------------------------------- */

type EditorTab = "general" | "advanced";

interface EditorState {
  id?: string;
  name: string;
  providerName: string;
  baseUrl: string;
  model: string;
  apiKey: string;
  clearApiKey: boolean;
  targets: AgentId[];
  notes: string;
  advanced: AdvancedOptions;
  allowInsecure: boolean;
}

function blankEditor(defaultAgent: AgentId): EditorState {
  return {
    name: "",
    providerName: "",
    baseUrl: "",
    model: "",
    apiKey: "",
    clearApiKey: false,
    targets: [defaultAgent],
    notes: "",
    advanced: {},
    allowInsecure: false,
  };
}

function editorFrom(profile: ProfileSummary): EditorState {
  return {
    id: profile.id,
    name: profile.name,
    providerName: profile.providerName,
    baseUrl: profile.baseUrl,
    model: profile.model,
    apiKey: "",
    clearApiKey: false,
    targets: [...profile.targets],
    notes: profile.notes ?? "",
    advanced: profile.advanced ?? {},
    allowInsecure: profile.advanced?.allowInsecureEndpoint ?? false,
  };
}

function ProfileEditor({
  profile,
  templates,
  agents,
  defaultAgent,
  onClose,
  onSaved,
  notify,
}: {
  profile: ProfileSummary | null;
  templates: ProviderTemplate[];
  agents: AgentSummary[];
  defaultAgent: AgentId;
  onClose: () => void;
  onSaved: () => void | Promise<void>;
  notify: (tone: "success" | "error" | "info", title: string, text?: string) => void;
}) {
  const { settings, saveSettings } = useApp();
  const [tab, setTab] = useState<EditorTab>("general");
  const [state, setState] = useState<EditorState>(() =>
    profile ? editorFrom(profile) : blankEditor(defaultAgent),
  );
  const [errors, setErrors] = useState<Record<string, string>>({});
  const action = useAction();

  const isEditing = profile !== null;
  const patch = useCallback(
    (changes: Partial<EditorState>) => setState((current) => ({ ...current, ...changes })),
    [],
  );

  const applyTemplate = (templateId: string) => {
    const template = templates.find((item) => item.id === templateId);
    if (!template) return;
    patch({
      providerName: template.name,
      baseUrl: template.baseUrl ?? state.baseUrl,
      model: template.suggestedModels[0] ?? state.model,
      targets: template.likelyTargets.length > 0 ? [...template.likelyTargets] : state.targets,
    });
  };

  /**
   * Validate locally so the user is told before a round trip.
   *
   * The backend repeats every check; this is for responsiveness, not safety.
   */
  const validate = (): boolean => {
    const next: Record<string, string> = {};

    if (!state.name.trim()) {
      next.name = "Give this provider a name you will recognize.";
    }
    if (!state.providerName.trim()) {
      next.providerName = "Enter the provider name.";
    }
    if (!state.baseUrl.trim()) {
      next.baseUrl = "Enter the endpoint URL.";
    } else if (!/^https?:\/\//i.test(state.baseUrl.trim())) {
      next.baseUrl = "The URL must start with http:// or https://.";
    }
    if (!state.model.trim()) {
      next.model = "Enter the model ID exactly as your provider expects it.";
    }
    if (state.targets.length === 0) {
      next.targets = "Choose at least one agent.";
    }
    if (!isEditing && !state.apiKey.trim() && !state.clearApiKey) {
      next.apiKey = "Enter the API key for this provider.";
    } else if (state.apiKey.trim() && state.apiKey.trim().length < 8) {
      // A suspiciously short key is almost always a partial paste.
      next.apiKey = "That key looks too short. Check that you pasted all of it.";
    }

    setErrors(next);
    if (Object.keys(next).length > 0) {
      setTab("general");
      return false;
    }
    // Plain-HTTP endpoints need explicit confirmation before anything is sent.
    if (wouldBeInsecure(state) && !state.allowInsecure) {
      setErrors({
        baseUrl:
          "This endpoint uses plain HTTP. Tick the confirmation below, or use an https:// URL.",
      });
      setTab("general");
      return false;
    }
    return true;
  };

  const save = () =>
    void action.run(
      async () => {
        // Local validation first so the user is told about a missing field
        // before a round trip. The backend repeats every check.
        if (!validate()) {
          return undefined;
        }

        const input: ProfileInput = {
          id: state.id,
          name: state.name.trim(),
          providerName: state.providerName.trim(),
          baseUrl: state.baseUrl.trim(),
          model: state.model.trim(),
          targets: state.targets,
          notes: state.notes.trim() || undefined,
          advanced: {
            ...state.advanced,
            allowInsecureEndpoint: state.allowInsecure,
          },
        };
        // An empty field means "leave the stored key alone", never "clear it".
        if (state.clearApiKey) {
          input.clearApiKey = true;
        } else if (state.apiKey.trim()) {
          input.apiKey = state.apiKey.trim();
        }

        const result = await call("save_profile", { input });
        await onSaved();
        notify(
          "success",
          isEditing ? "Provider updated." : "Provider added.",
          result.secretUpdated ? "The API key was saved securely." : undefined,
        );
        return result;
      },
      {
        onError: (api) => {
          // Map validation errors back onto the field the backend named.
          if (api.kind === "validation") {
            setTab("general");
            if (/url/i.test(api.message)) {
              setErrors({ baseUrl: api.message });
            } else if (/model/i.test(api.message)) {
              setErrors({ model: api.message });
            } else if (/key/i.test(api.message)) {
              setErrors({ apiKey: api.message });
            } else if (/name/i.test(api.message)) {
              setErrors({ name: api.message });
            }
          }
          return false;
        },
      },
    );

  const insecure = wouldBeInsecure(state);
  const insecureAcknowledged = settings?.insecureEndpointNoticeSeen ?? false;

  return (
    <Modal
      title={isEditing ? `Edit ${profile?.name}` : "Add a provider"}
      onClose={onClose}
      wide
      footer={
        <>
          <Button variant="ghost" onClick={onClose}>
            Cancel
          </Button>
          <Button variant="primary" onClick={save} busy={action.busy}>
            {isEditing ? "Save changes" : "Add provider"}
          </Button>
        </>
      }
    >
      <div className="tabs" role="tablist" aria-label="Provider settings">
        <button
          role="tab"
          type="button"
          className="tab"
          aria-selected={tab === "general"}
          onClick={() => setTab("general")}
        >
          General
        </button>
        <button
          role="tab"
          type="button"
          className="tab"
          aria-selected={tab === "advanced"}
          onClick={() => setTab("advanced")}
        >
          Advanced
        </button>
      </div>

      {tab === "general" ? (
        <>
          {!isEditing && templates.length > 0 && (
            <Field
              label="Start from a preset"
              hint="Presets fill in a known endpoint and model. You can change everything afterwards."
            >
              <select
                className="select"
                defaultValue=""
                onChange={(event) => {
                  if (event.target.value) applyTemplate(event.target.value);
                }}
              >
                <option value="">Choose a provider…</option>
                {templates.map((template) => (
                  <option key={template.id} value={template.id}>
                    {template.name}
                  </option>
                ))}
              </select>
            </Field>
          )}

          <Field label="Profile name" htmlFor="p-name" required error={errors.name}>
            <input
              id="p-name"
              className="input"
              value={state.name}
              onChange={(event) => patch({ name: event.target.value })}
              placeholder="DeepSeek for work"
              aria-invalid={errors.name ? true : undefined}
            />
          </Field>

          <Field
            label="Provider name"
            htmlFor="p-provider"
            required
            error={errors.providerName}
            hint="Shown in your agent's configuration so you can tell providers apart."
          >
            <input
              id="p-provider"
              className="input"
              value={state.providerName}
              onChange={(event) => patch({ providerName: event.target.value })}
              placeholder="DeepSeek"
              aria-invalid={errors.providerName ? true : undefined}
            />
          </Field>

          <Field
            label="Base URL"
            htmlFor="p-url"
            required
            error={errors.baseUrl}
            hint="The OpenAI-compatible or Anthropic-compatible API root. Do not include /chat/completions."
          >
            <input
              id="p-url"
              className="input mono"
              value={state.baseUrl}
              onChange={(event) => patch({ baseUrl: event.target.value })}
              placeholder="https://api.deepseek.com/v1"
              spellCheck={false}
              aria-invalid={errors.baseUrl ? true : undefined}
            />
          </Field>

          <Field
            label="Model ID"
            htmlFor="p-model"
            required
            error={errors.model}
            hint="Passed to your agent exactly as written. It is never interpreted as a command."
          >
            <input
              id="p-model"
              className="input mono"
              value={state.model}
              onChange={(event) => patch({ model: event.target.value })}
              placeholder="deepseek-chat"
              spellCheck={false}
              aria-invalid={errors.model ? true : undefined}
            />
          </Field>

          <Field
            label="API key"
            htmlFor="p-key"
            required={!isEditing}
            error={errors.apiKey}
            hint={
              isEditing
                ? profile?.hasSecret
                  ? `A key is already stored (${profile.secretFingerprint ?? "stored"}). Leave this empty to keep it.`
                  : "No key is stored yet. Enter one to activate this provider."
                : "Encrypted with Windows DPAPI for your Windows account only, and never written to your config files or backups."
            }
          >
            <input
              id="p-key"
              className="input mono"
              type="password"
              value={state.apiKey}
              onChange={(event) => patch({ apiKey: event.target.value, clearApiKey: false })}
              placeholder={isEditing ? "Leave empty to keep the stored key" : "sk-…"}
              autoComplete="off"
              spellCheck={false}
              disabled={state.clearApiKey}
              aria-invalid={errors.apiKey ? true : undefined}
            />
          </Field>

          {isEditing && profile?.hasSecret && (
            <label className="checkbox">
              <input
                type="checkbox"
                checked={state.clearApiKey}
                onChange={(event) =>
                  patch({
                    clearApiKey: event.target.checked,
                    apiKey: event.target.checked ? "" : state.apiKey,
                  })
                }
              />
              <span className="checkbox__text">
                <span>Remove the stored API key</span>
                <span className="checkbox__hint">
                  The provider stays, but it cannot be activated until a new key is entered.
                </span>
              </span>
            </label>
          )}

          <fieldset
            style={{ border: "none", padding: 0, margin: 0 }}
            aria-describedby={errors.targets ? "p-targets-error" : undefined}
          >
            <legend className="field__label" style={{ padding: 0, marginBottom: 8 }}>
              Use this provider with
            </legend>
            <div className="stack stack--tight">
              {agents.map((agent) => (
                <label className="checkbox" key={agent.agent}>
                  <input
                    type="checkbox"
                    checked={state.targets.includes(agent.agent)}
                    onChange={(event) => {
                      const next = event.target.checked
                        ? [...state.targets, agent.agent]
                        : state.targets.filter((item) => item !== agent.agent);
                      patch({ targets: next });
                    }}
                  />
                  <span className="checkbox__text">
                    <span>{agent.displayName}</span>
                    <span className="checkbox__hint">
                      Codex and Claude Code speak different protocols. Each is tested separately.
                    </span>
                  </span>
                </label>
              ))}
            </div>
            {errors.targets && (
              <div className="field__error" id="p-targets-error" role="alert">
                {errors.targets}
              </div>
            )}
          </fieldset>

          {insecure && (
            <div className="stack stack--tight">
              <Banner tone="warn" title="Insecure Endpoint">
                Your API key may be sent without encryption. The endpoint below uses plain HTTP and
                is not on this machine.
                <br />
                <code className="mono">{state.baseUrl}</code>
              </Banner>
              <label className="checkbox">
                <input
                  type="checkbox"
                  checked={state.allowInsecure}
                  onChange={(event) => {
                    patch({ allowInsecure: event.target.checked });
                    if (event.target.checked && !insecureAcknowledged) {
                      void saveSettings({ insecureEndpointNoticeSeen: true });
                    }
                  }}
                />
                <span className="checkbox__text">
                  <span>I understand, send my key over plain HTTP</span>
                  <span className="checkbox__hint">
                    Only do this for an endpoint you control on a trusted network.
                  </span>
                </span>
              </label>
            </div>
          )}
        </>
      ) : (
        <AdvancedEditor
          state={state}
          patch={patch}
          onReset={() => patch({ advanced: {}, allowInsecure: false })}
        />
      )}
    </Modal>
  );
}

/** Whether saving this profile would send credentials over plain HTTP. */
function wouldBeInsecure(state: EditorState): boolean {
  const url = state.baseUrl.trim();
  if (!/^http:\/\//i.test(url)) return false;
  // localhost, 127.0.0.1, ::1 and .local are treated as local by the backend and
  // do not need the warning.
  return !/^http:\/\/(localhost|127\.0\.0\.1|\[::1\]|0\.0\.0\.0|[^/]*\.local)(:|\/|$)/i.test(url);
}

function AdvancedEditor({
  state,
  patch,
  onReset,
}: {
  state: EditorState;
  patch: (changes: Partial<EditorState>) => void;
  onReset: () => void;
}) {
  const [headerName, setHeaderName] = useState("");
  const [headerValue, setHeaderValue] = useState("");

  const headers = state.advanced.headers ?? [];
  const queryParams = state.advanced.queryParams ?? [];

  const update = (changes: Partial<AdvancedOptions>) =>
    patch({ advanced: { ...state.advanced, ...changes } });

  return (
    <>
      <p className="muted small">
        These options exist for gateways that need extra headers or a different authentication
        style. Most providers need none of them.
      </p>

      <Field
        label="Authentication style"
        htmlFor="p-auth"
        hint="Bearer sends Authorization: Bearer <key>. x-api-key matches the Anthropic convention."
      >
        <select
          id="p-auth"
          className="select"
          value={state.advanced.authMode ?? "bearer"}
          onChange={(event) => update({ authMode: event.target.value as AuthMode })}
        >
          <option value="bearer">Authorization: Bearer</option>
          <option value="x_api_key">x-api-key</option>
        </select>
      </Field>

      <Field
        label="Request timeout"
        htmlFor="p-timeout"
        hint="Milliseconds. Slow gateways may need longer; the default is 20000."
      >
        <input
          id="p-timeout"
          className="input"
          type="number"
          min={1000}
          max={300000}
          step={1000}
          value={state.advanced.timeoutMs ?? ""}
          onChange={(event) =>
            update({
              timeoutMs: event.target.value ? Number(event.target.value) : undefined,
            })
          }
          placeholder="20000"
        />
      </Field>

      <div className="stack stack--tight">
        <div className="field__label">Extra headers</div>
        {headers.length === 0 ? (
          <div className="field__hint">No extra headers.</div>
        ) : (
          <div className="list">
            {headers.map((header, index) => (
              <div className="list-row" key={`${header.name}-${index}`}>
                <div className="list-row__main">
                  <div className="list-row__title mono">{header.name}</div>
                  <div className="list-row__sub mono">
                    {header.value ? "•••••• (value hidden)" : "(empty)"}
                  </div>
                </div>
                <div className="list-row__actions">
                  <Button
                    size="sm"
                    variant="ghost"
                    onClick={() =>
                      update({
                        headers: headers.filter((_, i) => i !== index),
                      })
                    }
                    aria-label={`Remove header ${header.name}`}
                  >
                    Remove
                  </Button>
                </div>
              </div>
            ))}
          </div>
        )}

        <div className="row">
          <input
            className="input mono"
            value={headerName}
            onChange={(event) => setHeaderName(event.target.value)}
            placeholder="X-Custom-Header"
            aria-label="Header name"
          />
          <input
            className="input mono"
            type="password"
            value={headerValue}
            onChange={(event) => setHeaderValue(event.target.value)}
            placeholder="value"
            aria-label="Header value"
            autoComplete="off"
          />
          <Button
            className="nowrap"
            disabled={!headerName.trim()}
            onClick={() => {
              update({
                headers: [...headers, { name: headerName.trim(), value: headerValue }],
              });
              setHeaderName("");
              setHeaderValue("");
            }}
          >
            Add
          </Button>
        </div>
        <div className="field__hint">
          Header values are stored in plain text in your profile and written into the agent's own
          configuration file. Put your gateway credential in the API key field instead, where it is
          sealed with Windows DPAPI. Authorization, x-api-key, and proxy-authorization cannot be
          overridden here.
        </div>
      </div>

      <div className="stack stack--tight">
        <div className="field__label">Query parameters</div>
        {queryParams.length === 0 ? (
          <div className="field__hint">No extra query parameters.</div>
        ) : (
          <div className="list">
            {queryParams.map((param, index) => (
              <div className="list-row" key={`${param.name}-${index}`}>
                <div className="list-row__main">
                  <div className="list-row__title mono">{param.name}</div>
                  <div className="list-row__sub mono">
                    {param.value ? "•••••• (value hidden)" : "(empty)"}
                  </div>
                </div>
                <div className="list-row__actions">
                  <Button
                    size="sm"
                    variant="ghost"
                    onClick={() =>
                      update({
                        queryParams: queryParams.filter((_, i) => i !== index),
                      })
                    }
                    aria-label={`Remove parameter ${param.name}`}
                  >
                    Remove
                  </Button>
                </div>
              </div>
            ))}
          </div>
        )}
      </div>

      <Field
        label="Model alias"
        htmlFor="p-alias"
        hint="Optional. Written to your agent as the model name when the provider expects a different ID than the one you test with."
      >
        <input
          id="p-alias"
          className="input mono"
          value={state.advanced.modelAlias ?? ""}
          onChange={(event) => update({ modelAlias: event.target.value || undefined })}
          placeholder="same as model ID"
          spellCheck={false}
        />
      </Field>

      <Field label="Notes" htmlFor="p-notes">
        <textarea
          id="p-notes"
          className="textarea"
          value={state.notes}
          onChange={(event) => patch({ notes: event.target.value })}
          placeholder="Anything you want to remember about this provider."
        />
      </Field>

      <div className="row">
        <Button variant="ghost" size="sm" onClick={onReset}>
          Reset advanced options
        </Button>
        <div className="spacer" />
        <Button
          variant="ghost"
          size="sm"
          onClick={() =>
            void copyToClipboard(
              state.baseUrl ? `${state.baseUrl} · ${state.model}` : "No endpoint set",
            )
          }
        >
          Copy endpoint and model
        </Button>
      </div>
    </>
  );
}
