//! Ollama provider for SC Node.

use async_trait::async_trait;
use reqwest::Client;
use sc_config::OllamaConfig;
use sc_message_types::{CompletionRequest, ContentBlock, ModelInfo, Role, StreamEvent};
use sc_provider_core::{EventStream, Provider, ProviderError, Result};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

// ── Ollama API types ──────────────────────────────────────────

#[derive(Debug, Clone, serde::Deserialize)]
struct OllamaTagsResponse {
    models: Vec<OllamaModelItem>,
}

#[derive(Debug, Clone, serde::Deserialize)]
struct OllamaModelItem {
    name: String,
    #[allow(dead_code)]
    modified_at: Option<String>,
    #[allow(dead_code)]
    size: Option<u64>,
    #[allow(dead_code)]
    digest: Option<String>,
    details: Option<OllamaModelDetails>,
}

#[derive(Debug, Clone, serde::Deserialize)]
struct OllamaModelDetails {
    family: Option<String>,
    #[allow(dead_code)]
    families: Option<Vec<String>>,
    #[allow(dead_code)]
    parameter_size: Option<String>,
    #[allow(dead_code)]
    quantization_level: Option<String>,
    #[allow(dead_code)]
    format: Option<String>,
}

#[derive(Debug, Clone, serde::Serialize)]
struct OllamaChatRequest {
    model: String,
    messages: Vec<OllamaChatMessage>,
    #[serde(skip_serializing_if = "Option::is_none")]
    tools: Option<Vec<serde_json::Value>>,
    stream: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    options: Option<OllamaChatOptions>,
    #[serde(skip_serializing_if = "Option::is_none")]
    keep_alive: Option<String>,
}

#[derive(Debug, Clone, serde::Serialize)]
struct OllamaChatOptions {
    #[serde(skip_serializing_if = "Option::is_none")]
    temperature: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    num_predict: Option<u32>,
}

#[derive(Debug, Clone, serde::Serialize)]
struct OllamaChatMessage {
    role: String,
    content: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    tool_calls: Option<Vec<serde_json::Value>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    tool_call_id: Option<String>,
}

#[derive(Debug, Clone, serde::Deserialize)]
struct OllamaChatChunk {
    #[allow(dead_code)]
    model: String,
    #[allow(dead_code)]
    created_at: Option<String>,
    message: Option<OllamaChunkMessage>,
    done: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    done_reason: Option<String>,
}

#[derive(Debug, Clone, serde::Deserialize)]
struct OllamaChunkMessage {
    #[allow(dead_code)]
    role: Option<String>,
    content: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    tool_calls: Option<Vec<serde_json::Value>>,
}

// ── Conversion helpers ────────────────────────────────────────

fn map_ollama_model_to_model_info(m: &OllamaModelItem) -> ModelInfo {
    let context_window = m
        .details
        .as_ref()
        .and_then(|d| d.family.as_deref())
        .map(|family| match family {
            "llama" => 8192,
            "mistral" => 32000,
            "gemma" => 8192,
            "phi" => 2048,
            _ => 4096,
        })
        .unwrap_or(4096);

    ModelInfo {
        id: m.name.clone(),
        name: m.name.clone(),
        context_window,
        supports_tools: true,
        supports_streaming: true,
    }
}

fn parse_ollama_chat_line(line: &str) -> Option<StreamEvent> {
    let trimmed = line.trim();
    if trimmed.is_empty() {
        return None;
    }

    let chunk: OllamaChatChunk = match serde_json::from_str(trimmed) {
        Ok(c) => c,
        Err(_) => {
            return Some(StreamEvent::Error {
                message: format!("Failed to parse chunk: {}", trimmed),
            });
        }
    };

    if chunk.done {
        return Some(StreamEvent::End {
            finish_reason: chunk.done_reason,
        });
    }

    if let Some(msg) = &chunk.message {
        if let Some(content) = &msg.content
            && !content.is_empty()
        {
            return Some(StreamEvent::TextDelta {
                text: content.clone(),
            });
        }

        if let Some(tool_calls) = &msg.tool_calls {
            for tc in tool_calls {
                if let Some(function) = tc.get("function") {
                    let name = function
                        .get("name")
                        .and_then(|v| v.as_str())
                        .unwrap_or("unknown");
                    let arguments = function
                        .get("arguments")
                        .and_then(|v| v.as_str())
                        .and_then(|s| serde_json::from_str::<serde_json::Value>(s).ok())
                        .unwrap_or(serde_json::Value::Null);

                    static TC_COUNTER: AtomicU64 = AtomicU64::new(0);
                    let id = format!("ollama-tc-{}", TC_COUNTER.fetch_add(1, Ordering::Relaxed));

                    return Some(StreamEvent::ToolUse {
                        id,
                        name: name.to_string(),
                        input: arguments,
                    });
                }
            }
        }
    }

    None
}

fn build_ollama_chat_request(req: &CompletionRequest, config: &OllamaConfig) -> OllamaChatRequest {
    let messages: Vec<OllamaChatMessage> = req
        .messages
        .iter()
        .map(|msg| {
            let role = match msg.role {
                Role::System => "system",
                Role::User => "user",
                Role::Assistant => "assistant",
                Role::Tool => "tool",
            };

            let mut text = String::new();
            for block in &msg.content {
                match block {
                    ContentBlock::Text { text: t } => {
                        if !text.is_empty() {
                            text.push('\n');
                        }
                        text.push_str(t);
                    }
                    ContentBlock::ToolResult { content, .. } => {
                        if !text.is_empty() {
                            text.push('\n');
                        }
                        text.push_str(content);
                    }
                    ContentBlock::ToolUse { .. } => {}
                }
            }

            OllamaChatMessage {
                role: role.to_string(),
                content: text,
                tool_calls: None,
                tool_call_id: None,
            }
        })
        .collect();

    let model = if req.model.is_empty() {
        config.default_model.clone()
    } else {
        req.model.clone()
    };

    let tools = if req.tools.is_empty() {
        None
    } else {
        Some(
            req.tools
                .iter()
                .map(|t| {
                    serde_json::json!({
                        "type": "function",
                        "function": {
                            "name": t.name,
                            "description": t.description,
                            "parameters": t.parameters
                        }
                    })
                })
                .collect(),
        )
    };

    OllamaChatRequest {
        model,
        messages,
        tools,
        stream: true,
        options: Some(OllamaChatOptions {
            temperature: req.temperature,
            num_predict: req.max_tokens,
        }),
        keep_alive: Some(config.keep_alive.clone()),
    }
}

// ── Provider struct ───────────────────────────────────────────

pub struct OllamaProvider {
    client: Client,
    config: OllamaConfig,
}

impl OllamaProvider {
    pub fn new(config: OllamaConfig) -> Result<Self> {
        let client = Client::builder()
            .timeout(Duration::from_secs(config.timeout_secs))
            .build()
            .map_err(ProviderError::Http)?;

        Ok(Self { client, config })
    }
}

#[async_trait]
impl Provider for OllamaProvider {
    fn key(&self) -> &str {
        "ollama"
    }

    fn name(&self) -> &str {
        "Ollama"
    }

    async fn list_models(&self) -> Result<Vec<ModelInfo>> {
        let url = format!("{}/api/tags", self.config.base_url.trim_end_matches('/'));

        let resp = self
            .client
            .get(&url)
            .send()
            .await
            .map_err(|e| match e.is_timeout() {
                true => ProviderError::Network(format!(
                    "Timeout connecting to Ollama at {}. Is Ollama running?",
                    self.config.base_url
                )),
                false => match e.is_connect() {
                    true => ProviderError::Network(format!(
                        "Cannot connect to Ollama at {}. Is Ollama running?",
                        self.config.base_url
                    )),
                    false => ProviderError::Http(e),
                },
            })?;

        if !resp.status().is_success() {
            let status = resp.status().as_u16();
            let body = resp.text().await.unwrap_or_default();
            return Err(ProviderError::Api(format!(
                "Ollama returned HTTP {}: {}",
                status, body
            )));
        }

        let tags: OllamaTagsResponse = resp
            .json()
            .await
            .map_err(|e| ProviderError::Api(format!("Failed to parse Ollama response: {}", e)))?;

        let models: Vec<ModelInfo> = tags
            .models
            .iter()
            .map(map_ollama_model_to_model_info)
            .collect();

        Ok(models)
    }

    async fn complete(&self, request: CompletionRequest) -> Result<EventStream> {
        let url = format!("{}/api/chat", self.config.base_url.trim_end_matches('/'));
        let ollama_req = build_ollama_chat_request(&request, &self.config);

        let resp = self
            .client
            .post(&url)
            .json(&ollama_req)
            .send()
            .await
            .map_err(|e| match e.is_timeout() {
                true => ProviderError::Network(format!(
                    "Timeout connecting to Ollama at {}",
                    self.config.base_url
                )),
                false => match e.is_connect() {
                    true => ProviderError::Network(format!(
                        "Cannot connect to Ollama at {}. Is Ollama running?",
                        self.config.base_url
                    )),
                    false => ProviderError::Http(e),
                },
            })?;

        if !resp.status().is_success() {
            let status = resp.status().as_u16();
            let body = resp.text().await.unwrap_or_default();
            return Err(ProviderError::Api(format!(
                "Ollama returned HTTP {}: {}",
                status, body
            )));
        }

        // Collect full body text, split on newlines, emit events sequentially
        let full_body = resp.text().await.map_err(ProviderError::Http)?;
        let events: Vec<Result<StreamEvent>> = full_body
            .lines()
            .filter_map(parse_ollama_chat_line)
            .map(Ok)
            .collect();

        Ok(Box::pin(futures::stream::iter(events)))
    }

    async fn health_check(&self) -> Result<bool> {
        let url = format!("{}/api/tags", self.config.base_url.trim_end_matches('/'));
        match self
            .client
            .get(&url)
            .timeout(Duration::from_secs(5))
            .send()
            .await
        {
            Ok(resp) => Ok(resp.status().is_success()),
            Err(_) => Ok(false),
        }
    }
}

// ── Unit tests ────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_text_chunk() {
        let event = parse_ollama_chat_line(
            r#"{"model":"llama3","message":{"role":"assistant","content":"Hello"},"done":false}"#,
        )
        .unwrap();
        assert!(matches!(event, StreamEvent::TextDelta { .. }));
    }

    #[test]
    fn test_parse_done_chunk() {
        let event = parse_ollama_chat_line(
            r#"{"model":"llama3","message":{},"done":true,"done_reason":"stop"}"#,
        )
        .unwrap();
        assert!(matches!(event, StreamEvent::End { .. }));
    }

    #[test]
    fn test_parse_empty_content() {
        let event = parse_ollama_chat_line(
            r#"{"model":"llama3","message":{"role":"assistant","content":""},"done":false}"#,
        );
        assert!(event.is_none());
    }

    #[test]
    fn test_map_model_info_llama() {
        let item = OllamaModelItem {
            name: "llama3.2:3b".into(),
            modified_at: None,
            size: None,
            digest: None,
            details: Some(OllamaModelDetails {
                family: Some("llama".into()),
                families: None,
                parameter_size: None,
                quantization_level: None,
                format: None,
            }),
        };
        let mi = map_ollama_model_to_model_info(&item);
        assert_eq!(mi.id, "llama3.2:3b");
        assert_eq!(mi.context_window, 8192);
        assert!(mi.supports_tools);
    }

    #[test]
    fn test_map_model_info_unknown_family() {
        let item = OllamaModelItem {
            name: "x-model".into(),
            modified_at: None,
            size: None,
            digest: None,
            details: Some(OllamaModelDetails {
                family: Some("unknown-family".into()),
                families: None,
                parameter_size: None,
                quantization_level: None,
                format: None,
            }),
        };
        let mi = map_ollama_model_to_model_info(&item);
        assert_eq!(mi.context_window, 4096);
    }

    #[test]
    fn test_map_model_info_no_details() {
        let item = OllamaModelItem {
            name: "minimal".into(),
            modified_at: None,
            size: None,
            digest: None,
            details: None,
        };
        let mi = map_ollama_model_to_model_info(&item);
        assert_eq!(mi.context_window, 4096);
    }

    #[test]
    fn test_serialize_chat_request() {
        let config = OllamaConfig::default();
        let req = CompletionRequest {
            model: "".into(),
            messages: vec![sc_message_types::Message::user("Hello world")],
            tools: vec![],
            system: None,
            stream: true,
            temperature: Some(0.7),
            max_tokens: Some(100),
        };

        let ollama_req = build_ollama_chat_request(&req, &config);
        assert_eq!(ollama_req.model, config.default_model);
        assert_eq!(ollama_req.messages.len(), 1);
        assert_eq!(ollama_req.messages[0].role, "user");
        assert_eq!(ollama_req.messages[0].content, "Hello world");
        assert!(ollama_req.stream);
        assert!(ollama_req.keep_alive.is_some());
    }

    #[test]
    fn test_parse_tool_call_chunk() {
        let json = r#"{"model":"llama3","message":{"role":"assistant","content":"","tool_calls":[{"function":{"name":"read_file","arguments":{"path":"/tmp/test.txt"}}}]},"done":false}"#;
        if let Some(StreamEvent::ToolUse { name, .. }) = parse_ollama_chat_line(json) {
            assert_eq!(name, "read_file");
        } else {
            panic!("Expected ToolUse");
        }
    }

    #[test]
    fn test_parse_malformed_json() {
        match parse_ollama_chat_line("not valid json") {
            Some(StreamEvent::Error { message }) => assert!(message.contains("Failed to parse")),
            other => panic!("Expected Error, got {:?}", other),
        }
    }

    #[test]
    fn test_parse_empty_line() {
        assert!(parse_ollama_chat_line("").is_none());
    }

    #[test]
    fn test_parse_tags_response() {
        let json = r#"{"models":[{"name":"llama3.2:3b","details":{"family":"llama"}}]}"#;
        let parsed: OllamaTagsResponse = serde_json::from_str(json).unwrap();
        assert_eq!(parsed.models.len(), 1);
        assert_eq!(parsed.models[0].name, "llama3.2:3b");
    }
}
