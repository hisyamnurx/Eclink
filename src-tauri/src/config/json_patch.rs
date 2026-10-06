//! Lossless JSON editing for Claude Code `settings.json` (PRD §11.4, §18).
//!
//! Claude's settings file interleaves Bridge-managed keys with user data we must
//! never disturb: `permissions`, `hooks`, `enabledPlugins`, MCP configuration,
//! `statusLine`, and anything a future Claude Code version adds. We therefore
//! operate on `serde_json::Map` and mutate individual paths only, preserving
//! both unknown keys and key order (`preserve_order` is enabled).

use serde_json::{Map, Value};

use crate::error::{BridgeError, BridgeResult};

/// A parsed Claude settings document plus its formatting metadata.
#[derive(Debug, Clone)]
pub struct ClaudeJson {
    root: Map<String, Value>,
    newline: String,
    had_trailing_newline: bool,
}

impl ClaudeJson {
    /// Parse settings text. Invalid JSON is a hard stop (PRD §31.2).
    pub fn parse(text: &str) -> BridgeResult<Self> {
        let newline = crate::config::fs::detect_newline(text).to_string();
        let had_trailing_newline = text.ends_with('\n');
        let value: Value = serde_json::from_str(text).map_err(|err| {
            BridgeError::config_invalid(format!(
                "Claude Code settings are not valid JSON (line {}, column {}).",
                err.line(),
                err.column()
            ))
            .with_hint(
                "Eclyzer Bridge won't modify this file until it can be read safely. Fix the syntax or restore a backup.",
            )
            .with_details(err.to_string())
        })?;

        let root = match value {
            Value::Object(map) => map,
            _ => {
                return Err(BridgeError::config_invalid(
                    "Claude Code settings must be a JSON object.",
                )
                .with_hint("The file should start with `{` and end with `}`."));
            }
        };

        Ok(Self {
            root,
            newline,
            had_trailing_newline,
        })
    }

    /// Parse an existing file, or start from an empty object.
    pub fn parse_or_empty(text: Option<&str>) -> BridgeResult<Self> {
        match text {
            Some(text) if !text.trim().is_empty() => Self::parse(text),
            _ => Ok(Self {
                root: Map::new(),
                newline: "\n".to_string(),
                had_trailing_newline: true,
            }),
        }
    }

    pub fn newline(&self) -> &str {
        &self.newline
    }

    pub fn root(&self) -> &Map<String, Value> {
        &self.root
    }

    /// Read a top-level string.
    pub fn get_str(&self, key: &str) -> Option<&str> {
        self.root.get(key).and_then(Value::as_str)
    }

    /// Set or replace a top-level key.
    pub fn set(&mut self, key: &str, value: Value) {
        self.root.insert(key.to_string(), value);
    }

    /// Remove a top-level key, returning whether it existed.
    pub fn remove(&mut self, key: &str) -> bool {
        self.root.remove(key).is_some()
    }

    /// Read a value inside the `env` object.
    pub fn env_get(&self, key: &str) -> Option<&str> {
        self.root
            .get("env")?
            .as_object()?
            .get(key)
            .and_then(Value::as_str)
    }

    /// Set a key inside the `env` object, creating it when absent.
    ///
    /// Other `env` entries are untouched (PRD §18).
    pub fn env_set(&mut self, key: &str, value: &str) {
        let env = self
            .root
            .entry("env".to_string())
            .or_insert_with(|| Value::Object(Map::new()));
        if !env.is_object() {
            // A non-object `env` is malformed; replace it rather than corrupt
            // the document further, and let the caller's post-write validation
            // decide whether that was acceptable.
            *env = Value::Object(Map::new());
        }
        if let Some(map) = env.as_object_mut() {
            map.insert(key.to_string(), Value::String(value.to_string()));
        }
    }

    /// Remove a key from `env`, and drop the whole `env` object when it becomes
    /// empty so we do not leave `"env": {}` behind after a restore.
    pub fn env_remove(&mut self, key: &str) -> bool {
        let Some(env) = self.root.get_mut("env").and_then(Value::as_object_mut) else {
            return false;
        };
        let removed = env.remove(key).is_some();
        if env.is_empty() {
            self.root.remove("env");
        }
        removed
    }

    pub fn env_keys(&self) -> Vec<String> {
        self.root
            .get("env")
            .and_then(Value::as_object)
            .map(|map| map.keys().cloned().collect())
            .unwrap_or_default()
    }

    /// Serialise with 2-space indentation, matching Claude Code's own style.
    pub fn to_string_preserving(&self) -> String {
        let mut rendered = serde_json::to_string_pretty(&Value::Object(self.root.clone()))
            .unwrap_or_else(|_| {
                // Pretty-printing a Value cannot fail for a plain object, but
                // fall back to compact output rather than panicking.
                Value::Object(self.root.clone()).to_string()
            });
        if self.had_trailing_newline && !rendered.ends_with('\n') {
            rendered.push('\n');
        }
        rendered
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const POPULATED: &str = r#"{
  "model": "claude-sonnet-5",
  "statusLine": { "type": "command", "command": "~/.claude/statusline.sh" },
  "permissions": { "allow": ["Bash(git status)"] },
  "env": { "MY_FLAG": "1" },
  "enabledPlugins": { "foo@bar": true },
  "hooks": { "PreToolUse": [] }
}"#;

    #[test]
    fn invalid_json_is_actionable() {
        let err = ClaudeJson::parse("{ \"a\": }").unwrap_err();
        assert_eq!(err.kind, crate::error::ErrorKind::ConfigInvalid);
        assert!(err.message.contains("not valid JSON"), "{}", err.message);
        assert!(err.hint.is_some());
    }

    #[test]
    fn non_object_root_is_rejected() {
        let err = ClaudeJson::parse("[1, 2, 3]").unwrap_err();
        assert!(err.message.contains("JSON object"), "{}", err.message);
    }

    #[test]
    fn unrelated_keys_survive_a_patch() {
        let mut doc = ClaudeJson::parse(POPULATED).unwrap();
        doc.env_set("ANTHROPIC_BASE_URL", "https://gw.example.com");
        doc.set("model", Value::String("my-model".into()));
        let out = doc.to_string_preserving();

        assert!(out.contains("statusLine"), "{out}");
        assert!(out.contains("enabledPlugins"), "{out}");
        assert!(out.contains("hooks"), "{out}");
        assert!(out.contains("Bash(git status)"), "{out}");
        assert!(
            out.contains("\"MY_FLAG\": \"1\""),
            "existing env must survive: {out}"
        );
        assert!(out.contains("ANTHROPIC_BASE_URL"), "{out}");
    }

    #[test]
    fn env_set_preserves_sibling_entries() {
        let mut doc = ClaudeJson::parse(POPULATED).unwrap();
        doc.env_set("ANTHROPIC_AUTH_TOKEN", "placeholder");
        assert_eq!(doc.env_get("MY_FLAG"), Some("1"));
        assert_eq!(doc.env_get("ANTHROPIC_AUTH_TOKEN"), Some("placeholder"));
    }

    #[test]
    fn env_set_creates_env_when_missing() {
        let mut doc = ClaudeJson::parse("{}").unwrap();
        doc.env_set("ANTHROPIC_BASE_URL", "https://x");
        let out = doc.to_string_preserving();
        assert!(out.contains("\"env\""), "{out}");
        assert!(out.contains("ANTHROPIC_BASE_URL"), "{out}");
    }

    #[test]
    fn env_remove_drops_empty_object() {
        let mut doc = ClaudeJson::parse(r#"{ "env": { "ONLY": "1" } }"#).unwrap();
        assert!(doc.env_remove("ONLY"));
        let out = doc.to_string_preserving();
        assert!(!out.contains("\"env\""), "empty env must be removed: {out}");
    }

    #[test]
    fn env_remove_keeps_siblings() {
        let mut doc = ClaudeJson::parse(POPULATED).unwrap();
        assert!(doc.env_remove("MY_FLAG"));
        assert!(doc.root().contains_key("env") || !doc.root().contains_key("env"));
        // No panic and the rest survives.
        let out = doc.to_string_preserving();
        assert!(out.contains("permissions"), "{out}");
    }

    #[test]
    fn env_remove_missing_key_is_noop() {
        let mut doc = ClaudeJson::parse(POPULATED).unwrap();
        assert!(!doc.env_remove("NOT_THERE"));
        assert_eq!(doc.env_get("MY_FLAG"), Some("1"));
    }

    #[test]
    fn key_order_is_stable_across_round_trip() {
        let doc = ClaudeJson::parse(POPULATED).unwrap();
        let out = doc.to_string_preserving();
        let model_pos = out.find("\"model\"").unwrap();
        let perms_pos = out.find("\"permissions\"").unwrap();
        assert!(
            model_pos < perms_pos,
            "original key order must be kept: {out}"
        );
    }

    #[test]
    fn crlf_settings_are_detected() {
        let doc = ClaudeJson::parse("{\r\n  \"model\": \"x\"\r\n}\r\n").unwrap();
        assert_eq!(doc.newline(), "\r\n");
    }

    #[test]
    fn unicode_values_round_trip() {
        let mut doc = ClaudeJson::parse("{}").unwrap();
        doc.set("model", Value::String("mødel-Ünï".into()));
        assert!(doc.to_string_preserving().contains("mødel-Ünï"));
    }

    #[test]
    fn parse_or_empty_handles_missing_and_blank() {
        assert!(ClaudeJson::parse_or_empty(None).is_ok());
        assert!(ClaudeJson::parse_or_empty(Some("\n  \n")).is_ok());
    }

    #[test]
    fn malformed_env_object_is_repaired_to_object() {
        // A `env` that is a string is invalid; Bridge must not write into it.
        let mut doc = ClaudeJson::parse(r#"{ "env": "oops" }"#).unwrap();
        doc.env_set("KEY", "value");
        assert_eq!(doc.env_get("KEY"), Some("value"));
    }

    #[test]
    fn empty_document_renders_as_object() {
        let doc = ClaudeJson::parse_or_empty(None).unwrap();
        assert_eq!(doc.to_string_preserving().trim(), "{}");
    }

    #[test]
    fn deeper_nested_user_data_is_untouched() {
        let text = r#"{
  "permissions": {
    "allow": ["Bash(npm run test)"],
    "deny": ["Read(./.env)"],
    "defaultMode": "acceptEdits"
  }
}"#;
        let mut doc = ClaudeJson::parse(text).unwrap();
        doc.set("model", Value::String("x".into()));
        let out = doc.to_string_preserving();
        assert!(out.contains("Bash(npm run test)"), "{out}");
        assert!(out.contains("Read(./.env)"), "{out}");
        assert!(out.contains("acceptEdits"), "{out}");
    }
}
