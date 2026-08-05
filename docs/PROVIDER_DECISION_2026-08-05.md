# Provider direction decision — 2026-08-05

## Decision

Harden and finish verification of the existing OpenRouter adapter before adding a fourth provider adapter.

## Why

- Ollama already covers the local-first path.
- NVIDIA NIM already covers one verified cloud path.
- OpenRouter is already integrated into configuration, routing, and the shared OpenAI-compatible transport, but its provider-specific metadata mapping and live verification were incomplete.
- Adding another adapter now would increase maintenance surface while leaving an existing supported route partially verified.

## Work completed in this decision

- Preserve provider-reported model names, context lengths, and supported parameters.
- Derive tool support from the live catalog rather than assuming every model supports tools.
- Add deterministic catalog-shape and capability tests.
- Add a manual public-catalog verification gate that checks the configured default and research models.
- Replace the previous no-key live-test no-op with an explicitly ignored authenticated catalog + streaming completion test.

## Verification status

- Deterministic adapter tests: required in normal CI.
- Public OpenRouter catalog/schema and configured model IDs: manually verified without credentials.
- Authenticated completion through the Rust adapter: remains **SKIP** until `SC_AGENT_OPENROUTER_API_KEY` is supplied and `scripts/verify-openrouter.ps1` is run.

## Next provider after OpenRouter

After authenticated OpenRouter verification, the preferred next expansion is a generic OpenAI-compatible local/server adapter for endpoints such as LM Studio, vLLM, and llama.cpp server. That provides broader value than another single-vendor adapter and can reuse the hardened shared transport.

No new provider is added by this decision, and no credentials are stored in the repository.
