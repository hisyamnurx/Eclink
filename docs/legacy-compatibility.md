# Legacy compatibility identifiers

Eclink was previously released as **Eclyzer Bridge**. The rename is a public
branding change; several internal identifiers deliberately keep their old
spelling because existing installations depend on them.

This document exists so a future maintainer does not "clean up" an old string and
silently break every upgraded installation. Each entry records the identifier,
why it is retained, what depends on it, and whether it may ever be migrated.

**Rule of thumb:** a string that appears in a file Eclink does not own, or in data
that was encrypted or written before the rename, is not a branding string. It is
a compatibility contract.

## Retained identifiers

### `EclyzerBridge` - application data directory

| | |
| --- | --- |
| **Value** | `%LOCALAPPDATA%\EclyzerBridge` |
| **Defined in** | `src-tauri/src/adapters/mod.rs` (`APP_DATA_DIR_NAME`), `helper/src/main.rs` (`APP_DATA_DIR_NAME`) |
| **Why retained** | Every existing installation already stores `app.json`, `profiles.json`, `secrets.dat`, `backups/`, `logs/` and `cache/` here. |
| **What depends on it** | Profiles, app settings (including the language preference), the DPAPI secret blob, snapshot and history ledgers, and the credential helper's own path resolution. |
| **If it were renamed** | An upgraded install would start empty. The DPAPI blob would be stranded in the old directory, so every API key would appear to be missing and the user would have to re-enter all of them. |
| **May it be migrated later?** | Only with a transactional, verified, reversible migration that copies data, validates that secrets still decrypt, and leaves the original untouched. Not worth the risk for a directory name. |

Note the deliberate distinction between the **data** directory (`EclyzerBridge`,
no space) and the **install** directory (`Eclink`). `install_survival.rs` asserts
they are different names, so an uninstall cleanup can never reach user data.

### `eclyzer_bridge_` - managed provider prefix

| | |
| --- | --- |
| **Value** | `eclyzer_bridge_` |
| **Defined in** | `src-tauri/src/lib.rs` (`MANAGED_PROVIDER_PREFIX`) |
| **Why retained** | It is the ownership marker written into the user's `config.toml` as `[model_providers.eclyzer_bridge_<slug>]`. |
| **What depends on it** | `restore_default` removes exactly the providers carrying this prefix; `get_active_profile` recognizes an active provider by it; the Codex adapter's ownership check and key list are string comparisons against it. |
| **If it were renamed** | Eclink would treat its own previously written providers as user-owned. `restore_default` would stop removing them, and an existing active provider would no longer be recognized as managed, so the UI would report the user's own configuration as active. |
| **May it be migrated later?** | Only by rewriting every managed provider entry in every user's `config.toml` while preserving comments and key order. The prefix is invisible to users, so there is no benefit. |

### `ECLYZER_BRIDGE_DATA_DIR` - data directory override

| | |
| --- | --- |
| **Value** | `ECLYZER_BRIDGE_DATA_DIR` |
| **Defined in** | `helper/src/main.rs` (`LEGACY_DATA_DIR_ENV`) |
| **Why retained** | The credential helper reads this to locate `secrets.dat`. A Codex `auth.command` written before the rename may still pass it. |
| **What depends on it** | Integration tests (`helper_roundtrip.rs`), and any pre-existing configuration that launches the helper with an explicit data directory. |
| **If it were renamed** | A configuration that passes the old variable would fall through to the default directory. That happens to resolve to the same place today, but it would silently ignore an explicit override. |
| **May it be migrated later?** | A new variable could be added alongside it. The old one should keep working. |

### `eclyzer-bridge-helper.exe` - legacy helper executable name

| | |
| --- | --- |
| **Value** | `eclyzer-bridge-helper.exe` |
| **Defined in** | `src-tauri/src/app/state.rs` (`LEGACY_FILE_NAME`, probed as a fallback candidate) |
| **Why retained** | An installation upgraded in place may still have the old binary on disk, and a `config.toml` written by the old app contains an **absolute path** to it inside `[model_providers.*.auth].command`. |
| **What depends on it** | Credential resolution for Codex on any machine upgraded from Eclyzer Bridge. |
| **If it were removed from the candidate list** | Eclink would report the helper as missing and silently fall back to an environment-variable key, which is a visible behaviour change for an existing user. |
| **May it be migrated later?** | The new build ships `eclink-helper.exe` and probes it first. The old name stays as a fallback; it costs one `Path::exists` call. |

### `com.eclyzer.bridge` - Tauri bundle identifier

| | |
| --- | --- |
| **Value** | `com.eclyzer.bridge` |
| **Defined in** | `src-tauri/tauri.conf.json` (`identifier`) |
| **Why retained** | The identifier is the Windows application identity used for the WebView2 data directory, per-app paths, and installer bookkeeping. |
| **What depends on it** | The existing installation's identity on disk and in the registry. |
| **If it were renamed** | Windows would treat Eclink as a different application. The WebView2 profile directory would change, and the installer would no longer recognize the previous installation. |
| **May it be migrated later?** | Only as part of a deliberate break, with an explicit uninstall/install path. A branding change is not a reason. |

### `Eclyzer Bridge` - legacy NSIS uninstall registry key

| | |
| --- | --- |
| **Value** | `HKCU\Software\Microsoft\Windows\CurrentVersion\Uninstall\Eclyzer Bridge` |
| **Referenced in** | `src-tauri/installer-hooks.nsh` |
| **Why retained** | Tauri's NSIS template derives the uninstall key from `${PRODUCTNAME}`, so the previous release registered itself under this exact key. |
| **What depends on it** | Removing the stale entry after an upgrade, so Add/Remove Programs does not list both products. |
| **If it were ignored** | Users who upgrade would see two entries for one application and could uninstall the wrong one. |
| **May it be migrated later?** | It is deleted by the installer once the old executable is gone. Until every user has upgraded, the reference must stay. |

## Not retained: what did change

These were public branding and carry no compatibility obligation:

| Old | New |
| --- | --- |
| Product name, window title, sidebar wordmark | `Eclink` |
| `eclyzer-bridge` package / binary | `eclink` |
| `eclyzer_bridge_lib` library crate | `eclink_lib` |
| `eclyzer-bridge-helper` crate and binary | `eclink-helper` |
| `eclyzer-bridge.log` log file | `eclink.log` |
| `Eclyzer Bridge contributors` | `Eclink contributors` |
| GitHub `hisyamnurx/Eclyzer-Bridge` | `hisyamnurx/Eclink` |
| Installer `Eclyzer-Bridge-Setup-*.exe` | `Eclink-Setup-*.exe` |
| Install directory `%LOCALAPPDATA%\Eclyzer Bridge` | `%LOCALAPPDATA%\Eclink` |

Existing log files are **not** rewritten. Old logs keep their old name and are
pruned by the normal 7-day retention, so nothing historical is falsified.
