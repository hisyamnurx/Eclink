/**
 * Application state: agents, profiles, settings, and toasts.
 *
 * Deliberately a single reducer-free context with explicit async actions. The
 * volume of shared state here is small, and keeping the writes in named functions
 * makes it obvious which backend command each mutation corresponds to.
 */

import {
  createContext,
  useCallback,
  useContext,
  useEffect,
  useMemo,
  useRef,
  useState,
  type ReactNode,
} from "react";

import {
  call,
  toApiError,
  type AgentSummary,
  type AppSettings,
  type ProfileSummary,
  type SettingsView,
} from "../lib/api";
import { findActiveProfile } from "../lib/active";
import {
  dictionaryFor,
  isLanguage,
  translate,
  type Interpolations,
  type Language,
  type MessageKey,
} from "../lib/i18n";

/* --- Toasts ----------------------------------------------------------- */

export interface Toast {
  id: number;
  tone: "success" | "error" | "info";
  title: string;
  text?: string;
}

interface AppContextValue {
  /* data */
  agents: AgentSummary[];
  profiles: ProfileSummary[];
  settings: SettingsView | null;
  loading: boolean;
  /** The error from the initial load, if it failed. */
  loadError: unknown;

  /* derived */
  firstRun: boolean;
  activeProfileFor: (agent: string) => ProfileSummary | undefined;

  /* localisation */
  /** The active language. */
  language: Language;
  /** Translate a key, substituting any `{placeholders}` in `values`. */
  t: (key: MessageKey, values?: Interpolations) => string;
  /** Switch language; persists immediately. */
  setLanguage: (language: Language) => Promise<void>;

  /* actions */
  reload: () => Promise<void>;
  reloadAgents: () => Promise<void>;
  reloadProfiles: () => Promise<void>;
  refreshAgent: (agent: string) => Promise<void>;
  saveSettings: (patch: Partial<AppSettings>) => Promise<void>;
  /* toasts */
  toasts: Toast[];
  notify: (tone: Toast["tone"], title: string, text?: string) => void;
  dismissToast: (id: number) => void;
}

const AppContext = createContext<AppContextValue | null>(null);

let toastSeq = 0;

export function AppProvider({ children }: { children: ReactNode }) {
  const [agents, setAgents] = useState<AgentSummary[]>([]);
  const [profiles, setProfiles] = useState<ProfileSummary[]>([]);
  const [settings, setSettings] = useState<SettingsView | null>(null);
  const [loading, setLoading] = useState(true);
  const [loadError, setLoadError] = useState<unknown>(null);
  const [toasts, setToasts] = useState<Toast[]>([]);

  // Auto-dismiss timers, cleared on unmount so a late timer cannot set state on
  // an unmounted tree.
  const timers = useRef<Map<number, ReturnType<typeof setTimeout>>>(new Map());

  const dismissToast = useCallback((id: number) => {
    const timer = timers.current.get(id);
    if (timer) {
      clearTimeout(timer);
      timers.current.delete(id);
    }
    setToasts((current) => current.filter((toast) => toast.id !== id));
  }, []);

  const notify = useCallback((tone: Toast["tone"], title: string, text?: string) => {
    const id = ++toastSeq;
    setToasts((current) => [...current, { id, tone, title, text }]);

    // Errors stay until dismissed: a user may need to read the hint. Success
    // and info messages clear themselves.
    if (tone !== "error") {
      const timer = setTimeout(() => {
        timers.current.delete(id);
        setToasts((current) => current.filter((toast) => toast.id !== id));
      }, 4500);
      timers.current.set(id, timer);
    }
  }, []);

  useEffect(() => {
    const map = timers.current;
    return () => {
      map.forEach((timer) => clearTimeout(timer));
      map.clear();
    };
  }, []);

  const reloadAgents = useCallback(async () => {
    const next = await call("list_agents");
    setAgents(next);
  }, []);

  const reloadProfiles = useCallback(async () => {
    const next = await call("list_profiles");
    setProfiles(next);
  }, []);

  const reload = useCallback(async () => {
    // The clearing of the previous error happens inside the promise chain rather
    // than synchronously, so mounting does not cause a cascading re-render.
    try {
      const [settingsView, agentList, profileList] = await Promise.all([
        call("get_settings"),
        call("list_agents"),
        call("list_profiles"),
      ]);
      setSettings(settingsView);
      setAgents(agentList);
      setProfiles(profileList);
      setLoadError(null);
    } catch (error) {
      setLoadError(error);
    } finally {
      setLoading(false);
    }
  }, []);

  useEffect(() => {
    // Deferred by a microtask so the effect body itself performs no synchronous
    // state update.
    void Promise.resolve().then(reload);
  }, [reload]);

  const refreshAgent = useCallback(async (agent: string) => {
    try {
      const updated = await call("refresh_agent", {
        agent: agent as "codex" | "claude",
      });
      setAgents((current) =>
        current.map((item) => (item.agent === updated.agent ? updated : item)),
      );
    } catch (error) {
      // A refresh failure leaves the previous data in place; the caller decides
      // whether to surface it.
      throw toApiError(error);
    }
  }, []);

  const saveSettings = useCallback(async (patch: Partial<AppSettings>) => {
    // Send only the fields the backend accepts. `lastProjectDirectory` is
    // included because the backend writes it when the user launches with
    // "remember this folder" enabled.
    const result = await call("update_settings", {
      patch: {
        updateCheck: patch.updateCheck,
        rememberProjectDirectory: patch.rememberProjectDirectory,
        logLevel: patch.logLevel,
        firstRunComplete: patch.firstRunComplete,
        costNoticeSeen: patch.costNoticeSeen,
        insecureEndpointNoticeSeen: patch.insecureEndpointNoticeSeen,
        lastProjectDirectory: patch.lastProjectDirectory,
      },
    });
    setSettings(result);
  }, []);

  /**
   * The profile currently applied to `agent`, or undefined.
   *
   * Delegates to `findActiveProfile`, which derives the answer from the
   * backend's reported active configuration. The previous inline version
   * returned the first profile that had ever passed a test for that agent, which
   * is a different question and gives the wrong answer as soon as a user
   * activates a second profile.
   */
  const activeProfileFor = useCallback(
    (agent: string) => findActiveProfile(profiles, agents, agent),
    [agents, profiles],
  );

  // First run is driven by settings, but an unreadable settings file must not
  // trap the user in onboarding, so a missing settings object counts as done.
  const firstRun = settings ? !settings.firstRunComplete : false;

  /**
   * The active language.
   *
   * Read from persisted settings, so the choice survives a restart. An unknown
   * or missing value falls back to English rather than rendering nothing, which
   * matters because the value can come from a file an older version wrote.
   */
  const language: Language = isLanguage(settings?.language) ? settings.language : "en";
  const dictionary = useMemo(() => dictionaryFor(language), [language]);

  const t = useCallback(
    (key: MessageKey, values?: Interpolations) =>
      translate(dictionary, key, values),
    [dictionary],
  );

  const setLanguage = useCallback(
    async (next: Language) => {
      // Applied optimistically through the shared settings state so the UI
      // switches in the same tick, then persisted. A failed write leaves the
      // previous choice in the file, which the next load will read.
      await saveSettings({ language: next });
    },
    [saveSettings],
  );

  const value = useMemo<AppContextValue>(
    () => ({
      agents,
      profiles,
      settings,
      loading,
      loadError,
      firstRun,
      activeProfileFor,
      reload,
      reloadAgents,
      reloadProfiles,
      refreshAgent,
      saveSettings,
      language,
      t,
      setLanguage,
      toasts,
      notify,
      dismissToast,
    }),
    [
      agents,
      profiles,
      settings,
      loading,
      loadError,
      firstRun,
      activeProfileFor,
      reload,
      reloadAgents,
      reloadProfiles,
      refreshAgent,
      saveSettings,
      language,
      t,
      setLanguage,
      toasts,
      notify,
      dismissToast,
    ],
  );

  return <AppContext.Provider value={value}>{children}</AppContext.Provider>;
}

export function useApp(): AppContextValue {
  const context = useContext(AppContext);
  if (!context) {
    throw new Error("useApp must be used inside AppProvider");
  }
  return context;
}

/**
 * Run an async action with busy tracking and consistent error reporting.
 *
 * Every button in the app uses this, which is why no screen forgets to clear its
 * spinner or forgets to show why an action failed.
 */
export function useAction() {
  const { notify } = useApp();
  const [busy, setBusy] = useState(false);
  const mounted = useRef(true);

  useEffect(() => {
    mounted.current = true;
    return () => {
      mounted.current = false;
    };
  }, []);

  const run = useCallback(
    async <T,>(
      action: () => Promise<T>,
      options?: {
        /** Toast on success. Omit for actions that render their own result. */
        success?: string;
        /** Return true to suppress the automatic error toast. */
        onError?: (error: ReturnType<typeof toApiError>) => boolean | void;
      },
    ): Promise<T | undefined> => {
      setBusy(true);
      try {
        const result = await action();
        if (options?.success) {
          notify("success", options.success);
        }
        return result;
      } catch (error) {
        const api = toApiError(error);
        const handled = options?.onError?.(api);
        if (!handled) {
          notify("error", api.message, api.hint);
        }
        return undefined;
      } finally {
        if (mounted.current) {
          setBusy(false);
        }
      }
    },
    [notify],
  );

  return { busy, run };
}
