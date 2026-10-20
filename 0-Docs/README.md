<img src="../public/logo-mark.png" alt="Eclink" width="120" align="right" />

# Eclink

A lightweight provider and profile manager for AI coding agents on Windows.

Eclink points **OpenAI Codex** and **Claude Code** at an
OpenAI-compatible or Anthropic-compatible provider you choose — without you
hand-editing `config.toml` or `settings.json`, and without pasting an API key
into a file you might commit by accident.

> **Eclink is an independent open-source project and is not affiliated
> with, endorsed by, or sponsored by OpenAI or Anthropic.**

---

## Why it exists

Pointing a coding agent at a custom provider usually means opening its config
file and editing it by hand. That file often already contains settings you care
about: MCP servers, sandbox rules, other providers, comments explaining why
something is the way it is. A careless edit loses them.

Eclink does that edit for you, carefully:

- It reads and parses your existing configuration first, and **refuses to touch
  a file it cannot parse.** It never auto-repairs, because "repairing" a file it
  does not understand means deleting settings it does not recognize.
- It changes **only the keys it owns**, leaving comments, formatting, key order,
  line endings, and every other provider exactly as they were.
- It **snapshots before every change**, so any change can be undone — including
  the undo.
- It keeps your API key **out of your configuration files entirely** where the
  agent allows it (see [How your API key is stored](#how-your-api-key-is-stored)).

---

## Requirements

- Windows 10 or Windows 11, 64-bit.
- At least one of:
  - [OpenAI Codex CLI](https://github.com/openai/codex)
  - [Claude Code](https://docs.claude.com/en/docs/claude-code/setup)
- An API key for a provider you want to use.

Non-Windows platforms are not supported and are not tested.

---

## Install

Download the latest `Eclink_x.y.z_x64-setup.exe` from
[Releases](https://github.com/hisyamnurx/Eclink/releases) and run it.

The installer is per-user: it does not need administrator rights and does not
modify anything outside your own Windows account.

---

## How to use it

1. **Open Eclink.** The Overview screen detects Codex and Claude Code and
   shows what each one is currently pointed at.
2. **Add a provider.** Choose a preset or enter a base URL and model ID, then
   paste your API key.
3. **Test it.** Test Connection sends real requests and reports each step:
   whether the endpoint is reachable, whether your key is accepted, whether the
   model exists, whether streaming and tool calling work. You see exactly which
   step failed and why.
4. **Activate it.** Once a test passes, activate the provider for that agent.
   Eclink snapshots your configuration, applies the change, and re-reads
   the file to verify what it wrote.
5. **Open your agent.** Eclink opens it in a new terminal window in the
   folder you choose.

### Getting back to where you were

The History screen offers four ways back, all reversible:

| Option               | What it does                                                                        |
| -------------------- | ----------------------------------------------------------------------------------- |
| **Undo last change** | Returns the agent to the state just before its most recent change.                  |
| **Snapshots**        | The 20 most recent states, one taken before every change.                           |
| **Restore default**  | Removes the provider Eclink added. Nothing else changes.                    |
| **Restore original** | Puts back the exact file that existed before Eclink ever touched the agent. |

The original backup is taken once, is never overwritten, and is never deleted
automatically.

---

## How your API key is stored

Your key is encrypted with **Windows DPAPI, scoped to your Windows account**, and
stored in `%LOCALAPPDATA%\EclyzerBridge\secrets.dat`.

It is never written to `profiles.json`, to `app.json`, to a backup, to a log, or
to this repository.

Where the agent supports it, the key never enters your configuration file at all:

- **Codex** is configured to run a small bundled helper (`eclink-helper.exe`)
  that fetches the key from encrypted storage on demand. Your `config.toml`
  contains only the helper's path. A configuration written by the previous
  release, which names `eclyzer-bridge-helper.exe`, keeps working.
- **Claude Code** has no equivalent mechanism that does not embed a command
  string, so the key is written to the user settings file, which Eclink
  then restricts to your Windows account. This is a deliberate trade-off
  documented in the code.

**What DPAPI protects and what it does not.** It protects your key _at rest_ —
someone who copies `secrets.dat` off your machine cannot read it. It does **not**
protect your key from malware already running as you. No local secret store can.

---

## Privacy

**Eclink collects nothing.** There is no account, no telemetry, no
analytics, no crash reporting, no install identifier, and no server operated by
this project. Nothing about you or your machine is ever sent anywhere except as
described below.

The only network traffic the app can produce is:

1. **Provider requests.** Test Connection and the Playground talk directly to
   the provider you configured, using your key. Only when you press the button.
2. **Update checks.** At most once every 24 hours, an ordinary HTTPS request to
   GitHub Releases. It sends nothing about you, and can be turned off in
   Settings. Updates are never installed without your confirmation.
3. **Links you click.** Opened in your own browser.

Diagnostics you copy, and logs written to disk, are passed through a redactor
that strips API keys, tokens, passwords, cookies, and long opaque strings before
they are written. See [PRIVACY.md](PRIVACY.md) for the full detail.

---

## Building from source

Prerequisites: [Node.js](https://nodejs.org/) 20+, [Rust](https://rustup.rs/)
stable (MSVC toolchain), and the Visual Studio Build Tools with the Windows SDK.

```powershell
git clone https://github.com/hisyamnurx/Eclink.git
cd Eclink
npm install
npm run tauri:dev      # development
npm run tauri:build    # produces the installer
```

The installer appears in `target/release/bundle/nsis/`.

### Running the tests

```powershell
cargo test --manifest-path src-tauri/Cargo.toml   # Rust: config safety, adapters, recovery
npm test                                          # frontend
npm run typecheck
npm run lint
```

The test suite never contacts a paid provider. Provider behaviour is covered by
fixtures and a local mock server.

### Signing updates

Update bundles are signed with a minisign key. The **public** key is committed in
`src-tauri/tauri.conf.json`; the private key must never be committed.

Maintainers set these two environment variables when building a release:

```
TAURI_SIGNING_PRIVATE_KEY           = path to the private key file
TAURI_SIGNING_PRIVATE_KEY_PASSWORD  = its password
```

If you are building your own copy for personal use and do not need working
updates, replace the `pubkey` value in `src-tauri/tauri.conf.json` with your own
public key — or generate one with `npm run tauri signer generate`.

---

## What Eclink is not

Worth stating plainly, because the boundaries are deliberate:

- It is **not** an API gateway or a local proxy. It does not sit between your
  agent and your provider, and it does not translate between protocols.
- It does not provide or resell model access. You bring your own key.
- It is **not** an IDE, a terminal emulator, or a chat client. The Playground
  sends one message so you can check an endpoint; it is not a place to work.
- It does not install, update, or manage Codex or Claude Code for you.
- It does not run in the background. When you close the window, it is gone.

---

## Contributing

Issues and pull requests are welcome. Please read
[CONTRIBUTING.md](CONTRIBUTING.md) first, and note that this project follows the
[Contributor Covenant Code of Conduct](CODE_OF_CONDUCT.md).

Two rules matter more than any style preference:

1. **Never replace a user's configuration file wholesale.** Read, parse, patch,
   verify.
2. **Never let an API key reach a file, a log, or a diagnostic output.**

## Security

Please report vulnerabilities privately, as described in
[SECURITY.md](SECURITY.md). Do not open a public issue for a security problem.

## License

[MIT](../LICENSE).
