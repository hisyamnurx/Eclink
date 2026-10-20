<img src="public/logo-mark.png" alt="Eclink" width="120" align="right" />

# Eclink

**AI Provider Manager for Coding Agents.**

Eclink is a lightweight Windows application that lets you configure, test, and
switch AI providers for **OpenAI Codex** and **Claude Code** without manually
editing configuration files. It edits only the keys it owns, backs up everything
before it touches it, and can restore your previous configuration exactly.

- Windows 10/11 x64. The installer is an NSIS `.exe`.
- API keys are sealed with Windows DPAPI, scoped to your user account.
- Codex and Claude Code are configured and tested independently, because they use
  different APIs. A provider is reported compatible for each agent only when it
  actually answers that agent's client-facing endpoint.
- No telemetry, no analytics, no crash reporting, no Eclink server.
- MIT licensed.

Eclink is the current name of the project previously released as **Eclyzer
Bridge**. Upgrading keeps your existing profiles, encrypted API keys, backups and
history; see [Migration from Eclyzer Bridge](#migration-from-eclyzer-bridge).

## Install

Download the latest `Eclink-Setup-*.exe` from
[Releases](https://github.com/hisyamnurx/Eclink/releases) and run it. It installs
per-user, so no administrator rights are needed.

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

To confirm a built executable actually carries the current icon set, rather than a
cached Windows resource from an earlier build:

```bash
node scripts/compare-icon.mjs target/release/eclink.exe src-tauri/icons/icon.ico
```

## What it touches

Eclink writes to exactly two configuration files, and only the keys it owns:

| Agent       | File                                  |
| ----------- | ------------------------------------- |
| Codex       | `%USERPROFILE%\.codex\config.toml`    |
| Claude Code | `%USERPROFILE%\.claude\settings.json` |

Comments, key order, line endings, and every unrelated setting survive. An edit
made in another program between reading and writing aborts the change rather than
clobbering it.

Application data lives in `%LOCALAPPDATA%\EclyzerBridge`, which is separate from
the install directory, `%LOCALAPPDATA%\Eclink`. Uninstalling removes the
application and leaves your provider profiles and credentials in place.

> The data directory still carries the pre-rename name on purpose. It is where
> every existing installation already keeps its profiles, secrets and snapshots,
> so renaming it would strand that data. The reasoning and the full list of
> retained identifiers are in
> [docs/legacy-compatibility.md](docs/legacy-compatibility.md).

## Migration from Eclyzer Bridge

Installing Eclink over an existing Eclyzer Bridge installation is supported.

- **Profiles, endpoints, model IDs and agent targets** are read from the existing
  data directory and continue to work unchanged.
- **API keys** stay usable: the DPAPI blob is bound to your Windows user account,
  not to the product name, so it decrypts exactly as before. You do not re-enter
  your keys.
- **Backups, snapshots and history** remain in place and remain restorable, so
  Restore Original still returns the file that existed before any of this.
- **Your language preference** is preserved, including Indonesian.
- **Codex and Claude Code keep working.** A Codex configuration that already
  points at the old credential helper keeps resolving credentials, because Eclink
  accepts both the old and the new helper executable name.

The installer retires the old "Eclyzer Bridge" entry from Add/Remove Programs once
its executable is gone, so you do not end up with two entries for one app. If the
old installation is still present, the entry is deliberately left alone.

## Documentation

| I want to…                     | Go to                                       |
| ------------------------------ | ------------------------------------------- |
| Build and contribute           | [CONTRIBUTING.md](0-Docs/CONTRIBUTING.md)   |
| Report a security problem      | [SECURITY.md](0-Docs/SECURITY.md)           |
| Know what data is stored where | [PRIVACY.md](0-Docs/PRIVACY.md)             |
| Cut a release                  | [RELEASING.md](0-Docs/RELEASING.md)         |
| See what changed               | [CHANGELOG.md](0-Docs/CHANGELOG.md)         |
| Understand retained old names  | [legacy-compatibility.md](docs/legacy-compatibility.md) |

## Licence

MIT. See [LICENSE](LICENSE).

**Eclink is an independent open-source project and is not affiliated with,
endorsed by, or sponsored by OpenAI or Anthropic.**
