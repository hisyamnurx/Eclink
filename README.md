<img src="public/logo-mark.png" alt="Eclyzer Bridge" width="120" align="right" />

# Eclyzer Bridge

A Windows desktop app that points the **OpenAI Codex CLI** and **Claude Code** at
whichever provider you choose, without hand-editing configuration files. It edits
only the keys it owns, backs up everything before it touches it, and can restore
your previous configuration exactly.

- Windows 10/11 x64. The installer is a signed NSIS `.exe`.
- API keys are sealed with Windows DPAPI, scoped to your user account.
- Codex and Claude Code are configured and tested independently, because they use
  different APIs. A provider is reported compatible for each agent only when it
  actually answers that agent's client-facing endpoint.
- No telemetry, no analytics, no crash reporting, no Eclyzer server.
- MIT licensed.

## Install

Download the latest `Eclyzer-Bridge-Setup-*.exe` from
[Releases](../../releases) and run it. It installs per-user, so no administrator
rights are needed.

Windows SmartScreen will warn on first run because the binaries are not
code-signed. The updater signature is separate and is verified.

## Build from source

Requires Node 20+, Rust 1.85+, and the MSVC build tools.

```bash
npm ci
npm run tauri:build
```

`tauri:build` builds the bundled credential helper first, then the app. That order
matters: `tauri.conf.json` bundles the helper from `target/release/`, and Tauri
aborts the build when it is missing. A clean checkout fails with
`resource path ... doesn't exist` if the helper is skipped.

Checks:

```bash
npm run typecheck && npm run lint && npm run format:check && npm test
cargo fmt --all -- --check
cargo clippy --workspace --all-targets --all-features -- -D warnings
cargo test --workspace
```

## What it touches

Bridge writes to exactly two configuration files, and only the keys it owns:

| Agent       | File                                  |
| ----------- | ------------------------------------- |
| Codex       | `%USERPROFILE%\.codex\config.toml`    |
| Claude Code | `%USERPROFILE%\.claude\settings.json` |

Comments, key order, line endings, and every unrelated setting survive. An edit
made in another program between reading and writing aborts the change rather than
clobbering it.

Application data lives in `%LOCALAPPDATA%\EclyzerBridge`, which is separate from
the install directory, `%LOCALAPPDATA%\Eclyzer Bridge`. Uninstalling removes the
application and leaves your provider profiles and credentials in place.

## Documentation

| I want to…                     | Go to                                     |
| ------------------------------ | ----------------------------------------- |
| Build and contribute           | [CONTRIBUTING.md](0-Docs/CONTRIBUTING.md) |
| Report a security problem      | [SECURITY.md](0-Docs/SECURITY.md)         |
| Know what data is stored where | [PRIVACY.md](0-Docs/PRIVACY.md)           |
| Cut a release                  | [RELEASING.md](0-Docs/RELEASING.md)       |
| See what changed               | [CHANGELOG.md](0-Docs/CHANGELOG.md)       |

## Licence

MIT. See [LICENSE](LICENSE).

**Eclyzer Bridge is an independent open-source project and is not affiliated
with, endorsed by, or sponsored by OpenAI or Anthropic.**
