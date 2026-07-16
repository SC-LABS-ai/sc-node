# SC Node — Providers

> **As of:** 2026-07-16 · Experimental public alpha.

SC Node is provider-neutral. A provider is any implementation of the
`sc_provider_core::Provider` trait; the binary constructs whichever providers are
`enabled = true` in the config and hands them to the deterministic router.

## Summary

| Provider | Kind | Default | Streaming | Credential | Status |
|----------|------|---------|-----------|------------|--------|
| Ollama | Local | Enabled | Batch-collect | None (local) | Live-tested |
| NVIDIA NIM | Cloud | Disabled | Incremental SSE | `SC_AGENT_NVIDIA_API_KEY` | Live-tested |
| OpenRouter | Cloud | Disabled | Incremental SSE | `SC_AGENT_OPENROUTER_API_KEY` | Implemented, not live-tested |

Cloud providers are opt-in. Enabling at least one cloud provider is the explicit
cloud opt-in that lets the router select a cloud target; there is no silent cloud
fallback.

## Ollama (live-tested)

- **Endpoint:** `http://localhost:11434` by default; needs no API key.
- **Enabled by default.** Talks directly to the Ollama HTTP API
  (`GET /api/tags`, `POST /api/chat`).
- **Streaming:** currently collects the full response body and then emits events
  (batch-collect), rather than decoding incrementally.
- Tool calls emitted by the model are parsed into `ToolUse` events.

```toml
[providers.ollama]
enabled = true
base_url = "http://localhost:11434"
default_model = "llama3.2:3b"
keep_alive = "5m"
timeout_secs = 120
```

## NVIDIA NIM (live-tested)

- **Endpoint:** `https://integrate.api.nvidia.com/v1` by default.
- **Opt-in.** Set `enabled = true` and provide the key via the
  `SC_AGENT_NVIDIA_API_KEY` environment variable — never in the config file.
- Built on the shared OpenAI-compatible client (see below): incremental SSE
  streaming, bounded retries, secret redaction, HTTPS enforcement.

```toml
[providers.nvidia]
enabled = true
base_url = "https://integrate.api.nvidia.com/v1"
default_model = "meta/llama-3.3-70b-instruct"
timeout_secs = 60
max_retries = 3
```

```bash
# key via environment only
export SC_AGENT_NVIDIA_API_KEY="<your key>"   # PowerShell: $env:SC_AGENT_NVIDIA_API_KEY = "<your key>"
```

## OpenAI-compatible layer

`sc_provider_core::openai_compat::OpenAiCompatClient` is the reusable client that
cloud adapters share. It normalizes the parts of the OpenAI-compatible protocol
that differ or trip up strict endpoints:

- **Tools serialization.** Tool definitions are serialized in the
  `{"type":"function","function":{name,description,parameters}}` envelope that
  strict endpoints (NVIDIA NIM in particular) require; sending the bare
  `{name,description,parameters}` shape is rejected with HTTP 400.
- **System-parameter folding.** SC Node's top-level `system` prompt is folded
  into a leading `{"role":"system"}` message, because some endpoints reject a
  top-level `system` parameter.
- **Incremental SSE.** The `chat/completions` response body is decoded as
  Server-Sent Events as bytes arrive (via `sse::SseDecoder`) rather than being
  buffered whole; text deltas, tool-call deltas, and `[DONE]` are handled, and
  dropping the stream cancels the underlying request.
- **Retry & errors.** 401/403/429 are never retried; 5xx and transport
  timeouts/connect failures are retried up to a bounded limit; a failure is never
  turned into a fabricated success; secrets are redacted from error text.
- **Credential safety.** The client refuses to attach a credential to a non-`https`
  base URL unless it points at a local/loopback host.

## OpenRouter (implemented, not live-tested)

- **Endpoint:** `https://openrouter.ai/api/v1` by default.
- **Opt-in.** Set `enabled = true` and provide the key via
  `SC_AGENT_OPENROUTER_API_KEY`.
- Uses the same shared OpenAI-compatible client as NVIDIA NIM (real
  `list_models` and streaming `complete`). It has **not been live-tested**, so its
  status is not promoted beyond "implemented" — treat behaviour against the live
  endpoint as unverified and report issues.

```toml
[providers.openrouter]
enabled = false
base_url = "https://openrouter.ai/api/v1"
default_model = "openai/gpt-4.1-mini"
timeout_secs = 60
max_retries = 3
```

## Adding a provider

Implement the `sc_provider_core::Provider` trait:

```rust
#[async_trait]
pub trait Provider: Send + Sync {
    fn key(&self) -> &str;   // stable id, e.g. "ollama"
    fn name(&self) -> &str;  // human-readable label

    async fn list_models(&self) -> Result<Vec<ModelInfo>>;
    async fn complete(&self, request: CompletionRequest) -> Result<EventStream>;
    async fn health_check(&self) -> Result<bool> { Ok(true) } // default

    fn info(&self) -> ProviderInfo { /* default */ }
}
```

- For any OpenAI-compatible endpoint, build on `OpenAiCompatClient` — you get the
  serialization, SSE, retry, and credential-safety behaviour above for free (see
  `sc-provider-nvidia` / `sc-provider-openrouter` as references, which are thin
  wrappers that only add model-metadata mapping).
- `complete` returns a stream of `StreamEvent`s (`TextDelta`, `ToolUse`, `End`,
  `Error`). An empty model slug means "use the provider's own default model."
- Register the provider in `src/main.rs` and, if it is a cloud provider, ensure
  the router treats it as non-local so it stays behind the cloud opt-in.

See [ARCHITECTURE.md](ARCHITECTURE.md) §3–§4 for how providers plug into routing
and the run loop.
