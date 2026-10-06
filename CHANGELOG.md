# Changelog

All notable changes to this project are documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

> **Eclyzer Bridge is an independent open-source project and is not affiliated
> with, endorsed by, or sponsored by OpenAI or Anthropic.**

## [Unreleased]

## [0.1.0] - 2025-10-06

The initial release.

### Added

- **Provider profiles for Codex and Claude Code.** Create a profile with a base
  URL, model ID, and API key, then target it at one or both agents. A profile is
  a single record, so the same provider can be pointed at both agents without
  being entered twice.

- **Lossless configuration editing.** Existing configuration files are read and
  parsed first, and only the keys Eclyzer Bridge owns are patched. Comments, key
  order, formatting, line endings, and every unrelated setting are preserved.
  Configuration files that cannot be parsed are reported and left untouched —
  there is no auto-repair path, because repairing a file the parser does not
  understand would mean deleting settings it does not recognize.

- **Per-agent detection and version reporting.** The Overview screen detects
  whether Codex and Claude Code are installed, reports the version each one is
  running, and shows what each is currently pointed at. Where an installed agent
  is too old for a configuration mechanism Eclyzer Bridge prefers, the fallback
  is used and stated rather than applied silently.

- **An 8-step Test Connection pipeline.** Reachability, authentication, model
  availability, agent protocol, streaming, tool calling, and a final verdict,
  each reported individually with its state, detail, and latency. The pipeline
  stops as soon as a step makes the remaining ones meaningless, and says
  explicitly which steps were skipped and why. Optional capabilities that fail
  lower the verdict to partially compatible rather than failing the whole
  profile. A network problem is reported as a failed test, never as an
  incompatible provider.

- **The Playground.** Send a single message to a configured endpoint to check
  that it answers, using the same validation, header handling, and
  insecure-endpoint rules as Test Connection.

- **Activation with snapshot-before-change.** Activating a profile for an agent
  takes a snapshot first, applies the change, writes atomically, and re-reads the
  file to verify what was written. A profile that has never passed a test cannot
  be activated, so an agent cannot silently end up broken. An edit made to the
  configuration in another window aborts the write instead of being clobbered.

- **Recovery: Undo, Snapshot, Restore default, Restore original.** Undo last
  change returns the agent to its previous state. Snapshots keep the 20 most
  recent states per agent, one taken before every change. Restore default removes
  the provider Eclyzer Bridge added and changes nothing else. Restore original
  writes back the exact file that existed before Eclyzer Bridge ever touched the
  agent — that original is captured once, is never overwritten, and is never
  deleted automatically. Every one of these is itself reversible.

- **DPAPI secret storage and a stateless credential helper.** API keys are
  encrypted with Windows DPAPI at Current User scope and stored in
  `secrets.dat`. A key is never written to `profiles.json`, `app.json`, a backup,
  a log, or a diagnostics report. For Codex, the configuration file contains the
  path of a bundled credential helper (`eclyzer-bridge-helper.exe`) that fetches
  the key from encrypted storage on demand, so the token never enters
  `config.toml`. For Claude Code, which offers no declarative credential-helper
  mechanism that avoids embedding a command string, the key is written into the
  user settings file, which is then restricted to the user's own Windows account;
  Restore default removes it again.

- **Redacted diagnostics and local logs.** "Copy Diagnostics" produces a
  report containing app version, platform, paths, the encryption backend, counts,
  and per-agent configuration states — never a key, a credential, or a prompt.
  Every log line passes through a mandatory redactor before it is written, and
  again when recent logs are read back. Log retention is bounded to 7 days and
  20 MB; logs can be cleared or the folder opened from Settings. Nothing is
  logged to a network sink.

- **A signed updater with explicit confirmation.** Update checks go to this
  project's public GitHub Releases manifest at most once every 24 hours, send
  nothing about the user, and can be turned off in Settings. An available update
  is reported; installing it is a separate, explicit user action. The downloaded
  bundle's signature is verified against the public key compiled into the app,
  and a payload whose signature does not match is rejected with nothing changed
  on disk.

- **An NSIS per-user installer.** The installer targets `currentUser` install
  mode, so it does not need administrator rights and does not modify anything
  outside the installing user's own Windows account. No application data is
  written during installation; directories are created lazily on first use.

### Notes

- **Windows only.** Windows 10 and Windows 11, 64-bit. Other platforms are not
  supported and are not tested.

- **Bring your own key.** Eclyzer Bridge does not provide, resell, or proxy model
  access. Test Connection and the Playground send real requests to the endpoint
  you configured, using your key, and those requests may count toward your
  provider usage.

- **DPAPI protects secrets at rest only.** It protects a key against another
  Windows user account or another machine. It does not protect a key from
  malware already running as the same Windows user. No local secret store on
  Windows does.

- **No telemetry of any kind.** No analytics, no crash reporting, no install
  identifier, and no server operated by this project.

- **The app does not run in the background.** Closing the window exits it.

[Unreleased]: https://github.com/hisyamnurx/Eclyzer-Bridge/compare/v0.1.0...HEAD
[0.1.0]: https://github.com/hisyamnurx/Eclyzer-Bridge/releases/tag/v0.1.0
