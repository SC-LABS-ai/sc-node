# SC Node — Roadmap

> **As of:** 2026-08-05 · Experimental public alpha.

This roadmap describes direction, not commitments. **No dates are promised**, and
priorities may change based on feedback. Items are roughly ordered by near-term
intent.

## Near-term

- **Stabilize the public Rust API.** The workspace crates are usable as path
  dependencies today, but the API is unstable and unpublished. The goal is a
  documented, semver-respecting surface for at least the core traits
  (`Provider`, `Tool`) and the pure modules (routing, contracts, proof).
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

## Recently completed

- **Reproducible tool-using examples.** Added an isolated multi-round file/shell
  walkthrough, approval-gate session, audit inspection, denied-call scenarios,
  and deterministic guard checks.
- **Linux verification.** Added a separate Ubuntu CI gate, committed `Cargo.lock`
  for reproducible `--locked` builds, made the smoke harness cross-platform, and
  split Windows NTFS from POSIX path semantics with dedicated tests.

## Later / under consideration

- Verify the full gate on macOS and document any platform differences.
- A generic `openai_compatible` provider for local endpoints (LM Studio, vLLM,
  llama.cpp server, etc.).
- Wiring contracts and proof bundles into the run loop end-to-end (not just the
  standalone `contract`/`proof` subcommands).
- Process containment and resource limits (Windows Job Objects, Linux
  cgroups/ulimits).
- Incremental streaming for the Ollama provider.
- `cargo audit` / `cargo deny` in CI.

Progress against these items is reflected in [STATUS.md](STATUS.md).
