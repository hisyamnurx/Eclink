//! Encrypted local secret storage (PRD §13, §44.3).
//!
//! API keys never touch `profiles.json`, logs, backups, or the repository. They
//! live in `secrets.dat` as a versioned JSON envelope whose records are sealed
//! with Windows DPAPI scoped to the current user.
//!
//! Security boundary, stated plainly: DPAPI protects secrets **at rest**. It
//! does not protect them from malware already running as the same Windows user.

pub mod crypto;
pub mod store;

pub use crypto::{Cipher, DpapiCipher, PlaintextCipher};
pub use store::{SecretStore, SecretStoreData, SECRETS_SCHEMA_VERSION};

/// Human name of the encryption backend protecting stored secrets.
///
/// Shown in Settings and in the diagnostics report so a user can tell at a
/// glance whether their keys are actually encrypted.
pub fn backend_name() -> &'static str {
    if cfg!(target_os = "windows") {
        "Windows DPAPI (Current User)"
    } else {
        "Unsupported platform (secrets are not protected)"
    }
}
