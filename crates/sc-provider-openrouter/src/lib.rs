//! OpenRouter provider for SC Node.
//!
//! Built on the shared OpenAI-compatible core in `sc-provider-core`:
//! config-driven base URL, API key resolved from the
//! `SC_AGENT_OPENROUTER_API_KEY` environment variable, bounded retry, and
//! typed/categorized/redacted errors all come from there.

use async_trait::async_trait;
use sc_config::OpenRouterConfig;
use sc_message_types::{CompletionRequest, ModelInfo};
use sc_provider_core::openai_compat::{OpenAiCompatClient, OpenAiCompatConfig, OpenAiModel};
use sc_provider_core::{
    ChatCompletionRequest, EventStream, Provider, Result, message_to_chat_message,
};
use std::time::Duration;

/// Conservative default context window used when mapping an OpenRouter
/// model listing entry to SC Node's [`ModelInfo`]. OpenRouter proxies a
/// large, heterogeneous set of upstream models; the minimal
/// `/models` schema this crate parses (id/object/created/owned_by) does
/// not carry a context length, so we do not attempt to guess one from the
/// model id the way the single-vendor NIM provider does.
const DEFAULT_CONTEXT_WINDOW: u32 = 8192;

/// Map an OpenRouter model listing entry to SC Node's [`ModelInfo`].
fn map_openrouter_model_to_model_info(m: &OpenAiModel) -> ModelInfo {
    ModelInfo {
        id: m.id.clone(),
        name: m.id.clone(),
        context_window: DEFAULT_CONTEXT_WINDOW,
        supports_tools: true,
        supports_streaming: true,
    }
}

/// Build a shared-core chat completion request from SC Node's
/// [`CompletionRequest`].
fn build_chat_completion_request(
    req: &CompletionRequest,
    config: &OpenRouterConfig,
) -> ChatCompletionRequest {
    let model = if req.model.is_empty() {
        config.default_model.clone()
    } else {
        req.model.clone()
    };

    let messages = req
        .messages
        .iter()
        .cloned()
        .map(message_to_chat_message)
        .collect();

    ChatCompletionRequest {
        model,
        messages,
        tools: req.tools.clone(),
        system: req.system.clone(),
        stream: false,
        temperature: req.temperature,
        max_tokens: req.max_tokens,
    }
}

pub struct OpenRouterProvider {
    client: OpenAiCompatClient,
    config: OpenRouterConfig,
}

impl OpenRouterProvider {
    pub fn new(config: OpenRouterConfig) -> Result<Self> {
        let compat_config =
            OpenAiCompatConfig::new(config.base_url.clone(), "SC_AGENT_OPENROUTER_API_KEY")
                .with_timeout(Duration::from_secs(config.timeout_secs))
                .with_max_retries(config.max_retries);
        let client = OpenAiCompatClient::new(compat_config)?;

        Ok(Self { client, config })
    }
}

#[async_trait]
impl Provider for OpenRouterProvider {
    fn key(&self) -> &str {
        "openrouter"
    }

    fn name(&self) -> &str {
        "OpenRouter"
    }

    async fn list_models(&self) -> Result<Vec<ModelInfo>> {
        let models = self.client.list_models().await?;
        Ok(models
            .iter()
            .map(map_openrouter_model_to_model_info)
            .collect())
    }

    async fn health_check(&self) -> Result<bool> {
        Ok(self.client.list_models().await.is_ok())
    }

    async fn complete(&self, request: CompletionRequest) -> Result<EventStream> {
        let chat_request = build_chat_completion_request(&request, &self.config);
        self.client.chat_completion_stream(&chat_request).await
    }
}

// ── Unit tests ─────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use sc_message_types::Message;

    #[test]
    fn test_build_chat_completion_request_uses_default_model_when_empty() {
        let config = OpenRouterConfig::default();
        let req = CompletionRequest {
            model: "".into(),
            messages: vec![Message::user("Hello world")],
            tools: vec![],
            system: Some("You are helpful".into()),
            stream: true,
            temperature: Some(0.7),
            max_tokens: Some(100),
        };

        let chat_req = build_chat_completion_request(&req, &config);

        assert_eq!(chat_req.model, config.default_model);
        assert_eq!(chat_req.messages.len(), 1);
        assert_eq!(chat_req.messages[0].role, "user");
        assert_eq!(chat_req.messages[0].content, Some("Hello world".into()));
        assert_eq!(chat_req.temperature, Some(0.7));
        assert_eq!(chat_req.max_tokens, Some(100));
        assert_eq!(chat_req.system, Some("You are helpful".into()));
    }

    #[test]
    fn test_build_chat_completion_request_preserves_explicit_model() {
        let config = OpenRouterConfig::default();
        let req = CompletionRequest {
            model: "explicit/model".into(),
            messages: vec![Message::user("hi")],
            tools: vec![],
            system: None,
            stream: true,
            temperature: None,
            max_tokens: None,
        };

        let chat_req = build_chat_completion_request(&req, &config);

        assert_eq!(chat_req.model, "explicit/model");
    }

    #[test]
    fn test_map_model_info_uses_conservative_default_context_window() {
        let model = OpenAiModel {
            id: "openai/gpt-4.1-mini".into(),
            object: None,
            created: None,
            owned_by: None,
        };
        let info = map_openrouter_model_to_model_info(&model);
        assert_eq!(info.id, "openai/gpt-4.1-mini");
        assert_eq!(info.context_window, DEFAULT_CONTEXT_WINDOW);
        assert!(info.supports_tools);
        assert!(info.supports_streaming);
    }

    #[test]
    fn test_provider_key_and_name() {
        let provider = OpenRouterProvider::new(OpenRouterConfig::default()).unwrap();
        assert_eq!(provider.key(), "openrouter");
        assert_eq!(provider.name(), "OpenRouter");
    }

    /// Live smoke test: only runs a real network call if a real
    /// OpenRouter API key is already present in the environment.
    /// Otherwise it is a no-op pass, by design - CI/dev machines without
    /// the key must never be blocked on network access.
    #[tokio::test]
    async fn live_list_models_if_key_present() {
        if std::env::var("SC_AGENT_OPENROUTER_API_KEY").is_err() {
            eprintln!(
                "skipping live OpenRouter test: SC_AGENT_OPENROUTER_API_KEY not set in this environment"
            );
            return;
        }

        let provider = OpenRouterProvider::new(OpenRouterConfig::default()).unwrap();
        let models = provider.list_models().await.unwrap();
        assert!(!models.is_empty(), "expected at least one live model");
    }
}
