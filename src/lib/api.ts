/**
 * The single boundary between the UI and the Rust backend.
 *
 * Every call here maps to one `#[tauri::command]`. There is deliberately no
 * generic `invoke` escape hatch exported from this module: if a screen needs new
 * backend behaviour, it gets a named command, which keeps the IPC surface
 * auditable.
 *
 * The invoke function is injected rather than imported at module scope so the
 * whole API layer can be exercised in tests without a Tauri runtime.
 */

/** A command error as the backend reports it. */
export interface CommandError {
  kind: ErrorKind;
  message: string;
  hint?: string;
  details?: string;
}

export type ErrorKind =
  | "validation"
  | "io"
  | "config_invalid"
  | "config_conflict"
  | "secret"
  | "network"
  | "auth"
  | "model_not_found"
  | "incompatible"
  | "rate_limited"
  | "provider_error"
  | "agent_unavailable"
  | "managed_policy"
  | "locked"
  | "update"
  | "internal";

export type AgentId = "codex" | "claude";
export type ConfigParseState = "missing" | "valid" | "invalid";
export type ActiveKind = "default" | "managed" | "external";
export type AuthMode = "bearer" | "x_api_key";

export type CompatibilityStatus =
  | "untested"
  | "testing"
  | "compatible"
  | "partially_compatible"
  | "incompatible"
  | "unknown"
  | "test_failed";

export interface AgentSummary {
  agent: AgentId;
  displayName: string;
  installed: boolean;
  version?: string;
  executable?: string;
  configPath?: string;
  configState: ConfigParseState;
  note?: string;
  installGuideUrl?: string;
  activeLabel: string;
  activeKind: ActiveKind;
  activeProfileName?: string;
  configWritable: boolean;
}

export interface HeaderPair {
  name: string;
  value: string;
}

export interface AdvancedOptions {
  headers?: HeaderPair[];
  queryParams?: HeaderPair[];
  timeoutMs?: number;
  authMode?: AuthMode;
  allowInsecureEndpoint?: boolean;
  modelAlias?: string;
}

export interface CompatibilityRecord {
  status: CompatibilityStatus;
  testedAt?: string;
  latencyMs?: number;
  streaming?: boolean;
  toolCalling?: boolean;
  reasonCode?: string;
  reason?: string;
  warning?: string;
  staleKey?: string;
}

export interface Profile {
  id: string;
  name: string;
  providerName: string;
  baseUrl: string;
  model: string;
  secretRef?: string;
  targets: AgentId[];
  advanced?: AdvancedOptions;
  compatibility?: Partial<Record<AgentId, CompatibilityRecord>>;
  notes?: string;
  createdAt: string;
  updatedAt: string;
}

export interface ProfileSummary extends Profile {
  hasSecret: boolean;
  secretFingerprint?: string;
  ready: boolean;
}

export interface ProfileInput {
  id?: string;
  name: string;
  providerName: string;
  baseUrl: string;
  model: string;
  apiKey?: string;
  clearApiKey?: boolean;
  targets: AgentId[];
  advanced?: AdvancedOptions;
  notes?: string;
}

export interface SaveProfileResult {
  profile: ProfileSummary;
  secretUpdated: boolean;
}

export interface ProviderTemplate {
  id: string;
  name: string;
  baseUrl?: string;
  suggestedModels: string[];
  likelyTargets: AgentId[];
  note?: string;
  docsUrl?: string;
}

export type TestStepState = "passed" | "warning" | "failed" | "skipped";

export interface TestStep {
  id: string;
  label: string;
  state: TestStepState;
  detail?: string;
  latencyMs?: number;
}

export interface TestOutcome {
  profileId: string;
  agent: AgentId;
  status: CompatibilityStatus;
  reasonCode: string;
  reason: string;
  warning?: string;
  steps: TestStep[];
  latencyMs?: number;
  streaming: boolean;
  toolCalling: boolean;
}

export interface ApplyOutcome {
  agent: AgentId;
  configPath: string;
  snapshotId: string;
  createdOriginalBackup: boolean;
  managedKeys: string[];
  active: {
    kind: ActiveKind;
    profileId?: string;
    profileName?: string;
    label: string;
    providerId?: string;
    model?: string;
  };
}

export type CredentialMode = "credential_helper" | "settings_file" | "environment_variable";

/**
 * What a stock reset would remove.
 *
 * Computed by the backend from the same key lists the reset itself uses, so the
 * preview cannot drift from the operation. Keys are shown with their current
 * values so the user can recognise what is about to go, and the values are
 * already redacted for anything secret.
 */
export interface StockResetPreview {
  envKeys: Array<[string, string]>;
  aliasKeys: Array<[string, string]>;
  rootKeys: Array<[string, string]>;
  /** Managed settings files that make a reset unsafe, if any. */
  blockedByManaged?: string[];
}

export interface ActivationResult extends ApplyOutcome {
  message: string;
  credentialMode: CredentialMode;
}

export interface LaunchPlan {
  agent: AgentId;
  program: string;
  args: string[];
  workingDir: string;
  display: string;
}

export interface LaunchPreview {
  plan: LaunchPlan;
  envNames: string[];
  display: string;
}

export interface SnapshotView {
  id: string;
  agent: string;
  label: string;
  reason: string;
  createdAt: string;
  profileId?: string;
  bytes: number;
}

export interface HistoryEntry {
  id: string;
  agent: string;
  createdAt: string;
  label: string;
  reason: string;
  profileId?: string;
  profileName?: string;
  snapshotId?: string;
  success: boolean;
  error?: string;
}

export interface RecoveryStatus {
  agent: string;
  hasOriginal: boolean;
  snapshotCount: number;
  hasManagedConfiguration: boolean;
  canUndo: boolean;
  lastChangeLabel?: string;
  externalConfiguration: boolean;
}

export interface DeleteImpact {
  profileName: string;
  activeFor: AgentId[];
  requiresDecision: boolean;
}

export type LogLevel = "error" | "warn" | "info" | "debug" | "trace";

export interface AppSettings {
  schemaVersion: number;
  updateCheck: boolean;
  lastUpdateCheck?: string;
  lastProjectDirectory?: string;
  rememberProjectDirectory: boolean;
  logLevel: LogLevel;
  firstRunComplete: boolean;
  costNoticeSeen: boolean;
  insecureEndpointNoticeSeen: boolean;
}

export interface SettingsView extends AppSettings {
  appVersion: string;
  dataDirectory: string;
  logDirectory: string;
  repositoryUrl: string;
  secretBackend: string;
  secretCount: number;
  profileCount: number;
  credentialHelperAvailable: boolean;
}

export interface SettingsPatch {
  updateCheck?: boolean;
  rememberProjectDirectory?: boolean;
  logLevel?: LogLevel;
  firstRunComplete?: boolean;
  costNoticeSeen?: boolean;
  insecureEndpointNoticeSeen?: boolean;
  /** Written only as a side effect of launching with "remember" enabled. */
  lastProjectDirectory?: string;
}

export interface DiagnosticsBundle {
  text: string;
  appVersion: string;
  dataDirectory: string;
  logDirectory: string;
}

export interface AgentDiagnostics {
  agent: AgentId;
  displayName: string;
  installed: boolean;
  version?: string;
  configPath?: string;
  configState: ConfigParseState;
  activeProfileId?: string;
  backupsPresent: boolean;
  originalBackupPresent: boolean;
  snapshotCount: number;
}

export interface UpdateStatus {
  checked: boolean;
  available: boolean;
  version?: string;
  notes?: string;
  currentVersion: string;
  error?: string;
}

export interface CostNotice {
  message: string;
  acknowledged: boolean;
}

/** A single-turn playground reply. */
export interface PlaygroundResult {
  text: string;
  latencyMs: number;
  model: string;
  /** Always false: the playground sends a buffered, non-streaming request. */
  streaming: boolean;
  promptTokens?: number;
  completionTokens?: number;
}

/** Commands that can be invoked. Kept explicit so typos fail at compile time. */
export interface Commands {
  list_agents: { args: Record<string, never>; result: AgentSummary[] };
  refresh_agent: { args: { agent: AgentId }; result: AgentSummary };
  get_diagnostics: { args: Record<string, never>; result: AgentDiagnostics[] };

  list_profiles: { args: Record<string, never>; result: ProfileSummary[] };
  get_profile: { args: { id: string }; result: ProfileSummary };
  save_profile: { args: { input: ProfileInput }; result: SaveProfileResult };
  /**
   * Copy a profile, including its stored key.
   *
   * The key never passes through the frontend: the backend moves it between
   * sealed records itself, so a duplicate does not require the user to re-enter
   * a credential they cannot read back.
   */
  duplicate_profile: { args: { id: string }; result: ProfileSummary };
  delete_profile: { args: { id: string; force: boolean }; result: null };
  profile_delete_impact: { args: { id: string }; result: DeleteImpact };
  list_provider_templates: { args: Record<string, never>; result: ProviderTemplate[] };
  test_profile_connection: {
    args: { id: string; agent: AgentId };
    result: TestOutcome;
  };
  clear_compatibility: { args: { id: string; agent: AgentId }; result: ProfileSummary };
  get_cost_notice: { args: Record<string, never>; result: CostNotice };
  playground_send: {
    args: { id: string; prompt: string; agent?: AgentId };
    result: PlaygroundResult;
  };

  activate_profile: {
    args: { id: string; agent: AgentId };
    result: ActivationResult;
  };
  preview_launch: {
    args: { agent: AgentId; projectDirectory: string };
    result: LaunchPreview;
  };
  launch_agent: {
    args: { agent: AgentId; projectDirectory: string };
    result: number;
  };

  restore_default: { args: { agent: AgentId }; result: ApplyOutcome };
  /**
   * Report what a stock reset would remove, without changing anything.
   *
   * `null` for an agent with no preview available, so the UI must show the
   * generic warning rather than an empty list it might mistake for "nothing".
   */
  preview_stock_reset: {
    args: { agent: AgentId };
    result: StockResetPreview | null;
  };
  /**
   * Remove every provider and model override, including ones the user created.
   *
   * Stronger than `restore_default`, which only removes what Eclyzer Bridge can
   * prove it owns. Callers must confirm first.
   */
  reset_to_stock_defaults: { args: { agent: AgentId }; result: ApplyOutcome };
  restore_original: { args: { agent: AgentId }; result: ApplyOutcome };
  restore_snapshot: {
    args: { agent: AgentId; snapshotId: string };
    result: ApplyOutcome;
  };
  undo_last_change: { args: { agent: AgentId }; result: ApplyOutcome };
  list_snapshots: { args: { agent?: AgentId }; result: SnapshotView[] };
  list_history: {
    args: { agent?: AgentId; limit?: number };
    result: HistoryEntry[];
  };
  recovery_status: { args: { agent: AgentId }; result: RecoveryStatus };

  get_settings: { args: Record<string, never>; result: SettingsView };
  update_settings: { args: { patch: SettingsPatch }; result: SettingsView };
  build_diagnostics_report: { args: Record<string, never>; result: DiagnosticsBundle };
  read_logs: { args: { maxLines?: number }; result: string[] };
  clear_logs: { args: Record<string, never>; result: null };
  reveal_data_directory: { args: Record<string, never>; result: null };
  reveal_log_directory: { args: Record<string, never>; result: null };

  check_for_updates: { args: Record<string, never>; result: UpdateStatus };
  check_for_updates_on_startup: {
    args: Record<string, never>;
    result: UpdateStatus | null;
  };
  install_update: { args: Record<string, never>; result: UpdateStatus };
}

export type CommandName = keyof Commands;

/**
 * The invoke function shape we depend on.
 *
 * Tauri's own `invoke` converts a rejected promise into an object with the
 * serialized error payload, which may arrive as a string. Normalising here means
 * no component ever has to parse an error itself.
 */
export type InvokeFn = (command: string, args?: Record<string, unknown>) => Promise<unknown>;

let invokeImpl: InvokeFn | null = null;

/**
 * Supply the real invoke function. Called once from the app entry point so tests
 * can install a stub instead.
 */
export function setInvoke(fn: InvokeFn): void {
  invokeImpl = fn;
}

/** Whether a backend is available at all (false in a plain browser). */
export function hasBackend(): boolean {
  return invokeImpl !== null;
}

/** A normalised error carrying the backend's actionable copy. */
export class ApiError extends Error {
  readonly kind: ErrorKind;
  readonly hint?: string;
  readonly details?: string;

  constructor(error: CommandError) {
    super(error.message);
    this.name = "ApiError";
    this.kind = error.kind;
    this.hint = error.hint;
    this.details = error.details;
  }

  /** The full message a user should see, including the hint. */
  get fullMessage(): string {
    return this.hint ? `${this.message} ${this.hint}` : this.message;
  }
}

/** Turn anything thrown by invoke into an ApiError. */
export function toApiError(raw: unknown): ApiError {
  if (raw instanceof ApiError) {
    return raw;
  }

  // Tauri serializes a `Result::Err` payload as-is, but a transport-level
  // failure arrives as a bare string.
  if (typeof raw === "string") {
    // An empty string is not a message worth showing; fall through to the
    // generic copy rather than rendering a blank dialog.
    if (raw.trim()) {
      return new ApiError({ kind: "internal", message: raw });
    }
    return new ApiError({
      kind: "internal",
      message: "Something went wrong in Eclyzer Bridge.",
    });
  }

  if (raw && typeof raw === "object") {
    const candidate = raw as Partial<CommandError>;
    if (typeof candidate.message === "string" && candidate.message.trim()) {
      return new ApiError({
        kind: candidate.kind ?? "internal",
        message: candidate.message,
        hint: candidate.hint,
        details: candidate.details,
      });
    }
    // A thrown Error from the bridge itself. `String(error)` is used rather than
    // `error.message` because a non-Error object may still stringify usefully.
    if (raw instanceof Error && raw.message.trim()) {
      return new ApiError({ kind: "internal", message: raw.message });
    }
  }

  return new ApiError({
    kind: "internal",
    message: "Something went wrong in Eclyzer Bridge.",
  });
}

/** Invoke a backend command with full type safety. */
export async function call<K extends CommandName>(
  command: K,
  ...args: Commands[K]["args"] extends Record<string, never>
    ? [] | [Record<string, never>]
    : [Commands[K]["args"]]
): Promise<Commands[K]["result"]> {
  if (!invokeImpl) {
    throw new ApiError({
      kind: "internal",
      message:
        "Eclyzer Bridge is not connected to its backend. Open the desktop app instead of a browser tab.",
    });
  }

  try {
    const payload = (args[0] ?? {}) as Record<string, unknown>;
    const result = await invokeImpl(command, payload);
    return result as Commands[K]["result"];
  } catch (raw) {
    throw toApiError(raw);
  }
}
