# SC Node — Roadmap

> **As of:** 2026-07-16 · Experimental public alpha.

This roadmap describes direction, not commitments. **No dates are promised**, and
priorities may change based on feedback. Items are roughly ordered by near-term
intent.

## Near-term

- **Stabilize the public Rust API.** The workspace crates are usable as path
  dependencies today, but the API is unstable and unpublished. The goal is a
  documented, semver-respecting surface for at least the core traits
  (`Provider`, `Tool`) and the pure modules (routing, contracts, proof).
- **Linux verification.** SC Node is only exercised on Windows so far. Verify the
  build, tests, and CLI on Linux, then document any platform differences.
- **Benchmark methodology and first reproducible numbers.** Publish the
  methodology in [BENCHMARKING.md](BENCHMARKING.md) and then produce a first set
  of reproducible measurements (direct-provider-call vs through-SC-Node, routing
  and tool-dispatch overhead). No performance claims will be made before numbers
  exist.
- **Wire `sc-memory` into the runtime.** The memory/RAG crate exists but is not
  constructed by the binary. Integrate it behind an explicit, opt-in config
  section and a `memory` feature.
- **Complete the OpenRouter adapter.** The adapter is implemented on the shared
  OpenAI-compatible client but has not been live-tested; validate it against the
  live endpoint and promote its status once verified.
- **Improve tool-using examples.** Expand `examples/` with clearer,
  reproducible tool-agent walkthroughs (Ollama, NVIDIA NIM, and a generic
  OpenAI-compatible endpoint).

## Later / under consideration

- A generic `openai_compatible` provider for local endpoints (LM Studio, vLLM,
  llama.cpp server, etc.).
- Wiring contracts and proof bundles into the run loop end-to-end (not just the
  standalone `contract`/`proof` subcommands).
- Process containment and resource limits (Windows Job Objects, Linux
  cgroups/ulimits).
- Incremental streaming for the Ollama provider.
- `cargo audit` / `cargo deny` in CI.

Progress against these items is reflected in [STATUS.md](STATUS.md).
