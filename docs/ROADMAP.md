# SC Node — Roadmap

> **As of:** 2026-08-05 · Experimental public alpha.

This roadmap describes direction, not commitments. **No dates are promised**, and
priorities may change based on feedback. Items are roughly ordered by near-term
intent.

## Near-term

- **Wire `sc-memory` into the runtime.** The memory/RAG crate exists but is not
  constructed by the binary. Integrate it behind an explicit, opt-in config
  section and a `memory` feature.
- **Complete authenticated OpenRouter verification.** Provider-specific catalog
  metadata and deterministic tests are complete; run the explicit adapter-level
  catalog + streaming completion gate when a real key is available.

## Recently completed

- **Provider direction.** Chose to harden OpenRouter before adding another
  adapter, replaced guessed model capabilities with catalog metadata, and added
  honest public-catalog/authenticated-live verification gates.
- **Public Rust API baseline.** Classified intentional alpha surfaces, added a
  separate external consumer crate, made Rustdoc warnings fatal, documented
  SemVer intent and publication waves, and added reproducible package gates.
- **First reproducible overhead benchmark.** Added a locked benchmark helper,
  deterministic Ollama-compatible fixture, real local-model warm/cold pairs,
  process CPU/RAM capture, routing/permission/audit/proof microbenchmarks, raw
  evidence, and a configuration-specific interpretation without a generic
  framework-speed claim.
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
