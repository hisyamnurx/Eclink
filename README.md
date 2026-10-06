<img src="0-Docs/logo.png" alt="Eclyzer Bridge" width="120" align="right" />

# Eclyzer Bridge

> **Documentation lives in [`0-Docs/`](0-Docs/) — start with
> [the README there](0-Docs/README.md).**

This file exists only so the repository landing page is not empty. GitHub renders
a README from the repository root, and the real documentation is kept in
`0-Docs/` alongside the other guides, the product requirements, and the internal
notes.

## Quick links

| I want to…                        | Go to                                                      |
| --------------------------------- | ---------------------------------------------------------- |
| Understand what this is           | [0-Docs/README.md](0-Docs/README.md)                       |
| Install it                        | [0-Docs/README.md](0-Docs/README.md#install)               |
| Build it from source              | [0-Docs/CONTRIBUTING.md](0-Docs/CONTRIBUTING.md)           |
| Report a security problem         | [0-Docs/SECURITY.md](0-Docs/SECURITY.md)                   |
| Know what data is stored where    | [0-Docs/PRIVACY.md](0-Docs/PRIVACY.md)                     |
| Cut a release                     | [0-Docs/RELEASING.md](0-Docs/RELEASING.md)                 |
| See what changed                  | [0-Docs/CHANGELOG.md](0-Docs/CHANGELOG.md)                 |
| Read the product requirements     | [0-Docs/PRD/](0-Docs/PRD/)                                 |
| Pick up development where it left | [0-Docs/Internal/PROGRESS.md](0-Docs/Internal/PROGRESS.md) |

## The short version

Eclyzer Bridge is a Windows desktop app that points the OpenAI Codex CLI and
Claude Code at whichever provider you choose, without you hand-editing
configuration files. It edits only the keys it owns, backs up everything before
it touches it, and can restore your previous configuration exactly.

- Windows 10/11 x64. The installer is a signed NSIS `.exe`.
- API keys are sealed with Windows DPAPI, scoped to your user account.
- No telemetry, no analytics, no crash reporting, no Eclyzer server.
- MIT licensed.

**Eclyzer Bridge is an independent open-source project and is not affiliated
with, endorsed by, or sponsored by OpenAI or Anthropic.**
