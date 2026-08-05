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

/// Conservative fallback used only when an OpenRouter-compatible catalog
/// omits `context_length`. The live OpenRouter catalog normally provides the
/// real value for every model, so this is a compatibility fallback rather than
/// a claim about the model.
const DEFAULT_CONTEXT_WINDOW: u32 = 8192;

fn advertised_parameter(m: &OpenAiModel, expected: &str) -> bool {
    m.supported_parameters
        .as_deref()
        .unwrap_or_default()
        .iter()
        .any(|parameter| parameter == expected)
}

/// Map an OpenRouter model listing entry to SC Node's [`ModelInfo`] without
/// inventing model capabilities. Name, context length, and tool support come
/// from the provider catalog when present. Streaming remains `true` because it
/// is a capability of the OpenRouter adapter/protocol path itself.
fn map_openrouter_model_to_model_info(m: &OpenAiModel) -> ModelInfo {
    let name = m
        .name
        .as_deref()
        .filter(|name| !name.trim().is_empty())
        .unwrap_or(&m.id)
        .to_string();
    let context_window = m
        .context_length
        .map(|value| u32::try_from(value).unwrap_or(u32::MAX))
        .unwrap_or(DEFAULT_CONTEXT_WINDOW);

    ModelInfo {
        id: m.id.clone(),
        name,
        context_window,
        supports_tools: advertised_parameter(m, "tools"),
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

    fn model_fixture(
        id: &str,
        name: Option<&str>,
        context_length: Option<u64>,
        supported_parameters: Option<Vec<&str>>,
    ) -> OpenAiModel {
        OpenAiModel {
            id: id.into(),
            object: Some("model".into()),
            created: None,
            owned_by: None,
            name: name.map(str::to_string),
            context_length,
            supported_parameters: supported_parameters
                .map(|items| items.into_iter().map(str::to_string).collect()),
        }
    }

    #[test]
    fn test_map_model_info_uses_catalog_metadata() {
        let model = model_fixture(
            "vendor/model",
            Some("Vendor: Model"),
            Some(1_000_000),
            Some(vec!["temperature", "tools", "tool_choice"]),
        );
        let info = map_openrouter_model_to_model_info(&model);
        assert_eq!(info.id, "vendor/model");
        assert_eq!(info.name, "Vendor: Model");
        assert_eq!(info.context_window, 1_000_000);
        assert!(info.supports_tools);
        assert!(info.supports_streaming);
    }

    #[test]
    fn test_map_model_info_does_not_invent_tool_support() {
        let model = model_fixture(
            "vendor/text-only",
            Some("Text only"),
            Some(32_768),
            Some(vec!["temperature", "max_tokens"]),
        );
        let info = map_openrouter_model_to_model_info(&model);
        assert!(!info.supports_tools);
    }

    #[test]
    fn test_map_model_info_uses_conservative_fallbacks_when_metadata_is_absent() {
        let model = model_fixture("openai/gpt-4.1-mini", None, None, None);
        let info = map_openrouter_model_to_model_info(&model);
        assert_eq!(info.name, "openai/gpt-4.1-mini");
        assert_eq!(info.context_window, DEFAULT_CONTEXT_WINDOW);
        assert!(!info.supports_tools);
        assert!(info.supports_streaming);
    }

    #[test]
    fn test_openrouter_catalog_shape_deserializes() {
        let model: OpenAiModel = serde_json::from_value(serde_json::json!({
            "id": "qwen/qwen3.8-max",
            "name": "Qwen: Qwen3.8 Max",
            "context_length": 1000000,
            "supported_parameters": ["temperature", "tools", "tool_choice"]
        }))
        .expect("OpenRouter catalog entry should deserialize");
        let info = map_openrouter_model_to_model_info(&model);
        assert_eq!(info.context_window, 1_000_000);
        assert!(info.supports_tools);
    }

    #[test]
    fn test_provider_key_and_name() {
        let provider = OpenRouterProvider::new(OpenRouterConfig::default()).unwrap();
        assert_eq!(provider.key(), "openrouter");
        assert_eq!(provider.name(), "OpenRouter");
    }

    /// Authenticated OpenRouter smoke test. It is deliberately ignored in
    /// normal CI: running it requires a real key, live network access, and may
    /// incur a small provider charge. Use `scripts/verify-openrouter.ps1` to run
    /// it explicitly when credentials are available.
    #[tokio::test]
    #[ignore = "requires SC_AGENT_OPENROUTER_API_KEY and live OpenRouter access"]
    async fn live_authenticated_catalog_and_completion() {
        use futures::StreamExt;

        let key = std::env::var("SC_AGENT_OPENROUTER_API_KEY")
            .expect("SC_AGENT_OPENROUTER_API_KEY must be set for the ignored live test");
        assert!(!key.trim().is_empty(), "OpenRouter key must not be blank");

        let mut config = OpenRouterConfig::default();
        if let Ok(model) = std::env::var("SC_AGENT_OPENROUTER_TEST_MODEL")
            && !model.trim().is_empty()
        {
            config.default_model = model;
        }
        let provider = OpenRouterProvider::new(config.clone()).expect("provider construction");
        let models = provider.list_models().await.expect("live model catalog");
        assert!(!models.is_empty(), "expected at least one live model");
        assert!(
            models.iter().any(|model| model.id == config.default_model),
            "configured live-test model is missing from the catalog"
        );

        let request = CompletionRequest {
            model: config.default_model,
            messages: vec![Message::user(
                "Reply exactly with SC NODE OPENROUTER TEST OK",
            )],
            tools: vec![],
            system: None,
            stream: true,
            temperature: Some(0.0),
            max_tokens: Some(32),
        };
        let mut stream = provider.complete(request).await.expect("live completion");
        let mut text = String::new();
        while let Some(event) = stream.next().await {
            match event.expect("live stream event") {
                sc_message_types::StreamEvent::TextDelta { text: delta } => text.push_str(&delta),
                sc_message_types::StreamEvent::End { .. } => {}
                sc_message_types::StreamEvent::ToolUse { .. }
                | sc_message_types::StreamEvent::Error { .. } => {}
            }
        }
        assert!(
            text.contains("SC NODE OPENROUTER TEST OK"),
            "unexpected live completion response: {text:?}"
        );
    }
}
