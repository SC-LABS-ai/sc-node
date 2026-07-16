//! A reusable, OpenAI-compatible HTTP provider core.
//!
//! This module centralizes the plumbing that is identical across every
//! OpenAI-compatible provider SC Node talks to (NVIDIA NIM, OpenRouter, and
//! any future addition): a configurable base URL, an API key resolved from
//! a *named* environment variable (never a literal secret in code or
//! config), the `/models` and `/chat/completions` endpoints, request
//! timeouts, a small bounded retry policy for transient failures, and
//! typed, categorized errors with secrets always redacted before they can
//! reach a log line or an error message.
//!
//! [`OpenAiCompatClient::chat_completion`] performs a single non-streaming
//! JSON call (`stream` forced to `false` on the wire, so the response is
//! always one JSON document we can parse deterministically).
//! [`OpenAiCompatClient::chat_completion_stream`] performs the real
//! incremental streaming call (`stream` forced to `true`), decoding the
//! response body as it arrives via [`crate::sse::SseDecoder`] rather than
//! buffering it whole.
//!
//! Retry policy: 401/403/429 are never retried (retrying with the same
//! credentials/rate limit would not help - see [`RoutingError`]-style
//! "stop, don't silently paper over it" philosophy). 5xx responses and
//! network-level timeouts/connect failures are retried up to
//! `max_retries` bounded attempts; once that bound is reached the call
//! stops and returns a categorized [`ProviderError`]. A failure is never
//! turned into a fabricated success.

use crate::sse::SseDecoder;
use crate::{ChatCompletionRequest, ChatMessage, EventStream, ProviderError, Result};
use futures::StreamExt;
use reqwest::{Client, Method, StatusCode};
use sc_message_types::StreamEvent;
use std::collections::VecDeque;
use std::time::Duration;

/// Configuration for an [`OpenAiCompatClient`].
#[derive(Debug, Clone)]
pub struct OpenAiCompatConfig {
    /// Base URL, e.g. `"https://integrate.api.nvidia.com/v1"`. Trailing
    /// slashes are tolerated.
    pub base_url: String,
    /// Name of the environment variable holding the API key. This is a
    /// variable *name*, never the secret value itself.
    pub api_key_env: String,
    /// Per-request timeout.
    pub timeout: Duration,
    /// Maximum number of retries for retryable failures (5xx, timeout,
    /// connect errors). `0` disables retries entirely.
    pub max_retries: u32,
    /// Delay between retry attempts.
    pub retry_backoff: Duration,
}

impl OpenAiCompatConfig {
    pub fn new(base_url: impl Into<String>, api_key_env: impl Into<String>) -> Self {
        Self {
            base_url: base_url.into(),
            api_key_env: api_key_env.into(),
            timeout: Duration::from_secs(60),
            max_retries: 2,
            retry_backoff: Duration::from_millis(200),
        }
    }

    pub fn with_timeout(mut self, timeout: Duration) -> Self {
        self.timeout = timeout;
        self
    }

    pub fn with_max_retries(mut self, max_retries: u32) -> Self {
        self.max_retries = max_retries;
        self
    }

    pub fn with_retry_backoff(mut self, backoff: Duration) -> Self {
        self.retry_backoff = backoff;
        self
    }
}

/// One entry from a `GET /models` response.
#[derive(Debug, Clone, serde::Deserialize)]
pub struct OpenAiModel {
    pub id: String,
    #[serde(default)]
    pub object: Option<String>,
    #[serde(default)]
    pub created: Option<i64>,
    #[serde(default)]
    pub owned_by: Option<String>,
}

#[derive(Debug, Clone, serde::Deserialize)]
struct OpenAiModelsResponse {
    data: Vec<OpenAiModel>,
}

#[derive(Debug, Clone, serde::Deserialize)]
struct OpenAiChatCompletionResponse {
    choices: Vec<OpenAiChatChoice>,
}

#[derive(Debug, Clone, serde::Deserialize)]
struct OpenAiChatChoice {
    message: ChatMessage,
    #[serde(default)]
    finish_reason: Option<String>,
}

/// The result of a non-streaming chat completion call.
#[derive(Debug, Clone)]
pub struct ChatCompletionOutcome {
    pub message: ChatMessage,
    pub finish_reason: Option<String>,
}

/// Convert a single, already-complete chat completion response into the
/// same [`StreamEvent`] shape a real incremental stream would have
/// produced: an optional text delta, one [`StreamEvent::ToolUse`] per
/// tool call, then an [`StreamEvent::End`]. Shared by every
/// OpenAI-compatible provider that performs a one-shot call today and
/// still needs to hand a normal event stream back to callers.
pub fn outcome_to_stream_events(outcome: ChatCompletionOutcome) -> Vec<Result<StreamEvent>> {
    let mut events = Vec::new();

    if let Some(text) = outcome.message.content
        && !text.is_empty()
    {
        events.push(Ok(StreamEvent::TextDelta { text }));
    }

    if let Some(tool_calls) = outcome.message.tool_calls {
        for tc in tool_calls {
            let input =
                serde_json::from_str(&tc.function.arguments).unwrap_or(serde_json::Value::Null);
            events.push(Ok(StreamEvent::ToolUse {
                id: tc.id,
                name: tc.function.name,
                input,
            }));
        }
    }

    events.push(Ok(StreamEvent::End {
        finish_reason: outcome.finish_reason,
    }));

    events
}

/// Build the `/chat/completions` request body for an OpenAI-compatible
/// endpoint. Forces `stream` to the given value, and folds SC Node's
/// top-level `system` prompt into a leading `{"role":"system"}` message:
/// OpenAI-compatible servers (NVIDIA NIM in particular) reject a top-level
/// `system` parameter with HTTP 400 `Unsupported parameter(s): system`.
/// Tools are already serialized in the correct `{"type":"function",...}`
/// envelope by `ChatCompletionRequest`'s serializer.
fn build_request_body(request: &ChatCompletionRequest, stream: bool) -> Result<serde_json::Value> {
    let mut body = serde_json::to_value(request).map_err(ProviderError::Serialization)?;
    if let Some(obj) = body.as_object_mut() {
        obj.insert("stream".into(), serde_json::Value::Bool(stream));
        if let Some(system) = obj
            .remove("system")
            .and_then(|v| v.as_str().map(str::to_string))
        {
            let sys_msg = serde_json::json!({ "role": "system", "content": system });
            match obj.get_mut("messages").and_then(|m| m.as_array_mut()) {
                Some(msgs) => msgs.insert(0, sys_msg),
                None => {
                    obj.insert("messages".into(), serde_json::Value::Array(vec![sys_msg]));
                }
            }
        }
    }
    Ok(body)
}

/// A thin HTTP client shared by every OpenAI-compatible provider.
pub struct OpenAiCompatClient {
    client: Client,
    config: OpenAiCompatConfig,
    api_key: Option<String>,
}

impl std::fmt::Debug for OpenAiCompatClient {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("OpenAiCompatClient")
            .field("config", &self.config)
            .field("api_key", &self.api_key.as_ref().map(|_| "<redacted>"))
            .finish()
    }
}

impl OpenAiCompatClient {
    /// Build a client from `config`. The API key is resolved from
    /// `config.api_key_env` once, at construction time.
    ///
    /// Refuses to construct a client that would attach a real credential
    /// to a non-`https` base URL unless that URL points at a local/
    /// loopback host (`localhost`, `127.0.0.1`, `::1`): sending a secret
    /// in cleartext to anything else is a plaintext-credential-leak risk,
    /// not something this crate will do silently.
    pub fn new(config: OpenAiCompatConfig) -> Result<Self> {
        let api_key = std::env::var(&config.api_key_env)
            .ok()
            .filter(|k| !k.is_empty());

        if api_key.is_some() && !is_https_or_local_http(&config.base_url) {
            return Err(ProviderError::Config(format!(
                "refusing to attach an API key to non-https base_url '{}': use an https:// \
                 base_url, or only omit credentials for local endpoints (localhost/127.0.0.1/::1)",
                config.base_url
            )));
        }

        let client = Client::builder()
            .timeout(config.timeout)
            .build()
            .map_err(ProviderError::Http)?;

        Ok(Self {
            client,
            config,
            api_key,
        })
    }

    pub fn config(&self) -> &OpenAiCompatConfig {
        &self.config
    }

    /// Whether an API key was found for this client's configured
    /// environment variable at construction time.
    pub fn has_key(&self) -> bool {
        self.api_key.as_ref().is_some_and(|k| !k.is_empty())
    }

    fn base_url(&self) -> &str {
        self.config.base_url.trim_end_matches('/')
    }

    fn auth_header(&self) -> Result<String> {
        match &self.api_key {
            Some(key) if !key.is_empty() => Ok(format!("Bearer {key}")),
            _ => Err(ProviderError::Auth(format!(
                "API key not set. Set the {} environment variable.",
                self.config.api_key_env
            ))),
        }
    }

    /// Scrub the resolved API key out of any text before it is allowed
    /// into an error message. Defensive: protects against a misbehaving
    /// upstream echoing request headers back in an error body.
    fn redact(&self, text: &str) -> String {
        match &self.api_key {
            Some(key) if !key.is_empty() && text.contains(key.as_str()) => {
                text.replace(key.as_str(), "[REDACTED]")
            }
            _ => text.to_string(),
        }
    }

    /// `GET {base_url}/models`.
    pub async fn list_models(&self) -> Result<Vec<OpenAiModel>> {
        let resp: OpenAiModelsResponse = self.request_json(Method::GET, "/models", None).await?;
        Ok(resp.data)
    }

    /// `POST {base_url}/chat/completions` as a single non-streaming call.
    pub async fn chat_completion(
        &self,
        request: &ChatCompletionRequest,
    ) -> Result<ChatCompletionOutcome> {
        let body = build_request_body(request, false)?;

        let resp: OpenAiChatCompletionResponse = self
            .request_json(Method::POST, "/chat/completions", Some(&body))
            .await?;

        let choice = resp.choices.into_iter().next().ok_or_else(|| {
            ProviderError::Api("provider returned no choices in chat completion response".into())
        })?;

        Ok(ChatCompletionOutcome {
            message: choice.message,
            finish_reason: choice.finish_reason,
        })
    }

    /// `POST {base_url}/chat/completions` as a true incremental stream.
    ///
    /// The response body is never buffered whole: bytes are handed to an
    /// [`SseDecoder`] as they arrive and decoded events are yielded from
    /// the returned stream immediately. The initial request (up to and
    /// including the response status line) follows the same bounded
    /// retry/categorized-error policy as [`Self::chat_completion`]; once
    /// the body itself starts streaming, an error (malformed frame,
    /// oversized line, a transport error mid-body) ends the stream as one
    /// final `Err` item rather than being retried, since a
    /// partially-consumed stream cannot be safely replayed. Dropping the
    /// returned stream before it is exhausted cancels the underlying HTTP
    /// request.
    pub async fn chat_completion_stream(
        &self,
        request: &ChatCompletionRequest,
    ) -> Result<EventStream> {
        let body = build_request_body(request, true)?;

        let url = format!("{}/chat/completions", self.base_url());
        let mut attempt: u32 = 0;

        let response = loop {
            let auth = self.auth_header()?;
            let builder = self
                .client
                .post(&url)
                .header("Authorization", auth)
                .header("Content-Type", "application/json")
                .json(&body);

            let err: ProviderError = match builder.send().await {
                Ok(resp) if resp.status().is_success() => break resp,
                Ok(resp) => {
                    let status = resp.status();
                    let body_text = resp.text().await.unwrap_or_default();
                    self.categorize_status_error(status, &self.redact(&body_text))
                }
                Err(e) => self.categorize_transport_error(e),
            };

            if is_retryable(&err) && attempt < self.config.max_retries {
                attempt += 1;
                tokio::time::sleep(self.config.retry_backoff).await;
                continue;
            }
            return Err(err);
        };

        let byte_stream = response.bytes_stream();
        let decoder = SseDecoder::new();
        let pending: VecDeque<Result<StreamEvent>> = VecDeque::new();

        let event_stream = futures::stream::unfold(
            (byte_stream, decoder, pending, false),
            |(mut byte_stream, mut decoder, mut pending, mut ended)| async move {
                loop {
                    if let Some(item) = pending.pop_front() {
                        return Some((item, (byte_stream, decoder, pending, ended)));
                    }
                    if ended {
                        return None;
                    }
                    match byte_stream.next().await {
                        Some(Ok(bytes)) => {
                            pending.extend(decoder.feed(&bytes));
                        }
                        Some(Err(e)) => {
                            pending.push_back(Err(ProviderError::Http(e)));
                            ended = true;
                        }
                        None => {
                            pending.extend(decoder.finish());
                            ended = true;
                        }
                    }
                }
            },
        );

        Ok(Box::pin(event_stream))
    }

    async fn request_json<T: serde::de::DeserializeOwned>(
        &self,
        method: Method,
        path: &str,
        body: Option<&serde_json::Value>,
    ) -> Result<T> {
        let url = format!("{}{}", self.base_url(), path);
        let mut attempt: u32 = 0;

        loop {
            let auth = self.auth_header()?;
            let mut builder = self.client.request(method.clone(), &url);
            builder = builder.header("Authorization", auth);
            if let Some(b) = body {
                builder = builder.header("Content-Type", "application/json").json(b);
            }

            let err: ProviderError = match builder.send().await {
                Ok(resp) => {
                    let status = resp.status();
                    if status.is_success() {
                        let text = resp.text().await.map_err(ProviderError::Http)?;
                        return serde_json::from_str(&text).map_err(ProviderError::Serialization);
                    }
                    let body_text = resp.text().await.unwrap_or_default();
                    self.categorize_status_error(status, &self.redact(&body_text))
                }
                Err(e) => self.categorize_transport_error(e),
            };

            if is_retryable(&err) && attempt < self.config.max_retries {
                attempt += 1;
                tokio::time::sleep(self.config.retry_backoff).await;
                continue;
            }
            return Err(err);
        }
    }

    fn categorize_status_error(&self, status: StatusCode, body: &str) -> ProviderError {
        let code = status.as_u16();
        match code {
            401 | 403 => ProviderError::Auth(format!("authentication failed ({code}): {body}")),
            429 => ProviderError::RateLimited(format!("rate limited ({code}): {body}")),
            404 => ProviderError::ModelNotFound(format!("not found ({code}): {body}")),
            500..=599 => ProviderError::ServerError(format!("server error ({code}): {body}")),
            _ => ProviderError::Api(format!("unexpected status {code}: {body}")),
        }
    }

    fn categorize_transport_error(&self, e: reqwest::Error) -> ProviderError {
        if e.is_timeout() {
            ProviderError::Timeout(self.redact(&e.to_string()))
        } else if e.is_connect() {
            ProviderError::Network(self.redact(&e.to_string()))
        } else {
            ProviderError::Http(e)
        }
    }
}

fn is_retryable(err: &ProviderError) -> bool {
    matches!(
        err,
        ProviderError::ServerError(_) | ProviderError::Timeout(_) | ProviderError::Network(_)
    )
}

/// Whether `base_url` is safe to attach a real credential to: either
/// `https://` (any host), or `http://` pointed at a local/loopback host.
/// Anything else (plain `http://` to a non-local host, or an
/// unrecognized/missing scheme) is treated conservatively as unsafe.
fn is_https_or_local_http(base_url: &str) -> bool {
    let lower = base_url.to_ascii_lowercase();
    if let Some(rest) = lower.strip_prefix("https://") {
        let _ = rest;
        return true;
    }
    match lower.strip_prefix("http://") {
        Some(rest) => is_local_host(extract_host(rest)),
        None => false,
    }
}

/// Extract the host portion (no userinfo, no port, no path/query/
/// fragment) from the part of a URL following its `http://`/`https://`
/// scheme.
fn extract_host(rest_after_scheme: &str) -> &str {
    let after_userinfo = rest_after_scheme
        .rsplit('@')
        .next()
        .unwrap_or(rest_after_scheme);
    let end = after_userinfo
        .find(['/', '?', '#'])
        .unwrap_or(after_userinfo.len());
    let host_and_port = &after_userinfo[..end];

    if let Some(bracketed) = host_and_port.strip_prefix('[') {
        // IPv6 literal, e.g. "[::1]:8080".
        bracketed.split(']').next().unwrap_or(bracketed)
    } else {
        host_and_port.split(':').next().unwrap_or(host_and_port)
    }
}

fn is_local_host(host: &str) -> bool {
    let host = host.to_ascii_lowercase();
    host == "localhost" || host == "127.0.0.1" || host == "::1" || host.starts_with("127.")
}

#[cfg(test)]
mod tests {
    use super::*;
    use sc_message_types::Message;
    use std::collections::{HashMap, VecDeque};
    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
    use std::sync::{Arc, Mutex};
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::{TcpListener, TcpStream};

    // ── hand-rolled HTTP/1.1 mock server (no external test dependency) ──

    struct MockResponse {
        status: u16,
        body: String,
    }

    impl MockResponse {
        fn json(status: u16, body: impl Into<String>) -> Self {
            Self {
                status,
                body: body.into(),
            }
        }
    }

    struct MockServer {
        base_url: String,
        hits: Arc<AtomicUsize>,
    }

    impl MockServer {
        fn hit_count(&self) -> usize {
            self.hits.load(Ordering::SeqCst)
        }
    }

    /// Start a server that answers each received request with the next
    /// canned response from `responses` (repeating the last response once
    /// the queue is drained). `{{AUTH}}` in a response body is replaced
    /// with the raw `Authorization` header value the server received -
    /// used to prove secret redaction happens on our side, not the
    /// server's.
    async fn start_mock_server(responses: Vec<MockResponse>) -> MockServer {
        let listener = TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind mock listener");
        let addr = listener.local_addr().expect("mock listener addr");
        let queue = Arc::new(Mutex::new(VecDeque::from(responses)));
        let hits = Arc::new(AtomicUsize::new(0));
        let hits_for_task = hits.clone();

        tokio::spawn(async move {
            loop {
                let (socket, _) = match listener.accept().await {
                    Ok(pair) => pair,
                    Err(_) => break,
                };
                tokio::spawn(handle_connection(
                    socket,
                    queue.clone(),
                    hits_for_task.clone(),
                    false,
                ));
            }
        });

        MockServer {
            base_url: format!("http://{addr}"),
            hits,
        }
    }

    /// Start a server that accepts connections, fully reads each request,
    /// and then never responds - used to force a client-side timeout.
    async fn start_hanging_mock_server() -> MockServer {
        let listener = TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind mock listener");
        let addr = listener.local_addr().expect("mock listener addr");
        let hits = Arc::new(AtomicUsize::new(0));
        let hits_for_task = hits.clone();

        tokio::spawn(async move {
            loop {
                let (socket, _) = match listener.accept().await {
                    Ok(pair) => pair,
                    Err(_) => break,
                };
                tokio::spawn(handle_connection(
                    socket,
                    Arc::new(Mutex::new(VecDeque::new())),
                    hits_for_task.clone(),
                    true,
                ));
            }
        });

        MockServer {
            base_url: format!("http://{addr}"),
            hits,
        }
    }

    struct FragmentedServer {
        base_url: String,
        /// Set only once every fragment has been written and the socket
        /// was cleanly shut down - i.e. the client read the response to
        /// completion rather than dropping the stream early.
        fully_sent: Arc<AtomicBool>,
        hits: Arc<AtomicUsize>,
    }

    /// Start a server that streams a `text/event-stream` response body as
    /// a sequence of arbitrary, deliberately non-aligned byte fragments
    /// (each its own `write`, with `delay_between` paced between them),
    /// with no `Content-Length` header - the body is delimited purely by
    /// the connection closing, so a client reading it incrementally
    /// (rather than buffering the whole response) is required to see
    /// events as they arrive rather than only once the connection ends.
    async fn start_fragmented_sse_server(
        fragments: Vec<Vec<u8>>,
        delay_between: Duration,
    ) -> FragmentedServer {
        let listener = TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind mock listener");
        let addr = listener.local_addr().expect("mock listener addr");
        let fully_sent = Arc::new(AtomicBool::new(false));
        let hits = Arc::new(AtomicUsize::new(0));
        let fully_sent_task = fully_sent.clone();
        let hits_task = hits.clone();

        tokio::spawn(async move {
            loop {
                let (socket, _) = match listener.accept().await {
                    Ok(pair) => pair,
                    Err(_) => break,
                };
                hits_task.fetch_add(1, Ordering::SeqCst);
                tokio::spawn(serve_fragments(
                    socket,
                    fragments.clone(),
                    delay_between,
                    fully_sent_task.clone(),
                ));
            }
        });

        FragmentedServer {
            base_url: format!("http://{addr}"),
            fully_sent,
            hits,
        }
    }

    async fn serve_fragments(
        mut socket: TcpStream,
        fragments: Vec<Vec<u8>>,
        delay_between: Duration,
        fully_sent: Arc<AtomicBool>,
    ) {
        if read_request_headers(&mut socket).await.is_none() {
            return;
        }

        let header =
            "HTTP/1.1 200 RESPONSE\r\nContent-Type: text/event-stream\r\nConnection: close\r\n\r\n";
        if socket.write_all(header.as_bytes()).await.is_err() {
            return;
        }

        for fragment in &fragments {
            if socket.write_all(fragment).await.is_err() {
                return; // client disconnected mid-stream: cancellation
            }
            if !delay_between.is_zero() {
                tokio::time::sleep(delay_between).await;
            }
        }

        fully_sent.store(true, Ordering::SeqCst);
        let _ = socket.shutdown().await;
    }

    async fn handle_connection(
        mut socket: TcpStream,
        queue: Arc<Mutex<VecDeque<MockResponse>>>,
        hits: Arc<AtomicUsize>,
        hang: bool,
    ) {
        loop {
            let Some(headers) = read_request_headers(&mut socket).await else {
                return;
            };
            hits.fetch_add(1, Ordering::SeqCst);

            if hang {
                // Hold the connection open indefinitely; the client is
                // expected to time out and give up long before this
                // returns.
                tokio::time::sleep(Duration::from_secs(30)).await;
                return;
            }

            let auth = headers.get("authorization").cloned().unwrap_or_default();

            let resp = {
                let mut guard = queue.lock().unwrap();
                if guard.len() > 1 {
                    guard.pop_front().unwrap()
                } else if let Some(last) = guard.front() {
                    MockResponse::json(last.status, last.body.clone())
                } else {
                    MockResponse::json(500, "no canned responses configured")
                }
            };
            let body = resp.body.replace("{{AUTH}}", &auth);

            let raw = format!(
                "HTTP/1.1 {} RESPONSE\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: keep-alive\r\n\r\n{}",
                resp.status,
                body.len(),
                body,
            );

            if socket.write_all(raw.as_bytes()).await.is_err() {
                return;
            }
        }
    }

    async fn read_request_headers(socket: &mut TcpStream) -> Option<HashMap<String, String>> {
        let mut buf = Vec::new();
        let mut chunk = [0u8; 1024];

        let header_end = loop {
            let n = socket.read(&mut chunk).await.ok()?;
            if n == 0 {
                return None;
            }
            buf.extend_from_slice(&chunk[..n]);
            if let Some(pos) = buf.windows(4).position(|window| window == b"\r\n\r\n") {
                break pos;
            }
            if buf.len() > 64 * 1024 {
                return None;
            }
        };

        let header_text = String::from_utf8_lossy(&buf[..header_end]).to_string();
        let mut lines = header_text.split("\r\n");
        let _request_line = lines.next().unwrap_or_default();

        let mut headers = HashMap::new();
        let mut content_length: usize = 0;
        for line in lines {
            if let Some((k, v)) = line.split_once(':') {
                let key = k.trim().to_lowercase();
                let value = v.trim().to_string();
                if key == "content-length" {
                    content_length = value.parse().unwrap_or(0);
                }
                headers.insert(key, value);
            }
        }

        let mut body_read = buf.len().saturating_sub(header_end + 4);
        while body_read < content_length {
            let n = socket.read(&mut chunk).await.ok()?;
            if n == 0 {
                break;
            }
            body_read += n;
        }

        Some(headers)
    }

    /// A mock server that parses the request body and REJECTS (HTTP 400,
    /// mirroring NVIDIA NIM's real error) any chat request whose `tools`
    /// entries are missing the `{"type":"function","function":{...}}`
    /// envelope. `valid` is set true only when every tool was well-formed.
    async fn start_tool_type_validating_server() -> (MockServer, Arc<AtomicBool>) {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let listener = TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind mock listener");
        let addr = listener.local_addr().expect("mock listener addr");
        let hits = Arc::new(AtomicUsize::new(0));
        let hits_task = hits.clone();
        let valid = Arc::new(AtomicBool::new(false));
        let valid_task = valid.clone();

        tokio::spawn(async move {
            loop {
                let (mut socket, _) = match listener.accept().await {
                    Ok(pair) => pair,
                    Err(_) => break,
                };
                let hits_task = hits_task.clone();
                let valid_task = valid_task.clone();
                tokio::spawn(async move {
                    hits_task.fetch_add(1, Ordering::SeqCst);
                    let mut buf = Vec::new();
                    let mut chunk = [0u8; 4096];
                    let header_end = loop {
                        let n = match socket.read(&mut chunk).await {
                            Ok(n) => n,
                            Err(_) => return,
                        };
                        if n == 0 {
                            return;
                        }
                        buf.extend_from_slice(&chunk[..n]);
                        if let Some(pos) = buf.windows(4).position(|w| w == b"\r\n\r\n") {
                            break pos;
                        }
                        if buf.len() > 256 * 1024 {
                            return;
                        }
                    };
                    let header_text = String::from_utf8_lossy(&buf[..header_end]).to_string();
                    let mut content_length = 0usize;
                    for line in header_text.split("\r\n").skip(1) {
                        if let Some((k, v)) = line.split_once(':')
                            && k.trim().eq_ignore_ascii_case("content-length")
                        {
                            content_length = v.trim().parse().unwrap_or(0);
                        }
                    }
                    let mut body = buf[header_end + 4..].to_vec();
                    while body.len() < content_length {
                        let n = match socket.read(&mut chunk).await {
                            Ok(n) => n,
                            Err(_) => break,
                        };
                        if n == 0 {
                            break;
                        }
                        body.extend_from_slice(&chunk[..n]);
                    }
                    // Mirror NVIDIA NIM: reject a top-level `system` parameter,
                    // and require every tool in the OpenAI function envelope.
                    let ok = match serde_json::from_slice::<serde_json::Value>(&body) {
                        Ok(v) => {
                            let no_top_level_system = v.get("system").is_none();
                            let tools_ok = v
                                .get("tools")
                                .and_then(|t| t.as_array())
                                .map(|arr| {
                                    !arr.is_empty()
                                        && arr.iter().all(|t| {
                                            t.get("type").and_then(|x| x.as_str())
                                                == Some("function")
                                                && t.get("function")
                                                    .and_then(|f| f.get("name"))
                                                    .is_some()
                                        })
                                })
                                .unwrap_or(false);
                            no_top_level_system && tools_ok
                        }
                        Err(_) => false,
                    };
                    let (status, reason, resp_body) = if ok {
                        valid_task.store(true, Ordering::SeqCst);
                        (
                            200,
                            "OK",
                            r#"{"choices":[{"finish_reason":"stop","message":{"role":"assistant","content":"ok"}}]}"#,
                        )
                    } else {
                        (
                            400,
                            "Bad Request",
                            r#"{"error":{"message":"missing field `type`","type":"Bad Request","code":400}}"#,
                        )
                    };
                    let response = format!(
                        "HTTP/1.1 {status} {reason}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                        resp_body.len(),
                        resp_body
                    );
                    let _ = socket.write_all(response.as_bytes()).await;
                    let _ = socket.flush().await;
                });
            }
        });

        (
            MockServer {
                base_url: format!("http://{addr}"),
                hits,
            },
            valid,
        )
    }

    #[test]
    fn build_request_body_folds_system_into_leading_message() {
        let mut req = sample_request();
        req.system = Some("You are helpful".into());
        req.messages = vec![crate::message_to_chat_message(Message::user("hi"))];
        let body = build_request_body(&req, false).unwrap();
        let obj = body.as_object().unwrap();
        assert!(
            obj.get("system").is_none(),
            "top-level `system` must not be sent (NVIDIA NIM rejects it)"
        );
        let msgs = obj.get("messages").unwrap().as_array().unwrap();
        assert_eq!(msgs[0]["role"], "system");
        assert_eq!(msgs[0]["content"], "You are helpful");
        assert_eq!(msgs[1]["role"], "user");
        assert_eq!(obj.get("stream").unwrap(), &serde_json::Value::Bool(false));
    }

    #[tokio::test]
    async fn tools_sent_with_function_type_are_accepted_by_strict_server() {
        // The server rejects (400) any tool missing `"type": "function"` -
        // exactly like NVIDIA NIM. If our shared serializer is correct, the
        // request is accepted; a regression to the bare ToolDefinition shape
        // would make this fail with the same 400 seen in production.
        let (server, valid) = start_tool_type_validating_server().await;
        set_test_env("SC_TEST_TOOLTYPE_KEY", "fake-tooltype-secret");
        let client =
            OpenAiCompatClient::new(config_for(&server.base_url, "SC_TEST_TOOLTYPE_KEY")).unwrap();

        let mut req = sample_request();
        req.system = Some("You are a helpful assistant".into());
        req.tools = vec![sc_message_types::ToolDefinition {
            name: "get_weather".into(),
            description: "Get the weather".into(),
            parameters: serde_json::json!({"type":"object","properties":{"city":{"type":"string"}}}),
        }];
        let outcome = client.chat_completion(&req).await;
        remove_test_env("SC_TEST_TOOLTYPE_KEY");

        assert!(
            outcome.is_ok(),
            "strict server rejected our tool serialization: {outcome:?}"
        );
        assert!(
            valid.load(Ordering::SeqCst),
            "server did not observe a well-formed function-tool envelope"
        );
    }

    // ── test helpers ──────────────────────────────────────────────────

    /// Tests run concurrently in the same process, so each test uses its
    /// own, never-reused environment variable name to avoid cross-test
    /// races on process-global env state.
    fn set_test_env(name: &str, value: &str) {
        // SAFETY: each test uses a unique env var name it owns exclusively,
        // so there is no concurrent reader/writer of the same name.
        unsafe { std::env::set_var(name, value) };
    }

    fn remove_test_env(name: &str) {
        // SAFETY: see `set_test_env`.
        unsafe { std::env::remove_var(name) };
    }

    fn config_for(base_url: &str, env_var: &str) -> OpenAiCompatConfig {
        OpenAiCompatConfig::new(base_url, env_var)
            .with_timeout(Duration::from_millis(500))
            .with_max_retries(2)
            .with_retry_backoff(Duration::from_millis(5))
    }

    fn sample_request() -> ChatCompletionRequest {
        ChatCompletionRequest {
            model: "test-model".into(),
            messages: vec![crate::message_to_chat_message(Message::user("hi"))],
            tools: vec![],
            system: None,
            stream: true, // deliberately true: the client must force false on the wire
            temperature: None,
            max_tokens: None,
        }
    }

    // ── model list parse ─────────────────────────────────────────────

    #[tokio::test]
    async fn list_models_parses_response() {
        let server = start_mock_server(vec![MockResponse::json(
            200,
            r#"{"data":[{"id":"model-a","object":"model","created":1,"owned_by":"acme"},{"id":"model-b"}]}"#,
        )])
        .await;
        set_test_env("SC_TEST_LIST_MODELS_KEY", "fake-list-models-secret");
        let client =
            OpenAiCompatClient::new(config_for(&server.base_url, "SC_TEST_LIST_MODELS_KEY"))
                .unwrap();

        let models = client.list_models().await.unwrap();

        assert_eq!(models.len(), 2);
        assert_eq!(models[0].id, "model-a");
        assert_eq!(models[1].id, "model-b");
        assert_eq!(server.hit_count(), 1);
    }

    // ── chat completion parse ────────────────────────────────────────

    #[tokio::test]
    async fn chat_completion_parses_text_response() {
        let server = start_mock_server(vec![MockResponse::json(
            200,
            r#"{"choices":[{"finish_reason":"stop","message":{"role":"assistant","content":"Hello there"}}]}"#,
        )])
        .await;
        set_test_env("SC_TEST_CHAT_COMPLETION_KEY", "fake-chat-secret");
        let client =
            OpenAiCompatClient::new(config_for(&server.base_url, "SC_TEST_CHAT_COMPLETION_KEY"))
                .unwrap();

        let outcome = client.chat_completion(&sample_request()).await.unwrap();

        assert_eq!(outcome.message.content, Some("Hello there".to_string()));
        assert_eq!(outcome.finish_reason, Some("stop".to_string()));
    }

    // ── tool-call parse ───────────────────────────────────────────────

    #[tokio::test]
    async fn chat_completion_parses_tool_calls() {
        let server = start_mock_server(vec![MockResponse::json(
            200,
            r#"{"choices":[{"finish_reason":"tool_calls","message":{"role":"assistant","content":null,"tool_calls":[{"id":"call_1","type":"function","function":{"name":"get_weather","arguments":"{\"city\":\"Berlin\"}"}}]}}]}"#,
        )])
        .await;
        set_test_env("SC_TEST_TOOL_CALL_KEY", "fake-tool-secret");
        let client =
            OpenAiCompatClient::new(config_for(&server.base_url, "SC_TEST_TOOL_CALL_KEY")).unwrap();

        let outcome = client.chat_completion(&sample_request()).await.unwrap();

        let tool_calls = outcome.message.tool_calls.expect("expected tool calls");
        assert_eq!(tool_calls.len(), 1);
        assert_eq!(tool_calls[0].id, "call_1");
        assert_eq!(tool_calls[0].function.name, "get_weather");
        assert_eq!(tool_calls[0].function.arguments, r#"{"city":"Berlin"}"#);
    }

    // ── 401 / 403 / 429: stop, no retry, categorized ─────────────────

    #[tokio::test]
    async fn status_401_stops_without_retry() {
        let server = start_mock_server(vec![
            MockResponse::json(401, r#"{"error":"bad key"}"#),
            MockResponse::json(200, r#"{"data":[]}"#),
        ])
        .await;
        set_test_env("SC_TEST_401_KEY", "fake-401-secret");
        let client =
            OpenAiCompatClient::new(config_for(&server.base_url, "SC_TEST_401_KEY")).unwrap();

        let err = client.list_models().await.unwrap_err();

        assert!(matches!(err, ProviderError::Auth(_)));
        assert_eq!(server.hit_count(), 1, "401 must not be retried");
    }

    #[tokio::test]
    async fn status_403_stops_without_retry() {
        let server =
            start_mock_server(vec![MockResponse::json(403, r#"{"error":"forbidden"}"#)]).await;
        set_test_env("SC_TEST_403_KEY", "fake-403-secret");
        let client =
            OpenAiCompatClient::new(config_for(&server.base_url, "SC_TEST_403_KEY")).unwrap();

        let err = client.list_models().await.unwrap_err();

        assert!(matches!(err, ProviderError::Auth(_)));
        assert_eq!(server.hit_count(), 1, "403 must not be retried");
    }

    #[tokio::test]
    async fn status_429_stops_without_retry() {
        let server =
            start_mock_server(vec![MockResponse::json(429, r#"{"error":"rate limited"}"#)]).await;
        set_test_env("SC_TEST_429_KEY", "fake-429-secret");
        let client =
            OpenAiCompatClient::new(config_for(&server.base_url, "SC_TEST_429_KEY")).unwrap();

        let err = client.list_models().await.unwrap_err();

        assert!(matches!(err, ProviderError::RateLimited(_)));
        assert_eq!(server.hit_count(), 1, "429 must not be retried");
    }

    // ── 5xx: bounded retry, then stop and categorize ─────────────────

    #[tokio::test]
    async fn repeated_5xx_retries_then_stops() {
        let server =
            start_mock_server(vec![MockResponse::json(503, r#"{"error":"unavailable"}"#)]).await;
        set_test_env("SC_TEST_5XX_KEY", "fake-5xx-secret");
        let config = config_for(&server.base_url, "SC_TEST_5XX_KEY").with_max_retries(1);
        let client = OpenAiCompatClient::new(config).unwrap();

        let err = client.list_models().await.unwrap_err();

        assert!(matches!(err, ProviderError::ServerError(_)));
        assert_eq!(
            server.hit_count(),
            2,
            "expected exactly one retry (bounded), then stop"
        );
    }

    // ── timeout: bounded retry, then stop and categorize ─────────────

    #[tokio::test]
    async fn timeout_retries_then_stops() {
        let server = start_hanging_mock_server().await;
        set_test_env("SC_TEST_TIMEOUT_KEY", "fake-timeout-secret");
        let config = OpenAiCompatConfig::new(&server.base_url, "SC_TEST_TIMEOUT_KEY")
            .with_timeout(Duration::from_millis(60))
            .with_max_retries(1)
            .with_retry_backoff(Duration::from_millis(5));
        let client = OpenAiCompatClient::new(config).unwrap();

        let err = client.list_models().await.unwrap_err();

        assert!(matches!(err, ProviderError::Timeout(_)));
        assert_eq!(
            server.hit_count(),
            2,
            "expected exactly one retry (bounded), then stop"
        );
    }

    // ── never fabricate success on error ─────────────────────────────

    #[tokio::test]
    async fn error_is_never_turned_into_success() {
        let server = start_mock_server(vec![MockResponse::json(500, r#"{"error":"boom"}"#)]).await;
        set_test_env("SC_TEST_NO_FABRICATE_KEY", "fake-no-fabricate-secret");
        let config = config_for(&server.base_url, "SC_TEST_NO_FABRICATE_KEY").with_max_retries(0);
        let client = OpenAiCompatClient::new(config).unwrap();

        let result = client.list_models().await;

        assert!(result.is_err());
    }

    // ── secrets are always redacted ───────────────────────────────────

    #[tokio::test]
    async fn secret_is_redacted_even_when_echoed_back_by_server() {
        let server = start_mock_server(vec![MockResponse::json(
            401,
            r#"{"error":"bad key, received: {{AUTH}}"}"#,
        )])
        .await;
        set_test_env("SC_TEST_REDACT_KEY", "super-secret-value-do-not-leak");
        let client =
            OpenAiCompatClient::new(config_for(&server.base_url, "SC_TEST_REDACT_KEY")).unwrap();

        let err = client.list_models().await.unwrap_err();
        let message = err.to_string();

        assert!(
            !message.contains("super-secret-value-do-not-leak"),
            "secret leaked into error message: {message}"
        );
        assert!(message.contains("[REDACTED]"));
    }

    #[test]
    fn redact_replaces_secret_in_arbitrary_text() {
        set_test_env("SC_TEST_REDACT_UNIT_KEY", "unit-test-secret-abc123");
        let client = OpenAiCompatClient::new(OpenAiCompatConfig::new(
            "http://127.0.0.1:1",
            "SC_TEST_REDACT_UNIT_KEY",
        ))
        .unwrap();

        let redacted = client.redact("token=unit-test-secret-abc123;rest");

        assert_eq!(redacted, "token=[REDACTED];rest");
    }

    #[test]
    fn debug_output_never_contains_the_secret() {
        set_test_env("SC_TEST_DEBUG_REDACT_KEY", "debug-secret-should-not-appear");
        let client = OpenAiCompatClient::new(OpenAiCompatConfig::new(
            "http://127.0.0.1:1",
            "SC_TEST_DEBUG_REDACT_KEY",
        ))
        .unwrap();

        let debug_text = format!("{client:?}");

        assert!(!debug_text.contains("debug-secret-should-not-appear"));
    }

    #[tokio::test]
    async fn missing_key_errors_before_any_network_call() {
        remove_test_env("SC_TEST_MISSING_KEY_NEVER_SET");
        let client = OpenAiCompatClient::new(OpenAiCompatConfig::new(
            "http://127.0.0.1:1",
            "SC_TEST_MISSING_KEY_NEVER_SET",
        ))
        .unwrap();

        assert!(!client.has_key());
        let err = client.list_models().await.unwrap_err();
        assert!(matches!(err, ProviderError::Auth(_)));
    }

    // ── refuse to attach a credential to a non-https, non-local URL ───

    #[test]
    fn rejects_non_https_remote_base_url_when_key_is_present() {
        set_test_env("SC_TEST_INSECURE_URL_KEY", "fake-insecure-secret");
        let err = OpenAiCompatClient::new(OpenAiCompatConfig::new(
            "http://example.com/v1",
            "SC_TEST_INSECURE_URL_KEY",
        ))
        .unwrap_err();

        assert!(matches!(err, ProviderError::Config(_)));
    }

    #[test]
    fn allows_non_https_remote_base_url_when_no_key_is_present() {
        remove_test_env("SC_TEST_NO_KEY_INSECURE_URL");
        let client = OpenAiCompatClient::new(OpenAiCompatConfig::new(
            "http://example.com/v1",
            "SC_TEST_NO_KEY_INSECURE_URL",
        ))
        .unwrap();

        assert!(!client.has_key());
    }

    #[test]
    fn allows_http_localhost_when_key_is_present() {
        set_test_env("SC_TEST_LOCALHOST_URL_KEY", "fake-localhost-secret");
        let client = OpenAiCompatClient::new(OpenAiCompatConfig::new(
            "http://localhost:11434/v1",
            "SC_TEST_LOCALHOST_URL_KEY",
        ))
        .unwrap();

        assert!(client.has_key());
    }

    #[test]
    fn allows_http_loopback_ip_when_key_is_present() {
        set_test_env("SC_TEST_LOOPBACK_URL_KEY", "fake-loopback-secret");
        let client = OpenAiCompatClient::new(OpenAiCompatConfig::new(
            "http://127.0.0.1:8080/v1",
            "SC_TEST_LOOPBACK_URL_KEY",
        ))
        .unwrap();

        assert!(client.has_key());
    }

    #[test]
    fn allows_https_remote_base_url_when_key_is_present() {
        set_test_env("SC_TEST_HTTPS_URL_KEY", "fake-https-secret");
        let client = OpenAiCompatClient::new(OpenAiCompatConfig::new(
            "https://api.example.com/v1",
            "SC_TEST_HTTPS_URL_KEY",
        ))
        .unwrap();

        assert!(client.has_key());
    }

    #[test]
    fn outcome_to_stream_events_emits_text_then_end() {
        let outcome = ChatCompletionOutcome {
            message: ChatMessage {
                role: "assistant".into(),
                content: Some("hi there".into()),
                name: None,
                tool_calls: None,
                tool_call_id: None,
            },
            finish_reason: Some("stop".into()),
        };

        let events: Vec<StreamEvent> = outcome_to_stream_events(outcome)
            .into_iter()
            .map(|e| e.unwrap())
            .collect();

        assert_eq!(
            events,
            vec![
                StreamEvent::TextDelta {
                    text: "hi there".into()
                },
                StreamEvent::End {
                    finish_reason: Some("stop".into())
                },
            ]
        );
    }

    #[test]
    fn outcome_to_stream_events_emits_tool_use_then_end() {
        let outcome = ChatCompletionOutcome {
            message: ChatMessage {
                role: "assistant".into(),
                content: None,
                name: None,
                tool_calls: Some(vec![crate::ToolCall {
                    id: "call_1".into(),
                    kind: "function".into(),
                    function: crate::ToolCallFunction {
                        name: "get_weather".into(),
                        arguments: r#"{"city":"Berlin"}"#.into(),
                    },
                }]),
                tool_call_id: None,
            },
            finish_reason: Some("tool_calls".into()),
        };

        let events: Vec<StreamEvent> = outcome_to_stream_events(outcome)
            .into_iter()
            .map(|e| e.unwrap())
            .collect();

        assert_eq!(
            events,
            vec![
                StreamEvent::ToolUse {
                    id: "call_1".into(),
                    name: "get_weather".into(),
                    input: serde_json::json!({"city": "Berlin"}),
                },
                StreamEvent::End {
                    finish_reason: Some("tool_calls".into())
                },
            ]
        );
    }

    // ── true incremental SSE streaming over a fragmented HTTP mock ────

    /// Chops `bytes` into fixed-size pieces, deliberately ignoring line,
    /// JSON, and UTF-8 character boundaries - proving the client has to
    /// handle fragments that split frames and multi-byte characters
    /// mid-way.
    fn fragment_bytes(bytes: &[u8], piece_len: usize) -> Vec<Vec<u8>> {
        bytes.chunks(piece_len.max(1)).map(<[u8]>::to_vec).collect()
    }

    #[tokio::test]
    async fn streaming_reassembles_fragmented_frames_utf8_and_tool_calls() {
        // Deliberately includes: a keepalive comment line, a tool call
        // whose id/name arrive in one delta and whose arguments arrive in
        // a later delta, a text delta containing a multi-byte UTF-8
        // character, a finish_reason chunk, and the [DONE] marker.
        let body = concat!(
            ": keepalive\n\n",
            "data: {\"choices\":[{\"delta\":{\"tool_calls\":[{\"index\":0,\"id\":\"call_1\",\"function\":{\"name\":\"get_weather\",\"arguments\":\"\"}}]}}]}\n\n",
            "data: {\"choices\":[{\"delta\":{\"tool_calls\":[{\"index\":0,\"function\":{\"arguments\":\"{\\\"city\\\":\\\"Berlin\\\"}\"}}]}}]}\n\n",
            "data: {\"choices\":[{\"delta\":{\"content\":\"caf\u{00e9} done\"}}]}\n\n",
            "data: {\"choices\":[{\"delta\":{},\"finish_reason\":\"tool_calls\"}]}\n\n",
            "data: [DONE]\n\n",
        );
        let bytes = body.as_bytes();
        // 13 is coprime with essentially every structural token length in
        // the payload above, so fixed-size chopping reliably lands inside
        // the multi-byte UTF-8 sequence for 'é' and mid-JSON in several
        // places without needing to hand-pick offsets.
        let fragments = fragment_bytes(bytes, 13);
        assert!(
            fragments.len() > 5,
            "expected the payload to actually be split into several pieces"
        );

        let server = start_fragmented_sse_server(fragments, Duration::from_millis(0)).await;
        set_test_env("SC_TEST_STREAM_KEY", "fake-stream-secret");
        let client =
            OpenAiCompatClient::new(config_for(&server.base_url, "SC_TEST_STREAM_KEY")).unwrap();

        let stream = client
            .chat_completion_stream(&sample_request())
            .await
            .unwrap();
        let events: Vec<StreamEvent> = stream.map(|e| e.unwrap()).collect().await;

        assert_eq!(
            events,
            vec![
                StreamEvent::TextDelta {
                    text: "café done".into()
                },
                StreamEvent::ToolUse {
                    id: "call_1".into(),
                    name: "get_weather".into(),
                    input: serde_json::json!({"city": "Berlin"}),
                },
                StreamEvent::End {
                    finish_reason: Some("tool_calls".into())
                },
                StreamEvent::End {
                    finish_reason: None
                },
            ]
        );
    }

    #[tokio::test]
    async fn streaming_malformed_frame_over_http_yields_a_typed_error() {
        let fragments = fragment_bytes(b"data: { not valid json at all\n\n", 5);
        let server = start_fragmented_sse_server(fragments, Duration::from_millis(0)).await;
        set_test_env(
            "SC_TEST_STREAM_MALFORMED_KEY",
            "fake-stream-malformed-secret",
        );
        let client =
            OpenAiCompatClient::new(config_for(&server.base_url, "SC_TEST_STREAM_MALFORMED_KEY"))
                .unwrap();

        let mut stream = client
            .chat_completion_stream(&sample_request())
            .await
            .unwrap();
        let first = stream.next().await.expect("expected one error item");

        assert!(matches!(first, Err(ProviderError::Stream(_))));
    }

    #[tokio::test]
    async fn dropping_the_stream_cancels_the_underlying_request() {
        // Five fragments, each followed by a pause: enough time for the
        // client to consume the first couple of events and then drop the
        // stream well before the server would otherwise finish sending.
        let fragments = vec![
            b"data: {\"choices\":[{\"delta\":{\"content\":\"one\"}}]}\n\n".to_vec(),
            b"data: {\"choices\":[{\"delta\":{\"content\":\"two\"}}]}\n\n".to_vec(),
            b"data: {\"choices\":[{\"delta\":{\"content\":\"three\"}}]}\n\n".to_vec(),
            b"data: {\"choices\":[{\"delta\":{\"content\":\"four\"}}]}\n\n".to_vec(),
            b"data: [DONE]\n\n".to_vec(),
        ];
        let server = start_fragmented_sse_server(fragments, Duration::from_millis(80)).await;
        set_test_env("SC_TEST_STREAM_CANCEL_KEY", "fake-stream-cancel-secret");
        let client =
            OpenAiCompatClient::new(config_for(&server.base_url, "SC_TEST_STREAM_CANCEL_KEY"))
                .unwrap();

        {
            let mut stream = client
                .chat_completion_stream(&sample_request())
                .await
                .unwrap();
            let first = stream.next().await.expect("expected at least one event");
            assert_eq!(
                first.unwrap(),
                StreamEvent::TextDelta { text: "one".into() }
            );
            // `stream` (and the reqwest response body it owns) is dropped
            // here, before the remaining fragments were sent.
        }

        // Give the server ample time to have finished sending everything
        // had the client stayed connected, then confirm it did not.
        tokio::time::sleep(Duration::from_millis(500)).await;

        assert!(
            !server.fully_sent.load(Ordering::SeqCst),
            "server sent its full response even though the client dropped the stream early"
        );
        assert_eq!(server.hits.load(Ordering::SeqCst), 1);
    }
}
