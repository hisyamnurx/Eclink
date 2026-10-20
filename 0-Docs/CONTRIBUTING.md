# Contributing to Eclink

Thanks for wanting to help. This project edits other people's configuration
files and handles their API keys, so the bar for care is higher than usual and
the rules below are not stylistic preferences.

---

## Prerequisites

- **Windows 10 or Windows 11, 64-bit.** The repository is Windows-only. Changes
  for other platforms will not be tested and cannot be accepted as-is.
- **Node.js 20+**
- **Rust** stable with the MSVC toolchain ([rustup.rs](https://rustup.rs/))
- **Visual Studio Build Tools** with the Windows SDK
- **WebView2** runtime (present on current Windows builds)
- At least one of Codex or Claude Code, if you want to exercise the app against
  a real installation rather than fixtures

---

## Setup and commands

```powershell
git clone https://github.com/hisyamnurx/Eclink.git
cd Eclink
npm install
```

```powershell
npm run tauri:dev      # development build with hot reload
npm run tauri:build    # produces the NSIS installer in target/release/bundle/nsis/
```

Tests and checks, all of which must pass before you open a pull request:

```powershell
cargo test --manifest-path src-tauri/Cargo.toml   # Rust
npm test                                          # frontend
npm run typecheck                                 # tsc --noEmit
npm run lint                                      # eslint, zero warnings tolerated
npm run format                                    # prettier, writes
```

Rust formatting and lints:

```powershell
cargo fmt --manifest-path src-tauri/Cargo.toml
cargo clippy --manifest-path src-tauri/Cargo.toml --all-targets -- -D warnings
```

`npm run lint` runs with `--max-warnings 0`, so a warning fails the build the
same way an error does. `npm run format:check` verifies formatting without
rewriting files; CI uses it.

---

## Two non-negotiable rules

Everything else here is a guideline. These two are not.

### 1. Never replace a user's configuration file wholesale

Read it, parse it, patch **only the keys this project owns**, and verify what
you wrote. A configuration file typically contains settings the user cares about
and this project does not understand: MCP servers, sandbox rules, other
providers, permissions, hooks, and comments explaining why something is the way
it is. Rewriting the file from a parsed structure silently deletes all of it.

In practice that means:

- parse first; if parsing fails, stop (see below);
- edit through the comment- and key-order-preserving paths
  (`src-tauri/src/config/toml_patch.rs`, `src-tauri/src/config/json_patch.rs`);
- write atomically and re-read to confirm;
- take a snapshot before the change, and keep it restorable.

A pull request that regenerates a configuration file from a model instead of
patching it will be rejected regardless of how well it is tested.

### 2. An API key must never reach a file, a log, or a diagnostic output

The key belongs in `secrets.dat` and nowhere else. It must not appear in
`profiles.json`, `app.json`, a backup, a snapshot, a log line, an error message,
a diagnostics report, a test fixture, or a commit.

Route anything that could carry a secret through the redactor, and prefer a
fingerprint (`****abcd`) over a value when the UI needs to show that a key
exists. If you add a new output path — a new log target, a new report field, a
new error detail — assume it will eventually be handed a key and redact
accordingly. Tests exist that deliberately feed secret-shaped strings through
these paths; keep them passing, and add one when you add a path.

---

## An unparsable configuration is a hard stop

If a user's configuration file cannot be parsed, the app **refuses to modify
it**. It does not auto-repair, does not rewrite, and does not "fix" it.

This is deliberate, and it must stay that way. A file the parser does not
understand may be valid to a newer version of the agent, or may contain settings
this project has never heard of. "Repairing" it means deleting the parts that
did not parse — which is to say, deleting the user's work, using a guess.

The correct behaviour is to report the state clearly, surface it in diagnostics
(worded, not colour-coded), leave the file exactly as it is, and tell the user
which file is involved. Pull requests that add a repair path, a lenient fallback,
or a "best effort" rewrite will not be merged.

---

## Tests must never contact a paid provider

No test — unit, integration, or manual-in-CI — may send a request to a real
provider endpoint. Real requests cost contributors money, fail when offline, and
produce results that depend on someone else's uptime.

Use fixtures and a local mock server instead. The existing suite follows this:
provider behaviour is covered by recorded fixtures and a local HTTP server, and
the transport-level tests build their own client. Add a new mock server case
rather than a live endpoint.

The same applies to the update endpoint and to any other network call. If a test
needs the network, it needs a mock.

---

## Pull request checklist

Before you open a pull request, confirm:

- [ ] `cargo test --manifest-path src-tauri/Cargo.toml` passes.
- [ ] `npm test` passes.
- [ ] `npm run typecheck` is clean.
- [ ] `npm run lint` is clean (zero warnings).
- [ ] `cargo fmt` is clean.
- [ ] `cargo clippy --all-targets -- -D warnings` is clean.
- [ ] The pull request description states **whether user data changed** — a new
      file, a new stored field, a new backup artifact, or a change to what is
      written where.
- [ ] The pull request description states **whether the IPC surface changed** —
      a new command, a changed parameter, a changed return shape.
- [ ] If either answer is yes, the relevant documentation
      ([PRIVACY.md](PRIVACY.md), [SECURITY.md](SECURITY.md), the README) is
      updated in the same pull request.
- [ ] No API key, token, or credential appears anywhere in the diff.

Those last two checklist items exist because the two things most likely to go
wrong in this codebase are a new file being written somewhere undocumented and a
new command widening what the webview can ask for. Both are easy to miss in
review and both are easy to describe in a sentence.

---

## Commit style

- **Imperative subject line**, as though completing the sentence "This commit
  will…". Write `Add snapshot pruning test`, not `Added` or `Adds`.
- **Wrap at 72 columns.**
- **Explain _why_ in the body.** The diff already shows what changed; the body
  should say what problem it solves, what alternative you rejected, and any
  trade-off the reader would otherwise have to rediscover. A change that touches
  configuration safety or secret handling should say so explicitly in the body.
- Keep the subject line free of a trailing period and free of issue numbers;
  reference issues in the body.

Example:

```
Refuse to patch a config that fails to parse

A user reported that a hand-edited config.toml caused their MCP servers
to disappear. The parser accepted a partial document and the patch was
applied to the fragment, so everything outside it was dropped.

Parsing is now all-or-nothing for agent configs: a file that does not
parse is reported and left untouched. Auto-repair was considered and
rejected - repairing a file we do not understand means deleting the
parts that did not parse.
```

---

## Platform support

The repository is **Windows-only**. The app builds for Windows, and the paths,
ACLs, DPAPI usage, terminal resolution, and installer are all Windows-specific.

Changes for macOS or Linux will not be tested and will not be merged. There is a
`PlaintextCipher` used by tests so the crate compiles and the store's logic can
be exercised on other platforms, but it is explicitly not secure and is not a
portability story. If you want to work on cross-platform support, open an issue
to discuss the design before writing code.

---

## Reporting bugs

Open an issue with:

- the version shown in the app;
- your Windows version;
- steps to reproduce;
- a diagnostics report copied from Settings. It contains versions, paths, and
  configuration states, and no API keys, credentials, or prompts.

For anything that looks like a security problem, do **not** open a public issue.
Follow [SECURITY.md](SECURITY.md) instead.

## License

By contributing, you agree that your contributions are licensed under the
[MIT License](../LICENSE), the same terms as the project.
