//! A credential wrapper that cannot be logged by accident (PRD §13).
//!
//! `Secret` deliberately implements neither `Display` nor `Serialize`. Its
//! `Debug` output is a fixed placeholder. Reaching the real value requires an
//! explicit call to [`Secret::expose`], which makes every use of a credential
//! visible in review.

/// A secret value held in memory for the duration of one operation.
#[derive(Clone, Default)]
pub struct Secret(String);

impl Secret {
    pub fn new(value: impl Into<String>) -> Self {
        Self(value.into())
    }

    /// The raw value. Call sites are intentionally few and auditable.
    pub fn expose(&self) -> &str {
        &self.0
    }

    pub fn is_empty(&self) -> bool {
        self.0.trim().is_empty()
    }

    /// Length, for validation messages that must not reveal the value.
    pub fn len(&self) -> usize {
        self.0.len()
    }

    /// A non-reversible hint for the UI, e.g. `****cdef`.
    pub fn fingerprint(&self) -> String {
        crate::diagnostics::fingerprint(&self.0)
    }
}

impl From<&str> for Secret {
    fn from(value: &str) -> Self {
        Self(value.to_string())
    }
}

impl From<String> for Secret {
    fn from(value: String) -> Self {
        Self(value)
    }
}

impl std::fmt::Debug for Secret {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // Never the value, not even a length, in a debug rendering.
        f.write_str("Secret(****)")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn debug_never_reveals_the_value() {
        let secret = Secret::new("sk-super-secret-value");
        let rendered = format!("{secret:?}");
        assert!(!rendered.contains("super-secret"), "{rendered}");
        assert!(!rendered.contains("sk-"), "{rendered}");
        assert_eq!(rendered, "Secret(****)");
    }

    #[test]
    fn expose_returns_the_exact_value() {
        let secret = Secret::new("abc123");
        assert_eq!(secret.expose(), "abc123");
        assert_eq!(secret.len(), 6);
        assert!(!secret.is_empty());
    }

    #[test]
    fn empty_detection_ignores_whitespace() {
        assert!(Secret::new("   ").is_empty());
        assert!(!Secret::new(" a ").is_empty());
    }

    #[test]
    fn fingerprint_masks_short_values_entirely() {
        // A 4-character key must not round-trip in full through the UI.
        let short = Secret::new("abcd").fingerprint();
        assert!(!short.contains("abcd"), "{short}");

        let long = Secret::new("sk-abcdefghijklmnop").fingerprint();
        assert!(long.starts_with("****"), "{long}");
        assert!(long.ends_with("mnop"), "{long}");
    }
}
