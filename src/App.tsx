/**
 * Application shell: navigation, routing, and the update check on startup.
 *
 * Routing is a small switch over a page name rather than a router library: there
 * are five screens, and adding a dependency for that would be more code than the
 * router replaces.
 */

import { useCallback, useEffect, useState } from "react";

import { Toasts } from "./components/Toasts";
import { Banner, Button, ErrorState, LoadingState } from "./components/ui";
import { call, hasBackend, type AgentId, type UpdateStatus } from "./lib/api";
import { Configure } from "./pages/Configure";
import { History } from "./pages/History";
import { Home } from "./pages/Home";
import { Playground } from "./pages/Playground";
import { Settings } from "./pages/Settings";
import { AppProvider, useApp } from "./state/AppContext";

type Page = "home" | "configure" | "playground" | "history" | "settings";

const NAV: Array<{ id: Page; label: string; icon: string }> = [
  { id: "home", label: "Overview", icon: "◈" },
  { id: "configure", label: "Providers", icon: "⚙" },
  { id: "playground", label: "Playground", icon: "▶" },
  { id: "history", label: "History", icon: "↺" },
  { id: "settings", label: "Settings", icon: "☰" },
];

export function App() {
  return (
    <AppProvider>
      <Shell />
      <Toasts />
    </AppProvider>
  );
}

function Shell() {
  const { loading, loadError, reload, settings } = useApp();
  const [page, setPage] = useState<Page>("home");
  // Set when the user navigates from a card that targets one agent, so the
  // destination opens on the right tab.
  const [agentHint, setAgentHint] = useState<AgentId | undefined>(undefined);

  const go = useCallback((next: Page, agent?: AgentId) => {
    setPage(next);
    setAgentHint(agent);
    // Move focus to the main region so a keyboard or screen-reader user lands in
    // the new content rather than at the top of the document.
    requestAnimationFrame(() => {
      document.getElementById("main-region")?.focus();
    });
  }, []);

  if (!hasBackend()) {
    return (
      <div className="main">
        <div className="main__inner">
          <ErrorState
            error={
              new Error(
                "Eclyzer Bridge is not connected to its backend. Open the desktop app rather than this page in a browser.",
              )
            }
          />
        </div>
      </div>
    );
  }

  if (loading) {
    return (
      <div className="main">
        <div className="main__inner">
          <LoadingState label="Starting Eclyzer Bridge…" />
        </div>
      </div>
    );
  }

  if (loadError) {
    return (
      <div className="main">
        <div className="main__inner">
          <ErrorState error={loadError} onRetry={() => void reload()} />
        </div>
      </div>
    );
  }

  return (
    <div className="app">
      <nav className="sidebar" aria-label="Main">
        <div className="sidebar__brand">
          <div className="sidebar__wordmark">Eclyzer Bridge</div>
          <div className="sidebar__tagline">Local configuration bridge</div>
        </div>

        <div className="nav">
          {NAV.map((item) => (
            <button
              key={item.id}
              type="button"
              className="nav__item"
              aria-current={page === item.id ? "page" : undefined}
              onClick={() => go(item.id, undefined)}
            >
              <span className="nav__icon" aria-hidden="true">
                {item.icon}
              </span>
              {item.label}
            </button>
          ))}
        </div>

        <div className="sidebar__footer">
          <span>Version {settings?.appVersion ?? "—"}</span>
          <span>No telemetry</span>
        </div>
      </nav>

      <main className="main" id="main-region" tabIndex={-1}>
        {/* Rendered above the page content so an update offer is visible
            wherever the user happens to be. */}
        <UpdateNotice />

        {page === "home" && (
          <Home
            onConfigure={(agent) => go("configure", agent)}
            onHistory={(agent) => go("history", agent)}
            onPlayground={(agent) => go("playground", agent)}
          />
        )}
        {page === "configure" && <Configure initialAgent={agentHint} />}
        {page === "playground" && <Playground initialAgent={agentHint} />}
        {page === "history" && <History initialAgent={agentHint} />}
        {page === "settings" && <Settings />}
      </main>
    </div>
  );
}

/**
 * The startup update check.
 *
 * Runs once, honours the user's setting, and never installs anything: it only
 * offers. A failed check is silent, because being offline is not an error the
 * user needs to hear about every launch.
 */
function UpdateNotice() {
  const { settings } = useApp();
  const [status, setStatus] = useState<UpdateStatus | null>(null);
  const [dismissed, setDismissed] = useState(false);

  useEffect(() => {
    if (!settings?.updateCheck) return;
    let cancelled = false;

    void call("check_for_updates_on_startup")
      .then((result) => {
        // `null` means the check was not due yet, or is disabled.
        if (!cancelled && result?.available) {
          setStatus(result);
        }
      })
      .catch(() => {
        // Silent by design: a failed background check is not actionable.
      });

    return () => {
      cancelled = true;
    };
  }, [settings?.updateCheck]);

  if (!status?.available || dismissed) return null;

  return (
    <div className="main__inner" style={{ paddingBottom: 0 }}>
      <Banner
        tone="info"
        title={`Eclyzer Bridge ${status.version} is available`}
        actions={
          <>
            <Button
              size="sm"
              variant="ghost"
              onClick={() => setDismissed(true)}
            >
              Not now
            </Button>
            <Button
              size="sm"
              variant="secondary"
              onClick={() => setDismissed(true)}
            >
              Open Settings to install
            </Button>
          </>
        }
      >
        You are running {status.currentVersion}. Open Settings to review and
        install it.
      </Banner>
    </div>
  );
}
