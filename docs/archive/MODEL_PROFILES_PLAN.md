> **ARCHIVED / HISTORICAL — may be outdated.** Kept for context only; see
> [../ROADMAP.md](../ROADMAP.md) and [../PROVIDERS.md](../PROVIDERS.md) for current
> direction. Original follows.

---

# SC Node — Model Profiles Plan

> **Status:** Evaluation / Planning — No implementation yet
> **Date:** 2026-07-10 · **Branch:** main

---

## 1. Purpose

SC Node needs a structured way to configure and use **coding-agent models** across different providers (local and OpenAI-compatible). A model profile system will allow users to:

- Switch between models optimized for coding tasks
- Define per-model capabilities (context window, tool calls, reasoning)
- Set optimal sampling defaults per model
- Distinguish local vs cloud models at a glance

This plan defines the profile schema, candidate models, and integration path — **no implementation yet**.

---

## 2. Candidate Models

| Model | Provider | Local/Cloud | Notes |
|-------|----------|-------------|-------|
| **qwen2.5-coder** | Ollama / OpenAI-compatible | Local / Cloud | Strong coding benchmarks, multiple sizes |
| **deepseek-coder** | Ollama / OpenAI-compatible / NVIDIA NIM | Local / Cloud | Strong reasoning, tool calls |
| **codellama** | Ollama / OpenAI-compatible | Local | Meta's coding model, multiple sizes |
| **starcoder2** | Ollama / OpenAI-compatible | Local | BigCode, permissive license |
| **Ornith-1** | OpenAI-compatible (vLLM/SGLang) / GGUF | Cloud / Local | Agentic coding model family |
| **nemotron-3-ultra** | NVIDIA NIM | Cloud | NVIDIA flagship, strong coding |
| **llama-3.1-70b** | NVIDIA NIM / Ollama | Cloud / Local | General + coding, large context |

> **Note:** Model availability varies by provider. SC Node should not hardcode model names — use profiles.

---

## 3. Ornith-1 Notes

| Aspect | Detail |
|--------|--------|
| **Family** | Agentic coding model family (Ornith) |
| **Serving** | OpenAI-compatible via vLLM / SGLang |
| **Local variants** | GGUF quantizations for llama.cpp / Ollama-style inference |
| **Reasoning** | Supports reasoning-style output (chain-of-thought) |
| **Tool calls** | Tool-call parsing relevant for SC Node's future agent loop |
| **Verification** | **Requires live testing before implementation** — no live testing done yet |

> **Caution:** Ornith-1 support is **planned only**. No live validation of tool-call format, reasoning output, or SC Node compatibility has been performed. Treat as candidate only.

---

## 4. Future Config Sketch

```toml
# ~/.sc-agent/config.toml (future)
[model_profiles]
[model_profiles.qwen2_5_coder]
name = "qwen2.5-coder:7b"
provider = "ollama"
base_url = "http://localhost:11434"
context_window = 32768
tool_call_support = true
reasoning_content_support = false
sampling = { temperature = 0.1, top_p = 0.95 }
local = true

[model_profiles.nemotron_3_ultra]
name = "nvidia/nemotron-3-ultra"
provider = "nvidia"
base_url = "https://integrate.api.nvidia.com/v1"
context_window = 8192
tool_call_support = true
reasoning_content_support = false
sampling = { temperature = 0.2, top_p = 0.95 }
local = false
```

### Profile Fields

| Field | Type | Required | Description |
|-------|------|----------|-------------|
| `name` | String | Yes | Model identifier as known by provider |
| `provider` | String | Yes | `ollama`, `nvidia`, `openai_compatible`, `openrouter` |
| `base_url` | String | Yes | Provider endpoint |
| `context_window` | Integer | Yes | Max tokens (input + output) |
| `tool_call_support` | Boolean | Yes | Provider/model supports tool calls |
| `reasoning_content_support` | Boolean | No (default false) | Model emits reasoning content |
| `sampling` | Table | No | Default temperature, top_p, top_k |
| `local` | Boolean | Yes | True = local inference (no API key) |

---

## 5. Exclusions

| Exclusion | Reason |
|-----------|--------|
| **Hardcoded model dependency** | SC Node must remain provider/model agnostic |
| **Provider rewrite** | No changes to provider abstractions in this plan |
| **Benchmark claims** | No performance claims without local verification |
| **Production support promise** | Experimental alpha only |
| **Auto model selection** | Not in scope — manual profile selection only |
| **Model download/management** | Out of scope (user manages Ollama/vLLM) |

---

## 6. Integration Path (Future)

| Phase | Work |
|-------|------|
| 1 | Add `model_profiles` section to `Config` (optional, feature-gated) |
| 2 | Extend `sc-provider-core` with `ModelProfile` trait |
| 3 | Add `sc-agent models-list --profiles` CLI |
| 4 | Route `sc-agent run` to use profile by name |
| 5 | Document profile setup for each candidate model |

---

## 7. Verification Checklist (Before Any Implementation)

| Item | Status |
|------|--------|
| Ornith-1 tool-call format verified with SC Node | Not verified |
| Ornith-1 reasoning output format verified | Not verified |
| Qwen2.5-Coder tool calls via Ollama verified | Partial (Ollama supports) |
| DeepSeek Coder tool calls via NIM verified | Partial (NIM supports) |
| All profiles load without errors | Not implemented |
| Profile routing works with `sc-agent run` | Not implemented |

## Decision Log

| Date | Decision |
|------|----------|
| 2026-07-10 | Model profiles plan created with Ornith-1 as candidate. |
