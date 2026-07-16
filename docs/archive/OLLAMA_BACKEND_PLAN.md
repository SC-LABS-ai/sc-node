> **ARCHIVED / HISTORICAL — may be outdated.** Kept for context only; the Ollama
> provider is implemented — see [../PROVIDERS.md](../PROVIDERS.md) and
> [../STATUS.md](../STATUS.md). Original follows.

---

# SC Node — Ollama Backend Implementation Plan

> **Status:** IMPLEMENTED — Ollama provider is real.
> **Planning doc preserved for reference.**

## What Was Implemented

| Feature | Status | Notes |
|---------|--------|-------|
| `health_check()` | Done | GET /api/tags, 5s timeout, returns `Ok(true)` on 2xx, `Ok(false)` on error |
| `list_models()` | Done | GET /api/tags, parses JSON, maps family to context window (llama=8192, mistral=32000, etc.) |
| `complete()` streaming | Done | POST /api/chat, streams text deltas. Parses tool calls when emitted. |
| Error handling | Done | Timeout/connection refused -> descriptive Network error. HTTP non-2xx -> Api error. |
| Unit tests | Done | Chunk parsing (text, done, empty, tool call, malformed, empty line), model mapping, request serialization, tags response parsing |

## What Was Not Implemented (Deferred)

| Feature | Reason |
|---------|--------|
| Incremental streaming | Current approach collects full body then parses. MVP-simple. Upgrade in future sprint. |
| Model pull / management | Out of scope. User manages Ollama separately. |
| Embeddings / multimodal | Out of alpha scope. |

## Post-Implementation Notes

### What Changed From Plan
- Streaming approach: plan used `StreamReader` + `BufReader.lines()` + `filter_map`; implementation uses simpler `resp.text()` + `lines().filter_map(parse)` + `futures::stream::iter()`. Simpler, compiles clean, same result for MVP.
- No `tokio-util` dependency needed (was in plan, removed in implementation).
- Tool calls supported (parsed from Ollama JSON when present) — bonus beyond basic text plan.

### How To Use

```bash
ollama serve &
ollama pull llama3.2:3b
cargo run -- models-list
cargo run -- run "What is the capital of France?"
cargo run -- doctor
```
