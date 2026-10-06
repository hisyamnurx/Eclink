//! DPAPI encryption abstraction.
//!
//! The [`Cipher`] trait exists so the secret store can be unit-tested on any
//! platform and so a future non-Windows port has one seam to fill. The Windows
//! implementation uses `CryptProtectData`/`CryptUnprotectData` with the
//! `CRYPTPROTECT_UI_FORBIDDEN` flag (no UI prompt) and **no** additional
//! entropy, matching the PRD's "Current User scope, no setup" requirement.

use crate::error::{BridgeError, BridgeResult};

/// Symmetric sealing of small secret payloads.
pub trait Cipher: Send + Sync {
    /// Seal plaintext bytes into an opaque blob.
    fn seal(&self, plaintext: &[u8]) -> BridgeResult<Vec<u8>>;
    /// Open a blob produced by [`Cipher::seal`].
    fn open(&self, sealed: &[u8]) -> BridgeResult<Vec<u8>>;
    /// Human-readable backend name, surfaced in diagnostics.
    fn name(&self) -> &'static str;
}

/// Windows DPAPI, Current User scope.
#[cfg(windows)]
pub struct DpapiCipher;

#[cfg(windows)]
impl DpapiCipher {
    pub fn new() -> Self {
        Self
    }
}

#[cfg(windows)]
impl Default for DpapiCipher {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(windows)]
mod imp {
    use super::*;
    use windows::Win32::Foundation::{LocalFree, HLOCAL};
    use windows::Win32::Security::Cryptography::{
        CryptProtectData, CryptUnprotectData, CRYPTPROTECT_UI_FORBIDDEN, CRYPT_INTEGER_BLOB,
    };
    fn blob_from(bytes: &[u8]) -> CRYPT_INTEGER_BLOB {
        CRYPT_INTEGER_BLOB {
            cbData: bytes.len() as u32,
            pbData: bytes.as_ptr() as *mut u8,
        }
    }

    /// Copy an output blob into an owned Vec and free the DPAPI allocation.
    unsafe fn take_blob(out: &CRYPT_INTEGER_BLOB) -> Vec<u8> {
        let slice = std::slice::from_raw_parts(out.pbData, out.cbData as usize);
        let owned = slice.to_vec();
        let _ = LocalFree(Some(HLOCAL(out.pbData as *mut core::ffi::c_void)));
        owned
    }

    impl Cipher for DpapiCipher {
        fn seal(&self, plaintext: &[u8]) -> BridgeResult<Vec<u8>> {
            let input = blob_from(plaintext);
            let mut output = CRYPT_INTEGER_BLOB::default();

            // SAFETY: `input` points at a live slice for the duration of the
            // call; DPAPI copies what it needs before returning.
            let ok = unsafe {
                CryptProtectData(
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
                return Err(BridgeError::new(
                    crate::error::ErrorKind::Secret,
                    "Could not encrypt the API key.",
                )
                .with_hint("Windows Data Protection is unavailable for this user account."));
            }

            // SAFETY: DPAPI returned success, so `output` is a valid allocation
            // owned by us and released by `take_blob`.
            Ok(unsafe { take_blob(&output) })
        }

        fn open(&self, sealed: &[u8]) -> BridgeResult<Vec<u8>> {
            let input = blob_from(sealed);
            let mut output = CRYPT_INTEGER_BLOB::default();

            // SAFETY: as above; `sealed` outlives the call.
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
                return Err(BridgeError::new(
                    crate::error::ErrorKind::Secret,
                    "Could not decrypt the stored API key.",
                )
                .with_hint(
                    "The secret was encrypted for a different Windows user account or the file was modified.",
                ));
            }

            // SAFETY: as above.
            Ok(unsafe { take_blob(&output) })
        }

        fn name(&self) -> &'static str {
            "dpapi-current-user"
        }
    }
}

// On Windows the `Cipher` implementation for `DpapiCipher` lives in `imp`
// above; it is an inherent trait impl, so no glob re-export is needed.

/// Non-Windows fallback so the crate compiles and tests run elsewhere.
///
/// It is intentionally **not** secure and must never ship to users; the app
/// only builds for Windows in v0.1. Tests on other platforms use
/// [`PlaintextCipher`] explicitly instead.
#[cfg(not(windows))]
pub struct DpapiCipher;

#[cfg(not(windows))]
impl DpapiCipher {
    pub fn new() -> Self {
        Self
    }
}

#[cfg(not(windows))]
impl Default for DpapiCipher {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(not(windows))]
impl Cipher for DpapiCipher {
    fn seal(&self, _plaintext: &[u8]) -> BridgeResult<Vec<u8>> {
        Err(BridgeError::new(
            crate::error::ErrorKind::Secret,
            "Encrypted secret storage requires Windows.",
        ))
    }

    fn open(&self, _sealed: &[u8]) -> BridgeResult<Vec<u8>> {
        Err(BridgeError::new(
            crate::error::ErrorKind::Secret,
            "Encrypted secret storage requires Windows.",
        ))
    }

    fn name(&self) -> &'static str {
        "unsupported"
    }
}

/// Reversible obfuscation used only by tests and the fixture harness so the
/// store's logic can be exercised without touching the real DPAPI store.
#[derive(Debug, Default)]
pub struct PlaintextCipher;

impl PlaintextCipher {
    pub fn new() -> Self {
        Self
    }
}

impl Cipher for PlaintextCipher {
    fn seal(&self, plaintext: &[u8]) -> BridgeResult<Vec<u8>> {
        // XOR with a fixed mask: reversible, obviously not security, and it
        // still proves the store goes through the cipher seam.
        Ok(plaintext.iter().map(|b| b ^ 0x5A).collect())
    }

    fn open(&self, sealed: &[u8]) -> BridgeResult<Vec<u8>> {
        Ok(sealed.iter().map(|b| b ^ 0x5A).collect())
    }

    fn name(&self) -> &'static str {
        "plaintext-test-only"
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plaintext_cipher_round_trips() {
        let cipher = PlaintextCipher::new();
        let secret = b"sk-test-0123456789";
        let sealed = cipher.seal(secret).unwrap();
        assert_ne!(&sealed[..], &secret[..], "sealing must transform bytes");
        assert_eq!(cipher.open(&sealed).unwrap(), secret);
    }

    #[cfg(windows)]
    #[test]
    fn dpapi_round_trips() {
        let cipher = DpapiCipher::new();
        let secret = b"sk-dpapi-roundtrip-check";
        let sealed = cipher.seal(secret).unwrap();
        assert_ne!(&sealed[..], &secret[..]);
        assert_eq!(cipher.open(&sealed).unwrap(), secret);
    }

    #[cfg(windows)]
    #[test]
    fn dpapi_rejects_tampered_blob() {
        let cipher = DpapiCipher::new();
        let mut sealed = cipher.seal(b"sk-tamper").unwrap();
        let last = sealed.len() - 1;
        sealed[last] ^= 0xFF;
        assert!(
            cipher.open(&sealed).is_err(),
            "tampered blob must not decrypt"
        );
    }
}
