# Changelog

All notable changes to this project are documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

> **Eclink is an independent open-source project and is not affiliated
> with, endorsed by, or sponsored by OpenAI or Anthropic.**

## [0.2.1] - 2026-10-20

A patch release containing the Claude Code base-URL fix and the CI fixes. No
feature was added, and no profile, API key, or backup format changed.

### Fixed

- **A profile whose base URL ends in `/v1` now works in real Claude Code, not
  just in the Playground.** Claude Code appends `/v1/messages` to
  `ANTHROPIC_BASE_URL`, so writing a base URL that already ended in `/v1` made
  it request `/v1/v1/messages`. The provider answered
  `404 Invalid URL (POST /v1/v1/messages)`, and Claude Code reported that as
  "There's an issue with the selected model (…)", which points at the model ID
  instead of the URL. Activation now removes only a trailing version segment
  before writing the variable, and keeps a custom tenant path such as
  `/api/anthropic`. A base URL that is not a URL is rejected before the settings
  file is touched.
- **A custom model ID containing `:` (for example `deepseek-v4.1-flash:mr`) is
  preserved exactly.** It was never the cause of the failure above, and it is not
  rewritten to suppress the "model isn't recognized" startup notice.

### Changed

- **CI and the release workflow** now check out text files with LF line endings
  on Windows (`.gitattributes`), which is what made the "Format check" step fail
  with every file reported unformatted; the Rust job builds the credential helper
  before Clippy and the tests, because the Tauri build script refuses to compile
  while the helper declared as a bundle resource is missing. Both workflows moved
  to `actions/checkout@v5` and `actions/setup-node@v5` on Node 24, since the
  runners now force Node 20 actions onto Node 24.
- **The helper round-trip tests** looked for the pre-rebrand
  `eclyzer-bridge-helper.exe` only, so they failed on a clean build once the
  binary was renamed to `eclink-helper.exe`. They now try the current name first
  and keep the legacy name as a fallback.

### Known limitations

- **One provider still rejects Claude Code's full request.** After the base-URL
  fix above, ModelVIP (`https://modelrouter.id/v1`) answers Claude Code's complete
  agent request — roughly 58 KB, carrying `output_config.effort`, adaptive
  `thinking`, `context_management`, and 14 deferred tools — with HTTP 503 in about
  60–120 ms. The same credentials, model, and endpoint serve a minimal
  `/v1/messages` request in the same second, so the endpoint is correct and
  reachable. The unsupported field was not isolated after extensive bisection
  (each field, the total size, and the header set pass on their own). This is a
  provider-side limitation. Eclink does not rewrite, strip, or translate Claude
  Code's request to work around it, because that would silently change what the
  agent asks for. Streaming, tool use, and token accounting were all verified
  working against this endpoint.
- **The provider token lives in plaintext in Claude Code's `settings.json`.** That
  is Claude Code's own storage; Eclink writes what the agent reads. Within Eclink
  the token is encrypted at rest with DPAPI
  (`%LOCALAPPDATA%\EclyzerBridge\secrets.dat`), is redacted from log files, and is
  blanked (`ANTHROPIC_AUTH_TOKEN: ""`) in every backup snapshot and captured
  "original" copy, so Eclink never keeps a second readable copy. The file Eclink
  activates is the one Claude Code itself reads, so its contents are exactly as
  protected as that file already was.

## [0.2.0] - 2026-10-20

The rebranding release. **Eclyzer Bridge is now Eclink.** This release changes the
name, the installer, and the Windows application identity. It is not a rewrite:
the features, the configuration handling, and the data model are the same.

### Changed

- **The product is called Eclink.** Window title, sidebar wordmark, About and
  Settings copy, dialogs, notifications, diagnostics output and both language
  dictionaries now say Eclink. The English and Indonesian dictionaries were both
  updated; the brand name is not translated.
- **Windows identity.** The executable is `eclink.exe`, the installer is
  `Eclink-Setup-<version>.exe`, the install directory is `%LOCALAPPDATA%\Eclink`,
  and the Start Menu and Add/Remove Programs entries read "Eclink". The bundle
  identifier is unchanged so Windows still recognizes the existing installation.
- **The credential helper is `eclink-helper.exe`.** The old
  `eclyzer-bridge-helper.exe` is still accepted as a fallback, so a Codex
  configuration written before the rename keeps resolving credentials.
- **Repository moved** to <https://github.com/hisyamnurx/Eclink>. The updater
  endpoint follows it. The updater signing key is unchanged, so an installed
  build still verifies updates with the key it already trusts.
- **Publisher metadata** is now "Eclink contributors".

### Added

- **A safe upgrade path from Eclyzer Bridge.** Existing profiles, DPAPI-sealed
  API keys, backups, snapshots and history are read from the unchanged data
  directory, so an upgrade does not require re-entering keys or recreating
  profiles. The language preference survives.
- **An installer hook** that retires the legacy "Eclyzer Bridge" uninstall
  registry entry once its executable is gone, so one application does not appear
  twice in Add/Remove Programs. If the old installation is still present, the
  entry is left alone rather than removed under a live installation.
- **[docs/legacy-compatibility.md](../docs/legacy-compatibility.md)**, which lists
  every identifier intentionally kept from the previous name and what depends on
  it, so a later cleanup cannot silently break upgraded installations.
- **Regression tests** covering the retained identifiers and the upgrade path.

### Notes

- Application data stays in `%LOCALAPPDATA%\EclyzerBridge` on purpose. Renaming
  it would strand the DPAPI blob and every existing profile.
- Old log files are not rewritten. They keep their name and are pruned by the
  normal retention window.

## [0.1.0] - 2025-10-06

The initial release, published as **Eclyzer Bridge**.

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

[Unreleased]: https://github.com/hisyamnurx/Eclink/compare/v0.2.0...HEAD
[0.2.0]: https://github.com/hisyamnurx/Eclink/releases/tag/v0.2.0
[0.1.0]: https://github.com/hisyamnurx/Eclink/releases/tag/v0.1.0
