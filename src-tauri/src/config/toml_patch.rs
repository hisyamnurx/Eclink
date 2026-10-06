//! Lossless TOML editing for Codex `config.toml` (PRD §11.4, §16).
//!
//! `toml_edit` keeps comments, key order, whitespace, and unrelated tables
//! intact. Bridge only ever sets or removes the specific keys it owns, and
//! never regenerates the document from a template.

use toml_edit::{DocumentMut, Item, Table, Value};

use crate::error::{BridgeError, BridgeResult};

/// A parsed Codex config plus the metadata needed to write it back faithfully.
#[derive(Debug, Clone)]
pub struct CodexToml {
    doc: DocumentMut,
    newline: String,
    had_trailing_newline: bool,
}

impl CodexToml {
    /// Parse config text, preserving line-ending style.
    ///
    /// An unparseable file is a hard stop: Bridge refuses to repair or
    /// overwrite it (PRD §31.2).
    pub fn parse(text: &str) -> BridgeResult<Self> {
        let newline = crate::config::fs::detect_newline(text).to_string();
        let had_trailing_newline = text.ends_with('\n');
        let doc: DocumentMut = text.parse().map_err(|err: toml_edit::TomlError| {
            let (line, col) = err
                .span()
                .map(|span| {
                    let prefix = &text[..span.start.min(text.len())];
                    let line = prefix.matches('\n').count() + 1;
                    let col = prefix.rsplit('\n').next().map_or(1, |s| s.chars().count() + 1);
                    (line, col)
                })
                .unwrap_or((0, 0));
            let location = if line > 0 {
                format!(" (line {line}, column {col})")
            } else {
                String::new()
            };
            BridgeError::config_invalid(format!("Codex configuration is not valid TOML{location}."))
                .with_hint(
                    "Eclyzer Bridge won't modify this file until it can be read safely. Fix the syntax or restore a backup.",
                )
                .with_details(err.to_string())
        })?;
        Ok(Self {
            doc,
            newline,
            had_trailing_newline,
        })
    }

    /// Parse an existing file, or start from an empty document.
    pub fn parse_or_empty(text: Option<&str>) -> BridgeResult<Self> {
        match text {
            Some(text) if !text.trim().is_empty() => Self::parse(text),
            _ => Ok(Self {
                doc: DocumentMut::new(),
                newline: "\n".to_string(),
                had_trailing_newline: true,
            }),
        }
    }

    pub fn newline(&self) -> &str {
        &self.newline
    }

    /// Read a top-level string key.
    pub fn get_str(&self, key: &str) -> Option<&str> {
        self.doc.get(key).and_then(Item::as_str)
    }

    /// Set a top-level string key, preserving surrounding formatting.
    pub fn set_str(&mut self, key: &str, value: &str) {
        set_string_item(self.doc.as_table_mut(), key, value);
    }

    /// Remove a top-level key entirely.
    ///
    /// Removing the document's first key would discard the leading comment that
    /// `toml_edit` stores on its decor, so that comment is transferred to the
    /// new first key (PRD §16.2).
    pub fn remove(&mut self, key: &str) -> bool {
        let was_first = self
            .doc
            .as_table()
            .iter()
            .next()
            .is_some_and(|(name, _)| name == key);

        let carried_prefix = if was_first {
            self.doc
                .get(key)
                .and_then(Item::as_value)
                .map(|value| value.decor().prefix().cloned())
                .unwrap_or(None)
        } else {
            None
        };

        let removed = self.doc.remove(key).is_some();

        if let (true, Some(prefix)) = (removed, carried_prefix) {
            let first_name = self
                .doc
                .as_table()
                .iter()
                .next()
                .map(|(name, _)| name.to_string());
            if let Some(name) = first_name {
                if let Some(value) = self.doc.get_mut(&name).and_then(Item::as_value_mut) {
                    value.decor_mut().set_prefix(prefix);
                }
            }
        }

        removed
    }

    pub fn contains(&self, key: &str) -> bool {
        self.doc.contains_key(key)
    }

    /// All keys of the `model_providers` table, if present.
    pub fn provider_ids(&self) -> Vec<String> {
        self.doc
            .get("model_providers")
            .and_then(Item::as_table_like)
            .map(|table| table.iter().map(|(key, _)| key.to_string()).collect())
            .unwrap_or_default()
    }

    /// Whether a `[model_providers.<id>]` entry exists.
    pub fn has_provider(&self, id: &str) -> bool {
        self.provider_table(id).is_some()
    }

    /// Read a string field from a provider entry, e.g. `base_url`.
    pub fn provider_str(&self, id: &str, field: &str) -> Option<String> {
        self.provider_table(id)?
            .get(field)
            .and_then(Item::as_str)
            .map(str::to_string)
    }

    /// Read a string field from a provider's nested `auth` table, e.g.
    /// `command`.
    pub fn provider_auth_str(&self, id: &str, field: &str) -> Option<String> {
        self.provider_table(id)?
            .get("auth")?
            .as_table_like()?
            .get(field)?
            .as_str()
            .map(str::to_string)
    }

    /// Read the `args` array of a provider's `auth` table.
    pub fn provider_auth_args(&self, id: &str) -> Vec<String> {
        self.provider_table(id)
            .and_then(|table| table.get("auth"))
            .and_then(Item::as_table_like)
            .and_then(|auth| auth.get("args"))
            .and_then(Item::as_array)
            .map(|array| {
                array
                    .iter()
                    .filter_map(|value| value.as_str().map(str::to_string))
                    .collect()
            })
            .unwrap_or_default()
    }

    /// Create or replace a Bridge-managed provider entry.
    ///
    /// When an `auth_command` is supplied, a nested `[.. .auth]` table is
    /// written instead of an inline credential, so no secret ever lands in the
    /// file (PRD §13.4, §16.5).
    pub fn upsert_provider(&mut self, id: &str, spec: &ProviderSpec<'_>) {
        // A provider table is a real table with its own header, so it must not
        // be implicit; `provider_table_or_insert` handles both cases.
        let table = self.provider_table_or_insert(id);

        // Reuse the existing decor for every field so an in-place update keeps
        // any comments the user attached to these lines.
        set_string_item(table, "name", spec.name);
        set_string_item(table, "base_url", spec.base_url);
        set_string_item(table, "wire_api", spec.wire_api);

        // Bridge uses a credential helper for managed providers; clear any
        // inline token fields a previous version might have written.
        table.remove("experimental_bearer_token");

        if let Some(env_key) = spec.env_key {
            set_string_item(table, "env_key", env_key);
        } else {
            table.remove("env_key");
        }

        // `auth` is written as a nested table. Build it fresh each time: a
        // partially-updated auth table would be worse than a replaced one.
        table.remove("auth");

        if let Some(command) = &spec.auth_command {
            let mut auth = Table::new();
            set_string_item(&mut auth, "command", command.command);
            let mut args = toml_edit::Array::new();
            for arg in &command.args {
                args.push(*arg);
            }
            auth.insert("args", Item::Value(Value::Array(args)));
            if let Some(timeout) = command.timeout_ms {
                auth.insert(
                    "timeout_ms",
                    Item::Value(Value::Integer(toml_edit::Formatted::new(timeout as i64))),
                );
            }
            // The auth table must render under its own `[.. .auth]` header,
            // never inline, so it reads clearly in the user's config.
            auth.set_implicit(false);
            table.insert("auth", Item::Table(auth));
        }

        if !spec.http_headers.is_empty() {
            let mut headers = toml_edit::InlineTable::new();
            for (name, value) in &spec.http_headers {
                headers.insert(*name, Value::from((*value).to_string()));
            }
            table.insert("http_headers", Item::Value(Value::InlineTable(headers)));
        } else {
            table.remove("http_headers");
        }

        if !spec.query_params.is_empty() {
            let mut params = toml_edit::InlineTable::new();
            for (name, value) in &spec.query_params {
                params.insert(*name, Value::from((*value).to_string()));
            }
            table.insert("query_params", Item::Value(Value::InlineTable(params)));
        } else {
            table.remove("query_params");
        }
    }

    /// Remove a `[model_providers.<id>]` entry, and the parent table if it
    /// becomes empty so we do not leave a stray `[model_providers]` header.
    pub fn remove_provider(&mut self, id: &str) -> bool {
        let Some(parent) = self
            .doc
            .get_mut("model_providers")
            .and_then(Item::as_table_like_mut)
        else {
            return false;
        };
        let removed = parent.remove(id).is_some();
        if removed && parent.is_empty() {
            self.doc.remove("model_providers");
        }
        removed
    }

    fn provider_table(&self, id: &str) -> Option<&dyn toml_edit::TableLike> {
        self.doc
            .get("model_providers")?
            .as_table_like()?
            .get(id)?
            .as_table_like()
    }

    fn provider_table_or_insert(&mut self, id: &str) -> &mut dyn toml_edit::TableLike {
        if !self.doc.contains_key("model_providers") {
            let mut root = Table::new();
            root.set_implicit(true);
            self.doc.insert("model_providers", Item::Table(root));
        }
        let parent = self
            .doc
            .get_mut("model_providers")
            .expect("model_providers just inserted")
            .as_table_like_mut()
            .expect("model_providers is a table");

        if parent.get(id).is_none() {
            parent.insert(id, Item::Table(Table::new()));
        }
        parent
            .get_mut(id)
            .expect("provider just inserted")
            .as_table_like_mut()
            .expect("provider is a table")
    }

    /// Serialise back to text, preserving the original newline style.
    pub fn to_string_preserving(&self) -> String {
        let rendered = self.doc.to_string();
        if self.had_trailing_newline && !rendered.ends_with('\n') {
            format!("{rendered}\n")
        } else {
            rendered
        }
    }
}

/// Description of a provider entry to write.
pub struct ProviderSpec<'a> {
    pub name: &'a str,
    pub base_url: &'a str,
    pub wire_api: &'a str,
    pub env_key: Option<&'a str>,
    pub auth_command: Option<AuthCommand<'a>>,
    pub http_headers: Vec<(&'a str, &'a str)>,
    pub query_params: Vec<(&'a str, &'a str)>,
}

/// Command-backed credential resolver written into the provider table.
pub struct AuthCommand<'a> {
    pub command: &'a str,
    pub args: Vec<&'a str>,
    pub timeout_ms: Option<u64>,
}

/// Set a string key, preserving all surrounding formatting.
///
/// `toml_edit` attaches a document's leading comment to the decor of its **first
/// key**, so replacing that `Item` would silently delete the user's header
/// comment. When the key already exists we therefore mutate the existing
/// `Value` in place, which keeps the key's decor (prefix comment, suffix
/// newline) exactly as the user wrote it (PRD §16.2).
fn set_string_item(table: &mut dyn toml_edit::TableLike, key: &str, value: &str) {
    if let Some(existing) = table.get_mut(key) {
        if let Some(existing_value) = existing.as_value_mut() {
            // Only a string-typed slot is replaced in place; a differently
            // typed value (e.g. a table) must be replaced wholesale.
            if existing_value.is_str() || existing_value.is_inline_table() {
                *existing_value = Value::from(value.to_string());
                return;
            }
        }
    }

    // Key is absent (or holds an incompatible type): insert a new value. For a
    // brand-new key there is no decor to lose.
    let mut new_value = Value::from(value.to_string());
    if let Some(existing) = table.get(key) {
        if let Some(previous) = existing.as_value() {
            *new_value.decor_mut() = previous.decor().clone();
        }
    }
    table.insert(key, Item::Value(new_value));
}

#[cfg(test)]
mod tests {
    use super::*;

    const NORMAL: &str = r#"# My Codex config — do not lose this comment
model = "gpt-5"
approval_policy = "on-request"

[sandbox_workspace_write]
network_access = true

[mcp_servers.context7]
command = "npx"
args = ["-y", "@upstash/context7-mcp"]

[model_providers.my_gateway]
name = "My Gateway"
base_url = "https://gw.example.com/v1"
wire_api = "responses"
"#;

    fn spec() -> ProviderSpec<'static> {
        ProviderSpec {
            name: "DeepSeek",
            base_url: "https://api.deepseek.com/v1",
            wire_api: "responses",
            env_key: None,
            auth_command: None,
            http_headers: vec![],
            query_params: vec![],
        }
    }

    #[test]
    fn parsing_invalid_toml_is_actionable() {
        let err = CodexToml::parse("model = ").unwrap_err();
        assert_eq!(err.kind, crate::error::ErrorKind::ConfigInvalid);
        assert!(err.message.contains("not valid TOML"), "{}", err.message);
        assert!(err.hint.is_some());
        assert!(err.details.is_some(), "detail with line info expected");
    }

    #[test]
    fn parse_or_empty_handles_missing_and_blank() {
        assert!(CodexToml::parse_or_empty(None).is_ok());
        assert!(CodexToml::parse_or_empty(Some("   \n  ")).is_ok());
    }

    #[test]
    fn comments_survive_a_patch() {
        let mut doc = CodexToml::parse(NORMAL).unwrap();
        doc.set_str("model", "deepseek-chat");
        doc.set_str("model_provider", "eclyzer_bridge_deepseek_abcd1234");
        let out = doc.to_string_preserving();

        assert!(
            out.contains("# My Codex config — do not lose this comment"),
            "{out}"
        );
        assert!(out.contains("approval_policy = \"on-request\""), "{out}");
        assert!(out.contains("network_access = true"), "{out}");
        assert!(out.contains("[mcp_servers.context7]"), "{out}");
    }

    #[test]
    fn unrelated_providers_survive() {
        let mut doc = CodexToml::parse(NORMAL).unwrap();
        doc.upsert_provider("eclyzer_bridge_deepseek_abcd1234", &spec());
        let out = doc.to_string_preserving();

        assert!(out.contains("[model_providers.my_gateway]"), "{out}");
        assert!(out.contains("https://gw.example.com/v1"), "{out}");
        assert!(
            out.contains("[model_providers.eclyzer_bridge_deepseek_abcd1234]"),
            "{out}"
        );
    }

    #[test]
    fn upsert_is_idempotent() {
        let mut doc = CodexToml::parse(NORMAL).unwrap();
        doc.upsert_provider("eclyzer_bridge_x_1", &spec());
        let first = doc.to_string_preserving();
        doc.upsert_provider("eclyzer_bridge_x_1", &spec());
        let second = doc.to_string_preserving();
        assert_eq!(
            first, second,
            "re-applying the same profile must be a no-op"
        );
    }

    #[test]
    fn crlf_is_preserved() {
        let crlf = "# c\r\nmodel = \"gpt-5\"\r\n";
        let mut doc = CodexToml::parse(crlf).unwrap();
        assert_eq!(doc.newline(), "\r\n");
        doc.set_str("model", "x");
        assert!(doc.to_string_preserving().ends_with('\n'));
    }

    #[test]
    fn utf8_content_round_trips() {
        let text = "# Ünïcødé comment ✓\nmodel = \"gpt-5\"\n";
        let doc = CodexToml::parse(text).unwrap();
        let out = doc.to_string_preserving();
        assert!(out.contains("Ünïcødé comment ✓"), "{out}");
    }

    #[test]
    fn remove_provider_cleans_up_empty_parent() {
        let text = "[model_providers.only_one]\nname = \"X\"\nbase_url = \"https://x\"\n";
        let mut doc = CodexToml::parse(text).unwrap();
        assert!(doc.remove_provider("only_one"));
        let out = doc.to_string_preserving();
        assert!(
            !out.contains("model_providers"),
            "empty parent must be removed: {out}"
        );
    }

    #[test]
    fn remove_provider_keeps_siblings() {
        let mut doc = CodexToml::parse(NORMAL).unwrap();
        doc.upsert_provider("eclyzer_bridge_a_1", &spec());
        assert!(doc.remove_provider("eclyzer_bridge_a_1"));
        let out = doc.to_string_preserving();
        assert!(out.contains("[model_providers.my_gateway]"), "{out}");
        assert!(!out.contains("eclyzer_bridge_a_1"), "{out}");
    }

    #[test]
    fn provider_ids_reports_every_entry() {
        let mut doc = CodexToml::parse(NORMAL).unwrap();
        doc.upsert_provider("eclyzer_bridge_a_1", &spec());
        let mut ids = doc.provider_ids();
        ids.sort();
        assert_eq!(ids, vec!["eclyzer_bridge_a_1", "my_gateway"]);
    }

    #[test]
    fn auth_command_writes_argv_not_a_shell_string() {
        let mut doc = CodexToml::parse("").unwrap();
        let spec = ProviderSpec {
            auth_command: Some(AuthCommand {
                command: r"C:\Program Files\Eclyzer Bridge\eclyzer-bridge-helper.exe",
                args: vec!["secret", "resolve", "profile-1"],
                timeout_ms: Some(5000),
            }),
            ..spec()
        };
        doc.upsert_provider("eclyzer_bridge_x_1", &spec);
        let out = doc.to_string_preserving();

        assert!(
            out.contains("[model_providers.eclyzer_bridge_x_1.auth]"),
            "{out}"
        );
        // Args are a real TOML array, so Codex receives argv, not a shell line.
        assert!(
            out.contains("args = [\"secret\", \"resolve\", \"profile-1\"]"),
            "{out}"
        );
        assert!(out.contains("timeout_ms = 5000"), "{out}");

        // The space-containing path must survive as one value. toml_edit emits
        // it as a TOML literal string (single quotes) because that avoids
        // escaping every backslash; assert on the round-tripped value rather
        // than on the quoting style.
        let reparsed = CodexToml::parse(&out).unwrap();
        assert_eq!(
            reparsed
                .provider_auth_str("eclyzer_bridge_x_1", "command")
                .as_deref(),
            Some(r"C:\Program Files\Eclyzer Bridge\eclyzer-bridge-helper.exe")
        );
        // It must never be split into multiple argv entries by the file itself.
        assert!(
            !out.contains("command = \"C:\\Program Files\\Eclyzer"),
            "{out}"
        );
    }

    #[test]
    fn auth_and_env_key_are_mutually_exclusive() {
        // Codex rejects combining `[auth]` with `env_key`.
        let mut doc = CodexToml::parse("").unwrap();
        let with_env = ProviderSpec {
            env_key: Some("MY_KEY"),
            ..spec()
        };
        doc.upsert_provider("p", &with_env);
        assert!(doc.to_string_preserving().contains("env_key = \"MY_KEY\""));

        let with_auth = ProviderSpec {
            env_key: None,
            auth_command: Some(AuthCommand {
                command: "helper.exe",
                args: vec!["secret", "resolve", "p"],
                timeout_ms: None,
            }),
            ..spec()
        };
        doc.upsert_provider("p", &with_auth);
        let out = doc.to_string_preserving();
        assert!(!out.contains("env_key"), "env_key must be cleared: {out}");
        assert!(out.contains("[model_providers.p.auth]"), "{out}");
    }

    #[test]
    fn upsert_clears_legacy_inline_bearer_token() {
        let text = "[model_providers.p]\nname = \"P\"\nexperimental_bearer_token = \"sk-secret\"\n";
        let mut doc = CodexToml::parse(text).unwrap();
        doc.upsert_provider("p", &spec());
        let out = doc.to_string_preserving();
        assert!(
            !out.contains("sk-secret"),
            "inline token must be removed: {out}"
        );
    }

    #[test]
    fn http_headers_and_query_params_render() {
        let mut doc = CodexToml::parse("").unwrap();
        let spec = ProviderSpec {
            http_headers: vec![("X-Tenant", "acme")],
            query_params: vec![("api-version", "2025-04-01-preview")],
            ..spec()
        };
        doc.upsert_provider("p", &spec);
        let out = doc.to_string_preserving();
        assert!(out.contains("X-Tenant"), "{out}");
        assert!(out.contains("api-version"), "{out}");
    }

    #[test]
    fn empty_config_gets_keys_without_a_bare_header() {
        let mut doc = CodexToml::parse_or_empty(None).unwrap();
        doc.set_str("model", "m");
        doc.set_str("model_provider", "p");
        doc.upsert_provider("p", &spec());
        let out = doc.to_string_preserving();
        assert!(out.starts_with("model = \"m\""), "{out}");
        assert!(out.contains("[model_providers.p]"), "{out}");
    }

    #[test]
    fn unrelated_keys_of_managed_provider_are_replaced_not_merged_blindly() {
        // Bridge owns its own provider entries: applying again must fully
        // describe the profile rather than leaving stale fields behind.
        let text = "[model_providers.eclyzer_bridge_x_1]\nname = \"old\"\nbase_url = \"https://old\"\nwire_api = \"responses\"\n";
        let mut doc = CodexToml::parse(text).unwrap();
        doc.upsert_provider("eclyzer_bridge_x_1", &spec());
        let out = doc.to_string_preserving();
        assert!(out.contains("https://api.deepseek.com/v1"), "{out}");
        assert!(!out.contains("https://old"), "{out}");
    }
}
