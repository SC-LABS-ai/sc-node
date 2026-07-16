> **ARCHIVED / HISTORICAL — may be outdated.** Kept for context only; see
> [../PROVIDERS.md](../PROVIDERS.md) for the current provider layer. Note the
> OpenRouter adapter has since been implemented on the shared OpenAI-compatible
> client (not yet live-tested). Original follows.

---

# SC Node — Router Integrations Plan

> **Status:** Planning / Documentation only — No implementation yet
> **Date:** 2026-07-10 · **Branch:** main

---

## 1. Purpose

SC Node should support both **direct providers** (Ollama, NVIDIA NIM) and **optional upstream routers** (OpenRouter, 9Router). Routers act as an intermediate layer that can provide model aggregation, token savings, quota management, and fallback logic — while SC Node remains local-first and privacy-first by default.

---

## 2. OpenRouter

| Property | Value |
|----------|-------|
| **Type** | Direct cloud provider |
| **API Compatibility** | OpenAI-compatible (`/v1/chat/completions`, `/v1/models`) |
| **Base URL** | `https://openrouter.ai/api/v1` |
| **Auth** | Bearer token via `SC_AGENT_OPENROUTER_API_KEY` (env var only) |
| **Model Listing** | `GET /v1/models` |
| **Streaming Chat** | `POST /v1/chat/completions` with `stream=true` |
| **Status at time of writing** | Stubbed — since implemented on the shared client (not live-tested) |

### Notes
- OpenRouter is a **cloud aggregator** — provides access to many models via a single endpoint
- API key **must never** be stored in config file — environment variable only
- Not enabled by default — requires explicit `enabled = true` in config

---

## 3. 9Router

| Property | Value |
|----------|-------|
| **Type** | Optional local upstream router / proxy |
| **API Compatibility** | OpenAI-compatible |
| **Default Local Endpoint** | `http://localhost:20128/v1` |
| **Use Cases** | Token saving, fallback routing, quota tracking, multi-account management |
| **Status** | Not implemented — planned as generic OpenAI-compatible endpoint |

> 9Router is **not a provider** — it is a local proxy/router. SC Node should connect to it via a generic OpenAI-compatible provider, not a hardcoded "9Router provider", so any OpenAI-compatible local endpoint (9Router, LM Studio, LocalAI, vLLM, SGLang, llama.cpp server) works interchangeably.

---

## 4. Future Provider Architecture

```
+-----------------+-----------------+-------------------------+
|   Local-First   |   Cloud Direct  |   Generic Compatible    |
+-----------------+-----------------+-------------------------+
| ollama          | nvidia          | openrouter              |
|                 |                 | openai_compatible       |
|                 |                 |  - 9Router (local)      |
|                 |                 |  - LM Studio            |
|                 |                 |  - vLLM / SGLang        |
|                 |                 |  - llama.cpp server     |
+-----------------+-----------------+-------------------------+
```

---

## 5. Security

| Rule | Enforcement |
|------|-------------|
| **API keys only via env vars** | Config parser drops keys in TOML; env var only |
| **No cloud routing by default** | All cloud providers `enabled = false` in default config |
| **Local-first default** | Ollama enabled by default; all cloud providers disabled |
| **Secrets never in audit logs** | Audit logger scrubs API keys / Authorization headers |

---

## 6. Roadmap

| Phase | Scope | Status |
|-------|-------|--------|
| Phase 1 | Documentation only (this file) | Done |
| Phase 2 | Implement OpenRouter provider (real HTTP, streaming, model listing) | Implemented (not live-tested) |
| Phase 3 | Generic `openai_compatible` provider (9Router, LM Studio, vLLM, etc.) | Planned |
| Phase 4 | Router profile config + CLI | Planned |
| Phase 5 | Optional token-saver hooks / compression strategy | Future |

---

## 7. References

- OpenRouter API: https://openrouter.ai/docs
- OpenAI API Spec: https://platform.openai.com/docs/api-reference
- SC Node Provider Architecture: [../PROVIDERS.md](../PROVIDERS.md)
- Security Model: [../SECURITY_MODEL.md](../SECURITY_MODEL.md)
