# Changelog

All notable changes to this project are documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

> **Eclink is an independent open-source project and is not affiliated
> with, endorsed by, or sponsored by OpenAI or Anthropic.**

## [Unreleased]

## [0.3.3] - 2026-11-24

### Added

- **Per-agent install locations.** Settings now has an **Agent install locations**
  section with a row for Codex CLI, Claude Code, and Oh My Pi. Each row defaults
  to automatic detection; a folder or a launcher file can be chosen with the
  native picker or pasted as an absolute path, and each row can be reset back to
  automatic. A saved location takes priority over `PATH` for that agent only, and
  is the executable used by detection, version probing, Oh My Pi's registry
  validation, and Launch. It does not move configuration, change profiles, copy
  files, or modify `PATH`.
- **Windows shim support for probing and Launch.** A `.cmd`/`.bat` launcher
  (npm and Bun installs) is executed through `cmd.exe` for its version and
  registry probes, and Launch transports the same command line through the
  terminal. The path and every argument are quoted once and handed to `cmd.exe`
  as one raw command line, because it does not parse a command line the way Rust
  quotes argv. Installs in folders containing spaces, apostrophes, or non-ASCII
  characters now work; a path containing `%`, a double quote, or a control
  character is refused with an explanation rather than run.

### Changed

- **Detection probes are bounded.** Version and registry probes now share a
  timeout that kills the child process; a hung launcher is reported as a
  timeout instead of blocking the interface. A process the batch file started
  itself can outlive that timeout; this is a documented limit, not a guarantee.
- **`app.json` schema version 2.** Adds optional `agentLocations`. A settings
  file from an earlier version loads unchanged and keeps automatic detection for
  every agent.
- **Settings writes are serialised.** Concurrent updates can no longer each read
  the same base and drop one another's field from `app.json`. A write that fails
  leaves both the in-memory settings and the adapters' live view on the last
  value that was actually stored.

### Fixed

- **A broken saved install location is reported, not silently bypassed.** If a
  saved folder or launcher disappears or cannot be run, that agent is shown as
  not detected with the reason and a way back to automatic detection. Eclink no
  longer falls back to a different installation from `PATH` behind the user's
  back.
- **Launch actually runs shims, in both terminal hosts.** Windows Terminal's
  command-line parser treats an apostrophe as a quote, and Windows PowerShell
  5.1 mangles embedded quotes in native arguments. A shim command is therefore
  carried as an encoded PowerShell script, which contains no quotes, apostrophes,
  or spaces. Launching a native launcher in the PowerShell fallback now also
  gets a real console window instead of a detached process with nowhere to run.

- **Interactive PowerShell fallback.** Launch leaves the standard streams available
  to the new console so the coding agent can receive input and display output.
- **Fresh location settings.** A late general settings response no longer
  replaces a launcher location saved while that request was in flight.
- **Codex npm detection.** Automatic detection prefers supported Windows
  launchers such as codex.cmd over extensionless npm shell scripts.

## [0.3.0] - 2026-11-02

Eclink now manages **Oh My Pi (OMP)** alongside Codex and Claude Code. This is
an additive release: existing profiles, keys, snapshots, history, and updater
identity are unchanged.

### Added

- **Oh My Pi detection and version gate.** Eclink finds `omp` from PATH, the Bun
  global bin directory, and `%LOCALAPPDATA%\omp`, and requires OMP 18.0.0 or
  newer for the custom-provider schema this release writes.
- **OMP provider injection into `models.yml`.** A profile becomes a named
  custom provider with `baseUrl`, `api`, `authHeader`, a model list, and an
  `apiKey` that runs Eclink's credential helper. Comments, key order, and other
  providers are preserved by a lossless YAML editor.
- **OMP model roles in `config.yml`.** `default`, `smol`, `slow`, and `plan` can
  be assigned from the profile editor. Roles left empty are never written, and a
  role the user edits after activation is not reverted by Restore Default.
- **OMP profiles.** Eclink lists the default profile and named
  `%USERPROFILE%\.omp\profiles\<name>` profiles, lets the user choose which one
  it manages, and isolates originals and ownership records per OMP profile.
- **Preview before activation.** The OMP flow shows the exact provider ID,
  selector, role changes, config directory, and any ownership conflict before
  either file is written.
- **OMP restore and undo.** Restore Default removes only Eclink's provider and
  the role values it wrote. Restore Original returns both files to the state
  before Eclink first touched that OMP profile. Snapshots are reversible and
  retained under the existing history UI.
- **OMP connection testing and Playground.** Both follow the profile's selected
  OMP protocol: OpenAI Chat Completions, OpenAI Responses, or Anthropic
  Messages. Streaming and tool calling are probed with the matching wire format.
- **OMP launch.** Launch passes `--profile` and the managed `--model` as separate
  argv entries, never as a shell string.
- **Sealed OMP backups.** OMP permits a literal `apiKey` in `models.yml`. Instead
  of refusing to work or copying that key into a plaintext snapshot, Eclink
  seals OMP snapshots and originals with Windows DPAPI. Restore still works for
  the same Windows user, and the credential never appears in plaintext backups.
- **English and Indonesian strings** for every OMP control.

### Changed

- The agent list, provider editor, overview, history, and Playground render
  three agents. The old two-way Codex/Claude label ternaries were replaced with
  one shared label mapping.
- A full stock reset is intentionally not offered for OMP: OMP has no single
  built-in gateway to return to. The UI explains Restore Default and Restore
  Original instead of presenting a button that would fail.

### Security

- API keys still live only in the DPAPI-sealed `secrets.dat`. OMP receives a
  command, not a key. OMP snapshots and originals are DPAPI-sealed and excluded
  from logs and diagnostics.
- Eclink refuses to overwrite a provider ID it cannot prove it owns.

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

[Unreleased]: https://github.com/hisyamnurx/Eclink/compare/v0.3.3...HEAD
[0.3.3]: https://github.com/hisyamnurx/Eclink/compare/v0.3.0...v0.3.3
[0.3.0]: https://github.com/hisyamnurx/Eclink/releases/tag/v0.3.0
[0.2.0]: https://github.com/hisyamnurx/Eclink/releases/tag/v0.2.0
[0.1.0]: https://github.com/hisyamnurx/Eclink/releases/tag/v0.1.0
