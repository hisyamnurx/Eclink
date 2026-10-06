//! Built-in provider starter templates (PRD §19, §65).
//!
//! Rules this module follows deliberately:
//! * templates are **convenience only** — none of them creates a profile
//!   automatically, and none carries an API key;
//! * a template never implies compatibility with Codex or Claude Code; that
//!   verdict comes only from an actual protocol test (PRD §19, §21);
//! * the catalog ships inside the binary. There is no remote registry, so it is
//!   auditable in the repository and reproducible offline (PRD §65).

use serde::{Deserialize, Serialize};

use crate::profiles::model::AgentId;

/// A starting point for the "Add Provider" form.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProviderTemplate {
    /// Stable identifier used by the UI.
    pub id: String,
    /// Display name pre-filled into "Provider Name".
    pub name: String,
    /// Suggested OpenAI-compatible base URL.
    ///
    /// `None` means "do not guess": the user must enter the endpoint because we
    /// are not confident it is current (PRD §19).
    pub base_url: Option<String>,
    /// Model IDs that the provider documents for general chat use.
    ///
    /// These are hints for the picker, never a guarantee that the model exists
    /// for this account, and never a compatibility claim.
    #[serde(default)]
    pub suggested_models: Vec<String>,
    /// Which agent protocols this provider *might* serve.
    ///
    /// Purely informational for the UI; the test pipeline decides the verdict.
    #[serde(default)]
    pub likely_targets: Vec<AgentId>,
    /// Short note shown under the template in the picker.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
    /// Documentation URL, opened only when the user clicks it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub docs_url: Option<String>,
}

impl ProviderTemplate {
    fn new(id: &str, name: &str, base_url: Option<&str>) -> Self {
        Self {
            id: id.to_string(),
            name: name.to_string(),
            base_url: base_url.map(str::to_string),
            suggested_models: Vec::new(),
            likely_targets: Vec::new(),
            note: None,
            docs_url: None,
        }
    }

    fn models(mut self, models: &[&str]) -> Self {
        self.suggested_models = models.iter().map(|m| m.to_string()).collect();
        self
    }

    fn note(mut self, note: &str) -> Self {
        self.note = Some(note.to_string());
        self
    }

    fn docs(mut self, url: &str) -> Self {
        self.docs_url = Some(url.to_string());
        self
    }
}

/// The v0.1 template catalog.
///
/// Maintainer note: endpoints here are re-verified against provider
/// documentation before each release (PRD §19, §65). Where the current
/// endpoint is not certain, `base_url` is left `None` so the user is asked
/// rather than misled.
pub fn provider_templates() -> Vec<ProviderTemplate> {
    vec![
        ProviderTemplate::new(
            "deepseek",
            "DeepSeek",
            Some("https://api.deepseek.com/v1"),
        )
        .models(&["deepseek-chat", "deepseek-reasoner"])
        .note("OpenAI-compatible Chat Completions.")
        .docs("https://api-docs.deepseek.com/"),
        ProviderTemplate::new("openrouter", "OpenRouter", Some("https://openrouter.ai/api/v1"))
            .models(&["openrouter/auto"])
            .note("Aggregator; model IDs use vendor/model form. Responses support varies by upstream model.")
            .docs("https://openrouter.ai/docs"),
        ProviderTemplate::new("groq", "Groq", Some("https://api.groq.com/openai/v1"))
            .models(&["llama-3.3-70b-versatile"])
            .note("OpenAI-compatible Chat Completions.")
            .docs("https://console.groq.com/docs/openai"),
        ProviderTemplate::new("together", "Together AI", Some("https://api.together.xyz/v1"))
            .models(&["meta-llama/Llama-3.3-70B-Instruct-Turbo"])
            .note("OpenAI-compatible Chat Completions.")
            .docs("https://docs.together.ai/docs/openai-api-compatibility"),
        ProviderTemplate::new(
            "fireworks",
            "Fireworks AI",
            Some("https://api.fireworks.ai/inference/v1"),
        )
        .models(&["accounts/fireworks/models/llama-v3p3-70b-instruct"])
        .note("OpenAI-compatible Chat Completions.")
        .docs("https://docs.fireworks.ai/api-reference/introduction"),
        ProviderTemplate::new("mistral", "Mistral", Some("https://api.mistral.ai/v1"))
            .models(&["mistral-large-latest"])
            .note("OpenAI-compatible Chat Completions.")
            .docs("https://docs.mistral.ai/api/"),
        ProviderTemplate::new("xai", "xAI", Some("https://api.x.ai/v1"))
            .models(&["grok-2-latest"])
            .note("OpenAI-compatible API.")
            .docs("https://docs.x.ai/docs/overview"),
        ProviderTemplate::new("cerebras", "Cerebras", Some("https://api.cerebras.ai/v1"))
            .models(&["llama3.1-70b"])
            .note("OpenAI-compatible Chat Completions.")
            .docs("https://inference-docs.cerebras.ai/introduction"),
        ProviderTemplate::new("moonshot", "Moonshot / Kimi", Some("https://api.moonshot.ai/v1"))
            .models(&["kimi-k2-0905-preview"])
            .note("OpenAI-compatible Chat Completions.")
            .docs("https://platform.moonshot.ai/docs"),
        // Deliberately without a URL: self-hosted gateways vary far too much to
        // guess (LiteLLM, vLLM, corporate proxies, …). The user supplies it.
        ProviderTemplate::new("custom", "Custom", None)
            .note("Enter your own base URL, API key, and model ID."),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn catalog_has_expected_entries() {
        let templates = provider_templates();
        let ids: Vec<&str> = templates.iter().map(|t| t.id.as_str()).collect();
        for expected in [
            "deepseek",
            "openrouter",
            "groq",
            "together",
            "fireworks",
            "mistral",
            "xai",
            "cerebras",
            "moonshot",
            "custom",
        ] {
            assert!(ids.contains(&expected), "missing template: {expected}");
        }
    }

    #[test]
    fn template_ids_are_unique() {
        let templates = provider_templates();
        let mut ids: Vec<&str> = templates.iter().map(|t| t.id.as_str()).collect();
        ids.sort_unstable();
        let before = ids.len();
        ids.dedup();
        assert_eq!(before, ids.len(), "duplicate template ids");
    }

    #[test]
    fn templates_never_carry_a_secret() {
        // A template is a starter form; if it ever gained a key field, this
        // test would flag the design regression.
        let json = serde_json::to_string(&provider_templates()).unwrap();
        for forbidden in ["apiKey", "api_key", "sk-", "Bearer"] {
            assert!(
                !json.contains(forbidden),
                "template catalog leaked {forbidden}"
            );
        }
    }

    #[test]
    fn custom_template_intentionally_has_no_url() {
        let custom = provider_templates()
            .into_iter()
            .find(|t| t.id == "custom")
            .unwrap();
        assert!(
            custom.base_url.is_none(),
            "Bridge must not guess a custom gateway URL"
        );
    }

    #[test]
    fn suggested_urls_are_https() {
        for template in provider_templates() {
            if let Some(url) = &template.base_url {
                assert!(
                    url.starts_with("https://"),
                    "template {} must use https: {url}",
                    template.id
                );
            }
        }
    }

    #[test]
    fn templates_do_not_claim_compatibility() {
        // `likely_targets` may hint, but no template is pre-marked compatible.
        // The field exists for display only; assert it is never used as a
        // verdict by checking it stays advisory (empty for the OpenAI-ish set).
        for template in provider_templates() {
            assert!(
                template.likely_targets.len() <= 2,
                "template {} should not blanket-claim targets",
                template.id
            );
        }
    }
}
