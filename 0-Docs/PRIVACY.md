# Privacy

Eclink is a local desktop application. It has no backend, no account
system, and no way to report anything about you to its authors.

**Eclink collects nothing.** Specifically, it has no telemetry, no
analytics, no crash reporting, no usage statistics, no install identifier, no
device fingerprint, and no server operated by this project. Nothing about you,
your machine, your providers, your API keys, or your prompts is sent to the
people who make this app — because there is nowhere for it to be sent.

> **Eclink is an independent open-source project and is not affiliated
> with, endorsed by, or sponsored by OpenAI or Anthropic.**

---

## The only three outbound network cases

Everything the app can send over the network falls into one of these three
categories. There is no fourth.

### 1. Provider requests you initiate

Test Connection and the Playground send HTTPS requests **directly to the
endpoint you configured**, authenticated with **your own API key**. These
requests go to your provider, not to this project.

They only ever happen when you press a button — "Test Connection", or "Send" in
the Playground. The app never tests a provider in the background, never
pre-fetches a model list, and never validates a key on your behalf at startup.

Test Connection performs a small number of real requests (reachability,
authentication, model availability, protocol, streaming, tool calling). They may
count toward your provider usage and cost a small amount. The app says so before
the first run.

### 2. Update checks against GitHub Releases

At most **once every 24 hours**, on startup, the app makes an ordinary HTTPS
request for this project's public release manifest:

```
https://github.com/hisyamnurx/Eclink/releases/latest/download/latest.json
```

That request contains **nothing about you**: no identifier, no version report
tied to you, no machine details, no usage data. It is the same request any
browser would make for a public file. GitHub necessarily sees the requesting IP
address, as it would for any HTTPS request; that is a property of the network,
not of this app.

You can turn automatic update checks off in **Settings**. The last-check
timestamp is stored locally in `app.json` so the 24-hour interval can be
enforced; it is never transmitted. A failed check is reported as information,
not an error, because being offline is normal.

### 3. Links you click

Help and documentation links open in **your own default browser**, through the
operating system. The app itself does not fetch them.

---

## What is stored on disk, and where

All application data lives under one directory:

```
%LOCALAPPDATA%\EclyzerBridge\
```

Nothing is written during installation. Directories are created lazily on first
use, so a fresh install stays inert until you actually do something.

| Path            | What it holds                                                                                                                                                                                                                 |
| --------------- | ----------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| `app.json`      | Your settings: schema version, whether automatic update checks are on, the last update-check timestamp, the last project folder (only if you leave that option on), log level, and one-time notice flags. No secrets.         |
| `profiles.json` | Your provider profiles: name, base URL, model ID, which agents each targets, advanced options, and test results. **An API key never appears here.** A profile references its key by an opaque ID (`secretRef`) only.          |
| `secrets.dat`   | Your API keys, each sealed with Windows DPAPI scoped to your Windows account. This is the only place a key is stored sealed.                                                                                                  |
| `backups\`      | The Original backup, the 20 most recent snapshots per agent, and the change history ledger. These copy your agent configuration files, with the value of any credential key emptied first, so no API key is ever stored here. |
| `logs\`         | Local log files, redacted before writing. See below.                                                                                                                                                                          |
| `cache\`        | Local scratch data.                                                                                                                                                                                                           |

**Nothing is written outside these locations** except the agent configuration
files you explicitly ask the app to change (`config.toml` for Codex,
`settings.json` for Claude Code), the backups of those files, and Claude Code's
settings file permission hardening.

The app does not run in the background. When you close the window, it is gone.

---

## How your API key is protected

Keys are encrypted with **Windows DPAPI**, Current User scope
(`CryptProtectData` / `CryptUnprotectData`, with UI prompts forbidden and no
additional entropy). The Settings screen and the diagnostics report show the
active backend name — `Windows DPAPI (Current User)` — so you can tell at a
glance whether your keys are actually encrypted.

**What DPAPI protects against, honestly stated:**

- It protects your key **at rest**. Someone who copies `secrets.dat` to another
  machine, or opens it from another Windows account, cannot read it. The blob is
  cryptographically bound to your Windows user profile.
- It does **not** protect your key from malware running as you, in your own
  session, right now. Any process that can act as your Windows user can ask
  DPAPI to decrypt the same blob this app does. No local secret store on Windows
  can prevent that, and this project will not pretend otherwise.

Where the agent allows it, the key does not even enter the agent's configuration
file: **Codex** is pointed at a bundled `eclink-helper.exe` that fetches
the key from encrypted storage on demand, so `config.toml` contains only the
helper's path. **Claude Code** has no declarative equivalent that does not
embed a command string, so the key is written into Claude Code's user settings
file, which is then restricted to your Windows account. That trade-off is
documented in [SECURITY.md](SECURITY.md) rather than hidden.

---

## Redaction: what is scrubbed before anything is written or copied

Every string that can reach a log file, a diagnostics report, or the UI passes
through a mandatory redactor. It is fail-safe (unknown-shaped secrets are masked
rather than leaked), idempotent (redacting twice equals redacting once), and
context-aware.

These patterns are replaced with `[REDACTED]`:

- **Authorization headers** — `Authorization:` and `Proxy-Authorization:`, with
  or without a `Bearer`, `Basic`, or `Token` prefix.
- **API key headers** — `x-api-key`, `x-goog-api-key`, `api_key`, `apikey`,
  `anthropic-api-key`, `openai-api-key`, in either `:` or `=` form.
- **Cookies** — any `Cookie:` or `Set-Cookie:` header line.
- **Secret query parameters** — `?api_key=`, `&token=`, `&access_token=`,
  `&apikey=`, `&authorization=`, `&password=`, `&signature=`, `&secret=`,
  `&auth=`, `&key=`, `&sig=`. Non-secret parameters on the same query string
  survive, so redaction does not swallow the diagnostic value of the line.
- **Password and secret assignments** — `"api_key": "..."`, `api_key = "..."`,
  `apiKey: '...'`, and the bare `api_key=value` form, including
  `access_token`, `refresh_token`, `client_secret`, `password`, `secret`,
  `bearer`, and `token`.
- **Known key prefixes** without any surrounding context — `sk-`, `rk-`, `pk-`,
  `gsk-`, `xai-`, `fw-`, `hf-`, `api-`, `key-`, `tok-`, `pat-`, `ghp-`, `gho-`,
  `github_pat-`, `glpat-`, `SG-`, `AKIA-` followed by 12 or more characters.
- **URL userinfo credentials** — `scheme://user:password@host`, where both the
  user and the password are replaced.
- **Long opaque tokens** — any unbroken 40-or-more-character base64 or hex run
  that occupies a whole whitespace-delimited token. This is what catches a
  credential in a shape nobody anticipated.

Two deliberate restrictions keep diagnostics useful rather than useless: a model
ID such as `deepseek-chat-v3-0324-preview` is long but is not secret-shaped, so
it survives; and ordinary paths such as
`C:\Users\Test User\.codex\config.toml` pass through untouched.

When the UI needs to show that a key exists, it shows a fingerprint such as
`****abcd` — only the last four characters, and never for a key of 8 characters
or fewer, which is masked entirely.

Redaction is applied **before writing a log line**, not after. Retrieving recent
logs redacts again, in case a log file was edited or restored from elsewhere.
The diagnostics report is redacted per field and then once more over the
assembled text, as a final safety net.

---

## Logs

Logs are written locally, and only locally — there is no network sink and no
crash service. Retention is bounded:

- files older than **7 days** are deleted at startup;
- the log directory is trimmed to a **20 MB** total, oldest file first.

Retention runs at startup only, never on a background timer, so the app keeps
near-zero idle CPU.

You can **clear the logs** or **open the log folder** from Settings at any time.
Clearing removes the files; the currently open file may be held by the running
app and keep growing normally until you close it.

---

## The diagnostics report

"Copy Diagnostics" produces a pasteable text report containing:

- app version, platform, and architecture;
- the data directory and log directory paths;
- the encryption backend name and the number of stored secrets;
- the number of provider profiles;
- for each agent: whether it is installed, its detected version, its
  configuration file path, whether that configuration parses, whether backups
  exist, and how many snapshots are held;
- non-fatal warnings.

It contains **versions, paths, and configuration states only**. It never
contains an API key, a credential, a prompt, or a full log dump. The report says
so in its own text, and the redactor is applied over the finished document.

Every value in the report is drawn from the backend, never from the webview, so
a compromised page cannot inject content into a report you are about to paste
into a public issue.

---

## How to verify this yourself

You do not have to take any of this on trust. The app is MIT-licensed and the
complete source is at <https://github.com/hisyamnurx/Eclink>. The files
that make these claims true are:

| Claim                                                                   | File to read                                                   |
| ----------------------------------------------------------------------- | -------------------------------------------------------------- |
| Nothing is logged without redaction                                     | `src-tauri/src/diagnostics/redact.rs`                          |
| Every log line is redacted before it is written                         | `src-tauri/src/diagnostics/log.rs`                             |
| Log retention of 7 days / 20 MB                                         | `src-tauri/src/diagnostics/log.rs`                             |
| The diagnostics report contains no secrets                              | `src-tauri/src/diagnostics/report.rs`                          |
| Keys are sealed with DPAPI Current User                                 | `src-tauri/src/secrets/crypto.rs`                              |
| Keys live only in `secrets.dat`                                         | `src-tauri/src/secrets/store.rs`                               |
| `profiles.json` never holds a key                                       | `src-tauri/src/profiles/model.rs`                              |
| Backups never hold a key                                                | `src-tauri/src/backup/mod.rs`                                  |
| Nothing is written outside the app data folder                          | `src-tauri/src/paths.rs`                                       |
| Update checks, and that they install nothing silently                   | `src-tauri/src/updater.rs`                                     |
| The update endpoint, once per 24 hours                                  | `src-tauri/src/updater.rs`, `src-tauri/src/commands/update.rs` |
| TLS verification is never disabled, redirects are never followed        | `src-tauri/src/testing/probes.rs`                              |
| There is no generic file-read, file-write, or command-execution command | `src-tauri/src/commands/mod.rs`                                |

You can also simply watch it: the app makes no request that does not appear in
the table above, and you can run it with a proxy or network monitor of your
choosing. If you find behaviour that contradicts this document, that is a bug —
please report it as described in [SECURITY.md](SECURITY.md) if it is a security
problem, or as a normal issue otherwise.

---

## Children, tracking, and third parties

There is nothing to track and no third party to share with. No data is sold,
rented, or disclosed, because none is collected. The app contains no advertising
SDK, no analytics SDK, and no crash-reporting SDK.

## Changes to this document

This document describes version 0.2.0 of Eclink. If a future version
changes what the app sends or stores, this file will be updated in the same
change, because a privacy document that lags the code is worse than none.
