# Security Policy

## Scope

Eclyzer Bridge configures other programs, so it is worth being precise about
what this policy covers.

**In scope.** Vulnerabilities in this project and its code:

- the Eclyzer Bridge desktop application;
- the bundled credential helper (`eclyzer-bridge-helper.exe`) and how it reads
  and returns a stored credential;
- the configuration-writing logic — parsing, patching, verifying, and restoring
  agent configuration files;
- secret storage and the encryption path around Windows DPAPI;
- the update mechanism, including signature verification;
- the IPC surface between the webview and the Rust backend, and the app's
  handling of untrusted input from the frontend.

**Out of scope.** The following are not this project's security boundary:

- the provider you point the app at, and that provider's own key handling,
  logging, retention, and billing;
- OpenAI Codex and Claude Code themselves, their configuration formats, and
  their behaviour once launched;
- other software on your machine, your operating system, or a machine already
  compromised by malware running as your Windows user;
- the security of your API key once it has been handed to your provider;
- the GitHub release infrastructure used for update checks.

If a problem lies in a provider or in an agent, please report it to that project
instead. If you are unsure which side a problem is on, report it to us privately
and we will say so plainly.

---

## Reporting a vulnerability

**Please do not open a public issue for a security problem.** A public issue
tells everyone about a weakness before there is a fix, including people who
would use it.

Report privately through GitHub Security Advisories:

<https://github.com/hisyamnurx/Eclyzer-Bridge/security/advisories/new>

That channel is private between you and the maintainers until an advisory is
published. If you cannot use it, open a normal issue that says only that you
have a security report and ask for a private channel — do not include the
details, the steps, or any exploit in the issue itself.

### What to expect

- We aim to acknowledge a report and give an initial assessment within about
  **7 days**. This is a good-faith target maintained by volunteers, not a
  contractual service level.
- We will tell you what we found, whether we consider it in scope, and roughly
  what we intend to do.
- We will credit you in the advisory and in the release notes, **unless you ask
  us not to**. Anonymity is offered by default on request, no explanation
  needed.
- Please give us a reasonable window to ship a fix before you write about the
  issue publicly. We will not sit on a confirmed report.

---

## What to include in a report

The more precisely you can describe the conditions, the faster this goes:

- the **Eclyzer Bridge version** (shown in the app, and in the diagnostics
  report);
- your **Windows version** and architecture;
- **steps to reproduce**, in order, starting from a known state;
- what you expected to happen and what happened instead;
- any error message the app displayed;
- a **diagnostics report**, copied from Settings with "Copy Diagnostics".

The diagnostics report is designed to be safe to share: it contains versions,
paths, and configuration states, and passes through a mandatory redactor before
you ever see it. It contains **no API keys, no credentials, and no prompts**.

That said, read anything before you paste it into a public place. If you have
any doubt about a specific line, remove it — a report with one line removed is
far more useful than no report.

---

## Security design

These are the protections actually implemented in this codebase. Each is
verifiable in the source.

### Secret storage

API keys are sealed with **Windows DPAPI, Current User scope**
(`CryptProtectData` / `CryptUnprotectData`, `CRYPTPROTECT_UI_FORBIDDEN`, no
extra entropy) and stored in `secrets.dat` under
`%LOCALAPPDATA%\EclyzerBridge\`. The blob is bound to your Windows user profile:
copying the file to another machine or opening it from another account fails to
decrypt.

On Windows the non-Windows fallback cipher is not compiled in; it exists only so
the crate builds and its tests run elsewhere, and the app only builds for
Windows in this version.

### The key never reaches a file it should not

`profiles.json` stores an opaque `secretRef`, never a key. The key is not
written to `app.json`, not written to any backup or snapshot, not written to any
log line, and not included in the diagnostics report. Backups capture agent
configuration files only, and those files do not contain the key — which is what
makes it safe to back them up unconditionally.

### Codex: a credential-helper command instead of a key in the config

For Codex, the app writes **the path of a bundled credential helper** into
`config.toml`, not the key. The helper is a separate small executable that reads
the sealed secret on demand and returns it to Codex. The token therefore never
appears in the configuration file, which is the file most likely to be shared,
screenshotted, or committed.

Where the installed Codex is too old to support the helper mechanism, the app
falls back to an environment-based credential and tells you it did so, rather
than silently degrading.

### Atomic write-then-verify

Every configuration write follows the same sequence:

1. write a sibling temporary file in the same directory, so the rename stays on
   one volume;
2. flush and `sync_all`, so the bytes are durable before the swap;
3. atomically replace the destination;
4. **re-read the destination and compare it to what was written.**

Step 4 matters as much as the rest: a rename that succeeded but produced
different bytes is treated as a failure, not a success. If anything goes wrong
before the swap, the destination keeps its previous contents.

### An unparsable configuration is a hard stop

If an agent's configuration file cannot be parsed, the app **refuses to modify
it**. It does not auto-repair, does not rewrite, and does not "fix" it. A file
it does not understand may contain settings it does not recognize, and guessing
means deleting someone's work. The diagnostics report calls this state out
explicitly so support can see it immediately.

Beyond that, the app patches **only the keys it owns**, using key-order- and
comment-preserving editing, so unrelated settings, comments, formatting, and
line endings survive every change.

### Network hardening during provider tests

- **TLS verification is never disabled.** Not behind a flag, not behind an
  advanced option.
- **Redirects are not followed.** A 3xx response is reported as a failure rather
  than chased. Following a redirect could forward an `Authorization` header to
  an origin you never chose.
- **Plain HTTP to a remote host requires explicit per-profile confirmation**, and
  the app says why. Loopback addresses are exempt, because the key never leaves
  the machine.
- **A user-supplied header cannot override the credential.** Custom headers
  named `Authorization`, `x-api-key`, or `Proxy-Authorization` are filtered out
  of the profile's extra headers, so a profile cannot be configured to send the
  key somewhere unexpected.

### Process launching

The app never joins a shell command out of user-controlled text. It resolves a
terminal host and passes the agent executable and the project directory as
**separate argv entries**. A project path containing spaces, quotes, `&`, `;`,
or other shell metacharacters therefore remains one argument and cannot become a
command.

Where a PowerShell fallback is used, the path and executable are embedded as
single-quoted literals with quotes doubled, and the directory is set with
`Set-Location -LiteralPath` rather than string concatenation — defense in depth
on top of the argv separation.

The credential handed to a launched agent is applied as an **overlay to that
child process only**. Your global environment is never modified.

### IPC surface

Every Tauri command answers a product question. There is deliberately **no**
generic `read_file`, `write_file`, or `run_command`. A generic command would
make the webview a trust boundary, and any script that reached it could read
`secrets.dat` or write to a shell profile.

The frontend is treated as untrusted input. Identifiers coming from it are
length-bounded and restricted to alphanumerics, hyphens, and underscores before
they are used in a file name; agent names are parsed against a fixed set;
unknown values are rejected with a validation error. Values in the diagnostics
report are drawn from the backend, never from the webview, so a compromised page
cannot inject content into a report you are about to paste publicly.

### Updates

The updater verifies a minisign signature against a public key compiled into the
application before anything is installed. Only the public key is in the
repository; the private key never is. An artifact whose signature does not match
is rejected and **nothing on disk changes**.

Updates are never installed silently. Checking is at most once every 24 hours
and can be turned off in Settings; installing is always a separate, explicit
action.

### Locks against racing writes

Agent mutations take an exclusive file lock. A second operation — or a stale
lock — causes a clear refusal rather than two interleaved patches. Before
activation, the app also compares the configuration hash the UI last saw against
the file on disk, so an edit made in another window aborts the write instead of
being clobbered.

---

## Known limitations

Stated plainly, because a security document that only lists strengths is not
useful.

- **DPAPI does not stop malware running as you.** It protects secrets at rest
  against another Windows user or another machine. Any process that can act as
  your user can ask Windows to decrypt the same blob this app decrypts. No local
  secret store on Windows avoids this.
- **For Claude Code, the key is written into the user settings file.** Unlike
  Codex, Claude Code offers no declarative "run this command for the credential"
  field that works for a gateway. It has `apiKeyHelper`, but that is a shell
  command **string** evaluated by Claude Code itself, which would mean embedding
  an executable path into a shell line — exactly the injection surface this
  project refuses to build. The app therefore writes the credential into
  `~/.claude/settings.json` and then restricts that file to owner-only
  permissions, and **Restore default** removes it again. The UI states where the
  credential lives, because you deserve to know.
- **Permission hardening is best-effort.** Resetting the file's ACL is attempted
  and its failure does not make the app unusable, because the containing profile
  directory is already user-scoped. It is a guard against an unusual inherited
  ACL, not a substitute for one.
- **The app does not protect an agent from itself.** Once Codex or Claude Code
  has your credential and is running, its behaviour is its own. This project
  does not sandbox, monitor, or restrict the agents it configures.
- **A provider you choose sees your requests.** If you point the app at a
  gateway, that gateway sees your prompts and your key. Choosing a provider you
  trust is your decision, and it is the single largest one in the threat model.

## Verifying these claims

The source is public and MIT licensed. The files that matter most for the claims
above are `src-tauri/src/commands/mod.rs` (no generic escape hatch),
`src-tauri/src/config/fs.rs` (atomic write and verify, permission hardening),
`src-tauri/src/secrets/crypto.rs` and `src-tauri/src/secrets/store.rs` (DPAPI
storage), `src-tauri/src/adapters/codex.rs` and
`src-tauri/src/adapters/claude.rs` (how each agent is configured, including the
Claude trade-off in its module documentation),
`src-tauri/src/testing/probes.rs` (TLS and redirect policy),
`src-tauri/src/process/mod.rs` (argv-only launching), and
`src-tauri/src/updater.rs` (signature-verified, user-confirmed updates).
[PRIVACY.md](PRIVACY.md) covers the data side in more detail.
