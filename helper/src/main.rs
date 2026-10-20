//! `eclink-helper` — stateless credential resolver.
//!
//! Contract (PRD §13.4):
//!
//! ```text
//! eclink-helper secret resolve <profile-id>
//! ```
//!
//! Behaviour:
//! 1. read the encrypted secret record from `secrets.dat`;
//! 2. ask Windows DPAPI (Current User) to decrypt it;
//! 3. write **only the secret** to stdout;
//! 4. write nothing sensitive to stderr;
//! 5. log nothing, cache nothing, and exit immediately.
//!
//! Codex invokes this through `[model_providers.<id>.auth]`, so the token never
//! has to be written into `config.toml` (PRD §16.5).
//!
//! This binary intentionally does not link the main application crate: it must
//! stay small, audit-friendly, and free of Tauri/UI dependencies.

#![cfg_attr(not(debug_assertions), windows_subsystem = "console")]

use std::io::Write;
use std::path::PathBuf;

use base64::Engine as _;
use serde::Deserialize;

/// Exit code used for every failure. Callers only care that no token was
/// produced; the reason goes to stderr (never a secret).
const EXIT_FAILURE: i32 = 1;

#[derive(Debug, Deserialize)]
struct SecretStoreData {
    #[serde(default)]
    records: std::collections::BTreeMap<String, SecretRecord>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct SecretRecord {
    #[allow(dead_code)]
    #[serde(default)]
    created_at: String,
    #[allow(dead_code)]
    #[serde(default)]
    updated_at: String,
    blob: String,
}

fn main() {
    let code = run();
    std::process::exit(code);
}

fn run() -> i32 {
    let args: Vec<String> = std::env::args().skip(1).collect();

    match args.as_slice() {
        [scope, action, id] if scope == "secret" && action == "resolve" => match resolve(id) {
            Ok(secret) => {
                // stdout carries the token. No trailing decoration: Codex trims
                // surrounding whitespace itself, and a bare value is the
                // documented contract.
                let mut stdout = std::io::stdout();
                if stdout.write_all(secret.as_bytes()).is_err() {
                    fail("could not write the credential to stdout");
                    return EXIT_FAILURE;
                }
                let _ = stdout.flush();
                0
            }
            Err(message) => {
                fail(&message);
                EXIT_FAILURE
            }
        },
        _ => {
            fail("usage: eclink-helper secret resolve <profile-id>");
            EXIT_FAILURE
        }
    }
}

/// Write a diagnostic and return. Never includes secret material.
fn fail(message: &str) {
    let _ = writeln!(std::io::stderr(), "eclink-helper: {message}");
}

/// The data directory name shared with the main application.
///
/// Deliberately still `EclyzerBridge`: this is the directory every existing
/// installation already writes its profiles, snapshots and `secrets.dat` into.
/// Renaming it would strand that data. See `docs/legacy-compatibility.md`.
const APP_DATA_DIR_NAME: &str = "EclyzerBridge";

/// Environment override used by tests and by a non-default install location.
///
/// The legacy variable name is still read so an existing Codex `auth.command`
/// that passes `ECLYZER_BRIDGE_DATA_DIR` keeps resolving credentials.
const LEGACY_DATA_DIR_ENV: &str = "ECLYZER_BRIDGE_DATA_DIR";

/// Locate `secrets.dat` next to the application data root.
fn secrets_path() -> Result<PathBuf, String> {
    // An explicit override keeps the helper usable from tests and from a
    // non-default install location.
    if let Ok(path) = std::env::var(LEGACY_DATA_DIR_ENV) {
        let trimmed = path.trim();
        if !trimmed.is_empty() {
            return Ok(PathBuf::from(trimmed).join("secrets.dat"));
        }
    }
    let base = directories::BaseDirs::new()
        .ok_or_else(|| "could not determine the application data directory".to_string())?;
    Ok(base
        .data_local_dir()
        .join(APP_DATA_DIR_NAME)
        .join("secrets.dat"))
}

fn resolve(id: &str) -> Result<String, String> {
    if id.trim().is_empty() {
        return Err("a profile id is required".to_string());
    }

    let path = secrets_path()?;
    let raw =
        std::fs::read_to_string(&path).map_err(|_| format!("could not read {}", path.display()))?;

    let data: SecretStoreData =
        serde_json::from_str(&raw).map_err(|_| "the secret store is unreadable".to_string())?;

    let record = data
        .records
        .get(id)
        .ok_or_else(|| "no stored credential for this profile".to_string())?;

    let sealed = base64::engine::general_purpose::STANDARD
        .decode(record.blob.as_bytes())
        .map_err(|_| "the stored credential is damaged".to_string())?;

    let plaintext = unprotect(&sealed)?;

    String::from_utf8(plaintext).map_err(|_| "the stored credential is not valid text".to_string())
}

/// DPAPI Current User unprotect.
#[cfg(windows)]
fn unprotect(sealed: &[u8]) -> Result<Vec<u8>, String> {
    use windows::Win32::Foundation::{LocalFree, HLOCAL};
    use windows::Win32::Security::Cryptography::{
        CryptUnprotectData, CRYPTPROTECT_UI_FORBIDDEN, CRYPT_INTEGER_BLOB,
    };

    let input = CRYPT_INTEGER_BLOB {
        cbData: sealed.len() as u32,
        pbData: sealed.as_ptr() as *mut u8,
    };
    let mut output = CRYPT_INTEGER_BLOB::default();

    // SAFETY: `input` points at a live slice for the duration of the call.
    let ok = unsafe {
        CryptUnprotectData(
            &input,
            None,
            None,
            None,
            None,
            CRYPTPROTECT_UI_FORBIDDEN,
            &mut output,
        )
    };

    if ok.is_err() {
        return Err("could not decrypt the credential for this Windows user".to_string());
    }

    // SAFETY: DPAPI returned success, so `output` is a valid allocation we own.
    let owned = unsafe {
        let slice = std::slice::from_raw_parts(output.pbData, output.cbData as usize);
        let owned = slice.to_vec();
        let _ = LocalFree(Some(HLOCAL(output.pbData as *mut core::ffi::c_void)));
        owned
    };

    Ok(owned)
}

#[cfg(not(windows))]
fn unprotect(_sealed: &[u8]) -> Result<Vec<u8>, String> {
    Err("credential decryption requires Windows".to_string())
}
