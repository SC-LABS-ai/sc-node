//! Core provider traits and OpenAI-compatible helper types for SC Node.

pub mod openai_compat;
pub mod routing;
pub mod sse;

use async_trait::async_trait;
use futures::Stream;
use sc_message_types::{
    CompletionRequest, ContentBlock, Message, ModelInfo, ProviderInfo, Role, StreamEvent,
    ToolDefinition,
};
use std::pin::Pin;
use thiserror::Error;

pub type Result<T, E = ProviderError> = std::result::Result<T, E>;
pub type EventStream = Pin<Box<dyn Stream<Item = Result<StreamEvent>> + Send>>;

#[derive(Debug, Error)]
pub enum ProviderError {
    #[error("HTTP error: {0}")]
    Http(#[from] reqwest::Error),

    #[error("API error: {0}")]
    Api(String),

    #[error("Authentication failed: {0}")]
    Auth(String),

    #[error("Serialization error: {0}")]
    Serialization(#[from] serde_json::Error),

    #[error("Configuration error: {0}")]
    Config(String),

    #[error("Stream error: {0}")]
    Stream(String),

    #[error("Model not found: {0}")]
    ModelNotFound(String),

    #[error("Network error: {0}")]
    Network(String),

    #[error("Rate limited: {0}")]
    RateLimited(String),

    #[error("Server error: {0}")]
    ServerError(String),

    #[error("Timed out: {0}")]
    Timeout(String),
}

#[async_trait]
pub trait Provider: Send + Sync {
    fn key(&self) -> &str;
    fn name(&self) -> &str;

    async fn list_models(&self) -> Result<Vec<ModelInfo>>;

    async fn complete(&self, request: CompletionRequest) -> Result<EventStream>;

    async fn health_check(&self) -> Result<bool> {
        Ok(true)
    }

    fn info(&self) -> ProviderInfo {
        ProviderInfo {
            key: self.key().to_string(),
            name: self.name().to_string(),
            models: vec![],
        }
    }
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct ChatCompletionRequest {
    pub model: String,
    pub messages: Vec<ChatMessage>,

    #[serde(
        skip_serializing_if = "Vec::is_empty",
        serialize_with = "serialize_openai_tools"
    )]
    pub tools: Vec<ToolDefinition>,

    #[serde(skip_serializing_if = "Option::is_none")]
    pub system: Option<String>,

    pub stream: bool,

    #[serde(skip_serializing_if = "Option::is_none")]
    pub temperature: Option<f32>,

    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_tokens: Option<u32>,
}

/// Serialize SC Node tool definitions in the OpenAI-compatible
/// `{"type":"function","function":{name,description,parameters}}` envelope
/// that NVIDIA NIM, OpenRouter, and other strict OpenAI-compatible endpoints
/// require. Sending the bare `{name,description,parameters}` `ToolDefinition`
/// shape is rejected by NVIDIA NIM with HTTP 400 `missing field "type"`.
/// Centralised here so every OpenAI-compatible provider is correct; no
/// provider-specific tool formatting exists.
fn serialize_openai_tools<S>(
    tools: &[ToolDefinition],
    serializer: S,
) -> std::result::Result<S::Ok, S::Error>
where
    S: serde::Serializer,
{
    use serde::ser::SerializeSeq;

    #[derive(serde::Serialize)]
    struct FunctionRef<'a> {
        name: &'a str,
        description: &'a str,
        parameters: &'a serde_json::Value,
    }
    #[derive(serde::Serialize)]
    struct ToolRef<'a> {
        #[serde(rename = "type")]
        kind: &'static str,
        function: FunctionRef<'a>,
    }

    let mut seq = serializer.serialize_seq(Some(tools.len()))?;
    for t in tools {
        seq.serialize_element(&ToolRef {
            kind: "function",
            function: FunctionRef {
                name: &t.name,
                description: &t.description,
                parameters: &t.parameters,
            },
        })?;
    }
    seq.end()
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct ChatMessage {
    pub role: String,

    #[serde(skip_serializing_if = "Option::is_none")]
    pub content: Option<String>,

    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,

    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_calls: Option<Vec<ToolCall>>,

    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_call_id: Option<String>,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct ToolCall {
    pub id: String,

    #[serde(rename = "type")]
    pub kind: String,

    pub function: ToolCallFunction,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct ToolCallFunction {
    pub name: String,
    pub arguments: String,
}

#[derive(Debug, Clone, serde::Deserialize)]
pub struct ChatCompletionChunk {
    pub choices: Vec<ChatChoiceDelta>,
}

#[derive(Debug, Clone, serde::Deserialize)]
pub struct ChatChoiceDelta {
    pub delta: Option<ChatDelta>,
    pub finish_reason: Option<String>,
}

#[derive(Debug, Clone, serde::Deserialize)]
pub struct ChatDelta {
    pub content: Option<String>,
}

pub fn message_to_chat_message(message: Message) -> ChatMessage {
    let role = match message.role {
        Role::System => "system",
        Role::User => "user",
        Role::Assistant => "assistant",
        Role::Tool => "tool",
    }
    .to_string();

    let mut text = String::new();
    let mut tool_calls: Vec<ToolCall> = Vec::new();
    let mut tool_call_id: Option<String> = None;

    for block in message.content {
        match block {
            ContentBlock::Text { text: t } => {
                if !text.is_empty() {
                    text.push('\n');
                }
                text.push_str(&t);
            }
            ContentBlock::ToolResult {
                tool_use_id,
                content,
                ..
            } => {
                if !text.is_empty() {
                    text.push('\n');
                }
                text.push_str(&content);
                // Strict OpenAI-compatible endpoints (e.g. NVIDIA NIM) reject a
                // `tool` role message that lacks the id linking it to the
                // assistant's tool call.
                if tool_call_id.is_none() {
                    tool_call_id = Some(tool_use_id);
                }
            }
            ContentBlock::ToolUse { id, name, input } => {
                // The assistant turn must replay its tool calls, otherwise the
                // following `tool` message has no antecedent and strict
                // endpoints reject the conversation.
                tool_calls.push(ToolCall {
                    id,
                    kind: "function".to_string(),
                    function: ToolCallFunction {
                        name,
                        arguments: input.to_string(),
                    },
                });
            }
        }
    }

    ChatMessage {
        role,
        content: if text.is_empty() { None } else { Some(text) },
        name: message.name,
        tool_calls: if tool_calls.is_empty() {
            None
        } else {
            Some(tool_calls)
        },
        tool_call_id,
    }
}

pub fn chunk_to_stream_event(chunk: ChatCompletionChunk) -> Vec<StreamEvent> {
    let mut events = Vec::new();

    for choice in chunk.choices {
        if let Some(reason) = choice.finish_reason {
            events.push(StreamEvent::End {
                finish_reason: Some(reason),
            });
        }
    }

    events
}

#[cfg(test)]
mod tool_serialization_tests {
    use super::*;

    fn tool(name: &str, desc: &str, params: serde_json::Value) -> ToolDefinition {
        ToolDefinition {
            name: name.into(),
            description: desc.into(),
            parameters: params,
        }
    }

    fn request_with_tools(tools: Vec<ToolDefinition>) -> ChatCompletionRequest {
        ChatCompletionRequest {
            model: "m".into(),
            messages: vec![],
            tools,
            system: None,
            stream: false,
            temperature: None,
            max_tokens: None,
        }
    }

    #[test]
    fn tools_serialize_in_openai_function_envelope() {
        let req = request_with_tools(vec![
            tool(
                "get_weather",
                "Get weather",
                serde_json::json!({"type":"object","properties":{"city":{"type":"string"}},"required":["city"]}),
            ),
            tool(
                "list_files",
                "List files",
                serde_json::json!({"type":"object"}),
            ),
        ]);
        let v = serde_json::to_value(&req).unwrap();
        let tools = v.get("tools").expect("tools present").as_array().unwrap();
        assert_eq!(tools.len(), 2);

        for (t, name) in tools.iter().zip(["get_weather", "list_files"]) {
            // 1/5: every entry carries "type": "function".
            assert_eq!(t.get("type").and_then(|x| x.as_str()), Some("function"));
            let f = t.get("function").expect("function object present");
            // 2: name preserved. 3: description preserved. 4: parameters preserved.
            assert_eq!(f.get("name").and_then(|x| x.as_str()), Some(name));
            assert!(f.get("description").and_then(|x| x.as_str()).is_some());
            assert!(f.get("parameters").is_some());
            // The bare ToolDefinition shape must NOT leak to the top level.
            assert!(t.get("name").is_none());
            assert!(t.get("parameters").is_none());
        }
        // 4 (exact schema preserved through the envelope):
        assert_eq!(
            tools[0]["function"]["parameters"]["properties"]["city"]["type"],
            "string"
        );
    }

    #[test]
    fn no_tools_omits_the_tools_field() {
        // 6: a request with no tools omits the field entirely.
        let v = serde_json::to_value(request_with_tools(vec![])).unwrap();
        assert!(v.get("tools").is_none());
    }

    #[test]
    fn tool_result_message_carries_tool_call_id() {
        // Strict endpoints (NVIDIA NIM) reject a `tool` message without the
        // id linking it back to the assistant's tool call.
        let msg = Message {
            role: Role::Tool,
            content: vec![ContentBlock::ToolResult {
                tool_use_id: "call_42".into(),
                content: "exit 0".into(),
                is_error: false,
            }],
            name: None,
        };
        let v = serde_json::to_value(message_to_chat_message(msg)).unwrap();
        assert_eq!(v["role"], "tool");
        assert_eq!(v["tool_call_id"], "call_42");
        assert_eq!(v["content"], "exit 0");
    }

    #[test]
    fn assistant_message_replays_tool_calls() {
        // The assistant turn must include its tool_calls so the following
        // `tool` message has an antecedent.
        let msg = Message {
            role: Role::Assistant,
            content: vec![ContentBlock::ToolUse {
                id: "call_42".into(),
                name: "shell".into(),
                input: serde_json::json!({"cmd": "ls"}),
            }],
            name: None,
        };
        let v = serde_json::to_value(message_to_chat_message(msg)).unwrap();
        assert_eq!(v["role"], "assistant");
        let calls = v["tool_calls"].as_array().expect("tool_calls present");
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0]["id"], "call_42");
        assert_eq!(calls[0]["type"], "function");
        assert_eq!(calls[0]["function"]["name"], "shell");
        // arguments must be a JSON-encoded string, not an object.
        let args = calls[0]["function"]["arguments"]
            .as_str()
            .expect("arguments is a string");
        assert_eq!(
            serde_json::from_str::<serde_json::Value>(args).unwrap(),
            serde_json::json!({"cmd": "ls"})
        );
        // No stray tool_call_id on assistant messages.
        assert!(v.get("tool_call_id").is_none());
    }

    #[test]
    fn plain_text_message_omits_tool_fields() {
        let msg = Message {
            role: Role::User,
            content: vec![ContentBlock::Text {
                text: "hello".into(),
            }],
            name: None,
        };
        let v = serde_json::to_value(message_to_chat_message(msg)).unwrap();
        assert!(v.get("tool_calls").is_none());
        assert!(v.get("tool_call_id").is_none());
    }
}
