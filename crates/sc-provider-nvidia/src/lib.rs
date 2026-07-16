//! NVIDIA NIM provider for SC Node.
//!
//! Built on the shared OpenAI-compatible core in `sc-provider-core`:
//! config-driven base URL, API key resolved from the
//! `SC_AGENT_NVIDIA_API_KEY` environment variable, bounded retry, and
//! typed/categorized/redacted errors all come from there. This crate only
//! adds NVIDIA-specific model metadata mapping and request wiring.

use async_trait::async_trait;
use sc_config::NvidiaConfig;
use sc_message_types::{CompletionRequest, ModelInfo};
use sc_provider_core::openai_compat::{OpenAiCompatClient, OpenAiCompatConfig, OpenAiModel};
use sc_provider_core::{
    ChatCompletionRequest, EventStream, Provider, Result, message_to_chat_message,
};
use std::time::Duration;

/// Map an NVIDIA NIM model listing entry to SC Node's [`ModelInfo`].
fn map_nim_model_to_model_info(m: &OpenAiModel) -> ModelInfo {
    // Common NIM model context windows based on model family. NIM's
    // `/models` response does not carry this field, so we fall back to a
    // conservative default for anything we don't recognize.
    let context_window = if m.id.contains("nemotron-3-ultra")
        || m.id.contains("llama-3.1")
        || m.id.contains("llama-3")
    {
        8192
    } else if m.id.contains("mistral") {
        32768
    } else if m.id.contains("gemma") {
        8192
    } else {
        4096
    };

    ModelInfo {
        id: m.id.clone(),
        name: m.id.clone(),
        context_window,
        supports_tools: true,
        supports_streaming: true,
    }
}

/// Build a shared-core chat completion request from SC Node's
/// [`CompletionRequest`].
fn build_chat_completion_request(
    req: &CompletionRequest,
    config: &NvidiaConfig,
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

pub struct NvidiaProvider {
    client: OpenAiCompatClient,
    config: NvidiaConfig,
}

impl NvidiaProvider {
    pub fn new(config: NvidiaConfig) -> Result<Self> {
        let compat_config =
            OpenAiCompatConfig::new(config.base_url.clone(), "SC_AGENT_NVIDIA_API_KEY")
                .with_timeout(Duration::from_secs(config.timeout_secs))
                .with_max_retries(config.max_retries);
        let client = OpenAiCompatClient::new(compat_config)?;

        Ok(Self { client, config })
    }
}

#[async_trait]
impl Provider for NvidiaProvider {
    fn key(&self) -> &str {
        "nvidia"
    }

    fn name(&self) -> &str {
        "NVIDIA NIM"
    }

    async fn list_models(&self) -> Result<Vec<ModelInfo>> {
        let models = self.client.list_models().await?;
        Ok(models.iter().map(map_nim_model_to_model_info).collect())
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
        let config = NvidiaConfig::default();
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
        let config = NvidiaConfig::default();
        let req = CompletionRequest {
            model: "explicit-model".into(),
            messages: vec![Message::user("hi")],
            tools: vec![],
            system: None,
            stream: true,
            temperature: None,
            max_tokens: None,
        };

        let chat_req = build_chat_completion_request(&req, &config);

        assert_eq!(chat_req.model, "explicit-model");
    }

    #[test]
    fn test_map_model_info_nemotron() {
        let model = OpenAiModel {
            id: "nemotron-3-ultra".into(),
            object: None,
            created: None,
            owned_by: None,
        };
        let info = map_nim_model_to_model_info(&model);
        assert_eq!(info.id, "nemotron-3-ultra");
        assert_eq!(info.context_window, 8192);
        assert!(info.supports_tools);
        assert!(info.supports_streaming);
    }

    #[test]
    fn test_map_model_info_mistral() {
        let model = OpenAiModel {
            id: "mistral-large-2".into(),
            object: None,
            created: None,
            owned_by: None,
        };
        let info = map_nim_model_to_model_info(&model);
        assert_eq!(info.context_window, 32768);
    }

    #[test]
    fn test_map_model_info_unknown_defaults_conservatively() {
        let model = OpenAiModel {
            id: "some-unknown-model".into(),
            object: None,
            created: None,
            owned_by: None,
        };
        let info = map_nim_model_to_model_info(&model);
        assert_eq!(info.context_window, 4096);
    }

    #[test]
    fn test_provider_key_and_name() {
        let provider = NvidiaProvider::new(NvidiaConfig::default()).unwrap();
        assert_eq!(provider.key(), "nvidia");
        assert_eq!(provider.name(), "NVIDIA NIM");
    }

    /// Live smoke test: only runs a real network call if a real NVIDIA
    /// API key is already present in the environment. Otherwise it is a
    /// no-op pass, by design - CI/dev machines without the key must never
    /// be blocked on network access.
    #[tokio::test]
    async fn live_list_models_if_key_present() {
        if std::env::var("SC_AGENT_NVIDIA_API_KEY").is_err() {
            eprintln!(
                "skipping live NVIDIA test: SC_AGENT_NVIDIA_API_KEY not set in this environment"
            );
            return;
        }

        let provider = NvidiaProvider::new(NvidiaConfig::default()).unwrap();
        let models = provider.list_models().await.unwrap();
        assert!(!models.is_empty(), "expected at least one live model");
    }
}
