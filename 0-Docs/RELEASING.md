# Releasing Eclink

Maintainer-facing notes for cutting a release. Read the whole document before
starting: the signing steps are easy to get wrong and the failure mode is an
update that reaches users and is then rejected by every client.

---

## The update endpoint

Two places reference where clients look for updates, and they must agree:

- `src-tauri/src/updater.rs` — the update-checking logic, which uses the updater
  plugin configured by the Tauri config;
- `src-tauri/tauri.conf.json` — under `plugins.updater.endpoints`:

```json
{
  "plugins": {
    "updater": {
      "active": true,
      "dialog": false,
      "pubkey": "<committed minisign public key>",
      "endpoints": [
        "https://github.com/hisyamnurx/Eclink/releases/latest/download/latest.json"
      ],
      "windows": {
        "installMode": "passive"
      }
    }
  }
}
```

The endpoint resolves against whatever GitHub considers the _latest_ release, so
the release you publish must be marked as the latest one. A draft release has no
`latest/download` alias and clients will keep seeing the previous version. A
pre-release is not returned by `latest` either — if you intend to publish a
pre-release, change the approach deliberately rather than by accident.

The updater runs at most once every 24 hours on startup (and only when the user
has left automatic update checks enabled in Settings), plus whenever the user
asks. **The update is never applied without explicit user confirmation in the
app.**

---

## The signing key

Update bundles are signed with a **minisign** keypair.

- The **public** key is committed in `src-tauri/tauri.conf.json` under
  `plugins.updater.pubkey`. It is base64 of the minisign public key document.
- The **private** key must **NEVER** be committed, pasted into an issue, or
  attached to a release. It is a maintainer secret. Anyone holding it can sign an
  artifact that every existing installation will accept and install.

Clients verify the downloaded bundle's signature against the committed public
key and reject a payload whose signature does not match.

### Generate a keypair (once, per maintainer)

```powershell
npm run tauri signer generate -- -w <path-to-private-key>
```

Choose a strong password when prompted. Store the private key file somewhere
outside the repository, such as a password manager's file store or an encrypted
volume, and back it up: losing it means existing installations can no longer
receive updates and you must ship a new public key and a fresh install path.

### Supply the key when building

Set both environment variables for the release build:

```powershell
$env:TAURI_SIGNING_PRIVATE_KEY = "<path to the private key file, or the literal key>"
$env:TAURI_SIGNING_PRIVATE_KEY_PASSWORD = "<its password>"
```

`TAURI_SIGNING_PRIVATE_KEY` accepts either a path to the key file or the literal
key contents. A path is preferable: it keeps the key material out of the shell
history and out of any process listing.

If you are building your own copy for personal use and do not need working
updates, replace `pubkey` in `src-tauri/tauri.conf.json` with your own public
key instead of trying to obtain ours.

---

## A release must fail hard without the signing key

**An unsigned build must never be published.**

Two reasons, and the second is worse than the first:

1. **It will not work.** Clients verify the bundle against the committed public
   key. An unsigned artifact, or one signed with a different key, is rejected at
   install time and the user gets nothing but a failure message.
2. **It trains users to ignore failures.** If releases are sometimes unsigned,
   the rejection stops looking like a security control and starts looking like
   the app being flaky. A user who has learned that signature errors are normal
   is a user who will click through the one that matters.

Practically, this means: make the build fail rather than produce an artifact when
`TAURI_SIGNING_PRIVATE_KEY` is unset, and treat a missing `.sig` file after the
build as a failed release, not an inconvenience. Do not publish "just this once"
to unblock someone. Fix the key and rebuild.

The same reasoning applies to publishing by hand: if you are assembling a release
from files rather than from a signed build, stop and rebuild signed.

---

## Step-by-step release

### 1. Bump the version everywhere, so the three files agree

`package.json`, `src-tauri/tauri.conf.json`, and `Cargo.toml` (the workspace
manifest at the repository root) must all carry the same version string. The
crates declare `version.workspace = true`, so bump `[workspace.package]` in the
root `Cargo.toml`; there is no version to change in `src-tauri/Cargo.toml`. A
mismatch produces an installer whose version does not match its own update
manifest, and clients then either miss the update or offer it forever.

```powershell
# package.json
"version": "X.Y.Z"

# src-tauri/tauri.conf.json
"version": "X.Y.Z"

# Cargo.toml, under [workspace.package]
version = "X.Y.Z"
```

Then confirm the Rust side picked it up:

```powershell
cargo metadata --no-deps --format-version 1 |
  ConvertFrom-Json | Select-Object -ExpandProperty packages |
  Select-Object name, version
```

The release workflow checks all three against the pushed tag and refuses to build
if they disagree, so a missed bump fails in CI rather than shipping.

### 2. Update `CHANGELOG.md`

Move the relevant entries out of `[Unreleased]` into a new
`## [X.Y.Z] - YYYY-MM-DD` section, and add the comparison links at the bottom.
Follow the Keep a Changelog structure already in the file.

### 3. Run the full test suite

```powershell
cargo test --manifest-path src-tauri/Cargo.toml
npm test
npm run typecheck
npm run lint
cargo fmt --manifest-path src-tauri/Cargo.toml -- --check
cargo clippy --manifest-path src-tauri/Cargo.toml --all-targets -- -D warnings
```

Do not proceed with a red suite. Tagging a broken commit means the next release
has to explain it.

### 4. Build the signed installer

With both signing environment variables set:

```powershell
npm run tauri:build
```

The installer lands in `target/release/bundle/nsis/`, as
`Eclink_X.Y.Z_x64-setup.exe`.

### 5. Verify the signature file exists

```powershell
Get-ChildItem target\release\bundle\nsis
```

There must be a `.sig` file **next to the installer**, with a matching name. If
it is missing, the build was not signed — stop, fix the signing environment, and
rebuild. Do not publish.

### 6. Tag the release

```powershell
git tag vX.Y.Z
git push origin vX.Y.Z
```

The tag name must be `vX.Y.Z` with the `v`, matching the links in
`CHANGELOG.md`.

### 7. Publish the GitHub Release

Create a release for the tag, mark it as the latest release, and attach:

- the installer `.exe`;
- its `.sig` file;
- the updater `latest.json`.

Paste the changelog section into the release notes. Then configure the update
endpoint's source: `latest.json` must be reachable at
`https://github.com/hisyamnurx/Eclink/releases/latest/download/latest.json`,
which means it has to be a release asset, not a file committed to the repository.

---

## Writing `latest.json`

`latest.json` is a small JSON document that tells the updater what the newest
version is and where to get it.

```json
{
  "version": "X.Y.Z",
  "notes": "What changed in this release.",
  "pub_date": "2025-10-06T12:00:00Z",
  "platforms": {
    "windows-x86_64": {
      "signature": "<contents of the .sig file>",
      "url": "https://github.com/hisyamnurx/Eclink/releases/download/vX.Y.Z/Eclink_X.Y.Z_x64-setup.exe"
    }
  }
}
```

Field notes:

- **`version`** — the bare version string, without a leading `v`. It must match
  the version baked into the built installer, or clients will compare wrongly.
- **`notes`** — shown to the user in the app's update prompt. Keep it short and
  human.
- **`pub_date`** — RFC 3339. Not used for update decisions, but read by tooling
  and by people.
- **`platforms`** — keyed by target triple. For this project that is
  `windows-x86_64`. Add other keys only when there is a signed build for them.
- **`signature`** — the **entire contents** of the `.sig` file, as a single
  string. Not the file name, not a URL, not a truncated hash.
- **`url`** — a direct download URL for the installer, which must be publicly
  reachable without authentication. A `releases/download/...` URL for the tag is
  the correct shape; a URL to a draft or a private asset will fail for users.

The two values that must be right, and are easy to get wrong, are `signature`
and `version`. Everything else is presentation.

---

## Verifying a release locally

Before announcing anything, confirm that a real client will accept what you
published.

1. **Check the manifest is live and parses.** Fetch the endpoint URL in a
   browser or with `Invoke-RestMethod`. If it 404s, the release is a draft, is
   not marked latest, or `latest.json` was not attached as an asset.

   ```powershell
   Invoke-RestMethod "https://github.com/hisyamnurx/Eclink/releases/latest/download/latest.json"
   ```

2. **Check `version` against the installer.** The manifest version, the
   `package.json` version, the `tauri.conf.json` version, and the version shown
   in the running app must all be the same string.

3. **Check the signature matches.** Download the installer from the `url` in the
   manifest and verify the manifest's `signature` value against it with the
   minisign public key committed in `src-tauri/tauri.conf.json`. A mismatch here
   means every client will reject the update.

4. **Do a real update on a clean machine or a clean user account.** Install the
   previous version, launch it, let it check for updates, confirm the prompt
   appears, confirm the update installs, and confirm the version afterwards is
   the new one. Then open the app and confirm the version shown is the new one.

5. **Confirm the update was not applied automatically.** The check should report
   an available update; installing should have required your click. If an update
   installed itself, stop and investigate before announcing.

6. **Remove the temptation to skip this.** A release that installs for you is not
   evidence that it installs for users — you are the one machine guaranteed to
   have the private key material and the right cached state.

---

## If something is wrong after publishing

- **A bad signature or a missing `.sig`:** delete the release asset immediately
  and publish a corrected one. Clients that already failed saw a clear failure
  and changed nothing, which is the intended outcome.
- **A wrong version string:** publish a corrected `latest.json` as the release
  asset. Do not delete and re-upload the tag unless the tag itself is wrong;
  moving a tag breaks anyone who already fetched it.
- **A broken build that installs:** publish a fix as the next patch version
  rather than replacing the artifact in place, so the version in the manifest
  never refers to two different binaries.

The single rule behind all of these: never let the artifact at a given version
change silently. Users and their machines compare versions, and a version that
means two different things makes every future update decision unreliable.
